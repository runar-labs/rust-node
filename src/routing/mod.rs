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

use anyhow::Result;
use std::hash::{Hash, Hasher};
use std::collections::HashMap;

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

/// Represents a single segment in a path, which can be a literal or a wildcard
///
/// INTENTION: Allow paths to include wildcard patterns for flexible topic matching
/// in the publish-subscribe system. This enables powerful pattern-based subscriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSegment {
    /// A literal string segment (e.g., "services", "auth", "login")
    Literal(String),
    
    /// A single-segment wildcard (*) - matches any single segment
    /// Example: "services/*/state" matches "services/math/state" but not "services/auth/user/state"
    SingleWildcard,
    
    /// A multi-segment wildcard (>) - matches one or more segments to the end
    /// Example: "services/>" matches "services/math", "services/auth/login", etc.
    /// Must be the last segment in a pattern.
    MultiWildcard,
}

impl PathSegment {
    /// Parse a string segment into a PathSegment
    ///
    /// INTENTION: Convert raw string segments into the appropriate PathSegment variant,
    /// handling special wildcard characters.
    fn from_str(segment: &str) -> Self {
        match segment {
            "*" => Self::SingleWildcard,
            ">" => Self::MultiWildcard,
            _ => Self::Literal(segment.to_string()),
        }
    }
    
    /// Check if this segment is a wildcard
    ///
    /// INTENTION: Quickly determine if a segment is a wildcard without pattern matching.
    pub fn is_wildcard(&self) -> bool {
        matches!(self, Self::SingleWildcard | Self::MultiWildcard)
    }
    
    /// Check if this segment is a multi-segment wildcard
    ///
    /// INTENTION: Identify multi-segment wildcards which have special matching rules.
    pub fn is_multi_wildcard(&self) -> bool {
        matches!(self, Self::MultiWildcard)
    }
}

/// Path representation with network ID and segments, supporting wildcard patterns
///
/// INTENTION: Provide a structured representation of a path string,
/// enabling proper splitting of parts, validation of format, and wildcard matching.
///
/// A TopicPath can be in several formats:
/// - `main:auth/login` (Full path with network_id, service_path, and action)
/// - `auth/login` (Shorthand without network_id, which will be added when TopicPath is created)
/// - `services/*/state` (Pattern with single-segment wildcard)
/// - `events/>` (Pattern with multi-segment wildcard)
#[derive(Debug, Clone)]
pub struct TopicPath {
    /// The raw path string with validated format
    path: String,
    
    /// The network ID for this path
    network_id: String,
    
    /// The segments after the network ID
    segments: Vec<PathSegment>,
    
    /// Whether this path contains wildcard patterns
    is_pattern: bool,
    
    /// The service name (first segment of the path) - cached for convenience
    service_path: String,
}

// Implement Hash trait for TopicPath to enable use in HashMaps
impl Hash for TopicPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash the network ID
        self.network_id.hash(state);
        
        // Hash each segment with type discrimination
        for segment in &self.segments {
            match segment {
                PathSegment::Literal(s) => {
                    // Hash type code + string for literal segments
                    0.hash(state);
                    s.hash(state);
                }
                PathSegment::SingleWildcard => {
                    // Just hash type code for wildcards
                    1.hash(state);
                }
                PathSegment::MultiWildcard => {
                    // Just hash type code for wildcards
                    2.hash(state);
                }
            }
        }
    }
}

// Implement PartialEq for TopicPath to enable equality comparisons and HashMap lookups
impl PartialEq for TopicPath {
    fn eq(&self, other: &Self) -> bool {
        // Network IDs must match
        if self.network_id != other.network_id {
            return false;
        }
        
        // Segment counts must match
        if self.segments.len() != other.segments.len() {
            return false;
        }
        
        // Each segment must match exactly
        for (s1, s2) in self.segments.iter().zip(other.segments.iter()) {
            if s1 != s2 {
                return false;
            }
        }
        
        true
    }
}

// Implement Eq to confirm TopicPath can be used as HashMap keys
impl Eq for TopicPath {}

impl TopicPath {
    /// Creates a new TopicPath for an action based on this service path
    ///
    /// INTENTION: Provide a simple way to create an action path from a service path,
    /// ensuring consistent formatting and proper path type.
    ///
    /// Example:
    /// ```
    /// let service_path = TopicPath::new("main:auth", "default").expect("Valid path");
    /// let action_path = service_path.new_action_topic("login").expect("Valid action path");
    ///
    /// assert_eq!(action_path.action_path(), "auth/login");
    /// ```
    pub fn new_action_topic(&self, action_name: &str) -> Result<Self, String> {
        // Combine full service path with action name
        // If this is a nested path with multiple segments, we need to use all segments
        let service_path = if self.segments.len() > 1 {
            // Use the existing path (all segments except the last one if it's not just a service path)
            self.action_path()
        } else {
            // For simple service paths with just one segment
            self.service_path.clone()
        };
        
        // Create the full path with the action name
        let full_path = format!("{}:{}/{}", self.network_id, service_path, action_name);
        TopicPath::new(full_path, &self.network_id)
    }
    
    /// Creates a new TopicPath for an event based on this service path
    ///
    /// INTENTION: Provide a simple way to create an event path from a service path,
    /// ensuring consistent formatting and proper path type.
    ///
    /// Example:
    /// ```
    /// let service_path = TopicPath::new("main:auth", "default").expect("Valid path");
    /// let event_path = service_path.new_event_topic("user_logged_in").expect("Valid event path");
    ///
    /// assert_eq!(event_path.action_path(), "auth/user_logged_in");
    /// ```
    pub fn new_event_topic(&self, event_name: &str) -> Result<Self, String> {
        // For now, the format is the same as action_topic, but we'll mark it as an event
        let path = self.new_action_topic(event_name)?;
        
        // In the future, we might want to distinguish events from actions in the path format
        // For now, we'll just create a path with the event name
        Ok(path)
    }

    /// Create a new TopicPath from a string
    ///
    /// INTENTION: Validate and construct a TopicPath from a string input,
    /// ensuring it follows the required format conventions.
    ///
    /// This method now supports wildcard patterns:
    /// - "*" matches any single segment
    /// - ">" matches one or more segments to the end (must be the last segment)
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// // With network_id prefix
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.network_id(), "main");
    /// assert_eq!(path.service_path(), "auth");
    /// assert_eq!(path.action_path(), "auth/login");
    ///
    /// // With wildcards
    /// let pattern = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
    /// assert!(pattern.is_pattern());
    /// ```
    pub fn new(path: impl Into<String>, default_network: impl Into<String>) -> Result<Self, String> {
        let path_string = path.into();
        let default_network_string = default_network.into();
        
        // Parse the network ID and path parts
        let (network_id, path_without_network) = if path_string.contains(':') {
            // Split at the first colon to separate network_id and path
            let parts: Vec<&str> = path_string.split(':').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid path format - should be 'network_id:service_path' or 'service_path': {}", path_string));
            }
            
            let network = parts[0].to_string();
            if network.is_empty() {
                return Err("Network ID cannot be empty".to_string());
            }
            
            (network, parts[1].to_string())
        } else {
            // No network ID provided, use the default
            (default_network_string, path_string)
        };
        
        // Split the path into segments
        let path_segments: Vec<&str> = path_without_network.split('/')
            .filter(|s| !s.is_empty())
            .collect();
            
        if path_segments.is_empty() {
            return Err("Path must have at least a service path segment".to_string());
        }
        
        // Convert string segments to PathSegment enum variants
        let mut segments = Vec::new();
        let mut is_pattern = false;
        let mut _multi_wildcard_found = false;
        
        for (i, segment_str) in path_segments.iter().enumerate() {
            let segment = PathSegment::from_str(segment_str);
            
            // Check for pattern and validate multi-wildcards
            match &segment {
                PathSegment::SingleWildcard => {
                    is_pattern = true;
                }
                PathSegment::MultiWildcard => {
                    is_pattern = true;
                    _multi_wildcard_found = true;
                    
                    // Ensure multi-wildcard is the last segment
                    if i < path_segments.len() - 1 {
                        return Err("Multi-segment wildcard (>) must be the last segment in a path".to_string());
                    }
                }
                PathSegment::Literal(_) => {}
            }
            
            segments.push(segment);
        }
        
        // Save the service path (first segment) for easy access
        let service_path = match &segments[0] {
            PathSegment::Literal(s) => s.clone(),
            PathSegment::SingleWildcard => "*".to_string(),
            PathSegment::MultiWildcard => ">".to_string(),
        };
        
        // Create the full path string (for display/serialization)
        let full_path_str = format!("{}:{}", network_id, path_without_network);
        
        Ok(Self {
            path: full_path_str,
            network_id,
            segments,
            is_pattern,
            service_path,
        })
    }
    
    /// Check if this path contains wildcards
    ///
    /// INTENTION: Quickly determine if a path is a wildcard pattern,
    /// which affects how it's used for matching.
    pub fn is_pattern(&self) -> bool {
        self.is_pattern
    }
    
    /// Check if this path contains a multi-segment wildcard
    ///
    /// INTENTION: Identify paths with multi-segment wildcards which have 
    /// special matching rules and storage requirements.
    pub fn has_multi_wildcard(&self) -> bool {
        self.segments.iter().any(|s| s.is_multi_wildcard())
    }

    /// Get the path after the network ID
    ///
    /// INTENTION: Get the complete path after the network ID, including
    /// all segments. This is useful when you need the path for routing.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
    /// assert_eq!(path.action_path(), "auth/login");
    /// 
    /// // When there is only a service with no action, returns empty string
    /// let service_only = TopicPath::new("main:auth", "default").expect("Valid path");
    /// assert_eq!(service_only.action_path(), "");
    /// ```
    pub fn action_path(&self) -> String {
        if self.segments.len() <= 1 {
            // If there's only one segment (just the service), return empty string
            return "".to_string();
        }
        
        //FIX: this methdosnis called a lot.. so we need to calculat this once and store in a field and just return it here
        // Otherwise, reconstruct the path from segments
        self.segments.iter()
            .map(|segment| match segment {
                PathSegment::Literal(s) => s.as_str(),
                PathSegment::SingleWildcard => "*",
                PathSegment::MultiWildcard => ">",
            })
            .collect::<Vec<&str>>()
            .join("/")
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
    /// use runar_node::routing::TopicPath;
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
    /// use runar_node::routing::TopicPath;
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
    /// use runar_node::routing::TopicPath;
    ///
    /// let path = TopicPath::new_service("main", "auth");
    /// assert_eq!(path.as_str(), "main:auth");
    /// ```
    pub fn new_service(network_id: impl Into<String>, service_name: impl Into<String>) -> Self {
        let network_id_string = network_id.into();
        let service_name_string = service_name.into();
        
        let path = format!("{}:{}", network_id_string, service_name_string);
        Self {
            path,
            network_id: network_id_string,
            service_path: service_name_string.clone(),
            segments: vec![PathSegment::Literal(service_name_string)],
            is_pattern: false,
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
    /// use runar_node::routing::TopicPath;
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
    /// use runar_node::routing::TopicPath;
    ///
    /// let base = TopicPath::new("main:auth", "default").expect("Valid path");
    /// let child = base.child("login").expect("Valid child path");
    ///
    /// assert_eq!(child.as_str(), "main:auth/login");
    /// ```
    pub fn child(&self, segment: impl Into<String>) -> Result<Self, String> {
        let segment_string = segment.into();
        
        // Ensure segment doesn't contain slashes
        if segment_string.contains('/') {
            return Err(format!("Child segment cannot contain slashes: {}", segment_string));
        }
        
        // Get the service_path if this is the first segment, or build a new path
        let current_path_str = self.segments.iter()
            .map(|s| match s {
                PathSegment::Literal(s) => s.as_str(),
                PathSegment::SingleWildcard => "*",
                PathSegment::MultiWildcard => ">",
            })
            .collect::<Vec<&str>>()
            .join("/");
            
        // Create the new path
        let new_path_str = if current_path_str.is_empty() {
            segment_string.clone()
        } else {
            format!("{}/{}", current_path_str, segment_string)
        };
        
        // Create a full path with network ID
        let full_path_str = format!("{}:{}", self.network_id, new_path_str);
        
        // Create a new set of segments based on the current segments plus the new one
        let mut new_segments = self.segments.clone();
        new_segments.push(PathSegment::Literal(segment_string.clone()));
        
        // Check if this path is a pattern (has wildcards)
        let is_pattern = self.is_pattern || segment_string == "*" || segment_string == ">";
        
        Ok(Self {
            path: full_path_str,
            network_id: self.network_id.clone(),
            segments: new_segments,
            is_pattern,
            service_path: self.service_path.clone(),
        })
    }

    /// Get the path segments
    ///
    /// INTENTION: Get the segments of the path after the network ID.
    /// This is useful for path analysis and custom path parsing.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/login/form", "default").expect("Valid path");
    /// let segments = path.get_segments();
    /// assert_eq!(segments, vec!["auth", "login", "form"]);
    /// ```
    pub fn get_segments(&self) -> Vec<String> {
        self.segments.iter()
            .map(|segment| match segment {
                PathSegment::Literal(s) => s.clone(),
                PathSegment::SingleWildcard => "*".to_string(),
                PathSegment::MultiWildcard => ">".to_string(),
            })
            .collect()
    }

    /// Create a parent path
    ///
    /// INTENTION: Generate a new TopicPath that is the parent of this path,
    /// useful for navigating up the path hierarchy.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// let path = TopicPath::new("main:auth/users", "default").expect("Valid path");
    /// let parent = path.parent().expect("Valid parent path");
    ///
    /// assert_eq!(parent.as_str(), "main:auth");
    /// ```
    pub fn parent(&self) -> Result<Self, String> {
        if self.segments.len() <= 1 {
            return Err("Cannot get parent of root or service-only path".to_string());
        }

        // Create a new set of segments without the last segment
        let parent_segments = self.segments[0..self.segments.len() - 1].to_vec();
        
        // Reconstruct the path string from segments
        let path_str = parent_segments.iter()
            .map(|segment| match segment {
                PathSegment::Literal(s) => s.as_str(),
                PathSegment::SingleWildcard => "*",
                PathSegment::MultiWildcard => ">",
            })
            .collect::<Vec<&str>>()
            .join("/");
            
        // Create the full path with network ID
        let full_path = format!("{}:{}", self.network_id, path_str);
        
        Ok(Self {
            path: full_path,
            network_id: self.network_id.clone(),
            segments: parent_segments,
            is_pattern: self.segments[0..self.segments.len() - 1].iter().any(|s| s.is_wildcard()),
            service_path: match &self.segments[0] {
                PathSegment::Literal(s) => s.clone(),
                PathSegment::SingleWildcard => "*".to_string(),
                PathSegment::MultiWildcard => ">".to_string(),
            },
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
    /// use runar_node::routing::TopicPath;
    ///
    /// let path = TopicPath::test_default("auth/login");
    /// assert_eq!(path.as_str(), "default:auth/login");
    /// ```
    pub fn test_default(path: impl Into<String>) -> Self {
        Self::new(path, "default").unwrap()
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
    /// use runar_node::routing::TopicPath;
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

    /// Check if a path matches a template pattern
    ///
    /// INTENTION: Quickly determine if a path can be processed by a particular
    /// template, without needing to extract parameters.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// let template = "services/{service_path}/state";
    /// let path = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    /// 
    /// assert!(path.matches_template(template));
    /// 
    /// let non_matching = TopicPath::new("main:users/profile", "default").expect("Valid path");
    /// assert!(!non_matching.matches_template(template));
    /// ```
    pub fn matches_template(&self, template: impl Into<String>) -> bool {
        let template_string = template.into();
        self.extract_params(&template_string).is_ok()
    }

    /// Create a TopicPath from a template and parameter values
    ///
    /// INTENTION: Allow dynamic construction of TopicPaths from templates with
    /// parameter placeholders, similar to route templates in web frameworks.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    /// use std::collections::HashMap;
    ///
    /// let mut params = HashMap::new();
    /// params.insert("service_path".to_string(), "math".to_string());
    /// params.insert("action".to_string(), "add".to_string());
    ///
    /// let path = TopicPath::from_template(
    ///     "services/{service_path}/{action}", 
    ///     params,
    ///     "main"
    /// ).expect("Valid template");
    ///
    /// assert_eq!(path.as_str(), "main:services/math/add");
    /// ```
    pub fn from_template(
        template: impl Into<String>, 
        params: std::collections::HashMap<String, String>,
        network_id: impl Into<String>
    ) -> Result<Self, String> {
        let template_string = template.into();
        let network_id_string = network_id.into();
        
        // Get segments from the template
        let template_segments: Vec<&str> = template_string.split('/')
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
                    Some(value) => path_segments.push(PathSegment::Literal(value.clone())),
                    None => return Err(format!("Missing parameter value for '{}'", param_name)),
                }
            } else {
                // This is a literal segment
                path_segments.push(PathSegment::Literal(template_segment.to_string()));
            }
        }
        
        // Construct the final path
        let path_str = path_segments.iter()
            .map(|segment| match segment {
                PathSegment::Literal(s) => s.as_str(),
                PathSegment::SingleWildcard => "*",
                PathSegment::MultiWildcard => ">",
            })
            .collect::<Vec<&str>>()
            .join("/");
        Self::new(path_str, network_id_string)
    }

    /// Implement pattern matching against another path
    ///
    /// INTENTION: Allow checking if a topic matches a pattern with wildcards,
    /// enabling powerful pattern-based subscriptions.
    ///
    /// Example:
    /// ```
    /// use runar_node::routing::TopicPath;
    ///
    /// let pattern = TopicPath::new("main:services/*/state", "default").expect("Valid pattern");
    /// let topic1 = TopicPath::new("main:services/math/state", "default").expect("Valid topic");
    /// let topic2 = TopicPath::new("main:services/math/config", "default").expect("Valid topic");
    ///
    /// assert!(pattern.matches(&topic1));
    /// assert!(!pattern.matches(&topic2));
    /// ```
    pub fn matches(&self, topic: &TopicPath) -> bool {
        // Network ID must match
        if self.network_id != topic.network_id {
            return false;
        }
        
        // If this is not a pattern, use exact equality
        if !self.is_pattern {
            return self == topic;
        }
        
        // Otherwise, perform segment-by-segment matching
        self.segments_match(&self.segments, &topic.segments)
    }
    
    // Internal helper for pattern matching
    fn segments_match(&self, pattern_segments: &[PathSegment], topic_segments: &[PathSegment]) -> bool {
        // Special case: multi-wildcard at the end
        if let Some(PathSegment::MultiWildcard) = pattern_segments.last() {
            // If pattern ends with >, topic must have at least as many segments as pattern minus 1
            if topic_segments.len() < pattern_segments.len() - 1 {
                return false;
            }
            
            // Check all segments before the multi-wildcard
            for i in 0..pattern_segments.len()-1 {
                match &pattern_segments[i] {
                    PathSegment::Literal(p) => {
                        // For literals, the segments must match exactly
                        match &topic_segments[i] {
                            PathSegment::Literal(t) if p == t => continue,
                            _ => return false,
                        }
                    }
                    PathSegment::SingleWildcard => {
                        // For single wildcards, any segment matches
                        continue;
                    }
                    PathSegment::MultiWildcard => {
                        // Should not happen, as we're iterating up to len-1
                        unreachable!("Multi-wildcard found before the end of pattern");
                    }
                }
            }
            
            // If we get here, all segments matched
            return true;
        }
        
        // If pattern doesn't end with multi-wildcard, segment counts must match
        if pattern_segments.len() != topic_segments.len() {
            return false;
        }
        
        // Check each segment
        for (p, t) in pattern_segments.iter().zip(topic_segments.iter()) {
            match p {
                PathSegment::Literal(p_str) => {
                    // For literals, check exact match
                    match t {
                        PathSegment::Literal(t_str) if p_str == t_str => continue,
                        _ => return false,
                    }
                }
                PathSegment::SingleWildcard => {
                    // Single wildcards match any segment
                    continue;
                }
                PathSegment::MultiWildcard => {
                    // Multi-wildcards should only appear at the end
                    return false;
                }
            }
        }
        
        // If we get here, all segments matched
        true
    }
}

/// WildcardSubscriptionRegistry provides efficient storage and lookup for subscriptions
/// with support for wildcard pattern matching.
///
/// INTENTION: Optimize subscription storage and lookup based on whether a topic is an
/// exact match or a wildcard pattern. This gives O(1) lookup for exact matches while
/// still supporting powerful wildcard pattern matching.
#[derive(Debug, Clone)]
pub struct WildcardSubscriptionRegistry<T> {
    /// Exact matches (no wildcards) - fastest lookup with O(1) complexity
    exact_matches: HashMap<TopicPath, Vec<T>>,
    
    /// Patterns with single wildcards (* only) - separate for optimization
    single_wildcard_patterns: Vec<(TopicPath, Vec<T>)>,
    
    /// Patterns with multi-segment wildcards (> wildcards) - separate for optimization
    multi_wildcard_patterns: Vec<(TopicPath, Vec<T>)>,
}

impl<T: Clone> Default for WildcardSubscriptionRegistry<T> {
    fn default() -> Self {
        Self {
            exact_matches: HashMap::new(),
            single_wildcard_patterns: Vec::new(),
            multi_wildcard_patterns: Vec::new(),
        }
    }
}

impl<T: Clone> WildcardSubscriptionRegistry<T> {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Add a subscription for a topic
    ///
    /// INTENTION: Store a subscription in the appropriate collection based on
    /// whether it's an exact match or contains wildcards.
    pub fn add(&mut self, topic: TopicPath, handler: T) {
        if topic.is_pattern() {
            if topic.has_multi_wildcard() {
                // Handle multi-segment wildcard patterns
                for (existing_pattern, handlers) in &mut self.multi_wildcard_patterns {
                    if existing_pattern == &topic {
                        handlers.push(handler);
                        return;
                    }
                }
                // If we didn't find an existing pattern, add a new entry
                self.multi_wildcard_patterns.push((topic, vec![handler]));
            } else {
                // Handle single-segment wildcard patterns
                for (existing_pattern, handlers) in &mut self.single_wildcard_patterns {
                    if existing_pattern == &topic {
                        handlers.push(handler);
                        return;
                    }
                }
                // If we didn't find an existing pattern, add a new entry
                self.single_wildcard_patterns.push((topic, vec![handler]));
            }
        } else {
            // Handle exact matches
            self.exact_matches
                .entry(topic)
                .or_insert_with(Vec::new)
                .push(handler);
        }
    }
    
    /// Remove a subscription
    ///
    /// INTENTION: Remove a subscription from the appropriate collection based on
    /// the topic pattern type.
    pub fn remove(&mut self, topic: &TopicPath) -> bool {
        if topic.is_pattern() {
            if topic.has_multi_wildcard() {
                // Remove from multi-segment wildcard patterns
                if let Some(index) = self.multi_wildcard_patterns
                    .iter()
                    .position(|(pattern, _)| pattern == topic) {
                    self.multi_wildcard_patterns.remove(index);
                    return true;
                }
            } else {
                // Remove from single-segment wildcard patterns
                if let Some(index) = self.single_wildcard_patterns
                    .iter()
                    .position(|(pattern, _)| pattern == topic) {
                    self.single_wildcard_patterns.remove(index);
                    return true;
                }
            }
            false
        } else {
            // Remove from exact matches
            self.exact_matches.remove(topic).is_some()
        }
    }
    
    /// Remove a specific subscription handler
    ///
    /// INTENTION: Remove a specific handler from a topic's subscription list
    /// without removing all handlers for that topic.
    pub fn remove_handler<F>(&mut self, topic: &TopicPath, predicate: F) -> bool 
    where
        F: Fn(&T) -> bool
    {
        if topic.is_pattern() {
            if topic.has_multi_wildcard() {
                // Remove from multi-segment wildcard patterns
                if let Some((_, handlers)) = self.multi_wildcard_patterns
                    .iter_mut()
                    .find(|(pattern, _)| pattern == topic) {
                    let original_len = handlers.len();
                    handlers.retain(|handler| !predicate(handler));
                    return handlers.len() < original_len;
                }
            } else {
                // Remove from single-segment wildcard patterns
                if let Some((_, handlers)) = self.single_wildcard_patterns
                    .iter_mut()
                    .find(|(pattern, _)| pattern == topic) {
                    let original_len = handlers.len();
                    handlers.retain(|handler| !predicate(handler));
                    return handlers.len() < original_len;
                }
            }
            false
        } else {
            // Remove from exact matches
            if let Some(handlers) = self.exact_matches.get_mut(topic) {
                let original_len = handlers.len();
                handlers.retain(|handler| !predicate(handler));
                handlers.len() < original_len
            } else {
                false
            }
        }
    }
    
    /// Find all handlers that match a given topic
    ///
    /// INTENTION: Efficiently find all subscription handlers that match a given topic,
    /// including both exact matches and wildcard pattern matches.
    pub fn find_matches(&self, topic: &TopicPath) -> Vec<T> {
        let mut matches = Vec::new();
        
        // 1. Check exact matches first (O(1) lookup)
        if let Some(handlers) = self.exact_matches.get(topic) {
            matches.extend(handlers.clone());
        }
        
        // 2. Check single-wildcard patterns (iterate through patterns)
        for (pattern, handlers) in &self.single_wildcard_patterns {
            if pattern.matches(topic) {
                matches.extend(handlers.clone());
            }
        }
        
        // 3. Check multi-wildcard patterns (iterate through patterns)
        for (pattern, handlers) in &self.multi_wildcard_patterns {
            if pattern.matches(topic) {
                matches.extend(handlers.clone());
            }
        }
        
        matches
    }
    
    /// Get all subscriptions in the registry
    ///
    /// INTENTION: Retrieve all subscriptions for monitoring or debugging purposes.
    pub fn get_all_subscriptions(&self) -> Vec<(TopicPath, Vec<T>)> {
        let mut result = Vec::new();
        
        // Add exact matches
        for (topic, handlers) in &self.exact_matches {
            result.push((topic.clone(), handlers.clone()));
        }
        
        // Add single-wildcard patterns
        for (topic, handlers) in &self.single_wildcard_patterns {
            result.push((topic.clone(), handlers.clone()));
        }
        
        // Add multi-wildcard patterns
        for (topic, handlers) in &self.multi_wildcard_patterns {
            result.push((topic.clone(), handlers.clone()));
        }
        
        result
    }
} 