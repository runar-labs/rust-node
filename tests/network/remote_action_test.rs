use anyhow::Result;
use runar_common::logging::Logger;
use runar_common::Component;
use runar_common::types::{ArcValueType, SerializerRegistry};
use runar_common::hmap;
use runar_node::network::transport::QuicTransportOptions;
use runar_node::node::{Node, NodeConfig, NetworkConfig, TransportType, LoggingConfig, LogLevel};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

// Import the fixture MathService
use crate::fixtures::math_service::MathService;

/// Tests for remote action invocation between nodes
///
/// These tests verify the functionality of remote action calls between two nodes
/// using the network layer with actual Node instances.
#[cfg(test)]
mod remote_action_tests {
    use runar_node::network::transport::SkipServerVerification;

    use crate::network::quic_transport_test::generate_test_certificates;

    use super::*;
 
    /// Test for remote action calls between two nodes
    ///
    /// INTENTION: Create two Node instances with network enabled, they should discover and connect to each other
    /// each node should have one math service with different path, so we can call it from each node and test
    /// the remote calls
    #[tokio::test]
    async fn test_remote_action_call() -> Result<()> {
        // Configure logging to ensure test logs are displayed
        let logging_config = LoggingConfig::new()
            .with_default_level(LogLevel::Debug);
        logging_config.apply();
        
        // Set up logger
        let logger = Logger::new_root(Component::Network, "remote_action_test");
        logger.info("Starting remote action call test");
  

        // Create math services with different paths using the fixture
        let math_service1 = MathService::new("math1", "math1");
        let math_service2 = MathService::new("math2", "math2");


        let (certs_a, key_a) = generate_test_certificates();
        let options_a = QuicTransportOptions::new()
            .with_certificates(certs_a)
            .with_private_key(key_a)
            .with_verify_certificates(false)
            .with_certificate_verifier(Arc::new(SkipServerVerification {}))
            .with_keep_alive_interval(Duration::from_secs(1))
            .with_connection_idle_timeout(Duration::from_secs(60))
            .with_stream_idle_timeout(Duration::from_secs(30));

        // Create node configurations with network enabled
        let node1_config = NodeConfig::new("node1", "test")
            .with_network_config(NetworkConfig::with_quic(options_a)
            .with_multicast_discovery());

        logger.info(format!("Node1 config: {}", node1_config));
        
        let mut node1 = Node::new(node1_config).await?;
        node1.add_service(math_service1).await?;

        node1.start().await?;
        //after node 1 starts and use the port .. next node will use the next available port


        let (certs_b, key_b) = generate_test_certificates();
        let options_b = QuicTransportOptions::new()
            .with_certificates(certs_b)
            .with_private_key(key_b)
            .with_verify_certificates(false)
            .with_certificate_verifier(Arc::new(SkipServerVerification {}))
            .with_keep_alive_interval(Duration::from_secs(1))
            .with_connection_idle_timeout(Duration::from_secs(60))
            .with_stream_idle_timeout(Duration::from_secs(30));


        let node2_config = NodeConfig::new("node2", "test")
            .with_network_config(NetworkConfig::with_quic(options_b).with_multicast_discovery());

        logger.info(format!("Node2 config: {}", node2_config));
          
        let mut node2 = Node::new(node2_config).await?;


        node2.add_service(math_service2).await?;
        node2.start().await?;

        // Wait for discovery and connection to happen (simple sleep)
        logger.info("Waiting for nodes to discover each other...");
        sleep(Duration::from_secs(55)).await;

        // Test calling math service1 (on node1) from node2
        logger.info("Testing remote action call from node2 to node1...");
        let add_params = ArcValueType::new_map(hmap! {
            "a" => 5.0,
            "b" => 3.0
        });
        
        // Use the proper network path format - with network ID for remote actions
        let response = node2.request("math1/add", add_params).await?;
        if let Some(mut result_value) = response.data {
            let result: f64 = result_value.as_type()?;
            assert_eq!(result, 8.0);
            logger.info(format!("Add operation succeeded: 5 + 3 = {}", result));
        } else {
            return Err(anyhow::anyhow!("Unexpected response type: {:?}", response.data));
        }

        // Test calling math service2 (on node2) from node1
        logger.info("Testing remote action call from node1 to node2...");
        let multiply_params = ArcValueType::new_map(hmap! {
            "a" => 4.0,
            "b" => 7.0
        });
        
        let response = node1.request("math2/multiply", multiply_params).await?;
        if let Some(mut result_value) = response.data {
            let result: f64 = result_value.as_type()?;
            assert_eq!(result, 28.0);
            logger.info(format!("Multiply operation succeeded: 4 * 7 = {}", result));
        } else {
            return Err(anyhow::anyhow!("Unexpected response type: {:?}", response.data));
        }

        // Shut down nodes
        node1.stop().await?;
        node2.stop().await?;

        logger.info("Remote action test completed successfully");
        Ok(())
    }
} 