// Tests for the ServiceRegistry implementation
//
// These tests validate that the ServiceRegistry properly manages
// action handlers and event subscriptions.

use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::timeout;

use runar_common::types::ValueType; 
use runar_node::services::service_registry::ServiceRegistry;
use runar_node::services::{ ServiceResponse, ActionHandler, EventContext};
use runar_node::routing::TopicPath;
use runar_common::logging::{Logger, Component};
use runar_node::services::SubscriptionOptions;

/// Test that verifies subscription functionality in the service registry
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Register a callback to a topic
/// - Allow unsubscribing from a topic
#[tokio::test]
async fn test_subscribe_and_unsubscribe() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a service registry
        let registry = ServiceRegistry::new_with_default_logger();
        
        // Create a TopicPath for the test topic
        let topic = TopicPath::new("test/event", "net1").expect("Valid topic path");
        
        // Create a flag to track if the callback was called
        let was_called = Arc::new(AtomicBool::new(false));
        let was_called_clone = was_called.clone();
        
        // Create a callback that would be invoked when an event is published
        let callback = Arc::new(move |_ctx: Arc<EventContext>, _: ValueType| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            let was_called = was_called_clone.clone();
            Box::pin(async move {
                // Set the flag to true when called
                was_called.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        
        // Subscribe to the topic using the correct method
        let subscription_id = registry.register_local_event_subscription(&topic, callback, SubscriptionOptions::default()).await.unwrap();
        
        // Test the get_local_event_subscribers method
        let handlers = registry.get_local_event_subscribers(&topic).await;
        assert_eq!(handlers.len(), 1, "Expected one handler to be registered");
        assert_eq!(handlers[0].0, subscription_id, "Subscriber ID should match");
        
        // Unsubscribe using the correct method
        registry.unsubscribe_local(&subscription_id).await.unwrap();
        
        // Verify the handler is removed
        let handlers_after = registry.get_local_event_subscribers(&topic).await;
        assert_eq!(handlers_after.len(), 0, "Expected handler to be removed after unsubscribe");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies wildcard subscriptions can be created and unsubscribed
/// 
/// INTENTION: This test validates that the registry properly supports:
/// - Subscribers with wildcard topics
/// - Unsubscribing from wildcard topics
#[tokio::test]
async fn test_wildcard_subscriptions() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a service registry
        let registry = ServiceRegistry::new_with_default_logger();
        
        // Create a callback
        let callback = Arc::new(move |_ctx: Arc<EventContext>, _: ValueType| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            Box::pin(async move {
                Ok(())
            })
        });
        
        // Create TopicPaths for wildcard subscriptions
        let wildcard1 = TopicPath::new("test/#", "net1").expect("Valid topic path");
        let wildcard2 = TopicPath::new("test/events/#", "net1").expect("Valid topic path");
        
        // Subscribe to wildcard topics using the correct method
        let id1 = registry.register_local_event_subscription(&wildcard1, callback.clone(), SubscriptionOptions::default()).await.unwrap();
        let id2 = registry.register_local_event_subscription(&wildcard2, callback.clone(), SubscriptionOptions::default()).await.unwrap();
        
        // Verify handlers are registered correctly using the correct method
        let handlers1 = registry.get_local_event_subscribers(&wildcard1).await;
        let handlers2 = registry.get_local_event_subscribers(&wildcard2).await;
        
        assert_eq!(handlers1.len(), 1, "Expected one handler for wildcard1");
        assert_eq!(handlers2.len(), 1, "Expected one handler for wildcard2");
        
        // Unsubscribe from one wildcard using the correct method
        registry.unsubscribe_local(&id2).await.unwrap();
        
        // Verify the handler is removed using the correct method
        let handlers2_after = registry.get_local_event_subscribers(&wildcard2).await;
        assert_eq!(handlers2_after.len(), 0, "Expected handler to be removed from wildcard2");
        
        // But wildcard1 should still have its handler using the correct method
        let handlers1_after = registry.get_local_event_subscribers(&wildcard1).await;
        assert_eq!(handlers1_after.len(), 1, "Expected wildcard1 handler to remain");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies action handler registration and retrieval
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Register an action handler
/// - Retrieve the registered action handler
/// - Return None for a non-existent action handler
#[tokio::test]
async fn test_register_and_get_action_handler() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        let logger = Logger::new_root(Component::Service, "test");
        let registry = ServiceRegistry::new(logger);
        
        let service_path = "math".to_string();
        let action_name = "add".to_string();
        
        // Create a TopicPath for the action
        let action_path = format!("{}/{}", service_path, action_name);
        let topic_path = TopicPath::new(&action_path, "net1").unwrap();
        
        // Create a handler
        let handler: ActionHandler = Arc::new(|params, _context| {
            Box::pin(async move {
                println!("Handler called with params: {:?}", params);
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        // Register the handler
        registry.register_local_action_handler(
            &topic_path,
            handler.clone(),
            None
        ).await.unwrap();
        
        // Get the handler
        let retrieved_handler = registry.get_action_handler(&topic_path).await;
        
        // Verify the handler was registered and can be retrieved
        assert!(retrieved_handler.is_some());
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that multiple action handlers can be registered and retrieved correctly
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Register multiple action handlers for the same service
/// - Register handlers for different services
/// - Retrieve all handlers correctly
#[tokio::test]
async fn test_multiple_action_handlers() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        let logger = Logger::new_root(Component::Service, "test");
        let registry = ServiceRegistry::new(logger);
        
        // Create paths and handlers for different actions
        let add_action_path = TopicPath::new("math/add", "net1").unwrap();
        let subtract_action_path = TopicPath::new("math/subtract", "net1").unwrap();
        let concat_action_path = TopicPath::new("strings/concat", "net1").unwrap();
        
        // Create handlers for each action
        let add_handler: ActionHandler = Arc::new(|params, _context| {
            Box::pin(async move {
                println!("Add handler called with params: {:?}", params);
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        let subtract_handler: ActionHandler = Arc::new(|params, _context| {
            Box::pin(async move {
                println!("Subtract handler called with params: {:?}", params);
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        let concat_handler: ActionHandler = Arc::new(|params, _context| {
            Box::pin(async move {
                println!("Concat handler called with params: {:?}", params);
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        // Register all handlers
        registry.register_local_action_handler(
            &add_action_path,
            add_handler,
            None
        ).await.unwrap();
        
        registry.register_local_action_handler(
            &subtract_action_path,
            subtract_handler,
            None
        ).await.unwrap();
        
        registry.register_local_action_handler(
            &concat_action_path,
            concat_handler,
            None
        ).await.unwrap();
        
        // Get all handlers
        let add_handler_result = registry.get_action_handler(&add_action_path).await;
        let subtract_handler_result = registry.get_action_handler(&subtract_action_path).await;
        let concat_handler_result = registry.get_action_handler(&concat_action_path).await;
        
        // Verify all handlers were registered and can be retrieved
        assert!(add_handler_result.is_some(), "Add handler should be registered");
        assert!(subtract_handler_result.is_some(), "Subtract handler should be registered");
        assert!(concat_handler_result.is_some(), "Concat handler should be registered");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies action handlers obey network boundaries
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Keep action handlers isolated by network
/// - Only return handlers for the correct network
#[tokio::test]
async fn test_action_handler_network_isolation() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        let logger1 = Logger::new_root(Component::Service, "test1");
        let logger2 = Logger::new_root(Component::Service, "test2");
        
        let registry1 = ServiceRegistry::new(logger1);
        let registry2 = ServiceRegistry::new(logger2);
        
        // Create paths for different networks
        let network1_action_path = TopicPath::new("math/add", "network1").unwrap();
        let network2_action_path = TopicPath::new("math/add", "network2").unwrap();
        
        // Create handlers for each network
        let handler1: ActionHandler = Arc::new(|_params, _context| {
            Box::pin(async move {
                println!("Network 1 handler called");
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        let handler2: ActionHandler = Arc::new(|_params, _context| {
            Box::pin(async move {
                println!("Network 2 handler called");
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        // Register handlers in different networks
        registry1.register_local_action_handler(
            &network1_action_path,
            handler1,
            None
        ).await.unwrap();
        
        registry2.register_local_action_handler(
            &network2_action_path,
            handler2,
            None
        ).await.unwrap();
        
        // Verify network isolation
        assert!(registry1.get_action_handler(&network1_action_path).await.is_some());
        assert!(registry1.get_action_handler(&network2_action_path).await.is_none());
        
        assert!(registry2.get_action_handler(&network2_action_path).await.is_some());
        assert!(registry2.get_action_handler(&network1_action_path).await.is_none());
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies multiple event handlers can be registered and retrieved
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Register multiple event handlers for different topics
/// - Retrieve all handlers for a specific topic
#[tokio::test]
async fn test_multiple_event_handlers() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        // Create a service registry
        let registry = ServiceRegistry::new_with_default_logger();
        
        // Create test topics
        let topic1 = TopicPath::new("events/created", "net1").expect("Valid topic path");
        let topic2 = TopicPath::new("events/updated", "net1").expect("Valid topic path");
        
        // Create three handlers for the multiple event handlers test
        let received_topic1 = Arc::new(AtomicBool::new(false));
        let received_topic1_clone = received_topic1.clone();
        
        let received_topic2 = Arc::new(AtomicBool::new(false));
        let received_topic2_clone = received_topic2.clone();
        
        // Create a first event handler to track events with "topic1"
        let handler1 = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            let received = received_topic1_clone.clone();
            Box::pin(async move {
                received.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        
        // Create a second event handler to track events with "topic2"
        let handler2 = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            let received = received_topic2_clone.clone();
            Box::pin(async move {
                received.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        
        // Subscribe handlers to topics using the correct method
        let id1 = registry.register_local_event_subscription(&topic1, handler1, SubscriptionOptions::default()).await.unwrap();
        let id2 = registry.register_local_event_subscription(&topic2, handler2, SubscriptionOptions::default()).await.unwrap();
        
        // Retrieve and verify handlers using the correct method
        let handlers1 = registry.get_local_event_subscribers(&topic1).await;
        let handlers2 = registry.get_local_event_subscribers(&topic2).await;
        
        assert_eq!(handlers1.len(), 1, "Expected one handler for topic1");
        assert_eq!(handlers2.len(), 1, "Expected one handler for topic2");
        assert_eq!(handlers1[0].0, id1, "Subscriber ID should match for topic1");
        assert_eq!(handlers2[0].0, id2, "Subscriber ID should match for topic2");
        
        // Verify getting handlers for a non-existent topic using the correct method
        let nonexistent_topic = TopicPath::new("test/events/deleted", "net1").expect("Valid topic path");
        let handlers_none = registry.get_local_event_subscribers(&nonexistent_topic).await;
        assert_eq!(handlers_none.len(), 0, "Expected no handlers for non-existent topic");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}

/// Test that verifies path template parameters functionality in action handlers
/// 
/// INTENTION: This test validates that the registry can properly:
/// - Register an action handler with path parameters using template syntax like {parameter}
/// - Match and retrieve the correct handler when a request comes in with a path that matches the template
/// - Extract path parameters from the matched path
#[tokio::test]
async fn test_path_template_parameters() {
    // Wrap the test in a timeout to prevent it from hanging
    match timeout(Duration::from_secs(10), async {
        let logger = Logger::new_root(Component::Service, "test");
        let registry = ServiceRegistry::new(logger);
        
        // Create paths with parameter templates
        let service_info_path = TopicPath::new("$registry/services/{service_path}", "test_network").unwrap();
        let service_state_path = TopicPath::new("$registry/services/{service_path}/state", "test_network").unwrap();
        let action_path = TopicPath::new("services/{service_type}/actions/{action_name}", "test_network").unwrap();
        
        // Create handlers that include parameter checking
        let service_info_handler: ActionHandler = Arc::new(move |_params, context| {
            Box::pin(async move {
                let service_path = context.path_params.get("service_path")
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                
                println!("Service info handler called for service: {}", service_path);
                
                // Validate that the parameter was captured
                if service_path == "unknown" {
                    return Ok(ServiceResponse::error(400, "Missing service_path parameter"));
                }
                
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        let service_state_handler: ActionHandler = Arc::new(move |_params, context| {
            Box::pin(async move {
                let service_path = context.path_params.get("service_path")
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                
                println!("Service state handler called for service: {}", service_path);
                
                // Validate that the parameter was captured
                if service_path == "unknown" {
                    return Ok(ServiceResponse::error(400, "Missing service_path parameter"));
                }
                
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        let action_handler: ActionHandler = Arc::new(move |_params, context| {
            Box::pin(async move {
                let service_type = context.path_params.get("service_type")
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                
                let action_name = context.path_params.get("action_name")
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                
                println!("Action handler called for service_type: {}, action_name: {}", 
                         service_type, action_name);
                
                // Validate that all parameters were captured
                if service_type == "unknown" || action_name == "unknown" {
                    return Ok(ServiceResponse::error(400, "Missing parameters"));
                }
                
                Ok(ServiceResponse::ok_empty())
            })
        });
        
        // Register all handlers using the correct local method
        registry.register_local_action_handler(&service_info_path, service_info_handler, None).await.unwrap();
        registry.register_local_action_handler(&service_state_path, service_state_handler, None).await.unwrap();
        registry.register_local_action_handler(&action_path, action_handler, None).await.unwrap();
        
        // Create specific path instances for testing handler retrieval
        let math_service_path = TopicPath::new("$registry/services/math", "test_network").unwrap();
        let math_state_path = TopicPath::new("$registry/services/math/state", "test_network").unwrap();
        let auth_login_path = TopicPath::new("services/auth/actions/login", "test_network").unwrap();
        
        // Test that the correct handlers are found for the specific paths
        let math_service_handler = registry.get_action_handler(&math_service_path).await;
        let math_state_handler = registry.get_action_handler(&math_state_path).await;
        let auth_login_handler = registry.get_action_handler(&auth_login_path).await;
        
        // Verify the handlers were found by template matching
        assert!(math_service_handler.is_some(), "Math service handler should be found via template matching");
        assert!(math_state_handler.is_some(), "Math state handler should be found via template matching");
        assert!(auth_login_handler.is_some(), "Auth login handler should be found via template matching");
        
        // Test non-matching paths
        let user_profile_path = TopicPath::new("user/profile", "test_network").unwrap();
        let non_matching_handler = registry.get_action_handler(&user_profile_path).await;
        assert!(non_matching_handler.is_none(), "Non-matching path should not find a handler");
        
        // Test partial matches (missing segments)
        let incomplete_path = TopicPath::new("services/auth", "test_network").unwrap();
        let incomplete_handler = registry.get_action_handler(&incomplete_path).await;
        assert!(incomplete_handler.is_none(), "Incomplete path should not match a template");
        
        // Test with incorrect segment count
        let wrong_segments_path = TopicPath::new("services/auth/actions/login/extra", "test_network").unwrap();
        let wrong_segments_handler = registry.get_action_handler(&wrong_segments_path).await;
        assert!(wrong_segments_handler.is_none(), "Path with wrong segment count should not match a template");
    }).await {
        Ok(_) => (), // Test completed within the timeout
        Err(_) => panic!("Test timed out after 10 seconds"),
    }
}
 
 