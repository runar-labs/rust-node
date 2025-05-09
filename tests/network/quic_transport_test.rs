// QUIC Transport Tests
//
// Tests for the QUIC transport implementation

use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::sync::mpsc;
use tokio::time::{Duration, timeout};
use std::net::{SocketAddr, IpAddr, Ipv4Addr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::collections::HashSet;

use runar_node::network::transport::{NetworkTransport, NetworkMessage, PeerId, TransportOptions, pick_free_port};
use runar_node::network::transport::quic_transport::QuicTransport;
use runar_node::node::NetworkConfig;
use runar_common::types::{ArcValueType, SerializerRegistry};
use runar_common::Logger;
use runar_common::Component;
use uuid::Uuid;
use runar_node::network::discovery::NodeInfo;
use runar_node::network::discovery::multicast_discovery::PeerInfo;

#[cfg(test)]
mod tests {
    use runar_node::network::transport::NetworkMessagePayloadItem;

    use super::*;

    // Helper function to create and initialize a test QuicTransport with custom port
    async fn create_test_transport(port: Option<u16>) -> Result<Arc<QuicTransport>> {
        // Create a unique PeerId for testing
        let node_id = PeerId::new(format!("test-node-{}", Uuid::new_v4()));
        
        // Create logger using runar_common Logger
        let logger = Logger::new_root(Component::Network, &node_id.public_key);

        // Create a NetworkConfig with QUIC, using false to disable certificate validation
        let mut config = NetworkConfig::with_quic(false);
        
        // Set a specific bind address with localhost (127.0.0.1)
        // If port is None, the OS will choose a random free port (use 0)
        let bind_port = port.unwrap_or(0);
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind_port);
        
        // Replace the transport_options with our custom bind address
        let transport_options = TransportOptions {
            timeout: Some(Duration::from_secs(30)),
            max_message_size: Some(1024 * 1024), // 1MB
            bind_address: bind_addr,
        };
        config.transport_options = transport_options;
        
        // Create transport instance with the config
        let transport = QuicTransport::new(
            node_id.clone(), 
            config, 
            logger
        );
         
        // Start the transport
        transport.start().await?;

        // Return Arc for easier sharing if needed
        Ok(Arc::new(transport))
    }
    
    #[test]
    fn test_pick_free_port_randomized() -> Result<()> {
        // Define a port range for testing
        let port_range = 40000..50000;
        
        // Try to find multiple free ports to verify randomization
        let mut ports = HashSet::new();
        
        // Try finding 5 different ports
        for _ in 0..5 {
            match pick_free_port(port_range.clone()) {
                Some(port) => {
                    // The port should be within the specified range
                    assert!(port >= port_range.start);
                    assert!(port < port_range.end);
                    
                    // Verify the port is actually usable for TCP
                    let tcp_result = TcpListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
                    assert!(tcp_result.is_ok(), "Port {} should be available for TCP", port);
                    
                    // Verify the port is actually usable for UDP
                    let udp_result = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port));
                    assert!(udp_result.is_ok(), "Port {} should be available for UDP", port);
                    
                    // Save the port to check for randomness
                    ports.insert(port);
                },
                None => {
                    // It's unlikely that no free port is found in such a large range,
                    // so this is probably an error
                    panic!("Failed to find a free port in range {:?}", port_range);
                }
            }
        }
        
        // If the function is properly randomized, we should get multiple different ports
        // However, there's a small chance all 5 could be the same just by coincidence
        // So we'll just check that we found at least 2 different ports
        assert!(ports.len() > 1, "Expected multiple different ports, found: {:?}", ports);
        
        Ok(())
    }

    #[tokio::test]
    async fn test_quic_transport_local_address() -> Result<()> {
        // Create transport with port 0 - OS will assign a free port
        let transport = create_test_transport(None).await?;
        
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
        // Create two transports with automatic port assignment
        // Let the OS choose free ports to avoid conflicts
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
                    log::error!("Test channel send error: {}", e);
                }
            });
            Ok(())
        }))?;
        
        // Connect transport1 to transport2 (using PeerId and SocketAddr)
        println!(
            "Attempting to connect from {} ({}) to {} ({})",
            addr1, node_id1, addr2, node_id2
        );
        
        // Create a local node info to pass to connect_node
        let local_node_info = NodeInfo {
            peer_id: node_id1.clone(),
            network_ids: vec!["test-network".to_string()],
            addresses: vec![addr1.to_string()],
            capabilities: vec![],
            last_seen: std::time::SystemTime::now(),
        };
        
        // Create the PeerInfo for the remote node
        let peer_info = PeerInfo {
            public_key: node_id2.public_key.clone(),
            addresses: vec![addr2.to_string()],
        };
        
        // Create a test connection - add a sleep to allow for connection processing
        transport1.connect_node(peer_info, local_node_info).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
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
        let test_payload = ArcValueType::new_primitive("test-payload-data".to_string());
        
        let registry = SerializerRegistry::with_defaults();
        // Serialize the ArcValueType
        let serialized_payload = registry.serialize_value(&test_payload)?;
        let test_corr_id = "corr-id-123".to_string();
        
        let message = NetworkMessage {
            source: node_id1.clone(),
            destination: node_id2.clone(),
            message_type: "Request".to_string(),
            payloads: vec![NetworkMessagePayloadItem::new(
                test_topic.clone(),
                serialized_payload.to_vec(),
                test_corr_id.clone(),
            )],
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
        assert!(!received_message.payloads.is_empty(), "No payloads in received message");
        assert_eq!(received_message.payloads[0].path, test_topic);
        // If test_payload is ArcValueType, compare to value_bytes as bytes
        // Otherwise, ensure both are Vec<u8>
        // Compare serialized bytes directly
        let registry = SerializerRegistry::with_defaults();
        let expected_bytes = registry.serialize_value(&test_payload)?;
        assert_eq!(received_message.payloads[0].value_bytes, expected_bytes.as_ref().to_vec());
        assert_eq!(received_message.payloads[0].correlation_id, test_corr_id);
        
        // Shutdown both transports
        if let Err(e) = transport1.stop().await {
            println!("Error shutting down transport1: {:?}", e);
        }
        if let Err(e) = transport2.stop().await {
            println!("Error shutting down transport2: {:?}", e);
        }
        
        Ok(())
    }

    #[tokio::test]
    async fn test_handshake_serialization() -> Result<()> {
        // This test specifically aims to debug the handshake serialization issue with ValueType
        
        // Create a NodeInfo similar to what would be sent in a handshake
        let local_node_info = NodeInfo {
            peer_id: PeerId::new(format!("test-node-{}", Uuid::new_v4())),
            network_ids: vec!["test-network".to_string()],
            addresses: vec!["127.0.0.1:12345".to_string()],
            capabilities: vec![],
            last_seen: std::time::SystemTime::now(),
        };
        
        // Step 1: Create an ArcValueType from the NodeInfo
        println!("Step 1: Creating ArcValueType from NodeInfo");
        let arc_value_type = ArcValueType::from_struct(local_node_info.clone());
        
        // Create a SerializerRegistry for serialization
        let registry = SerializerRegistry::with_defaults();
        
        // Convert ValueType to binary
        println!("Step 2: Converting ValueType to binary");
        let binary_data = bincode::serialize(&local_node_info).expect("Failed to serialize NodeInfo");
        
        // Step 3: Create a NetworkMessage with the binary payload
        println!("Step 3: Creating NetworkMessage with binary payload");
        let source_id = PeerId::new(format!("test-node-{}", Uuid::new_v4()));
        let dest_id = PeerId::new(format!("test-node-{}", Uuid::new_v4()));
        
        let handshake_msg = NetworkMessage {
            source: source_id.clone(),
            destination: dest_id.clone(),
            message_type: "Handshake".to_string(),
            payloads: vec![NetworkMessagePayloadItem::new(
                "".to_string(),
                binary_data,
                "".to_string(),
            )],
        };
        
        // Step 4: Serialize the NetworkMessage with bincode
        println!("Step 4: Serializing NetworkMessage with bincode");
        
        let serialized_data = match bincode::serialize(&handshake_msg) {
            Ok(data) => data,
            Err(e) => {
                println!("Serialization failed: {}", e);
                return Err(anyhow!("Serialization error: {}", e));
            }
        };
        println!("Serialized message size: {} bytes", serialized_data.len());
        
        // Debug: Print the serialized data as hex dump to analyze it
        println!("Serialized data hex dump (first 100 bytes):");
        for (i, chunk) in serialized_data.chunks(16).enumerate().take(6) {
            let hex = chunk.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
            println!("{:04x}: {}", i * 16, hex);
        }
        
        // Step 5: Attempt to deserialize back to NetworkMessage
        println!("Step 5: Deserializing back to NetworkMessage");
        let deserialized_msg = match bincode::deserialize::<NetworkMessage>(&serialized_data) {
            Ok(msg) => {
                println!("Successfully deserialized NetworkMessage");
                msg
            },
            Err(e) => {
                println!("Failed to deserialize: {}", e);
                
                // Try to deserialize in steps to isolate the problem
                println!("\nAttempting to deserialize message fields individually:");
                
                // Create a slice with the first 100 bytes for debugging
                let sample = &serialized_data[..std::cmp::min(100, serialized_data.len())];
                println!("First 100 bytes: {:?}", sample);
                
                return Err(anyhow!("Deserialization error: {}", e));
            }
        };
        
        // Step 6: Try to extract the NodeInfo from the payload
        println!("Step 6: Extracting NodeInfo from binary payload");
        if let Some(    NetworkMessagePayloadItem { path: _, value_bytes: payload_bytes, correlation_id: _ }) = deserialized_msg.payloads.get(0) {
            // Deserialize the binary data back to NodeInfo
            let node_info: NodeInfo = match bincode::deserialize(payload_bytes) {
                Ok(info) => {
                    println!("Successfully deserialized NodeInfo from binary payload");
                    info
                },
                Err(e) => {
                    println!("Failed to deserialize NodeInfo from binary payload: {}", e);
                    return Err(anyhow!("NodeInfo deserialization error: {}", e));
                }
            };
            
            println!("Successfully extracted NodeInfo: {:?}", node_info);
            assert_eq!(node_info.peer_id, local_node_info.peer_id);
            assert_eq!(node_info.network_ids, local_node_info.network_ids);
            assert_eq!(node_info.addresses, local_node_info.addresses);
        } else {
            return Err(anyhow!("No payload found in deserialized message"));
        }
        
        Ok(())
    }
} 