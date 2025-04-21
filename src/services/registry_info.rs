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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use std::collections::HashMap;

use crate::services::abstract_service::{AbstractService, ServiceState};
use crate::services::{LifecycleContext, RequestContext, ServiceResponse, RegistryDelegate};
use runar_common::logging::Logger;
use runar_common::types::ValueType;
use runar_common::vmap;
use crate::routing::TopicPath;

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
    /// INTENTION: Extract the target service path (the service we want info about) 
    /// from the path parameters extracted from the URL template.
    ///
    /// This is NOT about getting the context's handler service path.
    /// It's about extracting the service name from the URL pattern like:
    /// "$registry/services/math" where "math" is the service we want information about.
    /// it uses the context path_params which contains the extracted parameters from the template
    fn extract_service_path(&self, ctx: &RequestContext) -> Result<String> {
        // Look for the path parameter that was extracted during template matching
        if let Some(path) = ctx.path_params.get("service_path") {
            ctx.logger.debug(format!("Using service_path '{}' from path parameters", path));
            return Ok(path.clone());
        }
        
        // If we get here, we couldn't find the target service path
        ctx.logger.error("Missing required 'service_path' parameter");
        Err(anyhow!("Missing required 'service_path' parameter"))
    }
    
    /// Register the list services action
    async fn register_list_services_action(&self, context: &LifecycleContext) -> Result<()> {
        let self_clone = self.clone();
        // Add debug to see what is registered
        context.logger.debug(format!("Registering list_services handler with path: services/list"));
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
        
        // Add debug to see what is registered
        context.logger.debug(format!("Registering service_info handler with path: services/{{service_path}}"));
        
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
                            Ok(ServiceResponse::error(400, "Missing required service path parameter"))
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

        // Add debug to see what is registered
        context.logger.debug(format!("Registering service_state handler with path: services/{{service_path}}/state"));
        
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
                            Ok(ServiceResponse::error(400, "Missing required service path parameter"))
                        }
                    }
                })
            })
        ).await?;
        
        context.logger.debug("Registered services/{service_path}/state action");
        Ok(())
    }
    
    /// Handler for listing all services
    async fn handle_list_services(&self, params: ValueType, ctx: RequestContext) -> Result<ServiceResponse> {
        let mut services = Vec::new();
        
        // Parse parameters to check if we should include detailed action metadata
        let include_actions = if let ValueType::Map(param_map) = &params {
            match param_map.get("include_actions") {
                Some(ValueType::Bool(value)) => *value,
                Some(ValueType::String(value)) => value.to_lowercase() == "true",
                _ => false,
            }
        } else {
            false
        };
        
        ctx.logger.debug(format!("List services request with include_actions={}", include_actions));
        
        // Get all service states and metadata in one call
        let service_states = self.registry_delegate.get_all_service_states().await;
        let service_metadata = self.registry_delegate.get_all_service_metadata(false).await;
        
        // Combine the information
        for (path, state) in service_states {
            // Create base service info
            let mut service_info = if let Some(meta) = service_metadata.get(&path) {
                vmap! {
                    "path" => path.clone(),
                    "state" => format!("{:?}", state),
                    "name" => meta.name.clone(),
                    "version" => meta.version.clone(),
                    "description" => meta.description.clone()
                }
            } else {
                vmap! {
                    "path" => path.clone(),
                    "state" => format!("{:?}", state)
                }
            };
            
            // Include action metadata if requested
            if include_actions {
                if let Some(meta) = service_metadata.get(&path) {
                    if !meta.registered_actions.is_empty() {
                        // Extract network ID from the path (format: "network_id:path")
                        let network_id = if path.contains(':') {
                            path.split(':').next().unwrap_or("").to_string()
                        } else {
                            // Default to test-network if not specified
                            "test-network".to_string()
                        };
                        
                        let actions: Vec<ValueType> = meta.registered_actions.values()
                            .map(|action| {
                                let desc = action.description.clone();
                                
                                // First create the base metadata
                                let mut action_metadata_map = HashMap::new();
                                action_metadata_map.insert("name".to_string(), ValueType::String(action.name.clone()));
                                action_metadata_map.insert("description".to_string(), 
                                    ValueType::String(if desc.is_empty() { "No description".to_string() } else { desc }));
                                action_metadata_map.insert("full_path".to_string(), 
                                    ValueType::String(format!("{}:{}/{}", network_id, path, action.name)));
                                
                                // Add parameter schema if available
                                if let Some(params_schema) = &action.parameters_schema {
                                    action_metadata_map.insert("parameters_schema".to_string(), params_schema.clone());
                                }
                                
                                // Add return schema if available
                                if let Some(result_schema) = &action.return_schema {
                                    action_metadata_map.insert("return_schema".to_string(), result_schema.clone());
                                }
                                
                                ValueType::Map(action_metadata_map)
                            })
                            .collect();
                        
                        if let ValueType::Map(ref mut service_map) = service_info {
                            service_map.insert("actions".to_string(), ValueType::Array(actions));
                            // Include network ID at service level too
                            service_map.insert("network_id".to_string(), ValueType::String(network_id));
                        }
                    }
                }
            }
            
            services.push(service_info);
        }
        
        Ok(ServiceResponse::ok(ValueType::Array(services)))
    }
    
    /// Handler for getting detailed information about a specific service
    async fn handle_service_info(&self, _service_path: &str, params: ValueType, ctx: RequestContext) -> Result<ServiceResponse> {
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
            // Use vmap! macro to build the response
            let mut service_info = vmap! {
                "path" => actual_service_path.clone(),
                "state" => format!("{:?}", state)
            };
            
            // Create a TopicPath from the service path
            let service_topic = match TopicPath::new(&actual_service_path, "default") {
                Ok(topic) => topic,
                Err(_) => {
                    return Ok(ServiceResponse::error(500, "Invalid service path format"));
                }
            };
            
            // Add metadata if available
            if let Some(meta) = self.registry_delegate.get_service_metadata(&service_topic).await {
                // Extract network ID from the path (format: "network_id:path")
                let network_id = if actual_service_path.contains(':') {
                    actual_service_path.split(':').next().unwrap_or("").to_string()
                } else {
                    // Default to test-network if not specified
                    "test-network".to_string()
                };
                
                // Create a new map with metadata
                let metadata_info = vmap! {
                    "name" => meta.name.clone(),
                    "version" => meta.version.clone(),
                    "description" => meta.description.clone(),
                    "network_id" => network_id.clone()
                };
                
                // Merge the maps
                if let ValueType::Map(metadata_map) = metadata_info {
                    if let ValueType::Map(ref mut service_map) = service_info {
                        service_map.extend(metadata_map);
                    }
                }
                
                // Add registered actions with detailed metadata
                let actions: Vec<ValueType> = meta.registered_actions.values()
                    .map(|action| {
                        let desc = action.description.clone();
                        
                        // First create the base metadata
                        let mut action_metadata_map = HashMap::new();
                        action_metadata_map.insert("name".to_string(), ValueType::String(action.name.clone()));
                        action_metadata_map.insert("description".to_string(), 
                            ValueType::String(if desc.is_empty() { "No description".to_string() } else { desc }));
                        action_metadata_map.insert("full_path".to_string(), 
                            ValueType::String(format!("{}:{}/{}", network_id, actual_service_path, action.name)));
                        
                        // Add parameter schema if available
                        if let Some(params_schema) = &action.parameters_schema {
                            action_metadata_map.insert("parameters_schema".to_string(), params_schema.clone());
                        }
                        
                        // Add return schema if available
                        if let Some(result_schema) = &action.return_schema {
                            action_metadata_map.insert("return_schema".to_string(), result_schema.clone());
                        }
                        
                        ValueType::Map(action_metadata_map)
                    })
                    .collect();
                
                if let ValueType::Map(ref mut service_map) = service_info {
                    service_map.insert("actions".to_string(), ValueType::Array(actions));
                }
                
                // Add registered events with more detailed metadata
                let events: Vec<ValueType> = meta.registered_events.values()
                    .map(|event| {
                        let desc = event.description.clone();
                        
                        // First create the base metadata
                        let mut event_metadata_map = HashMap::new();
                        event_metadata_map.insert("name".to_string(), ValueType::String(event.name.clone()));
                        event_metadata_map.insert("description".to_string(), 
                            ValueType::String(if desc.is_empty() { "No description".to_string() } else { desc }));
                        event_metadata_map.insert("full_path".to_string(), 
                            ValueType::String(format!("{}:{}/{}", network_id, actual_service_path, event.name)));
                        
                        // Add schema if available
                        if let Some(schema) = &event.data_schema {
                            event_metadata_map.insert("schema".to_string(), schema.clone());
                        }
                        
                        ValueType::Map(event_metadata_map)
                    })
                    .collect();
                
                if let ValueType::Map(ref mut service_map) = service_info {
                    service_map.insert("events".to_string(), ValueType::Array(events));
                }
                
                // Add timestamps
                if let ValueType::Map(ref mut service_map) = service_info {
                    service_map.insert("registration_time".to_string(), 
                                       ValueType::Number(meta.registration_time as f64));
                    
                    if let Some(start_time) = meta.last_start_time {
                        service_map.insert("started_at".to_string(), 
                                           ValueType::Number(start_time as f64));
                        
                        // Calculate uptime if service is running
                        if *state == ServiceState::Running {
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let uptime = now.saturating_sub(start_time);
                            service_map.insert("uptime_seconds".to_string(), 
                                               ValueType::Number(uptime as f64));
                        }
                    }
                }
            }
            
            Ok(ServiceResponse::ok(service_info))
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
            // Use vmap! macro to build the response
            let state_info = vmap! {
                "state" => format!("{:?}", state)
            };
            
            ctx.logger.debug(format!("Found state {:?} for service {}", state, actual_service_path));
            
            Ok(ServiceResponse::ok(state_info))
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
        "Registry service for service discovery and metadata"
    }
    
    fn network_id(&self) -> Option<String> {
        None
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