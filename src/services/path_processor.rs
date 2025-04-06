// Path Processor Module
//
// INTENTION:
// This module provides helper functions for processing path templates and extracting
// parameters from paths, to be used with the TopicPath template matching functionality.

use std::collections::HashMap;
use anyhow::Result;
use crate::routing::TopicPath;
use crate::services::RequestContext;

/// Process a request path against a template
/// 
/// INTENTION: Extract parameters from a path based on a template pattern,
/// and add them to the request context. This allows handlers to easily access
/// named parameters extracted from the path.
///
/// Example:
/// ```
/// use runar_node::services::path_processor::process_template;
/// use runar_node::services::RequestContext;
/// use runar_node::routing::TopicPath;
/// use runar_common::logging::{Logger, Component};
/// 
/// // Create a logger for the context
/// let logger = Logger::new_root(Component::Service, "test-node");
/// 
/// // Create a topic path
/// let topic_path = TopicPath::new("services/math/state", "default").unwrap();
/// 
/// // Create request context
/// let mut context = RequestContext::new(&topic_path, logger);
/// 
/// // Process template and extract parameters
/// if process_template(&mut context, "services/{service_path_param}/state").unwrap() {
///     // Path matched template, parameters extracted
///     let service_path = context.path_params.get("service_path_param").unwrap();
///     assert_eq!(service_path, "math");
/// }
/// ```
pub fn process_template(
    context: &mut RequestContext, 
    template: &str
) -> Result<bool> {
    // Extract parameters if we have a topic path
    if let Some(topic_path) = &context.topic_path {
        // Check if the path matches the template
        match topic_path.extract_params(template) {
            Ok(params) => {
                // Add params to context
                context.path_params = params;
                Ok(true)
            },
            Err(_) => Ok(false),
        }
    } else {
        Ok(false)
    }
}

/// Check if a request context's path matches a template
/// 
/// INTENTION: Quickly determine if a request's path matches a template pattern
/// without extracting parameters. This is useful for routing decisions.
pub fn matches_template(context: &RequestContext, template: &str) -> bool {
    if let Some(topic_path) = &context.topic_path {
        topic_path.matches_template(template)
    } else {
        false
    }
} 