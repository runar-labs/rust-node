//! QUIC Transport Implementation
//!
//! This module implements the NetworkTransport trait using QUIC protocol.
//! It follows a layered architecture with clear separation of concerns:
//! - QuicTransport: Public API implementing NetworkTransport (thin wrapper)
//! - QuicTransportImpl: Core implementation managing connections and streams
//! - PeerState: Tracking state of individual peer connections
//! - ConnectionPool: Managing active connections and their lifecycle
//! - StreamPool: Managing stream reuse and resource cleanup

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
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

// Import rustls explicitly - these types need clear namespacing to avoid conflicts with quinn's types
// Quinn uses rustls internally but we need to reference specific rustls types
use rustls;


use super::{
    ConnectionPool, NetworkError, NetworkMessage, NetworkMessagePayloadItem, NetworkTransport, PeerId, PeerState
};
// Import PeerInfo and NodeInfo consistently with the module structure
use crate::network::discovery::multicast_discovery::PeerInfo;
use crate::network::discovery::NodeInfo;
  
/// QuicTransportImpl - Core implementation of QUIC transport
///
/// INTENTION: This component is the core implementation of the QUIC transport,
/// managing connections and stream handling. It contains all the protocol-specific logic.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Only accessed by QuicTransport (public API wrapper) through Arc
/// - Never cloned directly, only the Arc is cloned
/// - Manages ConnectionPool instance and peer states
/// - Handles protocol-specific logic and connection management
/// - Does not manage threads, tasks, or public API surface
/// - Returns task handles to QuicTransport for lifecycle management
struct QuicTransportImpl {
    node_id: PeerId,
    bind_addr: SocketAddr,
    // Using Mutex for proper interior mutability instead of unsafe pointer casting
    endpoint: Mutex<Option<Endpoint>>,
    connection_pool: Arc<ConnectionPool>,
    options: QuicTransportOptions,
    logger: Arc<Logger>,
    message_handlers: Arc<StdRwLock<Vec<Box<dyn Fn(NetworkMessage) -> Result<(), NetworkError> + Send + Sync + 'static>>>>,
}

/// Main QUIC transport implementation - Public API
///
/// INTENTION: This component provides the public API implementing NetworkTransport.
/// It is a thin wrapper around QuicTransportImpl which contains the actual logic.
///
/// ARCHITECTURAL BOUNDARIES:
/// - Exposes NetworkTransport trait to external callers
/// - Delegates protocol functionality to QuicTransportImpl
/// - Manages the lifecycle of the implementation
/// - Responsible for thread/task management
pub struct QuicTransport {
    // Internal implementation containing the actual logic
    inner: Arc<QuicTransportImpl>,
    // Keep logger and node_id at this level for compatibility
    logger: Logger,
    node_id: PeerId,
    // Background tasks for connection handling and message processing
    background_tasks: Mutex<Vec<JoinHandle<()>>>,
    // Track if the transport is running
    running: Arc<AtomicBool>,
}

// This function is no longer needed as we've integrated its functionality directly into create_quinn_configs

/// QUIC-specific transport options
#[derive(Clone)]
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
    // Custom certificate verifier for client connections
    certificate_verifier: Option<Arc<dyn rustls::client::ServerCertVerifier + Send + Sync>>,
}

impl fmt::Debug for QuicTransportOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QuicTransportOptions")
            .field("verify_certificates", &self.verify_certificates)
            .field("keep_alive_interval", &self.keep_alive_interval)
            .field("connection_idle_timeout", &self.connection_idle_timeout)
            .field("stream_idle_timeout", &self.stream_idle_timeout)
            .field("max_idle_streams_per_peer", &self.max_idle_streams_per_peer)
            .field("certificates", &self.certificates)
            .field("private_key", &self.private_key)
            .field("cert_path", &self.cert_path)
            .field("certificate_verifier", &"<certificate verifier>")
            .finish()
    }
}

/// Helper function to generate self-signed certificates for testing
///
/// INTENTION: Provide a consistent way to generate test certificates across test and core code, using explicit rustls namespaces to avoid type conflicts.
pub(crate) fn generate_test_certificates() -> (Vec<rustls::Certificate>, rustls::PrivateKey) {
    use rcgen;
    use rustls;
    // Create certificate parameters with default values
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]);
    params.alg = &rcgen::PKCS_ECDSA_P256_SHA256;
    params.not_before = rcgen::date_time_ymd(2023, 1, 1);
    params.not_after = rcgen::date_time_ymd(2026, 1, 1);

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

impl QuicTransportOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder: Attach test certificates for use in test environments.
    ///
    /// In production, certificates must be provided by the node. In tests, this method
    /// attaches self-signed certificates for convenience. This is a temporary measure.
    ///
    /// # Example
    /// ```rust
    /// let opts = QuicTransportOptions::new().with_test_certificates();
    /// ```
    pub fn with_test_certificates(mut self) -> Self {
        let (certs, key) = generate_test_certificates();
        self.certificates = Some(certs);
        self.private_key = Some(key);
        self
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
    
    /// Builder: Set a custom certificate verifier for client connections
    ///
    /// INTENTION: Allow custom certificate verification logic, primarily for testing
    /// 
    /// # Example
    /// ```rust
    /// let verifier = Arc::new(CustomVerifier {});
    /// let opts = QuicTransportOptions::new().with_certificate_verifier(verifier);
    /// ```
    pub fn with_certificate_verifier(mut self, verifier: Arc<dyn rustls::client::ServerCertVerifier + Send + Sync>) -> Self {
        self.certificate_verifier = Some(verifier);
        self
    }
    
    pub fn certificate_verifier(&self) -> Option<&Arc<dyn rustls::client::ServerCertVerifier + Send + Sync>> {
        self.certificate_verifier.as_ref()
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
            certificate_verifier: None,
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
            logger:Arc::new(logger),
            message_handlers: Arc::new(StdRwLock::new(Vec::new())),
        })
    }
    
    /// Configure and create a QUIC endpoint
    ///
    /// INTENTION: Set up the QUIC endpoint with appropriate TLS and transport settings.
    async fn configure_endpoint(self: &Arc<Self>) -> Result<Endpoint, NetworkError> {
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
    fn create_quinn_configs(self: &Arc<Self>) -> Result<(ServerConfig, ClientConfig), NetworkError> {
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
        
        // Get certificates from options
        let (cert_chain, priv_key) = match (self.options.certificates(), self.options.private_key()) {
            (Some(certs), Some(key)) => (certs.clone(), key.clone()),
            _ => return Err(NetworkError::ConfigurationError(
                "Certificates and private key must be provided in QuicTransportOptions".to_string()))
        };
        
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
        let client_crypto_builder = rustls::ClientConfig::builder()
            .with_safe_defaults();
            
        // Use custom certificate verifier if provided, otherwise use default verification
        let client_crypto = if let Some(verifier) = self.options.certificate_verifier() {
            client_crypto_builder
                .with_custom_certificate_verifier(verifier.clone())
                .with_client_auth_cert(cert_chain.clone(), priv_key.clone())
        } else if !self.options.verify_certificates {
            // If verification is disabled but no custom verifier is provided,
            // we need to create a simple verifier that accepts all certificates
            // This is only for testing and should not be used in production
            let mut root_store = rustls::RootCertStore::empty();
            // Add our self-signed cert to the root store
            for cert in cert_chain.iter() {
                root_store.add(cert).map_err(|e| NetworkError::ConfigurationError(
                    format!("Failed to add certificate to root store: {}", e)))?
            }
            
            // With no-verify mode, we still need to provide certificates but we'll accept any
            client_crypto_builder
                .with_root_certificates(root_store)
                .with_client_auth_cert(cert_chain.clone(), priv_key.clone())
        } else {
            // Use default verification with provided certificates
            let mut root_store = rustls::RootCertStore::empty();
            for cert in cert_chain.iter() {
                root_store.add(cert).map_err(|e| NetworkError::ConfigurationError(
                    format!("Failed to add certificate to root store: {}", e)))?
            }
            
            client_crypto_builder
                .with_root_certificates(root_store)
                .with_client_auth_cert(cert_chain.clone(), priv_key.clone())
        }
        .map_err(|e| NetworkError::ConfigurationError(format!("Failed to create client crypto config: {}", e)))?;
        
        let mut client_crypto = client_crypto;
        
        // Set ALPN protocols for the client
        client_crypto.alpn_protocols = vec![b"quic-transport".to_vec()];
        
        // Create client config with the crypto configuration
        let mut client_config = ClientConfig::new(Arc::new(client_crypto));
        
        // Apply transport configurations to both server and client
        server_config.transport_config(transport_config.clone());
        client_config.transport_config(transport_config);
        
        Ok((server_config, client_config))
    }
    

    
    // Certificate generation has been moved to the test file
    
    // Test certificate generation has been moved to the test file
    
    // The server-side TLS configuration is now handled in create_quinn_configs
    
    // The client-side TLS configuration is now handled in create_quinn_configs
    
    // The self-signed certificate generation is handled in test code only
    
    /// Start the QUIC transport
    ///
    /// INTENTION: Initialize the endpoint and start accepting connections.
    async fn start(self: &Arc<Self>, running: &Arc<AtomicBool>, background_tasks: &Mutex<Vec<JoinHandle<()>>>) -> Result<(), NetworkError> {
        if running.load(Ordering::Relaxed) {
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
        let mut endpoint_guard = self.endpoint.lock().await;
        *endpoint_guard = Some(endpoint.clone());
        
        // Spawn a task to accept incoming connections
        let inner_arc = self.clone();
        let running_clone = running.clone();
        let task = tokio::spawn(async move {
            inner_arc.accept_connections(endpoint, running_clone).await;
        });
        
        // Store the task handle
        let mut tasks = background_tasks.lock().await;
        tasks.push(task);
        
        running.store(true, Ordering::Relaxed);
        self.logger.info("QUIC transport started successfully");
        Ok(())
    }
    
    /// Accept incoming connections
    ///
    /// INTENTION: Listen for and handle incoming QUIC connections.
    async fn accept_connections(self: &Arc<Self>, endpoint: Endpoint, running: Arc<AtomicBool>) {
        self.logger.info("Accepting incoming connections");
        
        while running.load(Ordering::Relaxed) {
            match endpoint.accept().await {
                Some(connecting) => {
                    // Process the connection in a separate task
                    let inner_arc = self.clone();
                    let logger = self.logger.clone();
                    let _ = tokio::spawn(async move {
                        match inner_arc.handle_new_connection(connecting).await {
                            Ok(_) => {}, // Task handle is returned but not stored here
                            Err(e) => logger.error(&format!("Error handling connection: {}", e)),
                        }
                    });
                    // Note: We're not storing these task handles since they're short-lived
                    // and will complete when the connection is established
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
    async fn stop(self: &Arc<Self>, running: &Arc<AtomicBool>, background_tasks: &Mutex<Vec<JoinHandle<()>>>) -> Result<(), NetworkError> {
        if !running.load(Ordering::Relaxed) {
            return Ok(());
        }
        
        running.store(false, Ordering::Relaxed);
        
        // Close the endpoint - using proper Mutex access pattern
        let endpoint_guard = self.endpoint.lock().await;
        if let Some(endpoint) = &*endpoint_guard {
            endpoint.close(0u32.into(), b"Transport stopped");
        }
        
        // Wait for background tasks to complete
        let mut tasks = background_tasks.lock().await;
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
    async fn disconnect(self: &Arc<Self>, peer_id: PeerId, running: &Arc<AtomicBool>) -> Result<(), NetworkError> {
        if !running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Remove the peer from the connection pool
        self.connection_pool.remove_peer(&peer_id).await
    }
    
    /// Check if connected to a specific peer
    ///
    /// INTENTION: Determine if there's an active connection to the specified peer.
    fn is_connected(self: &Arc<Self>, peer_id: PeerId) -> bool {
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
    async fn send_message(self: &Arc<Self>, message: NetworkMessage, running: &Arc<AtomicBool>) -> Result<(), NetworkError> {
        if !running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Use the destination field to determine the peer to send to
        let peer_id = &message.destination;
        
        // Get the peer state
        let peer_state = match self.connection_pool.get_peer(peer_id) {
            Some(state) => state,
            None => return Err(NetworkError::ConnectionError(format!("Peer {} not found", peer_id))),
        };
        
        // Check if the peer is connected
        if !peer_state.is_connected().await {
            return Err(NetworkError::ConnectionError(format!("Not connected to peer {}", peer_id)));
        }
        
        // Get a stream for sending the message
        let mut stream = peer_state.get_send_stream().await?;
        
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
    
    /// Connect to a peer using the provided discovery message
    ///
    /// INTENTION: Establish a connection to a remote peer using the provided discovery information.
    /// This method will attempt to connect to each address in the discovery message until one succeeds.
    /// Returns a task handle for the message receiver.
    async fn connect_peer(
        self: &Arc<Self>,
        discovery_msg: PeerInfo,
        running: &Arc<AtomicBool>,
    ) -> Result<JoinHandle<()>, NetworkError> {
        if !running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Ensure we have at least one address to try
        if discovery_msg.addresses.is_empty() {
            return Err(NetworkError::ConnectionError("No addresses found for peer".to_string()));
        }
        
        // Get the peer ID based on the public_key from PeerInfo
        let peer_id = PeerId::new(discovery_msg.public_key.clone());
        
        // Check if we're already connected to this peer
        if self.connection_pool.is_peer_connected(&peer_id).await {
            self.logger.info(&format!("Already connected to peer {}", peer_id));
            
            // Return a dummy task that does nothing
            let task = tokio::spawn(async {});
            return Ok(task);
        }
        
        // Get the endpoint
        let endpoint = match self.endpoint.lock().await.as_ref() {
            Some(endpoint) => endpoint.clone(),
            None => return Err(NetworkError::TransportError("Transport not initialized".to_string())),
        };
        
        // Try each address in the discovery message
        let mut last_error = None;
        
        for peer_addr in &discovery_msg.addresses {
            // Parse the socket address
            let socket_addr = match peer_addr.parse::<SocketAddr>() {
                Ok(addr) => addr,
                Err(e) => {
                    self.logger.warn(&format!("Invalid address {}: {}", peer_addr, e));
                    last_error = Some(NetworkError::ConnectionError(format!("Invalid address {}: {}", peer_addr, e)));
                    continue; // Try the next address
                }
            };
            
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
                            );
                            
                            // Set the connection in the peer state
                            peer_state.set_connection(connection).await;
                            
                            // Successfully connected to this address
                            
                            // Start a task to receive incoming messages
                            let task = self.spawn_message_receiver(peer_id.clone(), peer_state.clone());
                            
                            // Verify the connection is properly registered
                            let is_connected = self.connection_pool.is_peer_connected(&peer_id).await;
                            self.logger.info(&format!("Connection verification for {}: {}", peer_id, is_connected));
                            
                            return Ok(task);
                        },
                        Err(e) => {
                            self.logger.warn(&format!("Failed to connect to peer {} at {}: {}", peer_id, socket_addr, e));
                            last_error = Some(NetworkError::ConnectionError(format!("Failed to establish connection to {}: {}", socket_addr, e)));
                            // Continue to the next address
                        }
                    }
                },
                Err(e) => {
                    self.logger.warn(&format!("Failed to initiate connection to peer {} at {}: {}", peer_id, socket_addr, e));
                    last_error = Some(NetworkError::ConnectionError(format!("Failed to initiate connection to {}: {}", socket_addr, e)));
                    // Continue to the next address
                }
            }
        }
        
        // If we get here, all connection attempts failed
        Err(last_error.unwrap_or_else(|| NetworkError::ConnectionError(format!("Failed to connect to peer {} on any address", peer_id))))
    }
    
    fn get_local_address(self: &Arc<Self>) -> String {
        self.bind_addr.to_string()
    }
    
    /// Perform handshake with a peer after connection is established
    ///
    /// INTENTION: Exchange node information with the peer to complete the connection setup.
    /// This is called after a successful connection to exchange node information.
    /// Returns the peer's NodeInfo after successful handshake.
    async fn handshake_peer(
        self: &Arc<Self>,
        discovery_msg: PeerInfo,
        local_node: NodeInfo,
        running: &Arc<AtomicBool>,
    ) -> Result<NodeInfo, NetworkError> {
        if !running.load(Ordering::Relaxed) {
            return Err(NetworkError::TransportError("Transport not running".to_string()));
        }
        
        // Get the peer ID based on the public_key from PeerInfo
        let peer_id = PeerId::new(discovery_msg.public_key.clone());
        
        self.logger.info(&format!("Starting handshake with peer {}", peer_id));
        
        // Check if we're connected to this peer
        if !self.connection_pool.is_peer_connected(&peer_id).await {
            return Err(NetworkError::ConnectionError(format!("Not connected to peer {}, cannot perform handshake", peer_id)));
        }
        
        // Create a handshake message containing our node info
        let handshake_message = NetworkMessage {
            source: self.node_id.clone(),
            destination: peer_id.clone(),
            message_type: "NODE_INFO_HANDSHAKE".to_string(),
            payloads: vec![NetworkMessagePayloadItem {
                path: "".to_string(),  
                value_bytes: bincode::serialize(&local_node)
                    .map_err(|e| NetworkError::MessageError(format!("Failed to serialize node info: {}", e)))?,
                correlation_id: format!("handshake-{}-{}", self.node_id, SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_millis()),
            }],
        };
        
        // Create a channel for receiving the handshake response
        let (tx, mut rx) = tokio::sync::mpsc::channel::<NetworkMessage>(1);
        
        // Register a temporary message handler for the handshake response
        let peer_id_clone = peer_id.clone();
        let handler_tx = tx.clone();
        let error_logger = Arc::new(self.logger.clone()); 
        let handler = Box::new(move |msg: NetworkMessage| -> Result<(), NetworkError> {
            let error_logger = error_logger.clone();
            if msg.source == peer_id_clone && msg.message_type == "NODE_INFO_HANDSHAKE_RESPONSE" {
                // Send the message to our channel
                let tx = handler_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = tx.send(msg).await {
                        // This is not a critical error, just log it
                        error_logger.warn(&format!("Failed to send handshake response to channel: {}", e)); 
                    }
                });
            }
            Ok(())
        });
        
        // Register the handler
        self.register_message_handler(handler).await?;
        
        // Send the handshake message
        self.send_message(handshake_message, running).await?;
        self.logger.info(&format!("Sent handshake message to peer {}", peer_id));
        
        // Wait for the handshake response with a timeout
        let response = match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
            Ok(Some(response)) => {
                self.logger.info(&format!("Received handshake response from peer {}", peer_id));
                response
            },
            Ok(None) => {
                return Err(NetworkError::ConnectionError(format!("Handshake channel closed before receiving response from peer {}", peer_id)));
            },
            Err(_) => {
                return Err(NetworkError::ConnectionError(format!("Timeout waiting for handshake response from peer {}", peer_id)));
            }
        };
        
        // Process the handshake response
        if response.message_type == "NODE_INFO_HANDSHAKE_RESPONSE" {
            if let Some(payload) = response.payloads.first() {
                match bincode::deserialize::<NodeInfo>(&payload.value_bytes) {
                    Ok(peer_node_info) => {
                        self.logger.info(&format!("Handshake completed with peer {}, received node info: {:?}", 
                                                peer_id, peer_node_info));
                        
                        // Update peer state with the node info
                        if let Some(state) = self.connection_pool.get_peer(&peer_id) {
                            state.set_node_info(peer_node_info.clone()).await;
                        }
                        
                        // Return the peer's node info
                        return Ok(peer_node_info);
                    },
                    Err(e) => {
                        return Err(NetworkError::MessageError(format!("Failed to deserialize peer node info: {}", e)));
                    }
                }
            }
        }
        
        Err(NetworkError::MessageError(format!("Received unexpected message type during handshake: {}", response.message_type)))
    }
    
    
    async fn register_message_handler(
        self: &Arc<Self>,
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
    async fn process_incoming_message(self: &Arc<Self>, mut message: NetworkMessage) -> Result<(), NetworkError> {
        self.logger.debug(&format!("Processing message from {}, type: {}", message.source, message.message_type));
        
        // Special handling for handshake messages
        if message.message_type == "NODE_INFO_HANDSHAKE" {
            self.logger.debug(&format!("Received handshake message from {}", message.source));
            
            // Extract the node info from the message
            if let Some(payload) = message.payloads.first() {
                match bincode::deserialize::<NodeInfo>(&payload.value_bytes) {
                    Ok(peer_node_info) => {
                        self.logger.debug(&format!("Received node info from {}: {:?}", message.source, peer_node_info));
                        
                        // Store the node info in the peer state
                        if let Some(peer_state) = self.connection_pool.get_peer(&message.source) {
                            peer_state.set_node_info(peer_node_info.clone()).await;
                            
                            // Create a response message with our node info
                            // For simplicity, we'll create a minimal NodeInfo with just our ID
                            let local_node_info = NodeInfo {
                                peer_id: self.node_id.clone(),
                                network_ids: vec!["default".to_string()],
                                addresses: vec![self.bind_addr.to_string()],
                                capabilities: vec![],
                                last_seen: std::time::SystemTime::now(),
                            };
                            
                            // Create the response message
                            let response = NetworkMessage {
                                source: self.node_id.clone(),
                                destination: message.source.clone(),
                                message_type: "NODE_INFO_HANDSHAKE_RESPONSE".to_string(),
                                payloads: vec![NetworkMessagePayloadItem {
                                    // Preserve the original path from the request
                                    path: payload.path.clone(),
                                    value_bytes: bincode::serialize(&local_node_info)
                                        .map_err(|e| NetworkError::MessageError(format!("Failed to serialize node info: {}", e)))?,
                                    correlation_id: payload.correlation_id.clone(),
                                }],
                            };
                            
                            // Send the response
                            self.send_message(response, &Arc::new(AtomicBool::new(true))).await?;
                            self.logger.debug(&format!("Sent handshake response to {}", message.source));
                        }
                    },
                    Err(e) => {
                        self.logger.error(&format!("Failed to deserialize node info from {}: {}", message.source, e));
                    }
                }
            }
            return Ok(());
        } else {
            self.logger.debug(&format!("Received message from {} with type: {}", message.source, message.message_type)); 
        }
        
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
        self: &Arc<Self>,
        conn: quinn::Connecting,
    ) -> Result<JoinHandle<()>, Box<dyn std::error::Error + Send + Sync>> {
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
        );

        // Set the connection in the peer state
        {
            let mut conn_guard = peer_state.connection.lock().await;
            *conn_guard = Some(connection);
        }

        // Spawn a task to receive incoming messages and return the task handle
        let task = self.spawn_message_receiver(peer_id.clone(), peer_state.clone());
        
        // Return the task handle so the caller can store it in background_tasks
        Ok(task)
    }


fn spawn_message_receiver(self: &Arc<Self>, peer_id: PeerId, peer_state: Arc<PeerState>) -> JoinHandle<()> {
    // Clone the Arc<Self> to move into the task
    let inner_arc = self.clone();
    let peer_id_clone = peer_id.clone();
    let logger = self.logger.clone();

    tokio::spawn(async move {
        loop {
            // Get the connection from the peer state
            let connection = {
                let conn_guard = peer_state.connection.lock().await;
                conn_guard.as_ref().cloned()
            };

            if let Some(connection) = connection {
                // Accept an incoming stream
                match connection.accept_uni().await {
                    Ok(stream) => {
                        // Process the incoming message
                        if let Err(e) = inner_arc.receive_message(peer_id_clone.clone(), stream).await {
                            logger.error(&format!("Error receiving message from {}: {}", peer_id_clone, e));
                        }
                    },
                    Err(quinn::ConnectionError::ApplicationClosed { .. }) => {
                        // Connection closed by the peer, exit the loop
                        logger.info(&format!("Connection closed by peer {}", peer_id_clone));
                        break;
                    },
                    Err(e) => {
                        // Other connection error
                        logger.error(&format!("Connection error from {}: {}", peer_id_clone, e));
                        break;
                    }
                }
            } else {
                // Connection is gone, exit the loop
                break;
            }
        }
    })
}

    /// Receive a message from a peer over a QUIC stream
    /// INTENTION: Read, deserialize, and dispatch the message to registered handlers.
    async fn receive_message(self: &Arc<Self>, peer_id: PeerId, mut stream: quinn::RecvStream) -> Result<(), NetworkError> {
        // Read the 4-byte length prefix
        let mut len_buf = [0u8; 4];
        
        // Read bytes one by one since we can't use AsyncReadExt
        for i in 0..4 {
            match stream.read_chunk(1, false).await {
                Ok(Some(chunk)) => {
                    if !chunk.bytes.is_empty() {
                        len_buf[i] = chunk.bytes[0];
                    } else {
                        return Err(NetworkError::MessageError("Empty chunk received".to_string()));
                    }
                },
                Ok(None) => return Err(NetworkError::MessageError("Stream closed prematurely".to_string())),
                Err(e) => {
                    self.logger.error(&format!("Failed to read message length from {}: {}", peer_id, e));
                    return Err(NetworkError::MessageError(format!("Failed to read message length: {}", e)));
                }
            }
        }
        
        let msg_len = u32::from_be_bytes(len_buf) as usize;

        // Read the message bytes
        let mut data = Vec::with_capacity(msg_len);
        let mut remaining = msg_len;
        
        while remaining > 0 {
            match stream.read_chunk(remaining.min(1024), false).await {
                Ok(Some(chunk)) => {
                    if !chunk.bytes.is_empty() {
                        data.extend_from_slice(&chunk.bytes);
                        remaining -= chunk.bytes.len();
                    } else {
                        break; // No more data
                    }
                },
                Ok(None) => break, // Stream closed
                Err(e) => {
                    self.logger.error(&format!("Failed to read message from {}: {}", peer_id, e));
                    return Err(NetworkError::MessageError(format!("Failed to read message: {}", e)));
                }
            }
        }
        
        if data.len() != msg_len {
            return Err(NetworkError::MessageError(
                format!("Incomplete message: expected {} bytes, got {}", msg_len, data.len())
            ));
        }

        // Deserialize the message
        let message: NetworkMessage = match bincode::deserialize(&data) {
            Ok(msg) => msg,
            Err(e) => {
                self.logger.error(&format!("Failed to deserialize message from {}: {}", peer_id, e));
                return Err(NetworkError::MessageError(format!("Failed to deserialize message: {}", e)));
            }
        };
        
        // Log the received message details for debugging
        if !message.payloads.is_empty() {
            self.logger.debug(&format!("Received message from {} with path: {}", 
                                      peer_id, message.payloads[0].path));
        }
        
        // Dispatch to handlers
        self.process_incoming_message(message).await?;
        Ok(())
    }

    // Clone implementation removed as per architectural refactoring
    // QuicTransportImpl should not be cloned directly, only accessed through Arc
}

#[async_trait]
impl NetworkTransport for QuicTransport {
    async fn start(&self) -> Result<(), NetworkError> {
        self.inner.start(&self.running, &self.background_tasks).await
    }
    
    async fn stop(&self) -> Result<(), NetworkError> {
        self.inner.stop(&self.running, &self.background_tasks).await
    }
    
    async fn disconnect(&self, peer_id: PeerId) -> Result<(), NetworkError> {
        self.inner.disconnect(peer_id, &self.running).await
    }
    
    fn is_connected(&self, peer_id: PeerId) -> bool {
        self.inner.is_connected(peer_id)
    }
    
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        self.inner.send_message(message, &self.running).await
    }
    
    async fn connect_peer(
        &self,
        discovery_msg: PeerInfo,
        local_node: NodeInfo,
    ) -> Result<NodeInfo, NetworkError> {
        // Call the inner implementation which returns a task handle
        match self.inner.connect_peer(discovery_msg.clone(), &self.running).await {
            Ok(task) => {
                // Store the task handle for proper lifecycle management
                let mut tasks = self.background_tasks.lock().await;
                tasks.push(task);
                
                // After connection is established, start the handshake process
                // Send the node info to the peer and wait for the response
                match self.inner.handshake_peer(discovery_msg, local_node, &self.running).await {
                    Ok(peer_node_info) => {
                        // Return the peer's node info
                        Ok(peer_node_info)
                    },
                    Err(e) => {
                        self.logger.error(&format!("Handshake failed after successful connection: {}", e));
                        Err(e)
                    }
                }
            },
            Err(e) => Err(e),
        }
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
    /// 
    /// This implementation follows the architectural design where QuicTransport is responsible
    /// for thread/task management and lifecycle, while delegating protocol-specific logic to
    /// the QuicTransportImpl which is held in an Arc.
    pub fn new(
        node_id: PeerId,
        bind_addr: SocketAddr,
        options: QuicTransportOptions,
        logger: Logger,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Create the inner implementation
        let inner = QuicTransportImpl::new(node_id.clone(), bind_addr, options, logger.clone())?;
        
        // Create and return the public API wrapper with proper task management
        Ok(Self {
            inner: Arc::new(inner),
            logger,
            node_id,
            background_tasks: Mutex::new(Vec::new()),
            running: Arc::new(AtomicBool::new(false)),
        })
    }
}
