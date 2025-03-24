// Basic utilities for working with the node in tests
use anyhow::Result;
use runar_node::{Node, NodeConfig};
use std::path::Path;
use tempfile::TempDir;

/// Create a test node with a temporary directory
pub async fn create_test_node(network_id: &str) -> Result<(Node, TempDir)> {
    // Create a temporary directory for the test
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    // Create a node configuration with the temporary directory
    let config = NodeConfig::new(network_id, temp_path, temp_path);
    
    // Create and initialize the node
    let mut node = Node::new(temp_path, network_id).await?;
    node.init().await?;
    
    Ok((node, temp_dir))
}

/// Wait for node initialization to complete
pub async fn wait_for_node_init(node: &Node, duration_ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
} 