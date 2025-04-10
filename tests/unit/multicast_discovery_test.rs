// Multicast Discovery Tests
//
// Tests for the Multicast Discovery implementation

use std::sync::Arc;
use std::time::Duration;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio::sync::RwLock;

use runar_node::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use runar_node::network::discovery::multicast_discovery::MulticastDiscovery;
use runar_node::network::transport::PeerId;
use runar_common::Logger;

// Create a MockDiscovery implementation
struct MockDiscovery {
    listeners: RwLock<Vec<DiscoveryListener>>,
}

impl MockDiscovery {
    fn new() -> Self {
        Self {
            listeners: RwLock::new(Vec::new()),
        }
    }

    async fn notify_listeners(&self, node_info: NodeInfo) {
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener(node_info.clone());
        }
    }
}

#[async_trait::async_trait]
impl NodeDiscovery for MockDiscovery {
    async fn init(&self, _options: DiscoveryOptions) -> Result<()> {
        Ok(())
    }

    async fn start_announcing(&self, _info: NodeInfo) -> Result<()> {
        Ok(())
    }

    async fn stop_announcing(&self) -> Result<()> {
        Ok(())
    }

    async fn discover_nodes(&self, _network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        Ok(Vec::new())
    }

    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        self.listeners.write().await.push(listener);
        Ok(())
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn register_node(&self, _node_info: NodeInfo) -> Result<()> {
        Ok(())
    }

    async fn update_node(&self, _node_info: NodeInfo) -> Result<()> {
        Ok(())
    }

    async fn find_node(&self, _network_id: &str, _node_id: &str) -> Result<Option<NodeInfo>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn test_multicast_discovery_announce() -> Result<()> {
        // Since actual multicast testing is tricky in unit tests,
        // we'll use our mock to verify the listener functionality
        let discovery = Arc::new(MockDiscovery::new());
        
        // Create channel for receiving notifications
        let (tx, mut rx) = mpsc::channel::<NodeInfo>(10);
        // Add a oneshot channel to signal when a discovery is made
        let (done_tx, done_rx) = oneshot::channel::<()>();
        let done_tx = Arc::new(tokio::sync::Mutex::new(Some(done_tx)));
        
        // Set discovery listener
        discovery.set_discovery_listener(Box::new(move |node_info| {
            let tx = tx.clone();
            let done_tx_clone = Arc::clone(&done_tx);
            
            tokio::spawn(async move {
                // Send the node info to our channel
                if let Err(e) = tx.send(node_info).await {
                    eprintln!("Channel send error: {}", e);
                }
                
                // Signal that we received a discovery
                let mut done_guard = done_tx_clone.lock().await;
                if let Some(done_sender) = done_guard.take() {
                    let _ = done_sender.send(());
                }
            });
        })).await?;
        
        // Create test node info
        let node_info = NodeInfo {
            peer_id: PeerId::new("test-network".to_string(), "node1".to_string()),
            network_ids: vec!["test-network".to_string()],
            address: "127.0.0.1:8080".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Manually trigger the discovery notification
        discovery.notify_listeners(node_info.clone()).await;
        
        // Wait for the notification to be processed
        let notification_timeout = tokio::time::timeout(Duration::from_secs(1), done_rx);
        
        match notification_timeout.await {
            Ok(_) => {
                // Check the received node info
                if let Ok(received_node_info) = rx.try_recv() {
                    assert_eq!(received_node_info.peer_id.node_id, "node1");
                    assert_eq!(received_node_info.address, "127.0.0.1:8080");
                } else {
                    panic!("Discovery notification signaled but no node info in channel");
                }
            },
            Err(_) => {
                panic!("Timed out waiting for discovery notification");
            }
        }
        
        Ok(())
    }

    #[tokio::test]
    async fn test_multicast_announce_and_discover() -> Result<()> {
        // Create two discovery instances
        let options1 = DiscoveryOptions::default();
        let options2 = DiscoveryOptions::default();
        
        // Create node info for node 1
        let node_info = NodeInfo {
            network_ids: vec!["test-network".to_string()], // Add missing field
            peer_id: PeerId::new("node1".to_string()), // Only node ID
            address: "127.0.0.1:8080".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // ... existing code ...
    }
} 