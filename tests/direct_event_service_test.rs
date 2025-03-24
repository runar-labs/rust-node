use anyhow::Result;
use runar_node::services::abstract_service::AbstractService;
use runar_node::services::ResponseStatus;
use runar_node::{ServiceRequest, ValueType, RequestContext};
use runar_node::routing::TopicPath;
use serde_json::json;
use std::sync::Arc;
use std::collections::HashMap;

// Local fixture imports
mod fixtures {
    pub mod direct_api {
        pub mod event_publisher {
            include!("fixtures/direct_api/event_publisher.rs");
        }
        pub mod event_subscriber {
            include!("fixtures/direct_api/event_subscriber.rs");
        }
    }
}

use fixtures::direct_api::event_publisher::EventPublisherService;
use fixtures::direct_api::event_subscriber::EventSubscriberService;

/// Test setup for direct API event services
fn setup_direct_api_services() -> (EventPublisherService, EventSubscriberService) {
    let publisher = EventPublisherService::new("publisher");
    let subscriber = EventSubscriberService::new("subscriber");
    
    (publisher, subscriber)
}

#[cfg(test)]
mod tests {
    use runar_node::services::{ServiceRequest, ServiceResponse};
    use runar_node::services::types::ValueType;
    use runar_node::services::ResponseStatus;
    use runar_node::services::abstract_service::AbstractService;
    use runar_node::routing::TopicPath;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    
    // Local fixtures
    use crate::fixtures::direct_api::event_publisher::EventPublisherService;
    use crate::fixtures::direct_api::event_subscriber::EventSubscriberService;
    
    #[tokio::test]
    async fn test_simple_publisher_handle_request() {
        // Create a publisher service
        let publisher = EventPublisherService::new("test_publisher");
        println!("Created publisher service");
        
        // Create a simple request
        let simple_request = ServiceRequest {
            path: "publish/full_path".to_string(),
            action: "execute".to_string(),
            data: Some(runar_node::ValueType::String("Hello, test event".to_string())),
            request_id: Some("test-req-1".to_string()),
            context: Default::default(),
            metadata: Default::default(),
            topic_path: Some(TopicPath::parse("publish/full_path", "local").unwrap())
        };
        
        // Test the handle_request method
        match publisher.handle_request(simple_request).await {
            Ok(response) => {
                println!("Publisher response: {:?}", response);
                // Just check it returned something
                assert!(true);
            },
            Err(e) => {
                println!("Error: {:?}", e);
                assert!(false, "Handle request failed: {}", e);
            }
        }
    }
    
    #[tokio::test]
    async fn test_event_publisher_service() {
        // Create a publisher service
        let publisher = EventPublisherService::new("test_publisher");
        
        println!("Created publisher service");
        
        // Create request for publish_with_full_path
        let validate_request = ServiceRequest {
            path: "publish/full_path".to_string(),
            action: "execute".to_string(),
            data: Some(ValueType::Json(json!({
                "payload": "test_event_data_with_full_path"
            }))),
            request_id: Some("test-req-1".to_string()),
            context: Default::default(),
            metadata: Default::default(),
            topic_path: Some(TopicPath::parse("publish/full_path", "local").unwrap())
        };
        
        // Test the handle_request method for full path publish
        let validate_result = publisher.handle_request(validate_request).await.unwrap();
        println!("Full path publish result: {:?}", validate_result);
        assert_eq!(validate_result.status, ResponseStatus::Success);
        
        // Create request for publish with topic only
        let publish_request = ServiceRequest {
            path: "publish/topic_only".to_string(),
            action: "execute".to_string(),
            data: Some(ValueType::Json(json!({
                "payload": "test_event_data_topic_only"
            }))),
            request_id: Some("test-req-2".to_string()),
            context: Default::default(),
            metadata: Default::default(),
            topic_path: Some(TopicPath::parse("publish/topic_only", "local").unwrap())
        };
        
        // Test the handle_request method for topic only publish
        let publish_result = publisher.handle_request(publish_request).await.unwrap();
        println!("Topic only publish result: {:?}", publish_result);
        assert_eq!(publish_result.status, ResponseStatus::Success);
    }
    
    #[tokio::test]
    async fn test_event_subscriber_service() {
        // Create a subscriber service
        let subscriber = EventSubscriberService::new("test_subscriber");
        
        println!("Created subscriber service");
        
        // Create request for get_events
        let get_events_request = ServiceRequest {
            path: "get_events".to_string(),
            action: "query".to_string(),
            data: None,
            request_id: Some("test-req-1".to_string()),
            context: Default::default(),
            metadata: Default::default(),
            topic_path: Some(TopicPath::parse("get_events", "local").unwrap())
        };
        
        // Test the handle_request method for get_events
        let get_events_result = subscriber.handle_request(get_events_request).await.unwrap();
        println!("Get events result: {:?}", get_events_result);
        assert_eq!(get_events_result.status, ResponseStatus::Success);
        
        // Create request for clear_events
        let clear_events_request = ServiceRequest {
            path: "clear_events".to_string(),
            action: "execute".to_string(),
            data: None,
            request_id: Some("test-req-2".to_string()),
            context: Default::default(),
            metadata: Default::default(),
            topic_path: Some(TopicPath::parse("clear_events", "local").unwrap())
        };
        
        // Test the handle_request method for clear_events
        let clear_events_result = subscriber.handle_request(clear_events_request).await.unwrap();
        println!("Clear events result: {:?}", clear_events_result);
        assert_eq!(clear_events_result.status, ResponseStatus::Success);
    }
} 