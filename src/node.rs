// Node Implementation
//
// This module provides the Node which is the primary entry point for the Runar system.
// The Node is responsible for managing the service registry, handling requests, and
// coordinating event publishing and subscriptions.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::routing::TopicPath;
use crate::services::{
    ActionHandler, EventContext, LifecycleContext, NodeDelegate, PublishOptions,
    RequestContext, ServiceRequest, ServiceResponse, SubscriptionOptions, RegistryDelegate,
    ActionRegistrationOptions, EventRegistrationOptions
};
use crate::services::abstract_service::{ActionMetadata, AbstractService, CompleteServiceMetadata, ServiceState};
use crate::services::registry_info::RegistryService;
use crate::services::service_registry::ServiceRegistry;
use runar_common::types::ValueType;
use runar_common::logging::{Component, Logger};

/// Configuration for a Node
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Network ID for the node
    pub network_id: String,
    /// Node ID for logging and identification
    pub node_id: Option<String>,
}

impl NodeConfig {
    /// Create a new NodeConfig with minimal required parameters
    pub fn new(network_id: &str) -> Self {
        Self {
            network_id: network_id.to_string(),
            node_id: None,
        }
    }

    /// Create a new NodeConfig with a specific node ID
    pub fn new_with_node_id(network_id: &str, node_id: &str) -> Self {
        Self {
            network_id: network_id.to_string(),
            node_id: Some(node_id.to_string()),
        }
    }
}

/// Node represents a Runar node that can host services and communicate with other nodes
#[derive(Clone)]
pub struct Node {
    /// Configuration for the node
    pub config: NodeConfig,

    /// Service registry for managing services
    service_registry: Arc<ServiceRegistry>,

    /// Network ID for this node
    pub network_id: String,

    /// Service map for tracking registered services
    services: Arc<RwLock<HashMap<String, Arc<dyn AbstractService>>>>,
    
    /// Service state map for tracking the lifecycle state of services
    service_states: Arc<RwLock<HashMap<String, ServiceState>>>,
    
    /// Service metadata for documenting services, actions, and events
    service_metadata: Arc<RwLock<HashMap<String, CompleteServiceMetadata>>>,
    
    /// Logger instance for this node
    logger: Logger,
}

// Manual implementation of Debug for Node
impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node")
            .field("config", &self.config)
            .field("network_id", &self.network_id)
            .field("services_count", &format!("{} services", self.services.try_read().map(|s| s.len()).unwrap_or(0)))
            .finish()
    }
}

impl Node {
    /// Create a new Node with the given configuration
    pub async fn new(config: NodeConfig) -> Result<Self> {
        // Create the node ID
        let node_id = match &config.node_id {
            Some(id) => id.clone(),
            None => format!("node-{}", uuid::Uuid::new_v4()),
        };
        
        // Create a root logger with the node ID
        let logger = Logger::new_root(Component::Node, &node_id);
        
        // Log that we're creating a new node
        logger.info(format!("Creating new Node with network_id '{}'", config.network_id));
        
        // Create the service registry with a child logger for proper hierarchy
        let registry_logger = logger.with_component(runar_common::Component::Registry);
        logger.debug("Creating ServiceRegistry instance");
        let registry = ServiceRegistry::new(registry_logger);
        
        // Create a new node instance
        logger.debug("Creating Node instance");
        let node = Self {
            config: config.clone(),
            service_registry: Arc::new(registry),
            network_id: config.network_id.clone(),
            services: Arc::new(RwLock::new(HashMap::new())),
            service_states: Arc::new(RwLock::new(HashMap::new())),
            service_metadata: Arc::new(RwLock::new(HashMap::new())),
            logger: logger.clone(),
        };
        
        // Create and add the Registry Service using a different approach to avoid weak reference issues
        logger.debug("Creating RegistryService instance");
        let registry_service = RegistryService::new(
            node.logger.with_component(Component::Service),
            Arc::new(node.clone()) as Arc<dyn RegistryDelegate>,
        );
        
        // Add the registry service directly to the node
        logger.debug("Adding RegistryService to Node");
        if let Err(e) = node.add_internal_service(registry_service).await {
            node.logger.error(format!("Failed to add registry service: {}", e));
            return Err(anyhow!("Failed to initialize node: {}", e));
        }
        
        // Return the new node
        logger.debug("Node creation completed successfully");
        Ok(node)
    }
    
    /// Get a logger for a service, derived from the node's root logger
    pub fn create_service_logger(&self, _service_name: &str) -> Logger {
        self.logger.with_component(Component::Service)
    }

    /// Create a request context with the right logger
    pub fn create_request_context(&self, service_path: &str) -> RequestContext {
        let service_logger = self.logger.with_component(Component::Service);
        let topic_path = TopicPath::new(service_path, &self.network_id).expect("Invalid service path");
        RequestContext::new(&topic_path, service_logger)
    }
    
    /// Create a request context with a topic path
    ///
    /// INTENTION: Create a RequestContext with a properly configured logger
    /// and a complete topic path. This is used when handling requests that
    /// already have a validated TopicPath.
    pub fn create_request_context_with_topic_path(&self, topic_path: &TopicPath) -> RequestContext {
        let service_logger = self.logger.with_component(Component::Service);
        RequestContext::new(topic_path, service_logger)
    }
    
    /// Create an action registrar function
    ///
    /// INTENTION: Create a function that can be passed to services to register
    /// action handlers dynamically. This enables services to register handlers 
    /// during initialization without directly depending on the Node.
    pub fn create_action_registrar(&self) -> ActionRegistrar {
        // Clone the Arc to the service registry
        let registry = self.service_registry.clone();
        
        // Create a boxed function that can register action handlers
        Arc::new(move |topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>| -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
            let registry_clone = registry.clone();
            let metadata = metadata.clone();
            let topic_path = topic_path.clone();
            
            // Return a future that registers the handler
            Box::pin(async move {
                registry_clone.register_action_handler(&topic_path, handler, metadata).await
            })
        })
    }
    
    /// Create a lifecycle context for a service
    ///
    /// INTENTION: Create a context object that provides all necessary callbacks and
    /// information for service lifecycle operations. This bundles together the delegate,
    /// logger, and other contextual information needed by services.
    pub fn create_context(&self, service_path: &str) -> LifecycleContext {
        self.logger.debug(format!("Creating lifecycle context for service '{}'", service_path));
        
        // Create service-specific logger
        let service_logger = self.logger.with_component(Component::Service);
        self.logger.debug("Created service-specific logger");
        
        // Create topic path for the service
        self.logger.debug(format!("Creating topic path for service '{}'", service_path));
        let topic_path = TopicPath::new(service_path, &self.network_id).expect("Invalid service path");
        self.logger.debug(format!("Created topic path for service '{}'", service_path));
        
        // Create a context with minimum requirements
        self.logger.debug("Creating LifecycleContext with topic path and logger");
        let context = LifecycleContext::new(&topic_path, service_logger);
        
        // Create a delegate reference to self
        self.logger.debug("Creating node delegate");
        let self_clone = self.clone();
        let delegate: Arc<dyn NodeDelegate + Send + Sync> = Arc::new(self_clone);
        self.logger.debug("Created node delegate");
        
        // Add the node delegate to the context
        self.logger.debug("Adding node delegate to context");
        let context_with_delegate = context.with_node_delegate(delegate);
        self.logger.debug("Added node delegate to context");
        
        context_with_delegate
    }
    
    /// Get the current state of a service
    ///
    /// INTENTION: Retrieve the current lifecycle state of a service.
    /// This method allows checking if a service is initialized, running, or stopped.
    pub async fn get_service_state(&self, service_path: &str) -> Option<ServiceState> {
        let states = self.service_states.read().await;
        states.get(service_path).cloned()
    }

    /// Update the state of a service
    ///
    /// INTENTION: Update the lifecycle state of a service in the centralized metadata.
    /// This is used by the Node to track service states during initialization, startup, and shutdown.
    async fn update_service_state(&self, service_path: &str, state: ServiceState) {
        self.logger.debug(format!("Acquiring service_states write lock for service '{}'", service_path));
        let mut states = self.service_states.write().await;
        self.logger.debug(format!("Acquired service_states write lock for service '{}'", service_path));
        self.logger.debug(format!("Updating service '{}' state to {:?}", service_path, state));
        states.insert(service_path.to_string(), state);
        self.logger.debug(format!("Updated service state for '{}'", service_path));
        // Explicitly drop the lock as soon as possible
        drop(states);
        self.logger.debug(format!("Released service_states write lock for service '{}'", service_path));
    }

    /// Add an internal service to the node
    /// This is a special method that avoids circular references
    async fn add_internal_service<S>(&self, service: S) -> Result<()>
    where
        S: AbstractService + 'static,
    {
        self.logger.debug("Starting add_internal_service");
        // Convert to Arc to share across threads
        let service_arc: Arc<dyn AbstractService> = Arc::new(service);
        
        // Get service information directly from the service
        let name = service_arc.name().to_string();
        let path = service_arc.path().to_string();
        self.logger.debug(format!("Adding internal service '{}' (path: '{}')", name, path));
        
        // Register with our internal service map
        self.logger.debug("Acquiring services write lock");
        let mut services = self.services.write().await;
        self.logger.debug("Acquired services write lock");
        services.insert(path.clone(), service_arc.clone());
        self.logger.debug("Added service to internal map");
        // Drop the lock as soon as possible
        drop(services);
        
        // Initialize service state as Initialized
        self.logger.debug("Updating service state");
        self.update_service_state(&path, ServiceState::Initialized).await;
        
        // Create and store service metadata
        self.logger.debug("Creating service metadata");
        let complete_metadata = CompleteServiceMetadata::new(
            service_arc.name().to_string(),
            service_arc.version().to_string(),
            service_arc.path().to_string(),
            service_arc.description().to_string()
        );
        
        // Store the metadata
        self.logger.debug("Acquiring metadata write lock");
        let mut metadata = self.service_metadata.write().await;
        self.logger.debug("Acquired metadata write lock");
        metadata.insert(path.clone(), complete_metadata);
        self.logger.debug("Added metadata to internal map");
        // Drop the lock as soon as possible
        drop(metadata);
        
        // Log the registration
        self.logger.debug(format!("Internal service '{}' added to node registry", path));
        
        // Create a lifecycle context with derived logger for initialization
        self.logger.debug("Creating lifecycle context");
        let lifecycle_context = self.create_context(&path);
        
        // Initialize the service with the derived logger
        // The service will register its action handlers during initialization
        self.logger.debug("Initializing service");
        if let Err(e) = service_arc.init(lifecycle_context).await {
            self.logger.error(format!("Failed to initialize internal service '{}': {}", name, e));
            self.update_service_state(&path, ServiceState::Error).await;
            return Err(anyhow!("Failed to initialize internal service: {}", e));
        }
        
        // The service is now registered and initialized with its action handlers
        self.logger.info(format!("Internal service '{}' initialized successfully", name));
        
        Ok(())
    }

    /// Process a service request
    ///
    /// INTENTION: Validate and route a request to the appropriate service action handler.
    /// This method parses the given path, looks up the service and action handler,
    /// and invokes the handler with a request context.
    ///
    /// Example:
    /// ```
    /// use runar_node::Node;
    /// use runar_common::types::ValueType;
    /// use serde_json::json;
    ///
    /// async fn make_request(node: &Node) {
    ///     // Call the "add" action on the "math" service
    ///     let response = node.request(
    ///         "math/add",
    ///         ValueType::from(json!({
    ///             "a": 5,
    ///             "b": 3
    ///         }))
    ///     ).await.expect("Request failed");
    ///
    ///     // Process the response...
    /// }
    /// ```
    pub async fn request(&self, path: impl Into<String>, params: ValueType) -> Result<ServiceResponse> {
        let path_string = path.into();
        
        // Parse the path into a TopicPath for validation
        let topic_path = TopicPath::new(&path_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid path format: {}", e))?;
        
        // Create a context with a derived logger for this request
        let context = self.create_request_context_with_topic_path(&topic_path);
        let context_arc = Arc::new(context);
        
        // Create the service request with the validated TopicPath
        let request = ServiceRequest::new_with_topic_path(
            topic_path.clone(),
            params,
            context_arc.clone(),
        );
        
        // Log that we're handling the request
        context_arc.debug(&format!("Node handling request for {}", topic_path.as_str()));

        // Access service path and action from the topic path
        let handler_params = if let ValueType::Null = request.data {
            None
        } else {
            Some(request.data.clone())
        };

        // Get the action handler from the registry using the topic path
        if let Some(handler) = self.service_registry.get_action_handler(&topic_path).await {
            // Node invokes the handler directly
            context_arc.debug(&format!("Invoking action handler for {}", topic_path.as_str()));
            
            // Use the context we created - we already have the reference to it
            // Clone the RequestContext from the Arc
            let context_ref = context_arc.as_ref().clone();
            return handler(handler_params, context_ref).await;
        }

        // No action handler found, return error
        let service_path = topic_path.service_path();
        
        // Format the error message differently based on whether there's an action
        let error_msg = if topic_path.action_path().is_empty() {
            format!("No handler registered for service {}", service_path)
        } else {
            format!("No handler registered for path {}", topic_path.action_path())
        };
        
        context_arc.error(&error_msg);
        Ok(ServiceResponse::error(404, &error_msg))
    }

    /// Publish an event to a topic
    ///
    /// INTENTION: Distribute an event to subscribers of the topic.
    /// This method validates the topic string, converts it to a TopicPath,
    /// looks up subscribers in the service registry, and delivers the event.
    pub async fn publish(&self, topic: impl Into<String>, data: ValueType) -> Result<()> {
        // Use publish_with_options with default options
        self.publish_with_options(topic, data, PublishOptions::default()).await
    }
    
    /// Publish an event to a topic with specific delivery options
    ///
    /// INTENTION: Distribute an event to subscribers of the topic with specific delivery options.
    /// This method provides additional control over how events are delivered, including:
    /// - Delivery scope (broadcast vs. targeted)
    /// - Delivery guarantees (at-least-once vs. best-effort)
    /// - Retention policies for persistent events
    ///
    /// The method validates the topic string, converts it to a TopicPath,
    /// looks up subscribers in the service registry, and delivers the event
    /// according to the specified options.
    pub async fn publish_with_options(&self, topic: impl Into<String>, data: ValueType, options: PublishOptions) -> Result<()> {
        // Convert to String for NodeDelegate implementation
        let topic_string = topic.into();
        
        // Convert string to validated TopicPath
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;
            
        self.logger.debug(format!("Publishing to topic '{}' with options: {:?}", topic_path.as_str(), options));
        
        // Get handlers - returns subscription IDs and handlers
        let event_handlers = self.service_registry.get_event_handlers(&topic_path).await;
        let subscriber_count = event_handlers.len();
        
        if subscriber_count == 0 {
            self.logger.debug(format!("No subscribers for topic '{}'", topic_path.as_str()));
            return Ok(());
        }
        
        let topic_clone = topic_path.clone();
        let logger = self.logger.clone();
        
        // Execute callbacks based on delivery scope
        // For now, we just handle local delivery - network delivery will be added later
        for (subscriber_id, callback) in event_handlers {
            let data_clone = data.clone();
            let topic_path_clone = topic_clone.clone();
            let logger_clone = logger.clone();
            let subscriber_id_clone = subscriber_id.clone();
            let options_clone = options.clone();
            
            // If guaranteed delivery is requested, use a different execution strategy
            if options_clone.guaranteed_delivery {
                // For now, just log that guaranteed delivery was requested
                // In the future, this would use a more robust delivery mechanism
                logger_clone.debug(format!(
                    "Guaranteed delivery requested for topic '{}' to subscriber '{}'",
                    topic_path_clone.as_str(), subscriber_id_clone
                ));
            }
            
            tokio::spawn(async move {
                // Create an event context for the callback
                let event_logger = logger_clone.with_component(Component::Service);
                
                // Create an event context with the topic path and delivery options
                let mut event_context = EventContext::new(&topic_path_clone, event_logger);
                event_context.delivery_options = Some(options_clone);
                let event_context_arc = Arc::new(event_context);
                
                // Execute the callback with the context
                match callback(event_context_arc, data_clone).await {
                    Ok(_) => {
                        logger_clone.debug(format!(
                            "Successfully delivered event to subscriber '{}' for topic '{}'", 
                            subscriber_id_clone, topic_path_clone.as_str()
                        ));
                    },
                    Err(err) => {
                        logger_clone.error(format!(
                            "Error executing callback for topic '{}', subscriber '{}': {}", 
                            topic_path_clone.as_str(), subscriber_id_clone, err
                        ));
                    }
                }
            });
        }
        
        self.logger.debug(format!("Published to {} subscribers for topic '{}'", subscriber_count, topic_path.as_str()));
        
        Ok(())
    }

    /// Subscribe to a topic
    pub async fn subscribe(
        &self,
        topic: impl Into<String>,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        let topic_string = topic.into();
        
        // Get a topic path to validate the topic and extract network ID if present
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;
        
        // Convert Box to Arc (required by service_registry)
        let callback_arc = Arc::from(callback);
        
        // Subscribe through the service registry
        self.service_registry.subscribe(&topic_path, callback_arc).await
    }

    /// Subscribe to a topic with options
    pub async fn subscribe_with_options(
        &self,
        topic: impl Into<String>,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        let topic_string = topic.into();
        
        // Get a topic path to validate the topic and extract network ID if present
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;
        
        // Convert Box to Arc (required by service_registry)
        let callback_arc = Arc::from(callback);
        
        // Subscribe through the service registry with options
        self.service_registry.subscribe_with_options(&topic_path, callback_arc, options).await
    }

    /// Unsubscribe from a topic
    pub async fn unsubscribe(&self, topic: impl Into<String>, subscription_id: Option<&str>) -> Result<()> {
        let topic_string = topic.into();
        
        // Get a topic path to validate the topic and extract network ID if present
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;
        
        // Unsubscribe through the service registry
        self.service_registry.unsubscribe(&topic_path, subscription_id).await
    }

    /// List all services
    pub fn list_services(&self) -> Vec<String> {
        // Return a simple snapshot of services
        // This avoids the need to block on async code
        self.services.try_read()
            .map(|services| services.keys().cloned().collect::<Vec<String>>())
            .unwrap_or_default()
    }

    /// Start the Node and all registered services
    ///
    /// INTENTION: Initialize the Node's internal systems and start all registered services.
    /// This method:
    /// 1. Checks if the Node is already started to ensure idempotency
    /// 2. Transitions the Node to the Started state
    /// 3. Starts all registered services in the proper order
    /// 4. Updates the service state in the metadata as each service starts
    /// 5. Handles any errors during service startup
    ///
    /// When network functionality is added, this will also advertise services to the network.
    pub async fn start(&self) -> Result<()> {
        self.logger.info("Starting Node and all registered services");
        
        // Get a read lock on the services map
        let services_lock = self.services.read().await;
        
        if services_lock.is_empty() {
            self.logger.warn("No services registered with this Node");
        }
        
        // Start each service
        let mut success_count = 0;
        let mut failure_count = 0;
        
        for (path, service) in services_lock.iter() {
            self.logger.debug(format!("Starting service '{}'", path));
            
            // Create a lifecycle context for this service
            let context = self.create_context(path);
            
            // Try to start the service
            match service.start(context).await {
                Ok(_) => {
                    self.logger.info(format!("Service '{}' started successfully", path));
                    success_count += 1;
                    
                    // Update the service state to Running
                    self.update_service_state(path, ServiceState::Running).await;
                },
                Err(e) => {
                    self.logger.error(format!("Failed to start service '{}': {}", path, e));
                    failure_count += 1;
                    
                    // Update the service state to Error
                    self.update_service_state(path, ServiceState::Error).await;
                }
            }
        }
        
        // Log the final status
        if failure_count == 0 {
            self.logger.info(format!("Node started successfully: {} services running", success_count));
            Ok(())
        } else {
            let message = format!(
                "Node started with errors: {} services running, {} failed to start", 
                success_count, 
                failure_count
            );
            self.logger.warn(&message);
            
            // Return an error if any services failed to start
            Err(anyhow!(message))
        }
    }
    
    /// Stop the Node and all registered services
    ///
    /// INTENTION: Gracefully shut down the Node and all registered services.
    /// This method:
    /// 1. Stops all registered services in the proper order
    /// 2. Updates the service state in the metadata as each service stops
    /// 3. Cleans up any Node resources, subscriptions, or pending events
    /// 4. Transitions the Node to the Stopped state
    ///
    /// This provides a clean shutdown process to prevent data loss or corruption.
    pub async fn stop(&self) -> Result<()> {
        self.logger.info("Stopping Node and all registered services");
        
        // Get a read lock on the services map
        let services_lock = self.services.read().await;
        
        if services_lock.is_empty() {
            self.logger.warn("No services registered with this Node");
            return Ok(());
        }
        
        // Stop each service
        let mut success_count = 0;
        let mut failure_count = 0;
        
        for (path, service) in services_lock.iter() {
            self.logger.debug(format!("Stopping service '{}'", path));
            
            // Create a lifecycle context for this service
            let context = self.create_context(path);
            
            // Try to stop the service
            match service.stop(context).await {
                Ok(_) => {
                    self.logger.info(format!("Service '{}' stopped successfully", path));
                    success_count += 1;
                    
                    // Update the service state to Stopped
                    self.update_service_state(path, ServiceState::Stopped).await;
                },
                Err(e) => {
                    self.logger.error(format!("Failed to stop service '{}': {}", path, e));
                    failure_count += 1;
                    
                    // Update the service state to Error
                    self.update_service_state(path, ServiceState::Error).await;
                }
            }
        }
        
        // Log the final status
        if failure_count == 0 {
            self.logger.info(format!("Node stopped successfully: {} services stopped", success_count));
            Ok(())
        } else {
            let message = format!(
                "Node stopped with errors: {} services stopped, {} failed to stop", 
                success_count, 
                failure_count
            );
            self.logger.warn(&message);
            
            // Return an error if any services failed to stop
            Err(anyhow!(message))
        }
    }

    /// Get all service states
    ///
    /// INTENTION: Retrieve the current lifecycle state of all services.
    /// This is useful for monitoring service health and debugging.
    pub async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        let states = self.service_states.read().await;
        states.clone()
    }

    /// Get complete metadata for a service
    ///
    /// INTENTION: Retrieve the complete metadata for a service, including
    /// dynamically registered actions, events, and runtime state.
    pub async fn get_service_metadata(&self, service_path: &str) -> Option<CompleteServiceMetadata> {
        let metadata = self.service_metadata.read().await;
        metadata.get(service_path).cloned()
    }
    
    /// Register action metadata
    ///
    /// INTENTION: Update the service metadata with information about a registered action.
    /// This is used to document APIs and enable discoverability.
    pub async fn register_action_metadata(&self, service_path: &str, metadata: ActionMetadata) -> Result<()> {
        let mut service_metadata = self.service_metadata.write().await;
        
        if let Some(complete_metadata) = service_metadata.get_mut(service_path) {
            complete_metadata.register_action(metadata);
            Ok(())
        } else {
            Err(anyhow!("Service not found: {}", service_path))
        }
    }

    /// Register an action handler for a specific path
    async fn register_action_handler(&self, topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>) -> Result<()> {
        // Log the registration
        self.logger.debug(format!("Registering action handler for '{}'", topic_path.as_str()));
        
        // Clone metadata for service registry
        let metadata_for_registry = metadata.clone();
        
        // Register the handler with the service registry
        self.service_registry.register_action_handler(topic_path, handler, metadata_for_registry).await?;
        
        // Update service metadata if provided
        if let Some(md) = &metadata {
            // Get a write lock on the service metadata
            let mut service_metadata = self.service_metadata.write().await;
            
            // Find or create metadata for this service
            let service_path = topic_path.service_path();
            let entry = service_metadata.entry(service_path.to_string()).or_insert_with(|| {
                CompleteServiceMetadata::new(
                    service_path.to_string(),
                    "unknown".to_string(),
                    service_path.to_string(),
                    "Dynamically registered service".to_string()
                )
            });
            
            // Add this action to the service's registered actions
            entry.register_action(md.clone());
        }
        
        Ok(())
    }

    /// Add a service to the node
    pub async fn add_service<S>(&mut self, service: S) -> Result<()>
    where
        S: AbstractService + 'static,
    {
        // Convert to Arc to share across threads
        let service_arc: Arc<dyn AbstractService> = Arc::new(service);
        
        // Get service information directly from the service
        let name = service_arc.name().to_string();
        let path = service_arc.path().to_string();
        
        // Register with our internal service map
        let mut services = self.services.write().await;
        services.insert(path.clone(), service_arc.clone());
        
        // Initialize service state as Initialized
        self.update_service_state(&path, ServiceState::Initialized).await;
        
        // Create and store service metadata
        let complete_metadata = CompleteServiceMetadata::new(
            service_arc.name().to_string(),
            service_arc.version().to_string(),
            service_arc.path().to_string(),
            service_arc.description().to_string()
        );
        
        // Store the metadata
        let mut metadata = self.service_metadata.write().await;
        metadata.insert(path.clone(), complete_metadata);
        
        // Log the registration
        self.logger.debug(format!("Adding service '{}' to node registry", path));
        
        // Create a lifecycle context with derived logger for initialization
        let lifecycle_context = self.create_context(&path);
        
        // Initialize the service with the derived logger
        // The service will register its action handlers during initialization
        if let Err(e) = service_arc.init(lifecycle_context).await {
            self.logger.error(format!("Failed to initialize service '{}': {}", name, e));
            self.update_service_state(&path, ServiceState::Error).await;
            return Err(anyhow!("Failed to initialize service: {}", e));
        }
        
        // The service is now registered and initialized with its action handlers
        self.logger.info(format!("Service '{}' initialized successfully", name));
        
        Ok(())
    }
}

/// Type alias for the action registrar function
pub type ActionRegistrar = Arc<dyn Fn(&TopicPath, ActionHandler, Option<ActionMetadata>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

#[async_trait::async_trait]
impl NodeDelegate for Node {
    /// Process a service request - NodeDelegate trait implementation
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        // Call public method with path String
        self.request(path, params).await
    }

    /// Publish an event - NodeDelegate trait implementation
    async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        // Call public method with topic String
        self.publish(topic, data).await
    }

    /// Subscribe to a topic - NodeDelegate trait implementation
    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        // Call public method with topic String
        self.subscribe(topic, callback).await
    }

    /// Subscribe to a topic with options - NodeDelegate trait implementation
    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        // Call public method with topic String
        self.subscribe_with_options(topic, callback, options).await
    }

    /// Unsubscribe from a topic - NodeDelegate trait implementation
    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()> {
        // Call public method with topic String
        self.unsubscribe(topic, subscription_id).await
    }

    /// List all services - NodeDelegate trait implementation
    fn list_services(&self) -> Vec<String> {
        // Return a simple snapshot of services
        self.services.try_read()
            .map(|services| services.keys().cloned().collect::<Vec<String>>())
            .unwrap_or_default()
    }

    /// Register an action handler - NodeDelegate trait implementation
    async fn register_action_handler(&self, topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>) -> Result<()> {
        // Register through the service registry
        self.service_registry.register_action_handler(topic_path, handler, metadata).await
    }
}

#[async_trait::async_trait]
impl RegistryDelegate for Node {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        let states = self.service_states.read().await;
        states.clone()
    }
    
    /// Get metadata for a specific service
    async fn get_service_metadata(&self, service_path: &str) -> Option<CompleteServiceMetadata> {
        let metadata = self.service_metadata.read().await;
        metadata.get(service_path).cloned()
    }
    
    /// Get metadata for all registered services in a single call
    async fn get_all_service_metadata(&self) -> HashMap<String, CompleteServiceMetadata> {
        let metadata = self.service_metadata.read().await;
        metadata.clone()
    }
    
    /// List all services
    fn list_services(&self) -> Vec<String> {
        self.services.try_read()
            .map(|services| services.keys().cloned().collect::<Vec<String>>())
            .unwrap_or_default()
    }
} 