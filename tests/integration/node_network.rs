// Node Network Integration Test
//
// INTENTION: Test two nodes connected over a real network (not mocks or memory transport)
// where each node has a math service with a different path, and they call each other's services.

use std::time::Duration;
use anyhow::Result;
use tokio::time::timeout;

use runar_common::types::ValueType;
use runar_common::{Component, Logger, hmap};
use runar_node::node::{Node, NodeConfig};
use runar_node::network::discovery::{DiscoveryOptions, MulticastDiscovery};
use runar_node::network::transport::quic_transport::QuicTransportOptions;
use runar_node::routing::TopicPath;

// Import the math service from fixtures
use crate::fixtures::math_service::MathService;

/// Integration test that connects two nodes over the network
/// Each node has a math service with a different path
/// The test verifies that each node can call the math service on the other node
#[tokio::test]
async fn test_node_network_remote_calls() -> Result<()> {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(45), async {
        // Set up node 1 with MathA service
        println!("Creating node1...");
        let config1 = NodeConfig::new_with_node_id("test-network", "node1")
            .with_networking_enabled(true);
        
        // Create node1
        let mut node1 = Node::new(config1).await?;
        
        // Add math service to node1 with path "MathA"
        println!("Adding MathA service to node1...");
        let math_service1 = MathService::new("Math1", "MathA");
        node1.add_service(math_service1).await?;
        
        // Start node1 (networking will be started automatically since it's enabled in config)
        println!("Starting node1...");
        node1.start().await?;
        
        // Set up node 2 with MathB service
        println!("Creating node2...");
        let config2 = NodeConfig::new_with_node_id("test-network", "node2")
            .with_networking_enabled(true);
        
        // Create node2
        let mut node2 = Node::new(config2).await?;
        
        // Add math service to node2 with path "MathB"
        println!("Adding MathB service to node2...");
        let math_service2 = MathService::new("Math2", "MathB");
        node2.add_service(math_service2).await?;
        
        // Start node2 (networking will be started automatically since it's enabled in config)
        println!("Starting node2...");
        node2.start().await?;
        
        // Wait for nodes to discover each other (increased for better discovery)
        println!("Waiting for nodes to discover each other (5 seconds)...");
        tokio::time::sleep(Duration::from_secs(5)).await;
        
        // Debug: Check the action handlers registered with each node
        println!("Checking node1's registry services...");
        let registry_response1 = node1.request("$registry/services/list".to_string(), 
            ValueType::Map(hmap! {"include_actions" => true})).await;
        println!("Node1 services with actions: {:?}", registry_response1);

        println!("Checking node2's registry services...");
        let registry_response2 = node2.request("$registry/services/list".to_string(), 
            ValueType::Map(hmap! {"include_actions" => true})).await;
        println!("Node2 services with actions: {:?}", registry_response2);

        // Check the specific service information
        println!("Checking MathB service details on node1...");
        let mathb_info = node1.request("$registry/services/MathB".to_string(), ValueType::Null).await;
        println!("MathB service details on node1: {:?}", mathb_info);

        // Check if the services are properly registered in the registry
        println!("Checking registered services on node1...");
        let services_resp1 = node1.request("$registry/services/list".to_string(), ValueType::Null).await?;
        println!("Node1 registry services: {:?}", services_resp1);

        println!("Checking registered services on node2...");
        let services_resp2 = node2.request("$registry/services/list".to_string(), ValueType::Null).await?;
        println!("Node2 registry services: {:?}", services_resp2);
        
        // Test 1: Node1 calls Node2's add service
        println!("Test 1: Node1 calling Node2's add service...");
        
        // Here's the expected path format for the remote call
        println!("Attempting to call: test-network:MathB/add");
        
        // Call the node with the full network path
        let add_params = ValueType::Map(hmap! {
            "a" => 5.0,
            "b" => 3.0
        });
        
        let result = node1.request(
            "test-network:MathB/add".to_string(),
            add_params,
        ).await?;
        
        println!("Result from Node2's add service: {:?}", result);
        assert_eq!(result.status, 200);
        
        if let Some(ValueType::Number(sum)) = result.data {
            assert_eq!(sum, 8.0);
        } else {
            panic!("Unexpected result format: {:?}", result.data);
        }
        
        // Test 2: Node2 calls Node1's multiply service
        println!("Test 2: Node2 calling Node1's multiply service...");
        
        // Create parameters for multiplication using hmap! macro
        let params2 = ValueType::Map(hmap! {
            "a" => 4.0,
            "b" => 7.0
        });
        
        // Create the request path using TopicPath for proper formatting
        let remote_path2 = "test-network:MathA/multiply".to_string();
        
        // Make request from node2 to node1's math service
        let response2 = node2.request(remote_path2, params2).await?;
        
        // Verify response
        if let Some(ValueType::Number(result2)) = response2.data {
            assert_eq!(result2, 28.0, "Expected 4 * 7 = 28");
            println!("✓ Success: Node2 successfully called Node1's MathA/multiply service (4 * 7 = {})", result2);
        } else {
            panic!("Expected numeric result from Node1's math service, got: {:?}", response2);
        }
        
        // Cleanup: Stop both nodes
        println!("Cleaning up: Stopping nodes...");
        node1.stop().await?;
        node2.stop().await?;
        
        println!("✓ Integration test completed successfully");
        Ok(())
    }).await {
        Ok(result) => result,
        Err(_) => panic!("Test timed out after 45 seconds"),
    }
} 