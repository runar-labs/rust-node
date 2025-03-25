use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState, ActionMetadata, EventMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A ship service that can land and take off, publishing events when it does
pub struct ShipService {
    name: String,
    path: String,
    state: ServiceState,
    context: Option<RequestContext>,
    flight_status: Arc<Mutex<String>>,
}

impl Clone for ShipService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            state: self.state,
            context: self.context.clone(),
            flight_status: Arc::clone(&self.flight_status),
        }
    }
}

impl ShipService {
    /// Create a new ship service
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: "ship".to_string(), // Fixed path for all ship services
            state: ServiceState::Created,
            context: None,
            flight_status: Arc::new(Mutex::new("airborne".to_string())),
        }
    }
    
    /// Handle land action: Ship lands and publishes a 'landed' event
    async fn handle_land(&self, ctx: &RequestContext) -> Result<ValueType> {
        println!("[ShipService] Handling request: action=land");
        println!("[ShipService] Service path is '{}'", self.path);

        // Update the internal flight status
        {
            let mut status = self.flight_status.lock().unwrap();
            *status = "landed".to_string();
        }

        // Prepare event data
        let mut event_data = HashMap::new();
        event_data.insert("status".to_string(), ValueType::String("landed".to_string()));
        event_data.insert("shipId".to_string(), ValueType::String(self.name.clone()));
        event_data.insert("timestamp".to_string(), ValueType::Number(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as f64
        ));

        // Publish using the full path format (serviceName/eventName)
        let topic = format!("{}/landed", self.path);
        println!("[ShipService] Publishing to topic: '{}'", topic);
        ctx.publish(topic, ValueType::Map(event_data)).await?;

        // Return a simple success response
        Ok(ValueType::String("Ship successfully landed".to_string()))
    }
    
    /// Handle takeOff action: Ship takes off and publishes a 'tookOff' event
    async fn handle_take_off(&self, ctx: &RequestContext) -> Result<ValueType> {
        println!("[ShipService] Handling request: action=takeOff");
        println!("[ShipService] Service path is '{}'", self.path);

        // Update the internal flight status
        {
            let mut status = self.flight_status.lock().unwrap();
            *status = "airborne".to_string();
        }

        // Prepare event data
        let mut event_data = HashMap::new();
        event_data.insert("status".to_string(), ValueType::String("airborne".to_string()));
        event_data.insert("shipId".to_string(), ValueType::String(self.name.clone()));
        event_data.insert("timestamp".to_string(), ValueType::Number(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as f64
        ));

        // Publish using the full path format (serviceName/eventName)
        let topic = format!("{}/tookOff", self.path);
        println!("[ShipService] Publishing to topic: '{}'", topic);
        ctx.publish(topic, ValueType::Map(event_data)).await?;

        // Return a simple success response
        Ok(ValueType::String("Ship successfully took off".to_string()))
    }
    
    /// Get the current flight status
    async fn handle_status(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        let status = self.flight_status.lock().unwrap().clone();
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Ship {} is currently {}", self.name, status),
            data: Some(ValueType::String(status)),
        })
    }
}

#[async_trait]
impl AbstractService for ShipService {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn path(&self) -> &str {
        &self.path
    }
    
    fn state(&self) -> ServiceState {
        self.state
    }
    
    fn description(&self) -> &str {
        "A service that simulates a ship that can land and take off"
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "land".to_string() },
            ActionMetadata { name: "takeOff".to_string() },
            ActionMetadata { name: "status".to_string() },
        ]
    }
    
    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "landed".to_string() },
            EventMetadata { name: "tookOff".to_string() },
        ]
    }
    
    async fn init(&mut self, ctx: &RequestContext) -> Result<()> {
        println!("ShipService init called");
        self.context = Some(ctx.clone());
        self.state = ServiceState::Initialized;
        Ok(())
    }
    
    async fn start(&mut self) -> Result<()> {
        println!("ShipService start called");
        self.state = ServiceState::Running;
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        println!("ShipService stop called");
        self.state = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Delegate to specialized methods based on action
        match request.action.as_str() {
            "land" => {
                let result = self.handle_land(&request.context).await?;
                Ok(ServiceResponse {
                    status: ResponseStatus::Success,
                    message: format!("Ship {} has landed", self.name),
                    data: Some(result),
                })
            },
            "takeOff" => {
                let result = self.handle_take_off(&request.context).await?;
                Ok(ServiceResponse {
                    status: ResponseStatus::Success,
                    message: format!("Ship {} has taken off", self.name),
                    data: Some(result),
                })
            },
            "status" => self.handle_status(request).await,
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