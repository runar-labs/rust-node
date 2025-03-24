use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState};
use runar_node::services::types::ValueType;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A direct API service for subscribing to events
/// Follows the clean separation of concerns architecture
#[derive(Clone)]
pub struct EventSubscriberService {
    name: String,
    context: Option<Arc<RequestContext>>,
    events: Arc<Mutex<HashMap<String, Vec<String>>>>,
    subscription_ids: Arc<Mutex<Vec<String>>>,
}

impl EventSubscriberService {
    /// Create a new EventSubscriberService
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            context: None,
            events: Arc::new(Mutex::new(HashMap::new())),
            subscription_ids: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Set up subscriptions for different event patterns
    pub async fn setup_subscriptions(&self, context: &RequestContext) -> Result<()> {
        // Subscribe to specific service/topic
        let full_path_sub_id = self.subscribe_to_full_path(context).await?;
        
        // Subscribe to just topic
        let topic_only_sub_id = self.subscribe_to_topic_only(context).await?;
        
        // Subscribe to global events
        let global_sub_id = self.subscribe_to_global(context).await?;
        
        // Store subscription IDs
        let mut sub_ids = self.subscription_ids.lock().unwrap();
        sub_ids.push(full_path_sub_id);
        sub_ids.push(topic_only_sub_id);
        sub_ids.push(global_sub_id);
        
        Ok(())
    }

    /// Subscribe to events with full path
    async fn subscribe_to_full_path(&self, context: &RequestContext) -> Result<String> {
        let topic = "publisher/events/test";
        let events_clone = self.events.clone();
        
        let sub_id = context.subscribe(topic, move |payload| {
            let events = events_clone.clone();
            Box::pin(async move {
                let mut events_map = events.lock().unwrap();
                let event_list = events_map.entry(topic.to_string()).or_insert_with(Vec::new);
                
                let event_data = match payload {
                    ValueType::String(s) => s,
                    _ => format!("{:?}", payload),
                };
                
                event_list.push(event_data);
                Ok(())
            })
        }).await?;
        
        Ok(sub_id)
    }

    /// Subscribe to events with topic only
    async fn subscribe_to_topic_only(&self, context: &RequestContext) -> Result<String> {
        let topic = "test_event";
        let events_clone = self.events.clone();
        
        let sub_id = context.subscribe(topic, move |payload| {
            let events = events_clone.clone();
            Box::pin(async move {
                let mut events_map = events.lock().unwrap();
                let event_list = events_map.entry(topic.to_string()).or_insert_with(Vec::new);
                
                let event_data = match payload {
                    ValueType::String(s) => s,
                    _ => format!("{:?}", payload),
                };
                
                event_list.push(event_data);
                Ok(())
            })
        }).await?;
        
        Ok(sub_id)
    }

    /// Subscribe to global events
    async fn subscribe_to_global(&self, context: &RequestContext) -> Result<String> {
        let topic = "/global_event";
        let events_clone = self.events.clone();
        
        let sub_id = context.subscribe(topic, move |payload| {
            let events = events_clone.clone();
            Box::pin(async move {
                let mut events_map = events.lock().unwrap();
                let event_list = events_map.entry(topic.to_string()).or_insert_with(Vec::new);
                
                let event_data = match payload {
                    ValueType::String(s) => s,
                    _ => format!("{:?}", payload),
                };
                
                event_list.push(event_data);
                Ok(())
            })
        }).await?;
        
        Ok(sub_id)
    }

    /// Get all events or events for a specific topic
    pub async fn get_events(&self, topic: Option<&str>) -> Result<ServiceResponse> {
        let events_map = self.events.lock().unwrap();
        
        if let Some(topic_filter) = topic {
            // Get events for specific topic
            if let Some(events) = events_map.get(topic_filter) {
                return Ok(ServiceResponse::success(
                    &format!("Found {} events for topic '{}'", events.len(), topic_filter),
                    Some(ValueType::Json(json!({
                        "topic": topic_filter,
                        "events": events,
                        "count": events.len()
                    })))
                ));
            } else {
                return Ok(ServiceResponse::success(
                    &format!("No events found for topic '{}'", topic_filter),
                    Some(ValueType::Json(json!({
                        "topic": topic_filter,
                        "events": Vec::<String>::new(),
                        "count": 0
                    })))
                ));
            }
        } else {
            // Get all events
            let mut all_events: HashMap<String, Vec<String>> = HashMap::new();
            let mut total_count = 0;
            
            for (topic, events) in events_map.iter() {
                all_events.insert(topic.clone(), events.clone());
                total_count += events.len();
            }
            
            return Ok(ServiceResponse::success(
                &format!("Found {} events across {} topics", total_count, all_events.len()),
                Some(ValueType::Json(json!({
                    "topics": all_events.keys().collect::<Vec<_>>(),
                    "events": all_events,
                    "count": total_count
                })))
            ));
        }
    }

    /// Clear all events
    pub async fn clear_events(&self) -> Result<ServiceResponse> {
        let mut events_map = self.events.lock().unwrap();
        let total_count = events_map.values().map(|v| v.len()).sum::<usize>();
        events_map.clear();
        
        Ok(ServiceResponse::success(
            &format!("Cleared {} events", total_count),
            Some(ValueType::Json(json!({
                "cleared": total_count
            })))
        ))
    }
}

#[async_trait]
impl AbstractService for EventSubscriberService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        "subscriber"
    }

    fn state(&self) -> ServiceState {
        ServiceState::Started
    }

    fn description(&self) -> &str {
        "Direct API event subscriber service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }

    fn operations(&self) -> Vec<String> {
        vec![
            "get_events".to_string(),
            "clear_events".to_string(),
        ]
    }

    async fn init(&mut self, context: &RequestContext) -> Result<()> {
        self.context = Some(Arc::new(context.clone()));
        // Set up subscriptions
        self.setup_subscriptions(context).await?;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        // Clean up subscriptions if needed
        if let Some(context) = &self.context {
            let sub_ids = self.subscription_ids.lock().unwrap();
            for id in sub_ids.iter() {
                let _ = context.unsubscribe("*", Some(id)).await;
            }
        }
        Ok(())
    }

    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract the context from the request
        let _context = &request.context;
        
        // Process based on the operation path
        match request.path.as_str() {
            "get_events" => {
                // Check if we have a topic filter
                if let Some(data) = &request.data {
                    if let ValueType::Map(map) = data {
                        if let Some(ValueType::String(topic)) = map.get("topic") {
                            return self.get_events(Some(topic)).await;
                        }
                    }
                }
                // No topic specified, return all events
                self.get_events(None).await
            },
            "clear_events" => {
                self.clear_events().await
            },
            _ => Err(anyhow::anyhow!("Unknown operation: {}", request.path)),
        }
    }
}
