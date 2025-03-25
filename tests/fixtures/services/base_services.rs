// Common base services for testing
use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState, ActionMetadata, EventMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType, vmap};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// A minimal test service that just responds with a simple message
/// This is useful for basic service registration/discovery tests
pub struct SimpleTestService {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
}

impl SimpleTestService {
    /// Create a new SimpleTestService
    pub fn new(name: &str) -> Self {
        SimpleTestService {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
        }
    }

    /// Process any operation generically - this service just echoes back the operation and parameters
    async fn process_any_operation(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Simple echo behavior - returns the action and data
        let action = if request.action.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.action.clone()
        };

        // Return a response with the operation name and the parameters
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Echo service processed operation '{}'", action),
            data: Some(ValueType::Map(HashMap::from([
                ("action".to_string(), ValueType::String(action)),
                ("data".to_string(), request.data.clone().unwrap_or(ValueType::Null)),
                ("service".to_string(), ValueType::String(self.name.clone())),
            ]))),
        })
    }
}

#[async_trait]
impl AbstractService for SimpleTestService {
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
        "Simple Test Service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "test".to_string() },
            ActionMetadata { name: "dummy".to_string() },
        ]
    }

    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "test_event".to_string() },
        ]
    }

    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Process any operation
        self.process_any_operation(request).await
    }
}

/// A counter service for testing
/// This service maintains a counter that can be incremented and retrieved
pub struct CounterService {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
    counter: Mutex<i32>,
}

impl CounterService {
    /// Create a new CounterService
    pub fn new(name: &str) -> Self {
        CounterService {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
            counter: Mutex::new(0),
        }
    }

    /// Handle increment operation
    async fn handle_increment(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let amount = if let Some(data) = &request.data {
            match data {
                ValueType::Number(n) => *n as i32,
                ValueType::Map(map) => {
                    if let Some(ValueType::Number(n)) = map.get("amount") {
                        *n as i32
                    } else {
                        1 // Default increment
                    }
                },
                _ => 1, // Default increment
            }
        } else {
            1 // Default increment
        };
        
        // Increment the counter
        let mut counter = self.counter.lock().unwrap();
        *counter += amount;
        
        // Return the new counter value
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Counter incremented by {}", amount),
            data: Some(ValueType::Number(*counter as f64)),
        })
    }

    /// Handle get_value operation
    async fn handle_get_value(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        // Get the current counter value
        let counter = self.counter.lock().unwrap();
        
        // Return the current counter value
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: "Current counter value".to_string(),
            data: Some(ValueType::Number(*counter as f64)),
        })
    }
}

#[async_trait]
impl AbstractService for CounterService {
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
        "Counter Service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "increment".to_string() },
            ActionMetadata { name: "get_value".to_string() },
        ]
    }

    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "incremented".to_string() },
        ]
    }

    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Determine which operation to handle
        let action = if request.action.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.action.clone()
        };
        
        match action.as_str() {
            "increment" => self.handle_increment(request).await,
            "get_value" => self.handle_get_value(request).await,
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown operation: {}", action),
                data: None,
            }),
        }
    }
} 