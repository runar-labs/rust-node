use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState, ActionMetadata, EventMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use std::pin::Pin;
use std::future::Future;

/// A dedicated event subscriber service for testing the event system
pub struct EventSubscriberService {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
    context: Option<RequestContext>,
    received_events: Arc<Mutex<Vec<(String, ValueType)>>>,
    subscriptions: Arc<Mutex<Vec<String>>>,
}

impl Clone for EventSubscriberService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            state: *self.state.lock().unwrap(),
            context: self.context.clone(),
            received_events: Arc::clone(&self.received_events),
            subscriptions: Arc::clone(&self.subscriptions),
        }
    }
}

impl EventSubscriberService {
    /// Create a new event subscriber service
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
            context: None,
            received_events: Arc::new(Mutex::new(Vec::new())),
            subscriptions: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Handle subscribe action - subscribes to a specific event topic
    async fn handle_subscribe(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract topic from request
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
                ValueType::String(topic) => topic.clone(),
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
        
        // Subscribe to the topic
        if let Some(ctx) = &self.context {
            let self_clone = self.clone();
            
            println!("[EventSubscriberService] Subscribing to topic: '{}'", topic);
            
            ctx.subscribe(&topic, move |event_data| {
                let topic_clone = topic.clone();
                let self_clone_inner = self_clone.clone();
                
                Box::pin(async move {
                    // Store the received event
                    let mut events = self_clone_inner.received_events.lock().unwrap();
                    events.push((topic_clone.clone(), event_data.clone()));
                    
                    println!("[EventSubscriberService] Received event on topic '{}': {:?}", 
                             topic_clone, event_data);
                    
                    Ok(())
                }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
            }).await?;
            
            // Add to local subscriptions list
            {
                let mut subscriptions = self.subscriptions.lock().unwrap();
                subscriptions.push(topic.clone());
            }
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Subscribed to topic '{}'", topic),
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
    
    /// Handle get_received_events action - returns all events that have been received
    async fn handle_get_received_events(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        let events = self.received_events.lock().unwrap();
        
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
            message: format!("Retrieved {} received events", event_list.len()),
            data: Some(ValueType::Map(HashMap::from([
                ("events".to_string(), ValueType::Vec(event_list)),
                ("count".to_string(), ValueType::Number(event_list.len() as f64)),
            ]))),
        })
    }
    
    /// Handle get_subscriptions action - returns all active subscriptions
    async fn handle_get_subscriptions(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        let subscriptions = self.subscriptions.lock().unwrap();
        
        // Convert to a format suitable for response
        let subscription_list: Vec<ValueType> = subscriptions
            .iter()
            .map(|topic| ValueType::String(topic.clone()))
            .collect();
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Retrieved {} active subscriptions", subscription_list.len()),
            data: Some(ValueType::Map(HashMap::from([
                ("subscriptions".to_string(), ValueType::Vec(subscription_list)),
                ("count".to_string(), ValueType::Number(subscription_list.len() as f64)),
            ]))),
        })
    }
    
    /// Sets up test subscriptions for common event topics
    async fn setup_test_subscriptions(&self) -> Result<()> {
        if let Some(ctx) = &self.context {
            let self_clone = self.clone();
            
            // Subscribe to a test topic
            ctx.subscribe("test/event", move |event_data| {
                let self_clone_inner = self_clone.clone();
                
                Box::pin(async move {
                    let mut events = self_clone_inner.received_events.lock().unwrap();
                    events.push(("test/event".to_string(), event_data.clone()));
                    
                    println!("[EventSubscriberService] Received test event: {:?}", event_data);
                    
                    Ok(())
                }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
            }).await?;
            
            // Add to local subscriptions list
            {
                let mut subscriptions = self.subscriptions.lock().unwrap();
                subscriptions.push("test/event".to_string());
            }
            
            println!("[EventSubscriberService] Set up test subscriptions");
        }
        
        Ok(())
    }
}

#[async_trait]
impl AbstractService for EventSubscriberService {
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
        "A service that subscribes to events for testing"
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "subscribe".to_string() },
            ActionMetadata { name: "get_received_events".to_string() },
            ActionMetadata { name: "get_subscriptions".to_string() },
        ]
    }
    
    fn events(&self) -> Vec<EventMetadata> {
        vec![]
    }
    
    async fn init(&mut self, ctx: &RequestContext) -> Result<()> {
        println!("[EventSubscriberService] Initializing");
        self.context = Some(ctx.clone());
        *self.state.lock().unwrap() = ServiceState::Initialized;
        
        // Set up default test subscriptions
        let self_clone = self.clone();
        self_clone.setup_test_subscriptions().await?;
        
        Ok(())
    }
    
    async fn start(&mut self) -> Result<()> {
        println!("[EventSubscriberService] Starting");
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        println!("[EventSubscriberService] Stopping");
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Delegate to specialized methods based on action
        match request.action.as_str() {
            "subscribe" => self.handle_subscribe(request).await,
            "get_received_events" => self.handle_get_received_events(request).await,
            "get_subscriptions" => self.handle_get_subscriptions(request).await,
            _ => {
                println!("[EventSubscriberService] Unknown action: {}", request.action);
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: format!("Unknown action: {}", request.action),
                    data: None,
                })
            }
        }
    }
} 