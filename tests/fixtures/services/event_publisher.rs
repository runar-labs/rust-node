use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState, ActionMetadata, EventMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A dedicated event publisher service for testing the event system
pub struct EventPublisherService {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
    context: Option<RequestContext>,
    published_events: Arc<Mutex<Vec<(String, ValueType)>>>,
}

impl Clone for EventPublisherService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            state: *self.state.lock().unwrap(),
            context: self.context.clone(),
            published_events: Arc::clone(&self.published_events),
        }
    }
}

impl EventPublisherService {
    /// Create a new event publisher service
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
            context: None,
            published_events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Handle publish action - publishes an event to the specified topic
    async fn handle_publish(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract topic and data from request
        let topic = if let Some(data) = &request.data {
            match data {
                ValueType::Map(map) => {
                    if let Some(ValueType::String(topic)) = map.get("topic") {
                        topic.clone()
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Missing 'topic' field in parameters".to_string(),
                            data: None,
                        });
                    }
                },
                _ => {
                    return Ok(ServiceResponse {
                        status: ResponseStatus::Error,
                        message: "Invalid parameters format".to_string(),
                        data: None,
                    });
                }
            }
        } else {
            return Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Missing parameters".to_string(),
                data: None,
            });
        };
        
        // Extract event data
        let event_data = if let Some(ValueType::Map(map)) = &request.data {
            if let Some(data) = map.get("data") {
                data.clone()
            } else {
                ValueType::Map(HashMap::new())
            }
        } else {
            ValueType::Map(HashMap::new())
        };
        
        // Store the event locally for verification
        {
            let mut events = self.published_events.lock().unwrap();
            events.push((topic.clone(), event_data.clone()));
        }
        
        // Publish the event
        if let Some(ctx) = &self.context {
            println!("[EventPublisherService] Publishing to topic: '{}'", topic);
            ctx.publish(topic.clone(), event_data).await?;
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Event published to topic '{}'", topic),
                data: None,
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Service not initialized".to_string(),
                data: None,
            })
        }
    }
    
    /// Handle get_published_events action - returns all events that have been published
    async fn handle_get_published_events(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        let events = self.published_events.lock().unwrap();
        
        // Convert to a format suitable for response
        let mut event_list = Vec::new();
        for (topic, data) in events.iter() {
            let mut event_map = HashMap::new();
            event_map.insert("topic".to_string(), ValueType::String(topic.clone()));
            event_map.insert("data".to_string(), data.clone());
            event_list.push(ValueType::Map(event_map));
        }
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Retrieved {} published events", event_list.len()),
            data: Some(ValueType::Map(HashMap::from([
                ("events".to_string(), ValueType::Vec(event_list)),
                ("count".to_string(), ValueType::Number(event_list.len() as f64)),
            ]))),
        })
    }
    
    /// Creates a standard event with timestamp and source metadata
    pub fn create_event(&self, event_type: &str, data: HashMap<String, ValueType>) -> ValueType {
        let mut event_data = HashMap::new();
        
        // Add standard metadata
        event_data.insert("type".to_string(), ValueType::String(event_type.to_string()));
        event_data.insert("source".to_string(), ValueType::String(self.name.clone()));
        event_data.insert("timestamp".to_string(), ValueType::Number(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64()
        ));
        
        // Add custom data
        for (key, value) in data {
            event_data.insert(key, value);
        }
        
        ValueType::Map(event_data)
    }
}

#[async_trait]
impl AbstractService for EventPublisherService {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn path(&self) -> &str {
        &self.path
    }
    
    fn state(&self) -> ServiceState {
        *self.state.lock().unwrap()
    }
    
    fn description(&self) -> &str {
        "A service that publishes events for testing"
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "publish".to_string() },
            ActionMetadata { name: "get_published_events".to_string() },
        ]
    }
    
    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "test_event".to_string() },
            EventMetadata { name: "data_event".to_string() },
        ]
    }
    
    async fn init(&mut self, ctx: &RequestContext) -> Result<()> {
        println!("[EventPublisherService] Initializing");
        self.context = Some(ctx.clone());
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }
    
    async fn start(&mut self) -> Result<()> {
        println!("[EventPublisherService] Starting");
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        println!("[EventPublisherService] Stopping");
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Delegate to specialized methods based on action
        match request.action.as_str() {
            "publish" => self.handle_publish(request).await,
            "get_published_events" => self.handle_get_published_events(request).await,
            _ => {
                println!("[EventPublisherService] Unknown action: {}", request.action);
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: format!("Unknown action: {}", request.action),
                    data: None,
                })
            }
        }
    }
} 