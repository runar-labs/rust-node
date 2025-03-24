use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Storage for events with validation status
#[derive(Clone)]
pub struct EventStorage {
    valid_events: Arc<Mutex<Vec<String>>>,
    invalid_events: Arc<Mutex<Vec<String>>>,
}

impl EventStorage {
    pub fn new() -> Self {
        Self {
            valid_events: Arc::new(Mutex::new(Vec::new())),
            invalid_events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

/// A service for publishing and subscribing to events
/// This implementation uses the Node API directly without macros
pub struct EventService {
    name: String,
    path: String,
    storage: EventStorage,
    context: Option<Arc<RequestContext>>,
}

impl EventService {
    /// Create a new EventService
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: format!("/events/{}", name),
            storage: EventStorage::new(),
            context: None,
        }
    }

    /// Validate event data
    pub async fn validate(&self, event_data: &str) -> Result<ServiceResponse> {
        // Simple validation logic
        if event_data.contains("valid") {
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: "Event data is valid".to_string(),
                data: Some(ValueType::Json(json!({
                    "valid": true,
                    "data": event_data
                }))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Event data is invalid".to_string(),
                data: Some(ValueType::Json(json!({
                    "valid": false,
                    "data": event_data
                }))),
            })
        }
    }

    /// Publish an event
    pub async fn publish(&self, topic: &str, event_data: &str) -> Result<ServiceResponse> {
        // Validate the event data first
        let validation_result = self.validate(event_data).await?;
        let is_valid = validation_result.status == ResponseStatus::Success;
        
        // Publish the event if we have a context
        if let Some(context) = &self.context {
            // Create the event payload
            let payload = json!({
                "topic": topic,
                "data": event_data,
                "valid": is_valid,
                "publisher": self.name
            });
            
            // Publish to the appropriate topic
            let topic_path = format!("{}/{}", self.path, topic);
            context.publish(&topic_path, ValueType::Json(payload.clone())).await?;
            
            // Store the event locally based on validation status
            if is_valid {
                let mut valid_events = self.storage.valid_events.lock().unwrap();
                valid_events.push(event_data.to_string());
            } else {
                let mut invalid_events = self.storage.invalid_events.lock().unwrap();
                invalid_events.push(event_data.to_string());
            }
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Event published to topic: {}", topic),
                data: Some(ValueType::Json(json!({
                    "topic": topic,
                    "data": event_data,
                    "valid": is_valid
                }))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Service not initialized with context".to_string(),
                data: None,
            })
        }
    }

    /// Get valid events
    pub async fn get_valid_events(&self) -> Result<ServiceResponse> {
        let valid_events = self.storage.valid_events.lock().unwrap();
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Found {} valid events", valid_events.len()),
            data: Some(ValueType::Json(json!({
                "events": valid_events.clone()
            }))),
        })
    }

    /// Get invalid events
    pub async fn get_invalid_events(&self) -> Result<ServiceResponse> {
        let invalid_events = self.storage.invalid_events.lock().unwrap();
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Found {} invalid events", invalid_events.len()),
            data: Some(ValueType::Json(json!({
                "events": invalid_events.clone()
            }))),
        })
    }

    /// Handle a valid event
    async fn handle_valid_event(&self, payload: serde_json::Value) {
        if let Some(data) = payload.get("data").and_then(|v| v.as_str()) {
            let mut valid_events = self.storage.valid_events.lock().unwrap();
            valid_events.push(data.to_string());
        }
    }

    /// Handle an invalid event
    async fn handle_invalid_event(&self, payload: serde_json::Value) {
        if let Some(data) = payload.get("data").and_then(|v| v.as_str()) {
            let mut invalid_events = self.storage.invalid_events.lock().unwrap();
            invalid_events.push(data.to_string());
        }
    }

    /// Set up subscriptions
    pub async fn setup_subscriptions(&self, context: &RequestContext) -> Result<()> {
        // Subscribe to valid events
        let valid_topic = format!("{}/valid", self.path);
        let storage_clone = self.storage.clone();
        
        context.subscribe(&valid_topic, move |payload| {
            let storage = storage_clone.clone();
            Box::pin(async move {
                if let ValueType::Json(json_value) = payload {
                    let mut valid_events = storage.valid_events.lock().unwrap();
                    if let Some(data) = json_value.get("data").and_then(|v| v.as_str()) {
                        valid_events.push(data.to_string());
                    }
                }
                Ok(())
            })
        }).await?;
        
        // Subscribe to invalid events
        let invalid_topic = format!("{}/invalid", self.path);
        let storage_clone = self.storage.clone();
        
        context.subscribe(&invalid_topic, move |payload| {
            let storage = storage_clone.clone();
            Box::pin(async move {
                if let ValueType::Json(json_value) = payload {
                    let mut invalid_events = storage.invalid_events.lock().unwrap();
                    if let Some(data) = json_value.get("data").and_then(|v| v.as_str()) {
                        invalid_events.push(data.to_string());
                    }
                }
                Ok(())
            })
        }).await?;
        
        Ok(())
    }
}

#[async_trait]
impl AbstractService for EventService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        ServiceState::Running
    }

    fn description(&self) -> &str {
        "Event publishing and subscription service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn operations(&self) -> Vec<String> {
        vec![
            "validate".to_string(),
            "publish".to_string(),
            "getValidEvents".to_string(),
            "getInvalidEvents".to_string(),
        ]
    }

    async fn init(&mut self, context: &RequestContext) -> Result<()> {
        // Store the context for later use
        self.context = Some(Arc::new(context.clone()));
        
        // Set up subscriptions
        self.setup_subscriptions(context).await?;
        
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Check if operation is empty and extract it from path if needed
        let operation = if request.operation.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.operation.clone()
        };

        match operation.as_str() {
            "validate" => {
                if let Some(data) = &request.data {
                    if let ValueType::Map(map_value) = data {
                        if let Some(ValueType::String(data)) = map_value.get("data") {
                            return self.validate(data).await;
                        }
                    } else if let ValueType::Map(map) = data {
                        if let Some(ValueType::String(data)) = map.get("data") {
                            return self.validate(data).await;
                        }
                    } else if let ValueType::String(data) = params {
                        return self.validate(data).await;
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for validate".to_string(),
                    data: None,
                })
            },
            "publish" => {
                if let Some(data) = &request.data {
                    if let ValueType::Map(map_value) = data {
                        let topic = match map_value.get("topic") {
                            Some(ValueType::String(topic_str)) => topic_str.as_str(),
                            _ => "default",
                        };
                        let data_str = match map_value.get("data") {
                            Some(ValueType::String(data_str)) => data_str.clone(),
                            _ => String::new(),
                        };
                        
                        return self.publish(topic, data).await;
                    } else if let ValueType::Map(map) = data {
                        let topic = match map.get("topic") {
                            Some(ValueType::String(t)) => t.as_str(),
                            _ => "default",
                        };
                        
                        let data = match map.get("data") {
                            Some(ValueType::String(d)) => d.as_str(),
                            _ => "",
                        };
                        
                        return self.publish(topic, data).await;
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for publish".to_string(),
                    data: None,
                })
            },
            "getValidEvents" => {
                return self.get_valid_events().await;
            },
            "getInvalidEvents" => {
                return self.get_invalid_events().await;
            },
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown operation: {}", operation),
                data: None,
            }),
        }
    }
}

impl Clone for EventService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            storage: self.storage.clone(),
            context: self.context.clone(),
        }
    }
}
