use anyhow::Result;
use tokio::sync::Mutex;
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;

use runar_node::routing::TopicPath;
use runar_node::services::EventContext;
use runar_node::ServiceRegistry;
use runar_common::types::ValueType;
use runar_common::logging::{Component, Logger};
use runar_node::services::SubscriptionOptions;

/// INTENTION: Test comprehensive scenarios for wildcard pattern matching in TopicPath
#[cfg(test)]
mod topic_path_wildcard_tests {
    use super::*;
    
    #[test]
    fn test_is_pattern() {
        // Test without wildcards
        let path1 = TopicPath::new("main:services/auth/login", "default").expect("Valid path");
        assert!(!path1.is_pattern());
        
        // Test with single-segment wildcard
        let pattern1 = TopicPath::new("main:services/*/login", "default").expect("Valid pattern");
        assert!(pattern1.is_pattern());
        
        // Test with multi-segment wildcard
        let pattern2 = TopicPath::new("main:services/>", "default").expect("Valid pattern");
        assert!(pattern2.is_pattern());
        assert!(pattern2.has_multi_wildcard());
    }
    
    #[test]
    fn test_single_wildcard_matching() {
        // Create pattern with single-segment wildcard
        let pattern = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
        
        // Test successful matches
        let path1 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
        let path2 = TopicPath::new("main:services/math/state", "default").expect("Valid path");
        
        assert!(pattern.matches(&path1));
        assert!(pattern.matches(&path2));
        
        // Test non-matches
        let non_match1 = TopicPath::new("main:services/auth/login", "default").expect("Valid path");
        let non_match2 = TopicPath::new("main:services/auth/state/active", "default").expect("Valid path");
        let non_match3 = TopicPath::new("main:events/user/created", "default").expect("Valid path");
        
        assert!(!pattern.matches(&non_match1)); // Different last segment
        assert!(!pattern.matches(&non_match2)); // Too many segments
        assert!(!pattern.matches(&non_match3)); // Different service path
    }
    
    #[test]
    fn test_multi_wildcard_matching() {
        // Create pattern with multi-segment wildcard
        let pattern = TopicPath::new("main:services/>", "default").expect("Valid pattern");
        
        // Test successful matches (should match any path that starts with "services")
        let path1 = TopicPath::new("main:services/auth", "default").expect("Valid path");
        let path2 = TopicPath::new("main:services/auth/login", "default").expect("Valid path");
        let path3 = TopicPath::new("main:services/math/add/numbers", "default").expect("Valid path");
        
        assert!(pattern.matches(&path1));
        assert!(pattern.matches(&path2));
        assert!(pattern.matches(&path3));
        
        // Test non-matches
        let non_match1 = TopicPath::new("main:events/user/created", "default").expect("Valid path");
        
        assert!(!pattern.matches(&non_match1)); // Different service path
    }
    
    #[test]
    fn test_multi_wildcard_position() {
        // Multi-wildcard must be the last segment
        let invalid_pattern = TopicPath::new("main:services/>/state", "default");
        assert!(invalid_pattern.is_err());
        
        // But can be in the middle of a pattern as long as it's the last segment
        let valid_pattern = TopicPath::new("main:services/>", "default").expect("Valid pattern");
        assert!(valid_pattern.is_pattern());
        assert!(valid_pattern.has_multi_wildcard());
    }
    
    #[test]
    fn test_complex_patterns() {
        // Pattern with both types of wildcards
        let pattern = TopicPath::new("main:services/*/events/>", "default").expect("Valid pattern");
        
        // Test successful matches
        let path1 = TopicPath::new("main:services/auth/events/user/login", "default").expect("Valid path");
        let path2 = TopicPath::new("main:services/math/events/calculation/completed", "default").expect("Valid path");
        
        assert!(pattern.matches(&path1));
        assert!(pattern.matches(&path2));
        
        // Test non-matches
        let non_match1 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
        let non_match2 = TopicPath::new("main:services/auth/logs/error", "default").expect("Valid path");
        
        assert!(!pattern.matches(&non_match1)); // Different segment after service
        assert!(!pattern.matches(&non_match2)); // "logs" instead of "events"
    }
    
    #[test]
    fn test_wildcard_at_beginning() {
        // Pattern with wildcard at beginning
        let pattern = TopicPath::new("main:*/state", "default").expect("Valid pattern");
        
        // Test successful matches (should match any service with "state" action)
        let path1 = TopicPath::new("main:auth/state", "default").expect("Valid path");
        let path2 = TopicPath::new("main:math/state", "default").expect("Valid path");
        
        assert!(pattern.matches(&path1));
        assert!(pattern.matches(&path2));
        
        // Test non-matches
        let non_match1 = TopicPath::new("main:auth/login", "default").expect("Valid path");
        
        assert!(!pattern.matches(&non_match1)); // Different action
    }
    
    #[test]
    fn test_network_isolation() {
        // Patterns should only match within the same network
        let pattern = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
        let path1 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
        let path2 = TopicPath::new("other:services/auth/state", "default").expect("Valid path");
        
        assert!(pattern.matches(&path1));       // Same network
        assert!(!pattern.matches(&path2));      // Different network
    }
}

/// INTENTION: Test wildcard pattern matching with the service registry for event subscriptions
#[cfg(test)]
mod service_registry_wildcard_tests {
    use super::*;
    
    /// Test event handler for wildcard subscriptions
    #[tokio::test]
    async fn test_wildcard_event_subscriptions() -> Result<()> {
        // Create service registry
        let registry = ServiceRegistry::new_with_default_logger();
        
        // Create a counter to track event deliveries
        let counter = Arc::new(Mutex::new(0));
        
        // Create a callback that increments the counter
        let counter_clone = counter.clone();
        let callback = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| {
            let counter = counter_clone.clone();
            Box::pin(async move {
                let mut lock = counter.lock().await;
                *lock += 1;
                Ok(())
            }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
        });
        
        // Subscribe to a pattern with a single-level wildcard
        let pattern1 = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
        let _sub_id1 = registry.register_local_event_subscription(&pattern1, callback.clone(), SubscriptionOptions::default()).await?;
        
        // Subscribe to a pattern with a multi-level wildcard
        let pattern2 = TopicPath::new("main:events/>", "default").expect("Valid pattern");
        let _sub_id2 = registry.register_local_event_subscription(&pattern2, callback.clone(), SubscriptionOptions::default()).await?;
        
        // Subscribe to a specific path to compare
        let specific_path = TopicPath::new("main:services/math/add", "default").expect("Valid path");
        let _sub_id3 = registry.register_local_event_subscription(&specific_path, callback.clone(), SubscriptionOptions::default()).await?;
        
        // Publish to various topics and check if they match
        
        // Should match pattern1
        let topic1 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
        let topic2 = TopicPath::new("main:services/math/state", "default").expect("Valid path");
        
        // Should match pattern2
        let topic3 = TopicPath::new("main:events/user/created", "default").expect("Valid path");
        let topic4 = TopicPath::new("main:events/system/started", "default").expect("Valid path");
        
        // Should match specific_path
        let topic5 = TopicPath::new("main:services/math/add", "default").expect("Valid path");
        
        // Should not match any subscriptions
        let topic6 = TopicPath::new("main:services/auth/login", "default").expect("Valid path");
        
        // Get handlers for each topic and call them
        let data = ValueType::Null;
        
        // Should match pattern1 (services/*/state)
        let handlers1 = registry.get_local_event_subscribers(&topic1).await;
        assert_eq!(handlers1.len(), 1);
        for (_, handler) in handlers1 {
            let context = Arc::new(EventContext::new(&topic1, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Should match pattern1 (services/*/state)
        let handlers2 = registry.get_local_event_subscribers(&topic2).await;
        assert_eq!(handlers2.len(), 1);
        for (_, handler) in handlers2 {
            let context = Arc::new(EventContext::new(&topic2, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Should match pattern2 (events/>)
        let handlers3 = registry.get_local_event_subscribers(&topic3).await;
        assert_eq!(handlers3.len(), 1);
        for (_, handler) in handlers3 {
            let context = Arc::new(EventContext::new(&topic3, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Should match pattern2 (events/>)
        let handlers4 = registry.get_local_event_subscribers(&topic4).await;
        assert_eq!(handlers4.len(), 1);
        for (_, handler) in handlers4 {
            let context = Arc::new(EventContext::new(&topic4, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Should match specific_path (services/math/add)
        let handlers5 = registry.get_local_event_subscribers(&topic5).await;
        assert_eq!(handlers5.len(), 1);
        for (_, handler) in handlers5 {
            let context = Arc::new(EventContext::new(&topic5, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Should not match any patterns
        let handlers6 = registry.get_local_event_subscribers(&topic6).await;
        assert_eq!(handlers6.len(), 0);
        
        // Check that the counter was incremented the correct number of times
        let final_count = *counter.lock().await;
        assert_eq!(final_count, 5); // 5 matching topics
        
        Ok(())
    }
    
    /// Test that wildcards can be unsubscribed properly
    #[tokio::test]
    async fn test_wildcard_unsubscription() -> Result<()> {
        // Create service registry
        let registry = ServiceRegistry::new_with_default_logger();
        
        // Create a callback
        let callback = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| {
            Box::pin(async move { Ok(()) }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
        });
        
        // Subscribe to a pattern with a wildcard
        let pattern = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
        let sub_id = registry.register_local_event_subscription(&pattern, callback.clone(), SubscriptionOptions::default()).await?;
        
        // Publish to a matching topic
        let topic = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
        let handlers_before = registry.get_local_event_subscribers(&topic).await;
        assert_eq!(handlers_before.len(), 1);
        
        // Unsubscribe using the subscription ID
        registry.unsubscribe_local(&sub_id).await?;
        
        // Publish again, should not receive the event
        let handlers_after = registry.get_local_event_subscribers(&topic).await;
        assert_eq!(handlers_after.len(), 0);
        
        // Ensure the counter was incremented only once
        // let final_count = *counter.lock().await; // Remove this line
        // assert_eq!(final_count, 0); // Remove this line // Handler should not have been called after unsubscribe

        Ok(())
    }
    
    /// Test that multiple wildcard handlers can be registered and receive events
    #[tokio::test]
    async fn test_multiple_wildcard_handlers() -> Result<()> {
        let registry = ServiceRegistry::new_with_default_logger();
        let counter1 = Arc::new(Mutex::new(0));
        let counter2 = Arc::new(Mutex::new(0));
        
        // Callback 1
        let counter1_clone = counter1.clone();
        let callback1 = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| {
            let counter = counter1_clone.clone();
            Box::pin(async move {
                let mut lock = counter.lock().await;
                *lock += 1;
                Ok(())
            }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
        });
        
        // Callback 2
        let counter2_clone = counter2.clone();
        let callback2 = Arc::new(move |_ctx: Arc<EventContext>, _data: ValueType| {
            let counter = counter2_clone.clone();
            Box::pin(async move {
                let mut lock = counter.lock().await;
                *lock += 1;
                Ok(())
            }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
        });
        
        // Subscribe both callbacks to the same wildcard pattern
        let pattern = TopicPath::new("main:events/>", "default").expect("Valid pattern");
        let _sub_id1 = registry.register_local_event_subscription(&pattern, callback1, SubscriptionOptions::default()).await?;
        let _sub_id2 = registry.register_local_event_subscription(&pattern, callback2, SubscriptionOptions::default()).await?;
        
        // Publish to a matching topic
        let topic = TopicPath::new("main:events/user/updated", "default").expect("Valid path");
        let data = ValueType::Null;
        
        // Get handlers and call them
        let handlers = registry.get_local_event_subscribers(&topic).await;
        assert_eq!(handlers.len(), 2); // Both handlers should match
        
        for (_, handler) in handlers {
            let context = Arc::new(EventContext::new(&topic, Logger::new_root(Component::Service, "test")));
            handler(context, data.clone()).await?;
        }
        
        // Check counters
        let count1 = *counter1.lock().await;
        let count2 = *counter2.lock().await;
        assert_eq!(count1, 1);
        assert_eq!(count2, 1);

        Ok(())
    }
} 