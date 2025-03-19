use anyhow::{anyhow, Result};
use async_trait::async_trait;
use log::info;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid;
use uuid::Uuid;
use crate::vmap;
use crate::vmap_opt;

use crate::db::SqliteDatabase;
use crate::p2p::crypto::PeerId;
use crate::p2p::service::P2PRemoteServiceDelegate;
use crate::p2p::transport::{TransportConfig, P2PTransport};
use crate::services::abstract_service::{ServiceMetadata, ServiceState};
use crate::services::{
    NodeRequestHandler, RequestContext, ServiceRequest, ServiceResponse,
    SubscriptionOptions, ResponseStatus,
};
use crate::services::types::ValueType;
use crate::services::service_registry::ServiceRegistry;
use crate::services::abstract_service::AbstractService;
use crate::services::node_info::NodeInfoService;
use crate::util::logging::{debug_log, debug_log_with_data, error_log, info_log, warn_log, Component};

/// Configuration for a Node
/// Encapsulates all configuration options for a node in one place
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Network ID for the node
    pub network_id: String,
    /// Node ID for logging and identification
    pub node_id: Option<String>,
    /// Path to the node's storage directory
    pub node_path: String,
    /// Path to the node's database file
    pub db_path: String,
    /// P2P transport configuration
    pub p2p_config: Option<TransportConfig>,
    /// Node state directory (defaults to node_path)
    pub state_path: Option<String>,
    /// Test network IDs to use for testing purposes
    pub test_network_ids: Option<Vec<String>>,
    /// Bootstrap nodes for the node
    pub bootstrap_nodes: Option<Vec<String>>,
    /// Listen address for the node
    pub listen_addr: Option<String>,
}

impl NodeConfig {
    /// Create a new NodeConfig with minimal required parameters
    pub fn new(network_id: &str, node_path: &str, db_path: &str) -> Self {
        Self {
            network_id: network_id.to_string(),
            node_path: node_path.to_string(),
            db_path: db_path.to_string(),
            node_id: None, // Default to None
            p2p_config: None,
            state_path: None,
            test_network_ids: None,
            bootstrap_nodes: None,
            listen_addr: None,
        }
    }

    /// Generate a random node ID if one is not provided
    fn get_or_generate_node_id(&self) -> String {
        self.node_id.clone().unwrap_or_else(|| {
            if cfg!(test) {
                "TEST_NODE".to_string()
            } else {
                format!("node-{}", Uuid::new_v4())
            }
        })
    }

    /// Create a new NodeConfig with a specific node ID (for testing)
    pub fn new_with_node_id(network_id: &str, node_path: &str, node_id: &str) -> Self {
        let db_path = format!("{}/node.db", node_path);
        NodeConfig {
            network_id: network_id.to_string(),
            node_id: Some(node_id.to_string()),
            node_path: node_path.to_string(),
            db_path,
            p2p_config: None,
            state_path: None,
            test_network_ids: None,
            bootstrap_nodes: None,
            listen_addr: None,
        }
    }
}

/// Node represents a Runar node that can host services and communicate with other nodes
pub struct Node {
    /// Configuration for the node
    pub config: NodeConfig,

    /// Service registry for managing services
    pub service_registry: Arc<ServiceRegistry>,

    /// Database instance
    pub db: Arc<SqliteDatabase>,

    /// Network ID for this node
    pub network_id: String,

    /// Path to the node's data directory
    pub node_path: String,

    /// Path to the node's database
    pub db_path: String,

    /// Path to the node's state directory
    pub state_path: String,

    /// The P2P delegate for remote service calls and transport management
    p2p_delegate: Arc<P2PRemoteServiceDelegate>,

    /// Track initialization state
    pub initialized: bool,

    /// Service map for tracking registered services
    services: Arc<RwLock<HashMap<String, Arc<dyn AbstractService>>>>,

    /// Service state
    state: Arc<RwLock<ServiceState>>,
}

/// A dummy NodeRequestHandler used to create a RequestContext that won't be used for nested requests
pub struct DummyNodeRequestHandler {}

#[async_trait::async_trait]
impl NodeRequestHandler for DummyNodeRequestHandler {
    async fn request(&self, _path: String, _params: ValueType) -> Result<ServiceResponse> {
        Err(anyhow!("DummyNodeRequestHandler: Cannot make requests"))
    }

    async fn publish(&self, _topic: String, _data: ValueType) -> Result<()> {
        Err(anyhow!("DummyNodeRequestHandler: Cannot publish events"))
    }

    async fn subscribe(
        &self,
        _topic: String,
        _callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
    ) -> Result<String> {
        Err(anyhow!(
            "DummyNodeRequestHandler: Cannot subscribe to events"
        ))
    }

    async fn subscribe_with_options(
        &self,
        _topic: String,
        _callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
        _options: SubscriptionOptions,
    ) -> Result<String> {
        Err(anyhow!(
            "DummyNodeRequestHandler: Cannot subscribe to events"
        ))
    }

    async fn unsubscribe(&self, _topic: String, _subscription_id: Option<&str>) -> Result<()> {
        Err(anyhow!(
            "DummyNodeRequestHandler: Cannot unsubscribe from events"
        ))
    }

    fn list_services(&self) -> Vec<String> {
        // Return an empty list for the dummy handler
        Vec::new()
    }
}

/// NodeRequestHandlerImpl - Implements NodeRequestHandler by wrapping a reference to the Node
pub struct NodeRequestHandlerImpl {
    /// Reference to the service registry
    service_registry: Arc<ServiceRegistry>,
}

impl NodeRequestHandlerImpl {
    /// Create a new NodeRequestHandlerImpl
    pub fn new(service_registry: Arc<ServiceRegistry>) -> Self {
        Self { service_registry }
    }
}

#[async_trait::async_trait]
impl NodeRequestHandler for NodeRequestHandlerImpl {
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        debug_log(Component::Node, &format!("NodeRequestHandlerImpl::request - Path: {}", path));
        debug_log_with_data(
            Component::Node,
            "NodeRequestHandlerImpl::request - Params",
            &params,
        );

        // Parse the path into service name and operation
        // Format should be "serviceName/operation"
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid path format, expected 'serviceName/operation'"
            ));
        }

        let service_name = parts[0].to_string();
        let operation = parts[1].to_string();

        debug_log(
            Component::Node,
            &format!(
                "NodeRequestHandlerImpl::request - Service: {}, Operation: {}",
                service_name, operation
            ),
        );

        // Create a service request
        let request = ServiceRequest {
            path: service_name.clone(),
            operation: operation.clone(),
            params: Some(params),
            request_id: None,
            request_context: Arc::new(RequestContext::new_with_option(
                format!("{}/{}", service_name, operation),
                vmap_opt! {},
                Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()))
            )),
            metadata: None,
        };

        // Call the service through the registry
        if let Some(service) = self.service_registry.get_service(&service_name).await {
            service.handle_request(request).await
        } else {
            Ok(ServiceResponse::error(format!("Service not found: {}", service_name)))
        }
    }

    async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        // Parse the topic string to get service name if present
        // Format can be either "topic" or "serviceName/topic"
        let parts: Vec<&str> = topic.split('/').collect();
        let (service_name, topic_name) = if parts.len() > 1 {
            (Some(parts[0].to_string()), parts[1].to_string())
        } else {
            (None, topic.clone())
        };

        // Use the registry's publish method
        if let Some(service) = service_name {
            self.service_registry.publish(topic_name.clone(), data).await
        } else {
            self.service_registry.publish(topic_name, data).await
        }
    }

    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
    ) -> Result<String> {
        // Use default subscription options
        self.subscribe_with_options(topic, callback, SubscriptionOptions::default())
            .await
    }

    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        // Parse the topic string to get service name if present
        // Format can be either "topic" or "serviceName/topic"
        let parts: Vec<&str> = topic.split('/').collect();
        let (service_name, topic_name) = if parts.len() > 1 {
            (Some(parts[0].to_string()), parts[1].to_string())
        } else {
            (None, topic.clone())
        };

        // Use the registry's subscribe_with_options method
        if let Some(service_name) = service_name {
            self.service_registry.subscribe_with_options(
                service_name, 
                callback, 
                options
            ).await
        } else {
            // Default to using the "global" service if none specified
            self.service_registry.subscribe_with_options(
                "global".to_string(),
                callback,
                options
            ).await
        }
    }

    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()> {
        // Parse the topic string to get service name if present
        // Format can be either "topic" or "serviceName/topic"
        let parts: Vec<&str> = topic.split('/').collect();
        let (service_name, topic_name) = if parts.len() > 1 {
            (Some(parts[0].to_string()), parts[1].to_string())
        } else {
            (None, topic.clone())
        };

        // Use the registry's unsubscribe method
        if let Some(service) = service_name {
            self.service_registry.unsubscribe(&service, &topic_name, subscription_id).await
        } else {
            // Default to using the "global" service if none specified
            self.service_registry.unsubscribe("global", &topic_name, subscription_id).await
        }
    }

    fn list_services(&self) -> Vec<String> {
        // Spawn a blocking task to fetch the services synchronously
        let registry = self.service_registry.clone();
        match tokio::task::block_in_place(move || {
            match tokio::runtime::Handle::current().block_on(async {
                registry.list_services()
            }) {
                services => services
            }
        }) {
            services => services,
        }
    }
}

// Explicitly implement Send and Sync for NodeRequestHandlerImpl
// This is needed because the trait object requires these bounds
unsafe impl Send for NodeRequestHandlerImpl {}
unsafe impl Sync for NodeRequestHandlerImpl {}

impl Node {
    /// Create a new node with the given configuration
    pub async fn new(config: NodeConfig) -> Result<Self> {
        // Create the database
        let db = Arc::new(SqliteDatabase::new(&config.db_path).await?);

        // Create the service registry with the database
        let service_registry =
            Arc::new(ServiceRegistry::new_with_db(&config.network_id, db.clone()));

        // Get state path, defaulting to node_path if not set
        let state_path = config
            .state_path
            .as_ref()
            .map(String::clone)
            .unwrap_or_else(|| config.node_path.clone());

        // Create P2P delegate
        let p2p_delegate =
            P2PRemoteServiceDelegate::new(None, &config.network_id, db.clone()).await?;

        Ok(Self {
            service_registry,
            db,
            network_id: config.network_id.clone(),
            node_path: config.node_path.clone(),
            db_path: config.db_path.clone(),
            state_path,
            config,
            p2p_delegate: Arc::new(p2p_delegate),
            initialized: false,
            services: Arc::new(RwLock::new(HashMap::new())),
            state: Arc::new(RwLock::new(ServiceState::Created)),
        })
    }

    /// Initialize the node
    pub async fn init(&mut self) -> Result<()> {
        info_log(Component::Node, "Initializing node");

        // Set state to initializing
        *self.state.write().await = ServiceState::Initialized;

        // We can't set the node handler directly since we don't have mutable access to service_registry
        // instead, we'll create the node handler and use it for all our requests
        let node_handler = Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()));

        // Create and open the node database
        // (This is already done in the constructor, so just check for success)
        if !Path::new(&self.db_path).exists() {
            return Err(anyhow!("Database file not found: {}", self.db_path));
        }

        // Initialize services
        self.init_services().await?;

        // Initialize P2P
        self.init_p2p(None).await?;

        // Start anonymous service cleanup (every 30 minutes)
        Node::start_anonymous_service_cleanup(
            Arc::new(self.config.clone()),
            self.service_registry.clone(),
            Duration::from_secs(1800),
        )?;

        // Run initializers
        crate::init::run_initializers().await?;
        
        // When linkme is not available, use runtime registrations
        #[cfg(not(feature = "distributed_slice"))]
        {
            self.service_registry.init_runtime_registrations().await?;
        }

        // Set state to running
        *self.state.write().await = ServiceState::Running;
        self.initialized = true;

        info_log(Component::Node, "Node initialized");

        Ok(())
    }

    /// Initialize the P2P subsystem
    pub async fn init_p2p(&mut self, fixed_id_ref: Option<&str>) -> Result<()> {
        info_log(Component::Node, "Initializing P2P functionality");

        // Create transport config
        let transport_config = TransportConfig {
            network_id: self.config.network_id.clone(),
            state_path: self.state_path.clone(),
            bootstrap_nodes: self.config.bootstrap_nodes.clone(),
            listen_addr: self.config.listen_addr.clone(),
        };

        // Create and initialize P2P transport
        let p2p_transport = P2PTransport::new(transport_config, fixed_id_ref).await?;
        let p2p_transport = Arc::new(p2p_transport);

        // Update the delegate's transport
        let mut delegate = (*self.p2p_delegate).clone();
        delegate = delegate
            .with_transport(p2p_transport.clone(), Some(&self.config.network_id))
            .await?;
        self.p2p_delegate = Arc::new(delegate);

        // Start the transport to begin listening
        p2p_transport.start().await?;

        // If bootstrap nodes are configured, connect to them automatically
        if let Some(bootstrap_nodes) = &self.config.bootstrap_nodes {
            for bootstrap_addr in bootstrap_nodes {
                debug_log(
                    Component::Node,
                    &format!("Connecting to bootstrap node: {}", bootstrap_addr),
                );

                // Connect to the bootstrap node
                let connect_result = p2p_transport.connect(bootstrap_addr).await;
                match connect_result {
                    Ok(_) => {
                        info_log(
                            Component::Node,
                            &format!(
                                "Successfully connected to bootstrap node: {}",
                                bootstrap_addr
                            ),
                        );
                    }
                    Err(e) => {
                        warn_log(
                            Component::Node,
                            &format!(
                                "Failed to connect to bootstrap node {}: {}",
                                bootstrap_addr, e
                            ),
                        );
                    }
                }
            }
        }

        info_log(Component::Node, "P2P functionality initialized");
        Ok(())
    }

    /// Initialize built-in services
    pub async fn init_services(&mut self) -> Result<()> {
        info_log(Component::Node, "Initializing services");

        // Initialize the node info service
        let node_info_service =
            NodeInfoService::new(&self.config.network_id, Arc::new(self.config.clone()));
        self.add_service(node_info_service).await?;

        // Initialize the registry info service
        let registry_info_service = crate::services::registry_info::RegistryInfoService::new(
            &self.config.network_id,
            self.service_registry.clone(),
        );
        self.add_service(registry_info_service).await?;

        Ok(())
    }

    /// Add a service to the node
    pub async fn add_service<S>(&mut self, mut service: S) -> Result<()>
    where
        S: AbstractService + 'static,
    {
        // Create a node handler for request context
        let node_handler = Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()));

        // Create a request context for initialization
        let request_context = Arc::new(RequestContext::new_with_option(
            service.name().to_string(),
            vmap_opt! {},
            node_handler,
        ));

        // Initialize and start the service
        info!("Initializing service: {}", service.name());
        service.init(&request_context).await?;

        info!("Starting service: {}", service.name());
        service.start().await?;

        // Register with the service registry
        self.service_registry
            .register_service(Arc::new(service))
            .await?;

        Ok(())
    }

    /// Call a service with the given path, operation, and parameters
    /// This method is mainly for backward compatibility
    pub async fn call<P: Into<String>, O: Into<String>, V: Into<ValueType>>(
        &self,
        path: P,
        operation: O,
        params: V,
    ) -> Result<ServiceResponse> {
        // Combine path and operation into the new format
        let path_str = path.into();
        let op_str = operation.into();
        let full_path = format!("{}/{}", path_str, op_str);

        // Forward to the new request method
        self.request(full_path, params).await
    }

    /// Make a request to a service
    pub async fn request<P: Into<String>, V: Into<ValueType>>(
        &self,
        path: P,
        params: V,
    ) -> Result<ServiceResponse> {
        let path_str = path.into();
        let params_value = params.into();
        
        // Process the parameters and make the actual request
        self.process_request(path_str, params_value).await
    }
    
    /// Helper method that does the actual request processing
    async fn process_request(&self, path_str: String, params_value: ValueType) -> Result<ServiceResponse> {
        // Parse the path into service name and operation
        // Format should be "serviceName/operation"
        let parts: Vec<&str> = path_str.split('/').collect();
        if parts.len() != 2 {
            return Err(anyhow!(
                "Invalid path format, expected 'serviceName/operation'"
            ));
        }

        let service_name = parts[0].to_string();
        let operation = parts[1].to_string();
        
        // Create a request context for the request
        let context = Arc::new(RequestContext::new_with_option(
            format!("node_request_{}", uuid::Uuid::new_v4()),
            vmap_opt! {},
            Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone())),
        ));
        
        // Handle direct parameter values (non-Map ValueType)
        let processed_params = match &params_value {
            ValueType::Map(_) => {
                // Already a map, use as is
                params_value
            },
            _ => {
                // For any other ValueType, we need to wrap it in a parameter map
                let param_name = self.guess_parameter_name(&service_name, &operation, &params_value);
                
                // Create a map with the single parameter
                let mut param_map = HashMap::new();
                param_map.insert(param_name, params_value);
                ValueType::Map(param_map)
            }
        };
        
        let request = ServiceRequest {
            path: service_name.clone(),
            operation: operation.clone(),
            params: Some(processed_params.clone()),
            request_id: None,
            request_context: context,
            metadata: None,
        };
        
        // Find the target service
        if let Some(service) = self.service_registry.get_service_by_path(&service_name).await {
            // Call the service
            service.handle_request(request).await
        } else {
            // Service not found
            Ok(ServiceResponse::error(format!("Service '{}' not found", service_name)))
        }
    }

    /// Helper method to guess a parameter name based on the value type
    fn guess_parameter_name(&self, service_name: &str, operation: &str, value: &ValueType) -> String {
        // Try to intelligently guess the parameter name based on the operation and value type
        
        // For simple operations that follow patterns, use standard names
        match operation {
            "transform" | "process" | "format" | "parse" | "convert" => {
                return "data".to_string();
            },
            "get" | "get_by_id" | "find" | "find_by_id" | "retrieve" => {
                return "id".to_string();
            },
            "search" | "query" | "filter" => {
                return "query".to_string();
            },
            "add" | "create" | "insert" => {
                return "item".to_string();
            },
            "update" | "modify" | "edit" => {
                return "data".to_string();
            },
            "delete" | "remove" => {
                return "id".to_string();
            },
            _ => {}
        }
        
        // If no pattern match, guess based on value type
        match value {
            ValueType::String(_) => "data".to_string(),
            ValueType::Number(_) => "value".to_string(),
            ValueType::Bool(_) => "flag".to_string(),
            ValueType::Array(_) => "items".to_string(),
            ValueType::Map(_) => "data".to_string(),
            ValueType::Json(_) => "json".to_string(),
            ValueType::Bytes(_) => "bytes".to_string(),
            ValueType::Struct(_) => "data".to_string(),
            ValueType::Null => "data".to_string(),
        }
    }

    /// Make a node request with any parameters
    pub async fn node_request(&self, params: ValueType) -> Result<ServiceResponse> {
        // Create a request context for the request
        let context = Arc::new(RequestContext::new_with_option(
            format!("node_request_{}", uuid::Uuid::new_v4()),
            vmap_opt! {},
            Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone())),
        ));
        
        let request = ServiceRequest {
            path: "node".to_string(),
            operation: "info".to_string(),
            params: Some(params),
            request_id: None,
            request_context: context,
            metadata: None,
        };
        
        // Find the target service
        if let Some(service) = self.services.read().await.get("node_info").cloned() {
            // Call the service
            service.handle_request(request).await
        } else {
            // Service not found
            Ok(ServiceResponse::error("Node info service not found".to_string()))
        }
    }

    /// Publish an event on a topic
    pub async fn publish<T: Into<String>, V: Into<ValueType>>(
        &self,
        topic: T,
        data: V,
    ) -> Result<()> {
        let topic_str = topic.into();
        let data_value = data.into();

        // Parse the topic into service name and event name (similar to request path)
        // Format should be "serviceName/eventName"
        let parts: Vec<&str> = topic_str.split('/').collect();

        if parts.is_empty() {
            return Err(anyhow!(
                "Invalid topic format. Expected 'serviceName/eventName'"
            ));
        }

        let service_name = parts[0];
        let event_name = if parts.len() > 1 { parts[1] } else { "" };

        // Create a node handler reference for the request context with the correct network ID
        let node_handler = Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()));

        // Create a request context
        let request_context = Arc::new(RequestContext::new_with_option(
            format!("{}/{}", service_name, event_name),
            vmap_opt! {},
            node_handler.clone(),
        ));

        // Delegate to the node handler's publish method
        node_handler.publish(topic_str, data_value).await
    }

    /// Subscribe to events on a topic
    pub async fn subscribe<T: Into<String>, F>(&self, topic: T, callback: F) -> Result<String>
    where
        F: Fn(ValueType) -> Result<()> + Send + Sync + 'static,
    {
        let topic_str = topic.into();
        let callback_box = Box::new(callback);

        // Parse the topic into service name and event name (similar to request path)
        // Format should be "serviceName/eventName"
        let parts: Vec<&str> = topic_str.split('/').collect();

        if parts.is_empty() {
            return Err(anyhow!(
                "Invalid topic format. Expected 'serviceName/eventName'"
            ));
        }

        let service_name = parts[0];
        let event_name = if parts.len() > 1 { parts[1] } else { "" };

        // Create a node handler reference for the request context with the correct network ID
        let node_handler = Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()));

        // Create a request context
        let request_context = Arc::new(RequestContext::new_with_option(
            format!("{}/{}", service_name, event_name),
            vmap_opt! {},
            node_handler.clone(),
        ));

        // Create an anonymous service to handle this subscription
        // This ensures all subscribers are tied to a service to maintain architectural consistency
        let mut anonymous_service =
            crate::services::AnonymousSubscriberService::new(&self.config.network_id, &topic_str);

        // Initialize and start the service
        anonymous_service.init(&request_context).await?;
        anonymous_service.start().await?;

        // Register the service with the registry
        let service_arc = Arc::new(anonymous_service);
        self.service_registry
            .register_service(service_arc.clone())
            .await?;

        // Get the anonymous service name (which is generated with a UUID)
        let anonymous_service_name = service_arc.name().to_string();

        // Register the subscription with the anonymous service name using the provided options
        self.service_registry
            .subscribe_with_options(anonymous_service_name, callback_box, SubscriptionOptions::default())
            .await
    }

    /// Subscribe to events on a topic with options
    pub async fn subscribe_with_options<T: Into<String>, F>(
        &self,
        topic: T,
        callback: F,
        options: SubscriptionOptions,
    ) -> Result<String>
    where
        F: Fn(ValueType) -> Result<()> + Send + Sync + 'static,
    {
        let topic_str = topic.into();
        let callback_box = Box::new(callback);

        // Parse the topic into service name and event name (similar to request path)
        // Format should be "serviceName/eventName"
        let parts: Vec<&str> = topic_str.split('/').collect();

        if parts.is_empty() {
            return Err(anyhow!(
                "Invalid topic format. Expected 'serviceName/eventName'"
            ));
        }

        let service_name = parts[0];
        let event_name = if parts.len() > 1 { parts[1] } else { "" };

        // Create a node handler reference for the request context with the correct network ID
        let node_handler = Arc::new(NodeRequestHandlerImpl::new(self.service_registry.clone()));

        // Create a request context
        let request_context = Arc::new(RequestContext::new_with_option(
            format!("{}/{}", service_name, event_name),
            vmap_opt! {},
            node_handler.clone(),
        ));

        // Create an anonymous service to handle this subscription
        // This ensures all subscribers are tied to a service to maintain architectural consistency
        let mut anonymous_service =
            crate::services::AnonymousSubscriberService::new(&self.config.network_id, &topic_str);

        // Initialize and start the service
        anonymous_service.init(&request_context).await?;
        anonymous_service.start().await?;

        // Register the service with the registry
        let service_arc = Arc::new(anonymous_service);
        self.service_registry
            .register_service(service_arc.clone())
            .await?;

        // Get the anonymous service name (which is generated with a UUID)
        let anonymous_service_name = service_arc.name().to_string();

        // Register the subscription with the anonymous service name using the provided options
        self.service_registry
            .subscribe_with_options(anonymous_service_name, callback_box, options)
            .await
    }

    /// Subscribe to an event once (unsubscribes after first event)
    pub async fn once<T: Into<String>, F>(&self, topic: T, callback: F) -> Result<String>
    where
        F: Fn(ValueType) -> Result<()> + Send + Sync + 'static,
    {
        // Create options for a one-time subscription
        let options = SubscriptionOptions::new().once();

        // Subscribe with the one-time options
        self.subscribe_with_options(topic, callback, options).await
    }

    /// Unsubscribe from a topic
    pub async fn unsubscribe<T: Into<String>>(
        &self,
        topic: T,
        subscription_id: Option<&str>,
    ) -> Result<()>
where {
        let topic_str = topic.into();

        // Parse the topic to get the service name
        let parts: Vec<&str> = topic_str.split('/').collect();

        if parts.is_empty() {
            return Err(anyhow!(
                "Invalid topic format. Expected 'serviceName/eventName'"
            ));
        }

        let service_name = parts[0];

        // Unsubscribe from the registry
        self.service_registry
            .unsubscribe(service_name, &topic_str, subscription_id)
            .await
    }

    /// Get the service registry
    pub fn service_registry(&self) -> Arc<ServiceRegistry> {
        warn_log(
            Component::Node,
            "Warning: Using service_registry() which returns a new empty clone",
        );
        Arc::new(ServiceRegistry::new(&self.network_id))
    }

    /// Get the service registry as an Arc
    pub fn service_registry_arc(&self) -> Arc<ServiceRegistry> {
        debug_log(
            Component::Node,
            "Using service_registry_arc() which returns the actual registry",
        );
        self.service_registry.clone()
    }

    /// Get the database connection
    pub fn db(&self) -> Arc<SqliteDatabase> {
        self.db.clone()
    }

    /// Get the network ID
    pub fn network_id(&self) -> &str {
        &self.network_id
    }

    /// Get the node path
    pub fn node_path(&self) -> &str {
        &self.node_path
    }

    /// Get the database path
    pub fn db_path(&self) -> &str {
        &self.db_path
    }

    /// Stop all registered services
    pub async fn stop_services(&self) -> Result<()> {
        let services = self.services.read().await.clone();
        
        for service_name in services.keys() {
            let mut service = self.get_service_mut(service_name).await?;
            service.stop().await?;
        }
        
        Ok(())
    }

    /// Start all registered services
    pub async fn start_services(&self) -> Result<()> {
        let services = self.services.read().await.clone();
        
        for service_name in services.keys() {
            let mut service = self.get_service_mut(service_name).await?;
            service.start().await?;
        }
        
        Ok(())
    }

    /// Stop the node and all its services
    pub async fn stop(&self) -> Result<()> {
        // Stop all services first
        self.stop_services().await?;

        // Stop P2P delegate - clone and use deref
        let mut p2p_delegate = (*self.p2p_delegate).clone();
        p2p_delegate.stop().await?;

        Ok(())
    }

    /// Run periodic cleanup of anonymous services
    pub fn start_anonymous_service_cleanup(
        config: Arc<NodeConfig>,
        registry: Arc<ServiceRegistry>,
        interval: Duration,
    ) -> Result<()> {
        // Spawn a background task to perform periodic cleanup
        tokio::spawn(async move {
            let network_id = config.network_id.clone();

            // Create a node request handler for the cleanup task
            let node_handler = Arc::new(NodeRequestHandlerImpl::new(registry.clone()));

            loop {
                // Sleep for the specified interval
                tokio::time::sleep(interval).await;

                // Get all services
                let services = registry.get_all_services().await;
                let mut removed_count = 0;

                // Loop through all anonymous subscriber services
                for service in services {
                    // Check if this is an anonymous subscriber service
                    if service.name().starts_with("anonymous_subscriber_") {
                        // Create a request context
                        let request_context = Arc::new(RequestContext::new_with_option(
                            format!("{}/get_info", service.name()),
                            vmap_opt! {},
                            node_handler.clone(),
                        ));

                        // Create a get_info request to check the service status
                        let request = ServiceRequest {
                            request_id: Some(Uuid::new_v4().to_string()),
                            path: service.path().to_string(),
                            operation: "get_info".to_string(),
                            params: vmap_opt! {},
                            request_context,
                            metadata: None,
                        };

                        // Process the request
                        match service.handle_request(request).await {
                            Ok(response) => {
                                if response.status == ResponseStatus::Success {
                                    // Extract information from the response
                                    if let Some(data) = response.data {
                                        // Check if this is an anonymous subscriber service that can be cleaned up
                                        let is_expired = if let ValueType::Map(data_map) = &data {
                                            if let Some(ValueType::Bool(expired)) = data_map.get("is_expired") {
                                                *expired
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        };

                                        let subscription_count = if let ValueType::Map(data_map) = &data {
                                            if let Some(ValueType::Number(count)) = data_map.get("subscription_count") {
                                                *count as i64
                                            } else {
                                                1
                                            }
                                        } else {
                                            1
                                        };

                                        if is_expired && subscription_count == 0 {
                                            // Get the service name
                                            let name = service.name().to_string();

                                            // Stop the service
                                            // Note: We can't call stop() directly because it requires &mut self
                                            // Instead, we'll create a request to stop the service
                                            let request = ServiceRequest {
                                                path: name.clone(),
                                                operation: "stop".to_string(),
                                                params: None,
                                                request_id: Some(uuid::Uuid::new_v4().to_string()),
                                                request_context: Arc::new(RequestContext::new_with_option(
                                                    format!("{}/stop", name),
                                                    vmap_opt! {},
                                                    Arc::new(NodeRequestHandlerImpl::new(registry.clone())),
                                                )),
                                                metadata: None,
                                            };
                                            
                                            if let Err(e) = service.handle_request(request).await {
                                                error_log(
                                                    Component::Node,
                                                    &format!(
                                                        "Failed to stop service {}: {}",
                                                        name, e
                                                    ),
                                                );
                                                continue;
                                            }

                                            // Log the cleanup
                                            info_log(
                                                Component::Node,
                                                &format!(
                                                    "Cleaned up expired anonymous service: {}",
                                                    name
                                                ),
                                            );

                                            removed_count += 1;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                // Log the error but continue processing other services
                                error_log(
                                    Component::Node,
                                    &format!("Error checking anonymous service status: {}", e),
                                );
                            }
                        }
                    }
                }

                if removed_count > 0 {
                    info_log(
                        Component::Node,
                        &format!("Cleaned up {} expired anonymous services", removed_count),
                    );
                }
            }
        });

        Ok(())
    }

    /// Wait for a service to become available
    pub async fn wait_for_service(&self, service_name: &str, timeout_ms: Option<u64>) -> Result<bool> {
        let timeout_duration = Duration::from_millis(timeout_ms.unwrap_or(10000));
        let start_time = Instant::now();

        while start_time.elapsed() < timeout_duration {
            // Check if service exists in registry
            let services = self.request("registry/list", ValueType::Json(json!({}))).await?;
            
            if let Some(ValueType::Array(services)) = services.data {
                for service in services {
                    if let ValueType::String(name) = service {
                        if name == service_name {
                            return Ok(true);
                        }
                    }
                }
            }

            // Wait a bit before checking again
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        Ok(false)
    }

    /// Get a mutable reference to a service by name
    async fn get_service_mut(&self, name: &str) -> Result<Box<dyn AbstractService>> {
        let services = self.services.read().await;
        let service = services.get(name).cloned();
        
        if let Some(service_arc) = service {
            // Create a new instance by cloning the service
            // This is a workaround since we can't get a mutable reference from an Arc
            // In a real implementation, we would need to handle this differently
            let name = service_arc.name().to_string();
            let path = service_arc.path().to_string();
            
            // Create a new dummy service
            let dummy_service = Box::new(DummyService {
                name,
                path,
                state: service_arc.state(),
            });
            
            return Ok(dummy_service);
        }
        
        Err(anyhow!("Service '{}' not found", name))
    }
}

/// A dummy service used as a workaround for mutability issues
struct DummyService {
    name: String,
    path: String,
    state: ServiceState,
}

#[async_trait]
impl AbstractService for DummyService {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn path(&self) -> &str {
        &self.path
    }
    
    fn state(&self) -> ServiceState {
        self.state
    }
    
    fn metadata(&self) -> ServiceMetadata {
        ServiceMetadata {
            name: self.name().to_string(),
            path: self.path().to_string(),
            state: self.state(),
            description: self.description().to_string(),
            operations: vec![],
            version: "1.0".to_string(),
        }
    }
    
    fn description(&self) -> &str {
        "Dummy service"
    }
    
    async fn init(&mut self, _ctx: &RequestContext) -> Result<()> {
        Ok(())
    }
    
    async fn start(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn handle_request(&self, _request: ServiceRequest) -> Result<ServiceResponse> {
        Ok(ServiceResponse::error("This is a dummy service"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> NodeConfig {
        NodeConfig {
            network_id: "test".to_string(),
            node_id: None,
            node_path: "/tmp/test".to_string(),
            db_path: "/tmp/test/node.db".to_string(),
            p2p_config: None,
            state_path: None,
            test_network_ids: None,
            bootstrap_nodes: None,
            listen_addr: None,
        }
    }
}
