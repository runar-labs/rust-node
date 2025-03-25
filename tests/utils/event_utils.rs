// Event testing utilities
use anyhow::Result;
use runar_node::{Node, NodeConfig, RequestContext, ServiceRequest, ServiceResponse, ValueType};
use std::sync::Arc;
use std::collections::HashMap;
use serde_json::json;

/// Create a request to publish an event
pub fn create_publish_request(
    service_name: &str,
    event_name: &str,
    data: ValueType,
) -> ServiceRequest {
    let topic = format!("{}/{}", service_name, event_name);
    let context = Arc::new(RequestContext::default());
    
    ServiceRequest {
        path: topic.clone(),
        action: "publish".to_string(),
        data: Some(data),
        request_id: Some(uuid::Uuid::new_v4().to_string()),
        metadata: None,
        topic_path: None,
        context,
    }
}

/// Helper to create an event subscription request
pub fn create_subscribe_request(
    service_name: &str,
    event_name: &str,
) -> ServiceRequest {
    let topic = format!("{}/{}", service_name, event_name);
    let context = Arc::new(RequestContext::default());
    
    ServiceRequest {
        path: topic.clone(),
        action: "subscribe".to_string(),
        data: None,
        request_id: Some(uuid::Uuid::new_v4().to_string()),
        metadata: None,
        topic_path: None,
        context,
    }
}

/// Set up a test node with standard test services for event testing
pub async fn setup_event_test_node(network_id: &str) -> Result<Node> {
    // Create a temporary directory for the node
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().to_str().unwrap();
    
    // Create node config
    let config = NodeConfig::new(network_id, temp_path, temp_path);
    
    // Create and initialize the node
    let mut node = Node::new(temp_path, network_id).await?;
    node.init().await?;
    
    Ok(node)
}

/// Helper to create standard event data
pub fn create_event_data(
    event_type: &str,
    source: &str,
    additional_fields: Option<HashMap<String, ValueType>>,
) -> ValueType {
    let mut data = HashMap::new();
    
    // Add standard fields
    data.insert("type".to_string(), ValueType::String(event_type.to_string()));
    data.insert("source".to_string(), ValueType::String(source.to_string()));
    data.insert("timestamp".to_string(), ValueType::Number(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    ));
    
    // Add any additional fields
    if let Some(additional) = additional_fields {
        for (key, value) in additional {
            data.insert(key, value);
        }
    }
    
    ValueType::Map(data)
}

/// Convert a HashMap of ValueType to JSON
pub fn value_map_to_json(map: &HashMap<String, ValueType>) -> serde_json::Value {
    let mut json_map = serde_json::Map::new();
    
    for (key, value) in map {
        json_map.insert(key.clone(), value_to_json(value));
    }
    
    serde_json::Value::Object(json_map)
}

/// Convert a ValueType to JSON
pub fn value_to_json(value: &ValueType) -> serde_json::Value {
    match value {
        ValueType::String(s) => serde_json::Value::String(s.clone()),
        ValueType::Number(n) => serde_json::Value::Number(serde_json::Number::from_f64(*n).unwrap_or_default()),
        ValueType::Bool(b) => serde_json::Value::Bool(*b),
        ValueType::Map(map) => value_map_to_json(map),
        ValueType::Json(j) => j.clone(),
        ValueType::Null => serde_json::Value::Null,
        ValueType::Binary(b) => {
            // Convert binary to base64 string
            let base64 = base64::encode(b);
            serde_json::Value::String(base64)
        }
    }
} 