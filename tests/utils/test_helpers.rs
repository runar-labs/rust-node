// Helper functions for test operations
use anyhow::Result;
use runar_node::{ServiceRequest, ServiceResponse, ValueType, RequestContext};
use serde_json::json;
use uuid::Uuid;
use std::sync::Arc;
use runar_node::services::{RequestContext, NodeRequestHandler, ServiceRequest, ServiceResponse};
use async_trait::async_trait;

/// Create a service request with the given parameters
pub fn create_test_request(
    path: &str,
    action: &str,
    data: Option<ValueType>,
) -> ServiceRequest {
    ServiceRequest {
        path: path.to_string(),
        action: action.to_string(),
        data,
        request_id: Some(Uuid::new_v4().to_string()),
        metadata: None,
        topic_path: None,
        context: Arc::new(RequestContext::default()),
    }
}

/// Create a service request with JSON data
pub fn create_json_request(
    path: &str,
    action: &str,
    data: serde_json::Value,
) -> ServiceRequest {
    create_test_request(path, action, Some(ValueType::Json(data)))
}

/// Create a service request with string data
pub fn create_string_request(
    path: &str,
    action: &str,
    data: &str,
) -> ServiceRequest {
    create_test_request(path, action, Some(ValueType::String(data.to_string())))
}

/// Extract a field from a JSON response as a string
pub fn extract_json_string(response: &ServiceResponse, field: &str) -> Option<String> {
    if let Some(ValueType::Json(json_data)) = &response.data {
        json_data.get(field).and_then(|v| v.as_str().map(|s| s.to_string()))
    } else {
        None
    }
}

/// Extract a field from a JSON response as a boolean
pub fn extract_json_bool(response: &ServiceResponse, field: &str) -> Option<bool> {
    if let Some(ValueType::Json(json_data)) = &response.data {
        json_data.get(field).and_then(|v| v.as_bool())
    } else {
        None
    }
}

/// Creates a minimal RequestContext for testing purposes
/// This replaces the RequestContext::for_tests() method in the core code
pub fn create_minimal_request_context() -> RequestContext {
    // Implement a minimal NodeRequestHandler for testing
    struct MinimalTestHandler;
    
    #[async_trait]
    impl NodeRequestHandler for MinimalTestHandler {
        async fn request(&self, _path: &str, _data: Option<runar_node::services::types::ValueType>) -> Result<ServiceResponse> {
            Ok(ServiceResponse::success("Test handler response", None))
        }
    }
    
    RequestContext::new(Arc::new(MinimalTestHandler))
} 