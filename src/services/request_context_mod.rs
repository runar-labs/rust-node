// This is a dummy file for storing modifications

// RequestContext Implementation with path_params
use std::fmt;
use std::sync::Arc;
use std::collections::HashMap;
use anyhow::{Result, anyhow};
use crate::routing::TopicPath;
use crate::node::NodeDelegate;
use crate::logging::{Logger, LoggingContext, Component};
use runar_common::types::ValueType;
use crate::services::ServiceResponse;

/// Context for handling service requests
///
/// INTENTION: Provide context information for action handlers, allowing them
/// to access network information, logging, and perform operations like
/// making other service requests.
///
/// ARCHITECTURAL PRINCIPLE:
/// Each request should have its own isolated context that moves with the
/// request through the entire processing pipeline, ensuring proper tracing
/// and consistent handling.
pub struct RequestContext {
    /// Network ID for this request - ensures network isolation
    pub network_id: String,
    /// Service path for the service handling this request - target service path
    pub service_path: String,
    /// Complete topic path for this request (optional) - includes service path and action
    pub topic_path: Option<TopicPath>,
    /// Metadata for this request - additional contextual information
    pub metadata: Option<ValueType>,
    /// Logger for this context - pre-configured with the appropriate component and path
    pub logger: Logger,
    /// Path parameters extracted from template matching
    pub path_params: HashMap<String, String>,
}

// Manual implementation of Debug for RequestContext
impl fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestContext")
            .field("network_id", &self.network_id)
            .field("service_path", &self.service_path)
            .field("topic_path", &self.topic_path)
            .field("metadata", &self.metadata)
            .field("logger", &"<Logger>") // Avoid trying to Debug the Logger
            .field("path_params", &self.path_params)
            .finish()
    }
}

// Manual implementation of Clone for RequestContext
impl Clone for RequestContext {
    fn clone(&self) -> Self {
        Self {
            network_id: self.network_id.clone(),
            service_path: self.service_path.clone(),
            topic_path: self.topic_path.clone(),
            metadata: self.metadata.clone(),
            logger: self.logger.clone(),
            path_params: self.path_params.clone(),
        }
    }
}

// Manual implementation of Default for RequestContext
impl Default for RequestContext {
    fn default() -> Self {
        panic!("RequestContext should not be created with default. Use new instead");
    }
}

/// Constructors follow the builder pattern principle:
/// - Prefer a single primary constructor with required parameters
/// - Use builder methods for optional parameters
/// - Avoid creating specialized constructors for every parameter combination
impl RequestContext {
    /// Create a new RequestContext with the given logger
    ///
    /// This is the primary constructor that takes the minimum required parameters.
    pub fn new(network_id: &str, service_path: &str, logger: Logger) -> Self {
        // Create topic path for the service path
        let topic_path = TopicPath::new(service_path, network_id).ok();
        
        Self {
            network_id: network_id.to_string(),
            service_path: service_path.to_string(),
            topic_path,
            metadata: None,
            logger,
            path_params: HashMap::new(),
        }
    }
    
    /// Create a new RequestContext with a TopicPath
    ///
    /// Used when a validated TopicPath already exists.
    pub fn new_with_topic_path(network_id: &str, topic_path: &TopicPath, logger: Logger) -> Self {
        let service_path = topic_path.service_path();
        
        Self {
            network_id: network_id.to_string(),
            service_path,
            topic_path: Some(topic_path.clone()),
            metadata: None,
            logger,
            path_params: HashMap::new(),
        }
    }
    
    /// Add metadata to a RequestContext
    ///
    /// Use builder-style methods instead of specialized constructors.
    pub fn with_metadata(mut self, metadata: ValueType) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Helper method to log debug level message
    ///
    /// INTENTION: Provide a convenient way to log debug messages with the
    /// context's logger, without having to access the logger directly.
    pub fn debug(&self, message: impl Into<String>) {
        self.logger.debug(message);
    }
    
    /// Helper method to log info level message
    ///
    /// INTENTION: Provide a convenient way to log info messages with the
    /// context's logger, without having to access the logger directly.
    pub fn info(&self, message: impl Into<String>) {
        self.logger.info(message);
    }
    
    /// Helper method to log warning level message
    ///
    /// INTENTION: Provide a convenient way to log warning messages with the
    /// context's logger, without having to access the logger directly.
    pub fn warn(&self, message: impl Into<String>) {
        self.logger.warn(message);
    }
    
    /// Helper method to log error level message
    ///
    /// INTENTION: Provide a convenient way to log error messages with the
    /// context's logger, without having to access the logger directly.
    pub fn error(&self, message: impl Into<String>) {
        self.logger.error(message);
    }
}

impl LoggingContext for RequestContext {
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
