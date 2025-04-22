// Network Transport Module
//
// This module defines the network transport interfaces and implementations.

// Standard library imports
use std::time::Duration;
use std::sync::{Arc, RwLock as StdRwLock};
use std::net::SocketAddr;
use std::collections::HashMap;
use std::time::Instant;
use std::collections::VecDeque;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::ops::Range;

// External crate imports
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use quinn::{Endpoint, ServerConfig, ClientConfig, ConnectionError, TransportConfig};
use rustls::{ServerName, Certificate, PrivateKey};
use tokio::sync::{RwLock as TokioRwLock, Mutex as TokioMutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio::sync::oneshot;
use uuid;

// Internal module imports
use super::{NetworkTransport, TransportFactory, TransportOptions, NetworkMessage, NetworkError, PeerId, MessageHandler, ConnectionCallback};
use crate::network::discovery::NodeDiscovery;
use runar_common::Logger;

/// Find a free port in the given range
pub fn pick_free_port(port_range: Range<u16>) -> Option<u16> {
    for port in port_range {
        if let Ok(listener) = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)) {
            return Some(listener.local_addr().ok()?.port());
        }
    }
    None
}

// QUIC Transport Implementation
//
// INTENTION: Implement the NetworkTransport trait using high-performance,
// secure, multiplexed communication between nodes. QUIC provides transport
// security, connection migration, and reliable messaging.

/// Holds the state for a connection to a specific peer.
#[derive(Debug)]
struct PeerState {
    peer_id: PeerId,
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
    /// Base transport options
    pub transport_options: TransportOptions,
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
        let port = pick_free_port(50000..51000).unwrap_or(0);
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        
        Self {
            transport_options: TransportOptions {
                bind_address: bind_addr,
                timeout: Some(Duration::from_secs(30)),
                max_message_size: Some(1024 * 1024), // 1MB default
            },
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
    
    /// Set custom bind address
    pub fn with_bind_address(mut self, addr: SocketAddr) -> Self {
        self.transport_options.bind_address = addr;
        self
    }
    
    /// Set custom port (using localhost/127.0.0.1)
    pub fn with_port(mut self, port: u16) -> Self {
        self.transport_options.bind_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
        self
    }
    
    /// Set port from a specific range (using localhost/127.0.0.1)
    pub fn with_port_in_range(self, port_range: Range<u16>) -> Self {
        let port = pick_free_port(port_range).unwrap_or(0);
        self.with_port(port)
    }
    
    /// Set timeout for network operations
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.transport_options.timeout = Some(timeout);
        self
    }
    
    /// Set maximum message size
    pub fn with_max_message_size(mut self, size: usize) -> Self {
        self.transport_options.max_message_size = Some(size);
        self
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
    
    /// Create server configuration for QUIC
    pub fn create_server_config(&self, logger: &Logger) -> Result<ServerConfig> {
        // Create transport config
        let mut transport_config = TransportConfig::default();
        
        // Set keep-alive interval
        transport_config.keep_alive_interval(Some(Duration::from_millis(
            self.keep_alive_interval_ms
        )));
        
        // Set max concurrent streams
        transport_config.max_concurrent_bidi_streams(
            self.max_concurrent_bidi_streams.into()
        );
        
        // Load certificates from memory or file
        let (certs, key) = self.get_certificates_and_key(logger)?;
        
        // Create server configuration (will fail if no valid certificates are provided)
        let mut server_config = ServerConfig::with_single_cert(certs, key)?;
        server_config.transport = Arc::new(transport_config);
        
        Ok(server_config)
    }
    
    /// Create client configuration for QUIC
    pub fn create_client_config(&self, logger: &Logger) -> Result<ClientConfig> {
        // Create transport config
        let mut transport_config = TransportConfig::default();
        
        transport_config.keep_alive_interval(Some(Duration::from_millis(
            self.keep_alive_interval_ms
        )));
        
        transport_config.max_concurrent_bidi_streams(
            self.max_concurrent_bidi_streams.into()
        );
        
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
            crypto_config.dangerous().set_certificate_verifier(Arc::new(NoVerification {}));
        }
        
        // Apply transport config using the builder
        let mut client_config = ClientConfig::new(Arc::new(crypto_config));
        client_config.transport_config(shared_transport_config);
        
        Ok(client_config)
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
}

impl Default for QuicTransportOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// QUIC-based implementation of NetworkTransport
pub struct QuicTransport {
    /// Local node identifier
    node_id: PeerId,
    /// Transport options
    options: QuicTransportOptions,
    /// QUIC endpoint
    endpoint: Arc<TokioMutex<Option<Endpoint>>>,
    /// Active connections to other nodes, managed by PeerState
    connections: Arc<TokioRwLock<HashMap<PeerId, Arc<TokioMutex<PeerState>>>>>,
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
    /// Connection status callback
    connection_callback: Arc<TokioRwLock<Option<ConnectionCallback>>>,
    /// Logger instance
    logger: Logger,
}

impl QuicTransport {
    /// Create a new QUIC transport
    pub fn new(node_id: PeerId, options: QuicTransportOptions, logger: Logger) -> Self {
        Self {
            node_id,
            options,
            endpoint: Arc::new(TokioMutex::new(None)),
            connections: Arc::new(TokioRwLock::new(HashMap::new())),
            handlers: Arc::new(StdRwLock::new(Vec::new())),
            server_task: Arc::new(TokioMutex::new(None)),
            cleanup_task: Arc::new(TokioMutex::new(None)),
            message_tx: Arc::new(TokioMutex::new(None)),
            pending_requests: Arc::new(TokioRwLock::new(HashMap::new())),
            connection_callback: Arc::new(TokioRwLock::new(None)),
            logger,
        }
    }
    
    /// Setup the QUIC endpoint
    async fn setup_endpoint(&self) -> Result<Endpoint> {
        // Create server config using the options
        let server_config = self.options.create_server_config(&self.logger)?;
        
        // Use the bind address from the options directly
        let bind_addr = self.options.transport_options.bind_address;
            
        // Create endpoint from server config and socket address
        let endpoint = Endpoint::server(server_config, bind_addr)?;
        
        Ok(endpoint)
    }
    
    /// Start the server task to accept incoming connections
    async fn start_server_task(&self, endpoint: Endpoint) -> Result<JoinHandle<()>> {
        let logger = self.logger.clone();
        let handlers_arc: Arc<StdRwLock<Vec<MessageHandler>>> = Arc::clone(&self.handlers);
        let connections_arc: Arc<TokioRwLock<HashMap<PeerId, Arc<TokioMutex<PeerState>>>>> = Arc::clone(&self.connections);
        let callback_arc: Arc<TokioRwLock<Option<ConnectionCallback>>> = Arc::clone(&self.connection_callback);
        let max_idle_streams = self.options.max_idle_streams_per_peer;

        let task = tokio::spawn(async move {
            logger.info(format!("QUIC server listening on {}", endpoint.local_addr().unwrap()));

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
                            match Self::handle_incoming_connection(conn, conn_logger.clone(), conn_handlers, conn_connections, conn_callback, max_idle_streams).await {
                                Ok(_) => conn_logger.info(format!("Finished handling connection from {}", remote_addr)),
                                Err(e) => conn_logger.error(format!("Error handling connection from {}: {}", remote_addr, e)),
                            }
                        });
                    },
                    Err(e) => {
                        conn_logger.error(format!("Error accepting QUIC connection: {}", e));
                    }
                }
            }
            logger.info("QUIC server task stopped accepting connections.");
        });

        Ok(task)
    }
    
    /// Handles the lifecycle of a single accepted incoming connection
    async fn handle_incoming_connection(
        conn: quinn::Connection,
        logger: Logger,
        handlers: Arc<StdRwLock<Vec<MessageHandler>>>,
        connections: Arc<TokioRwLock<HashMap<PeerId, Arc<TokioMutex<PeerState>>>>>, 
        connection_callback: Arc<TokioRwLock<Option<ConnectionCallback>>>,
        max_idle_streams: usize,
    ) -> Result<()> {
        logger.debug("Waiting for handshake message...");

        // 1. Accept the first stream for handshake
        let (mut handshake_send, mut handshake_recv) = match timeout(Duration::from_secs(10), conn.accept_bi()).await {
            Ok(Ok(streams)) => streams,
            Ok(Err(e)) => {
                logger.error(format!("Error accepting handshake stream: {}", e));
                conn.close(1u32.into(), b"handshake stream error");
                return Err(anyhow!("Handshake stream error: {}", e));
            }
            Err(_) => { // Timeout
                logger.error("Timeout waiting for handshake stream.");
                conn.close(2u32.into(), b"handshake timeout");
                return Err(anyhow!("Handshake timeout"));
            }
        };
        
        // 2. Read and process the handshake message
        let remote_peer_id = match timeout(Duration::from_secs(5), handshake_recv.read_to_end(1024 * 10)).await { // Limit handshake size
            Ok(Ok(data)) => {
                logger.debug(format!("Received {} bytes for handshake", data.len()));
                match bincode::deserialize::<NetworkMessage>(&data) {
                    Ok(message) if message.message_type == "Handshake" => {
                        let peer_id = message.source;
                        logger.info(format!("Received valid handshake from peer {}", peer_id));
                        // Send ACK for handshake
                        if let Err(e) = handshake_send.write_all(b"ACK").await {
                             logger.error(format!("Failed to send handshake ACK to {}: {}", peer_id, e));
                             // Don't necessarily close connection, maybe just log
                        }
                        if let Err(e) = handshake_send.finish().await {
                            logger.warn(format!("Failed to finish handshake ACK stream for {}: {}", peer_id, e));
                        }
                        peer_id // Return the identified PeerId
                    }
                    Ok(message) => {
                        logger.error(format!("Received invalid message type during handshake: {}", message.message_type));
                        conn.close(3u32.into(), b"invalid handshake message type");
                        return Err(anyhow!("Invalid handshake message type"));
                    }
                    Err(e) => {
                        logger.error(format!("Failed to deserialize handshake message: {}", e));
                        conn.close(4u32.into(), b"handshake deserialization error");
                        return Err(anyhow!("Handshake deserialization error: {}", e));
                    }
                }
            }
            Ok(Err(e)) => { // Stream read error
                 logger.error(format!("Error reading handshake message: {}", e));
                 conn.close(5u32.into(), b"handshake read error");
                 return Err(anyhow!("Handshake read error: {}", e));
            }
            Err(_) => { // Timeout reading handshake
                 logger.error("Timeout reading handshake message.");
                 conn.close(6u32.into(), b"handshake read timeout");
                 return Err(anyhow!("Handshake read timeout"));
            }
        };

        // Capture peer_id before moving connection into state
        let identified_peer_id = remote_peer_id.clone(); 

        // 3. Store PeerState (Initialize idle_streams pool)
        logger.debug(format!("Storing connection state for incoming peer {}", remote_peer_id));
        let peer_state = PeerState {
            peer_id: remote_peer_id.clone(),
            connection: conn.clone(),
            last_used: Instant::now(),
            idle_streams: TokioMutex::new(VecDeque::with_capacity(max_idle_streams)),
        };
        {
            let mut connections_write = connections.write().await;
             // What if already connected? Maybe close the old one or reject new?
             // For now, let's overwrite, assuming the new one is valid.
            let replaced_existing = connections_write.insert(remote_peer_id.clone(), Arc::new(TokioMutex::new(peer_state))).is_some();
             // ... close old connection if replaced ...
             if replaced_existing {
                logger.warn(format!("Replaced existing connection state for peer {}", remote_peer_id));
                // TODO: Trigger callback for disconnect of OLD connection?
             }
        }

        // Trigger callback for new connection AFTER storing state
        Self::trigger_connection_callback(connection_callback.clone(), identified_peer_id.clone(), true).await; 

        // 4. Loop accepting regular message streams
        logger.debug(format!("Starting to accept regular message streams from {}", remote_peer_id));
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
                                        stream_logger.debug(format!("Received message: type={}, source={}", message.message_type, message.source));
                                        // Call handlers (Note: Handlers are Fn, might block)
                                         if let Ok(handlers_guard) = stream_handlers.read() {
                                             for handler in handlers_guard.iter() {
                                                 if let Err(e) = handler(message.clone()) {
                                                     stream_logger.error(format!("Error in message handler: {}", e));
                                                 }
                                             }
                                         }
                                        // Send ACK
                                        if let Err(e) = send_stream.write_all(b"ACK").await {
                                            stream_logger.error(format!("Failed to send ACK: {}", e));
                                        }
                                         if let Err(e) = send_stream.finish().await {
                                            stream_logger.warn(format!("Failed to finish ACK stream: {}", e));
                                        }
                                    }
                                    Err(e) => stream_logger.error(format!("Failed to deserialize message: {}", e)),
                                }
                            }
                            Err(e) => stream_logger.error(format!("Error reading stream: {}", e)),
                        }
                    });
                }
                // Handle connection closing errors
                Err(ConnectionError::ApplicationClosed { .. } ) => {
                     logger.info(format!("Connection closed by application (peer: {})", remote_peer_id));
                     break;
                }
                Err(ConnectionError::LocallyClosed) => {
                     logger.info(format!("Connection closed locally (peer: {})", remote_peer_id));
                     break;
                }
                Err(ConnectionError::TimedOut) => {
                     logger.warn(format!("Connection timed out (peer: {})", remote_peer_id));
                     break;
                }
                Err(e) => {
                    logger.error(format!("Error accepting stream from {}: {}", remote_peer_id, e));
                    break;
                }
            }
        }

        // Cleanup after loop breaks (connection closed)
        logger.info(format!("Connection handling loop finished for peer {}", remote_peer_id));
        // Remove PeerState from map
        let existed = {
             let mut connections_write = connections.write().await;
             connections_write.remove(&identified_peer_id).is_some()
        };
        if existed {
             logger.debug(format!("Removed connection state for disconnected peer {}", identified_peer_id));
              // Trigger callback for disconnect
             Self::trigger_connection_callback(connection_callback.clone(), identified_peer_id.clone(), false).await; 
         } else {
            logger.warn(format!("Attempted to remove state for peer {}, but it was already gone.", identified_peer_id));
         }

        Ok(())
    }
    
    /// Start the message sending task
    async fn start_message_sender(&self) -> Result<mpsc::Sender<(NetworkMessage, PeerId)>> {
        let (tx, mut rx) = mpsc::channel::<(NetworkMessage, PeerId)>(100);
        let connections_arc: Arc<TokioRwLock<HashMap<PeerId, Arc<TokioMutex<PeerState>>>>> = Arc::clone(&self.connections);
        let logger = self.logger.clone();
        let max_idle_streams = self.options.max_idle_streams_per_peer;

        tokio::spawn(async move {
            while let Some((message, target_id)) = rx.recv().await {
                logger.debug(format!("Sender task received message for {}", target_id));

                let peer_state_mutex = { 
                    let connections_read = connections_arc.read().await;
                    connections_read.get(&target_id).cloned()
                };

                if let Some(state_mutex) = peer_state_mutex {
                    let mut state = state_mutex.lock().await;
                    let conn = state.connection.clone();
                    state.last_used = Instant::now();
                    let mut idle_streams_guard = state.idle_streams.lock().await;

                    let stream_result = if let Some((send, recv, _idle_since)) = idle_streams_guard.pop_front() {
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
                                        logger.error(format!("Error writing to QUIC stream for {}: {}", target_id, e));
                                        continue;
                                    }
                                    if let Err(e) = send_stream.finish().await {
                                        logger.warn(format!("Error finishing QUIC send stream for {}: {}", target_id, e));
                                        continue;
                                    }
                                    logger.debug(format!("Message sent to {}, awaiting ACK...", target_id));

                                    let ack_result = timeout(Duration::from_secs(5), recv_stream.read_to_end(64)).await;
                                    
                                    let mut pool_full = false;
                                    let mut state = state_mutex.lock().await;
                                    let mut idle_streams_guard = state.idle_streams.lock().await;
                                    if idle_streams_guard.len() < max_idle_streams {
                                        logger.debug(format!("Returning stream to pool for {}", target_id));
                                        idle_streams_guard.push_back((send_stream, recv_stream, Instant::now()));
                                    } else {
                                        logger.debug(format!("Stream pool full for {}, closing stream.", target_id));
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
                                                logger.warn(format!("Received invalid ACK from {}: {:?}", target_id, ack_data));
                                            }
                                        }
                                        Ok(Err(e)) => logger.error(format!("Error reading ACK from {}: {}", target_id, e)),
                                        Err(_) => logger.warn(format!("Timeout waiting for ACK from {}", target_id)),
                                    }
                                }
                                Err(e) => {
                                    logger.error(format!("Failed to serialize message for {}: {}", target_id, e));
                                }
                            }
                        }
                        Err(e) => {
                            logger.error(format!("Failed to get/open stream for {}: {}", target_id, e));
                        }
                    }
                } else {
                    logger.warn(format!("Sender task: No active connection found for peer {}, dropping message.", target_id));
                }
            }
            logger.info("QUIC message sender task finished.");
        });
        Ok(tx)
    }
    
    /// Handle an incoming connection
    async fn handle_connection(&self, conn: quinn::Connection) -> Result<()> {
        self.logger.info(format!("Handling new QUIC connection from: {}", conn.remote_address()));
        loop {
            match conn.accept_bi().await {
                Ok((mut send_stream, mut recv_stream)) => {
                    self.logger.debug("Accepted new QUIC bidirectional stream");
                    
                    let handlers_arc: Arc<StdRwLock<Vec<MessageHandler>>> = Arc::clone(&self.handlers);
                    let logger_clone = self.logger.clone();

                    tokio::spawn(async move {
                        let max_size = 1024 * 1024; 
                        match recv_stream.read_to_end(max_size).await {
                            Ok(data) => {
                                logger_clone.debug(format!("Received {} bytes on QUIC stream", data.len()));
                                match bincode::deserialize::<NetworkMessage>(&data) {
                                    Ok(message) => {
                                        // Use standard RwLock instead of Tokio RwLock
                                        if let Ok(handlers_guard) = handlers_arc.read() {
                                            for handler in handlers_guard.iter() {
                                                if let Err(e) = handler(message.clone()) {
                                                    logger_clone.error(format!("Error in QUIC message handler: {}", e));
                                                }
                                            }
                                        }
                                        
                                        let ack = b"ACK";
                                        if let Err(e) = send_stream.write_all(ack).await {
                                            logger_clone.error(format!("Failed to send QUIC ACK: {}", e));
                                        }
                                        if let Err(e) = send_stream.finish().await {
                                             logger_clone.warn(format!("Failed to finish QUIC send stream after ACK: {}", e));
                                        }
                                    },
                                    Err(e) => {
                                        logger_clone.error(format!("Failed to deserialize QUIC message: {}", e));
                                    }
                                }
                            },
                            Err(e) => {
                                logger_clone.error(format!("Error reading QUIC receive stream: {}", e));
                            }
                        }
                    });
                },
                 Err(ConnectionError::ApplicationClosed { .. }) => {
                    self.logger.info("QUIC Connection closed by application.");
                    break;
                },
                 Err(ConnectionError::LocallyClosed) => {
                    self.logger.info("QUIC Connection closed locally.");
                    break;
                 },
                 Err(ConnectionError::TimedOut) => {
                     self.logger.warn("QUIC Connection timed out.");
                     break;
                 },
                 Err(e) => {
                    self.logger.error(format!("Error accepting QUIC stream: {}", e));
                    break; 
                }
            }
        }
        self.logger.info(format!("Finished handling QUIC connection from: {}", conn.remote_address()));
        Ok(())
    }

    /// Start the background task to clean up idle connections ONLY
    async fn start_cleanup_task(&self) -> Result<JoinHandle<()>> {
        let connections_arc: Arc<TokioRwLock<HashMap<PeerId, Arc<TokioMutex<PeerState>>>>> = Arc::clone(&self.connections);
        let callback_arc: Arc<TokioRwLock<Option<ConnectionCallback>>> = Arc::clone(&self.connection_callback);
        let logger = self.logger.clone();
        let connection_idle_timeout = Duration::from_millis(self.options.connection_idle_timeout_ms);
        // Revert: No stream timeout needed here anymore
        let check_interval = Duration::max(Duration::from_secs(5), connection_idle_timeout / 4);

        logger.info(format!(
            "Starting cleanup task: Conn Idle Timeout: {:?}, Check Interval: {:?}", // Simplified log
            connection_idle_timeout,
            check_interval
        ));

        let task = tokio::spawn(async move {
            loop {
                tokio::time::sleep(check_interval).await;
                logger.debug("Running connection cleanup check...");
                let now = Instant::now();
                let mut peers_to_remove = Vec::new();
                // Revert: Remove peers_to_update_time collection

                {
                    let connections_read = connections_arc.read().await;
                    for (peer_id, state_mutex) in connections_read.iter() {
                        // Only check connection last_used time
                        if let Ok(state) = state_mutex.try_lock() { // read lock is sufficient
                            if now.duration_since(state.last_used) > connection_idle_timeout {
                                logger.info(format!(
                                    "Peer {} connection idle ({:?}), scheduling removal.",
                                    peer_id,
                                    now.duration_since(state.last_used)
                                ));
                                peers_to_remove.push(peer_id.clone());
                            }
                        } else {
                            logger.warn(format!("Cleanup task could not lock state for peer {}, skipping check.", peer_id));
                        }
                    }
                } // connections_read lock released here

                // Revert: Remove Apply Updates block for last_used

                // Remove idle connections
                if !peers_to_remove.is_empty() {
                    let loop_callback_arc_remove = callback_arc.clone();
                    let mut connections_write = connections_arc.write().await;
                    for peer_id in peers_to_remove {
                        if let Some(state_mutex) = connections_write.remove(&peer_id) {
                           // ... close connection ...
                            Self::trigger_connection_callback(loop_callback_arc_remove.clone(), peer_id.clone(), false).await;
                        } else {
                            // ... log warning ...
                        }
                    }
                } else {
                    logger.debug("No idle connections found to remove.");
                }
            }
            // ... unreachable log ...
        });
        Ok(task)
    }

    /// Helper to trigger the connection callback if it's set
    async fn trigger_connection_callback(
        callback_arc: Arc<TokioRwLock<Option<ConnectionCallback>>>,
        peer_id: PeerId,
        is_connected: bool
    ) {
        let callback_opt = callback_arc.read().await;
        if let Some(callback) = callback_opt.as_ref() {
            // Clone the Arc<dyn Fn...> so we don't hold the lock while awaiting
            let cb = callback.clone();
            // Release the read lock
            drop(callback_opt);
            // Call the callback
            if let Err(e) = cb(peer_id.clone(), is_connected).await {
                // TODO: How to handle callback errors? Log for now.
                // Need access to logger here, maybe pass it in?
                 eprintln!("Error executing connection callback for peer {}: {}", peer_id, e); 
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
        self.options.transport_options.bind_address.to_string()
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
    /// Initializes the transport layer
    async fn initialize(&self) -> Result<(), NetworkError> {
        self.logger.info("Initializing QUIC transport...");
        let endpoint = self.setup_endpoint().await
            .map_err(|e| NetworkError::TransportError(format!("Failed to setup endpoint: {}", e)))?;
        let mut endpoint_guard = self.endpoint.lock().await;
        *endpoint_guard = Some(endpoint.clone());

        let mut server_task_guard = self.server_task.lock().await;
        if server_task_guard.is_none() {
             self.logger.info("Starting QUIC server task...");
             let server_task_handle = self.start_server_task(endpoint).await
                .map_err(|e| NetworkError::TransportError(format!("Failed to start server task: {}", e)))?;
             *server_task_guard = Some(server_task_handle);
             self.logger.info("QUIC server task started.");
        }
         drop(server_task_guard);

        let mut message_tx_guard = self.message_tx.lock().await;
        if message_tx_guard.is_none() {
            self.logger.info("Starting QUIC message sender task...");
            let sender_tx = self.start_message_sender().await
                .map_err(|e| NetworkError::TransportError(format!("Failed to start message sender task: {}", e)))?;
            *message_tx_guard = Some(sender_tx);
            self.logger.info("QUIC message sender task started.");
        }
         drop(message_tx_guard);

        let mut cleanup_task_guard = self.cleanup_task.lock().await;
        if cleanup_task_guard.is_none() {
             self.logger.info("Starting QUIC connection cleanup task...");
             let cleanup_handle = self.start_cleanup_task().await
                .map_err(|e| NetworkError::TransportError(format!("Failed to start cleanup task: {}", e)))?;
             *cleanup_task_guard = Some(cleanup_handle);
             self.logger.info("QUIC connection cleanup task started.");
        }

        self.logger.info("QUIC transport initialized successfully.");
        Ok(())
    }
    
    /// Start listening for incoming connections
    async fn start(&self) -> Result<(), NetworkError> {
        // Current implementation - call appropriate methods
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
                Err(e) => self.logger.error(format!("Error stopping QUIC cleanup task: {}", e)),
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
                 Err(e) => self.logger.error(format!("Error stopping QUIC server task: {}", e)),
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
        self.options.transport_options.bind_address.to_string()
    }
    
    /// Get the local node identifier
    fn get_local_node_id(&self) -> PeerId {
        self.node_id.clone()
    }
    
    /// Connect to a remote node at a specific address
    async fn connect(&self, target_peer_id: PeerId, target_addr: SocketAddr) -> Result<(), NetworkError> {
        self.logger.info(format!("Attempting QUIC connect to {} at {}", target_peer_id, target_addr));

        // Validate the target address
        if target_addr.port() == 0 {
            let err_msg = format!("Cannot connect to {}: invalid port 0", target_addr);
            self.logger.error(&err_msg);
            return Err(NetworkError::ConnectionError(err_msg));
        }

        // Check if already connected
        let connections_read = self.connections.read().await;
        if connections_read.contains_key(&target_peer_id) {
            self.logger.debug(format!("Already connected to peer {}", target_peer_id));
            return Ok(());
        }
        drop(connections_read);

        // Get endpoint
        let endpoint_guard = self.endpoint.lock().await;
        let endpoint = endpoint_guard.as_ref()
            .ok_or_else(|| NetworkError::TransportError("QUIC endpoint not initialized".to_string()))?;

        // Create client config
        let client_config = self.options.create_client_config(&self.logger)
            .map_err(|e| NetworkError::ConfigurationError(format!("Failed to create QUIC client config: {}", e)))?;

        // Attempt connection
        self.logger.debug(format!("Establishing QUIC connection to {} at {}", target_peer_id, target_addr));
        let connection = endpoint.connect_with(client_config, target_addr, &self.get_local_node_id().node_id)  
            .map_err(|e| NetworkError::ConnectionError(format!("Failed to initiate connection: {}", e)))?
            .await // Await the connection attempt
            .map_err(|e| NetworkError::ConnectionError(format!("QUIC connection failed for {}: {}", target_peer_id, e)))?;

        self.logger.info(format!("Successfully established QUIC connection with {} ({})", target_peer_id, connection.remote_address()));

        // --- Send Handshake --- 
        self.logger.debug(format!("Sending handshake to peer {}", target_peer_id));
        match connection.open_bi().await {
            Ok((mut send_stream, _recv_stream)) => { // We don't need recv_stream for simple handshake send
                let handshake_msg = NetworkMessage {
                    source: self.get_local_node_id(),
                    destination: target_peer_id.clone(), // Destination is the peer we connected to
                    message_type: "Handshake".to_string(),
                    payloads: Vec::new(), // No payload needed for handshake
                };

                match bincode::serialize(&handshake_msg) {
                    Ok(data) => {
                        if let Err(e) = send_stream.write_all(&data).await {
                            // Log error but don't necessarily fail the connection yet
                            self.logger.error(format!("Failed to write handshake message to {}: {}", target_peer_id, e));
                            // Close the stream potentially?
                            // let _ = send_stream.finish().await;
                        } else {
                             // Finish the stream after successful write
                             if let Err(e) = send_stream.finish().await {
                                 self.logger.warn(format!("Failed to finish handshake send stream for {}: {}", target_peer_id, e));
                             } else {
                                 self.logger.debug(format!("Handshake message sent successfully to {}", target_peer_id));
                             }
                        }
                    }
                    Err(e) => {
                        self.logger.error(format!("Failed to serialize handshake message for {}: {}", target_peer_id, e));
                    }
                }
            }
            Err(e) => {
                 // Log error but don't necessarily fail the connection yet
                 self.logger.error(format!("Failed to open stream for handshake to {}: {}", target_peer_id, e));
            }
        }
        // --- End Handshake --- 

        // Create PeerState (Initialize idle_streams pool)
        let peer_state = PeerState {
            peer_id: target_peer_id.clone(),
            connection,
            last_used: Instant::now(),
            idle_streams: TokioMutex::new(VecDeque::with_capacity(self.options.max_idle_streams_per_peer)), // Initialize pool
        };

        // Store PeerState
        let mut connections_write = self.connections.write().await;
        connections_write.insert(target_peer_id.clone(), Arc::new(TokioMutex::new(peer_state)));
        drop(connections_write);

        self.logger.debug(format!("Stored connection state for peer {}", target_peer_id));

        // Trigger callback AFTER storing state
        Self::trigger_connection_callback(self.connection_callback.clone(), target_peer_id.clone(), true).await;

        Ok(())
    }
    
    /// Disconnect from a remote node
    async fn disconnect(&self, target_peer_id: PeerId) -> Result<(), NetworkError> {
        self.logger.info(format!("Disconnecting from peer {}", target_peer_id));
        let callback_arc = self.connection_callback.clone();
        let mut connections_write = self.connections.write().await;
        
        if let Some(state_mutex) = connections_write.remove(&target_peer_id) {
            drop(connections_write);

            let state = state_mutex.lock().await;
            self.logger.debug(format!("Closing QUIC connection to {}", target_peer_id));
            // Close the connection with a specific error code and reason
            state.connection.close(0u32.into(), b"disconnect requested");
            drop(state);

            self.logger.info(format!("Successfully disconnected from peer {}", target_peer_id));
            // Trigger callback AFTER removing state and closing connection
            Self::trigger_connection_callback(callback_arc, target_peer_id.clone(), false).await;
            Ok(())
        } else {
            self.logger.warn(format!("Attempted to disconnect from non-connected peer {}", target_peer_id));
            // Not an error if already disconnected
            Ok(())
        }
    }
    
    /// Check if connected to a specific node
    fn is_connected(&self, node_id: PeerId) -> bool {
        // Use try_read instead of block_on to avoid runtime panic
        match self.connections.try_read() {
            Ok(guard) => guard.contains_key(&node_id),
            Err(_) => {
                // If we can't get the lock immediately, log a warning and return false
                self.logger.warn(format!("Failed to acquire read lock when checking connection to {}", node_id));
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
            },
            Err(_) => Err(anyhow::Error::msg("Failed to acquire write lock for message handlers").into()),
        }
    }
    
    /// Set a callback for connection status changes
    fn set_connection_callback(&self, callback: ConnectionCallback) -> Result<()> {
        let mut callback_guard = match self.connection_callback.try_write() {
             Ok(guard) => guard,
             // Using try_write to avoid blocking if called from async context inappropriately
             Err(_) => return Err(anyhow!("Failed to acquire lock for connection callback").into()),
        };
         *callback_guard = Some(callback);
         Ok(())
    }
    
    /// Send a message to a remote node
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        let tx_guard = self.message_tx.lock().await;
        if let Some(sender) = tx_guard.as_ref() {
            let target_id: PeerId = message.destination.clone();
            sender.send((message, target_id)).await 
                .map_err(|e| NetworkError::MessageError(format!("Failed to queue message for sending: {}", e)))
        } else {
            Err(NetworkError::TransportError("Message sender task not initialized".to_string()))
        }
    }
    
    /// Send a service request to a remote node
    async fn send_request(&self, message: NetworkMessage) -> Result<NetworkMessage, NetworkError> {
        // Requests should typically have one payload, extract its correlation ID
        // If multiple payloads are possible for requests, this logic needs adjustment
        let correlation_id = message.payloads.get(0)
            .map(|(_, _, corr_id)| corr_id.clone())
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
        let timeout_duration = self.options.transport_options.timeout
            .unwrap_or_else(|| Duration::from_secs(30));
        match timeout(timeout_duration, rx).await {
            Ok(response_result) => {
                match response_result {
                    Ok(response) => Ok(response),
                    Err(_) => Err(NetworkError::MessageError("Response channel was closed".to_string())),
                }
            },
            Err(_) => {
                // Clean up the pending request on timeout
                let mut pending_requests = self.pending_requests.write().await;
                pending_requests.remove(&correlation_id);
                Err(NetworkError::MessageError(format!("Request timed out after {:?}", timeout_duration)))
            }
        }
    }
    
    /// Handle an incoming network message
    async fn handle_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        // For response messages, complete the pending request for each payload
        if message.message_type == "Response" {
            let mut pending_requests = self.pending_requests.write().await;
            for (_topic, _payload, correlation_id) in &message.payloads {
                 if let Some(sender) = pending_requests.remove(correlation_id) {
                    // Clone the relevant parts of the message for this specific response payload
                    let single_payload_message = NetworkMessage {
                        source: message.source.clone(),
                        destination: message.destination.clone(), // Should be self
                        message_type: message.message_type.clone(),
                        payloads: vec![(_topic.clone(), _payload.clone(), correlation_id.clone())]
                    };
                    // Send only the relevant payload back
                    if sender.send(single_payload_message).is_err() {
                         self.logger.warn(format!("Response channel closed for correlation ID: {}", correlation_id));
                    }
                }
                 else {
                     self.logger.warn(format!("Received response for unknown correlation ID: {}", correlation_id));
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
                    self.logger.warn(&format!("Error in message handler: {}", e));
                    // Continue with other handlers
                }
            }
        }
        
        Ok(())
    }
    
    /// Start node discovery process
    async fn start_discovery(&self) -> Result<(), NetworkError> {
        self.logger.warn("start_discovery not implemented for QuicTransport");
        Ok(())
    }
    
    /// Stop node discovery process
    async fn stop_discovery(&self) -> Result<(), NetworkError> {
        self.logger.warn("stop_discovery not implemented for QuicTransport");
        Ok(())
    }
    
    /// Register a discovered node
    async fn register_discovered_node(&self, node_id: PeerId) -> Result<(), NetworkError> {
        self.logger.warn(format!("register_discovered_node not implemented for QuicTransport (called for {})", node_id));
        Ok(())
    }
    
    /// Get all discovered nodes
    fn get_discovered_nodes(&self) -> Vec<PeerId> {
        self.logger.warn("get_discovered_nodes not implemented for QuicTransport");
        Vec::new()
    }
    
    /// Set the node discovery mechanism
    fn set_node_discovery(&self, _discovery: Box<dyn NodeDiscovery>) -> Result<()> {
        Err(anyhow!("set_node_discovery not supported by QuicTransport").into())
    }
    
    /// Complete a pending request
    fn complete_pending_request(&self, correlation_id: String, response: NetworkMessage) -> Result<(), NetworkError> {
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

    /// Get a list of currently connected nodes
    fn get_connected_nodes(&self) -> Vec<PeerId> {
         // Use try_read to avoid blocking if called from non-async context
         match self.connections.try_read() {
            Ok(guard) => guard.keys().cloned().collect(),
            Err(_) => {
                self.logger.warn("Failed to acquire read lock for connections in get_connected_nodes");
                Vec::new()
            }
        }
    }
}

/// Factory for creating QUIC transport instances
#[derive(Clone)]
pub struct QuicTransportFactory {
    options: QuicTransportOptions,
    logger: Logger,
}

impl QuicTransportFactory {
    /// Create a new factory
    pub fn new(options: QuicTransportOptions, logger: Logger) -> Self {
        QuicTransportFactory { options, logger }
    }
}

#[async_trait]
impl TransportFactory for QuicTransportFactory {
    type Transport = QuicTransport;

    async fn create_transport(&self, node_id: PeerId, logger: Logger) -> Result<Self::Transport> {
        // Pass the factory's logger down, or use the provided one?
        // Let's use the one passed specifically for this transport instance.
        Ok(QuicTransport::new(node_id, self.options.clone(), logger))
    }
} 
