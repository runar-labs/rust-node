use runar_common::types::ValueType;
use runar_node::routing::TopicPath;
use runar_node::network::transport::{PeerId, NetworkMessage};
use runar_node::services::{ActionHandler, ServiceResponse};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::RwLock;
use anyhow::{Result, anyhow};

/// Tests for remote action handler topic path behavior
#[cfg(test)]
mod remote_action_tests {
    use super::*;
    
    /// Simple mock service registry for testing
    struct MockServiceRegistry {
        action_handlers: HashMap<String, ActionHandler>,
    }
    
    impl MockServiceRegistry {
        fn new() -> Self {
            Self {
                action_handlers: HashMap::new(),
            }
        }
        
        /// Register an action handler using topic path
        fn register_handler(&mut self, topic_path: &TopicPath, handler: ActionHandler) {
            // Key issue: what string to use as the key?
            let key = topic_path.as_str().to_string(); // Using as_str() includes the network ID
            self.action_handlers.insert(key, handler);
        }
        
        /// Get a handler by topic path
        fn get_handler(&self, topic_path: &TopicPath) -> Option<&ActionHandler> {
            // Key issue: what string to use for lookup?
            let key = topic_path.as_str().to_string(); // Using as_str() includes the network ID
            self.action_handlers.get(&key)
        }
    }
    
    /// Test to verify that remote action paths are correctly formatted
    #[test]
    fn test_remote_action_topic_path_formatting() {
        // Create a topic path for a remote action
        let topic_path = TopicPath::new("test-network:MathB/add", "default").unwrap();
        
        // Verify the path components
        assert_eq!(topic_path.network_id(), "test-network");
        assert_eq!(topic_path.service_path(), "MathB");
        assert_eq!(topic_path.action_path(), "MathB/add");
        
        // The key issue is whether we use .action_path() or .as_str() for the message topic
        // Using action_path() would give "MathB/add" (without network ID)
        // Using as_str() would give "test-network:MathB/add" (with network ID)
        assert_eq!(topic_path.as_str(), "test-network:MathB/add");
        
        // For remote actions, we need the full path including the network ID
        // This simulates what happens in create_remote_action_handler
        let message = NetworkMessage {
            source: PeerId::new("test-network".to_string(), "node1".to_string()),
            destination: Some(PeerId::new("test-network".to_string(), "node2".to_string())),
            message_type: "Request".to_string(),
            correlation_id: Some("test-123".to_string()),
            topic: topic_path.as_str().to_string(), // Correct way: use as_str()
            params: ValueType::Null,
            payload: ValueType::Null,
        };
        
        // The message topic should include the network ID for proper routing
        assert_eq!(message.topic, "test-network:MathB/add");
        
        // In the server side, when receiving the message, we need to ensure it can find the handler
        // based on the topic string (with network ID)
        let received_topic_path = TopicPath::new(&message.topic, "default").unwrap();
        assert_eq!(received_topic_path.network_id(), "test-network");
        assert_eq!(received_topic_path.service_path(), "MathB");
        
        // The action_path() method doesn't include network ID, which might cause mismatches
        assert_eq!(received_topic_path.action_path(), "MathB/add");
        
        // But the as_str() includes the network ID, which should be used for handler lookup
        assert_eq!(received_topic_path.as_str(), "test-network:MathB/add");
    }
    
    /// Test simulating registration and lookup of remote action handlers
    #[test]
    fn test_remote_action_registration_and_lookup() {
        // Mock service registry
        let mut registry = MockServiceRegistry::new();
        
        // 1. The server registers a local action handler
        let server_topic = TopicPath::new("test-network:MathB/add", "default").unwrap();
        
        // Create a handler that returns a fixed response
        let handler: ActionHandler = Arc::new(|params: Option<ValueType>, _ctx| {
            Box::pin(async move {
                let a = match &params {
                    Some(ValueType::Map(map)) => {
                        if let Some(ValueType::Number(a)) = map.get("a") {
                            *a
                        } else {
                            return Err(anyhow!("Missing parameter 'a'"));
                        }
                    },
                    _ => return Err(anyhow!("Invalid parameters")),
                };
                
                let b = match &params {
                    Some(ValueType::Map(map)) => {
                        if let Some(ValueType::Number(b)) = map.get("b") {
                            *b
                        } else {
                            return Err(anyhow!("Missing parameter 'b'"));
                        }
                    },
                    _ => return Err(anyhow!("Invalid parameters")),
                };
                
                Ok(ServiceResponse {
                    status: 200,
                    data: Some(ValueType::Number(a + b)),
                    error: None,
                })
            })
        });
        
        // Register the handler with the server_topic
        registry.register_handler(&server_topic, handler);
        
        // 2. Client creates a message to send to the server
        let client_topic = TopicPath::new("test-network:MathB/add", "default").unwrap();
        
        // Create the message
        let message = NetworkMessage {
            source: PeerId::new("test-network".to_string(), "node1".to_string()),
            destination: Some(PeerId::new("test-network".to_string(), "node2".to_string())),
            message_type: "Request".to_string(),
            correlation_id: Some("test-123".to_string()),
            topic: client_topic.as_str().to_string(), // This must match what the server registered
            params: ValueType::Map([
                ("a".to_string(), ValueType::Number(5.0)),
                ("b".to_string(), ValueType::Number(3.0)),
            ].into_iter().collect()),
            payload: ValueType::Null,
        };
        
        // 3. Server receives the message and looks up the handler
        let receive_topic = TopicPath::new(&message.topic, "default").unwrap();
        let handler = registry.get_handler(&receive_topic).expect("Handler should be found");
        
        // 4. Execute the handler with the message parameters
        let result = tokio_test::block_on(handler(Some(message.params.clone()), Default::default()));
        
        // 5. Verify the result
        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.data, Some(ValueType::Number(8.0)));
    }
    
    /// Test that demonstrates the issue when using action_path() instead of as_str()
    #[test]
    fn test_action_path_vs_as_str_mismatch() {
        // Mock service registry
        let mut registry = MockServiceRegistry::new();
        
        // 1. The server registers a local action handler
        let server_topic = TopicPath::new("test-network:MathB/add", "default").unwrap();
        
        // Create a simple handler
        let handler: ActionHandler = Arc::new(|_params, _ctx| {
            Box::pin(async move {
                Ok(ServiceResponse {
                    status: 200, 
                    data: Some(ValueType::String("Success".to_string())),
                    error: None,
                })
            })
        });
        
        // THIS SIMULATES THE BUG: Register with a key that doesn't include the network ID
        registry.action_handlers.insert(server_topic.action_path(), handler);
        
        // 2. Client creates a message to send to the server
        let client_topic = TopicPath::new("test-network:MathB/add", "default").unwrap();
        
        // Create the message with the full qualified path (includes network ID)
        let message = NetworkMessage {
            source: PeerId::new("test-network".to_string(), "node1".to_string()),
            destination: Some(PeerId::new("test-network".to_string(), "node2".to_string())),
            message_type: "Request".to_string(),
            correlation_id: Some("test-123".to_string()),
            topic: client_topic.as_str().to_string(), // Using the full path with network ID
            params: ValueType::Null,
            payload: ValueType::Null,
        };
        
        // 3. Server receives the message and tries to look up the handler
        let receive_topic = TopicPath::new(&message.topic, "default").unwrap();
        
        // Attempt lookup by full path (includes network ID)
        let handler_by_as_str = registry.get_handler(&receive_topic);
        
        // Attempt lookup by action path only (no network ID)
        let handler_by_action_path = registry.action_handlers.get(&receive_topic.action_path());
        
        // Using as_str() for both registration and lookup will fail,
        // because we registered with action_path()
        assert!(handler_by_as_str.is_none());
        
        // Using action_path() for both registration and lookup would succeed
        assert!(handler_by_action_path.is_some());
        
        // THIS DEMONSTRATES THE ISSUE:
        // If we register with action_path() but look up with as_str(),
        // or vice versa, we'll get a mismatch.
    }
} 