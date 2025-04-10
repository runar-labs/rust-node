// QUIC Transport Tests
//
// Tests for the QUIC transport implementation

use std::sync::Arc;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use std::net::SocketAddr;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::str::FromStr;

use runar_node::network::transport::{NetworkTransport, NetworkMessage, PeerId, TransportFactory, TransportOptions};
use runar_node::network::transport::quic_transport::{QuicTransport, QuicTransportOptions, QuicTransportFactory};
use runar_node::network::discovery::NodeInfo;
use runar_common::types::ValueType;
use runar_common::{Component, Logger};
use slog::{o, Drain, Logger};
use tokio::sync::oneshot;
use uuid;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // Helper function to create and initialize a test QuicTransport
    async fn create_test_transport(options: Option<QuicTransportOptions>) -> Result<Arc<QuicTransport>> {
        // Default to local IP and random port if not specified in options
        let default_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let bind_addr_str = options.as_ref()
            .and_then(|o| o.transport_options.bind_address.as_deref())
            .unwrap_or("127.0.0.1:0"); // Default if None
        
        let bind_addr = SocketAddr::from_str(bind_addr_str)?;
        
        // Ensure options reflect the actual bind address for consistency
        let mut resolved_options = options.unwrap_or_default();
        resolved_options.transport_options.bind_address = Some(bind_addr.to_string());

        // Create a unique PeerId for testing
        let node_id = PeerId::new(format!("test-node-{}", uuid::Uuid::new_v4()));
        
        // Create logger (using runar_common Logger)
        // Note: The original helper used slog directly, let's adapt to runar_common::Logger
        // Assuming Logger::new_root exists and works like this:
        let logger = Logger::new_root(Component::Network, &node_id.node_id);

        // Create transport instance
        let transport = QuicTransport::new(node_id.clone(), resolved_options, logger);
        
        // Initialize the transport
        transport.initialize().await?;

        // Return Arc for easier sharing if needed, or just transport
        Ok(Arc::new(transport))
    }

    #[tokio::test]
    async fn test_quic_transport_local_address() -> Result<()> {
        let transport = create_test_transport(None).await?;
        
        // Check that we got a local address
        let local_addr_str = transport.get_local_address();
        let local_addr = SocketAddr::from_str(&local_addr_str)?;
        assert!(local_addr.port() > 0, "Transport should have a non-zero port");
        assert!(local_addr.ip().is_loopback(), "Transport should bind to loopback by default");
        
        // Stop the transport
        transport.stop().await?;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_connect_and_send() -> Result<()> {
        // Create two transports with default options (which specify 127.0.0.1:0)
        let transport1 = create_test_transport(None).await?;
        let transport2 = create_test_transport(None).await?;
        
        // Get PeerIds and SocketAddrs
        let addr1_str = transport1.get_local_address();
        let addr1 = SocketAddr::from_str(&addr1_str)?;
        let node_id1 = transport1.get_local_node_id();
            
        let addr2_str = transport2.get_local_address();
        let addr2 = SocketAddr::from_str(&addr2_str)?;
        let node_id2 = transport2.get_local_node_id();
            
        println!("Transport 1 ({}) listening on: {}", node_id1, addr1);
        println!("Transport 2 ({}) listening on: {}", node_id2, addr2);
        
        // Create a channel for receiving messages on transport2
        let (tx, mut rx) = mpsc::channel::<NetworkMessage>(10);
        
        // Register a handler on transport2
        transport2.register_message_handler(Box::new(move |msg| {
            println!("Transport 2 received message: {:?}", msg);
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = tx_clone.send(msg).await {
                    eprintln!("Test channel send error: {}", e);
                }
            });
            Ok(())
        }))?;
        
        // Connect transport1 to transport2 (using PeerId and SocketAddr)
        println!(
            "Attempting to connect from {} ({}) to {} ({})",
            addr1, node_id1, addr2, node_id2
        );
        transport1.connect(node_id2.clone(), addr2).await?;
        println!("Connection initiated.");

        // Allow time for connection and handshake
        // Increased sleep time significantly to ensure handshake completes reliably
        tokio::time::sleep(Duration::from_millis(500)).await; 
        
        // Verify connection state
        assert!(transport1.is_connected(node_id2.clone()), "Transport 1 should be connected to Transport 2");
        // is_connected on transport2 might take slightly longer if handshake ACK is slow
        // Let's add a small delay and check again if needed
        tokio::time::sleep(Duration::from_millis(100)).await; 
        assert!(transport2.is_connected(node_id1.clone()), "Transport 2 should be connected to Transport 1");
        println!("Connections verified.");

        // Set up a test message using the new structure
        let test_topic = "test/service/action".to_string();
        let test_payload = ValueType::String("test-payload-data".to_string());
        let test_corr_id = "corr-id-123".to_string();
        
        let message = NetworkMessage {
            source: node_id1.clone(),
            destination: node_id2.clone(),
            message_type: "Request".to_string(), // Can be any type for this test
            payloads: vec![(test_topic.clone(), test_payload.clone(), test_corr_id.clone())],
        };
        
        // Send message from transport1 to transport2
        println!("Sending message from transport1 to transport2: {:?}", message);
        transport1.send_message(message.clone()).await?;
        println!("Message sent via transport1.");
        
        // Check if message was received by transport2's handler
        let received_message = match timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(msg)) => msg,
            Ok(None) => return Err(anyhow!("Test channel closed unexpectedly")),
            Err(_) => return Err(anyhow!("Timeout waiting for message on test channel")),
        };
        println!("Received message via handler: {:?}", received_message);
        
        assert_eq!(received_message.source, node_id1);
        assert_eq!(received_message.destination, node_id2);
        assert_eq!(received_message.message_type, "Request");
        assert_eq!(received_message.payloads.len(), 1);
        assert_eq!(received_message.payloads[0].0, test_topic);
        assert_eq!(received_message.payloads[0].1, test_payload);
        assert_eq!(received_message.payloads[0].2, test_corr_id);
        
        // Shutdown both transports
        transport1.stop().await?;
        transport2.stop().await?;
        
        Ok(())
    }
} 