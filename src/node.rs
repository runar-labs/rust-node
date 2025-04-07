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
use std::time::{SystemTime, UNIX_EPOCH};
use std::net::SocketAddr;
use async_trait::async_trait;

use crate::routing::TopicPath;
use crate::services::{
    ActionHandler, EventContext, LifecycleContext, NodeDelegate, PublishOptions,
    RequestContext, ServiceResponse, SubscriptionOptions, RegistryDelegate,
};
use crate::services::abstract_service::{ActionMetadata, AbstractService, CompleteServiceMetadata, ServiceState};
use crate::services::registry_info::RegistryService;
use crate::services::service_registry::ServiceRegistry;
use runar_common::types::ValueType;
use runar_common::{Component, Logger};

// Network-related imports
use crate::network::transport::{NetworkTransport, NetworkMessage, NodeIdentifier, TransportFactory};
use crate::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};

/// Configuration for a Node
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Network ID for the node
    pub network_id: String,
    /// Node ID for logging and identification
    pub node_id: Option<String>,
    /// Network binding address for the node (optional)
    pub network_bind_address: Option<SocketAddr>,
    /// Whether to enable network functionality
    pub enable_networking: bool,
}

impl NodeConfig {
    /// Create a new NodeConfig with minimal required parameters
    pub fn new(network_id: &str) -> Self {
        Self {
            network_id: network_id.to_string(),
            node_id: None,
            network_bind_address: None,
            enable_networking: false,
        }
    }

    /// Create a new NodeConfig with a specific node ID
    pub fn new_with_node_id(network_id: &str, node_id: &str) -> Self {
        Self {
            network_id: network_id.to_string(),
            node_id: Some(node_id.to_string()),
            network_bind_address: None,
            enable_networking: false,
        }
    }

    /// Set the network binding address
    pub fn with_network_bind_address(mut self, addr: SocketAddr) -> Self {
        self.network_bind_address = Some(addr);
        self
    }

    /// Enable network functionality
    pub fn with_networking_enabled(mut self, enabled: bool) -> Self {
        self.enable_networking = enabled;
        self
    }
}

/// Node represents a Runar node that can host services and communicate with other nodes
#[derive(Clone)]
pub struct Node {
    /// Network ID for this node
    pub network_id: String,

    /// Node ID for distinguishing nodes within a network
    pub node_id: String,

    /// Logger instance with node context
    logger: Logger,

    /// Service registry for managing services and action handlers
    service_registry: Arc<ServiceRegistry>,

    /// All registered services
    services: Arc<RwLock<HashMap<String, Arc<dyn AbstractService>>>>,
    
    /// Service states
    service_states: Arc<RwLock<HashMap<String, ServiceState>>>,
    
    /// Network transport for communication with other nodes
    network_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,

    /// Node discovery mechanism (if enabled)
    node_discovery: Arc<RwLock<Option<Box<dyn NodeDiscovery>>>>,

    /// Registry of discovered nodes
    discovered_nodes: Arc<RwLock<HashMap<String, NodeInfo>>>,

    /// Whether network functionality is enabled
    networking_enabled: bool,

    /// Pending network requests awaiting responses
    pending_requests: Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<NetworkMessage>>>>,
}

// Implementation for Node
impl Node {
    /// Create a new Node with the given configuration
    ///
    /// INTENTION: Initialize a new Node with the specified configuration, setting up
    /// all the necessary components and internal state. This is the primary
    /// entry point for creating a Node instance.
    ///
    /// This constructor does not start services - call start() separately
    /// after registering services.
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let node_id = config.node_id.unwrap_or_else(|| Uuid::new_v4().to_string()[..8].to_string());
        let logger = Logger::new_root(Component::Node, &node_id);
        
        logger.info(format!("Initializing node '{}' in network '{}'...", node_id, config.network_id));
        
        let service_registry = Arc::new(ServiceRegistry::new(logger.clone()));
        
        // Create the node (with network fields now included)
        let mut node = Self {
            network_id: config.network_id,
            node_id,
            logger: logger.clone(),
            service_registry,
            services: Arc::new(RwLock::new(HashMap::new())),
            service_states: Arc::new(RwLock::new(HashMap::new())),
            network_transport: Arc::new(RwLock::new(None)),
            node_discovery: Arc::new(RwLock::new(None)),
            discovered_nodes: Arc::new(RwLock::new(HashMap::new())),
            networking_enabled: config.enable_networking,
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
        };
        
        // Register the registry service
        let registry_service = RegistryService::new(
            logger.clone(),
            Arc::new(node.clone()) as Arc<dyn RegistryDelegate>,
        );
        
        // Add the registry service to the node
        node.add_service(registry_service).await?;
        
        Ok(node)
    }
    
    /// Add a service to this node
    ///
    /// INTENTION: Register a service with this node, making its actions available
    /// for requests and allowing it to receive events. This method initializes the
    /// service but does not start it - services are started when the node is started.
    pub async fn add_service<S: AbstractService + 'static>(&mut self, service: S) -> Result<()> {
        let service_path = service.path().to_string();
        
        // Use logger method
        self.logger.info(format!("Adding service '{}' to node", service_path));
        
        // Store the service instance
        self.services.write().await.insert(service_path.clone(), Arc::new(service));
        
        // Set the initial state to Created
        self.service_states.write().await.insert(service_path.clone(), ServiceState::Created);
        
        Ok(())
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
    pub async fn start(&mut self) -> Result<()> {
        self.logger.info("Starting node".to_string());
        
        // Get service instances
        let services = {
            self.services.read().await.clone()
        };

        // Initialize all services first
        for (service_name, service) in &services {
            self.logger.info(format!("Initializing service: {}", service_name));
            
            // Create a lifecycle context for the service
            let context = self.create_lifecycle_context(service.as_ref()).await?;
            
            // Initialize the service
            match service.init(context).await {
                Ok(_) => {
                    self.logger.info(format!("Service '{}' initialized successfully", service_name));
                    // Update state to Initialized
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Initialized);
                },
                Err(e) => {
                    self.logger.error(format!("Failed to initialize service '{}': {}", service_name, e));
                    // Update state to Error
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Error);
                    return Err(anyhow!("Failed to initialize service '{}': {}", service_name, e));
                }
            }
        }
        
        // Now start all services in the correct order
        for (service_name, service) in &services {
            self.logger.info(format!("Starting service: {}", service_name));
            
            // Create a lifecycle context for the service
            let context = self.create_lifecycle_context(service.as_ref()).await?;
            
            // Start the service
            match service.start(context).await {
                Ok(_) => {
                    self.logger.info(format!("Service '{}' started successfully", service_name));
                    // Update state to Running
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Running);
                },
                Err(e) => {
                    self.logger.error(format!("Failed to start service '{}': {}", service_name, e));
                    // Update state to Error
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Error);
                    return Err(anyhow!("Failed to start service '{}': {}", service_name, e));
                }
            }
        }
        
        self.logger.info("Node started successfully".to_string());
        
        Ok(())
    }
    
    /// Stop the Node and all registered services
    ///
    /// INTENTION: Gracefully stop the Node and all registered services. This method:
    /// 1. Transitions the Node to the Stopping state
    /// 2. Stops all registered services in the reverse order they were started
    /// 3. Updates the service state in the metadata as each service stops
    /// 4. Handles any errors during service shutdown
    /// 5. Transitions the Node to the Stopped state
    pub async fn stop(&mut self) -> Result<()> {
        self.logger.info("Stopping node".to_string());
        
        // Get service instances in reverse order (for proper shutdown sequence)
        let mut services: Vec<(String, Arc<dyn AbstractService>)> = {
            self.services.read().await
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        
        services.reverse();

        // Stop all services in reverse order
        for (service_name, service) in services {
            self.logger.info(format!("Stopping service: {}", service_name));
            
            // Create a lifecycle context for the service
            let context = self.create_lifecycle_context(service.as_ref()).await?;
            
            // Stop the service
            match service.stop(context).await {
                Ok(_) => {
                    self.logger.info(format!("Service '{}' stopped successfully", service_name));
                    // Update state to Stopped
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Stopped);
                },
                Err(e) => {
                    self.logger.error(format!("Error stopping service '{}': {}", service_name, e));
                    // Update state to Error
                    self.service_states.write().await.insert(service_name.clone(), ServiceState::Error);
                    // Continue stopping other services even if one fails
                }
            }
        }
        
        // Stop network components if enabled
        if self.networking_enabled {
            // Shutdown transport
            if let Some(transport) = &*self.network_transport.read().await {
                match transport.shutdown().await {
                    Ok(_) => self.logger.info("Network transport shut down successfully".to_string()),
                    Err(e) => self.logger.error(format!("Error shutting down network transport: {}", e)),
                }
            }
            
            // Shutdown discovery
            if let Some(discovery) = &*self.node_discovery.read().await {
                match discovery.shutdown().await {
                    Ok(_) => self.logger.info("Node discovery shut down successfully".to_string()),
                    Err(e) => self.logger.error(format!("Error shutting down node discovery: {}", e)),
                }
            }
        }
        
        self.logger.info("Node stopped successfully".to_string());
        
        Ok(())
    }

    /// Starts the networking components (transport and discovery).
    /// This should be called internally as part of the node.start process.
    async fn start_networking<TF, TD>(&self, transport_factory: TF, discovery_factory: TD) -> Result<()> 
    where
        TF: TransportFactory + Send + Sync + 'static,
        TD: FnOnce(Arc<RwLock<Option<Box<dyn NetworkTransport>>>>, Logger) -> Result<Box<dyn NodeDiscovery>> + Send + Sync + 'static,
    {
        if !self.networking_enabled {
            self.logger.info("Networking is disabled, skipping network start.".to_string());
            return Ok(());
        }

        self.logger.info("Starting networking...".to_string());
        
        // Create node identifier
        let node_identifier = NodeIdentifier::new(self.network_id.clone(), self.node_id.clone());
        
        // Create transport as child of node logger
        let transport_logger = self.logger.with_component(Component::Network);
        let transport_instance = transport_factory.create_transport(node_identifier, transport_logger).await?;
        
        // Initialize transport
        transport_instance.init().await?;
        
        // Register message handler
        let self_arc = self.clone();
        let message_handler = Box::new(move |message: NetworkMessage| {
            let node_clone = self_arc.clone();
            tokio::spawn(async move {
                if let Err(e) = node_clone.handle_network_message(message).await {
                    node_clone.logger.error(format!("Error handling network message: {}", e));
                }
            });
            Ok(())
        });
        transport_instance.register_handler(message_handler)?;

        // Box and store the transport
        *self.network_transport.write().await = Some(Box::new(transport_instance));

        // Start discovery with a child logger of the node logger
        let discovery_logger = self.logger.with_component(Component::Network);
        let discovery_instance = discovery_factory(self.network_transport.clone(), discovery_logger)?;
        
        // Store the discovery instance
        *self.node_discovery.write().await = Some(discovery_instance);

        // Initialize the discovery with options
        if let Some(discovery) = &*self.node_discovery.read().await {
            // Initialize discovery
            discovery.init(DiscoveryOptions::default()).await?;
            
            // Register this node with discovery
            let node_info = NodeInfo {
                identifier: NodeIdentifier::new(self.network_id.clone(), self.node_id.clone()),
                address: "unknown".to_string(), // This would need to be obtained from transport
                capabilities: self.list_services().await, // Use service list as capabilities
                last_seen: std::time::SystemTime::now(),
            };
            
            discovery.register_node(node_info.clone()).await?;
            
            // Set listener and start announcing
            let self_clone = self.clone();
            let discovery_listener: DiscoveryListener = Box::new(move |node_info: NodeInfo| {
                let node = self_clone.clone();
                tokio::spawn(async move {
                    if let Err(e) = node.handle_discovered_node(node_info).await {
                        node.logger.error(format!("Error handling discovered node: {}", e));
                    }
                });
            });
            
            discovery.set_discovery_listener(discovery_listener).await?;
            discovery.start_announcing(node_info).await?;
        }
        
        // Log address info
        let transport_guard = self.network_transport.read().await;
        if let Some(transport) = &*transport_guard {
            if let Some(addr) = transport.local_address().await {
                self.logger.info(format!("Network transport listening on: {}", addr));
            } else {
                self.logger.warn("Network transport address not available");
            }
        }
        
        self.logger.info("Networking started successfully");
        Ok(())
    }

    /// Create a lifecycle context for a service
    ///
    /// INTENTION: Build a context object for service lifecycle operations.
    /// This is used for service initialization, starting, and stopping.
    async fn create_lifecycle_context(&self, service: &dyn AbstractService) -> Result<LifecycleContext> {
        // Get the service path from the service
        let service_path = service.path().to_string();
        
        // Create a logger with the service context
        let logger = Logger::new_root(Component::Service, &service_path);
        
        // Create a context with the network ID, service path, and logger
        let topic_path = TopicPath::new(&service_path, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic path: {}", e))?;
            
        let context = LifecycleContext::new(&topic_path, logger)
            .with_node_delegate(Arc::new(self.clone()) as Arc<dyn NodeDelegate + Send + Sync>);
        
        Ok(context)
    }

    /// Send a request to a remote node and wait for a response
    ///
    /// INTENTION: Provide a way to make requests to remote nodes and handle the response.
    /// This is used by the Node when handling requests that need to be forwarded to remote services.
    pub async fn network_request(
        &self,
        destination: NodeIdentifier,
        topic: String,
        params: ValueType
    ) -> Result<NetworkMessage> {
        if !self.networking_enabled {
            return Err(anyhow!("Network functionality is disabled"));
        }
        
        // Generate a correlation ID for this request
        let correlation_id = uuid::Uuid::new_v4().to_string();
        
        // Create a channel for the response
        let (sender, receiver) = tokio::sync::oneshot::channel();
        
        // Store the sender in the pending requests map
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(correlation_id.clone(), sender);
        }
        
        // Create the network message
        let message = NetworkMessage {
            source: NodeIdentifier::new(self.network_id.clone(), self.node_id.clone()),
            destination: Some(destination.clone()),
            message_type: "Request".to_string(),
            correlation_id: Some(correlation_id.clone()),
            topic: topic.clone(),
            params,
            payload: ValueType::Null,
        };
        
        self.logger.info(format!("Sending network request to {}: {}", destination, topic));
        
        // Send the message
        if let Some(transport) = &*self.network_transport.read().await {
            transport.send(message).await?;
        } else {
            return Err(anyhow!("Network transport not available"));
        }
        
        // Wait for the response with a timeout
        match tokio::time::timeout(
            std::time::Duration::from_secs(30), // 30 second timeout
            receiver
        ).await {
            Ok(result) => {
                match result {
                    Ok(response) => Ok(response),
                    Err(_) => {
                        // The sender was dropped without sending a value
                        self.logger.error(format!("Request channel closed without response: {}", correlation_id));
                        Err(anyhow!("Request channel closed without response"))
                    }
                }
            },
            Err(_) => {
                // Timeout occurred
                // Remove the pending request since we're no longer waiting for it
                {
                    let mut pending = self.pending_requests.write().await;
                    pending.remove(&correlation_id);
                }
                
                self.logger.error(format!("Request timed out: {}", correlation_id));
                Err(anyhow!("Request timed out"))
            }
        }
    }

    // Update the request method to use network_request when needed
    pub async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        self.logger.debug(format!("Processing request to path '{}'", path));
        
        // Convert string to validated TopicPath
        let topic_path = TopicPath::new(&path, &self.network_id)
            .map_err(|e| anyhow!("Invalid path format: {}", e))?;
            
        // Get action handler (if available locally)
        let handler_option = self.service_registry.get_action_handler(&topic_path).await;
        
        // First try to handle the request locally
        if let Some(handler) = handler_option {
            // Create request context - using topic_path.action_path() instead of to_string()
            let request_context = RequestContext::new(
                &topic_path,
                Logger::new_root(Component::Service, &topic_path.action_path())
            );
            
            // Call the handler with the correct argument types
            return handler(Some(params), request_context).await;
        }
        
        // If we couldn't handle it locally and networking is enabled, try to forward it
        if self.networking_enabled {
            self.logger.debug(format!("No local handler found for '{}', checking for remote services", path));
            
            // Extract service path from topic path
            let service_path = topic_path.service_path();
            
            // Check if we have remote services registered for this service path
            let remote_services = self.service_registry.get_remote_services(&service_path).await;
            
            if !remote_services.is_empty() {
                // For now, just use the first available remote service
                // In the future, we could implement more sophisticated selection
                let remote_service = &remote_services[0];
                
                self.logger.info(format!("Forwarding request '{}' to remote service on node {}", 
                    path, remote_service.peer_id()));
                
                // Extract action name from topic path
                let action_name = match topic_path.get_segments().last() {
                    Some(segment) => segment.clone(),
                    None => return Err(anyhow!("Invalid action path: {}", path)),
                };
                
                // Create a full topic path for the remote action
                let remote_topic = format!("{}/{}", service_path, action_name);
                
                // Send request to remote node and wait for response
                let network_response = match self.network_request(
                    remote_service.peer_id().clone(),
                    remote_topic,
                    params
                ).await {
                    Ok(response) => response,
                    Err(e) => return Ok(ServiceResponse::error(500, format!("Failed to forward request: {}", e))),
                };
                
                // Convert network response to ServiceResponse
                if network_response.message_type == "Response" {
                    if let ValueType::Map(map) = &network_response.payload {
                        if let Some(error) = map.get("error") {
                            if let Some(true) = error.as_bool() {
                                // Error response
                                let error_message = map.get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("Unknown error").to_string();
                                
                                return Ok(ServiceResponse::error(400, error_message));
                            }
                        }
                        
                        // Success response
                        let data = map.get("data")
                            .unwrap_or(&ValueType::Null);
                        
                        return Ok(ServiceResponse::ok(data.clone()));
                    }
                    
                    // If not a map, just return the payload as is
                    return Ok(ServiceResponse::ok(network_response.payload));
                }
                
                // Unexpected response type
                return Ok(ServiceResponse::error(500, 
                    format!("Unexpected response type: {}", network_response.message_type)));
            } else {
                self.logger.warn(format!("No remote services found for path '{}'", path));
            }
        }
        
        // If we get here, we couldn't handle the request locally or remotely
        Err(anyhow!("No handler found for path: {}", path))
    }

    /// Create a context for node operations
    ///
    /// INTENTION: Build a context object for a specific service. This can be used
    /// to create logger instances and to pass node information to services.
    pub fn create_context(&self, service_path: &str) -> Arc<LifecycleContext> {
        // Create a logger with the service context
        let logger = Logger::new_root(Component::Service, service_path);
        
        // Create a topic path (unwrap is safe here since we're not validating)
        let topic_path = TopicPath::new(service_path, &self.network_id)
            .unwrap_or_else(|_| TopicPath::new("error", &self.network_id).unwrap());
            
        // Create the context with node delegate
        let context = LifecycleContext::new(&topic_path, logger)
            .with_node_delegate(Arc::new(self.clone()) as Arc<dyn NodeDelegate + Send + Sync>);
            
        Arc::new(context)
    }

    /// List all registered services
    ///
    /// INTENTION: Provide a list of all registered services for introspection
    /// and management purposes. This is useful for service discovery and monitoring.
    pub async fn list_services(&self) -> Vec<String> {
        // Get the keys from the services map directly
        self.services.read().await.keys().cloned().collect()
    }

    /// Synchronous version of list_services for RegistryDelegate trait
    /// 
    pub fn list_services_sync(&self) -> Vec<String> {
        // For backward compatibility with the trait
        tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                self.list_services().await
            })
        })
    }

    /// Subscribe to an event topic
    ///
    /// INTENTION: Register a callback to be invoked when events are published to
    /// the specified topic. This supports the publish/subscribe pattern for event-driven
    /// communication between services.
    pub async fn subscribe<F, Fut>(&self, topic: String, callback: F) -> Result<String>
    where
        F: Fn(Arc<EventContext>, ValueType) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.logger.debug(format!("Subscribing to topic '{}'", topic));
        
        // Use subscribe_with_options with default options
        self.subscribe_with_options(topic, callback, SubscriptionOptions::default()).await
    }

    /// Subscribe to an event topic with options
    ///
    /// INTENTION: Register a callback to be invoked when events are published to
    /// the specified topic, with additional subscription options. This provides
    /// more control over how subscriptions are handled.
    pub async fn subscribe_with_options<F, Fut>(
        &self,
        topic: String,
        callback: F,
        options: SubscriptionOptions,
    ) -> Result<String>
    where
        F: Fn(Arc<EventContext>, ValueType) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.logger.debug(format!("Subscribing to topic '{}' with options: {:?}", topic, options));
        
        // Create a unique subscription ID
        let subscription_id = Uuid::new_v4().to_string();
        
        // Convert to validated TopicPath
        let topic_path = TopicPath::new(&topic, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;
            
        // Convert the callback to the expected type (a boxed future)
        let event_callback: Arc<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync> =
            Arc::new(move |ctx, data| {
                let future = callback(ctx, data);
                Box::pin(future)
            });
            
        // Register subscription with the service registry
        self.service_registry.subscribe_with_options(&topic_path, event_callback, options).await
    }

    /// Unsubscribe from an event topic
    ///
    /// INTENTION: Remove a subscription based on its ID. This allows services to
    /// stop receiving events when they're no longer interested.
    pub async fn unsubscribe(&self, subscription_id: String) -> Result<()> {
        self.logger.debug(format!("Unsubscribing from subscription ID '{}'", subscription_id));
        
        // At this point we don't have the topic path, but the service registry needs it
        // This is a limitation of our current API - we would need to maintain a mapping of 
        // subscription IDs to topics to properly implement this
        // 
        // For now, let's issue a warning and return Ok
        self.logger.warn(format!("Cannot unsubscribe '{}' without topic path - API limitation", subscription_id));
        
        Ok(())
    }

    /// Publish an event to a topic
    ///
    /// INTENTION: Publish an event to all subscribers of the specified topic. This
    /// is the mechanism for event-driven communication between services.
    pub async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        // Use publish_with_options with default options
        self.publish_with_options(topic, data, PublishOptions::default()).await
    }

    /// Publish an event to a topic with options
    ///
    /// INTENTION: Publish an event to all subscribers of the specified topic, with
    /// additional publishing options. This provides more control over how events
    /// are delivered.
    pub async fn publish_with_options(
        &self,
        topic: String,
        data: ValueType,
        options: PublishOptions,
    ) -> Result<()> {
        self.logger.debug(format!("Publishing to topic '{}'", topic));
        
        // Convert to validated TopicPath
        let topic_path = TopicPath::new(&topic, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic format: {}", e))?;

        // Create event context - using topic_path.action_path() instead of to_string()
        let event_context = Arc::new(EventContext::new(
            &topic_path,
            Logger::new_root(Component::Service, &topic_path.action_path()),
        ));

        // Get subscribers from service registry
        let subscribers = self.service_registry.get_event_handlers(&topic_path).await;
        if subscribers.is_empty() && topic_path.network_id() == self.network_id {
            self.logger.debug(format!("No subscribers found for topic '{}'", topic));
        }

        // Call each subscriber
        for (subscription_id, callback) in subscribers {
            let callback_context = event_context.clone();
            let callback_data = data.clone();
            
            // Execute the callback
            tokio::spawn(async move {
                if let Err(e) = callback(callback_context, callback_data).await {
                    // For now, just log the error - we'll need better error handling
                    println!("Error in subscriber callback for subscription {}: {}", subscription_id, e);
                }
            });
        }
        
        // If networking is enabled, publish to the network
        if self.networking_enabled {
            // Network publishing will be implemented here
            self.logger.warn("Network event publishing not yet implemented");
        }
        
        Ok(())
    }

    /// Handle a discovered node
    async fn handle_discovered_node(&self, node_info: NodeInfo) -> Result<()> {
        let node_id = node_info.identifier.to_string();
        self.logger.info(format!("Discovered node: {}", node_id));
        
        // Store the discovered node
        self.discovered_nodes.write().await.insert(node_id, node_info);
        
        Ok(())
    }

    /// Handle a network message
    async fn handle_network_message(&self, message: NetworkMessage) -> Result<()> {
        self.logger.debug(format!("Received network message: {:?}", message));
        
        // Match on message type
        match message.message_type.as_str() {
            "Request" => self.handle_network_request(message).await,
            "Response" => self.handle_network_response(message).await,
            "Event" => self.handle_network_event(message).await,
            "Discovery" => self.handle_network_discovery(message).await,
            _ => {
                self.logger.warn(format!("Unknown message type: {}", message.message_type));
                Ok(())
            }
        }
    }

    /// Handle a network request
    async fn handle_network_request(&self, message: NetworkMessage) -> Result<()> {
        self.logger.info(format!("Handling network request from {}", message.source));
        
        // Extract topic and params directly from the message
        let topic = message.topic.clone();
        let params = message.params.clone();
        
        // Process the request locally
        self.logger.debug(format!("Processing network request for topic: {}", topic));
        match self.request(topic.clone(), params).await {
            Ok(response) => {
                // Create response message with proper conversion to ValueType
                let response_payload = ValueType::Map(vec![
                    ("success".to_string(), ValueType::Bool(true)),
                    ("data".to_string(), response.data.unwrap_or(ValueType::Null)),
                ].into_iter().collect());
                
                let response_message = NetworkMessage {
                    source: NodeIdentifier::new(self.network_id.clone(), self.node_id.clone()),
                    destination: Some(message.source.clone()),
                    message_type: "Response".to_string(),
                    correlation_id: message.correlation_id.clone(),
                    topic: topic.clone(),
                    params: ValueType::Null,
                    payload: response_payload,
                };
                
                // Send response back
                if let Some(transport) = &*self.network_transport.read().await {
                    transport.send(response_message).await?;
                } else {
                    return Err(anyhow!("Network transport not available"));
                }
            },
            Err(e) => {
                // Create error response
                let response_message = NetworkMessage {
                    source: NodeIdentifier::new(self.network_id.clone(), self.node_id.clone()),
                    destination: Some(message.source.clone()),
                    message_type: "Response".to_string(),
                    correlation_id: message.correlation_id.clone(), 
                    topic: topic.clone(),
                    params: ValueType::Null,
                    payload: ValueType::Map(HashMap::from([
                        ("error".to_string(), ValueType::Bool(true)),
                        ("message".to_string(), ValueType::String(e.to_string())),
                    ])),
                };
                
                // Send error response back
                if let Some(transport) = &*self.network_transport.read().await {
                    transport.send(response_message).await?;
                } else {
                    return Err(anyhow!("Network transport not available"));
                }
            }
        }
        
        Ok(())
    }

    /// Handle a network response
    async fn handle_network_response(&self, message: NetworkMessage) -> Result<()> {
        let correlation_id = message.correlation_id.clone()
            .ok_or_else(|| anyhow!("Missing correlation_id in network response"))?;
        
        self.logger.info(format!("Handling network response for request {}", correlation_id));
        
        // Look up the pending request by correlation ID
        let sender = {
            let mut pending = self.pending_requests.write().await;
            pending.remove(&correlation_id)
        };
        
        // If we found a pending request, resolve it with the response
        if let Some(sender) = sender {
            if sender.send(message.clone()).is_err() {
                self.logger.warn(format!("Failed to send response to pending request {}: receiver dropped", correlation_id));
            } else {
                self.logger.debug(format!("Successfully sent response to pending request {}", correlation_id));
            }
        } else {
            self.logger.warn(format!("Received response for unknown correlation ID: {}", correlation_id));
        }
        
        Ok(())
    }

    /// Handle a network event
    async fn handle_network_event(&self, message: NetworkMessage) -> Result<()> {
        self.logger.info(format!("Handling network event from {}", message.source));
        
        // Extract topic directly from the message
        let topic = message.topic.clone();
        
        // Extract event data from params
        let data = message.params.clone();
        
        // Process the event locally by publishing to local subscribers
        self.logger.debug(format!("Publishing network event to local subscribers: {}", topic));
        
        // Create a publish options that prevents re-broadcasting
        let options = PublishOptions {
            broadcast: false,
            guaranteed_delivery: false,
            retention_seconds: None,
            target: None,
        };
        
        // Publish to local subscribers only (to prevent loops)
        self.publish_with_options(topic, data, options).await?;
        
        Ok(())
    }

    /// Handle a network discovery message
    async fn handle_network_discovery(&self, message: NetworkMessage) -> Result<()> {
        self.logger.debug("Received discovery message");
        
        // This should be handled by the discovery system
        Ok(())
    }
}

#[async_trait]
impl NodeDelegate for Node {
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        self.request(path, params).await
    }

    async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        self.publish(topic, data).await
    }

    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        // Create an Arc around the callback so we can pass ownership to the closure
        let callback_arc = Arc::new(callback);
        
        // Pass a new closure that captures Arc<callback>
        self.subscribe(topic, move |ctx, data| {
            let cb = Arc::clone(&callback_arc);
            Box::pin(async move {
                (*cb)(ctx, data).await
            })
        }).await
    }

    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        // Create an Arc around the callback so we can pass ownership to the closure
        let callback_arc = Arc::new(callback);
        
        // Pass a new closure that captures Arc<callback>
        self.subscribe_with_options(topic, move |ctx, data| {
            let cb = Arc::clone(&callback_arc);
            Box::pin(async move {
                (*cb)(ctx, data).await
            })
        }, options).await
    }

    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()> {
        // Adapt to our API
        if let Some(id) = subscription_id {
            self.unsubscribe(id.to_string()).await
        } else {
            Err(anyhow!("Subscription ID is required"))
        }
    }

    fn list_services(&self) -> Vec<String> {
        self.list_services_sync()
    }
    
    async fn register_action_handler(&self, topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>) -> Result<()> {
        self.service_registry.register_action_handler(topic_path, handler, metadata).await
    }
}

#[async_trait]
impl RegistryDelegate for Node {
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        // Return a clone of the service states map
        self.service_states.read().await.clone()
    }
    
    async fn get_service_metadata(&self, service_path: &str) -> Option<CompleteServiceMetadata> {
        let services = self.services.read().await;
        let states = self.service_states.read().await;
        
        services.get(service_path).map(|service| {
            // Create a minimal CompleteServiceMetadata
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
                
            let state = states.get(service_path).copied().unwrap_or(ServiceState::Created);
                
            CompleteServiceMetadata {
                name: service.name().to_string(),
                path: service_path.to_string(),
                current_state: state, // Use tracked state
                version: service.version().to_string(),
                description: service.description().to_string(),
                registered_actions: HashMap::new(),
                registered_events: HashMap::new(),
                registration_time: now,
                last_start_time: None,
            }
        })
    }
    
    async fn get_all_service_metadata(&self) -> HashMap<String, CompleteServiceMetadata> {
        let mut metadata = HashMap::new();
        let services = self.services.read().await;
        let states = self.service_states.read().await;
        
        for (path, service) in services.iter() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
                
            let state = states.get(path).copied().unwrap_or(ServiceState::Created);
                
            metadata.insert(path.clone(), CompleteServiceMetadata {
                name: service.name().to_string(),
                path: path.clone(),
                current_state: state, // Use tracked state
                version: service.version().to_string(),
                description: service.description().to_string(),
                registered_actions: HashMap::new(),
                registered_events: HashMap::new(),
                registration_time: now,
                last_start_time: None,
            });
        }
        metadata
    }
    
    fn list_services(&self) -> Vec<String> {
        self.list_services_sync()
    }
}

/// Implementation of discovery listener that forwards node information to Node
struct NodeDiscoveryListener {
    logger: Logger,
    transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,
}

impl NodeDiscoveryListener {
    fn new(logger: Logger, transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>) -> Self {
        Self { logger, transport }
    }
    
    async fn handle_discovered_node(&self, node_info: NodeInfo) -> Result<()> {
        self.logger.info(format!("Discovery listener found node: {}", node_info.identifier));
        
        // Attempt to connect via the transport
        if let Some(transport) = &*self.transport.read().await {
            transport.connect(&node_info).await?;
            self.logger.info(format!("Connected to discovered node: {}", node_info.identifier));
        }
        
        Ok(())
    }
} 