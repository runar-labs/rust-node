// Registry Service Implementation
//
// INTENTION: Provide a consistent API for accessing service metadata through the 
// standard request interface, eliminating the need for direct methods and aligning
// with the architectural principle of using the service request pattern for all operations.
//
// This service provides access to service metadata like states, actions, events, etc.
// through standard request paths like:
// - internal/registry/services/list
// - internal/registry/services/{service_path}
// - internal/registry/services/{service_path}/state

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::services::abstract_service::{AbstractService, CompleteServiceMetadata, ServiceState};
use crate::services::{ActionHandler, LifecycleContext, RequestContext, ServiceResponse, RegistryDelegate};
use runar_common::logging::Logger;
use runar_common::types::ValueType;

/// Registry Info Service - provides information about registered services without holding state
pub struct RegistryService {
    /// The service path
    path: String,
    
    /// Logger instance
    logger: Logger,
    
    /// Registry delegate for accessing node registry information
    registry_delegate: Arc<dyn RegistryDelegate>,
}

impl RegistryService {
    /// Create a new Registry Service
    pub fn new(
        logger: Logger,
        delegate: Arc<dyn RegistryDelegate>,
    ) -> Self {
        RegistryService {
            path: "$registry".to_string(),
            logger,
            registry_delegate: delegate,
        }
    }
    
    /// Extract the service path parameter from the request context
    ///
    /// INTENTION: Consistently extract the service_path parameter from the context's path_params,
    /// providing clear error handling and logging for parameter extraction issues.
    ///
    /// This centralizes the parameter extraction logic that was previously duplicated across
    /// multiple handler methods.
    fn extract_service_path(&self, ctx: &RequestContext) -> Result<String> {
        // Get the service_path from path parameters
        if let Some(path) = ctx.path_params.get("service_path") {
            ctx.logger.debug(format!("Using service_path '{}' from path parameters", path));
            Ok(path.clone())
        } else {
            ctx.logger.error("Missing required 'service_path' parameter");
            Err(anyhow!("Missing required 'service_path' parameter"))
        }
    }
    
    /// Register the list services action
    async fn register_list_services_action(&self, context: &LifecycleContext) -> Result<()> {
        let self_clone = self.clone();
        context.register_action(
            "services/list", 
            Arc::new(move |params, ctx| {
                let inner_self = self_clone.clone();
                Box::pin(async move {
                    inner_self.handle_list_services(params.unwrap_or(ValueType::Null), ctx).await
                })
            })
        ).await?;
        context.logger.debug("Registered services/list action");
        Ok(())
    }
    
    /// Register the service info action
    async fn register_service_info_action(&self, context: &LifecycleContext) -> Result<()> {
        let self_clone = self.clone();
        
        // Use register_action directly since we don't need special options
        context.register_action(
            "services/{service_path}", 
            Arc::new(move |params, ctx| {
                let inner_self = self_clone.clone();
                Box::pin(async move {
                    // Use the helper method to extract service_path
                    match inner_self.extract_service_path(&ctx) {
                        Ok(service_path) => {
                            // Process the request with the extracted service path
                            inner_self.handle_service_info(&service_path, params.unwrap_or(ValueType::Null), ctx).await
                        },
                        Err(_) => {
                            // Return error response if service_path is missing
                            Ok(ServiceResponse::error(400, "Missing required 'service_path' parameter"))
                        }
                    }
                })
            })
        ).await?;
        
        context.logger.debug("Registered services/{service_path} action");
        Ok(())
    }
    
    /// Register the service state action
    async fn register_service_state_action(&self, context: &LifecycleContext) -> Result<()> {
        let self_clone = self.clone();
        
        // Use register_action directly since we don't need special options
        context.register_action(
            "services/{service_path}/state", 
            Arc::new(move |params, ctx| {
                let inner_self = self_clone.clone();
                Box::pin(async move {
                    // Use the helper method to extract service_path
                    match inner_self.extract_service_path(&ctx) {
                        Ok(service_path) => {
                            // Process the request with the extracted service path
                            inner_self.handle_service_state(&service_path, params.unwrap_or(ValueType::Null), ctx).await
                        },
                        Err(_) => {
                            // Return error response if service_path is missing
                            Ok(ServiceResponse::error(400, "Missing required 'service_path' parameter"))
                        }
                    }
                })
            })
        ).await?;
        
        context.logger.debug("Registered services/{service_path}/state action");
        Ok(())
    }
    
    /// Handler for listing all services
    async fn handle_list_services(&self, _params: ValueType, ctx: RequestContext) -> Result<ServiceResponse> {
        let mut services = Vec::new();
        
        // Get all service states and metadata in one call
        let service_states = self.registry_delegate.get_all_service_states().await;
        let service_metadata = self.registry_delegate.get_all_service_metadata().await;
        
        // Combine the information
        for (path, state) in service_states {
            let mut service_info = HashMap::new();
            service_info.insert("path".to_string(), ValueType::String(path.clone()));
            service_info.insert("state".to_string(), ValueType::String(format!("{:?}", state)));
            
            // Add metadata if available
            if let Some(meta) = service_metadata.get(&path) {
                service_info.insert("name".to_string(), ValueType::String(meta.name.clone()));
                service_info.insert("version".to_string(), ValueType::String(meta.version.clone()));
                service_info.insert("description".to_string(), ValueType::String(meta.description.clone()));
            }
            
            services.push(ValueType::Map(service_info));
        }
        
        Ok(ServiceResponse::ok(ValueType::Array(services)))
    }
    
    /// Handler for getting detailed information about a specific service
    async fn handle_service_info(&self, _service_path: &str, _params: ValueType, ctx: RequestContext) -> Result<ServiceResponse> {
        // Extract the service path from path parameters
        let actual_service_path = match self.extract_service_path(&ctx) {
            Ok(path) => path,
            Err(_) => {
                return Ok(ServiceResponse::error(400, "Missing required 'service_path' parameter"));
            }
        };
        
        // Get service state and metadata directly from the registry delegate
        let service_states = self.registry_delegate.get_all_service_states().await;
        
        // Check if the service exists
        if let Some(state) = service_states.get(&actual_service_path) {
            let mut service_info = HashMap::new();
            service_info.insert("path".to_string(), ValueType::String(actual_service_path.to_string()));
            service_info.insert("state".to_string(), ValueType::String(format!("{:?}", state)));
            
            // Add metadata if available
            if let Some(meta) = self.registry_delegate.get_service_metadata(&actual_service_path).await {
                service_info.insert("name".to_string(), ValueType::String(meta.name.clone()));
                service_info.insert("version".to_string(), ValueType::String(meta.version.clone()));
                service_info.insert("description".to_string(), ValueType::String(meta.description.clone()));
                
                // Add registered actions
                let actions: Vec<ValueType> = meta.registered_actions.values()
                    .map(|action| ValueType::String(action.name.clone()))
                    .collect();
                service_info.insert("actions".to_string(), ValueType::Array(actions));
                
                // Add registered events
                let events: Vec<ValueType> = meta.registered_events.values()
                    .map(|event| ValueType::String(event.name.clone()))
                    .collect();
                service_info.insert("events".to_string(), ValueType::Array(events));
                
                // Add timestamps
                service_info.insert("registration_time".to_string(), 
                                   ValueType::Number(meta.registration_time as f64));
                
                if let Some(start_time) = meta.last_start_time {
                    service_info.insert("started_at".to_string(), 
                                       ValueType::Number(start_time as f64));
                    
                    // Calculate uptime if service is running
                    if *state == ServiceState::Running {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let uptime = now.saturating_sub(start_time);
                        service_info.insert("uptime_seconds".to_string(), 
                                           ValueType::Number(uptime as f64));
                    }
                }
            }
            
            Ok(ServiceResponse::ok(ValueType::Map(service_info)))
        } else {
            ctx.logger.warn(format!("Service '{}' not found", actual_service_path));
            Ok(ServiceResponse::error(404, &format!("Service '{}' not found", actual_service_path)))
        }
    }
    
    /// Handler for getting just the state of a service
    async fn handle_service_state(&self, _service_path: &str, _params: ValueType, ctx: RequestContext) -> Result<ServiceResponse> {
        // Extract the service path from path parameters
        let actual_service_path = match self.extract_service_path(&ctx) {
            Ok(path) => path,
            Err(_) => {
                return Ok(ServiceResponse::error(400, "Missing required 'service_path' parameter"));
            }
        };
        
        // Get service state directly from the registry delegate
        let service_states = self.registry_delegate.get_all_service_states().await;
        
        // Print all service states for debugging
        ctx.logger.debug(format!("ALL SERVICE STATES:"));
        for (key, value) in &service_states {
            ctx.logger.debug(format!("  - '{}': {:?}", key, value));
        }
        
        ctx.logger.debug(format!("Looking for service state with key exactly '{}'", actual_service_path));
        
        if let Some(state) = service_states.get(&actual_service_path) {
            let mut state_info = HashMap::new();
            state_info.insert("state".to_string(), ValueType::String(format!("{:?}", state)));
            ctx.logger.debug(format!("Found state {:?} for service {}", state, actual_service_path));
            
            Ok(ServiceResponse::ok(ValueType::Map(state_info)))
        } else {
            ctx.logger.error(format!("Service '{}' not found in state map", actual_service_path));
            Ok(ServiceResponse::error(404, &format!("Service '{}' not found", actual_service_path)))
        }
    }
}

#[async_trait]
impl AbstractService for RegistryService {
    fn name(&self) -> &str {
        "registry"
    }

    fn path(&self) -> &str {
        &self.path
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn description(&self) -> &str {
        "Service providing metadata about registered services"
    }

    /// Initialize the Registry Service by registering all handlers
    ///
    /// INTENTION: Set up all the action handlers for the registry service,
    /// following the path template pattern for consistent parameter extraction.
    /// Each path template defines a specific API endpoint with parameters.
    async fn init(&self, context: LifecycleContext) -> Result<()> {
        context.logger.info("Initializing Registry Service");
        
        // Register all actions with their template patterns
        context.logger.debug("Registering Registry Service action handlers");
        
        // Services list does not require parameters
        self.register_list_services_action(&context).await?;
        context.logger.debug("Registered handler for listing all services");
        
        // Service info uses the {service_path} parameter
        self.register_service_info_action(&context).await?;
        context.logger.debug("Registered handler for service info with path parameter");
        
        // Service state also uses the {service_path} parameter
        self.register_service_state_action(&context).await?;
        context.logger.debug("Registered handler for service state with path parameter");
        
        context.logger.info("Registry Service initialization complete");
        Ok(())
    }

    async fn start(&self, context: LifecycleContext) -> Result<()> {
        context.logger.info("Starting Registry Service");
        Ok(())
    }

    async fn stop(&self, context: LifecycleContext) -> Result<()> {
        context.logger.info("Stopping Registry Service");
        Ok(())
    }
}

// Implement Clone manually since we can't derive it due to async_trait
impl Clone for RegistryService {
    fn clone(&self) -> Self {
        Self {
            path: self.path.clone(),
            logger: self.logger.clone(),
            registry_delegate: self.registry_delegate.clone(),
        }
    }
} 