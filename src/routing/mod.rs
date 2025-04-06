// Routing Module
//
// INTENTION:
// This module provides the core routing components for the node's service architecture,
// establishing the message addressing and topic-based communication system. It defines
// how services are addressed, discovered, and how messages are routed between them.
//
// ARCHITECTURAL PRINCIPLES:
// 1. Path-based Addressing - Services are addressed using hierarchical paths
// 2. Topic-based Communication - Events use topics for publish-subscribe pattern
// 3. Separation of Concerns - Routing is separate from service implementation
// 4. Deterministic Resolution - Path resolution follows consistent rules
// 5. Network Isolation - Routing respects network boundaries
//
// The routing system is a foundational layer that enables the service architecture
// to function in a decentralized, loosely-coupled manner.

use anyhow::{anyhow, Result};

/// Type of a path, indicating what kind of resource is being addressed
///
/// INTENTION: Clearly distinguish between different types of routing targets
/// to ensure proper handling of each type. This enables the system to apply
/// different routing rules and validations based on the path type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathType {
    /// A path to a service - used for service discovery and registration
    Service,
    /// A path to an action on a service - used for request routing
    Action,
    /// A path to an event - used for event routing
    Event,
}

/// Path representation with network ID, service path, and action/event
///
/// INTENTION: Provide a structured representation of a path string,
/// enabling proper splitting of parts and validation of format.
///
/// A TopicPath can be in several formats:
/// - `main:auth/login` (Full path with network_id, service_path, and action)
/// - `auth/login` (Shorthand without network_id, which will be added when TopicPath is created)
/// - `login` (When used in a service context, can be just the action name)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopicPath {
    /// The raw path string with validated format
    path: String,
    /// The network ID for this path
    network_id: String,
    /// The service name (first segment of the path)
    service_path: String,
    /// The full path after network ID (including service and action)
    full_path: String,
    /// The action or event (last segment of the path)
    action_or_event: Option<String>,
}

impl TopicPath {
    /// Create a new TopicPath from a string
    ///
    /// INTENTION: Validate and construct a TopicPath from a string input,
    /// ensuring it follows the required format conventions.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// // With network_id prefix
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.network_id(), "main");
    /// assert_eq!(path.service_path(), "auth"); // Note: just the service name
    /// assert_eq!(path.get_last_segment(), "login");
    ///
    /// // Without network_id (service path only)
    /// let default_network_id = "default";
    /// let service_path = TopicPath::new("auth/login", default_network_id).expect("Valid path");
    /// // Network ID will be set to the default value
    /// assert_eq!(service_path.service_path(), "auth");
    /// ```
    pub fn new(path: &str, default_network: &str) -> Result<Self, String> {
        // Check if we have a network_id prefix
        if path.contains(':') {
            let parts: Vec<&str> = path.split(':').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid path format - should be 'network_id:service_path' or 'service_path': {}", path));
            }
            
            let network_id = parts[0].to_string();
            let remainder = parts[1].to_string();
            
            if network_id.is_empty() {
                return Err("Network ID cannot be empty".to_string());
            }
            
            // Split the topic string into service path and action/event
            let path_parts: Vec<&str> = remainder.split('/').filter(|s| !s.is_empty()).collect();
            if path_parts.is_empty() {
                return Err("Topic must have at least a service path".to_string());
            }
            
            // Store just the service name (first segment)
            let service_path = path_parts[0].to_string();
            // Store the full path after network ID
            let full_path = remainder.to_string();
            
            let action_or_event = if path_parts.len() > 1 {
                Some(path_parts[path_parts.len() - 1].to_string())
            } else {
                None
            };
            
            let full_path_str = format!("{}:{}", network_id, full_path);
            
            return Ok(Self {
                path: full_path_str,
                network_id,
                service_path,
                full_path,
                action_or_event,
            });
        }
        
        // No network_id prefix - treat as service path with default network
        // If topic already has a network prefix, error out
        if path.contains(':') {
            return Err(format!("Topic already contains network ID prefix: {}", path));
        }
        
        // Split the topic string into service path and action/event
        let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            return Err("Topic must have at least a service path".to_string());
        }
        
        // Store just the service name (first segment)
        let service_path = parts[0].to_string();
        // Store the full path
        let full_path = path.to_string();
        
        let action_or_event = if parts.len() > 1 {
            Some(parts[parts.len() - 1].to_string())
        } else {
            None
        };
        
        let full_path_str = format!("{}:{}", default_network, full_path);
        
        Ok(Self {
            path: full_path_str,
            network_id: default_network.to_string(),
            service_path,
            full_path,
            action_or_event,
        })
    }

    /// Get the path after the network ID
    ///
    /// INTENTION: Get the complete path after the network ID, including
    /// all segments. This is useful when you need the path for routing.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.action_path(), "auth/login");
    /// 
    /// // When there is only a service with no action, returns empty string
    /// let service_only = TopicPath::new("main:auth", "default").expect("Valid path");
    /// assert_eq!(service_only.action_path(), "");
    /// ```
    pub fn action_path(&self) -> String {
        // If there's an action or event (more than just the service), return the full path
        if self.action_or_event.is_some() {
            return self.full_path.clone();
        }
        
        // If there's only a service name, return empty string
        "".to_string()
    }

    /// Get the path as a string
    ///
    /// INTENTION: Access the raw path string for this TopicPath,
    /// useful for display and logging purposes.
    pub fn as_str(&self) -> &str {
        &self.path
    }

    /// Get the network ID from this path
    ///
    /// INTENTION: Extract just the network segment from the path,
    /// which is crucial for network isolation and routing.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.network_id(), "main");
    /// ```
    pub fn network_id(&self) -> String {
        self.network_id.clone()
    }

    /// Get the service path part of this path
    ///
    /// INTENTION: Extract the service path from the full path,
    /// which is used for service lookup and addressing.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// // The service_path method returns just the service name (first segment)
    /// assert_eq!(path.service_path(), "auth");
    /// 
    /// // If you need just the first segment, use get_segments
    /// let segments = path.get_segments();
    /// assert_eq!(segments[0], "auth");
    /// ```
    pub fn service_path(&self) -> String {
        // Extract the first segment of the path - the service name
        let segments = self.get_segments();
        if segments.is_empty() {
            return String::new();
        }
        segments[0].clone()
    }

    /// Create a new service path from a network ID and service name
    ///
    /// INTENTION: Create a TopicPath specifically for a service,
    /// avoiding manual string concatenation and validation.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new_service("main", "auth");
    /// assert_eq!(path.as_str(), "main:auth");
    /// ```
    pub fn new_service(network_id: &str, service_name: &str) -> Self {
        let path = format!("{}:{}", network_id, service_name);
        Self {
            path,
            network_id: network_id.to_string(),
            service_path: service_name.to_string(),
            full_path: service_name.to_string(),
            action_or_event: None,
        }
    }

    /// Check if this path starts with another path
    ///
    /// INTENTION: Determine if this path is a "child" or more specific
    /// version of another path, useful for hierarchical routing and
    /// subscription matching.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/users", "default").expect("Valid path");
    /// let prefix = TopicPath::new("main:auth", "default").expect("Valid path");
    /// let different = TopicPath::new("main:payments", "default").expect("Valid path");
    ///
    /// assert!(path.starts_with(&prefix));
    /// assert!(!path.starts_with(&different));
    /// ```
    pub fn starts_with(&self, other: &TopicPath) -> bool {
        self.network_id == other.network_id && 
        self.service_path.starts_with(&other.service_path)
    }

    /// Create a child path by appending a segment
    ///
    /// INTENTION: Generate a new TopicPath that is a child of this path,
    /// useful for creating more specific paths from a base path.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let base = TopicPath::new("main:auth", "default").expect("Valid path");
    /// let child = base.child("login").expect("Valid child path");
    ///
    /// assert_eq!(child.as_str(), "main:auth/login");
    /// ```
    pub fn child(&self, segment: &str) -> Result<Self, String> {
        // Ensure segment doesn't contain slashes
        if segment.contains('/') {
            return Err(format!("Child segment cannot contain slashes: {}", segment));
        }

        let new_full_path = if self.full_path.is_empty() {
            segment.to_string()
        } else {
            format!("{}/{}", self.full_path, segment)
        };
        
        let new_path = format!("{}:{}", self.network_id, new_full_path);
        
        Ok(Self {
            path: new_path.clone(),
            network_id: self.network_id.clone(),
            service_path: self.service_path.clone(),
            full_path: new_full_path,
            action_or_event: Some(segment.to_string()),
        })
    }

    /// Get the path segments
    ///
    /// INTENTION: Get the segments of the path after the network ID.
    /// This is useful for path analysis and custom path parsing.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login/form", "default").expect("Valid path");
    /// let segments = path.get_segments();
    /// assert_eq!(segments, vec!["auth", "login", "form"]);
    /// ```
    pub fn get_segments(&self) -> Vec<String> {
        self.full_path.split('/')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    }

    /// Create a parent path
    ///
    /// INTENTION: Generate a new TopicPath that is the parent of this path,
    /// useful for navigating up the path hierarchy.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/users", "default").expect("Valid path");
    /// let parent = path.parent().expect("Valid parent path");
    ///
    /// assert_eq!(parent.as_str(), "main:auth");
    /// ```
    pub fn parent(&self) -> Result<Self, String> {
        let segments: Vec<&str> = self.full_path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() <= 1 {
            return Err("Cannot get parent of root or service-only path".to_string());
        }

        let parent_full_path = segments[..segments.len() - 1].join("/");
        let parent_path = format!("{}:{}", self.network_id, parent_full_path);
        
        Ok(Self {
            path: parent_path,
            network_id: self.network_id.clone(),
            service_path: self.service_path.clone(),
            full_path: parent_full_path,
            action_or_event: None,
        })
    }

    /// Create a default path for running unit tests with default network ID
    ///
    /// INTENTION: Simplify test setup by creating a valid path with a default network ID.
    ///
    /// Note: This is primarily intended for testing purposes.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::test_default("auth/login");
    /// assert_eq!(path.as_str(), "default:auth/login");
    /// ```
    pub fn test_default(path: &str) -> Self {
        Self::new(path, "default").unwrap()
    }

    /// Get the last segment of the path
    ///
    /// INTENTION: Extract just the last segment from the path,
    /// which represents the action or event name. This is helpful
    /// for identifying what operation is being requested.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.get_last_segment(), "login");
    /// ```
    pub fn get_last_segment(&self) -> String {
        // If we have an action_or_event, return it
        if let Some(action) = &self.action_or_event {
            return action.clone();
        }
        
        // Otherwise, return the service path as it's the only segment
        self.service_path.clone()
    }

    /// Match this path against a template pattern and extract parameters
    ///
    /// INTENTION: Enable services to define URL-like templates with named parameters
    /// and extract those parameter values from actual request paths.
    ///
    /// Template patterns can include segments like `{param_name}` which will match
    /// any value in that position and capture it with the specified name.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let template = "services/{service_path}/state";
    /// let path = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    /// 
    /// let params = path.extract_params(template).expect("Template should match");
    /// assert_eq!(params.get("service_path"), Some(&"math".to_string()));
    /// 
    /// // Non-matching templates return an error
    /// let non_matching = TopicPath::new("main:users/profile", "default").expect("Valid path");
    /// assert!(non_matching.extract_params(template).is_err());
    /// ```
    pub fn extract_params(&self, template: &str) -> Result<std::collections::HashMap<String, String>, String> {
        let mut params = std::collections::HashMap::new();
        
        // Get segments from the actual path (excluding network_id)
        let path_segments = self.get_segments();
        
        // Get segments from the template
        let template_segments: Vec<&str> = template.split('/')
            .filter(|s| !s.is_empty())
            .collect();
            
        // If segment counts don't match, the template doesn't apply
        if path_segments.len() != template_segments.len() {
            return Err(format!(
                "Path segment count ({}) doesn't match template segment count ({})",
                path_segments.len(), 
                template_segments.len()
            ));
        }
        
        // Match each segment and extract parameters
        for (i, template_segment) in template_segments.iter().enumerate() {
            if template_segment.starts_with('{') && template_segment.ends_with('}') {
                // This is a parameter segment - extract the parameter name
                let param_name = template_segment[1..template_segment.len()-1].to_string();
                // Store the actual value from the path
                params.insert(param_name, path_segments[i].clone());
            } else if template_segment != &path_segments[i] {
                // This is a literal segment and it doesn't match
                return Err(format!(
                    "Path segment '{}' doesn't match template segment '{}'",
                    path_segments[i], 
                    template_segment
                ));
            }
            // If it's a literal and matches, continue to next segment
        }
        
        Ok(params)
    }

    /// Check if this path matches a template pattern
    ///
    /// INTENTION: Quickly determine if a path matches a template pattern
    /// without extracting parameters. Useful for route matching.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    ///
    /// let template = "services/{service_path}/state";
    /// let path = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    /// 
    /// assert!(path.matches_template(template));
    /// 
    /// let non_matching = TopicPath::new("main:users/profile", "default").expect("Valid path");
    /// assert!(!non_matching.matches_template(template));
    /// ```
    pub fn matches_template(&self, template: &str) -> bool {
        self.extract_params(template).is_ok()
    }

    /// Create a new TopicPath by filling in a template with parameters
    ///
    /// INTENTION: Provide a way to generate a path from a template and parameters,
    /// useful for constructing specific paths based on a pattern.
    ///
    /// Example:
    /// ```
    /// use runar_node_new::routing::TopicPath;
    /// use std::collections::HashMap;
    ///
    /// let template = "services/{service_path}/state";
    /// let mut params = HashMap::new();
    /// params.insert("service_path".to_string(), "math".to_string());
    ///
    /// let path = TopicPath::from_template(template, params, "main").expect("Valid template");
    /// assert_eq!(path.as_str(), "main:services/math/state");
    /// ```
    pub fn from_template(
        template: &str, 
        params: std::collections::HashMap<String, String>,
        network_id: &str
    ) -> Result<Self, String> {
        // Get segments from the template
        let template_segments: Vec<&str> = template.split('/')
            .filter(|s| !s.is_empty())
            .collect();
            
        let mut path_segments = Vec::new();
        
        // Process each template segment
        for template_segment in template_segments.iter() {
            if template_segment.starts_with('{') && template_segment.ends_with('}') {
                // This is a parameter segment - extract the parameter name
                let param_name = &template_segment[1..template_segment.len()-1];
                
                // Look up the parameter value
                match params.get(param_name) {
                    Some(value) => path_segments.push(value.clone()),
                    None => return Err(format!("Missing parameter value for '{}'", param_name)),
                }
            } else {
                // This is a literal segment
                path_segments.push(template_segment.to_string());
            }
        }
        
        // Construct the final path
        let path_str = path_segments.join("/");
        Self::new(&path_str, network_id)
    }
} 