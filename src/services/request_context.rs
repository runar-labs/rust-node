// RequestContext Module
//
// INTENTION:
// This module provides the implementation of RequestContext, which encapsulates
// all contextual information needed to process service requests, including
// network identity, service path, and metadata.
//
// ARCHITECTURAL PRINCIPLE:
// Each request should have its own isolated context that moves with the
// request through the entire processing pipeline, ensuring proper tracing
// and consistent handling. The context avoids data duplication by
// deriving values from the TopicPath when needed.

use std::fmt;
use std::sync::Arc;
use std::collections::HashMap;
use anyhow::{Result, anyhow};
use crate::routing::TopicPath;
use crate::services::NodeDelegate;
use runar_common::logging::{Logger, LoggingContext, Component};
use runar_common::types::ValueType;
use crate::services::ServiceResponse;

/// Context for handling service requests
///
/// INTENTION: Encapsulate all contextual information needed to process
/// a service request, including network identity, service path, and metadata.
/// This ensures consistent request processing and proper logging.
///
/// The RequestContext is immutable and is passed with each request to provide:
/// - Network isolation (via network_id derived from topic_path)
/// - Service targeting (via service_path derived from topic_path)
/// - Request metadata and contextual information
/// - Logging capabilities with consistent context
///
/// ARCHITECTURAL PRINCIPLE:
/// Each request should have its own isolated context that moves with the
/// request through the entire processing pipeline, ensuring proper tracing
/// and consistent handling.
pub struct RequestContext {
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
            .field("network_id", &self.network_id())
            .field("service_path", &self.service_path())
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
    /// Create a new RequestContext with a TopicPath and logger
    ///
    /// This is the primary constructor that takes the minimum required parameters.
    pub fn new(topic_path: &TopicPath, logger: Logger) -> Self {
        Self {
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

    /// Get the network ID from the topic path
    pub fn network_id(&self) -> String {
        if let Some(topic_path) = &self.topic_path {
            topic_path.network_id()
        } else {
            "unknown".to_string()
        }
    }

    /// Get the service path from the topic path
    pub fn service_path(&self) -> String {
        if let Some(topic_path) = &self.topic_path {
            topic_path.service_path()
        } else {
            "unknown".to_string()
        }
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
        if let Some(topic_path) = &self.topic_path {
            let path = topic_path.service_path();
            Some(Box::leak(path.into_boxed_str()))
        } else {
            None
        }
    }
    
    fn logger(&self) -> &Logger {
        &self.logger
    }
}
