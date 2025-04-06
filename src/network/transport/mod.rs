// Network Transport Module
//
// This module defines the network transport interfaces and implementations.

// Standard library imports
use std::fmt;
use std::time::Duration;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use runar_common::types::ValueType;
use runar_common::Logger;
use std::net::SocketAddr;
use std::collections::HashMap;
use tokio::sync::{RwLock, mpsc};
use std::sync::Arc;

// Internal module declarations
pub mod quic_transport;
pub mod peer_registry;
// Removed WebSocket module completely

// Re-export types/traits from submodules or parent modules
pub use peer_registry::{PeerRegistry, PeerStatus, PeerEntry, PeerRegistryOptions};
pub use quic_transport::{QuicTransport, QuicTransportOptions};

// Import NodeInfo from the discovery module
use super::discovery::NodeInfo;

/// Unique identifier for a node in the network
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeIdentifier {
    /// The network ID this node belongs to
    pub network_id: String,
    /// Unique ID for this node within the network
    pub node_id: String,
}

impl NodeIdentifier {
    /// Create a new NodeIdentifier
    pub fn new(network_id: String, node_id: String) -> Self {
        Self { network_id, node_id }
    }
}

impl fmt::Display for NodeIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.network_id, self.node_id)
    }
}

/// Options for network transport configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportOptions {
    /// Timeout for network operations
    pub timeout: Option<Duration>,
    /// Whether to use encryption for transport
    pub use_encryption: bool,
    /// Maximum message size in bytes
    pub max_message_size: Option<usize>,
    /// Bind address for the transport (e.g., "0.0.0.0:8080")
    pub bind_address: Option<String>,
}

impl Default for TransportOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(30)),
            use_encryption: true,
            max_message_size: Some(1024 * 1024), // 1MB default
            bind_address: None,
        }
    }
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

/// Represents a message exchanged between nodes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NetworkMessage {
    pub source: NodeIdentifier,
    pub destination: Option<NodeIdentifier>,
    pub message_type: String,
    pub payload: ValueType,
    pub correlation_id: Option<String>,
}

/// Handler function type for incoming network messages
pub type MessageHandler = Box<dyn Fn(NetworkMessage) -> Result<()> + Send + Sync>;

/// Interface for network transport implementations
///
/// INTENTION: Define a consistent interface for network transport mechanisms
/// that can be implemented by different transport providers (WebSockets, gRPC, etc.)
///
/// This trait allows the node to send messages to other nodes without being coupled
/// to a specific transport implementation. It supports both direct messaging and
/// publish/subscribe patterns.
#[async_trait]
pub trait NetworkTransport: Send + Sync + 'static {
    /// Initialize the transport with the given options
    ///
    /// INTENTION: Set up any connections, listeners, or resources needed by the transport.
    async fn init(&self) -> Result<()>;
    
    /// Connect to a specific node by address
    async fn connect(&self, node_info: &NodeInfo) -> Result<()>;
    
    /// Sends a message over the network.
    /// If `message.destination` is Some, send to that specific node.
    /// If `message.destination` is None, broadcast to relevant peers.
    async fn send(&self, message: NetworkMessage) -> Result<()>;
    
    /// Register a handler for incoming messages
    ///
    /// INTENTION: Set a callback to be invoked when messages are received.
    /// The handler is responsible for processing the message and taking appropriate action.
    fn register_handler(&self, handler: MessageHandler) -> Result<()>;
    
    /// Get the PeerRegistry associated with this transport
    fn peer_registry(&self) -> &PeerRegistry;
    
    /// Get the local node's identifier
    fn local_node_id(&self) -> &NodeIdentifier;

    /// Get the local network address the transport is bound to
    async fn local_address(&self) -> Option<SocketAddr>;
    
    /// Shutdown the transport, closing connections and freeing resources
    ///
    /// INTENTION: Properly clean up resources when shutting down the node.
    async fn shutdown(&self) -> Result<()>;
}

/// Factory for creating network transport instances
#[async_trait]
pub trait TransportFactory: Send + Sync {
    type Transport: NetworkTransport;
    /// Create a new transport instance, passing the logger down
    async fn create_transport(
        &self, 
        node_id: NodeIdentifier, 
        logger: Logger
    ) -> Result<Self::Transport>;
}

// Removed mock implementation from core code - should be in tests only 