// Memory-based Node Discovery
//
// INTENTION: Provide a simple in-memory implementation of node discovery
// for development and testing. This implementation maintains a list of nodes 
// in memory and doesn't use actual network protocols for discovery.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Mutex};
use std::time::{Duration, SystemTime};
use tokio::task::JoinHandle;
use tokio::time;

use super::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use crate::network::transport::PeerId;

/// In-memory node discovery for development and testing
#[derive(Default)]
pub struct MemoryDiscovery {
    /// Nodes registered with this discovery mechanism, keyed by network_id
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,
    /// Node info for the local node
    local_node: Arc<RwLock<Option<NodeInfo>>>,
    /// Options for discovery
    options: RwLock<Option<DiscoveryOptions>>,
    /// Handle for the cleanup task
    cleanup_task: Mutex<Option<JoinHandle<()>>>,
    /// Handle for the announcement task
    announce_task: Mutex<Option<JoinHandle<()>>>,
    /// Listeners for discovery events
    listeners: Arc<RwLock<Vec<DiscoveryListener>>>,
}

impl MemoryDiscovery {
    /// Create a new in-memory discovery mechanism
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(RwLock::new(HashMap::new())),
            local_node: Arc::new(RwLock::new(None)),
            options: RwLock::new(None),
            cleanup_task: Mutex::new(None),
            announce_task: Mutex::new(None),
            listeners: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Start a background task to periodically clean up stale nodes
    fn start_cleanup_task(&self, options: DiscoveryOptions) -> JoinHandle<()> {
        let nodes = Arc::clone(&self.nodes);
        let ttl = options.node_ttl;

        tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(60));

            loop {
                interval.tick().await;
                Self::cleanup_stale_nodes(&nodes, ttl);
            }
        })
    }

    /// Start a background task to periodically announce our presence
    fn start_announce_task(&self, info: NodeInfo, options: DiscoveryOptions) -> JoinHandle<()> {
        let nodes = Arc::clone(&self.nodes);
        let interval = options.announce_interval;
        let node_info = info.clone();
        let listeners = Arc::clone(&self.listeners);

        tokio::spawn(async move {
            let mut ticker = time::interval(interval);

            loop {
                ticker.tick().await;
                
                // Update the node's last_seen timestamp
                let mut updated_info = node_info.clone();
                updated_info.last_seen = SystemTime::now();
                
                // "Announce" by updating our entry in the registry
                let node_key = updated_info.peer_id.to_string();
                {
                    let mut nodes_map = nodes.write().unwrap();
                    nodes_map.insert(node_key.clone(), updated_info.clone());
                }
                
                // Notify listeners (simulating network discovery update)
                {
                    let listeners_guard = listeners.read().unwrap();
                    for listener in listeners_guard.iter() {
                        listener(updated_info.clone());
                    }
                }
            }
        })
    }

    /// Remove nodes that haven't been seen for a while
    fn cleanup_stale_nodes(nodes: &RwLock<HashMap<String, NodeInfo>>, ttl: Duration) {
        let now = SystemTime::now();
        let mut nodes_map = nodes.write().unwrap();
        
        // Collect keys of stale nodes first to avoid borrowing issues
        let stale_keys: Vec<String> = nodes_map.iter()
            .filter_map(|(key, info)| {
                info.last_seen.elapsed()
                    .ok()
                    .filter(|elapsed| *elapsed > ttl)
                    .map(|_| key.clone())
            })
            .collect();
            
        // Remove stale nodes
        for key in stale_keys {
            nodes_map.remove(&key);
        }
    }

    /// Adds a node to the discovery registry.
    async fn add_node_internal(&self, node_info: NodeInfo) {
        let node_key = node_info.peer_id.to_string(); 
        {
            let mut nodes = self.nodes.write().unwrap();
            nodes.insert(node_key, node_info.clone());
        }
        // Notify listeners
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            listener(node_info.clone());
        }
    }
}

#[async_trait]
impl NodeDiscovery for MemoryDiscovery {
    async fn init(&self, options: DiscoveryOptions) -> Result<()> {
        *self.options.write().unwrap() = Some(options.clone());
        
        // Start the cleanup task
        let task = self.start_cleanup_task(options);
        *self.cleanup_task.lock().unwrap() = Some(task);
        
        Ok(())
    }
    
    async fn start_announcing(&self, info: NodeInfo) -> Result<()> {
        // Store the local node info
        *self.local_node.write().unwrap() = Some(info.clone());
        
        // Get the options
        let options = match *self.options.read().unwrap() {
            Some(ref opts) => opts.clone(),
            None => return Err(anyhow::anyhow!("Discovery not initialized")),
        };
        
        // Add our node to the registry
        self.add_node_internal(info.clone()).await;
        
        // Start the announcement task
        let task = self.start_announce_task(info, options);
        *self.announce_task.lock().unwrap() = Some(task);
        
        Ok(())
    }
    
    async fn stop_announcing(&self) -> Result<()> {
        // Stop the announcement task if it exists
        if let Some(task) = self.announce_task.lock().unwrap().take() {
            task.abort();
        }
        
        // Remove our node from the registry
        if let Some(info) = &*self.local_node.read().unwrap() {
            let mut nodes_map = self.nodes.write().unwrap();
            nodes_map.remove(&info.peer_id.to_string());
        }
        
        Ok(())
    }
    
    async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        self.add_node_internal(node_info).await;
        Ok(())
    }

    async fn update_node(&self, node_info: NodeInfo) -> Result<()> {
        // Same as register for in-memory
        self.add_node_internal(node_info).await;
        Ok(())
    }

    async fn discover_nodes(&self, network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        let nodes_map = self.nodes.read().unwrap();
        let result = nodes_map
            .values()
            // Filter based on whether the node's network_ids list contains the requested network_id
            .filter(|info| network_id.map_or(true, |net_id| info.network_ids.contains(&net_id.to_string())))
            .cloned()
            .collect();
        Ok(result)
    }

    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>> {
        let nodes = self.nodes.read().unwrap();
        let key = PeerId::new(node_id.to_string()).to_string();
        Ok(nodes.get(&key).cloned())
    }

    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        self.listeners.write().unwrap().push(listener);
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        // Stop the cleanup task
        if let Some(task) = self.cleanup_task.lock().unwrap().take() {
            task.abort();
        }
        
        // Stop the announcement task
        if let Some(task) = self.announce_task.lock().unwrap().take() {
            task.abort();
        }
        
        // Clear all nodes
        self.nodes.write().unwrap().clear();
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // Add necessary imports for testing
    use super::*; 
    use crate::network::transport::PeerId;
    use std::time::SystemTime;
    

    // ... other test helper functions ...

    #[tokio::test]
    async fn test_find_node() {
        // Setup discovery instance
        let discovery = MemoryDiscovery::new();
        
        // Create NodeInfo (corrected type, add network_ids)
        let node_info_1 = NodeInfo {
            peer_id: PeerId::new("node1".to_string()),
            network_ids: vec!["net1".to_string()], // Added network_ids
            address: "addr1".to_string(),
            capabilities: vec![],
            last_seen: SystemTime::now(),
        };
        discovery.register_node(node_info_1).await.unwrap();
        
        // Find the node
        let found_node = discovery.find_node("net1", "node1").await.unwrap();
        assert!(found_node.is_some());
        assert_eq!(found_node.unwrap().peer_id.node_id, "node1");

        // Find non-existent node
        let not_found_node = discovery.find_node("net1", "node2").await.unwrap();
        assert!(not_found_node.is_none());
    }
    // TODO: Add tests for register, update, discover, cleanup, listener
} 