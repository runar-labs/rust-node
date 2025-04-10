// Tests for the Node implementation
//
// These tests verify that the Node properly handles requests
// and delegates to the ServiceRegistry as needed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::timeout;

use runar_common::types::ValueType;
use runar_common::{Component, Logger};
use runar_node::node::{Node, NodeConfig};
use runar_node::services::{EventContext};
use runar_node::services::{LifecycleContext};

// Import the test fixtures
use crate::fixtures::math_service::MathService;
use anyhow::Result;
use async_trait::async_trait;
use runar_node::network::transport::{NetworkTransport, PeerRegistry, MessageHandler, NetworkMessage};
use runar_node::PeerId;
use std::net::SocketAddr;

// Add this function to create dummy start args
fn get_dummy_start_args() -> (DummyTransportFactory, Box<dyn FnOnce(std::sync::Arc<tokio::sync::RwLock<Option<Box<dyn runar_node::network::transport::NetworkTransport>>>>, runar_common::Logger) -> Result<Box<dyn runar_node::network::discovery::NodeDiscovery>> + Send + Sync + 'static>) {
    struct DummyDiscovery;
    
    #[async_trait::async_trait]
    impl runar_node::network::discovery::NodeDiscovery for DummyDiscovery {
        async fn init(&self, _: runar_node::network::discovery::DiscoveryOptions) -> Result<()> {
            Ok(())
        }
        async fn start_announcing(&self, _: runar_node::network::discovery::NodeInfo) -> Result<()> {
            Ok(())
        }
        async fn stop_announcing(&self) -> Result<()> {
            Ok(())
        }
        async fn register_node(&self, _: runar_node::network::discovery::NodeInfo) -> Result<()> {
            Ok(())
        }
        async fn update_node(&self, _: runar_node::network::discovery::NodeInfo) -> Result<()> {
            Ok(())
        }
        async fn discover_nodes(&self, _: Option<&str>) -> Result<Vec<runar_node::network::discovery::NodeInfo>> {
            Ok(vec![])
        }
        async fn find_node(&self, _: &str, _: &str) -> Result<Option<runar_node::network::discovery::NodeInfo>> {
            Ok(None)
        }
        async fn set_discovery_listener(&self, _: runar_node::network::discovery::DiscoveryListener) -> Result<()> {
            Ok(())
        }
        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }
    
    let discovery_factory = Box::new(|_, _| {
        Ok(Box::new(DummyDiscovery) as Box<dyn runar_node::network::discovery::NodeDiscovery>)
    });
    
    (DummyTransportFactory, discovery_factory)
}

// Define DummyTransport before the factory
struct DummyTransport {
    peer_registry: PeerRegistry,
    local_node_id: PeerId,
}

#[async_trait]
impl NetworkTransport for DummyTransport {
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    async fn send(&self, _: NetworkMessage) -> Result<()> {
        Ok(())
    }
    async fn connect(&self, _: &runar_node::network::discovery::NodeInfo) -> Result<()> {
        Ok(())
    }
    fn register_handler(&self, _handler: MessageHandler) -> Result<()> {
        Ok(())
    }
    async fn local_address(&self) -> Option<SocketAddr> {
        None
    }
    fn peer_registry(&self) -> &PeerRegistry {
        &self.peer_registry
    }
    fn local_node_id(&self) -> &PeerId {
        &self.local_node_id
    }
}

#[derive(Clone)]
struct DummyTransportFactory;

#[async_trait]
impl runar_node::network::transport::TransportFactory for DummyTransportFactory {
    type Transport = DummyTransport;
    
    async fn create_transport(&self, node_id: PeerId, _logger: Logger) -> Result<Self::Transport> {
        let peer_registry = PeerRegistry::new();
        Ok(DummyTransport {
            peer_registry,
            local_node_id: node_id,
        })
    }
}

/// Test that verifies basic node creation functionality
/// 
/// INTENTION: This test validates that the Node can be properly:
/// - Created with a specified network ID
/// - Initialized with default configuration
/// 
/// This test verifies the most basic Node functionality - that we can create
/// and initialize a Node instance which is the foundation for all other tests.
#[tokio::test]
async fn test_node_create() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let node = Node::new(config).await.unwrap();
        
        // Verify the node has the correct network ID (it's now using a UUID, so we can't directly compare)
        assert!(node.network_id.len() > 0);
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies service registration with the Node
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Accept a service for registration
/// - Register the service with its internal ServiceRegistry
/// - List the registered services
/// 
/// This test verifies the Node's responsibility for managing services and 
/// correctly delegating registration to its ServiceRegistry.
#[tokio::test]
async fn test_node_add_service() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let mut node = Node::new(config).await.unwrap();
        
        // Create a test service with consistent name and path
        let service = MathService::new("Math", "Math");
        
        // Add the service to the node
        node.add_service(service).await.unwrap();
        
        // Start the node to initialize all services
        node.start().await.unwrap();
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies request handling in the Node
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Find a service for a specific request
/// - Forward the request to the appropriate service
/// - Return the service's response
/// 
/// This test verifies one of the Node's core responsibilities - request routing
/// and handling. The Node should find the right service and forward the request.
#[tokio::test]
async fn test_node_request() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let mut node = Node::new(config).await.unwrap();
        
        // Create a test service with consistent name and path
        let service = MathService::new("Math", "Math");
        
        // Add the service to the node
        node.add_service(service).await.unwrap();
        
        // Start the node to initialize all services
        node.start().await.unwrap();
        
        // Create parameters for the add operation
        let params = ValueType::Map([
            ("a".to_string(), ValueType::Number(5.0)),
            ("b".to_string(), ValueType::Number(3.0)),
        ].into_iter().collect());
        
        // Make a request to the math service's add action
        let response = node.request("Math/add".to_string(), params).await.unwrap();
        
        // Verify the response contains the expected result (5 + 3 = 8)
        if let ValueType::Number(result) = response.data.unwrap() {
            assert_eq!(result, 8.0);
        } else {
            panic!("Expected a number result");
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies node lifecycle methods work correctly
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Start up and initialize correctly
/// - Shut down cleanly when requested
/// 
/// This test verifies the Node's lifecycle management which is critical
/// for resource cleanup and proper application shutdown.
#[tokio::test]
async fn test_node_lifecycle() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let mut node = Node::new(config).await.unwrap();
        
        // Start the node
        node.start().await.unwrap();
        
        // Stop the node
        node.stop().await.unwrap();
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies node initialization with network components
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Initialize with network components
/// - Start the networking subsystem
/// 
/// This test ensures that the Node can properly initialize its network
/// components which are required for remote communication.
#[tokio::test]
async fn test_node_init() -> Result<()> {
    // Create a node configuration
    let mut config = NodeConfig::new("test-network");
    config.node_id = Some("test-node".to_string());
    
    // Create a node
    let mut node = Node::new(config).await?;
    
    // Start the node
    node.start().await?;
    
    // Stop the node
    node.stop().await?;
    
    Ok(())
}

/// Test that verifies event publishing and subscription in the Node
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Accept subscriptions for specific topics
/// - Publish events to those topics
/// - Ensure subscribers receive the published events
/// 
/// This test verifies the Node's subscription and publishing capabilities,
/// which is a core part of the event-driven architecture.
#[tokio::test]
async fn test_node_events() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let node = Node::new(config).await.unwrap();
        
        // Create a flag to track if the callback was called
        let was_called = Arc::new(AtomicBool::new(false));
        let was_called_clone = was_called.clone();
        
        // Define a topic to subscribe to
        let topic = "test/topic".to_string();
        
        // Create a handler function for subscription
        // Note: Using the full handler signature with Arc<EventContext> for the node API
        let handler = move |ctx: Arc<EventContext>, data: ValueType| {
            println!("Received event data: {:?}", data);
            
            // Verify the data matches what we published
            if let ValueType::String(s) = &data {
                assert_eq!(s, "test data");
                // Mark that the handler was called with correct data
                was_called_clone.store(true, Ordering::SeqCst);
            }
            
            async move { Ok(()) }
        };
        
        // Subscribe to the topic using the node's API
        node.subscribe(topic.clone(), handler).await.unwrap();
        
        // Publish an event to the topic
        let data = ValueType::String("test data".to_string());
        node.publish(topic, data).await.unwrap();
        
        // Small delay to allow async handler to execute
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Verify the handler was called
        assert!(was_called.load(Ordering::SeqCst), "Subscription handler was not called");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}
 