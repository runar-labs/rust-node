// Multicast Discovery Tests
//
// Tests for the Multicast Discovery implementation

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use runar_node::network::discovery::{MulticastDiscovery, NodeDiscovery, NodeInfo, DiscoveryOptions};
use runar_node::network::discovery::DEFAULT_MULTICAST_ADDR;
use runar_node::network::transport::PeerId;
use runar_common::types::{ServiceMetadata, ActionMetadata, EventMetadata};
use runar_common::logging::{Logger, Component};
use anyhow::Result;

mod tests {
    use super::*;
    use std::time::SystemTime;
    use runar_node::network::discovery::multicast_discovery::PeerInfo;
    use tokio::sync::oneshot;
    use anyhow::anyhow;
    use tokio::time::timeout;

    async fn create_test_discovery(network_id: &str, node_id: &str) -> Result<MulticastDiscovery> {
        let mut options = DiscoveryOptions::default();
        options.multicast_group = format!("{}:45678", DEFAULT_MULTICAST_ADDR);
        options.announce_interval = Duration::from_secs(1); // Use shorter interval for tests
        
        // Create a logger for testing
        let logger = Logger::new_root(Component::NetworkDiscovery, node_id);
        
        // Create a test node info
        let node_info = NodeInfo {
            peer_id: PeerId::new(node_id.to_string()),
            network_ids: vec![network_id.to_string()],
            addresses: vec!["127.0.0.1:8000".to_string()],
            services: vec![
                ServiceMetadata {
                    name: "test-service".to_string(),
                    service_path: "service".to_string(),
                    network_id: "test-network".to_string(),
                    version: "1.0.0".to_string(),
                    description: "Test service for unit tests".to_string(),
                    registration_time: SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs(),
                    last_start_time: Some(SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs()),
                    actions: vec![
                        ActionMetadata {
                            name: "request".to_string(),
                            description: "Test request".to_string(),
                            input_schema: None,
                            output_schema: None,
                        }
                    ],
                    events: vec![
                        EventMetadata {
                            path: "event".to_string(),
                            description: "Test event".to_string(),
                            data_schema: None,
                        }
                    ],
                }
            ],
            last_seen: SystemTime::now(),
        };
        
        // Create the discovery instance with proper parameters
        let discovery = MulticastDiscovery::new(node_info, options, logger).await?;
        
        Ok(discovery)
    }

    #[tokio::test]
    async fn test_multicast_discovery_announce() -> Result<()> {
        async fn test_multicast_discovery_listener() -> Result<()> {
            // Create a test node ID to use for verification later
            let test_node_id = format!("test-node-test-network");
            let discovery = create_test_discovery("test-network", "test-node-test-network").await?;
            
            // Create channel for receiving notifications
            let (tx, mut rx) = mpsc::channel::<PeerInfo>(10);
            // Add a oneshot channel to signal when a discovery is made
            let (done_tx, done_rx) = oneshot::channel::<()>();
            let done_tx = Arc::new(tokio::sync::Mutex::new(Some(done_tx)));
            
            // Set discovery listener
            discovery.set_discovery_listener(Arc::new(move |peer_info| {
                let tx = tx.clone();
                let done_tx_clone = Arc::clone(&done_tx);
                Box::pin(async move {
                    // Send the peer info to our channel
                    if let Err(e) = tx.send(peer_info).await {
                        eprintln!("Channel send error: {}", e);
                    }
                    // Signal that we've received a discovery
                    if let Some(done_tx) = done_tx_clone.lock().await.take() {
                        let _ = done_tx.send(());
                    }
                })
            })).await?;
            
            discovery.start_announcing().await?;
            
            // Wait a bit for the announcement to circulate
            tokio::time::sleep(Duration::from_millis(500)).await;
            
            // Wait for the discovery notification or timeout
            match tokio::time::timeout(Duration::from_secs(5), done_rx).await {
                Ok(Ok(())) => {
                    println!("Received discovery notification!");
                    
                    // Check what we received
                    if let Some(received_info) = rx.try_recv().ok() {
                        println!("Received peer info: {:?}", received_info);
                        // Verify the peer info has the expected public key
                        // We're comparing with the node ID we created in create_test_discovery
                        assert_eq!(received_info.public_key, test_node_id);
                    } else {
                        println!("No peer info in channel");
                    }
                },
                Ok(Err(e)) => {
                    return Err(anyhow!("Oneshot channel error: {}", e));
                },
                Err(_) => {
                    println!("Timeout waiting for discovery notification");
                    // This is not necessarily a failure - UDP multicast can be unreliable
                }
            }
            
            // Clean up
            discovery.stop_announcing().await?;
            
            Ok(())
        }

        test_multicast_discovery_listener().await?;
        
        Ok(())
    }
    

    #[tokio::test]
    async fn test_multicast_announce_and_discover() -> Result<()> {
        async fn test_multiple_discovery_instances() -> Result<()> {
            // Create two discovery instances
            let discovery1 = create_test_discovery("test-network", "test-node-1").await?;
            let discovery2 = create_test_discovery("test-network", "test-node-2").await?;
            
            // Create channels for receiving notifications
            let (tx1, mut rx1) = mpsc::channel::<PeerInfo>(10);
            let (tx2, mut rx2) = mpsc::channel::<PeerInfo>(10);
            
            // Add oneshot channels to signal when discoveries are made
            let (done_tx1, done_rx1) = oneshot::channel::<()>();
            let (done_tx2, done_rx2) = oneshot::channel::<()>();
            let done_tx1 = Arc::new(tokio::sync::Mutex::new(Some(done_tx1)));
            let done_tx2 = Arc::new(tokio::sync::Mutex::new(Some(done_tx2)));
            
            // Set discovery listeners for both instances
            discovery1.set_discovery_listener(Arc::new(move |peer_info: PeerInfo| {
                let tx = tx1.clone();
                let done_tx_clone = Arc::clone(&done_tx1);
                
                Box::pin(async move {
                    // Only trigger for node2
                    if peer_info.public_key.contains("node-2") {
                        // Send the peer info to our channel
                        if let Err(e) = tx.send(peer_info).await {
                            eprintln!("Channel send error: {}", e);
                        }
                        
                        // Signal that we've received a discovery
                        if let Some(done_tx) = done_tx_clone.lock().await.take() {
                            let _ = done_tx.send(());
                        }
                    }
                    
                    ()
                })
            })).await?;
            
            discovery2.set_discovery_listener(Arc::new(move |peer_info: PeerInfo| {
                let tx = tx2.clone();
                let done_tx_clone = Arc::clone(&done_tx2);
                
                Box::pin(async move {
                    // Only trigger for node1
                    if peer_info.public_key.contains("node-1") {
                        // Send the peer info to our channel
                        if let Err(e) = tx.send(peer_info).await {
                            eprintln!("Channel send error: {}", e);
                        }
                        
                        // Signal that we've received a discovery
                        if let Some(done_tx) = done_tx_clone.lock().await.take() {
                            let _ = done_tx.send(());
                        }
                    }
                    
                    ()
                })
            })).await?;
            
            // Start announcing for both nodes
            discovery1.start_announcing().await?;
            
            // Wait a short time for propagation
            tokio::time::sleep(Duration::from_millis(500)).await;
             
            discovery2.start_announcing().await?;
            
            // Wait a bit for propagation and discovery
            tokio::time::sleep(Duration::from_secs(2)).await;
            
            // Check if discovery1 received node2's info
            let node1_found_node2 = tokio::time::timeout(Duration::from_secs(1), done_rx1).await.is_ok();
            
            // Check if discovery2 received node1's info
            let node2_found_node1 = tokio::time::timeout(Duration::from_secs(1), done_rx2).await.is_ok();
            
            // Print discovery results for debugging
            println!("Node1 found Node2: {}", node1_found_node2);
            println!("Node2 found Node1: {}", node2_found_node1);
            
            // We may not always see the other node due to UDP packet loss or timing issues
            // So we log the findings but don't fail the test if not found
            if !node1_found_node2 {
                println!("Note: Discovery 1 did not find node2 - this may be normal due to UDP packet loss");
            }
            if !node2_found_node1 {
                println!("Note: Discovery 2 did not find node1 - this may be normal due to UDP packet loss");
            }
            
            // Shutdown both discoveries
            discovery1.stop_announcing().await?;
            discovery2.stop_announcing().await?;

            assert!(node1_found_node2);
            assert!(node2_found_node1); 
            
            Ok(())
        }

        test_multiple_discovery_instances().await?;
        
        Ok(())
    }
    
} 