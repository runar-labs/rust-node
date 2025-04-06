// Node Discovery Interface
//
// INTENTION: Define interfaces for node discovery mechanisms. Discovery is responsible
// for finding and announcing node presence on the network, but NOT maintaining 
// a registry of nodes or managing connections.

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use serde::{Serialize, Deserialize};

use crate::network::transport::NodeIdentifier;

pub mod memory_discovery;
pub mod multicast_discovery;
pub mod mock;

pub use memory_discovery::MemoryDiscovery;
pub use multicast_discovery::MulticastDiscovery;
pub use mock::MockNodeDiscovery;

/// Configuration options for node discovery
#[derive(Clone, Debug)]
pub struct DiscoveryOptions {
    /// How often to announce this node's presence (in seconds)
    pub announce_interval: Duration,
    /// Timeout for discovery operations (in seconds)
    pub discovery_timeout: Duration,
    /// Time-to-live for discovered nodes (in seconds)
    pub node_ttl: Duration,
    /// Whether to use multicast for discovery (if supported)
    pub use_multicast: bool,
    /// Whether to limit discovery to the local network
    pub local_network_only: bool,
    /// The multicast group address (e.g., "239.255.42.98")
    pub multicast_group: String,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            announce_interval: Duration::from_secs(60),
            discovery_timeout: Duration::from_secs(10),
            node_ttl: Duration::from_secs(300),
            use_multicast: true,
            local_network_only: true,
            multicast_group: DEFAULT_MULTICAST_ADDR.to_string(),
        }
    }
}

// Make the constant public
pub const DEFAULT_MULTICAST_ADDR: &str = "239.255.42.98";

/// Information about a node in the network
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeInfo {
    /// The node's identifier
    pub identifier: NodeIdentifier,
    /// The node's network address
    pub address: String,
    /// Node capabilities (services, features, etc.)
    pub capabilities: Vec<String>,
    /// When this node was last seen
    pub last_seen: std::time::SystemTime,
}

/// Callback function type for discovery events
pub type DiscoveryListener = Box<dyn Fn(NodeInfo) + Send + Sync>;

/// Interface for node discovery mechanisms
#[async_trait]
pub trait NodeDiscovery: Send + Sync {
    /// Initialize the discovery mechanism with the given options
    async fn init(&self, options: DiscoveryOptions) -> Result<()>;
    
    /// Start announcing this node's presence on the network
    async fn start_announcing(&self, info: NodeInfo) -> Result<()>;
    
    /// Stop announcing this node's presence
    async fn stop_announcing(&self) -> Result<()>;
    
    /// Register a node manually (e.g., from static config)
    async fn register_node(&self, node_info: NodeInfo) -> Result<()>;

    /// Update information for an existing node
    async fn update_node(&self, node_info: NodeInfo) -> Result<()>;

    /// Discover nodes in the network
    /// If network_id is None, discover nodes across all known networks.
    async fn discover_nodes(&self, network_id: Option<&str>) -> Result<Vec<NodeInfo>>;

    /// Find a specific node by its identifier
    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>>;
    
    /// Set a listener to be called when nodes are discovered or updated
    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()>;
    
    /// Shutdown the discovery mechanism
    async fn shutdown(&self) -> Result<()>;
} 