// Discovery Module Tests
//
// Tests for the node discovery implementations

use std::time::{SystemTime, UNIX_EPOCH, Duration};
use anyhow::Result;
use tokio::time;

use runar_node::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, MemoryDiscovery};
use runar_node::network::transport::NodeIdentifier;

#[tokio::test]
async fn test_memory_discovery_register_and_find() -> Result<()> {
    // Create a discovery instance
    let discovery = MemoryDiscovery::new();
    
    // Initialize with default options
    discovery.init(DiscoveryOptions::default()).await?;
    
    // Create a node
    let node_info = NodeInfo {
        identifier: NodeIdentifier::new("test-network".to_string(), "node-1".to_string()),
        address: "localhost:8080".to_string(),
        capabilities: vec!["request".to_string(), "event".to_string()],
        last_seen: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    };
    
    // Register the node
    discovery.register_node(node_info.clone()).await?;
    
    // Find nodes in the network
    let nodes = discovery.find_nodes_by_network("test-network").await?;
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].identifier.node_id, "node-1");
    assert_eq!(nodes[0].address, "localhost:8080");
    
    // Find by ID
    let found = discovery.find_node_by_id("test-network", "node-1").await?;
    assert!(found.is_some());
    let found_node = found.unwrap();
    assert_eq!(found_node.identifier.node_id, "node-1");
    
    // Try to find a non-existent node
    let not_found = discovery.find_node_by_id("test-network", "node-2").await?;
    assert!(not_found.is_none());
    
    // Shutdown
    discovery.shutdown().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_memory_discovery_update_node() -> Result<()> {
    // Create a discovery instance
    let discovery = MemoryDiscovery::new();
    
    // Initialize with default options
    discovery.init(DiscoveryOptions::default()).await?;
    
    // Create a node
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    let node_info = NodeInfo {
        identifier: NodeIdentifier::new("test-network".to_string(), "node-1".to_string()),
        address: "localhost:8080".to_string(),
        capabilities: vec!["request".to_string()],
        last_seen: now,
    };
    
    // Register the node
    discovery.register_node(node_info.clone()).await?;
    
    // Update the node with new capabilities
    let updated_node = NodeInfo {
        identifier: NodeIdentifier::new("test-network".to_string(), "node-1".to_string()),
        address: "localhost:8080".to_string(),
        capabilities: vec!["request".to_string(), "event".to_string()],
        last_seen: now + 60, // Update the timestamp
    };
    
    discovery.update_node(updated_node).await?;
    
    // Find the node and verify updates
    let found = discovery.find_node_by_id("test-network", "node-1").await?;
    assert!(found.is_some());
    let found_node = found.unwrap();
    assert_eq!(found_node.capabilities.len(), 2);
    assert!(found_node.capabilities.contains(&"event".to_string()));
    assert!(found_node.last_seen > now);
    
    // Shutdown
    discovery.shutdown().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_memory_discovery_cleanup() -> Result<()> {
    // Create a discovery instance
    let discovery = MemoryDiscovery::new();
    
    // Initialize with custom options for quicker testing
    let options = DiscoveryOptions {
        interval_seconds: 1,
        max_nodes: 100,
        auto_cleanup: true,
        node_ttl_seconds: 2, // Very short TTL for testing
    };
    
    discovery.init(options).await?;
    
    // Create nodes with different timestamps
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Fresh node
    let fresh_node = NodeInfo {
        identifier: NodeIdentifier::new("test-network".to_string(), "fresh-node".to_string()),
        address: "localhost:8080".to_string(),
        capabilities: vec!["request".to_string()],
        last_seen: now,
    };
    
    // Stale node (old timestamp)
    let stale_node = NodeInfo {
        identifier: NodeIdentifier::new("test-network".to_string(), "stale-node".to_string()),
        address: "localhost:8081".to_string(),
        capabilities: vec!["request".to_string()],
        last_seen: now - 10, // 10 seconds old
    };
    
    // Register both nodes
    discovery.register_node(fresh_node).await?;
    discovery.register_node(stale_node).await?;
    
    // Verify both nodes exist initially
    let nodes = discovery.find_nodes_by_network("test-network").await?;
    assert_eq!(nodes.len(), 2);
    
    // Wait for cleanup to run (3 seconds should be enough with 1 second interval and 2 second TTL)
    time::sleep(Duration::from_secs(3)).await;
    
    // Verify that only the fresh node remains
    let nodes_after_cleanup = discovery.find_nodes_by_network("test-network").await?;
    assert_eq!(nodes_after_cleanup.len(), 1);
    assert_eq!(nodes_after_cleanup[0].identifier.node_id, "fresh-node");
    
    // Shutdown
    discovery.shutdown().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_memory_discovery_max_nodes() -> Result<()> {
    // Create a discovery instance
    let discovery = MemoryDiscovery::new();
    
    // Initialize with limited max nodes
    let options = DiscoveryOptions {
        interval_seconds: 30,
        max_nodes: 2, // Only allow 2 nodes
        auto_cleanup: false,
        node_ttl_seconds: 300,
    };
    
    discovery.init(options).await?;
    
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Create 3 nodes
    for i in 1..=3 {
        let node = NodeInfo {
            identifier: NodeIdentifier::new(
                "test-network".to_string(),
                format!("node-{}", i),
            ),
            address: format!("localhost:808{}", i),
            capabilities: vec!["request".to_string()],
            last_seen: now,
        };
        
        // Try to register the node
        let result = discovery.register_node(node).await;
        
        // First two should succeed, third should fail
        if i <= 2 {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
            let err = result.unwrap_err();
            assert!(err.to_string().contains("Maximum number of nodes reached"));
        }
    }
    
    // Verify only 2 nodes exist
    let nodes = discovery.find_nodes_by_network("test-network").await?;
    assert_eq!(nodes.len(), 2);
    
    // Shutdown
    discovery.shutdown().await?;
    
    Ok(())
}

#[tokio::test]
async fn test_memory_discovery_multiple_networks() -> Result<()> {
    // Create a discovery instance
    let discovery = MemoryDiscovery::new();
    
    // Initialize with default options
    discovery.init(DiscoveryOptions::default()).await?;
    
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    
    // Create nodes in different networks
    let node1 = NodeInfo {
        identifier: NodeIdentifier::new("network-1".to_string(), "node-1".to_string()),
        address: "localhost:8080".to_string(),
        capabilities: vec!["request".to_string()],
        last_seen: now,
    };
    
    let node2 = NodeInfo {
        identifier: NodeIdentifier::new("network-2".to_string(), "node-2".to_string()),
        address: "localhost:8081".to_string(),
        capabilities: vec!["request".to_string()],
        last_seen: now,
    };
    
    // Register both nodes
    discovery.register_node(node1).await?;
    discovery.register_node(node2).await?;
    
    // Verify nodes in network-1
    let nodes1 = discovery.find_nodes_by_network("network-1").await?;
    assert_eq!(nodes1.len(), 1);
    assert_eq!(nodes1[0].identifier.node_id, "node-1");
    
    // Verify nodes in network-2
    let nodes2 = discovery.find_nodes_by_network("network-2").await?;
    assert_eq!(nodes2.len(), 1);
    assert_eq!(nodes2[0].identifier.node_id, "node-2");
    
    // Verify empty for non-existent network
    let nodes3 = discovery.find_nodes_by_network("network-3").await?;
    assert_eq!(nodes3.len(), 0);
    
    // Shutdown
    discovery.shutdown().await?;
    
    Ok(())
} 