// Network Transport Module
//
// This module defines the network transport interfaces and implementations.

// Standard library imports
use std::fmt;
use std::time::Duration;
use std::ops::Range;
use std::net::{SocketAddr, IpAddr, Ipv4Addr, TcpListener};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use runar_common::types::ValueType;
use runar_common::Logger;
use std::collections::HashMap;
use tokio::sync::{RwLock, oneshot};
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use thiserror::Error;
use rand;
use serde::ser::{Serializer, SerializeStruct};
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor, Error as DeError};
use bincode;

// Internal module declarations
pub mod quic_transport;
pub mod peer_registry;
// Removed WebSocket module completely

// Re-export types/traits from submodules or parent modules
pub use peer_registry::{PeerRegistry, PeerStatus, PeerEntry, PeerRegistryOptions};
pub use quic_transport::{QuicTransport, QuicTransportOptions};
// Don't re-export pick_free_port since it's defined in this module

use super::discovery::multicast_discovery::PeerInfo;
// Import NodeInfo from the discovery module
use super::discovery::{NodeInfo, NodeDiscovery};
use crate::services::ServiceResponse;

/// Type alias for async-returning function
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Unique identifier for a node in the network
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PeerId {
    /// Unique ID for this node within the network
    pub public_key: String,
}

impl PeerId {
    /// Create a new NodeIdentifier
    pub fn new(node_id: String) -> Self {
        Self {public_key: node_id }
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.public_key)
    }
}

/// Options for network transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportOptions {
    /// Timeout for network operations
    pub timeout: Option<Duration>,
    /// Maximum message size in bytes
    pub max_message_size: Option<usize>,
    /// Bind address for the transport
    pub bind_address: SocketAddr,
}

impl Default for TransportOptions {
    fn default() -> Self {

        let port = pick_free_port(50000..51000).unwrap_or(0);
        let bind_address = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        println!("TransportOptions Using port: {}", port);
        Self {
            timeout: Some(Duration::from_secs(30)),
            max_message_size: Some(1024 * 1024), // 1MB default
            bind_address: bind_address,
        }
    }
}

/// Find a free port in the given range using a randomized approach
pub fn pick_free_port(port_range: Range<u16>) -> Option<u16> {
    use rand::Rng;
    let mut rng = rand::rng();
    let range_size = port_range.end - port_range.start;
    
    // Limit number of attempts to avoid infinite loops
    let max_attempts = 50;
    let mut attempts = 0;
    
    while attempts < max_attempts {
        // Generate a random port within the range
        let port = port_range.start + rng.random_range(0..range_size);
        
        // Check if the port is available for TCP
        if let Ok(tcp_listener) = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)) {
            let bound_port = match tcp_listener.local_addr() {
                Ok(addr) => addr.port(),
                Err(_) => {
                    attempts += 1;
                    continue;
                }
            };
            
            // For UDP/QUIC protocols, we should also check UDP availability
            // Since TcpListener only checks TCP ports
            if let Ok(_) = std::net::UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), bound_port)) {
                return Some(bound_port);
            }
        }
        
        attempts += 1;
    }
    
    None // No free port found after max attempts
}

/// Types of messages that can be sent over the network
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetworkMessageType {
    /// Service request message
    Request,
    /// Service response message
    Response,
    /// Event publication
    Event,
    /// Node discovery related message
    Discovery,
    /// Heartbeat/health check
    Heartbeat,
}

/// Represents a payload item in a network message
///
/// IMPORTANT: This is implemented as a struct with fields, but for backward compatibility
/// it can be used like a tuple (path, value, correlation_id).
/// New code should use the named fields directly.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetworkMessagePayloadItem {
    /// The path/topic associated with this payload (first tuple element)
    pub path: String,
    
    /// The value/payload data (second tuple element)
    pub value_bytes: Vec<u8>,
    
    /// Correlation ID for request/response tracking (third tuple element)
    pub correlation_id: String,
}

impl NetworkMessagePayloadItem {
    /// Create a new NetworkMessagePayloadItem
    pub fn new(path: String, value_bytes: Vec<u8>, correlation_id: String) -> Self {
        Self {
            path,
            value_bytes,
            correlation_id,
        }
    }
    
    /// Create a new NetworkMessagePayloadItem with a struct value that gets 
    /// automatically serialized using bincode
    pub fn with_struct<T>(path: String, value: T, correlation_id: String) -> Result<Self>
    where 
        T: std::fmt::Debug + serde::Serialize + Clone + Send + Sync + 'static
    {
        // Use the new create_struct method
        let value_type = ValueType::create_struct(value)?;
        
        Ok(Self::new(path, value_type.to_bytes(), correlation_id))
    }
    
    /// Create a new NetworkMessagePayloadItem with a map value
    pub fn with_map<V>(path: String, map: HashMap<String, V>, correlation_id: String) -> Result<Self>
    where 
        V: std::fmt::Debug + serde::Serialize + Clone + Send + Sync + 'static
    {
        // Use the new create_map method
        let value_type = ValueType::create_map(map)?;
        
        Ok(Self::new(path, value_type.to_bytes(), correlation_id))
    }
    
    /// Create a new NetworkMessagePayloadItem with an array value
    pub fn with_array<T>(path: String, array: Vec<T>, correlation_id: String) -> Result<Self>
    where 
        T: std::fmt::Debug + serde::Serialize + Clone + Send + Sync + 'static
    {
        // Use the new create_array method
        let value_type = ValueType::create_array<T>(array)?;
        
        Ok(Self::new(path, value_type.to_bytes(), correlation_id))
    }
      
}

// // For backward compatibility of code that treats NetworkMessagePayloadItem as a tuple
// // IMPORTANT: KEEP THIS - many places in the code are written assuming it's a tuple
// impl<'a> From<&'a NetworkMessagePayloadItem> for (&'a String, &'a ValueType, &'a String) {
//     fn from(item: &'a NetworkMessagePayloadItem) -> Self {
//         (&item.path, ValueType::from_bytes(&item.value_bytes).unwrap(), &item.correlation_id)
//     }
// }

// // Allow creation from a tuple 
// impl From<(String, ValueType, String)> for NetworkMessagePayloadItem {
//     fn from(tuple: (String, ValueType, String)) -> Self {
//         let (path, value, correlation_id) = tuple;
//         Self::new(path, value.to_bytes(), correlation_id)
//     }
// }

/// Represents a message exchanged between nodes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetworkMessage {
    /// Source node identifier
    pub source: PeerId,
    
    /// Destination node identifier (MUST be specified)
    pub destination: PeerId,
    
    /// Message type (Request, Response, Event, etc.)
    pub message_type: String,
    
    /// List of payloads  
    pub payloads: Vec<NetworkMessagePayloadItem>,
}

/// Handler function type for incoming network messages
pub type MessageHandler = Box<dyn Fn(NetworkMessage) -> Result<()> + Send + Sync>;

/// Callback type for message handling with future
pub type MessageCallback = Arc<dyn Fn(NetworkMessage) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Callback type for connection status changes
pub type ConnectionCallback = Arc<dyn Fn(PeerId, bool, Option<NodeInfo>) -> BoxFuture<'static, Result<()>> + Send + Sync>;

/// Network transport interface
#[async_trait]
pub trait NetworkTransport: Send + Sync {
 
    /// Start listening for incoming connections
    async fn start(&self) -> Result<(), NetworkError>;
    
    /// Stop listening for incoming connections
    async fn stop(&self) -> Result<(), NetworkError>;
    
    /// Check if the transport is running
    fn is_running(&self) -> bool;
    
    /// Get the local address this transport is bound to
    fn get_local_address(&self) -> String;
    
    /// Get the local node identifier
    fn get_local_node_id(&self) -> PeerId;
    
    /// Disconnect from a remote node
    async fn disconnect(&self, node_id: PeerId) -> Result<(), NetworkError>;
    
    /// Check if connected to a specific node
    fn is_connected(&self, node_id: PeerId) -> bool;
    
    /// Send a message to a remote node
    async fn send_message(&self, message: NetworkMessage) -> Result<(), NetworkError>;
    
    /// Register a message handler for incoming messages
    fn register_message_handler(&self, handler: MessageHandler) -> Result<()>;
    
    /// Set a callback for connection status changes
    fn set_connection_callback(&self, callback: ConnectionCallback) -> Result<()>;
    
    /// Send a service request to a remote node
    async fn send_request(&self, message: NetworkMessage) -> Result<NetworkMessage, NetworkError>;
    
    /// Handle an incoming network message
    async fn handle_message(&self, message: NetworkMessage) -> Result<(), NetworkError>;
    
    /// Start node discovery process
    async fn start_discovery(&self) -> Result<(), NetworkError>;
    
    /// Stop node discovery process
    async fn stop_discovery(&self) -> Result<(), NetworkError>;
    
    /// Register a discovered node
    async fn connect_node(&self, discovery_msg: PeerInfo, local_node:NodeInfo) -> Result<(), NetworkError>;
    
    /// Get all discovered nodes
    fn get_discovered_nodes(&self) -> Vec<PeerId>;
    
    /// Set the node discovery mechanism
    fn set_node_discovery(&self, discovery: Box<dyn NodeDiscovery>) -> Result<()>;
    
    /// Complete a pending request
    fn complete_pending_request(&self, correlation_id: String, response: NetworkMessage) -> Result<(), NetworkError>;
}

/// Factory for creating network transport instances
#[async_trait]
pub trait TransportFactory: Send + Sync {
    type Transport: NetworkTransport;
    /// Create a new transport instance, passing the logger down
    async fn create_transport(
        &self, 
        node_id: PeerId, 
        logger: Logger
    ) -> Result<Self::Transport>;
}

/// Base implementation for network transport with common fields
pub struct BaseNetworkTransport {
    /// Network-specific concerns moved from Node
    
    /// Node discovery mechanism
    pub node_discovery: Arc<RwLock<Option<Box<dyn NodeDiscovery>>>>,
    
    /// Registry of discovered nodes
    pub discovered_nodes: Arc<RwLock<HashMap<String, PeerInfo>>>,
    
    /// Pending network requests waiting for responses
    pub pending_requests: Arc<RwLock<HashMap<String, oneshot::Sender<Result<ServiceResponse>>>>>,
    
    /// Local node identifier
    pub local_node_id: PeerId,
    
    /// Logger instance
    pub logger: Logger,
    
    /// Message handler for incoming messages
    message_handler: Arc<RwLock<Option<MessageHandler>>>,
    
    /// Connection status callback
    connection_callback: Arc<RwLock<Option<ConnectionCallback>>>,
}

impl BaseNetworkTransport {
    /// Create a new BaseNetworkTransport with the given node ID and logger
    pub fn new(local_node_id: PeerId, logger: Logger) -> Self {
        Self {
            node_discovery: Arc::new(RwLock::new(None)),
            discovered_nodes: Arc::new(RwLock::new(HashMap::new())),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            local_node_id,
            logger,
            message_handler: Arc::new(RwLock::new(None)),
            connection_callback: Arc::new(RwLock::new(None)),
        }
    }
    
    /// Set the node discovery mechanism
    pub async fn set_node_discovery(&self, discovery: Box<dyn NodeDiscovery>) -> Result<()> {
        let mut node_discovery = self.node_discovery.write().await;
        *node_discovery = Some(discovery);
        Ok(())
    }
    
    /// Get the node discovery mechanism
    pub async fn get_node_discovery(&self) -> Option<Arc<RwLock<Option<Box<dyn NodeDiscovery>>>>> {
        Some(self.node_discovery.clone())
    }
    
    /// Register a discovered node
    pub async fn discovered_node(&self, discovery_msg: PeerInfo) -> Result<()> {
        let mut discovered_nodes = self.discovered_nodes.write().await;
        discovered_nodes.insert(discovery_msg.public_key.clone(), discovery_msg);
        Ok(())
    }
    
    /// Get all discovered nodes
    pub async fn get_discovered_nodes(&self) -> Vec<PeerInfo> {
        let discovered_nodes = self.discovered_nodes.read().await;
        discovered_nodes.values().cloned().collect()
    }
    
    /// Register a pending request
    pub async fn register_pending_request(&self, correlation_id: String, sender: oneshot::Sender<Result<ServiceResponse>>) -> Result<()> {
        let mut pending_requests = self.pending_requests.write().await;
        pending_requests.insert(correlation_id, sender);
        Ok(())
    }
    
    /// Complete a pending request
    pub async fn complete_pending_request(&self, correlation_id: &str, response: Result<ServiceResponse>) -> Result<()> {
        let mut pending_requests = self.pending_requests.write().await;
        if let Some(sender) = pending_requests.remove(correlation_id) {
            // It's okay if the receiver is dropped, this just means the caller timed out or is no longer interested
            let _ = sender.send(response);
        }
        Ok(())
    }
}

/// Error type for network operations
#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("Connection error: {0}")]
    ConnectionError(String),
    #[error("Message error: {0}")]
    MessageError(String),
    #[error("Discovery error: {0}")]
    DiscoveryError(String),
    #[error("Transport error: {0}")]
    TransportError(String),
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
}

#[async_trait]
impl NetworkTransport for BaseNetworkTransport {
 
    /// Start listening for incoming connections
    async fn start(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Stop listening for incoming connections
    async fn stop(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Check if the transport is running
    fn is_running(&self) -> bool {
        false
    }
    
    /// Get the local address this transport is bound to
    fn get_local_address(&self) -> String {
        "0.0.0.0:0".to_string()
    }
    
    /// Get the local node identifier
    fn get_local_node_id(&self) -> PeerId {
        self.local_node_id.clone()
    }
    
    /// Disconnect from a remote node
    async fn disconnect(&self, node_id: PeerId) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Check if connected to a specific node
    fn is_connected(&self, node_id: PeerId) -> bool {
        false
    }
    
    /// Send a message to a remote node
    async fn send_message(&self, _message: NetworkMessage) -> Result<(), NetworkError> {
        // Default implementation returns error - requires override in concrete types
        Err(NetworkError::TransportError("send_message not implemented for this base transport".to_string()))
    }
    
    /// Register a message handler for incoming messages
    fn register_message_handler(&self, handler: MessageHandler) -> Result<()> {
        let mut message_handler = match self.message_handler.try_write() {
            Ok(guard) => guard,
            Err(_) => return Err(anyhow::Error::msg("Failed to acquire write lock for message handler").into()),
        };
        *message_handler = Some(handler);
        Ok(())
    }
    
    /// Set a callback for connection status changes
    fn set_connection_callback(&self, callback: ConnectionCallback) -> Result<()> {
        let mut connection_callback = match self.connection_callback.try_write() {
            Ok(guard) => guard,
            Err(_) => return Err(anyhow::Error::msg("Failed to acquire write lock for connection callback").into()),
        };
        *connection_callback = Some(callback);
        Ok(())
    }
     
    
    /// Send a service request to a remote node
    async fn send_request(&self, message: NetworkMessage) -> Result<NetworkMessage, NetworkError> {
        Err(NetworkError::TransportError("Not implemented".to_string()))
    }
    
    /// Handle an incoming network message
    async fn handle_message(&self, message: NetworkMessage) -> Result<(), NetworkError> {
        // Default implementation does nothing - requires override in concrete types
        self.logger.warn(format!("Base NetworkTransport::handle_message called for message type '{}', but does not implement handling.", message.message_type));
        Ok(())
    }
    
    /// Start node discovery process
    async fn start_discovery(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Stop node discovery process
    async fn stop_discovery(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Register a discovered node
    async fn connect_node(&self, peer_info: PeerInfo, local_node:NodeInfo) -> Result<(), NetworkError> {
        Ok(())
    }
    
    /// Get all discovered nodes
    fn get_discovered_nodes(&self) -> Vec<PeerId> {
        Vec::new()
    }
    
    /// Set the node discovery mechanism
    fn set_node_discovery(&self, discovery: Box<dyn NodeDiscovery>) -> Result<()> {
        let mut node_discovery = match self.node_discovery.try_write() {
            Ok(guard) => guard,
            Err(_) => return Err(anyhow::Error::msg("Failed to acquire write lock for node discovery").into()),
        };
        *node_discovery = Some(discovery);
        Ok(())
    }
    
    /// Complete a pending request
    fn complete_pending_request(&self, correlation_id: String, response: NetworkMessage) -> Result<(), NetworkError> {
        // This is a synchronous method that needs to be implemented for each concrete transport
        // For the base implementation, we just return Ok
        Ok(())
    }
} 