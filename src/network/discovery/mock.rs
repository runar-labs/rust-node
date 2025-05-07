// Mock Node Discovery Implementation
//
// INTENTION: Provide a mock implementation of the NodeDiscovery trait for testing.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{DiscoveryListener, DiscoveryOptions, NodeDiscovery, NodeInfo};
use crate::network::transport::PeerId;

/// A mock implementation of NodeDiscovery that stores nodes in memory
pub struct MockNodeDiscovery {
    /// Known nodes
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    /// Listeners for discovery events
    listeners: Arc<RwLock<Vec<DiscoveryListener>>>,
}

impl MockNodeDiscovery {
    /// Create a new mock discovery instance
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            listeners: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a test node to the discovery service
    pub fn add_test_node(&self, info: NodeInfo) {
        self.nodes
            .write()
            .unwrap()
            .insert(info.peer_id.public_key.clone(), info);
    }

    /// Clear all nodes
    pub fn clear_nodes(&self) {
        self.nodes.write().unwrap().clear();
    }

    /// Helper to add nodes for testing
    pub async fn add_mock_node(&self, node_info: NodeInfo) {
        let key = node_info.peer_id.to_string();
        self.nodes.write().unwrap().insert(key, node_info.clone());
        // Notify listeners
        for listener in self.listeners.read().unwrap().iter() {
            listener(node_info.clone());
        }
    }
}

#[async_trait]
impl NodeDiscovery for MockNodeDiscovery {
    async fn init(&self, _options: DiscoveryOptions) -> Result<()> {
        Ok(())
    }

    async fn start_announcing(&self) -> Result<()> {
        Ok(())
    }

    async fn stop_announcing(&self) -> Result<()> {
        Ok(())
    }

    async fn discover_nodes(&self, network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        let nodes_map = self.nodes.read().unwrap();
        Ok(nodes_map
            .values()
            .filter(|info| {
                network_id.map_or(true, |net_id| {
                    info.network_ids.contains(&net_id.to_string())
                })
            })
            .cloned()
            .collect())
    }

    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        self.listeners.write().unwrap().push(listener);
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        self.nodes.write().unwrap().clear();
        Ok(())
    }

    async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        self.add_mock_node(node_info).await;
        Ok(())
    }

    async fn update_node(&self, node_info: NodeInfo) -> Result<()> {
        self.add_mock_node(node_info).await;
        Ok(())
    }

    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>> {
        let nodes = self.nodes.read().unwrap();
        let key = PeerId::new(node_id.to_string()).to_string();
        Ok(nodes.get(&key).cloned())
    }
}
