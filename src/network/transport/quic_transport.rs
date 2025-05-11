// Network Transport Module
//
// This module defines the network transport interfaces and implementations.

// Standard library imports
use std::collections::HashMap;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use std::time::Instant;

// External crate imports
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use quinn::{ClientConfig, ConnectionError, Endpoint, ServerConfig, TransportConfig};
// use runar_common::types::{ SerializerRegistry};
use rustls::{Certificate, PrivateKey, ServerName};
use tokio::sync::oneshot;
use tokio::sync::{mpsc, Mutex as TokioMutex, RwLock as TokioRwLock};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid;

// Internal module imports
use super::peer_registry::{PeerEntry, PeerRegistry, PeerStatus};
use super::{
    ConnectionCallback, MessageHandler, NetworkError, NetworkMessage,
    NetworkMessagePayloadItem, NetworkTransport, PeerId,
};
use crate::network::discovery::multicast_discovery::PeerInfo;
use crate::network::discovery::{NodeDiscovery, NodeInfo};
use crate::node::NetworkConfig;
use runar_common::Logger;

// Re-export pick_free_port from parent module for backward compatibility
pub use super::pick_free_port;

// QUIC Transport Implementation
//
// INTENTION: Implement the NetworkTransport trait using high-performance,
// secure, multiplexed communication between nodes. QUIC provides transport
// security, connection migration, and reliable messaging.

/// Holds the state for a connection to a specific peer.
#[derive(Debug)]
struct PeerState {
    peer_id: PeerId,
    address: String,
    connection: quinn::Connection,
    last_used: Instant,
    // Pool of reusable, idle streams. Mutex for interior mutability.
    idle_streams: TokioMutex<VecDeque<(quinn::SendStream, quinn::RecvStream, Instant)>>,
    // Add more metrics later if needed for dynamic policy
}

impl PeerState {
    /// Removes streams from the pool that have been idle longer than the timeout.
    async fn cleanup_idle_streams(&self, stream_idle_timeout: Duration) -> usize {
        let mut guard = self.idle_streams.lock().await;
        let now = Instant::now();
        let initial_len = guard.len();
        guard.retain(|(_, _, idle_since)| now.duration_since(*idle_since) <= stream_idle_timeout);
        initial_len - guard.len() // Return number of streams closed
    }
}

/// QUIC-specific transport options
#[derive(Debug, Clone)]
pub struct QuicTransportOptions {
    /// TLS certificate chain (in DER format)
    pub certificates: Option<Vec<Certificate>>,
    /// TLS private key (in DER format)
    pub private_key: Option<PrivateKey>,
    /// Path to the TLS certificate file
    pub cert_path: Option<String>,
    /// Path to the TLS private key file
    pub key_path: Option<String>,
    /// Whether to verify certificates (disable for development)
    pub verify_certificates: bool,
    /// Keep-alive interval in milliseconds
    pub keep_alive_interval_ms: u64,
    /// Maximum number of concurrent bi-directional streams per connection
    pub max_concurrent_bidi_streams: u32,
    /// Timeout in milliseconds for closing idle connections
    pub connection_idle_timeout_ms: u64,
    /// Timeout in milliseconds for closing idle streams within a connection pool
    pub stream_idle_timeout_ms: u64,
    /// Maximum number of idle streams to keep open per peer connection
    pub max_idle_streams_per_peer: usize,
}

impl QuicTransportOptions {
    /// Create a new instance with default settings and a randomly selected port
    pub fn new() -> Self {
        Self {
            certificates: None,
            private_key: None,
            cert_path: None,
            key_path: None,
            verify_certificates: true,
            keep_alive_interval_ms: 5000,
            max_concurrent_bidi_streams: 100,
            connection_idle_timeout_ms: 60000,
            stream_idle_timeout_ms: 30000,
            max_idle_streams_per_peer: 10,
        }
    }

    /// Set TLS certificate chain directly
    pub fn with_certificates(mut self, certs: Vec<Certificate>) -> Self {
        self.certificates = Some(certs);
        self
    }

    /// Set TLS private key directly
    pub fn with_private_key(mut self, key: PrivateKey) -> Self {
        self.private_key = Some(key);
        self
    }

    /// Set TLS certificate path
    pub fn with_cert_path(mut self, path: String) -> Self {
        self.cert_path = Some(path);
        self
    }

    /// Set TLS private key path
    pub fn with_key_path(mut self, path: String) -> Self {
        self.key_path = Some(path);
        self
    }

    /// Set whether to verify certificates
    pub fn with_verify_certificates(mut self, verify: bool) -> Self {
        self.verify_certificates = verify;
        self
    }

    /// Set keep-alive interval
    pub fn with_keep_alive_interval(mut self, interval_ms: u64) -> Self {
        self.keep_alive_interval_ms = interval_ms;
        self
    }

    /// Set maximum number of concurrent bi-directional streams
    pub fn with_max_concurrent_bidi_streams(mut self, max: u32) -> Self {
        self.max_concurrent_bidi_streams = max;
        self
    }

    /// Set connection idle timeout
    pub fn with_connection_idle_timeout(mut self, timeout_ms: u64) -> Self {
        self.connection_idle_timeout_ms = timeout_ms;
        self
    }

    /// Set stream idle timeout
    pub fn with_stream_idle_timeout(mut self, timeout_ms: u64) -> Self {
        self.stream_idle_timeout_ms = timeout_ms;
        self
    }

    /// Set maximum number of idle streams per peer
    pub fn with_max_idle_streams_per_peer(mut self, max: usize) -> Self {
        self.max_idle_streams_per_peer = max;
        self
    }

    /// Get certificates and key from configuration
    fn get_certificates_and_key(&self, logger: &Logger) -> Result<(Vec<Certificate>, PrivateKey)> {
        // First check if certificates are provided directly in memory
        if let (Some(certs), Some(key)) = (&self.certificates, &self.private_key) {
            return Ok((certs.clone(), key.clone()));
        }

        // Otherwise try to load from files
        if let (Some(cert_path), Some(key_path)) = (&self.cert_path, &self.key_path) {
            // Read certificate file
            let cert_file = std::fs::read(cert_path)?;
            let cert_chain = rustls_pemfile::certs(&mut cert_file.as_slice())?
                .into_iter()
                .map(Certificate)
                .collect::<Vec<_>>();

            // Read private key file
            let key_file = std::fs::read(key_path)?;
            let mut keys = rustls_pemfile::pkcs8_private_keys(&mut key_file.as_slice())?;

            if cert_chain.is_empty() {
                return Err(anyhow!("No certificates found in certificate file"));
            }

            if keys.is_empty() {
                return Err(anyhow!("No private keys found in key file"));
            }

            let key = PrivateKey(keys.remove(0));
            return Ok((cert_chain, key));
        }

        // If we get here, no certificates were provided
        logger.error("No TLS certificates provided. QUIC requires TLS certificates.");
        Err(anyhow!("No TLS certificates provided"))
    }

    /// Create server configuration for QUIC
    pub fn create_server_config(&self, logger: &Logger) -> Result<ServerConfig> {
        // Create transport config based on our options, not defaults
        let mut transport_config = TransportConfig::default();

        // Apply all settings from our transport options
        transport_config
            .keep_alive_interval(Some(Duration::from_millis(self.keep_alive_interval_ms)));

        transport_config.max_concurrent_bidi_streams(self.max_concurrent_bidi_streams.into());

        // Set connection timeout based on our options
        transport_config.max_idle_timeout(Some(
            Duration::from_millis(self.connection_idle_timeout_ms)
                .try_into()
                .unwrap_or_else(|_| {
                    logger.warn(format!(
                        "Connection idle timeout {} ms is too large, using maximum allowed",
                        self.connection_idle_timeout_ms
                    ));
                    // Use a very large timeout instead of MAX which doesn't exist
                    std::time::Duration::from_secs(u64::MAX / 1000)
                        .try_into()
                        .unwrap_or_default()
                }),
        ));

        // Load certificates from memory or file
        let (certs, key) = self.get_certificates_and_key(logger)?;

        // Create server configuration (will fail if no valid certificates are provided)
        let mut server_config = ServerConfig::with_single_cert(certs, key)?;
        server_config.transport = Arc::new(transport_config);

        Ok(server_config)
    }

    /// Create client configuration for QUIC
    pub fn create_client_config(&self, logger: &Logger) -> Result<ClientConfig> {
        // Create transport config based on our options, not defaults
        let mut transport_config = TransportConfig::default();

        // Apply all settings from our transport options
        transport_config
            .keep_alive_interval(Some(Duration::from_millis(self.keep_alive_interval_ms)));

        transport_config.max_concurrent_bidi_streams(self.max_concurrent_bidi_streams.into());

        // Set connection timeout based on our options
        transport_config.max_idle_timeout(Some(
            Duration::from_millis(self.connection_idle_timeout_ms)
                .try_into()
                .unwrap_or_else(|_| {
                    logger.warn(format!(
                        "Connection idle timeout {} ms is too large, using maximum allowed",
                        self.connection_idle_timeout_ms
                    ));
                    // Use a very large timeout instead of MAX which doesn't exist
                    std::time::Duration::from_secs(u64::MAX / 1000)
                        .try_into()
                        .unwrap_or_default()
                }),
        ));

        let shared_transport_config = Arc::new(transport_config);

        // Create client configuration - using ClientConfig builder
        let mut crypto_config = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth();

        // Disable certificate verification if needed
        // IMPORTANT: This should ONLY be used for development/testing and NEVER in production
        if !self.verify_certificates {
            logger.warn("SECURITY WARNING: Certificate verification is disabled. This configuration is NOT secure for production use!");
            crypto_config
                .dangerous()
                .set_certificate_verifier(Arc::new(NoVerification {}));
        }

        // Apply transport config using the builder
        let mut client_config = ClientConfig::new(Arc::new(crypto_config));
        client_config.transport_config(shared_transport_config);

        Ok(client_config)
    }
}

impl Default for QuicTransportOptions {
    fn default() -> Self {
        Self::new()
    }
}

pub struct QuicTransport {
    /// Local node identifier
    node_id: PeerId,
    /// Transport options
    network_config: NetworkConfig,
    /// QUIC endpoint
    endpoint: Arc<TokioMutex<Option<Endpoint>>>,
    /// Active connections to other nodes, managed by PeerState
    connections: Arc<TokioRwLock<HashMap<String, Arc<TokioMutex<PeerState>>>>>,
    /// Handler for incoming messages
    handlers: Arc<StdRwLock<Vec<MessageHandler>>>,
    /// Background server task
    server_task: Arc<TokioMutex<Option<JoinHandle<()>>>>,
    /// Background connection cleanup task
    cleanup_task: Arc<TokioMutex<Option<JoinHandle<()>>>>,
    /// Channel to send outgoing messages
    message_tx: Arc<TokioMutex<Option<mpsc::Sender<(NetworkMessage, PeerId)>>>>,
    /// Pending network requests waiting for responses
    pending_requests: Arc<TokioRwLock<HashMap<String, oneshot::Sender<NetworkMessage>>>>,
    /// Connection status callback - required for peer connection events
    /// ConnectionCallback is already an Arc<dyn Fn...> type
    connection_callback: ConnectionCallback,
    /// Registry of known peers
    peer_registry: Arc<PeerRegistry>,
    /// Logger instance
    logger: Logger,
}

impl QuicTransport {
    /// Create a new QUIC transport
    /// 
    /// - `node_id`: Local peer ID that identifies this node
    /// - `network_config`: Network configuration options
    /// - `logger`: Logger instance for transport logs
    /// - `connection_callback`: Required callback function triggered when peers connect/disconnect
    pub fn new(
        node_id: PeerId, 
        network_config: NetworkConfig, 
        logger: Logger,
        connection_callback: ConnectionCallback
    ) -> Self {

        // Create peer registry with default options
        let peer_registry = Arc::new(PeerRegistry::new());

        Self {
            node_id,
            network_config,
            endpoint: Arc::new(TokioMutex::new(None)),
            connections: Arc::new(TokioRwLock::new(HashMap::new())),
            handlers: Arc::new(StdRwLock::new(Vec::new())),
            server_task: Arc::new(TokioMutex::new(None)),
            cleanup_task: Arc::new(TokioMutex::new(None)),
            message_tx: Arc::new(TokioMutex::new(None)),
            pending_requests: Arc::new(TokioRwLock::new(HashMap::new())),
            connection_callback,
            peer_registry,
            logger,
        }
    }

    /// Setup the QUIC endpoint
    async fn setup_endpoint(&self) -> Result<Endpoint> {
        let bind_addr = self.network_config.transport_options.bind_address;

        // Log the bind address details
        self.logger.info(format!(
            "Setting up QUIC endpoint with bind address: {}",
            bind_addr
        ));
        self.logger.debug(format!(
            "QUIC endpoint port: {} (from port range 50000..51000)",
            bind_addr.port()
        ));

        // Create server config
        let server_config = self.create_server_config()?;

        // Create the endpoint
        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| anyhow::anyhow!("Failed to setup endpoint: {}", e))?;

        self.logger.info(format!(
            "QUIC endpoint successfully bound to {}",
            endpoint
                .local_addr()
                .unwrap_or_else(|_| String::from("unknown").parse().unwrap())
        ));

        Ok(endpoint)
    }

    /// Create server configuration for QUIC
    fn create_server_config(&self) -> Result<ServerConfig> {
        // Check if we have QUIC options configured
        let quic_options = self
            .network_config
            .quic_options
            .as_ref()
            .ok_or_else(|| anyhow!("No QUIC options provided in network config"))?;

        // Pass the logger to the options method
        quic_options.create_server_config(&self.logger)
    }

    /// Create client configuration for QUIC
    fn create_client_config(&self) -> Result<ClientConfig> {
        // Check if we have QUIC options configured
        let quic_options = self
            .network_config
            .quic_options
            .as_ref()
            .ok_or_else(|| anyhow!("No QUIC options provided in network config"))?;
        // Pass the logger to the options method
        quic_options.create_client_config(&self.logger)
    }

    /// Handle an incoming connection on the QUIC endpoint
    async fn handle_incoming_connection(
        conn: quinn::Connection,
        logger: Logger,
        handlers: Arc<StdRwLock<Vec<MessageHandler>>>,
        connections: Arc<TokioRwLock<HashMap<String, Arc<TokioMutex<PeerState>>>>>,
        connection_callback: ConnectionCallback,
        max_idle_streams: usize,
    ) -> Result<()> {
        // Define a default idle stream timeout
        let stream_idle_timeout = Duration::from_secs(30);
        
        // Create a default peer ID to use in case handshake fails
        let peer_addr = conn.remote_address().to_string();
        // Initial value that will be replaced with the actual peer ID from handshake
        let mut remote_peer_id = PeerId {
            public_key: format!("unknown-{}", peer_addr),
        };

        logger.info(format!("[HANDSHAKE] Starting handshake process with peer at {}", peer_addr));
        logger.debug("Waiting for handshake message...");

        // 1. Accept the first stream for handshake
        logger.info("[HANDSHAKE] Attempting to accept bidirectional stream for handshake...");
        let (mut handshake_send, mut handshake_recv) =
            match timeout(Duration::from_secs(10), conn.accept_bi()).await {
                Ok(Ok(streams)) => {
                    logger.info("[HANDSHAKE] Successfully accepted bidirectional stream");
                    streams
                },
                Ok(Err(e)) => {
                    logger.error(format!("[HANDSHAKE] Error accepting handshake stream: {}", e));
                    conn.close(1u32.into(), b"handshake stream error");
                    return Err(anyhow!("Handshake stream error: {}", e));
                }
                Err(_) => {
                    // Timeout
                    logger.error("[HANDSHAKE] Timeout waiting for handshake stream - no bidirectional stream was opened by the client");
                    logger.error("[HANDSHAKE] This likely means the client did not attempt to initiate the handshake");
                    conn.close(2u32.into(), b"handshake timeout");
                    return Err(anyhow!("Handshake timeout"));
                }
            };

        // 2. Read and process the handshake message (with size limit)
        // First read the handshake data
        logger.info("[HANDSHAKE] Reading handshake data from stream...");
        let handshake_data = match handshake_recv.read_to_end(1024 * 10).await {
            Ok(data) => {
                logger.info(format!("[HANDSHAKE] Received {} bytes for handshake", data.len()));
                data
            }
            Err(e) => {
                // Stream read error
                logger.error(format!("[HANDSHAKE] Error reading handshake message: {}", e));
                logger.error("[HANDSHAKE] This could indicate connection issues or malformed data");
                conn.close(5u32.into(), b"handshake read error");
                return Err(anyhow!("Handshake read error: {}", e));
            }
        };
        
        // Then deserialize and process it
        logger.info("[HANDSHAKE] Deserializing handshake message...");
        let message = match bincode::deserialize::<NetworkMessage>(&handshake_data) {
            Ok(msg) => msg,
            Err(e) => {
                logger.error(format!("Failed to deserialize handshake message: {}", e));
                conn.close(4u32.into(), b"handshake deserialization error");
                return Err(anyhow!("Handshake deserialization error: {}", e));
            }
        };
        
        // Validate the message type
        if message.message_type != "Handshake" {
            logger.error(format!(
                "Received invalid message type during handshake: {}",
                message.message_type
            ));
            conn.close(3u32.into(), b"invalid handshake message type");
            return Err(anyhow!("Invalid handshake message type"));
        }
        
        // Extract and use the peer ID - explicitly ensure it's a proper PeerId
        let remote_peer_id: PeerId = message.source.clone();
        logger.info(format!("Received valid handshake from peer {}", remote_peer_id));
        
        // Send ACK for handshake
        if let Err(e) = handshake_send.write_all(b"ACK").await {
            logger.error(format!(
                "Failed to send handshake ACK to {}: {}",
                remote_peer_id, e
            ));
            // Don't necessarily close connection, maybe just log
        }
        
        // Process NodeInfo if present
        if let Some(payload_item) = message.payloads.get(0) {
            // Extract and deserialize the NodeInfo directly from the binary data
            match bincode::deserialize::<NodeInfo>(&payload_item.value_bytes) {
                Ok(node_info) => {
                    logger.debug(format!(
                        "Successfully deserialized NodeInfo from handshake: {}",
                        node_info.peer_id
                    ));
                    // Trigger connection callback with node info
                    if let Err(e) = connection_callback(remote_peer_id.clone(), true, Some(node_info)).await {
                logger.error(format!("Error executing connection callback for peer {}: {}", remote_peer_id, e));
            }
                }
                Err(e) => {
                    logger.warn(format!(
                        "Failed to deserialize NodeInfo from handshake: {}",
                        e
                    ));
                }
            }
        } else {
            logger.warn("Handshake message contains no payloads");
        }
        
        // Finish the handshake stream
        if let Err(e) = handshake_send.finish().await {
            logger.warn(format!(
                "Failed to finish handshake ACK stream for {}: {}",
                remote_peer_id, e
            ));
        }
        
        // Ensure we have a valid PeerId for the connection tracking
        if remote_peer_id.public_key.is_empty() {
            logger.error("Remote peer ID is empty or invalid");
            conn.close(7u32.into(), b"invalid peer id");
            return Err(anyhow!("Invalid remote peer ID"));
        }

        // We'll use remote_peer_id consistently throughout the function
        // No need for a separate identified_peer_id variable

        let remote_addr = conn.remote_address();
        // 3. Store PeerState (Initialize idle_streams pool)
        logger.debug(format!(
            "Storing connection state for incoming peer {}",
            remote_peer_id
        ));
        let peer_state = PeerState {
            peer_id: remote_peer_id.clone(),
            address: remote_addr.to_string(),
            connection: conn.clone(),
            last_used: Instant::now(),
            idle_streams: TokioMutex::new(VecDeque::with_capacity(max_idle_streams)),
        };
        {
            let mut connections_write = connections.write().await;
            // What if already connected? Maybe close the old one or reject new?
            // For now, let's overwrite, assuming the new one is valid.
            let replaced_existing = connections_write
                .insert(
                    remote_peer_id.public_key.clone(),
                    Arc::new(TokioMutex::new(peer_state)),
                )
                .is_some();
            // ... close old connection if replaced ...
            if replaced_existing {
                logger.warn(format!(
                    "Replaced existing connection state for peer {}",
                    remote_peer_id.public_key
                ));
                // The connection callback for disconnect will be called if needed when the old connection is closed
            }
        }

        // 4. Loop accepting regular message streams
        logger.debug(format!(
            "Starting to accept regular message streams from {}",
            remote_peer_id
        ));
        loop {
            match conn.accept_bi().await {
                Ok((mut send_stream, mut recv_stream)) => {
                    logger.debug(format!("Accepted regular stream from {}", remote_peer_id));
                    let stream_handlers = handlers.clone();
                    let stream_logger = logger.clone();
                    // Spawn task to handle this specific message stream
                    tokio::spawn(async move {
                        let max_size = 1024 * 1024; // TODO: Use config
                        match recv_stream.read_to_end(max_size).await {
                            Ok(data) => {
                                // Deserialize and process message
                                match bincode::deserialize::<NetworkMessage>(&data) {
                                    Ok(message) => {
                                        stream_logger.debug(format!(
                                            "Received message: type={}, source={}",
                                            message.message_type, message.source
                                        ));
                                        // Call handlers (Note: Handlers are Fn, might block)
                                        if let Ok(handlers_guard) = stream_handlers.read() {
                                            for handler in handlers_guard.iter() {
                                                if let Err(e) = handler(message.clone()) {
                                                    stream_logger.error(format!(
                                                        "Error in message handler: {}",
                                                        e
                                                    ));
                                                }
                                            }
                                        }
                                        // Send ACK
                                        if let Err(e) = send_stream.write_all(b"ACK").await {
                                            stream_logger
                                                .error(format!("Failed to send ACK: {}", e));
                                        }
                                        if let Err(e) = send_stream.finish().await {
                                            stream_logger.warn(format!(
                                                "Failed to finish ACK stream: {}",
                                                e
                                            ));
                                        }
                                    }
                                    Err(e) => stream_logger
                                        .error(format!("Failed to deserialize message: {}", e)),
                                }
                            }
                            Err(e) => stream_logger.error(format!("Error reading stream: {}", e)),
                        }
                    });
                }
                // Handle connection closing errors
                Err(ConnectionError::ApplicationClosed { .. }) => {
                    logger.info(format!(
                        "Connection closed by application (peer: {})",
                        remote_peer_id
                    ));
                    break;
                }
                Err(ConnectionError::LocallyClosed) => {
                    logger.info(format!(
                        "Connection closed locally (peer: {})",
                        remote_peer_id
                    ));
                    break;
                }
                Err(ConnectionError::TimedOut) => {
                    logger.warn(format!("Connection timed out (peer: {})", remote_peer_id));
                    break;
                }
                Err(e) => {
                    logger.error(format!(
                        "Error accepting stream from {}: {}",
                        remote_peer_id, e
                    ));
                    break;
                }
            }
        }

        // Cleanup after loop breaks (connection closed)
        logger.info(format!(
            "Connection handling loop finished for peer {}",
            remote_peer_id
        ));
        
        // Remove PeerState from map using the peer's public key as the map key
        let existed = {
            let mut connections_write = connections.write().await;
            connections_write
                .remove(&remote_peer_id.public_key)
                .is_some()
        };
        
        if existed {
            logger.debug(format!(
                "Removed connection state for disconnected peer {}",
                remote_peer_id
            ));
            // Trigger callback for disconnect
            // Clone the peer_id before moving it to the callback
            let peer_id_clone = remote_peer_id.clone();
            if let Err(e) = connection_callback(remote_peer_id, false, None).await {
                logger.error(format!("Error executing connection callback for peer {}: {}", peer_id_clone, e));
            }
        } else {
            logger.warn(format!(
                "Attempted to remove state for peer {}, but it was already gone.",
                remote_peer_id
            ));
        }

        Ok(())
    }

    /// Start the message sending task
    async fn start_message_sender(&self) -> Result<mpsc::Sender<(NetworkMessage, PeerId)>> {
        let (tx, mut rx) = mpsc::channel::<(NetworkMessage, PeerId)>(100);
        let connections_arc: Arc<TokioRwLock<HashMap<String, Arc<TokioMutex<PeerState>>>>> =
            Arc::clone(&self.connections);
        let logger = self.logger.clone();
        let max_idle_streams = self
            .network_config
            .quic_options
            .as_ref()
            .map(|opts| opts.max_idle_streams_per_peer)
            .unwrap_or(10); // Default if not specified

        tokio::spawn(async move {
            while let Some((message, target_id)) = rx.recv().await {
                logger.debug(format!("Sender task received message for {}", target_id));

                let peer_state_mutex = {
                    let connections_read = connections_arc.read().await;
                    connections_read.get(&target_id.public_key).cloned()
                };

                if let Some(state_mutex) = peer_state_mutex {
                    let mut state = state_mutex.lock().await;
                    let conn = state.connection.clone();
                    state.last_used = Instant::now();
                    let mut idle_streams_guard = state.idle_streams.lock().await;

                    let stream_result =
                        if let Some((send, recv, _idle_since)) = idle_streams_guard.pop_front() {
                            logger.debug(format!("Reusing idle stream for {}", target_id));
                            drop(idle_streams_guard);
                            Ok((send, recv))
                        } else {
                            logger.debug(format!("Opening new stream for {}", target_id));
                            drop(idle_streams_guard);
                            drop(state);

                            match conn.open_bi().await {
                                Ok(streams) => Ok(streams),
                                Err(e) => Err(e),
                            }
                        };

                    // Drop state lock if held (already dropped if new stream opened)
                    // The variable `state` might or might not be valid here,
                    // depending on which branch was taken. We rely on prior drops.

                    match stream_result {
                        Ok((mut send_stream, mut recv_stream)) => {
                            match bincode::serialize(&message) {
                                Ok(data) => {
                                    if let Err(e) = send_stream.write_all(&data).await {
                                        logger.error(format!(
                                            "Error writing to QUIC stream for {}: {}",
                                            target_id, e
                                        ));
                                        continue;
                                    }
                                    if let Err(e) = send_stream.finish().await {
                                        logger.warn(format!(
                                            "Error finishing QUIC send stream for {}: {}",
                                            target_id, e
                                        ));
                                        continue;
                                    }
                                    logger.debug(format!(
                                        "Message sent to {}, awaiting ACK...",
                                        target_id
                                    ));

                                    let ack_result = timeout(
                                        Duration::from_secs(5),
                                        recv_stream.read_to_end(64),
                                    )
                                    .await;

                                    let mut pool_full = false;
                                    let state = state_mutex.lock().await;
                                    let mut idle_streams_guard = state.idle_streams.lock().await;
                                    if idle_streams_guard.len() < max_idle_streams {
                                        logger.debug(format!(
                                            "Returning stream to pool for {}",
                                            target_id
                                        ));
                                        idle_streams_guard.push_back((
                                            send_stream,
                                            recv_stream,
                                            Instant::now(),
                                        ));
                                    } else {
                                        logger.debug(format!(
                                            "Stream pool full for {}, closing stream.",
                                            target_id
                                        ));
                                        pool_full = true;
                                    }
                                    drop(idle_streams_guard);
                                    drop(state);

                                    if pool_full {
                                        // Stream is dropped here implicitly when it goes out of scope
                                    }

                                    match ack_result {
                                        Ok(Ok(ack_data)) => {
                                            if ack_data != b"ACK" {
                                                logger.warn(format!(
                                                    "Received invalid ACK from {}: {:?}",
                                                    target_id, ack_data
                                                ));
                                            }
                                        }
                                        Ok(Err(e)) => logger.error(format!(
                                            "Error reading ACK from {}: {}",
                                            target_id, e
                                        )),
                                        Err(_) => logger.warn(format!(
                                            "Timeout waiting for ACK from {}",
                                            target_id
                                        )),
                                    }
                                }
                                Err(e) => {
                                    logger.error(format!(
                                        "Failed to serialize message for {}: {}",
                                        target_id, e
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            logger.error(format!(
                                "Failed to get/open stream for {}: {}",
                                target_id, e
                            ));
                        }
                    }
                } else {
                    logger.warn(format!(
                        "Sender task: No active connection found for peer {}, dropping message.",
                        target_id
                    ));
                }
            }
            logger.info("QUIC message sender task finished.");
        });
        Ok(tx)
    }

    /// Start the background task to clean up idle connections ONLY
    async fn start_cleanup_task(&self) -> Result<JoinHandle<()>> {
        let connections_arc: Arc<TokioRwLock<HashMap<String, Arc<TokioMutex<PeerState>>>>> =
            Arc::clone(&self.connections);
        let callback_arc = self.connection_callback.clone();
        let logger_arc = self.logger.clone();
        let connection_idle_timeout = self
            .network_config
            .quic_options
            .as_ref()
            .map(|opts| Duration::from_millis(opts.connection_idle_timeout_ms))
            .unwrap_or(Duration::from_secs(60)); // Default 60 seconds if not specified
                                                 // Revert: No stream timeout needed here anymore
        let check_interval = Duration::max(Duration::from_secs(5), connection_idle_timeout / 4);

        logger_arc.info(format!(
            "Starting cleanup task: Conn Idle Timeout: {:?}, Check Interval: {:?}", // Simplified log
            connection_idle_timeout, check_interval
        ));

        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(check_interval).await;
                logger_arc.debug("Running connection cleanup check...");
                let now = Instant::now();
                let mut peers_to_remove = Vec::new();
                // Revert: Remove peers_to_update_time collection

                {
                    let connections_read = connections_arc.read().await;
                    for (peer_id, state_mutex) in connections_read.iter() {
                        // Only check connection last_used time
                        if let Ok(state) = state_mutex.try_lock() {
                            // read lock is sufficient
                            if now.duration_since(state.last_used) > connection_idle_timeout {
                                logger_arc.info(format!(
                                    "Peer {} connection idle ({:?}), scheduling removal.",
                                    peer_id,
                                    now.duration_since(state.last_used)
                                ));
                                peers_to_remove.push(peer_id.clone());
                            }
                        } else {
                            logger_arc.warn(format!(
                                "Cleanup task could not lock state for peer {}, skipping check.",
                                peer_id
                            ));
                        }
                    }
                } // connections_read lock released here

                // Revert: Remove Apply Updates block for last_used

                if !peers_to_remove.is_empty() {
                    // If we have any stale peers, iterate through them and close connections
                    let mut connections_write = connections_arc.write().await;
                    for peer_id in peers_to_remove {
                        if let Some(state_mutex) = connections_write.remove(&peer_id) {
                            // Close connection
                            let state = state_mutex.lock().await;
                            state.connection.close(0u32.into(), b"idle timeout");
                            
                            // Trigger callback for disconnect
                            if let Err(e) = callback_arc(PeerId::new(peer_id.clone()), false, None).await {
                                logger_arc.error(format!("Error executing connection callback for peer {}: {}", peer_id, e));
                            }
                        } else {
                            logger_arc.warn(format!("Failed to remove connection for peer {}", peer_id));
                        }
                    }
                } else {
                    logger_arc.debug("No stale peers found in cleanup cycle.");
                }          }
            // ... unreachable log ...
        });
        Ok(task)
    }

    /// Start the server task to accept incoming connections
    async fn start_server_task(&self, endpoint: Endpoint) -> Result<JoinHandle<()>> {
        let logger = self.logger.clone();
        let handlers_arc: Arc<StdRwLock<Vec<MessageHandler>>> = Arc::clone(&self.handlers);
        let connections_arc: Arc<TokioRwLock<HashMap<String, Arc<TokioMutex<PeerState>>>>> =
            Arc::clone(&self.connections);
        let callback_arc = self.connection_callback.clone();
        let max_idle_streams = self
            .network_config
            .quic_options
            .as_ref()
            .map(|opts| opts.max_idle_streams_per_peer)
            .unwrap_or(10); // Default if not specified

        let task = tokio::spawn(async move {
            logger.info(format!(
                "QUIC server listening on {}",
                endpoint.local_addr().unwrap()
            ));

            while let Some(conn_attempt) = endpoint.accept().await {
                let conn_logger = logger.clone(); // Clone logger for this connection attempt
                match conn_attempt.await {
                    Ok(conn) => {
                        let remote_addr = conn.remote_address();
                        conn_logger.info(format!("Accepted QUIC connection from {}", remote_addr));

                        // Spawn a task to handle the handshake and subsequent messages for this connection
                        let conn_handlers = handlers_arc.clone();
                        let conn_connections = connections_arc.clone();
                        let conn_callback = callback_arc.clone();
                        tokio::spawn(async move {
                            // Call the handle_incoming_connection method directly
                            match QuicTransport::handle_incoming_connection(
                                conn,
                                conn_logger.clone(),
                                conn_handlers,
                                conn_connections,
                                conn_callback,
                                max_idle_streams,
                            )
                            .await
                            {
                                Ok(_) => conn_logger.info(format!(
                                    "Finished handling connection from {}",
                                    remote_addr
                                )),
                                Err(e) => conn_logger.error(format!(
                                    "Error handling connection from {}: {}",
                                    remote_addr, e
                                )),
                            }
                        });
                    }
                    Err(e) => {
                        conn_logger.error(format!("Error accepting QUIC connection: {}", e));
                    }
                }
            }
            logger.info("QUIC server task stopped accepting connections.");
        });

        Ok(task)
    }
}

/// Certificate verifier that accepts any certificate
#[derive(Debug)]
struct NoVerification {}

impl rustls::client::ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &Certificate,
        _intermediates: &[Certificate],
        _server_name: &ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

#[async_trait]
impl NetworkTransport for QuicTransport {
    /// Start listening for incoming connections
    async fn start(&self) -> Result<(), NetworkError> {
        self.logger.info("Initializing QUIC transport...");
        let endpoint = self.setup_endpoint().await.map_err(|e| {
            NetworkError::TransportError(format!("Failed to setup endpoint: {}", e))
        })?;
        let mut endpoint_guard = self.endpoint.lock().await;
        *endpoint_guard = Some(endpoint.clone());

        let mut server_task_guard = self.server_task.lock().await;
        if server_task_guard.is_none() {
            self.logger.info("Starting QUIC server task...");
            let server_task_handle = self.start_server_task(endpoint).await.map_err(|e| {
                NetworkError::TransportError(format!("Failed to start server task: {}", e))
            })?;
            *server_task_guard = Some(server_task_handle);
            self.logger.info("QUIC server task started.");
        }
        drop(server_task_guard);

        let mut message_tx_guard = self.message_tx.lock().await;
        if message_tx_guard.is_none() {
            self.logger.info("Starting QUIC message sender task...");
            let sender_tx = self.start_message_sender().await.map_err(|e| {
                NetworkError::TransportError(format!("Failed to start message sender task: {}", e))
            })?;
            *message_tx_guard = Some(sender_tx);
            self.logger.info("QUIC message sender task started.");
        }
        drop(message_tx_guard);

        let mut cleanup_task_guard = self.cleanup_task.lock().await;
        if cleanup_task_guard.is_none() {
            self.logger.info("Starting QUIC connection cleanup task...");
            let cleanup_handle = self.start_cleanup_task().await.map_err(|e| {
                NetworkError::TransportError(format!("Failed to start cleanup task: {}", e))
            })?;
            *cleanup_task_guard = Some(cleanup_handle);
            self.logger.info("QUIC connection cleanup task started.");
        }

        self.logger.info("QUIC transport initialized successfully.");
        Ok(())
    }

    /// Stop listening for incoming connections
    async fn stop(&self) -> Result<(), NetworkError> {
        self.logger.info("Stopping QUIC transport...");

        // Stop the cleanup task first
        let mut cleanup_task_guard = self.cleanup_task.lock().await;
        if let Some(task) = cleanup_task_guard.take() {
            self.logger.debug("Stopping QUIC cleanup task...");
            task.abort();
            match task.await {
                Ok(_) => self.logger.info("QUIC cleanup task stopped."),
                Err(e) if e.is_cancelled() => self.logger.info("QUIC cleanup task cancelled."),
                Err(e) => self
                    .logger
                    .error(format!("Error stopping QUIC cleanup task: {}", e)),
            }
        }
        drop(cleanup_task_guard);

        // Stop the server task
        let mut server_task_guard = self.server_task.lock().await;
        if let Some(task) = server_task_guard.take() {
            self.logger.debug("Stopping QUIC server task...");
            // Close the endpoint to stop accepting new connections
            if let Some(endpoint) = self.endpoint.lock().await.as_ref() {
                endpoint.close(0u32.into(), b"shutting down");
            }
            task.abort(); // Abort in case it's stuck
            match task.await {
                Ok(_) => self.logger.info("QUIC server task stopped."),
                Err(e) if e.is_cancelled() => self.logger.info("QUIC server task cancelled."),
                Err(e) => self
                    .logger
                    .error(format!("Error stopping QUIC server task: {}", e)),
            }
        }
        drop(server_task_guard);

        // Close all active connections
        self.logger.debug("Closing all active QUIC connections...");
        let mut connections_guard = self.connections.write().await; // write lock to clear
        for (_peer_id, state_mutex) in connections_guard.iter() {
            let state = state_mutex.lock().await; // Lock each state to get connection
            state.connection.close(0u32.into(), b"transport stopping");
        }
        connections_guard.clear(); // Clear the map
        drop(connections_guard);
        self.logger.info("All active QUIC connections closed.");

        // Clear endpoint
        *self.endpoint.lock().await = None;

        // Clear message sender (the task should exit when the channel closes)
        *self.message_tx.lock().await = None;

        self.logger.info("QUIC transport stopped.");
        Ok(())
    }

    /// Check if the transport is running
    fn is_running(&self) -> bool {
        // Check if server_task is Some
        true
    }

    /// Register a discovered node
    async fn connect_node(
        &self,
        peer_info: PeerInfo,
        local_node: NodeInfo,
    ) -> Result<(), NetworkError> {
        let peer_public_key = peer_info.public_key.clone();
        // If we already have this node in the registry, just return success
        if let Some(_entry) = self.peer_registry.find_peer(peer_public_key.clone()) {
            self.logger
                .debug(format!("Node {} already in registry", peer_public_key));
            return Ok(());
        }
        // Add to registry using our helper method
        // Add the peer to registry
        let add_result = self.peer_registry.add_peer(peer_info.clone());
        match add_result {
            Ok(_) => {
                self.logger.debug(format!(
                    "Added peer {} to registry - will connect to it now",
                    peer_public_key
                ));

                // Check if already connected
                let connections_read = self.connections.read().await;
                if connections_read.contains_key(&peer_public_key) {
                    self.logger
                        .debug(format!("Already connected to peer {}", peer_public_key));
                    return Ok(());
                }
                drop(connections_read);

                //attempt to connect to the node using the list of addresses
                for address_str in peer_info.addresses {
                    match address_str.parse::<SocketAddr>() {
                        Ok(target_addr) => {
                            let peer_id = PeerId::new(peer_public_key.clone());
                            // Validate the target address
                            if target_addr.port() == 0 {
                                let err_msg =
                                    format!("Cannot connect to {}: invalid port 0", target_addr);
                                self.logger.error(&err_msg);
                                return Err(NetworkError::ConnectionError(err_msg));
                            }

                            // Get endpoint
                            let endpoint_guard = self.endpoint.lock().await;
                            let endpoint = endpoint_guard.as_ref().ok_or_else(|| {
                                NetworkError::TransportError(
                                    "QUIC endpoint not initialized".to_string(),
                                )
                            })?;

                            // Create client config
                            let client_config = self.create_client_config().map_err(|e| {
                                NetworkError::ConfigurationError(format!(
                                    "Failed to create QUIC client config: {}",
                                    e
                                ))
                            })?;

                            // Attempt connection
                            self.logger.debug(format!(
                                "Establishing QUIC connection to {} at {}",
                                peer_public_key, target_addr
                            ));
                            let connection = endpoint
                                .connect_with(client_config, target_addr, "runar-node")
                                .map_err(|e| {
                                    NetworkError::ConnectionError(format!(
                                        "Failed to initiate connection: {}",
                                        e
                                    ))
                                })?
                                .await // Await the connection attempt
                                .map_err(|e| {
                                    NetworkError::ConnectionError(format!(
                                        "QUIC connection failed for {}: {}",
                                        peer_public_key, e
                                    ))
                                })?;

                            self.logger.info(format!(
                                "Successfully established QUIC connection with {} ({})",
                                peer_public_key,
                                connection.remote_address()
                            ));

                            // --- Send Handshake ---
                            self.logger
                                .debug(format!("Sending handshake to peer {}", peer_public_key));
                            match connection.open_bi().await {
                                Ok((mut send_stream, _recv_stream)) => {
                                    // We don't need recv_stream for simple handshake send
                                    // Step 1: Directly serialize the NodeInfo to binary format
                                    self.logger.info("[HANDSHAKE] Creating handshake message with NodeInfo...");
                                    let node_info_bytes = match bincode::serialize(&local_node) {
                                        Ok(bytes) => {
                                            self.logger.info(format!("[HANDSHAKE] Successfully serialized NodeInfo ({} bytes)", bytes.len()));
                                            bytes
                                        },
                                        Err(e) => {
                                            self.logger.error(format!(
                                                "[HANDSHAKE] Failed to serialize NodeInfo for handshake: {}",
                                                e
                                            ));
                                            continue;
                                        }
                                    };

                                    // Step 2: Create a complete handshake message with metadata
                                    self.logger.info("[HANDSHAKE] Creating complete handshake message...");
                                    let handshake_message = NetworkMessage {
                                        source: self.get_local_node_id(),
                                        destination: peer_id.clone(), // Destination is the peer we connected to
                                        message_type: "Handshake".to_string(),
                                        // Use binary data directly
                                        payloads: vec![NetworkMessagePayloadItem::new(
                                            "".to_string(),
                                            node_info_bytes,
                                            "".to_string(),
                                        )],
                                    };
                                    self.logger.info(format!("[HANDSHAKE] Created handshake message to peer {}", peer_id.public_key));

                                    match bincode::serialize(&handshake_message) {
                                        Ok(data) => {
                                            let size = data.len();
                                            self.logger.debug(format!(
                                                "Writing {} bytes for handshake",
                                                size
                                            ));
                                            match send_stream.write_all(&data).await {
                                                Ok(_) => {
                                                    self.logger.info("[HANDSHAKE] Successfully sent handshake message");
                                                    // Handshake sent, now finish stream and wait for ACK
                                                    self.logger.info("[HANDSHAKE] Finishing handshake stream and waiting for ACK...");
                                                    if let Err(e) = send_stream.finish().await {
                                                        self.logger.warn(format!("Failed to finish handshake send stream for {}: {}", peer_public_key, e));
                                                    } else {
                                                        self.logger.debug(format!(
                                                            "Handshake message sent successfully to {}",
                                                            peer_public_key
                                                        ));
                                                    }
                                                }
                                                Err(e) => {
                                                    // Log error but don't necessarily fail the connection yet
                                                    self.logger.error(format!(
                                                        "Failed to write handshake message to {}: {}",
                                                        peer_public_key, e
                                                    ));
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            self.logger.error(format!(
                                                "Failed to serialize handshake message for {}: {}",
                                                peer_public_key, e
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    // Log error but don't necessarily fail the connection yet
                                    self.logger.error(format!(
                                        "Failed to open stream for handshake to {}: {}",
                                        peer_public_key, e
                                    ));
                                }
                            }
                            // --- End Handshake ---

                            // Create PeerState (Initialize idle_streams pool)
                            let peer_state = PeerState {
                                peer_id: peer_id.clone(),
                                address: address_str,
                                connection,
                                last_used: Instant::now(),
                                idle_streams: TokioMutex::new(VecDeque::with_capacity(
                                    self.network_config
                                        .quic_options
                                        .as_ref()
                                        .map(|opts| opts.max_idle_streams_per_peer)
                                        .unwrap_or(10), // Default if not specified
                                )),
                            };

                            // Store PeerState
                            let mut connections_write = self.connections.write().await;
                            connections_write.insert(
                                peer_public_key.clone(),
                                Arc::new(TokioMutex::new(peer_state)),
                            );
                            drop(connections_write);

                            self.logger.debug(format!(
                                "Stored connection state for peer {}",
                                peer_public_key
                            ));

                            // Trigger callback AFTER storing state
                            if let Err(e) = (self.connection_callback)(peer_id.clone(), true, Some(local_node.clone())).await {
                                self.logger.error(format!(
                                    "Error executing connection callback for peer {}: {}",
                                    peer_id, e
                                ));
                            }

                            //when connected, break out of the loop - no need to try other addresses
                            break;
                        }
                        Err(e) => {
                            self.logger.error(format!(
                                "Failed to parse peer address: {} - {}",
                                address_str, e
                            ));
                            continue;
                        }
                    };
                }
                Ok(())
            }
            Err(e) => {
                self.logger.error(format!(
                    "Failed to add node {} to registry: {}",
                    peer_public_key, e
                ));
                Err(NetworkError::DiscoveryError(format!(
                    "Failed to register node: {}",
                    e
                )))
            }
        }
    }

    /// Get the local address this transport is bound to
    fn get_local_address(&self) -> String {
        // Get the actual bound address from the endpoint
        if let Ok(ep_lock) = self.endpoint.try_lock() {
            if let Some(ep) = ep_lock.as_ref() {
                if let Ok(addr) = ep.local_addr() {
                    return addr.to_string();
                }
            }
        }
        // Fallback to the configured address if we can't get the actual one
        self.network_config
            .transport_options
            .bind_address
            .to_string()
    }

    /// Get the local node identifier
    fn get_local_node_id(&self) -> PeerId {
        self.node_id.clone()
    }

    /// Disconnect from a remote node
    async fn disconnect(&self, target_peer_id: PeerId) -> Result<(), NetworkError> {
        self.logger
            .info(format!("Disconnecting from peer {}", target_peer_id));
        let mut connections_write = self.connections.write().await;

        if let Some(state_mutex) = connections_write.remove(&target_peer_id.public_key) {
            drop(connections_write);

            let state = state_mutex.lock().await;
            self.logger
                .debug(format!("Closing QUIC connection to {}", target_peer_id));
            // Close the connection with a specific error code and reason
            state.connection.close(0u32.into(), b"disconnect requested");
            drop(state);

            self.logger.info(format!(
                "Successfully disconnected from peer {}",
                target_peer_id
            ));
            // Trigger callback AFTER removing state and closing connection
            // Clone the peer_id before using it in the callback to prevent ownership issues
            let peer_id_for_log = target_peer_id.clone();
            if let Err(e) = (self.connection_callback)(target_peer_id, false, None).await {
                self.logger.error(format!(
                    "Error executing connection callback for peer {}: {}",
                    peer_id_for_log, e
                ));
            }
            Ok(())
        } else {
            self.logger.warn(format!(
                "Attempted to disconnect from non-connected peer {}",
                target_peer_id
            ));
            // Not an error if already disconnected
            Ok(())
        }
    }

    /// Check if connected to a specific node
    fn is_connected(&self, peer_id: PeerId) -> bool {
        // Use try_read instead of block_on to avoid runtime panic
        match self.connections.try_read() {
            Ok(guard) => guard.contains_key(&peer_id.public_key),
            Err(_) => {
                // If we can't get the lock immediately, log a warning and return false
                self.logger.warn(format!(
                    "Failed to acquire read lock when checking connection to {}",
                    peer_id.public_key
                ));
                false
            }
        }
    }

    /// Register a message handler for incoming messages
    fn register_message_handler(&self, handler: MessageHandler) -> Result<()> {
        // Current implementation of register_handler
        match self.handlers.write() {
            Ok(mut handlers) => {
                handlers.push(handler);
                Ok(())
            }
            Err(_) => {
                Err(anyhow::Error::msg("Failed to acquire write lock for message handlers").into())
            }
        }
    }

    // Connection callback is now a required parameter during construction

    /// Send a message to a remote node
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        let tx_guard = self.message_tx.lock().await;
        if let Some(sender) = tx_guard.as_ref() {
            let target_id: PeerId = message.destination.clone();
            sender.send((message, target_id)).await.map_err(|e| {
                NetworkError::MessageError(format!("Failed to queue message for sending: {}", e))
            })
        } else {
            Err(NetworkError::TransportError(
                "Message sender task not initialized".to_string(),
            ))
        }
    }

    /// Send a service request to a remote node
    async fn send_request(&self, message: NetworkMessage) -> Result<NetworkMessage, NetworkError> {
        // Requests should typically have one payload, extract its correlation ID
        // If multiple payloads are possible for requests, this logic needs adjustment
        let correlation_id = message.payloads.get(0)
            .map(|payload_item| payload_item.correlation_id.clone())
            .unwrap_or_else(|| {
                // Generate a correlation ID if the first payload doesn't have one (or if no payloads)
                // This might indicate an issue with how the request NetworkMessage was constructed
                self.logger.warn("send_request called with NetworkMessage missing correlation_id in first payload, generating new one.");
                uuid::Uuid::new_v4().to_string()
            });

        // Create a channel for receiving the response
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Register the response channel with the correlation ID
        let mut pending_requests = self.pending_requests.write().await;
        pending_requests.insert(correlation_id.clone(), tx);

        // Send the message. Ensure the message being sent has the correct correlation_id in its payload(s).
        // The original message already contains the payloads vec.
        self.send_message(message.clone()).await?;

        // Wait for the response with a timeout
        let timeout_duration = self
            .network_config
            .transport_options
            .timeout
            .unwrap_or_else(|| Duration::from_secs(30));
        match timeout(timeout_duration, rx).await {
            Ok(response_result) => match response_result {
                Ok(response) => Ok(response),
                Err(_) => Err(NetworkError::MessageError(
                    "Response channel was closed".to_string(),
                )),
            },
            Err(_) => {
                // Clean up the pending request on timeout
                let mut pending_requests = self.pending_requests.write().await;
                pending_requests.remove(&correlation_id);
                Err(NetworkError::MessageError(format!(
                    "Request timed out after {:?}",
                    timeout_duration
                )))
            }
        }
    }

    /// Handle an incoming network message
    async fn handle_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        // For response messages, complete the pending request for each payload
        if message.message_type == "Response" {
            let mut pending_requests = self.pending_requests.write().await;
            for payload_item in &message.payloads {
                if let Some(sender) = pending_requests.remove(&payload_item.correlation_id) {
                    // Clone the relevant parts of the message for this specific response payload
                    let single_payload_message = NetworkMessage {
                        source: message.source.clone(),
                        destination: message.destination.clone(), // Should be self
                        message_type: message.message_type.clone(),
                        payloads: vec![payload_item.clone()],
                    };
                    // Send only the relevant payload back
                    if sender.send(single_payload_message).is_err() {
                        self.logger.warn(format!(
                            "Response channel closed for correlation ID: {}",
                            payload_item.correlation_id
                        ));
                    }
                } else {
                    self.logger.warn(format!(
                        "Received response for unknown correlation ID: {}",
                        payload_item.correlation_id
                    ));
                }
            }
            // Drop the lock before potentially calling handlers
            drop(pending_requests);

            // If it was a response, we assume it's handled, return Ok
            // Or should responses also go to handlers? Check design.
            return Ok(());
        }

        // For other messages, pass to registered handlers
        if let Ok(handlers) = self.handlers.read() {
            for handler in handlers.iter() {
                if let Err(e) = handler(message.clone()) {
                    self.logger
                        .warn(&format!("Error in message handler: {}", e));
                    // Continue with other handlers
                }
            }
        }

        Ok(())
    }

    /// Start node discovery process
    async fn start_discovery(&self) -> Result<(), NetworkError> {
        self.logger
            .warn("start_discovery not implemented for QuicTransport");
        Ok(())
    }

    /// Stop node discovery process
    async fn stop_discovery(&self) -> Result<(), NetworkError> {
        self.logger
            .warn("stop_discovery not implemented for QuicTransport");
        Ok(())
    }

    /// Get all discovered nodes
    fn get_discovered_nodes(&self) -> Vec<PeerId> {
        self.logger
            .warn("get_discovered_nodes not implemented for QuicTransport");
        Vec::new()
    }

    /// Set the node discovery mechanism
    fn set_node_discovery(&self, _discovery: Box<dyn NodeDiscovery>) -> Result<()> {
        Err(anyhow!("set_node_discovery not supported by QuicTransport").into())
    }

    /// Complete a pending request
    fn complete_pending_request(
        &self,
        correlation_id: String,
        response: NetworkMessage,
    ) -> Result<(), NetworkError> {
        let pending_requests = self.pending_requests.clone();
        // Use spawn_blocking if the map operation could potentially block significantly,
        // but write().await is async anyway, so regular spawn is likely fine.
        tokio::spawn(async move {
            let mut requests = pending_requests.write().await;
            if let Some(sender) = requests.remove(&correlation_id) {
                // It's okay if sending fails (receiver dropped)
                let _ = sender.send(response);
            }
            // If sender not found, maybe log a warning? Already logged in handle_message.
        });
        Ok(())
    }
}
