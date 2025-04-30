use crate::node::{Node, NodeConfig};
use crate::network::transport::{NetworkTransport, NetworkMessage, NodeIdentifier, TransportFactory};
use crate::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use crate::network::capabilities::{ServiceCapability, ActionCapability, EventCapability};
use crate::routing::TopicPath;
use crate::services::{ServiceResponse, ActionHandler, RequestContext};
use runar_common::types::ValueType;
use runar_common::Logger;

use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::{RwLock, oneshot, Mutex};
use uuid::Uuid;
use std::time::SystemTime;
use async_trait::async_trait;
use std::net::SocketAddr;
use std::str::FromStr;

// ===== Mock Network Transport =====

/// Mock network transport for testing
struct MockNetworkTransport {
    local_node_id: NodeIdentifier,
    message_handler: RwLock<Option<Box<dyn Fn(NetworkMessage) -> Result<()> + Send + Sync>>>,
    sent_messages: RwLock<Vec<NetworkMessage>>,
    pending_requests: RwLock<HashMap<String, oneshot::Sender<Result<ServiceResponse>>>>,
    connected_peers: RwLock<HashMap<String, NodeInfo>>,
    local_address: String,
}

impl MockNetworkTransport {
    fn new(local_node_id: NodeIdentifier) -> Self {
        Self {
            local_node_id,
            message_handler: RwLock::new(None),
            sent_messages: RwLock::new(Vec::new()),
            pending_requests: RwLock::new(HashMap::new()),
            connected_peers: RwLock::new(HashMap::new()),
            local_address: "mock://127.0.0.1:1234".to_string(),
        }
    }
    
    /// Simulate receiving a message from a remote node
    async fn simulate_message(&self, message: NetworkMessage) -> Result<()> {
        if let Some(handler) = &*self.message_handler.read().await {
            handler(message)
        } else {
            Err(anyhow!("No message handler registered"))
        }
    }
    
    /// Get all sent messages
    async fn get_sent_messages(&self) -> Vec<NetworkMessage> {
        self.sent_messages.read().await.clone()
    }
    
    /// Clear sent messages
    async fn clear_sent_messages(&self) {
        self.sent_messages.write().await.clear();
    }
}

#[async_trait]
impl NetworkTransport for MockNetworkTransport {
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    
    async fn send(&self, message: NetworkMessage) -> Result<()> {
        self.sent_messages.write().await.push(message.clone());
        Ok(())
    }
    
    async fn connect(&self, node_info: &NodeInfo) -> Result<()> {
        self.connected_peers.write().await.insert(node_info.identifier.to_string(), node_info.clone());
        Ok(())
    }
    
    async fn disconnect(&self, peer_id: &NodeIdentifier) -> Result<()> {
        self.connected_peers.write().await.remove(&peer_id.to_string());
        Ok(())
    }
    
    fn register_handler<F>(&self, handler: F) -> Result<()>
    where
        F: Fn(NetworkMessage) -> Result<()> + Send + Sync + 'static,
    {
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                *self.message_handler.write().await = Some(Box::new(handler));
                Ok(())
            })
        })
    }
    
    async fn local_address(&self) -> Option<String> {
        Some(self.local_address.clone())
    }
}

// ===== Mock Transport Factory =====

struct MockTransportFactory {
    transport: Arc<MockNetworkTransport>,
}

impl MockTransportFactory {
    fn new(transport: Arc<MockNetworkTransport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl TransportFactory for MockTransportFactory {
    async fn create_transport(&self, _node_id: NodeIdentifier, _logger: Logger) -> Result<Box<dyn NetworkTransport>> {
        Ok(Box::new(MockNetworkTransport::new(self.transport.local_node_id.clone())))
    }
}

// ===== Mock Node Discovery =====

struct MockNodeDiscovery {
    discovered_nodes: RwLock<HashMap<String, NodeInfo>>,
    local_node: RwLock<Option<NodeInfo>>,
    discovery_listener: RwLock<Option<DiscoveryListener>>,
}

impl MockNodeDiscovery {
    fn new() -> Self {
        Self {
            discovered_nodes: RwLock::new(HashMap::new()),
            local_node: RwLock::new(None),
            discovery_listener: RwLock::new(None),
        }
    }
    
    /// Simulate discovery of a node
    async fn simulate_discovery(&self, node_info: NodeInfo) -> Result<()> {
        self.discovered_nodes.write().await.insert(node_info.identifier.to_string(), node_info.clone());
        
        if let Some(listener) = &*self.discovery_listener.read().await {
            listener(node_info);
        }
        
        Ok(())
    }
}

#[async_trait]
impl NodeDiscovery for MockNodeDiscovery {
    async fn init(&self, _options: DiscoveryOptions) -> Result<()> {
        Ok(())
    }
    
    async fn start_announcing(&self ) -> Result<()> {
        // *self.local_node.write().await = Some(info);
        Ok(())
    }
    
    async fn stop_announcing(&self) -> Result<()> {
        *self.local_node.write().await = None;
        Ok(())
    }
    
    async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        self.discovered_nodes.write().await.insert(node_info.identifier.to_string(), node_info);
        Ok(())
    }
    
    async fn update_node(&self, node_info: NodeInfo) -> Result<()> {
        self.discovered_nodes.write().await.insert(node_info.identifier.to_string(), node_info);
        Ok(())
    }
    
    async fn discover_nodes(&self, _network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        Ok(self.discovered_nodes.read().await.values().cloned().collect())
    }
    
    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>> {
        let node_identifier = NodeIdentifier::new(network_id.to_string(), node_id.to_string());
        Ok(self.discovered_nodes.read().await.get(&node_identifier.to_string()).cloned())
    }
    
    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        *self.discovery_listener.write().await = Some(listener);
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

// ===== Mock Discovery Factory =====

fn create_mock_discovery(_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>, _logger: Logger) -> Result<Box<dyn NodeDiscovery>> {
    Ok(Box::new(MockNodeDiscovery::new()))
}

// ===== Tests =====

/// Create a test node with mock components for testing
async fn create_test_node() -> (Arc<Node>, Arc<MockNetworkTransport>, Arc<MockNodeDiscovery>) {
    // Create node configuration
    let config = NodeConfig::new_with_node_id("test-network", "test-node")
        .with_networking_enabled(true)
        .with_request_timeout(1000); // Short timeout for tests
    
    // Create the node
    let mut node = Node::new(config).await.unwrap();
    
    // Create mock transport
    let node_id = NodeIdentifier::new("test-network".to_string(), "test-node".to_string());
    let mock_transport = Arc::new(MockNetworkTransport::new(node_id));
    let mock_transport_factory = MockTransportFactory::new(mock_transport.clone());
    
    // Create mock discovery that we can get a reference to
    let mock_discovery = Arc::new(MockNodeDiscovery::new());
    let mock_discovery_clone = mock_discovery.clone();
    let discovery_factory = Box::new(move |_transport, _logger| {
        Ok(Box::new(MockNodeDiscovery::new()))
    });
    
    // Start node with mock components
    node.start(mock_transport_factory, discovery_factory).await.unwrap();
    
    // Return the node and mocks
    (Arc::new(node), mock_transport, mock_discovery)
}

/// Test node initialization with networking components
#[tokio::test]
async fn test_node_init_with_networking() {
    let (node, mock_transport, _) = create_test_node().await;
    
    // Verify networking is enabled
    assert_eq!(node.networking_enabled, true);
    
    // Verify transport is initialized
    assert!(mock_transport.local_address().await.is_some());
}

/// Test handling discovered nodes
#[tokio::test]
async fn test_handle_discovered_node() {
    let (node, _, mock_discovery) = create_test_node().await;
    
    // Create a mock remote node
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    let remote_node_info = NodeInfo {
        identifier: remote_node_id.clone(),
        address: "mock://192.168.1.100:1234".to_string(),
        capabilities: vec!["service1".to_string(), "service2".to_string()],
        last_seen: SystemTime::now(),
    };
    
    // Simulate discovery of the node
    mock_discovery.simulate_discovery(remote_node_info.clone()).await.unwrap();
    
    // Give a moment for async processing
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    
    // Verify node is in discovered nodes map
    let discovered = node.discovered_nodes.read().await;
    assert!(discovered.contains_key(&remote_node_id.to_string()));
}

/// Test service capability processing
#[tokio::test]
async fn test_handle_service_capabilities() {
    let (node, _, _) = create_test_node().await;
    
    // Create a mock remote node ID
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    
    // Create some capability strings
    let capabilities = vec!["service1".to_string(), "service2".to_string()];
    
    // Process them directly
    node.handle_service_capabilities(remote_node_id.clone(), capabilities).await.unwrap();
    
    // Try to make a request to a remote action that should have been registered
    let result = node.request("service1/get".to_string(), ValueType::Null).await;
    
    // It will fail due to no transport in the test, but it should find the handler
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Network transport not available"));
}

/// Test announcing local services
// #[tokio::test]
// async fn test_announce_local_services() {
//     let (node, mock_transport, _) = create_test_node().await;
    
//     // Reset sent messages
//     mock_transport.clear_sent_messages().await;
    
//     // Create a mock remote node ID
//     let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    
//     // Announce services to it
//     node.announce_local_services(&remote_node_id).await.unwrap();
    
//     // Verify an announcement message was sent
//     let sent_messages = mock_transport.get_sent_messages().await;
//     assert!(!sent_messages.is_empty());
    
//     // Find the service announcement message
//     let announcement = sent_messages.iter().find(|m| m.message_type == "ServiceAnnouncement");
//     assert!(announcement.is_some());
    
//     // Verify it's sent to the right node
//     let announcement = announcement.unwrap();
//     assert_eq!(announcement.destination.as_ref().unwrap(), &remote_node_id);
// }

/// Test handling a network request
#[tokio::test]
async fn test_handle_network_request() {
    let (node, mock_transport, _) = create_test_node().await;
    
    // Reset sent messages
    mock_transport.clear_sent_messages().await;
    
    // Create a mock request message
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    let request = NetworkMessage {
        source: remote_node_id.clone(),
        destination: Some(NodeIdentifier::new(node.network_id.clone(), node.node_id.clone())),
        message_type: "Request".to_string(),
        correlation_id: Some("test-correlation-id".to_string()),
        topic: "registry/list_services".to_string(), // Call the registry service
        params: ValueType::Null,
        payload: ValueType::Null,
    };
    
    // Send the request directly to the handler
    node.handle_network_request(request).await.unwrap();
    
    // Verify a response was sent
    let sent_messages = mock_transport.get_sent_messages().await;
    assert!(!sent_messages.is_empty());
    
    // Find the response message
    let response = sent_messages.iter().find(|m| m.message_type == "Response");
    assert!(response.is_some());
    
    // Verify response details
    let response = response.unwrap();
    assert_eq!(response.correlation_id.as_ref().unwrap(), "test-correlation-id");
    assert_eq!(response.destination.as_ref().unwrap(), &remote_node_id);
}

/// Test handling a network response
#[tokio::test]
async fn test_handle_network_response() {
    let (node, _, _) = create_test_node().await;
    
    // Set up a pending request
    let correlation_id = "test-correlation-id".to_string();
    let (sender, receiver) = oneshot::channel();
    node.pending_requests.write().await.insert(correlation_id.clone(), sender);
    
    // Create a mock response message
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    let response = NetworkMessage {
        source: remote_node_id.clone(),
        destination: Some(NodeIdentifier::new(node.network_id.clone(), node.node_id.clone())),
        message_type: "Response".to_string(),
        correlation_id: Some(correlation_id),
        topic: "test/action".to_string(),
        params: ValueType::Null,
        payload: ValueType::Map(vec![
            ("success".to_string(), ValueType::Bool(true)),
            ("data".to_string(), ValueType::String("test-response".to_string())),
        ].into_iter().collect()),
    };
    
    // Handle the response
    node.handle_network_response(response).await.unwrap();
    
    // Check if the response was received
    let result = receiver.await.unwrap();
    assert!(result.is_ok());
    
    // Verify the response data
    let response = result.unwrap();
    assert_eq!(response.data, Some(ValueType::String("test-response".to_string())));
}

/// Test the round-robin load balancer
#[tokio::test]
async fn test_round_robin_load_balancer() {
    let (node, _, _) = create_test_node().await;
    
    // Create multiple handlers
    let handler1: ActionHandler = Arc::new(|_, _| {
        Box::pin(async { Ok(ServiceResponse { data: Some(ValueType::String("handler1".to_string())), status: 200, error: None }) })
    });
    
    let handler2: ActionHandler = Arc::new(|_, _| {
        Box::pin(async { Ok(ServiceResponse { data: Some(ValueType::String("handler2".to_string())), status: 200, error: None }) })
    });
    
    let handler3: ActionHandler = Arc::new(|_, _| {
        Box::pin(async { Ok(ServiceResponse { data: Some(ValueType::String("handler3".to_string())), status: 200, error: None }) })
    });
    
    // Create a vector of handlers
    let handlers = vec![handler1, handler2, handler3];
    
    // Use the load balancer multiple times
    let mut results = Vec::new();
    for _ in 0..6 {
        let selected = node.load_balancer.select_handler(&handlers);
        
        // Execute the handler to see which one was selected
        let context = RequestContext::new(
            &TopicPath::new("test/action", &node.network_id).unwrap(),
            Logger::new_root(runar_common::Component::Service, "test"),
        );
        
        let response = selected(None, context).await.unwrap();
        results.push(response.data.unwrap());
    }
    
    // We should have seen each handler twice in our 6 selections
    let handler1_count = results.iter().filter(|r| matches!(r, ValueType::String(s) if s == "handler1")).count();
    let handler2_count = results.iter().filter(|r| matches!(r, ValueType::String(s) if s == "handler2")).count();
    let handler3_count = results.iter().filter(|r| matches!(r, ValueType::String(s) if s == "handler3")).count();
    
    // In a round-robin, we expect each handler to be used approximately the same number of times
    assert_eq!(handler1_count, 2);
    assert_eq!(handler2_count, 2);
    assert_eq!(handler3_count, 2);
}

/// Test creating a remote action handler
#[tokio::test]
async fn test_create_remote_action_handler() {
    let (node, mock_transport, _) = create_test_node().await;
    
    // Reset sent messages
    mock_transport.clear_sent_messages().await;
    
    // Create required parameters for a remote action handler
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    let topic_path = TopicPath::new("service/action", &node.network_id).unwrap();
    let local_node_id = NodeIdentifier::new(node.network_id.clone(), node.node_id.clone());
    
    // Create the remote action handler
    let handler = node.create_remote_action_handler(
        remote_node_id.clone(),
        topic_path.clone(),
        node.network_transport.clone(),
        node.logger.clone(),
        local_node_id.clone(),
        Some(1000), // 1 second timeout
    );
    
    // Create a context and call the handler
    let context = RequestContext::new(
        &topic_path,
        Logger::new_root(runar_common::Component::Service, &topic_path.action_path()),
    );
    
    // Call the handler (will eventually fail due to no response, but should send a message)
    let test_param = ValueType::String("test-param".to_string());
    let _ = handler(Some(test_param.clone()), context).await;
    
    // Verify a request message was sent
    let sent_messages = mock_transport.get_sent_messages().await;
    assert!(!sent_messages.is_empty());
    
    // Get the latest message
    let request = sent_messages.last().unwrap();
    
    // Verify request details
    assert_eq!(request.message_type, "Request");
    assert_eq!(request.destination.as_ref().unwrap(), &remote_node_id);
    assert_eq!(request.topic, "service/action");
    assert_eq!(request.params, test_param);
}

/// Test end-to-end request handling with a remote node
#[tokio::test]
async fn test_end_to_end_request() {
    let (node, mock_transport, _) = create_test_node().await;
    
    // Reset sent messages
    mock_transport.clear_sent_messages().await;
    
    // Create a mock remote node
    let remote_node_id = NodeIdentifier::new("test-network".to_string(), "remote-node".to_string());
    let remote_node_info = NodeInfo {
        identifier: remote_node_id.clone(),
        address: "mock://192.168.1.100:1234".to_string(),
        capabilities: vec!["test-service".to_string()],
        last_seen: SystemTime::now(),
    };
    
    // Register the node and its capabilities
    node.handle_discovered_node(remote_node_info).await.unwrap();
    
    // Make a request to the test service
    let request_future = node.request("test-service/get".to_string(), ValueType::Null);
    
    // Process the outgoing message and create a response
    let mut sent_messages = mock_transport.get_sent_messages().await;
    assert!(!sent_messages.is_empty());
    
    // Get the sent request
    let request = sent_messages.pop().unwrap();
    assert_eq!(request.message_type, "Request");
    
    // Create a response message
    let response = NetworkMessage {
        source: remote_node_id.clone(),
        destination: Some(NodeIdentifier::new(node.network_id.clone(), node.node_id.clone())),
        message_type: "Response".to_string(),
        correlation_id: request.correlation_id.clone(),
        topic: request.topic.clone(),
        params: ValueType::Null,
        payload: ValueType::Map(vec![
            ("success".to_string(), ValueType::Bool(true)),
            ("data".to_string(), ValueType::String("response-data".to_string())),
        ].into_iter().collect()),
    };
    
    // Manually deliver the response
    mock_transport.simulate_message(response).await.unwrap();
    
    // Now the request should complete successfully
    let result = request_future.await.unwrap();
    assert_eq!(result.data, Some(ValueType::String("response-data".to_string())));
} 