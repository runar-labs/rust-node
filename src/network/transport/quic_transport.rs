//! QUIC Transport Implementation
//!
//! This module implements the NetworkTransport trait using QUIC protocol.
//! It follows a layered architecture with clear separation of concerns:
//! - QuicTransport: Public API implementing NetworkTransport (thin wrapper)
//! - QuicTransportImpl: Core implementation managing connections and streams
//! - PeerState: Tracking state of individual peer connections
//! - ConnectionPool: Managing active connections and their lifecycle
//! - StreamPool: Managing stream reuse and resource cleanup

use std::collections::HashMap;
use std::fmt;
use std::net::{SocketAddr, IpAddr, Ipv4Addr};
use std::sync::{Arc, RwLock as StdRwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use bincode;
use quinn::{self, Endpoint, TransportConfig};
use quinn::{ServerConfig, ClientConfig};
use runar_common::logging::Logger;
use tokio::sync::{Mutex, RwLock, mpsc};
use tokio::task::JoinHandle;

// Import rustls explicitly - these types need clear namespacing to avoid conflicts with quinn's types
// Quinn uses rustls internally but we need to reference specific rustls types
use rustls;

/// Custom certificate verifier that skips verification for testing
/// 
/// INTENTION: Allow connections without certificate verification in test environments
struct SkipServerVerification {}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        // Skip verification and return success
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}


use super::{
    NetworkError, NetworkMessage, NetworkTransport, PeerId,
};
// Import PeerInfo and NodeInfo consistently with the module structure
use crate::network::discovery::multicast_discovery::PeerInfo;
use crate::network::discovery::NodeInfo;

/// PeerState - Manages the state of a connection to a remote peer
///
/// INTENTION: This component tracks the state of individual peer connections,
/// manages stream pools, and handles connection health.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Only accessed by ConnectionPool and QuicTransportImpl
/// - Manages its own StreamPool instance
/// - Handles connection lifecycle for a single peer
struct PeerState {
    peer_id: PeerId,
    address: String,
    stream_pool: StreamPool,
    connection: Option<quinn::Connection>,
    last_activity: std::time::Instant,
    logger: Logger,
    // Channel for notifying about connection status changes
    status_tx: mpsc::Sender<bool>,
    status_rx: Mutex<mpsc::Receiver<bool>>,
}

impl PeerState {
    /// Create a new PeerState with the specified peer ID and address
    ///
    /// INTENTION: Initialize a new peer state with the given parameters.
    fn new(peer_id: PeerId, address: String, max_idle_streams: usize, logger: Logger) -> Self {
        let (status_tx, status_rx) = mpsc::channel(10);
        
        Self {
            peer_id,
            address,
            stream_pool: StreamPool::new(max_idle_streams, logger.clone()),
            connection: None,
            last_activity: std::time::Instant::now(),
            logger,
            status_tx,
            status_rx: Mutex::new(status_rx),
        }
    }
    
    /// Set the connection for this peer
    ///
    /// INTENTION: Establish a connection to the peer and update the state.
    async fn set_connection(&mut self, connection: quinn::Connection) {
        self.connection = Some(connection);
        self.last_activity = std::time::Instant::now();
        
        // Notify about connection established
        let _ = self.status_tx.send(true).await;
        self.logger.info(&format!("Connection established with peer {}", self.peer_id));
    }
    
    /// Check if peer is connected
    ///
    /// INTENTION: Determine if there's an active connection to the peer.
    fn is_connected(&self) -> bool {
        self.connection.is_some()
    }
    
    /// Get a stream for sending messages to this peer
    ///
    /// INTENTION: Obtain a QUIC stream for sending data to this peer.
    async fn get_send_stream(&self) -> Result<quinn::SendStream, NetworkError> {
        // Try to get an idle stream from the pool first
        if let Some(stream) = self.stream_pool.get_idle_stream().await {
            return Ok(stream);
        }
        
        // Create a new stream if there are no idle streams available
        if let Some(conn) = &self.connection {
            match conn.open_uni().await {
                Ok(stream) => {
                    self.logger.debug(&format!("Opened new stream to peer {}", self.peer_id));
                    Ok(stream)
                },
                Err(e) => {
                    self.logger.error(&format!("Failed to open stream to peer {}: {}", self.peer_id, e));
                    Err(NetworkError::ConnectionError(format!("Failed to open stream: {}", e)))
                }
            }
        } else {
            Err(NetworkError::ConnectionError("Not connected to peer".to_string()))
        }
    }
    
    /// Return a stream to the pool for reuse
    ///
    /// INTENTION: Recycle streams to avoid the overhead of creating new ones.
    async fn return_stream(&self, stream: quinn::SendStream) -> Result<(), NetworkError> {
        self.stream_pool.return_stream(stream).await
    }
    
    /// Update the last activity timestamp
    ///
    /// INTENTION: Track when the peer was last active for connection management.
    fn update_activity(&mut self) {
        self.last_activity = std::time::Instant::now();
    }
    
    /// Close the connection to this peer
    ///
    /// INTENTION: Properly clean up resources when disconnecting from a peer.
    async fn close_connection(&mut self) -> Result<(), NetworkError> {
        if let Some(conn) = self.connection.take() {
            conn.close(0u32.into(), b"Connection closed by peer");
            let _ = self.status_tx.send(false).await;
            self.logger.info(&format!("Connection closed with peer {}", self.peer_id));
        }
        
        // Clear all streams in the pool
        self.stream_pool.clear().await;
        
        Ok(())
    }
}

impl fmt::Debug for PeerState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerState")
            .field("peer_id", &self.peer_id)
            .field("address", &self.address)
            .field("connection", &self.connection.is_some())
            .field("last_activity", &self.last_activity)
            .finish()
    }
}

/// StreamPool - Manages the reuse of QUIC streams
///
/// INTENTION: This component manages stream reuse, implements stream lifecycle,
/// and handles stream timeouts.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Only accessed by PeerState
/// - Manages creation, reuse, and cleanup of streams
struct StreamPool {
    idle_streams: RwLock<Vec<quinn::SendStream>>,
    max_idle_streams: usize,
    logger: Logger,
}

impl StreamPool {
    /// Create a new StreamPool with the specified maximum idle streams
    ///
    /// INTENTION: Initialize a stream pool with a capacity for idle streams reuse.
    fn new(max_idle_streams: usize, logger: Logger) -> Self {
        Self {
            idle_streams: RwLock::new(Vec::with_capacity(max_idle_streams)),
            max_idle_streams,
            logger,
        }
    }
    
    /// Get an idle stream from the pool if available
    ///
    /// INTENTION: Reuse existing streams to avoid the overhead of creating new ones.
    async fn get_idle_stream(&self) -> Option<quinn::SendStream> {
        let mut streams = self.idle_streams.write().await;
        streams.pop()
    }
    
    /// Return a stream to the pool for future reuse
    ///
    /// INTENTION: Efficiently manage QUIC stream resources.
    async fn return_stream(&self, stream: quinn::SendStream) -> Result<(), NetworkError> {
        let mut streams = self.idle_streams.write().await;
        
        // Only keep up to max_idle_streams
        if streams.len() < self.max_idle_streams {
            streams.push(stream);
            Ok(())
        } else {
            // Just let it drop if we have enough idle streams
            Ok(())
        }
    }
    
    /// Clear all idle streams in the pool
    ///
    /// INTENTION: Clean up resources when shutting down or disconnecting.
    async fn clear(&self) -> Result<(), NetworkError> {
        let mut streams = self.idle_streams.write().await;
        streams.clear();
        Ok(())
    }
}

impl fmt::Debug for StreamPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamPool")
            .field("max_idle_streams", &self.max_idle_streams)
            .finish()
    }
}

/// ConnectionPool - Manages active connections
///
/// INTENTION: This component manages active connections, handles connection reuse,
/// and implements connection cleanup.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Only accessed by QuicTransportImpl
/// - Manages PeerState instances
/// - Handles connection lifecycle across all peers
struct ConnectionPool {
    peers: RwLock<HashMap<PeerId, Arc<Mutex<PeerState>>>>,
    logger: Logger,
}

impl ConnectionPool {
    /// Create a new ConnectionPool
    ///
    /// INTENTION: Initialize a pool for managing peer connections.
    fn new(logger: Logger) -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
            logger,
        }
    }
    
    /// Get or create a peer state for the given peer ID and address
    ///
    /// INTENTION: Ensure we have a PeerState object for each peer we interact with.
    async fn get_or_create_peer(
        &self, 
        peer_id: PeerId, 
        address: String, 
        max_idle_streams: usize,
        logger: Logger
    ) -> Arc<Mutex<PeerState>> {
        let mut peers = self.peers.write().await;
        
        if !peers.contains_key(&peer_id) {
            // Create a new peer state if it doesn't exist
            let peer_state = PeerState::new(peer_id.clone(), address, max_idle_streams, logger);
            let peer_mutex = Arc::new(Mutex::new(peer_state));
            peers.insert(peer_id.clone(), peer_mutex.clone());
            peer_mutex
        } else {
            // Return the existing peer state
            peers.get(&peer_id).unwrap().clone()
        }
    }
    
    /// Get an existing peer state if it exists
    ///
    /// INTENTION: Retrieve the state for a specific peer connection.
    async fn get_peer(&self, peer_id: &PeerId) -> Option<Arc<Mutex<PeerState>>> {
        let peers = self.peers.read().await;
        peers.get(peer_id).cloned()
    }
    
    /// Remove a peer from the connection pool
    ///
    /// INTENTION: Clean up resources when a peer is disconnected.
    async fn remove_peer(&self, peer_id: &PeerId) -> Result<(), NetworkError> {
        let mut peers = self.peers.write().await;
        
        if let Some(peer_mutex) = peers.remove(peer_id) {
            // Close the connection before removing
            let mut peer = peer_mutex.lock().await;
            peer.close_connection().await?;
        }
        
        Ok(())
    }
    
    /// Check if a peer is connected
    ///
    /// INTENTION: Determine if we have an active connection to a specific peer.
    async fn is_peer_connected(&self, peer_id: &PeerId) -> bool {
        if let Some(peer_mutex) = self.get_peer(peer_id).await {
            let peer = peer_mutex.lock().await;
            peer.is_connected()
        } else {
            false
        }
    }
    
    /// Get all connected peers
    ///
    /// INTENTION: Provide information about all currently connected peers.
    async fn get_connected_peers(&self) -> Vec<PeerId> {
        let peers = self.peers.read().await;
        let mut connected_peers = Vec::new();
        
        for (peer_id, peer_mutex) in peers.iter() {
            let peer = peer_mutex.lock().await;
            if peer.is_connected() {
                connected_peers.push(peer_id.clone());
            }
        }
        
        connected_peers
    }
}

impl fmt::Debug for ConnectionPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionPool").finish()
    }
}

/// For backward compatibility with existing code
struct PeerConnection {
    peer_id: PeerId,
    address: String,
}

impl PeerConnection {
    fn new(peer_id: PeerId, address: String) -> Self {
        Self { peer_id, address }
    }
}

impl fmt::Debug for PeerConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerConnection")
            .field("peer_id", &self.peer_id)
            .field("address", &self.address)
            .finish()
    }
}

/// QuicTransportImpl - Core implementation of QUIC transport
///
/// INTENTION: This component is the core implementation of the QUIC transport,
/// managing connections and stream handling. It contains all the functional logic.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Only accessed by QuicTransport (public API wrapper)
/// - Manages ConnectionPool instance
/// - Handles core transport functionality
struct QuicTransportImpl {
    node_id: PeerId,
    bind_addr: SocketAddr,
    // Using Mutex for proper interior mutability instead of unsafe pointer casting
    endpoint: Mutex<Option<Endpoint>>,
    connection_pool: Arc<ConnectionPool>,
    options: QuicTransportOptions,
    logger: Logger,
    running: Arc<AtomicBool>,
    message_handlers: Arc<StdRwLock<Vec<Box<dyn Fn(NetworkMessage) -> Result<(), NetworkError> + Send + Sync + 'static>>>>,
    // Background tasks for connection handling and message processing
    background_tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// Main QUIC transport implementation - Public API
///
/// INTENTION: This component provides the public API implementing NetworkTransport.
/// It is a thin wrapper around QuicTransportImpl which contains the actual logic.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Exposes NetworkTransport trait to external callers
/// - Delegates all functionality to QuicTransportImpl
/// - Manages the lifecycle of the implementation
pub struct QuicTransport {
    // Internal implementation containing the actual logic
    inner: Arc<QuicTransportImpl>,
    // Keep logger and node_id at this level for compatibility
    logger: Logger,
    node_id: PeerId,
}

// This function is no longer needed as we've integrated its functionality directly into create_quinn_configs

/// QUIC-specific transport options
#[derive(Debug, Clone)]
pub struct QuicTransportOptions {
    verify_certificates: bool,
    keep_alive_interval: Duration,
    connection_idle_timeout: Duration,
    stream_idle_timeout: Duration,
    max_idle_streams_per_peer: usize,
    /// Optional TLS certificates for secure connections
    certificates: Option<Vec<rustls::Certificate>>,
    /// Optional private key for the certificate
    private_key: Option<rustls::PrivateKey>,
    /// Optional path to certificate file
    cert_path: Option<String>,
}

impl QuicTransportOptions {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn with_verify_certificates(mut self, verify: bool) -> Self {
        self.verify_certificates = verify;
        self
    }
    
    pub fn with_keep_alive_interval(mut self, interval: Duration) -> Self {
        self.keep_alive_interval = interval;
        self
    }
    
    pub fn with_connection_idle_timeout(mut self, timeout: Duration) -> Self {
        self.connection_idle_timeout = timeout;
        self
    }
    
    pub fn with_stream_idle_timeout(mut self, timeout: Duration) -> Self {
        self.stream_idle_timeout = timeout;
        self
    }
    
    pub fn with_max_idle_streams_per_peer(mut self, max_streams: usize) -> Self {
        self.max_idle_streams_per_peer = max_streams;
        self
    }
    
    pub fn with_certificates(mut self, certs: Vec<rustls::Certificate>) -> Self {
        self.certificates = Some(certs);
        self
    }
    
    pub fn with_private_key(mut self, key: rustls::PrivateKey) -> Self {
        self.private_key = Some(key);
        self
    }
    
    pub fn with_cert_path(mut self, path: String) -> Self {
        self.cert_path = Some(path);
        self
    }
    
    pub fn certificates(&self) -> Option<&Vec<rustls::Certificate>> {
        self.certificates.as_ref()
    }
    
    pub fn private_key(&self) -> Option<&rustls::PrivateKey> {
        self.private_key.as_ref()
    }
    
    pub fn cert_path(&self) -> Option<&str> {
        self.cert_path.as_deref()
    }
}

impl Default for QuicTransportOptions {
    fn default() -> Self {
        Self {
            verify_certificates: true,
            keep_alive_interval: Duration::from_millis(5000),
            connection_idle_timeout: Duration::from_secs(30),
            stream_idle_timeout: Duration::from_secs(10),
            max_idle_streams_per_peer: 10,
            certificates: None,
            private_key: None,
            cert_path: None,
        }
    }
}

// Implement Debug for QuicTransportImpl
impl fmt::Debug for QuicTransportImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // We can't access the Mutex in fmt because it might block, so we just indicate it exists
        f.debug_struct("QuicTransportImpl")
            .field("node_id", &self.node_id)
            .field("bind_addr", &self.bind_addr)
            .field("endpoint", &"<mutex>") // Can't access Mutex contents in fmt
            .field("options", &self.options)
            .field("running", &self.running.load(Ordering::Relaxed))
            .finish()
    }
}

impl fmt::Debug for QuicTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuicTransport")
            .field("node_id", &self.node_id)
            .field("inner", &self.inner)
            .finish()
    }
}

impl QuicTransportImpl {
    /// Create a new QuicTransportImpl instance
    ///
    /// INTENTION: Initialize the core implementation with the provided parameters.
    fn new(
        node_id: PeerId,
        bind_addr: SocketAddr,
        options: QuicTransportOptions,
        logger: Logger,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let connection_pool = Arc::new(ConnectionPool::new(logger.clone()));
        
        Ok(Self {
            node_id,
            bind_addr,
            // Initialize with Mutex for proper interior mutability
            endpoint: Mutex::new(None),
            connection_pool,
            options,
            logger,
            running: Arc::new(AtomicBool::new(false)),
            message_handlers: Arc::new(StdRwLock::new(Vec::new())),
            background_tasks: Mutex::new(Vec::new()),
        })
    }
    
    /// Configure and create a QUIC endpoint
    ///
    /// INTENTION: Set up the QUIC endpoint with appropriate TLS and transport settings.
    async fn configure_endpoint(&self) -> Result<Endpoint, NetworkError> {
        // Configure TLS for the endpoint
        let (server_config, client_config) = self.create_quinn_configs()?;
        
        // Create the endpoint
        let mut endpoint = quinn::Endpoint::server(server_config, self.bind_addr)
            .map_err(|e| NetworkError::TransportError(format!("Failed to create QUIC endpoint: {}", e)))?;
        
        // Set default client config for outgoing connections
        endpoint.set_default_client_config(client_config);
        
        self.logger.info(&format!("QUIC endpoint configured on {}", self.bind_addr));
        Ok(endpoint)
    }
    
    /// Create QUIC server and client configurations
    ///
    /// INTENTION: Set up the TLS and transport configurations for QUIC connections.
    fn create_quinn_configs(&self) -> Result<(ServerConfig, ClientConfig), NetworkError> {
        // INTENTION: Create a transport config with desired parameters from our options
        let mut transport_config = TransportConfig::default();
        
        // Configure QUIC transport parameters based on our options with proper type conversions
        transport_config.max_concurrent_uni_streams((self.options.max_idle_streams_per_peer as u32).into());
        transport_config.keep_alive_interval(Some(self.options.keep_alive_interval));
        
        // Convert Duration to IdleTimeout for max_idle_timeout
        // Quinn expects milliseconds as a VarInt
        let millis = self.options.connection_idle_timeout.as_millis();
        if millis <= u64::MAX as u128 {
            let timeout_ms = quinn::VarInt::from_u64(millis as u64)
                .unwrap_or(quinn::VarInt::MAX);
            transport_config.max_idle_timeout(Some(timeout_ms.into()));
        } else {
            // If the duration is too large, use the maximum allowed value
            transport_config.max_idle_timeout(Some(quinn::IdleTimeout::from(quinn::VarInt::MAX)));
        }
        
        // Convert to Arc for sharing between configs
        let transport_config = Arc::new(transport_config);
        
        // Generate a self-signed certificate for testing
        let (cert_chain, priv_key) = self.generate_self_signed_cert();
        
        // Create server config using Quinn's API
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(cert_chain.clone(), priv_key.clone())
            .map_err(|e| NetworkError::ConfigurationError(format!("Failed to create server crypto config: {}", e)))?;
        
        // Set ALPN protocols for the server
        server_crypto.alpn_protocols = vec![b"quic-transport".to_vec()];
        
        // Create server config with the crypto configuration
        let mut server_config = ServerConfig::with_crypto(Arc::new(server_crypto));
        
        // Create a client crypto configuration
        let mut client_crypto = rustls::ClientConfig::builder()
            .with_safe_defaults()
            .with_custom_certificate_verifier(Arc::new(SkipServerVerification {}))
            .with_client_auth_cert(cert_chain.clone(), priv_key.clone())
            .map_err(|e| NetworkError::ConfigurationError(format!("Failed to create client crypto config: {}", e)))?;
        
        // Set ALPN protocols for the client
        client_crypto.alpn_protocols = vec![b"quic-transport".to_vec()];
        
        // Create client config with the crypto configuration
        let mut client_config = ClientConfig::new(Arc::new(client_crypto));
        
        // Apply transport configurations to both server and client
        server_config.transport_config(transport_config.clone());
        client_config.transport_config(transport_config);
        
        Ok((server_config, client_config))
    }
    

    
    /// Generate a self-signed certificate for testing
    ///
    /// INTENTION: Create a certificate for secure connections in test environments
    fn generate_self_signed_cert(&self) -> (Vec<rustls::Certificate>, rustls::PrivateKey) {
        // Create certificate parameters with sensible defaults
        let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]);
        
        // Set the certificate to be valid for a reasonable time (for testing)
        params.not_before = rcgen::date_time_ymd(2020, 1, 1);
        params.not_after = rcgen::date_time_ymd(2030, 1, 1);
        
        // Generate the certificate with our parameters
        let cert = rcgen::Certificate::from_params(params)
            .expect("Failed to generate certificate");
            
        // Get the DER encoded certificate and private key
        let cert_der = cert.serialize_der().expect("Failed to serialize certificate");
        let key_der = cert.serialize_private_key_der();
        
        // Convert to rustls types with explicit namespace qualification
        let rustls_cert = rustls::Certificate(cert_der);
        let rustls_key = rustls::PrivateKey(key_der);
        
        (vec![rustls_cert], rustls_key)
    }
    
    /// Create test certificates for use in test environments
    ///
    /// INTENTION: Generate a self-signed certificate for testing purposes
    #[cfg(test)]
    fn create_test_certificates(&self) -> Result<(Vec<rustls::Certificate>, rustls::PrivateKey), NetworkError> {
        // Use rcgen to generate a self-signed certificate
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| NetworkError::ConfigurationError(format!("Failed to generate certificate: {}", e)))?;
        
        // Serialize the certificate to DER format
        let cert_der = cert.serialize_der()
            .map_err(|e| NetworkError::ConfigurationError(format!("Failed to serialize certificate: {}", e)))?;
        let key_der = cert.serialize_private_key_der();
        
        // Convert to rustls types with explicit namespace
        let rustls_cert = rustls::Certificate(cert_der);
        let rustls_key = rustls::PrivateKey(key_der);
        
        // Return a vector of certificates as required by rustls API
        Ok((vec![rustls_cert], rustls_key))
    }
    
    // The server-side TLS configuration is now handled in create_quinn_configs
    
    // The client-side TLS configuration is now handled in create_quinn_configs
    
    // The self-signed certificate generation is handled in test code only
    
    /// Start the QUIC transport
    ///
    /// INTENTION: Initialize the endpoint and start accepting connections.
    async fn start(&self) -> Result<(), NetworkError> {
        if self.running.load(Ordering::Relaxed) {
            return Ok(());
        }
        
        self.logger.info(&format!("Starting QUIC transport on {}", self.bind_addr));
        
        // Create configurations for the QUIC endpoint
        let (server_config, client_config) = self.create_quinn_configs()?;
        
        // Create the endpoint with the server configuration
        // Bind to 0.0.0.0 instead of 127.0.0.1 to allow connections from any interface
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), self.bind_addr.port());
        self.logger.info(&format!("Creating endpoint bound to {}", bind_addr));
        
        let mut endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|e| NetworkError::TransportError(format!("Failed to create endpoint: {}", e)))?;
        
        // Set client configuration for outgoing connections
        endpoint.set_default_client_config(client_config);
        
        self.logger.info(&format!("Endpoint created successfully with server and client configs"));
        
        // Store the endpoint in our state using proper interior mutability pattern
        // INTENTION: Replace unsafe pointer casting with a safe alternative
        let mut endpoint_guard = self.endpoint.lock().await;
        *endpoint_guard = Some(endpoint.clone());
        
        // Spawn a task to accept incoming connections
        let self_clone = Arc::new(self.clone());
        let task = tokio::spawn(async move {
            self_clone.accept_connections(endpoint).await;
        });
        
        // Store the task handle
        let mut tasks = self.background_tasks.lock().await;
        tasks.push(task);
        
        self.running.store(true, Ordering::Relaxed);
        self.logger.info("QUIC transport started successfully");
        Ok(())
    }
    
    /// Accept incoming connections
    ///
    /// INTENTION: Listen for and handle incoming QUIC connections.
    async fn accept_connections(&self, endpoint: Endpoint) {
        self.logger.info("Accepting incoming connections");
        
        while self.running.load(Ordering::Relaxed) {
            match endpoint.accept().await {
                Some(connecting) => {
                    let self_clone = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = self_clone.handle_new_connection(connecting).await {
                            self_clone.logger.error(&format!("Error handling connection: {}", e));
                        }
                    });
                },
                None => {
                    // Endpoint is closed
                    self.logger.info("Endpoint closed, no longer accepting connections");
                    break;
                }
            }
        }
    }
    
    /// Stop the QUIC transport
    ///
    /// INTENTION: Gracefully shut down the transport and clean up resources.
    async fn stop(&self) -> Result<(), NetworkError> {
        if !self.running.load(Ordering::Relaxed) {
            return Ok(());
        }
        
        self.running.store(false, Ordering::Relaxed);
        
        // Close the endpoint - using proper Mutex access pattern
        let endpoint_guard = self.endpoint.lock().await;
        if let Some(endpoint) = &*endpoint_guard {
            endpoint.close(0u32.into(), b"Transport stopped");
        }
        
        // Wait for background tasks to complete
        let mut tasks = self.background_tasks.lock().await;
        for task in tasks.drain(..) {
            // We don't care about the result, just wait for it to finish
            let _ = task.await;
        }
        
        self.logger.info("QUIC transport stopped");
        Ok(())
    }
    
    /// Disconnect from a peer
    ///
    /// INTENTION: Properly clean up resources when disconnecting from a peer.
    async fn disconnect(&self, peer_id: PeerId) -> Result<(), NetworkError> {
        if !self.running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Remove the peer from the connection pool
        self.connection_pool.remove_peer(&peer_id).await
    }
    
    /// Check if connected to a specific peer
    ///
    /// INTENTION: Determine if there's an active connection to the specified peer.
    fn is_connected(&self, peer_id: PeerId) -> bool {
        // We need to use block_in_place because this is a sync function
        // but we need to perform async operations
        tokio::task::block_in_place(move || {
            tokio::runtime::Handle::current().block_on(async {
                self.connection_pool.is_peer_connected(&peer_id).await
            })
        })
    }
    
    /// Send a message to a peer
    ///
    /// INTENTION: Serialize and send a message to a specified peer.
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        if !self.running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Use the destination field to determine the peer to send to
        let peer_id = &message.destination;
        
        // Get the peer state
        let peer_state = match self.connection_pool.get_peer(peer_id).await {
            Some(state) => state,
            None => return Err(NetworkError::ConnectionError(format!("Peer {} not found", peer_id))),
        };
        
        // Check if the peer is connected
        let peer = peer_state.lock().await;
        if !peer.is_connected() {
            return Err(NetworkError::ConnectionError(format!("Not connected to peer {}", peer_id)));
        }
        
        // Get a stream for sending the message
        let mut stream = peer.get_send_stream().await?;
        
        // Serialize the message
        let data = bincode::serialize(&message)
            .map_err(|e| NetworkError::MessageError(format!("Failed to serialize message: {}", e)))?;
        
        // Write the message length first (4 bytes), then the message data
        let len = data.len() as u32;
        stream.write_all(&len.to_be_bytes()).await
            .map_err(|e| NetworkError::MessageError(format!("Failed to write message length: {}", e)))?;
        
        stream.write_all(&data).await
            .map_err(|e| NetworkError::MessageError(format!("Failed to write message data: {}", e)))?;
        
        // Finish the stream
        stream.finish().await
            .map_err(|e| NetworkError::MessageError(format!("Failed to finish stream: {}", e)))?;
        
        self.logger.debug(&format!("Sent message to peer {}", peer_id));
        Ok(())
    }
    
    /// Connect to a peer using discovery information
    ///
    /// INTENTION: Establish a connection with a peer discovered via the discovery mechanism.
    async fn connect_peer(
        &self,
        discovery_msg: PeerInfo,
        _local_node: NodeInfo, // Mark as unused with underscore prefix
    ) -> Result<(), NetworkError> {
        if !self.running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Get the peer ID based on the public_key from PeerInfo
        // This is a simplification for testing - in production we'd have a more robust
        // mechanism for deriving peer IDs from public keys
        let peer_id = PeerId::new(discovery_msg.public_key.clone());
        
        self.logger.info(&format!("Attempting to connect to peer {} with addresses: {:?}", peer_id, discovery_msg.addresses));
        
        // Check if we're already connected to this peer
        if self.connection_pool.is_peer_connected(&peer_id).await {
            self.logger.debug(&format!("Already connected to peer {}", peer_id));
            return Ok(());
        }
        
        // Get the peer's address from the discovery message
        let peer_addr = match discovery_msg.addresses.first() {
            Some(addr) => addr,
            None => return Err(NetworkError::ConnectionError("No address found for peer".to_string())),
        };
        
        // Parse the address
        let socket_addr = match peer_addr.parse::<SocketAddr>() {
            Ok(addr) => addr,
            Err(e) => return Err(NetworkError::ConnectionError(format!("Invalid address: {}", e))),
        };
        
        // Get the endpoint
        let endpoint_guard = self.endpoint.lock().await;
        let endpoint = match &*endpoint_guard {
            Some(endpoint) => endpoint.clone(),
            None => return Err(NetworkError::ConnectionError("Endpoint not initialized".to_string())),
        };
        drop(endpoint_guard); // Release the lock as early as possible
        
        // Connect to the peer
        self.logger.info(&format!("Connecting to peer {} at {}", peer_id, socket_addr));
        
        // Print detailed connection information for debugging
        self.logger.info(&format!("Detailed connection attempt - Local node: {}, Remote peer: {}, Socket: {}", 
                                 self.node_id, peer_id, socket_addr));
        
        // Create a new connection to the peer
        // For testing, we use "localhost" as the server name to avoid certificate validation issues
        // In production, we would use the peer_id or a proper domain name
        let connect_result = endpoint.connect(socket_addr, "localhost");
        
        match connect_result {
            Ok(connecting) => {
                // Wait for the connection to be established
                match connecting.await {
                    Ok(connection) => {
                        self.logger.info(&format!("Connected to peer {} at {}", peer_id, socket_addr));
                        
                        // Get or create the peer state
                        let peer_state = self.connection_pool.get_or_create_peer(
                            peer_id.clone(),
                            peer_addr.clone(),
                            self.options.max_idle_streams_per_peer,
                            self.logger.clone(),
                        ).await;
                        
                        // Set the connection in the peer state
                        let mut peer = peer_state.lock().await;
                        peer.set_connection(connection).await;
                        drop(peer); // Explicitly drop the lock
                        
                        // Start a task to receive incoming messages
                        self.spawn_message_receiver(peer_id.clone(), peer_state.clone()).await;
                        
                        // Verify the connection is properly registered
                        let is_connected = self.connection_pool.is_peer_connected(&peer_id).await;
                        self.logger.info(&format!("Connection verification for {}: {}", peer_id, is_connected));
                        
                        Ok(())
                    },
                    Err(e) => {
                        self.logger.error(&format!("Failed to connect to peer {}: {}", peer_id, e));
                        Err(NetworkError::ConnectionError(format!("Failed to establish connection: {}", e)))
                    }
                }
            },
            Err(e) => Err(NetworkError::ConnectionError(format!("Failed to initiate connection: {}", e))),
        }
    }
    
    fn get_local_address(&self) -> String {
        self.bind_addr.to_string()
    }
    
    
    async fn register_message_handler(
        &self,
        handler: Box<dyn Fn(NetworkMessage) -> Result<(), NetworkError> + Send + Sync + 'static>,
    ) -> Result<(), NetworkError> {
        // Get a write lock and push the handler
        match self.message_handlers.write() {
            Ok(mut handlers) => {
                handlers.push(handler);
                Ok(())
            },
            Err(_) => Err(NetworkError::ConnectionError("Failed to acquire write lock".to_string()))
        }
    }
    
    /// Process an incoming message
    ///
    /// INTENTION: Route an incoming message to registered handlers.
    async fn process_incoming_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        self.logger.debug(&format!("Processing message from {}", message.source));
        
        // Get a read lock on the handlers
        match self.message_handlers.read() {
            Ok(handlers) => {
                // Call each registered handler
                for handler in handlers.iter() {
                    if let Err(e) = handler(message.clone()) {
                        self.logger.error(&format!("Error in message handler: {}", e));
                    }
                }
                Ok(())
            },
            Err(_) => Err(NetworkError::TransportError("Failed to acquire read lock on message handlers".to_string())),
        }
    }
    
    /// Handle a new incoming connection
    ///
    /// INTENTION: Process an incoming connection request and set up the connection state.
    async fn handle_new_connection(
        &self,
        conn: quinn::Connecting,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.logger.debug("Handling new incoming connection");
        
        // Wait for the connection to be established
        let connection = conn.await?;
        
        // Get connection info
        let remote_addr = connection.remote_address();
        
        self.logger.info(&format!("New incoming connection from {}", remote_addr));
        
        // Create a temporary peer ID for this connection
        // In a real implementation, we would validate the peer ID from a handshake message
        let peer_id = PeerId::new(format!("temp-{}", remote_addr));
        
        // Get or create the peer state
        let peer_state = self.connection_pool.get_or_create_peer(
            peer_id.clone(),
            remote_addr.to_string(),
            self.options.max_idle_streams_per_peer,
            self.logger.clone(),
        ).await;
        
        // Set the connection in the peer state
        let mut peer = peer_state.lock().await;
        peer.set_connection(connection).await;
        
        // Spawn a task to receive incoming messages
        self.spawn_message_receiver(peer_id.clone(), peer_state.clone()).await;
        
        Ok(())
    }
    
    async fn spawn_message_receiver(&self, peer_id: PeerId, peer_state: Arc<Mutex<PeerState>>) {
        let self_clone = self.clone();
        
        let task = tokio::spawn(async move {
            loop {
                // Get the connection from the peer state
                let connection = {
                    let peer = peer_state.lock().await;
                    if let Some(conn) = &peer.connection {
                        conn.clone()
                    } else {
                        // Connection is gone, exit the loop
                        break;
                    }
                };
                
                // Accept an incoming stream
                match connection.accept_uni().await {
                    Ok(stream) => {
                        // Process the incoming message
                        if let Err(e) = self_clone.receive_message(peer_id.clone(), stream).await {
                            self_clone.logger.error(&format!("Error receiving message from {}: {}", peer_id, e));
                        }
                    },
                    Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                        // Connection closed by the peer, exit the loop
                        self_clone.logger.info(&format!("Connection closed by peer {}", peer_id));
                        break;
                    },
                    Err(e) => {
                        // Other connection error
                        self_clone.logger.error(&format!("Connection error from {}: {}", peer_id, e));
                        break;
                    }
                }
            }
            
            // Connection is closed, clean up the peer state
            let mut peer = peer_state.lock().await;
            let _ = peer.close_connection().await;
        });
        
        // Store the task handle
        let mut tasks = self.background_tasks.lock().await;
        tasks.push(task);
    }
    
    /// Receive and process a message from a peer
    ///
    /// INTENTION: Handle the deserialization and processing of incoming messages.
    async fn receive_message(&self, peer_id: PeerId, mut stream: quinn::RecvStream) -> Result<(), NetworkError> {
        // Read the message length (4 bytes)
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await
            .map_err(|e| NetworkError::MessageError(format!("Failed to read message length: {}", e)))?;
        
        let msg_len = u32::from_be_bytes(len_bytes) as usize;
        
        // Read the message data
        let mut buffer = vec![0u8; msg_len];
        stream.read_exact(&mut buffer).await
            .map_err(|e| NetworkError::MessageError(format!("Failed to read message data: {}", e)))?;
        
        // Deserialize the message
        let message: NetworkMessage = bincode::deserialize(&buffer)
            .map_err(|e| NetworkError::MessageError(format!("Failed to deserialize message: {}", e)))?;
        
        // Log the received message details
        self.logger.debug(&format!("Received message from {} (connected as {})", message.source, peer_id));
        
        // Process the message
        self.process_incoming_message(message).await
    }
    
    /// Get or establish a connection to a peer
    ///
    /// INTENTION: Ensure there's a valid connection to the specified peer.
    async fn get_or_connect(
        &self,
        peer_id: &PeerId,
    ) -> Result<Arc<PeerConnection>, NetworkError> {
        // Check if we're already connected to this peer
        if self.connection_pool.is_peer_connected(peer_id).await {
            // Get the peer state
            if let Some(peer_state) = self.connection_pool.get_peer(peer_id).await {
                let peer = peer_state.lock().await;
                return Ok(Arc::new(PeerConnection::new(peer_id.clone(), peer.address.clone())));
            }
        }
        
        // We're not connected, so we can't get a connection
        Err(NetworkError::ConnectionError(format!("Not connected to peer {}", peer_id)))
    }
    
    /// Clone implementation for QuicTransportImpl
    ///
    /// INTENTION: Allow cloning of QuicTransportImpl for use in async tasks.
    fn clone(&self) -> Self {
        Self {
            node_id: self.node_id.clone(),
            bind_addr: self.bind_addr,
            // Clone the Mutex itself, not its contents
            endpoint: Mutex::new(None), // Initialize as None, will be set if needed
            connection_pool: self.connection_pool.clone(),
            options: self.options.clone(),
            logger: self.logger.clone(),
            running: self.running.clone(),
            message_handlers: self.message_handlers.clone(),
            background_tasks: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl NetworkTransport for QuicTransport {
    async fn start(&self) -> Result<(), NetworkError> {
        self.inner.start().await
    }
    
    async fn stop(&self) -> Result<(), NetworkError> {
        self.inner.stop().await
    }
    
    async fn disconnect(&self, peer_id: PeerId) -> Result<(), NetworkError> {
        self.inner.disconnect(peer_id).await
    }
    
    fn is_connected(&self, peer_id: PeerId) -> bool {
        self.inner.is_connected(peer_id)
    }
    
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        self.inner.send_message(message).await
    }
    
    async fn connect_peer(
        &self,
        discovery_msg: PeerInfo,
        _local_node: NodeInfo, // Mark as unused with underscore prefix
    ) -> Result<(), NetworkError> {
        self.inner.connect_peer(discovery_msg, _local_node).await
    }
    
    fn get_local_address(&self) -> String {
        self.inner.get_local_address()
    }
    
    async fn register_message_handler(
        &self,
        handler: Box<dyn Fn(NetworkMessage) -> Result<(), NetworkError> + Send + Sync + 'static>,
    ) -> Result<(), NetworkError> {
        self.inner.register_message_handler(handler).await
    }
}

impl QuicTransport {
    /// Create a new QuicTransport instance
    ///
    /// INTENTION: Create a new QuicTransport with the given node ID, bind address,
    /// options, and logger. This is the primary constructor for QuicTransport.
    pub fn new(
        node_id: PeerId,
        bind_addr: SocketAddr,
        options: QuicTransportOptions,
        logger: Logger,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Create the inner implementation
        let inner = QuicTransportImpl::new(node_id.clone(), bind_addr, options, logger.clone())?;
        
        // Create and return the public API wrapper
        Ok(Self {
            inner: Arc::new(inner),
            logger,
            node_id,
        })
    }
}
