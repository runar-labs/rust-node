// QUIC Transport Tests
//
// Tests for the QUIC transport implementation

use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use std::net::SocketAddr;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::str::FromStr;

use runar_node::network::transport::{NetworkTransport, NetworkMessage, PeerId, TransportOptions};
use runar_node::network::transport::quic_transport::{QuicTransport, QuicTransportOptions, pick_free_port};
use runar_common::types::ValueType;
use runar_common::Logger;
use runar_common::Component;
use tokio::sync::oneshot;
use uuid::Uuid;
use rustls::{Certificate, PrivateKey};

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    // Helper function to create a self-signed certificate for testing
    fn create_test_certificates() -> (Vec<Certificate>, PrivateKey) {
        // Generate a self-signed certificate for testing only
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = cert.serialize_der().unwrap();
        let priv_key = cert.serialize_private_key_der();
        let priv_key = PrivateKey(priv_key);
        let cert_chain = vec![Certificate(cert_der)];
        
        (cert_chain, priv_key)
    }

    // Helper function to create and initialize a test QuicTransport
    async fn create_test_transport(options: Option<QuicTransportOptions>) -> Result<Arc<QuicTransport>> {
        // Create a unique PeerId for testing
        let node_id = PeerId::new(format!("test-node-{}", Uuid::new_v4()));
        
        // Create logger using runar_common Logger
        let logger = Logger::new_root(Component::Network, &node_id.node_id);

        // If no options are provided, create default options with test certificates
        let transport_options = if let Some(opts) = options {
            opts
        } else {
            let (certs, key) = create_test_certificates();
            QuicTransportOptions::new()
                .with_certificates(certs)
                .with_private_key(key)
                .with_verify_certificates(false)
        };
        
        // Create transport instance with the options
        let transport = QuicTransport::new(
            node_id.clone(), 
            transport_options, 
            logger
        );
        
        // Initialize the transport
        transport.initialize().await?;
        
        // Start the transport
        transport.start().await?;

        // Return Arc for easier sharing if needed
        Ok(Arc::new(transport))
    }

    #[tokio::test]
    async fn test_quic_transport_local_address() -> Result<()> {
        // Create custom options with a dynamic port range for the test using builder pattern
        let (certs, key) = create_test_certificates();
        let options = QuicTransportOptions::new()
            .with_port_in_range(50000..50500)
            .with_certificates(certs)
            .with_private_key(key)
            .with_verify_certificates(false);
        
        let transport = create_test_transport(Some(options)).await?;
        
        // Check that we got a local address
        let local_addr_str = transport.get_local_address();
        let local_addr = SocketAddr::from_str(&local_addr_str)?;
        assert!(local_addr.port() > 0, "Transport should have a non-zero port");
        assert!(local_addr.ip().is_loopback(), "Transport should bind to loopback address");
        
        // Stop the transport
        transport.stop().await?;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_connect_and_send() -> Result<()> {
        // Create test certificates
        let (certs1, key1) = create_test_certificates();
        let (certs2, key2) = create_test_certificates();
        
        // Create two transports with custom options using builder pattern
        let options1 = QuicTransportOptions::new()
            .with_port_in_range(50500..51000)
            .with_certificates(certs1)
            .with_private_key(key1)
            .with_verify_certificates(false);
        
        let options2 = QuicTransportOptions::new()
            .with_port_in_range(51000..51500)
            .with_certificates(certs2)
            .with_private_key(key2)
            .with_verify_certificates(false);
        
        let transport1 = create_test_transport(Some(options1)).await?;
        let transport2 = create_test_transport(Some(options2)).await?;
        
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
        tokio::time::sleep(Duration::from_millis(500)).await; 
        
        // Verify connection state with improved retry logic
        let max_retries = 10;
        let retry_delay = Duration::from_millis(100);
        
        // Check transport1 connection to transport2
        let mut retry_count = 0;
        while !transport1.is_connected(node_id2.clone()) && retry_count < max_retries {
            println!("Waiting for transport1 to recognize connection to transport2 (attempt {}/{})", 
                     retry_count + 1, max_retries);
            tokio::time::sleep(retry_delay).await;
            retry_count += 1;
        }
        
        if !transport1.is_connected(node_id2.clone()) {
            return Err(anyhow!("Transport 1 failed to connect to Transport 2 after {} attempts", max_retries));
        }
        println!("Transport 1 connected to Transport 2 successfully.");
        
        // Check transport2 connection to transport1
        let mut retry_count = 0;
        while !transport2.is_connected(node_id1.clone()) && retry_count < max_retries {
            println!("Waiting for transport2 to recognize connection to transport1 (attempt {}/{})", 
                     retry_count + 1, max_retries);
            tokio::time::sleep(retry_delay).await;
            retry_count += 1;
        }
        
        if !transport2.is_connected(node_id1.clone()) {
            return Err(anyhow!("Transport 2 failed to connect to Transport 1 after {} attempts", max_retries));
        }
        println!("Transport 2 connected to Transport 1 successfully.");
        
        println!("Connections verified.");

        // Set up a test message using the updated structure with payloads
        let test_topic = "test/service/action".to_string();
        let test_payload = ValueType::String("test-payload-data".to_string());
        let test_corr_id = "corr-id-123".to_string();
        
        let message = NetworkMessage {
            source: node_id1.clone(),
            destination: node_id2.clone(),
            message_type: "Request".to_string(),
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