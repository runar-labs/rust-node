// Services Module
//
// INTENTION:
// This module defines the core service architecture components, interfaces, and types
// that form the foundation of the service-oriented architecture. It establishes
// the contracts between services, the node, and clients that use them.
//
// ARCHITECTURAL PRINCIPLES:
// 1. Service Isolation - Services are self-contained with clear boundaries
// 2. Request-Response Pattern - All service interactions follow a consistent
//    request-response pattern for predictability and simplicity
// 3. Contextual Logging - All operations include context for proper tracing
// 4. Network Isolation - Services operate within specific network boundaries
// 5. Event-Driven Communication - Services can publish and subscribe to events
//
// This module is the cornerstone of the system's service architecture, defining
// how services are structured, discovered, and interacted with.

use anyhow::{anyhow, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::fmt::{self, Debug};
use std::collections::HashMap;
use crate::routing::TopicPath;
use serde::{Serialize, Deserialize};
use tokio::sync::RwLock;

pub mod service_registry;
pub mod abstract_service;
pub mod registry_info;
pub mod request_context;
pub mod event_context;

use runar_common::types::ValueType;
use runar_common::logging::{Component, LoggingContext, Logger};
use crate::services::abstract_service::{ActionMetadata, EventMetadata, CompleteServiceMetadata, ServiceState};
// Re-export the context types from their dedicated modules
pub use crate::services::request_context::RequestContext;
pub use crate::services::event_context::EventContext;

/// Handler for a service action
///
/// INTENTION: Define the signature for a function that handles a service action.
/// This provides a consistent interface for all action handlers and enables
/// them to be stored, passed around, and invoked uniformly.
pub type ActionHandler = Arc<dyn Fn(Option<ValueType>, RequestContext) -> ServiceFuture + Send + Sync>;

/// Type for action registration function
pub type ActionRegistrar = Arc<dyn Fn(&TopicPath, ActionHandler, Option<ActionMetadata>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Context for service lifecycle management
///
/// INTENTION: Provide services with the context needed for lifecycle operations
/// such as initialization and shutdown. Includes access to the node for
/// registering action handlers, subscribing to events, and other operations.
pub struct LifecycleContext {
    /// Network ID for the context
    pub network_id: String,
    /// Service path - identifies the service within the network
    pub service_path: String,
    /// Optional configuration data
    pub config: Option<ValueType>,
    /// Logger instance with service context
    pub logger: Logger,
    /// Node delegate for node operations
    node_delegate: Option<Arc<dyn NodeDelegate + Send + Sync>>,
}

impl LifecycleContext {
    /// Create a new LifecycleContext with the given topic path and logger
    ///
    /// This is the primary constructor that takes the minimum required parameters.
    pub fn new(topic_path: &TopicPath, logger: Logger) -> Self {
        Self {
            network_id: topic_path.network_id().to_string(),
            service_path: topic_path.service_path(),
            config: None,
            logger,
            node_delegate: None,
        }
    }
    
    /// Legacy constructor for backward compatibility
    pub fn new_from_path(network_id: &str, service_path: &str, logger: Logger) -> Self {
        Self {
            network_id: network_id.to_string(),
            service_path: service_path.to_string(),
            config: None,
            logger,
            node_delegate: None,
        }
    }
    
    /// Add configuration to a LifecycleContext
    ///
    /// Use builder-style methods instead of specialized constructors.
    pub fn with_config(mut self, config: ValueType) -> Self {
        self.config = Some(config);
        self
    }
    
    /// Add a NodeDelegate to a LifecycleContext
    ///
    /// INTENTION: Provide access to node operations during service lifecycle events,
    /// including action registration, request handling, and event dispatching.
    pub fn with_node_delegate(mut self, delegate: Arc<dyn NodeDelegate + Send + Sync>) -> Self {
        self.node_delegate = Some(delegate);
        self
    }
    
    /// Create a new LifecycleContext with a provided node delegate
    ///
    /// INTENTION: Create a fully functional LifecycleContext with all needed components
    /// for proper service initialization, including the delegate that allows services
    /// to register handlers and interact with the node.
    /// 
    /// This is kept for backward compatibility until all code is migrated to the builder pattern.
    pub fn new_with_delegate(
        network_id: &str,
        service_path: &str,
        logger: Logger,
        delegate: Arc<dyn NodeDelegate + Send + Sync>
    ) -> Self {
        let topic_path = TopicPath::new(service_path, network_id).expect("Invalid service path");
        Self::new(&topic_path, logger).with_node_delegate(delegate)
    }

    /// Helper method to log debug level message
    pub fn debug(&self, message: impl Into<String>) {
        self.logger.debug(message);
    }

    /// Helper method to log info level message
    pub fn info(&self, message: impl Into<String>) {
        self.logger.info(message);
    }

    /// Helper method to log warning level message
    pub fn warn(&self, message: impl Into<String>) {
        self.logger.warn(message);
    }

    /// Helper method to log error level message
    pub fn error(&self, message: impl Into<String>) {
        self.logger.error(message);
    }

    /// Register an action handler for a specific action
    ///
    /// INTENTION: Allow a service to register a handler function for one of its actions.
    /// This is typically done during service initialization to set up handlers
    /// for all supported actions.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::{LifecycleContext, ServiceResponse, ActionHandler};
    /// use runar_common::types::ValueType;
    /// use std::sync::Arc;
    /// use anyhow::Result;
    ///
    /// async fn init_service(context: LifecycleContext) -> Result<()> {
    ///     // Register a handler for the "add" action
    ///     context.register_action(
    ///         "add", 
    ///         Arc::new(|params, ctx| {
    ///             Box::pin(async move {
    ///                 // Handler implementation
    ///                 Ok(ServiceResponse::ok_empty())
    ///             })
    ///         })
    ///     ).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    pub async fn register_action(&self, action_name: &str, handler: ActionHandler) -> Result<()> {
        // Get the node delegate
        let delegate = match &self.node_delegate {
            Some(d) => d,
            None => return Err(anyhow!("No node delegate available")),
        };
        
        // Create a topic path for this action
        let action_path = format!("{}/{}", self.service_path, action_name);
        let topic_path = TopicPath::new(&action_path, &self.network_id)
            .map_err(|e| anyhow!("Invalid action path: {}", e))?;
        
        // Call the delegate with no metadata
        delegate.register_action_handler(&topic_path, handler, None).await
    }

    /// Register an action handler with metadata
    ///
    /// INTENTION: Allow a service to register a handler function for an action,
    /// along with descriptive metadata. This enables documentation and discovery
    /// of the action's purpose and parameters.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::{LifecycleContext, ServiceResponse, ActionHandler, ActionRegistrationOptions};
    /// use runar_common::types::ValueType;
    /// use std::sync::Arc;
    /// use anyhow::Result;
    ///
    /// async fn init_service(context: LifecycleContext) -> Result<()> {
    ///     // Create options for the action
    ///     let options = ActionRegistrationOptions {
    ///         description: Some("Adds two numbers".to_string()),
    ///         params_schema: None,
    ///         return_schema: None,
    ///     };
    ///     
    ///     // Register a handler with metadata
    ///     context.register_action_with_options(
    ///         "add", 
    ///         Arc::new(|params, ctx| {
    ///             Box::pin(async move {
    ///                 // Handler implementation
    ///                 Ok(ServiceResponse::ok_empty())
    ///             })
    ///         }),
    ///         options
    ///     ).await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    pub async fn register_action_with_options(
        &self,
        action_name: &str,
        handler: ActionHandler,
        options: ActionRegistrationOptions,
    ) -> Result<()> {
        // Create action metadata from the options
        let metadata = ActionMetadata {
            name: action_name.to_string(),
            description: options.description.unwrap_or_default(),
            parameters_schema: options.params_schema.map(|v| v),
            return_schema: options.return_schema.map(|v| v),
        };
        
        // Get the node delegate
        let delegate = match &self.node_delegate {
            Some(d) => d,
            None => return Err(anyhow!("No node delegate available")),
        };
        
        // Create a topic path for this action
        let action_path = format!("{}/{}", self.service_path, action_name);
        let topic_path = TopicPath::new(&action_path, &self.network_id)
            .map_err(|e| anyhow!("Invalid action path: {}", e))?;
        
        // Register the action with the provided options
        delegate.register_action_handler(&topic_path, handler, Some(metadata)).await
    }
    
    /// Register an event with extra metadata
    ///
    /// INTENTION: This method allows registering an event with additional
    /// information such as a description and data schema, which can be used
    /// for documentation and validation.
    pub async fn register_event_with_options(
        &self,
        event_name: &str,
        options: EventRegistrationOptions,
    ) -> Result<()> {
        // Create event metadata
        let metadata = EventMetadata {
            name: event_name.to_string(),
            description: options.description.unwrap_or_default(),
            data_schema: options.data_schema.map(|v| v),
        };
        
        // Log event registration with metadata
        self.logger.debug(&format!(
            "Registered event '{}' with metadata: description='{}', has_schema={}",
            event_name,
            metadata.description,
            metadata.data_schema.is_some()
        ));
        
        // Note: The NodeDelegate trait doesn't currently support registering events with metadata.
        // This information is logged for documentation purposes but not stored in the registry.
        // When event publishing occurs, this metadata would ideally be accessible.
        
        Ok(())
    }
    
    /// Register an action handler with template pattern matching
    ///
    /// INTENTION: Allow services to register handlers for path templates with named parameters,
    /// such as "services/{service_path}/state", extracting the parameters from matched paths.
    ///
    /// Example:
    /// ```rust,no_run
    /// use runar_node::services::{LifecycleContext, ServiceResponse};
    /// use runar_node::routing::TopicPath;
    /// use runar_common::types::ValueType;
    /// use runar_common::logging::{Logger, Component};
    /// use std::collections::HashMap;
    /// use std::sync::Arc;
    /// use anyhow::Result;
    /// 
    /// async fn example() -> Result<()> {
    ///     // Create a logger
    ///     let logger = Logger::new_root(Component::Service, "test-node");
    ///     
    ///     // Create a topic path for the service
    ///     let topic_path = TopicPath::new_service("test-service", "default");
    ///     
    ///     // Create a lifecycle context
    ///     let context = LifecycleContext::new(&topic_path, logger);
    /// 
    ///     // Register a handler for a templated path
    ///     context.register_action_pattern("services/{service_path}/state", 
    ///         |params, ctx, path_params| {
    ///             let service_path = path_params.get("service_path").map_or("", |v| v).to_string();
    ///             // Use service_path in the handler
    ///             Box::pin(async move {
    ///                 // Handler implementation
    ///                 Ok(ServiceResponse::ok(ValueType::Null))
    ///             })
    ///         })
    ///         .await?;
    ///     
    ///     Ok(())
    /// }
    /// ```
    pub async fn register_action_pattern<F, Fut>(&self, template: &str, handler: F) -> Result<()>
    where
        F: Fn(Option<ValueType>, RequestContext, std::collections::HashMap<String, String>) -> Fut + Send + Sync + Clone + 'static,
        Fut: Future<Output = Result<ServiceResponse>> + Send + 'static,
    {
        // Get the node delegate
        let delegate = match &self.node_delegate {
            Some(d) => d,
            None => return Err(anyhow!("No node delegate available")),
        };
        
        // Store template as String for move into closure
        let template_str = template.to_string();
        
        // Create a topic path for this template
        let topic_path = TopicPath::new(template, &self.network_id)
            .map_err(|e| anyhow!("Invalid template path: {}", e))?;
        
        // Create a wrapper for the handler that extracts parameters
        let action_handler = Arc::new(move |params: Option<ValueType>, mut context: RequestContext| -> ServiceFuture {
            // Capture handler and template by value
            let template_owned = template_str.clone();
            let handler_owned = handler.clone(); // This requires F: Clone
            
            Box::pin(async move {
                // Use the topic path's extract_params method directly
                if let Some(topic_path) = &context.topic_path {
                    if let Ok(path_params) = topic_path.extract_params(&template_owned) {
                        // Template matched, set path_params in the context
                        context.path_params = path_params.clone();
                        
                        // Call the handler with extracted parameters
                        handler_owned(params, context, path_params).await
                    } else {
                        // Template didn't match
                        Ok(ServiceResponse::error(400, "Path doesn't match template"))
                    }
                } else {
                    // No topic path in context
                    Ok(ServiceResponse::error(400, "No topic path in context"))
                }
            })
        });
        
        // Register the action with the provided template name
        delegate.register_action_handler(&topic_path, action_handler, None).await
    }
    
    /// Helper method to format a service-specific path
    fn format_path(&self, path: &str) -> String {
        format!("{}/{}", self.service_path, path)
    }
}

impl LoggingContext for LifecycleContext {
    fn component(&self) -> Component {
        Component::Service
    }
    
    fn service_path(&self) -> Option<&str> {
        Some(&self.service_path)
    }
    
    fn logger(&self) -> &Logger {
        &self.logger
    }
}

/// Service request for an action on a specific service
///
/// INTENTION: Represent a request to perform an action on a service.
/// Includes the destination, requested action, and input data.
///
/// ARCHITECTURAL PRINCIPLE:
/// The ServiceRequest should be self-contained with all routing and
/// context information needed to process a request from start to finish.
pub struct ServiceRequest {
    /// Topic path for the service and action
    pub topic_path: TopicPath,
    /// Data for the request
    pub data: ValueType,
    /// Request context
    pub context: Arc<RequestContext>,
}

impl ServiceRequest {
    /// Create a new service request
    ///
    /// INTENTION: Create a service request using the service path and action name.
    /// This method will construct the appropriate TopicPath and initialize the request.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::{ServiceRequest, RequestContext};
    /// use runar_common::types::ValueType;
    /// use std::sync::Arc;
    ///
    /// fn create_request(context: Arc<RequestContext>) {
    ///     // Create a request to the "auth" service with "login" action
    ///     let request = ServiceRequest::new(
    ///         "auth",
    ///         "login",
    ///         ValueType::Null,
    ///         context
    ///     );
    /// }
    /// ```
    pub fn new(
        service_path: &str,
        action_or_event: &str,
        data: ValueType,
        context: Arc<RequestContext>,
    ) -> Self {
        // Create a path string combining service path and action
        let path_string = format!("{}/{}", service_path, action_or_event);
        
        // Parse the path using the context's network_id method
        let topic_path = TopicPath::new(&path_string, &context.network_id())
            .expect("Invalid path format");
            
        Self {
            topic_path,
            data,
            context,
        }
    }

    /// Create a new service request with a TopicPath
    ///
    /// INTENTION: Create a service request using an existing TopicPath object.
    /// This is useful when the TopicPath has already been constructed and validated.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::{ServiceRequest, RequestContext};
    /// use runar_node::routing::TopicPath;
    /// use runar_common::types::ValueType;
    /// use std::sync::Arc;
    ///
    /// fn create_request(context: Arc<RequestContext>) {
    ///     // Create a topic path for the "auth/login" action
    ///     let topic_path = TopicPath::new("auth/login", &context.network_id())
    ///         .expect("Valid path");
    ///     
    ///     // Create a request with the topic path
    ///     let request = ServiceRequest::new_with_topic_path(
    ///         topic_path,
    ///         ValueType::Null,
    ///         context
    ///     );
    /// }
    /// ```
    pub fn new_with_topic_path(
        topic_path: TopicPath,
        data: ValueType,
        context: Arc<RequestContext>,
    ) -> Self {
        Self {
            topic_path,
            data,
            context,
        }
    }

    /// Create a new service request with optional data
    ///
    /// INTENTION: Create a service request where the data parameter is optional.
    /// If no data is provided, ValueType::Null will be used.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::{ServiceRequest, RequestContext};
    /// use runar_common::types::ValueType;
    /// use std::sync::Arc;
    ///
    /// fn create_request(context: Arc<RequestContext>, data: Option<ValueType>) {
    ///     // Create a request with optional data
    ///     let request = ServiceRequest::new_with_optional(
    ///         "auth",
    ///         "login",
    ///         data,
    ///         context
    ///     );
    /// }
    /// ```
    pub fn new_with_optional(
        service_path: &str,
        action_or_event: &str,
        data: Option<ValueType>,
        context: Arc<RequestContext>,
    ) -> Self {
        // Create a TopicPath from the service path and action
        let path_string = format!("{}/{}", service_path, action_or_event);
        
        // Parse the path using the context's network_id method
        let topic_path = TopicPath::new(&path_string, &context.network_id())
            .expect("Invalid path format");
            
        Self {
            topic_path,
            data: data.unwrap_or(ValueType::Null),
            context,
        }
    }
    
    /// Get the service path from the topic path
    ///
    /// INTENTION: Extract the service path component from the topic path.
    /// This is useful for service identification and routing.
    pub fn path(&self) -> String {
        self.topic_path.service_path()
    }
    
    /// Get the action or event name from the topic path
    ///
    /// INTENTION: Extract the action or event name from the topic path.
    /// This identifies what operation is being requested.
    pub fn action_or_event(&self) -> String {
        // Get the action path and extract the last part which is the action name
        let action_path = self.topic_path.action_path();
        
        // If action_path contains a slash, take the part after the last slash
        if let Some(idx) = action_path.rfind('/') {
            action_path[idx + 1..].to_string()
        } else {
            // If there's no slash, it's just the action name itself
            action_path
        }
    }
}

/// Status of a service response
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResponseStatus {
    /// The request was successful
    Success,
    /// The request failed
    Error,
}

/// A response from a service
///
/// INTENTION: Provide a consistent structure for all service responses, 
/// ensuring uniform error handling, result communication, and metadata
/// exchange between services and clients.
///
/// ARCHITECTURAL PRINCIPLE:
/// All service interactions should follow a consistent request-response pattern
/// with a clearly defined response structure that includes status, result data,
/// and possible error information.
#[derive(Debug, Clone)]
pub struct ServiceResponse {
    /// Status code - indicates success or error type
    pub status: i32,
    /// Response data (if successful)
    pub data: Option<ValueType>,
    /// Error details (if status indicates error)
    pub error: Option<String>,
}

impl ServiceResponse {
    /// Create a successful response with data
    ///
    /// INTENTION: Provide a convenient way to create a successful response
    /// with optional result data.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::ServiceResponse;
    /// use runar_common::types::ValueType;
    /// use serde_json::json;
    ///
    /// fn handle_get_user() -> ServiceResponse {
    ///     // Successful response with data
    ///     ServiceResponse::ok(ValueType::from(json!({
    ///         "id": "123",
    ///         "name": "John Doe"
    ///     })))
    /// }
    /// ```
    pub fn ok(data: ValueType) -> Self {
        Self {
            status: 200,
            data: Some(data),
            error: None,
        }
    }

    /// Create a successful response with no data
    ///
    /// INTENTION: Provide a convenient way to create a successful response
    /// when no result data is needed (for acknowledgement-type responses).
    pub fn ok_empty() -> Self {
        Self {
            status: 200,
            data: None,
            error: None,
        }
    }

    /// Create an error response
    ///
    /// INTENTION: Provide a convenient way to create an error response
    /// with an error message and appropriate status code.
    ///
    /// Example:
    /// ```
    /// use runar_node::services::ServiceResponse;
    ///
    /// fn handle_permission_check() -> ServiceResponse {
    ///     // Error response - unauthorized
    ///     ServiceResponse::error(403, "Insufficient permissions to access resource")
    /// }
    /// ```
    pub fn error(status: i32, message: &str) -> Self {
        Self {
            status,
            data: None,
            error: Some(message.to_string()),
        }
    }

    /// Check if the response indicates success
    ///
    /// INTENTION: Provide a convenient way to check if a response indicates
    /// success (2xx status code).
    pub fn is_success(&self) -> bool {
        self.status >= 200 && self.status < 300
    }

    /// Check if the response indicates an error
    ///
    /// INTENTION: Provide a convenient way to check if a response indicates
    /// an error (non-2xx status code).
    pub fn is_error(&self) -> bool {
        !self.is_success()
    }
}

/// Options for a subscription
#[derive(Debug, Clone, Default)]
pub struct SubscriptionOptions {
    // Add subscription options as needed
}

/// Options for publishing an event
///
/// INTENTION: Provide configuration options for event publishing,
/// allowing control over delivery scope, guarantees, and retention.
#[derive(Debug, Clone)]
pub struct PublishOptions {
    /// Whether this event should be broadcast to all nodes in the network
    pub broadcast: bool,
    
    /// Whether delivery should be guaranteed (at-least-once semantics)
    pub guaranteed_delivery: bool,
    
    /// How long the event should be retained for late subscribers (in seconds)
    /// None means no retention, 0 means forever
    pub retention_seconds: Option<u64>,
    
    /// Target a specific node or service instead of all subscribers
    pub target: Option<String>,
}

impl Default for PublishOptions {
    fn default() -> Self {
        Self {
            broadcast: false,
            guaranteed_delivery: false,
            retention_seconds: None,
            target: None,
        }
    }
}

/// Options for registering an action handler
///
/// INTENTION: Provide a way to specify metadata about an action when registering it,
/// reducing the need for services to define complete metadata upfront.
#[derive(Debug, Clone)]
pub struct ActionRegistrationOptions {
    /// Description of what the action does
    pub description: Option<String>,
    /// Parameter schema for validation and documentation
    pub params_schema: Option<ValueType>,
    /// Return value schema for documentation
    pub return_schema: Option<ValueType>,
}

impl Default for ActionRegistrationOptions {
    fn default() -> Self {
        Self {
            description: None,
            params_schema: None,
            return_schema: None,
        }
    }
}

/// Options for registering an event
///
/// INTENTION: Provide a way to specify metadata about an event when registering it,
/// reducing the need for services to define complete metadata upfront.
#[derive(Debug, Clone)]
pub struct EventRegistrationOptions {
    /// Description of what the event represents
    pub description: Option<String>,
    /// Schema of the event data
    pub data_schema: Option<ValueType>,
}

impl Default for EventRegistrationOptions {
    fn default() -> Self {
        Self {
            description: None,
            data_schema: None,
        }
    }
}

/// Interface for handling service requests
///
/// INTENTION: Define a consistent interface for handling node requests. This trait
/// is implemented by components that need to handle service requests, specifically
/// the Node component itself which should be responsible for routing requests to
/// appropriate services.
///
/// ARCHITECTURAL PRINCIPLE:
/// Request handling should be centralized in the Node component, with a clear
/// interface that allows for proper routing, logging, and error handling. This
/// separates the responsibility of request handling from service management.
#[async_trait::async_trait]
pub trait NodeRequestHandler: Send + Sync {
    /// Process a service request
    ///
    /// ```
    /// // Example implementation
    /// use runar_node::services::{NodeRequestHandler, ServiceResponse, EventContext, SubscriptionOptions};
    /// use runar_common::types::ValueType;
    /// use anyhow::Result;
    /// use async_trait::async_trait;
    /// use std::future::Future;
    /// use std::pin::Pin;
    /// use std::sync::Arc;
    /// 
    /// struct Node {}
    /// 
    /// #[async_trait]
    /// impl NodeRequestHandler for Node {
    ///     async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
    ///         // Process the request
    ///         Ok(ServiceResponse::ok_empty())
    ///     }
    ///     
    ///     // Other methods implemented...
    /// #   async fn publish(&self, _topic: String, _data: ValueType) -> Result<()> { Ok(()) }
    /// #   async fn subscribe(&self, _topic: String, _callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>) -> Result<String> { Ok(String::new()) }
    /// #   async fn subscribe_with_options(&self, _topic: String, _callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>, _options: SubscriptionOptions) -> Result<String> { Ok(String::new()) }
    /// #   async fn unsubscribe(&self, _topic: String, _subscription_id: Option<&str>) -> Result<()> { Ok(()) }
    /// #   fn list_services(&self) -> Vec<String> { Vec::new() }
    /// }
    /// ```
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse>;

    /// Publish an event to a topic
    async fn publish(&self, topic: String, data: ValueType) -> Result<()>;

    /// Subscribe to a topic
    ///
    /// INTENTION: Register a callback that will be invoked when events are published 
    /// to the specified topic. The callback receives both an EventContext and the event data.
    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String>;

    /// Subscribe to a topic with options
    ///
    /// INTENTION: Register a callback with specific delivery options for the subscription.
    /// The callback receives both an EventContext and the event data.
    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String>;

    /// Unsubscribe from a topic
    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()>;

    /// List all services
    /// 
    /// This method is provided in the trait to allow clients to discover
    /// available services without needing to know the internal structure
    /// of the service registry. It provides a simple way to enumerate
    /// the services available for requests.
    /// 
    /// In more complex implementations, this could be expanded to include
    /// filtering by service type, status, or other attributes.
    fn list_services(&self) -> Vec<String>;
}

/// Helper trait to enable easy logging from an Arc<RequestContext>
pub trait ArcContextLogging {
    /// Log a debug level message
    fn debug(&self, message: impl Into<String>);
    
    /// Log an info level message
    fn info(&self, message: impl Into<String>);
    
    /// Log a warning level message
    fn warn(&self, message: impl Into<String>);
    
    /// Log an error level message
    fn error(&self, message: impl Into<String>);
}

/// Implement logging methods for Arc<RequestContext>
impl ArcContextLogging for Arc<RequestContext> {
    fn debug(&self, message: impl Into<String>) {
        self.logger.debug(message);
    }
    
    fn info(&self, message: impl Into<String>) {
        self.logger.info(message);
    }
    
    fn warn(&self, message: impl Into<String>) {
        self.logger.warn(message);
    }
    
    fn error(&self, message: impl Into<String>) {
        self.logger.error(message);
    }
}

/// Type alias for a boxed future returning a Result with ServiceResponse
///
/// INTENTION: Provide a consistent return type for asynchronous service methods,
/// simplifying method signatures and ensuring uniformity across the codebase.
pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>>;

/// Event Dispatcher trait
///
/// INTENTION: Define a consistent interface for publishing events from services.
/// This trait is implemented by components that need to dispatch events,
/// specifically the Node component which should be responsible for publishing
/// events to subscribers.
///
/// ARCHITECTURAL PRINCIPLE:
/// Event publishing should be centralized in the Node component, with a clear
/// interface that allows for proper topic-based routing and delivery to subscribers.
/// This separates the responsibility of event publishing from service management.
#[async_trait::async_trait]
pub trait EventDispatcher: Send + Sync {
    /// Publish an event to subscribers
    ///
    /// INTENTION: Distribute an event to all subscribers of the specified topic.
    /// Implementations should handle subscriber lookups and asynchronous delivery.
    ///
    /// Example implementation (simplified):
    /// ```
    /// # use runar_node::services::{EventDispatcher, ServiceRequest, ServiceResponse, SubscriptionOptions, EventContext, RequestContext};
    /// # use runar_common::types::ValueType;
    /// # use runar_common::logging::{Logger, Component};
    /// # use runar_node::routing::TopicPath;
    /// # use anyhow::Result;
    /// # use async_trait::async_trait;
    /// # use std::future::Future;
    /// # use std::pin::Pin;
    /// # use std::sync::Arc;
    /// 
    /// struct Node {
    ///     // Node state...
    ///     network_id: String,
    /// }
    /// 
    /// type SubscriberCallback = Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;
    /// struct Subscriber {
    ///     id: String,
    ///     callback: SubscriberCallback,
    /// }
    /// 
    /// #[async_trait]
    /// impl EventDispatcher for Node {
    ///     async fn publish(&self, topic: &str, event: &str, data: Option<ValueType>, network_id: &str) -> Result<()> {
    ///         // Find subscribers for the topic
    ///         let subscribers: Vec<Subscriber> = Vec::new(); // Would fetch from a storage
    ///         
    ///         // Deliver the event to each subscriber
    ///         for subscriber in subscribers {
    ///             let data = data.clone().unwrap_or(ValueType::Null);
    ///             let logger = Logger::new_root(Component::Service, network_id);
    ///             
    ///             // Create a topic path from the topic and event
    ///             let full_topic = format!("{}/{}", topic, event);
    ///             let topic_path = TopicPath::new(&full_topic, network_id).expect("Valid topic path");
    ///             
    ///             // Create event context with the topic path
    ///             let event_context = Arc::new(EventContext::new(&topic_path, logger));
    ///             
    ///             // Call the subscriber's callback
    ///             let future = (subscriber.callback)(event_context, data);
    ///             future.await?;
    ///         }
    ///         
    ///         Ok(())
    ///     }
    /// }
    /// ```
    async fn publish(&self, topic: &str, event: &str, data: Option<ValueType>, network_id: &str) -> Result<()>;
}

/// Unified Node Delegate trait 
///
/// INTENTION: Provide a single interface for all Node operations that services
/// need to interact with, combining both request handling and event dispatching.
///
/// ARCHITECTURAL PRINCIPLE:
/// A single unified interface simplifies service interactions with the Node,
/// reducing code duplication and complexity while maintaining clear architectural
/// boundaries. This follows the Interface Segregation Principle by providing
/// a focused, cohesive interface for services.
#[async_trait::async_trait]
pub trait NodeDelegate: Send + Sync {
    /// Process a service request
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse>;
 
    /// Simplified publish for common cases
    async fn publish(&self, topic: String, data: ValueType) -> Result<()>;

    /// Subscribe to a topic
    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String>;

    /// Subscribe to a topic with options
    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String>;

    /// Unsubscribe from a topic
    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()>;

    /// List all services
    fn list_services(&self) -> Vec<String>;
    
    /// Register an action handler for a specific path
    ///
    /// INTENTION: Allow services to register handlers for actions through the NodeDelegate.
    /// This consolidates all node interactions through a single interface.
    async fn register_action_handler(&self, topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>) -> Result<()>;
}

/// Registry Delegate trait for registry service operations
///
/// INTENTION: Provide a dedicated interface for the Registry Service
/// to interact with the Node without creating circular references.
/// This follows the same pattern as NodeDelegate but provides only
/// the functionality needed by registry operations.
#[async_trait::async_trait]
pub trait RegistryDelegate: Send + Sync {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState>;
    
    /// Get metadata for a specific service
    async fn get_service_metadata(&self, service_path: &str) -> Option<CompleteServiceMetadata>;
    
    /// Get metadata for all registered services in a single call
    async fn get_all_service_metadata(&self) -> HashMap<String, CompleteServiceMetadata>;
    
    /// List all services
    fn list_services(&self) -> Vec<String>;
} 