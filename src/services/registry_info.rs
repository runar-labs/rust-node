use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState};
use crate::services::service_registry::ServiceRegistry;
use crate::services::{ResponseStatus, ServiceRequest, ServiceResponse, ValueType};
use crate::services::NodeRequestHandler;

/// Registry Info Service - provides information about registered services
pub struct RegistryInfoService {
    /// The service name
    name: String,

    /// The path at which the service is available
    path: String,

    /// Current state of the service
    state: Mutex<ServiceState>,

    /// Reference to the ServiceRegistry
    registry: Arc<ServiceRegistry>,
}

impl RegistryInfoService {
    /// Create a new Registry Info Service
    pub fn new(network_id: &str, registry: Arc<ServiceRegistry>) -> Self {
        RegistryInfoService {
            name: "registry_info".to_string(),
            path: format!("{}/registry", network_id),
            state: Mutex::new(ServiceState::Created),
            registry,
        }
    }

    // Helper method to get services without await
    fn get_services(&self) -> Vec<String> {
        // Use the NodeRequestHandler trait method
        NodeRequestHandler::list_services(&*self.registry)
    }
}

#[async_trait]
impl AbstractService for RegistryInfoService {
    fn name(&self) -> &str {
        "registry_info"
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        ServiceState::Running
    }

    fn metadata(&self) -> ServiceMetadata {
        ServiceMetadata {
            name: self.name().to_string(),
            path: self.path().to_string(),
            state: self.state(),
            description: self.description().to_string(),
            operations: vec![
                "list".to_string(),
                "get".to_string(),
                "register_metadata".to_string(),
                "unregister_metadata".to_string(),
            ],
            version: "1.0".to_string(),
        }
    }

    async fn init(&mut self, _context: &crate::services::RequestContext) -> Result<()> {
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Service providing metadata about registered services"
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract the operation from the request
        let operation = &request.operation;
        
        match operation.as_str() {
            "list" => {
                // List all services from the registry
                let services = self.get_services();
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Success,
                    message: "Services listed successfully".to_string(),
                    data: Some(ValueType::Array(
                        services.into_iter().map(|s| ValueType::String(s)).collect()
                    )),
                })
            }
            "get" => {
                // Get information about a specific service
                if let Some(params) = &request.params {
                    if let ValueType::Map(param_map) = params {
                        if let Some(ValueType::String(name)) = param_map.get("name") {
                            match self.registry.get_service(&name).await {
                                Some(service) => {
                                    let mut service_map = HashMap::new();
                                    service_map.insert("name".to_string(), ValueType::String(service.name().to_string()));
                                    service_map.insert("path".to_string(), ValueType::String(service.path().to_string()));
                                    service_map.insert("state".to_string(), ValueType::String(service.state().to_string()));
                                    service_map.insert("description".to_string(), ValueType::String(service.description().to_string()));
                                    
                                    return Ok(ServiceResponse::success(
                                        format!("Service '{}' info", name),
                                        Some(ValueType::Map(service_map)),
                                    ));
                                }
                                None => {
                                    return Ok(ServiceResponse::error(format!("Service '{}' not found", name)));
                                }
                            }
                        }
                    }
                }
                
                Ok(ServiceResponse::error("Missing or invalid service name parameter"))
            }
            _ => {
                Ok(ServiceResponse::error(format!("Unknown operation: {}", operation)))
            }
        }
    }
}
