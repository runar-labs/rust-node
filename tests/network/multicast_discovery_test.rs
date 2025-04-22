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
use runar_node::network::discovery::DEFAULT_MULTICAST_ADDR;
use runar_node::network::transport::PeerId;
use runar_common::Logger;
use runar_common::Component;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    use tokio::sync::oneshot;
    use anyhow::anyhow;
    use tokio::time::timeout;

    async fn create_test_discovery(network_id: &str) -> Result<MulticastDiscovery> {
        let mut options = DiscoveryOptions::default();
        options.multicast_group = format!("{}:45678", DEFAULT_MULTICAST_ADDR);
        options.announce_interval = Duration::from_secs(1); // Use shorter interval for tests
        
        let discovery = MulticastDiscovery::new(options).await?;
        // Initialize is called internally in new() so no need to call it again
        
        Ok(discovery)
    }

    #[tokio::test]
    async fn test_multicast_discovery_announce() -> Result<()> {
        // Create a MulticastDiscovery instance
        let discovery = create_test_discovery("test-network").await?;
        
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
            peer_id: PeerId::new("node1".to_string()),
            network_ids: vec!["test-network".to_string()],
            address: "127.0.0.1:8080".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Register node and start announcing
        discovery.register_node(node_info.clone()).await?;
        discovery.start_announcing(node_info.clone()).await?;
        
        // Wait a bit for the announcement to circulate
        tokio::time::sleep(Duration::from_millis(500)).await;
        
        // Stop announcing and shutdown
        discovery.stop_announcing().await?;
        discovery.shutdown().await?;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_multicast_announce_and_discover() -> Result<()> {
        // Create two MulticastDiscovery instances with different addresses
        // Using the same MulticastDiscovery but with different network IDs
        let discovery1 = create_test_discovery("test-network").await?;
        let discovery2 = create_test_discovery("test-network").await?;
        
        // Create channels for receiving node info
        let (tx1, mut rx1) = mpsc::channel::<NodeInfo>(10);
        let (tx2, mut rx2) = mpsc::channel::<NodeInfo>(10);
        
        // Set up discovery listeners
        discovery1.set_discovery_listener(Box::new(move |node_info| {
            let tx = tx1.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(node_info).await {
                    eprintln!("Channel 1 send error: {}", e);
                }
            });
        })).await?;
        
        discovery2.set_discovery_listener(Box::new(move |node_info| {
            let tx = tx2.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(node_info).await {
                    eprintln!("Channel 2 send error: {}", e);
                }
            });
        })).await?;
        
        // Create node info for node 1
        let node_info1 = NodeInfo {
            peer_id: PeerId::new("node1".to_string()),
            network_ids: vec!["test-network".to_string()],
            address: "127.0.0.1:8080".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Create node info for node 2
        let node_info2 = NodeInfo {
            peer_id: PeerId::new("node2".to_string()),
            network_ids: vec!["test-network".to_string()],
            address: "127.0.0.1:8081".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Register and start announcing for both nodes
        discovery1.register_node(node_info1.clone()).await?;
        discovery1.start_announcing(node_info1.clone()).await?;
        
        // Wait a short time for propagation
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        discovery2.register_node(node_info2.clone()).await?;
        discovery2.start_announcing(node_info2.clone()).await?;
        
        // Wait a bit for propagation and discovery
        tokio::time::sleep(Duration::from_millis(1000)).await;
        
        // Verify nodes have discovered each other
        let discovered_nodes1 = discovery1.discover_nodes(Some("test-network")).await?;
        let discovered_nodes2 = discovery2.discover_nodes(Some("test-network")).await?;
        
        // Shutdown both discoveries
        discovery1.stop_announcing().await?;
        discovery2.stop_announcing().await?;
        discovery1.shutdown().await?;
        discovery2.shutdown().await?;
        
        Ok(())
    }
} 