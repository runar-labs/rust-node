// EventContext Module
//
// INTENTION:
// This module provides the implementation of EventContext, which encapsulates
// all contextual information needed to process service events, including
// the event topic, logging capabilities, and access to node functionality.
//
// ARCHITECTURAL PRINCIPLE:
// Each event handler should have its own isolated context that provides
// the necessary information and functionality to process the event,
// while maintaining proper isolation between events.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use anyhow::{Result, anyhow};
use crate::routing::TopicPath;
use crate::services::NodeDelegate;
use runar_common::logging::{Logger, LoggingContext, Component};
use runar_common::types::ValueType;
use crate::services::{ServiceResponse, PublishOptions};

/// Context for handling events
///
/// INTENTION: Provide context information for event handlers, allowing them
/// to access event details, logging, and perform operations like
/// publishing other events or making service requests.
///
/// ARCHITECTURAL PRINCIPLE:
/// Event handlers should have access to necessary context for performing 
/// their operations, but with appropriate isolation and scope control.
/// The EventContext provides a controlled interface to the node's capabilities.
pub struct EventContext {
    /// Complete topic path for this event
    pub topic_path: TopicPath,
    
    /// Logger instance specific to this context
    pub logger: Logger,
    
    /// Node delegate for making requests or publishing events
    pub(crate) node_delegate: Option<Arc<dyn NodeDelegate + Send + Sync>>,
    
    /// Delivery options used when publishing this event
    pub delivery_options: Option<PublishOptions>,
}

impl fmt::Debug for EventContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventContext")
            .field("topic_path", &self.topic_path)
            .field("logger", &"<Logger>") // Avoid trying to Debug the Logger
            .field("node_delegate", &self.node_delegate.is_some())
            .field("delivery_options", &self.delivery_options)
            .finish()
    }
}

// Manual implementation of Clone for EventContext
impl Clone for EventContext {
    fn clone(&self) -> Self {
        panic!("EventContext should not be cloned directly. Use new instead");
    }
}

// Manual implementation of Default for EventContext
impl Default for EventContext {
    fn default() -> Self {
        panic!("EventContext should not be created with default. Use new instead");
    }
}

/// Constructors follow the builder pattern principle:
/// - Prefer a single primary constructor with required parameters 
/// - Use builder methods for optional parameters
/// - Avoid creating specialized constructors for every parameter combination
impl EventContext {
    /// Create a new EventContext with the given topic path and logger
    ///
    /// This is the primary constructor that takes the minimum required parameters.
    pub fn new(topic_path: &TopicPath, logger: Logger) -> Self {
        Self {
            topic_path: topic_path.clone(),
            logger,
            node_delegate: None,
            delivery_options: None,
        }
    }
    
    /// Legacy constructor for backward compatibility
    pub fn new_from_path(network_id: &str, topic: &str, service_path: &str, logger: Logger) -> Self {
        // Create a path string combining service path and topic
        let path_string = format!("{}/{}", service_path, topic);
        
        // Create a TopicPath
        let topic_path = TopicPath::new(&path_string, network_id)
            .expect("Invalid path format");
            
        Self {
            topic_path,
            logger,
            node_delegate: None,
            delivery_options: None,
        }
    }
    
    /// Add node delegate to an EventContext
    ///
    /// Used to make service requests from within an event handler.
    pub fn with_node_delegate(mut self, delegate: Arc<dyn NodeDelegate + Send + Sync>) -> Self {
        self.node_delegate = Some(delegate);
        self
    }
    
    /// Add delivery options to an EventContext
    ///
    /// Used to specify how this event was delivered.
    pub fn with_delivery_options(mut self, options: PublishOptions) -> Self {
        self.delivery_options = Some(options);
        self
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
    
    /// Publish an event
    ///
    /// INTENTION: Allow event handlers to publish their own events.
    /// This method provides a convenient way to publish events from within
    /// an event handler.
    pub async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        if let Some(delegate) = &self.node_delegate {
            delegate.publish(topic, data).await
        } else {
            Err(anyhow!("No node delegate available in this context"))
        }
    }
    
    /// Make a service request
    ///
    /// INTENTION: Allow event handlers to make requests to other services.
    /// This method provides a convenient way to call service actions from
    /// within an event handler.
    pub async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        if let Some(delegate) = &self.node_delegate {
            delegate.request(path, params).await
        } else {
            Err(anyhow!("No node delegate available in this context"))
        }
    }
}

impl LoggingContext for EventContext {
    fn component(&self) -> Component {
        Component::Service
    }
    
    fn service_path(&self) -> Option<&str> {
        // Convert the owned String to a string slice that lives as long as self
        let path = self.topic_path.service_path();
        Some(Box::leak(path.into_boxed_str()))
    }
    
    fn logger(&self) -> &Logger {
        &self.logger
    }
} 