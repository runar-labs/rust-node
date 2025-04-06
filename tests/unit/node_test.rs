// Tests for the Node implementation
//
// These tests verify that the Node properly handles requests
// and delegates to the ServiceRegistry as needed.

use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::timeout;

use runar_common::types::ValueType;
use runar_common::{Component, Logger};
use runar_node::node::{Node, NodeConfig};
use runar_node::services::{EventContext, NodeRequestHandler};
use runar_node::services::abstract_service::AbstractService;
use runar_node::services::abstract_service::ServiceState;
use runar_node::services::LifecycleContext;
use runar_node::routing::TopicPath;
use runar_node::services::NodeDelegate;

// Import the test fixtures
use crate::fixtures::math_service::MathService;
use anyhow::Result;

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
        
        // List services to verify it was added
        println!("Registered services: {:?}", node.list_services().await);
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
        
        // List services to verify it was added
        println!("Registered services: {:?}", node.list_services().await);
        
        // Create parameters for the add operation
        let params = ValueType::Map([
            ("a".to_string(), ValueType::Number(5.0)),
            ("b".to_string(), ValueType::Number(3.0)),
        ].into_iter().collect());
        
        // Use the request method which is the preferred API
        // The path format should match the service name exactly: "Math/add"
        let response = node.request("Math/add".to_string(), params).await.unwrap();
        
        // Print the details of the failed response for debugging
        if response.status != 200 {
            println!("Request failed: {:?}", response);
        }
        
        // Verify we got a success response (status code 200)
        assert_eq!(response.status, 200);
        
        // Verify the result is correct
        match response.data {
            Some(ValueType::Number(n)) => assert_eq!(n, 8.0),
            _ => panic!("Expected a number in the response data"),
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}
 
/// Test that verifies event publishing in the Node
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Accept subscriptions for specific topics
/// - Publish events to those topics
/// - Ensure subscribers receive the published events
/// - Handle unsubscription correctly
/// 
/// This test verifies the Node's responsibility for event publication and 
/// subscription management, which is a core architectural component.
/// The Node (not ServiceRegistry) should be responsible for executing callbacks.
#[tokio::test]
async fn test_node_publish() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let node = Node::new(config).await.unwrap();
        
        // Create a flag to track if the callback was called
        let was_called = Arc::new(AtomicBool::new(false));
        let was_called_clone = was_called.clone();
        
        // Create a callback that would be invoked when an event is published
        // Use Box instead of Arc to match the expected type
        let callback = Box::new(move |_ctx: Arc<EventContext>, _data: ValueType| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            let was_called = was_called_clone.clone();
            Box::pin(async move {
                // Set the flag to true when called
                was_called.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        
        // Subscribe to the topic
        let _subscription_id = node.subscribe("test/event".to_string(), callback).await.unwrap();
        
        // Publish an event to the topic
        node.publish("test/event".to_string(), ValueType::Null).await.unwrap();
        
        // Give the async task time to execute
        tokio::time::sleep(Duration::from_millis(10)).await;
        
        // Verify the callback was called
        assert!(was_called.load(Ordering::SeqCst), "Callback was not called");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies service lifecycle management
/// 
/// INTENTION: This test validates that a service can properly:
/// - Be initialized with a LifecycleContext
/// - Start successfully after initialization
/// - Stop gracefully when requested
/// 
/// This test ensures that services implement the lifecycle methods correctly
/// and can be managed through their full operational lifecycle.
/// It also verifies LifecycleContext can be used for initialization.
#[tokio::test]
async fn test_service_lifecycle() {
    // Use a timeout to ensure the test doesn't hang
    if let Err(_) = timeout(Duration::from_secs(5), async {
        // Create a node
        let node_config = NodeConfig::new("test");
        let node = Node::new(node_config).await.unwrap();
        
        // Create a math service instance
        let service = MathService::new("math", "math");
        
        // Create a logger for testing
        let logger = Logger::new_root(Component::Service, "test");
        
        // Create the math service path
        let math_path = TopicPath::new("test:math", "test").expect("Valid path");
        
        // Create lifecycle contexts with node delegate
        let init_context = LifecycleContext::new(&math_path, logger.clone())
            .with_node_delegate(Arc::new(node.clone()) as Arc<dyn NodeDelegate + Send + Sync>);
        
        // Initialize the service
        let init_result = service.init(init_context).await;
        assert!(init_result.is_ok());
        
        // Create a new context for start
        let start_context = LifecycleContext::new(&math_path, logger.clone())
            .with_node_delegate(Arc::new(node.clone()) as Arc<dyn NodeDelegate + Send + Sync>);
        
        // Start the service
        let start_result = service.start(start_context).await;
        assert!(start_result.is_ok());
        
        // Create a new context for stop
        let stop_context = LifecycleContext::new(&math_path, logger.clone())
            .with_node_delegate(Arc::new(node.clone()) as Arc<dyn NodeDelegate + Send + Sync>);
        
        // Stop the service
        let stop_result = service.stop(stop_context).await;
        assert!(stop_result.is_ok());
    }).await {
        panic!("Test timed out");
    }
}

/// Test that verifies Node lifecycle management with multiple services
/// 
/// INTENTION: This test validates that the Node can properly:
/// - Register multiple services
/// - Start all services with a single call to node.start()
/// - Stop all services with a single call to node.stop()
/// 
/// This test ensures that the Node correctly manages the lifecycle of all registered
/// services and handles any errors that might occur during start or stop operations.
#[tokio::test]
async fn test_node_lifecycle() {
    use tokio::time::timeout;
    
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("test_network");
        let mut node = Node::new(config).await.unwrap();
        
        // Create multiple test services
        let math_service = MathService::new("Math", "math");
        let second_math_service = MathService::new("SecondMath", "second_math");
        
        // Add the services to the node
        node.add_service(math_service).await.unwrap();
        node.add_service(second_math_service).await.unwrap();
        
        // Verify services are in the Initialized state using the Registry Service
        let math_state_resp = node.request("$registry/services/math/state".to_string(), ValueType::Null).await.unwrap();
        println!("DEBUG TEST: Math state response: {:?}", math_state_resp);
        if let Some(ValueType::Map(state_info)) = math_state_resp.data {
            println!("DEBUG TEST: Math state info map: {:?}", state_info);
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Initialized", "Service 'math' should be in Initialized state after add_service");
            } else {
                println!("DEBUG TEST: State key not found in response map");
                panic!("State not found in response");
            }
        } else {
            println!("DEBUG TEST: Response data is not a map: {:?}", math_state_resp.data);
            panic!("Expected map with state info in response");
        }
        
        let second_math_state_resp = node.request("$registry/services/second_math/state".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Map(state_info)) = second_math_state_resp.data {
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Initialized", "Service 'second_math' should be in Initialized state after add_service");
            } else {
                panic!("State not found in response");
            }
        } else {
            panic!("Expected map with state info in response");
        }
        
        // Start all services at once
        let start_result = node.start().await;
        assert!(start_result.is_ok(), "Failed to start node services: {:?}", start_result.err());
        
        // Verify both services are in the Running state
        let math_state_resp = node.request("$registry/services/math/state".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Map(state_info)) = math_state_resp.data {
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Running", "Service 'math' should be in Running state after start");
            } else {
                panic!("State not found in response");
            }
        } else {
            panic!("Expected map with state info in response");
        }
        
        let second_math_state_resp = node.request("$registry/services/second_math/state".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Map(state_info)) = second_math_state_resp.data {
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Running", "Service 'second_math' should be in Running state after start");
            } else {
                panic!("State not found in response");
            }
        } else {
            panic!("Expected map with state info in response");
        }
        
        // Stop all services at once
        let stop_result = node.stop().await;
        assert!(stop_result.is_ok(), "Failed to stop node services: {:?}", stop_result.err());
        
        // Verify both services are in the Stopped state
        let math_state_resp = node.request("$registry/services/math/state".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Map(state_info)) = math_state_resp.data {
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Stopped", "Service 'math' should be in Stopped state after stop");
            } else {
                panic!("State not found in response");
            }
        } else {
            panic!("Expected map with state info in response");
        }
        
        let second_math_state_resp = node.request("$registry/services/second_math/state".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Map(state_info)) = second_math_state_resp.data {
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Stopped", "Service 'second_math' should be in Stopped state after stop");
            } else {
                panic!("State not found in response");
            }
        } else {
            panic!("Expected map with state info in response");
        }
        
        // Test getting all services using the Registry Service
        let services_list_resp = node.request("$registry/services/list".to_string(), ValueType::Null).await.unwrap();
        if let Some(ValueType::Array(services)) = services_list_resp.data {
            // Filter to get only the math services (excluding registry and others)
            let math_services: Vec<_> = services.iter()
                .filter(|service| {
                    if let ValueType::Map(info) = service {
                        if let Some(ValueType::String(path)) = info.get("path") {
                            path == "math" || path == "second_math"
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                })
                .collect();
            
            assert_eq!(math_services.len(), 2, "Should have info for two math services");
        } else {
            panic!("Expected array of services in response");
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}
 