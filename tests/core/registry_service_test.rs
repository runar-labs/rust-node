// Tests for the Registry Service
//
// INTENTION: Verify that the Registry Service correctly provides
// information about registered services through standard requests.

use std::time::Duration;
use tokio::time::timeout;
use runar_common::types::ValueType;
use runar_node::node::{Node, NodeConfig};

// Import the test fixtures
use crate::fixtures::math_service::MathService;

/// Test that the Registry Service correctly lists all services
///
/// INTENTION: This test validates that:
/// - The Registry Service is automatically registered during Node creation
/// - It properly responds to a services/list request
/// - The response contains expected service information
#[tokio::test]
async fn test_registry_service_list_services() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("node-reg-list".to_string(), "test_network".to_string());
        let mut node = Node::new(config).await.unwrap();
        
        // Create a test service
        let math_service = MathService::new("Math", "math");
        
        // Add the service to the node
        node.add_service(math_service).await.unwrap();
        
        // Use the request method to query the registry service
        let response = node.request("$registry/services/list".to_string(), ValueType::Null).await.unwrap();
        
        // Verify response is successful
        assert_eq!(response.status, 200, "Registry service request failed: {:?}", response);
        
        // Parse the response to verify it contains our registered services
        if let Some(ValueType::Array(services)) = response.data {
            // The services list should contain at least the math service and the registry service itself
            assert!(services.len() >= 2, "Expected at least 2 services, got {}", services.len());
            
            // Verify the math service is in the list
            let has_math_service = services.iter().any(|service| {
                if let ValueType::Map(service_info) = service {
                    if let Some(ValueType::String(path)) = service_info.get("path") {
                        path == "math"
                    } else {
                        false
                    }
                } else {
                    false
                }
            });
            
            assert!(has_math_service, "Math service not found in registry service response");
        } else {
            panic!("Expected array of services in response, got {:?}", response.data);
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that the Registry Service provides detailed service information
///
/// INTENTION: This test validates that:
/// - The Registry Service can return detailed information about a specific service
/// - The response contains proper service state and metadata
#[tokio::test]
async fn test_registry_service_get_service_info() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("node-reg-info".to_string(), "test_network".to_string());
        let mut node = Node::new(config).await.unwrap();
        
        // Create a test service
        let math_service = MathService::new("Math", "math");
        
        // Add the service to the node
        node.add_service(math_service).await.unwrap();
        
        // Start the services to check that we get the correct state
        node.start().await.unwrap();
        
        // Debug print available handlers
        let response = node.request("$registry/services/list".to_string(), ValueType::Null).await.unwrap();
        println!("Available services: {:?}", response);
        
        // Use the request method to query the registry service for the math service
        // Note: We should use the correct parameter path format
        let response = node.request("$registry/services/math".to_string(), ValueType::Null).await.unwrap();
        
        // Verify response is successful
        assert_eq!(response.status, 200, "Registry service request failed: {:?}", response);
        
        // Parse the response to verify it contains correct service information
        if let Some(ValueType::Map(service_info)) = response.data {
            // Verify service path
            if let Some(ValueType::String(path)) = service_info.get("path") {
                assert_eq!(path, "math", "Expected service path 'math', got '{}'", path);
            } else {
                panic!("Service path not found in response");
            }
            
            // Verify service state is Running
            if let Some(ValueType::String(state)) = service_info.get("state") {
                assert_eq!(state, "Running", "Expected service state 'Running', got '{}'", state);
            } else {
                panic!("Service state not found in response");
            }
            
            // Verify service name
            if let Some(ValueType::String(name)) = service_info.get("name") {
                assert_eq!(name, "Math", "Expected service name 'Math', got '{}'", name);
            } else {
                panic!("Service name not found in response");
            }
            
            // Verify actions list is present
            assert!(service_info.contains_key("actions"), "Actions list not found in response");
            
        } else {
            panic!("Expected map of service info in response, got {:?}", response.data);
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that the Registry Service provides just the state of a service
///
/// INTENTION: This test validates that:
/// - The Registry Service can return just the state information of a specific service
/// - The response contains the correct service state
#[tokio::test]
async fn test_registry_service_get_service_state() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a node with a test network ID
        let config = NodeConfig::new("node-reg-state".to_string(), "test_network".to_string());
        let mut node = Node::new(config).await.unwrap();
        
        // Create a test service
        let math_service = MathService::new("Math", "math");
        
        // Add the service to the node
        node.add_service(math_service).await.unwrap();
        
        // Use the request method to query the registry service for the math service state
        let response = node.request("$registry/services/math/state".to_string(), ValueType::Null).await.unwrap();
        
        // Verify response is successful
        assert_eq!(response.status, 200, "Registry service request failed: {:?}", response);
        
        // Parse the response to verify it contains service state
        if let Some(ValueType::Map(state_info)) = response.data {
            // Verify service state is Initialized (since we didn't start it)
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Initialized", "Expected service state 'Initialized', got '{}'", state);
            } else {
                panic!("Service state not found in response");
            }
            
            // Verify that only state information is returned
            assert_eq!(state_info.len(), 1, "Expected only state information, got {:?}", state_info);
        } else {
            panic!("Expected map with state info in response, got {:?}", response.data);
        }
        
        // Start the service
        node.start().await.unwrap();
        
        // Check that state is now Running
        let response = node.request("$registry/services/math/state".to_string(), ValueType::Null).await.unwrap();
        
        if let Some(ValueType::Map(state_info)) = response.data {
            // Verify service state is now Running
            if let Some(ValueType::String(state)) = state_info.get("state") {
                assert_eq!(state, "Running", "Expected service state 'Running', got '{}'", state);
            } else {
                panic!("Service state not found in response");
            }
        } else {
            panic!("Expected map with state info in response, got {:?}", response.data);
        }
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
} 