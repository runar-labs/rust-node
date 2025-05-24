use std::sync::Arc;
use std::net::SocketAddr;
use std::time::Duration;
use std::time::SystemTime;
use tokio::sync::mpsc;
use rcgen;
use rustls;
use runar_common::logging::{Component, Logger};
 

use runar_node::network::transport::{
    NetworkMessage, NetworkMessagePayloadItem, NetworkTransport, PeerId,
    quic_transport::{QuicTransport, QuicTransportOptions},
};
use runar_node::network::discovery::multicast_discovery::PeerInfo;
use runar_node::network::discovery::NodeInfo;

/// Custom certificate verifier that skips verification for testing
/// 
/// INTENTION: Allow connections without certificate verification in test environments
struct SkipServerVerification {}

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        // Skip verification and return success
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}

/// Helper function to generate self-signed certificates for testing
/// 
/// INTENTION: Provide a consistent way to generate test certificates across test cases
/// using explicit rustls namespaces to avoid type conflicts
pub fn generate_test_certificates() -> (Vec<rustls::Certificate>, rustls::PrivateKey) {
    // Create certificate parameters with default values
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()]);
    params.alg = &rcgen::PKCS_ECDSA_P256_SHA256;
    params.not_before = rcgen::date_time_ymd(2023, 1, 1);
    params.not_after = rcgen::date_time_ymd(2026, 1, 1);
    
    // Generate the certificate with default parameters
    let cert = rcgen::Certificate::from_params(params)
        .expect("Failed to generate certificate");
        
    // Get the DER encoded certificate and private key
    let cert_der = cert.serialize_der().expect("Failed to serialize certificate");
    let key_der = cert.serialize_private_key_der();
    
    // Convert to rustls types with explicit namespace qualification
    // This avoids conflicts between rustls and Quinn's types
    let rustls_cert = rustls::Certificate(cert_der);
    let rustls_key = rustls::PrivateKey(key_der);
    
    (vec![rustls_cert], rustls_key)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_quic_transport_connection_end_to_end() {
    // Create two transport instances for testing
    let logger_a = Arc::new(Logger::new_root(Component::Network, "transporter-a"));
    let logger_b = Arc::new(Logger::new_root(Component::Network, "transporter-b"));

    // Create node info for both nodes with proper PeerId type
    let node_a_id_str = "node-a".to_string();
    let node_b_id_str = "node-b".to_string();
    let node_a_id = PeerId::new(node_a_id_str.clone());
    let node_b_id = PeerId::new(node_b_id_str.clone());
    
    // Use different ports for each node to avoid conflicts
    // Use higher port numbers to avoid potential conflicts
    let node_a_addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let node_b_addr: SocketAddr = "127.0.0.1:9001".parse().unwrap();
    
    // Create capabilities for the nodes
    let capabilities = vec![];
    
    // Create node info for both nodes
    let node_a_info = NodeInfo {
        peer_id: node_a_id.clone(),
        network_ids: vec!["default".to_string()],  // Use "default" to match what QuicTransport uses internally
        addresses: vec![node_a_addr.to_string()],
        services: capabilities.clone(),
        last_seen: std::time::SystemTime::now(),
    };
    
    let node_b_info = NodeInfo {
        peer_id: node_b_id.clone(),
        network_ids: vec!["default".to_string()],  // Use "default" to match what QuicTransport uses internally
        addresses: vec![node_b_addr.to_string()],
        services: capabilities.clone(),
        last_seen: std::time::SystemTime::now(),
    };
    
    // Log test setup information
    logger_a.info(&format!("Setting up test with Node A (ID: {}, Address: {})", node_a_id, node_a_addr));
    logger_b.info(&format!("Setting up test with Node B (ID: {}, Address: {})", node_b_id, node_b_addr));
    
    // Create the transport instances with proper Logger type
    // Get a runar_common::Logger for each transport
    let common_logger_a = Logger::new_root(Component::Network, "node-a");
    let common_logger_b = Logger::new_root(Component::Network, "node-b");
    
    // Create options for both transports with test certificates and verification disabled
    let (certs_a, key_a) = generate_test_certificates();
    let options_a = QuicTransportOptions::new()
        .with_certificates(certs_a)
        .with_private_key(key_a)
        .with_verify_certificates(false)
        .with_certificate_verifier(Arc::new(SkipServerVerification {}))
        .with_keep_alive_interval(Duration::from_secs(1))
        .with_connection_idle_timeout(Duration::from_secs(60))
        .with_stream_idle_timeout(Duration::from_secs(30));
    
    let (certs_b, key_b) = generate_test_certificates();
    let options_b = QuicTransportOptions::new()
        .with_certificates(certs_b)
        .with_private_key(key_b)
        .with_verify_certificates(false)
        .with_certificate_verifier(Arc::new(SkipServerVerification {}))
        .with_keep_alive_interval(Duration::from_secs(1))
        .with_connection_idle_timeout(Duration::from_secs(60))
        .with_stream_idle_timeout(Duration::from_secs(30));
    
    // Create the transport instances
    let transport_a = Arc::new(QuicTransport::new(
        node_a_id.clone(),
        node_a_addr,
        options_a,
        common_logger_a,
    ).expect("Failed to create transport A"));
    
    let transport_b = Arc::new(QuicTransport::new(
        node_b_id.clone(),
        node_b_addr,
        options_b,
        common_logger_b,
    ).expect("Failed to create transport B"));
    
    // Start both transports
    logger_a.info("Starting transport A");
    transport_a.start().await.expect("Failed to start transport A");
    
    logger_b.info("Starting transport B");
    transport_b.start().await.expect("Failed to start transport B");
    
    // Give the transports a moment to initialize
    tokio::time::sleep(Duration::from_millis(500)).await;
    
    // Create peer info for both nodes with correct fields
    let peer_a_info = PeerInfo {
        addresses: vec![node_a_addr.to_string()],
        public_key: node_a_id_str.clone(), // Using node_id as public key for simplicity
    };
    
    let peer_b_info = PeerInfo {
        addresses: vec![node_b_addr.to_string()],
        public_key: node_b_id_str.clone(), // Using node_id as public key for simplicity
    };
    
    // Create message channels for testing
    let (tx_b, mut rx_b) = mpsc::channel::<NetworkMessage>(10);
    
    // Register message handler for node B
    transport_b.register_message_handler(Box::new(move |msg| {
        let tx = tx_b.clone();
        tokio::spawn(async move {
            let _ = tx.send(msg).await;
        });
        Ok(())
    })).await.expect("Failed to register handler for B");
    
    // Attempt to establish connections between the nodes
    // We'll retry a few times to ensure the connections are established
    let max_retries = 10;
    let mut a_to_b_connected = false;
    let mut b_to_a_connected = false;
    
    for retry in 0..max_retries {
        println!("Retry {}: A->B connected: {}, B->A connected: {}", retry, a_to_b_connected, b_to_a_connected);
        
        // Try to connect A to B if not already connected
        if !a_to_b_connected {
            println!("Attempting to connect A to B (retry {})", retry);
            // Clone the peer info for each connection attempt
            let peer_b_info_clone = peer_b_info.clone();
            let node_a_info_clone = node_a_info.clone();
            match transport_a.connect_peer(peer_b_info_clone, node_a_info_clone).await {
                Ok(peer_node_info) => {
                    a_to_b_connected = true;
                    println!("Successfully connected A to B");
                    
                    // Validate the returned peer node info
                    assert_eq!(peer_node_info.peer_id, node_b_id, "Peer ID in returned NodeInfo doesn't match expected node B ID");
                    assert_eq!(peer_node_info.network_ids, node_b_info.network_ids, "Network IDs in returned NodeInfo don't match");
                    assert!(!peer_node_info.addresses.is_empty(), "Addresses in returned NodeInfo should not be empty");
                },
                Err(e) => println!("Failed to connect A to B: {}", e),
            }
        }
        
        // Try to connect B to A if not already connected
        if !b_to_a_connected {
            println!("Attempting to connect B to A (retry {})", retry);
            // Clone the peer info for each connection attempt
            let peer_a_info_clone = peer_a_info.clone();
            let node_b_info_clone = node_b_info.clone();
            match transport_b.connect_peer(peer_a_info_clone, node_b_info_clone).await {
                Ok(peer_node_info) => {
                    b_to_a_connected = true;
                    println!("Successfully connected B to A");
                    
                    // Validate the returned peer node info
                    assert_eq!(peer_node_info.peer_id, node_a_id, "Peer ID in returned NodeInfo doesn't match expected node A ID");
                    assert_eq!(peer_node_info.network_ids, node_a_info.network_ids, "Network IDs in returned NodeInfo don't match");
                    assert!(!peer_node_info.addresses.is_empty(), "Addresses in returned NodeInfo should not be empty");
                },
                Err(e) => println!("Failed to connect B to A: {}", e),
            }
        }
        
        // If both connections are established, break out of the loop
        if a_to_b_connected && b_to_a_connected {
            println!("Both connections established successfully!");
            break;
        }
        
        // Sleep for a short time before retrying
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    
    println!("Final connection status - A->B: {}, B->A: {}", a_to_b_connected, b_to_a_connected);
    
    // Verify connections by checking if peers are connected
    let a_connected_to_b = transport_a.is_connected(node_b_id.clone()).await;
    let b_connected_to_a = transport_b.is_connected(node_a_id.clone()).await;
    
    println!("Transport A connected to B: {}", a_connected_to_b);
    println!("Transport B connected to A: {}", b_connected_to_a);
    
    // Assert that both connections were established
    assert!(a_to_b_connected, "Failed to establish connection from A to B after {} retries", max_retries);
    assert!(b_to_a_connected, "Failed to establish connection from B to A after {} retries", max_retries);
    
    // Assert that peers are registered in each transport
    assert!(a_connected_to_b, "Node B not connected to Node A");
    assert!(b_connected_to_a, "Node A not connected to Node B");
    
    // Test sending a message from A to B
    
    println!("Testing message sending from A to B");
    let test_message = NetworkMessage {
        source: node_a_id.clone(),
        destination: node_b_id.clone(),
        payloads: vec![NetworkMessagePayloadItem {
            path: "".to_string(),
            value_bytes: vec![1, 2, 3, 4],
            correlation_id: "test-123".to_string(),
        }],
        message_type: "TEST_MESSAGE".to_string(),
    };
    
    // Send message from A to B
    transport_a.send_message(test_message.clone()).await
        .expect("Failed to send message from A to B");
    
    // Wait for the message to be received with timeout
    let received_message = tokio::time::timeout(
        Duration::from_secs(5),
        rx_b.recv()
    ).await
        .expect("Timeout waiting for message")
        .expect("Failed to receive message");
    
    // Verify message contents
    assert_eq!(received_message.source, node_a_id);
    assert_eq!(received_message.destination, node_b_id);
    assert_eq!(received_message.payloads.len(), 1);
    assert_eq!(received_message.payloads[0].path, "");
    
    println!("Message successfully received by B from A");

    
    // Clean up
    println!("Stopping transports");
    transport_a.stop().await.expect("Failed to stop transport A");
    transport_b.stop().await.expect("Failed to stop transport B");
}
