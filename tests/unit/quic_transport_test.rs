// QUIC Transport Tests
//
// Tests for the QUIC transport implementation

use std::sync::Arc;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use std::net::SocketAddr;

use runar_node::network::transport::{NetworkTransport, NetworkMessage, NodeIdentifier, TransportFactory, TransportOptions};
use runar_node::network::transport::quic_transport::{QuicTransport, QuicTransportOptions, QuicTransportFactory};
use runar_node::network::discovery::NodeInfo;
use runar_common::types::ValueType;
use runar_common::{Component, Logger};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // Helper function to create a test QuicTransport
    async fn create_test_transport(node_id: &str, bind_address: &str) -> Result<QuicTransport> {
        let logger = Logger::new_root(Component::Network, node_id);
        let node_identifier = NodeIdentifier::new("test-network".to_string(), node_id.to_string());
        
        let mut options = QuicTransportOptions::default();
        let mut transport_options = TransportOptions::default();
        transport_options.bind_address = Some(bind_address.to_string());
        options.transport_options = transport_options;
        
        let factory = QuicTransportFactory::new(options, logger.clone());
        let transport = factory.create_transport(node_identifier, logger).await?;
        
        transport.init().await?;
        
        Ok(transport)
    }

    #[tokio::test]
    async fn test_quic_transport_local_address() -> Result<()> {
        let transport = create_test_transport("node1", "127.0.0.1:0").await?;
        
        // Check that we got a local address
        let local_addr = transport.local_address().await;
        assert!(local_addr.is_some(), "Transport should have a local address");
        
        // Shutdown the transport
        transport.shutdown().await?;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_quic_transport_connect_send() -> Result<()> {
        // Create two transports on different addresses
        let transport1 = create_test_transport("node1", "127.0.0.1:0").await?;
        let transport2 = create_test_transport("node2", "127.0.0.1:0").await?;
        
        // Get local addresses
        let addr1 = transport1.local_address().await
            .expect("Transport 1 should have a local address");
            
        let addr2 = transport2.local_address().await
            .expect("Transport 2 should have a local address");
            
        println!("Transport 1 listening on: {}", addr1);
        println!("Transport 2 listening on: {}", addr2);
        
        // Create a channel for receiving messages on transport2
        let (tx, mut rx) = mpsc::channel(10);
        
        // Register a handler on transport2
        transport2.register_handler(Box::new(move |msg| {
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(msg).await {
                    eprintln!("Channel send error: {}", e);
                }
            });
            Ok(())
        }))?;
        
        // Create NodeInfo for transport2
        let node_info2 = NodeInfo {
            identifier: NodeIdentifier::new("test-network".to_string(), "node2".to_string()),
            address: addr2.to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Connect transport1 to transport2
        transport1.connect(&node_info2).await?;
        
        // Set up a test message
        let test_message = NetworkMessage {
            source: NodeIdentifier::new("test".to_string(), "node1".to_string()),
            destination: Some(NodeIdentifier::new("test".to_string(), "node2".to_string())),
            message_type: "Request".to_string(),
            correlation_id: Some("test-correlation-id".to_string()),
            topic: "test/service/action".to_string(),
            params: ValueType::String("test-params".to_string()),
            payload: ValueType::Null,
        };
        
        // Send message from transport1 to transport2
        transport1.send(test_message.clone()).await?;
        
        // Give some time for message to be received
        sleep(Duration::from_millis(100)).await;
        
        // Check if message was received
        let received = rx.try_recv();
        assert!(received.is_ok(), "Message should have been received");
        
        if let Ok(msg) = received {
            assert_eq!(msg.source.node_id, "node1");
            assert_eq!(msg.message_type, "Request");
            assert_eq!(msg.correlation_id, Some("test-correlation-id".to_string()));
        }
        
        // Shutdown both transports
        transport1.shutdown().await?;
        transport2.shutdown().await?;
        
        Ok(())
    }
} 