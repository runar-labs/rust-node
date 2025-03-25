use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState, ActionMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use runar_node::logging::{info_log, Component};
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

/// A base station service that monitors ship events
pub struct BaseStationService {
    name: String,
    path: String,
    state: ServiceState,
    context: Option<RequestContext>,
    events: Arc<Mutex<Vec<ValueType>>>, // Store all received events
}

impl Clone for BaseStationService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            state: self.state,
            context: self.context.clone(),
            events: Arc::clone(&self.events),
        }
    }
}

impl BaseStationService {
    /// Create a new base station service
    pub fn new(name: &str) -> Self {
        println!("[BaseStationService] Creating new instance with name={}", name);
        Self {
            name: name.to_string(),
            path: "base".to_string(),
            state: ServiceState::Created,
            context: None,
            events: Arc::new(Mutex::new(Vec::new())),
        }
    }
    
    /// Handle get_events action: returns all collected events
    async fn handle_get_events(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        // Get all stored events
        let events = self.events.lock().unwrap().clone();
        
        // Create response data with counts by type
        let mut landed_count = 0;
        let mut took_off_count = 0;
        
        for event in &events {
            if let ValueType::Map(map) = event {
                if let Some(ValueType::String(status)) = map.get("status") {
                    if status == "landed" {
                        landed_count += 1;
                    } else if status == "airborne" {
                        took_off_count += 1;
                    }
                }
            }
        }
        
        // Build response with counts and full events
        let mut data = HashMap::new();
        data.insert("total_events".to_string(), ValueType::Number(events.len() as f64));
        data.insert("landed_count".to_string(), ValueType::Number(landed_count as f64));
        data.insert("took_off_count".to_string(), ValueType::Number(took_off_count as f64));
        data.insert("events".to_string(), ValueType::Array(events));
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: "Event data retrieved".to_string(),
            data: Some(ValueType::Map(data)),
        })
    }
    
    /// Set up subscriptions to ship events
    async fn setup_subscriptions(&self, ctx: &RequestContext) -> Result<()> {
        info_log(Component::Service, "About to set up subscriptions").await;
        info_log(Component::Service, "Setting up subscriptions").await;

        // Using shorthand format without network ID for subscriptions, matching how ShipService publishes
        println!("[BaseStationService] DEBUG: Using shorthand formats (without network ID)");

        // Test a simple subscription 
        info_log(Component::Service, "Testing simple subscription to 'debug/test'").await;
        match ctx.subscribe("debug/test", |_payload| {
            // This is just a test subscription
            Ok(())
        }).await {
            Ok(_) => info_log(Component::Service, "Debug subscription succeeded").await,
            Err(e) => info_log(Component::Service, &format!("Debug subscription failed: {}", e)).await,
        }

        // Test with just the service name (should fail because it lacks the event name)
        info_log(Component::Service, "Experimenting with 'ship' alone to see what happens").await;
        match ctx.subscribe("ship", |_payload| {
            Ok(())
        }).await {
            Ok(_) => info_log(Component::Service, "Ship-only subscription worked unexpectedly").await,
            Err(e) => info_log(Component::Service, &format!("'ship' subscription failed as expected: {}", e)).await,
        }

        // Subscribe to ship landed event using shorthand format (without network ID)
        let base_clone = self.clone();
        println!("[BaseStationService] Subscribing to 'ship/landed'");
        match ctx.subscribe("ship/landed", move |payload| {
            if let ValueType::Map(event_data) = &payload {
                // Store the event in non-await context (can't use await in closures)
                let mut events = base_clone.events.lock().unwrap();
                events.push(payload.clone());
                println!("[BaseStationService] Received 'landed' event: {:?}", event_data);
            }
            Ok(())
        }).await {
            Ok(_) => info_log(Component::Service, "Successfully subscribed to 'ship/landed'").await,
            Err(e) => info_log(Component::Service, &format!("'ship/landed' subscription failed: {}", e)).await,
        }

        // Subscribe to ship tookOff event using shorthand format (without network ID)
        let base_clone = self.clone();
        println!("[BaseStationService] Subscribing to 'ship/tookOff'");
        match ctx.subscribe("ship/tookOff", move |payload| {
            if let ValueType::Map(event_data) = &payload {
                // Store the event in non-await context (can't use await in closures)
                let mut events = base_clone.events.lock().unwrap();
                events.push(payload.clone());
                println!("[BaseStationService] Received 'tookOff' event: {:?}", event_data);
            }
            Ok(())
        }).await {
            Ok(_) => info_log(Component::Service, "Successfully subscribed to 'ship/tookOff'").await,
            Err(e) => info_log(Component::Service, &format!("'ship/tookOff' subscription failed: {}", e)).await,
        }

        info_log(Component::Service, "Subscriptions setup complete").await;
        Ok(())
    }
}

#[async_trait]
impl AbstractService for BaseStationService {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn path(&self) -> &str {
        &self.path
    }
    
    fn state(&self) -> ServiceState {
        self.state
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn description(&self) -> &str {
        "A base station that monitors ship events"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "get_events".to_string() }
        ]
    }
    
    async fn init(&mut self, ctx: &RequestContext) -> Result<()> {
        println!("BaseStationService init called");
        self.context = Some(ctx.clone());
        
        // Set up event subscriptions
        println!("[BaseStationService] About to set up subscriptions");
        self.setup_subscriptions(ctx).await?;
        println!("[BaseStationService] Subscriptions setup complete");
        
        self.state = ServiceState::Initialized;
        Ok(())
    }
    
    async fn start(&mut self) -> Result<()> {
        println!("BaseStationService start called");
        self.state = ServiceState::Running;
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        println!("BaseStationService stop called");
        self.state = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Debug the request
        println!("[BaseStationService] Handling request: action={}", request.action);
        
        // Delegate to specialized methods based on action
        match request.action.as_str() {
            "get_events" => self.handle_get_events(request).await,
            _ => {
                println!("  → Error: unknown action '{}'", request.action);
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: format!("Unknown action: {}", request.action),
                    data: None,
                })
            }
        }
    }
} 