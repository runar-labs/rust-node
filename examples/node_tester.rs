use anyhow::Result;
use runar_common::{Logger, Component};
use runar_node::node::{Node, NodeConfig};
use tokio::time::Duration;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logger
    let logger = Logger::new_root(Component::Node, "node_tester");
    logger.info("Starting Node tester example");

    // Create a node configuration with networking disabled
    let mut config = NodeConfig::new("example-node", "test_network");
    config.network_config = None; // Explicitly disable networking

    logger.info("Creating Node with networking disabled");
    
    // Create a new node with the configuration
    let mut node = Node::new(config).await?;
    
    logger.info("Node created successfully!");
    
    // Start the node
    logger.info("Starting Node...");
    node.start().await?;
    logger.info("Node started successfully!");
    
    // Wait a bit
    logger.info("Node running... will stop in 2 seconds");
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Stop the node
    logger.info("Stopping Node...");
    node.stop().await?;
    logger.info("Node stopped successfully!");
    
    Ok(())
} 