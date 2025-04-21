use std::collections::HashMap;
use crate::routing::TopicPath;

/// Helper function to check if a segment is a template parameter
fn is_template_param(segment: &str) -> bool {
    segment.starts_with("{") && segment.ends_with("}")
}

/// Helper function to extract parameter name from a template segment
fn extract_param_name(segment: &str) -> String {
    // Remove the { and } from the segment
    segment[1..segment.len()-1].to_string()
}

/// Result type that includes both the handler and any extracted parameters
#[derive(Clone)]
pub struct PathTrieMatch<T: Clone> {
    /// The handler that matched the path
    pub handler: T,
    /// Parameters extracted from template segments
    pub params: HashMap<String, String>,
}

/// Optimized path trie structure for faster matching
/// 
/// INTENTION: Provide a hierarchical data structure that efficiently matches paths
/// by walking down the segments one at a time. This allows for fast lookup of handlers
/// registered for exact paths, template patterns, and wildcard patterns.
#[derive(Clone)]
pub struct PathTrie<T: Clone> {
    /// Handlers for this exact path
    handlers: Vec<T>,
    
    /// Child nodes for literal segments
    children: HashMap<String, PathTrie<T>>,
    
    /// Child node for single wildcard (*)
    wildcard_child: Option<Box<PathTrie<T>>>,
    
    /// Child node for template parameters ({param})
    template_child: Option<Box<PathTrie<T>>>,
    
    /// Template parameter name at this level (if this is a template node)
    template_param_name: Option<String>,
    
    /// Handlers for multi-wildcard (>) at this level
    multi_wildcard_handlers: Vec<T>,
    
    /// Network-specific tries - the top level trie is keyed by network_id
    networks: HashMap<String, PathTrie<T>>,
}

impl<T: Clone> Default for PathTrie<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> PathTrie<T> {
    /// Create a new empty path trie
    pub fn new() -> Self {
        PathTrie {
            handlers: Vec::new(),
            children: HashMap::new(),
            wildcard_child: None,
            template_child: None,
            template_param_name: None,
            multi_wildcard_handlers: Vec::new(),
            networks: HashMap::new(),
        }
    }
    
    /// Add a topic path and its handlers to the trie
    pub fn add(&mut self, topic: TopicPath, handlers: Vec<T>) {
        let network_id = topic.network_id();
        
        // Get or create network-specific trie
        let network_trie = self.networks
            .entry(network_id.to_string())
            .or_insert_with(PathTrie::new);
            
        // Add to the network-specific trie
        network_trie.add_internal(&topic.get_segments(), 0, handlers);
    }
    
    /// Add a single handler for a topic path
    pub fn add_handler(&mut self, topic: TopicPath, handler: T) {
        self.add(topic, vec![handler]);
    }
    
    /// Add handlers for a vector of topic paths
    pub fn add_handlers(&mut self, topics: Vec<TopicPath>, handlers: Vec<T>) {
        for topic in topics {
            self.add(topic, handlers.clone());
        }
    }
    
    /// Internal recursive implementation of add
    fn add_internal(&mut self, segments: &[String], index: usize, handlers: Vec<T>) {
        if index >= segments.len() {
            // We've reached the end of the path, add handlers here
            self.handlers.extend(handlers);
            return;
        }
        
        let segment = &segments[index];
        
        if segment == ">" {
            // Multi-wildcard - adds handlers here and matches everything below
            self.multi_wildcard_handlers.extend(handlers);
        } else if segment == "*" {
            // Single wildcard - create/get wildcard child and continue
            if self.wildcard_child.is_none() {
                self.wildcard_child = Some(Box::new(PathTrie::new()));
            }
            
            if let Some(child) = &mut self.wildcard_child {
                child.add_internal(segments, index + 1, handlers);
            }
        } else if is_template_param(segment) {
            // Template parameter - create/get template child and continue
            if self.template_child.is_none() {
                self.template_child = Some(Box::new(PathTrie::new()));
                // Store the parameter name
                self.template_param_name = Some(extract_param_name(segment));
            }
            
            if let Some(child) = &mut self.template_child {
                child.add_internal(segments, index + 1, handlers);
            }
        } else {
            // Literal segment - create/get child and continue
            let child = self.children
                .entry(segment.clone())
                .or_insert_with(PathTrie::new);
                
            child.add_internal(segments, index + 1, handlers);
        }
    }
    
    /// Find all handlers that match the given topic, including any extracted parameters
    pub fn find_matches(&self, topic: &TopicPath) -> Vec<PathTrieMatch<T>> {
        let network_id = topic.network_id();
        let segments = topic.get_segments();
        let mut results = Vec::new();
        
        // Only look in the network-specific trie
        if let Some(network_trie) = self.networks.get(&network_id.to_string()) {
            // Start with empty parameters map
            let mut params = HashMap::new();
            network_trie.find_matches_internal(&segments, 0, &mut results, &mut params);
        }
        
        results
    }
    
    /// Internal recursive implementation of find_matches
    fn find_matches_internal(&self, segments: &[String], index: usize, results: &mut Vec<PathTrieMatch<T>>, current_params: &mut HashMap<String, String>) {
        if index >= segments.len() {
            // We've reached the end of the path
            // Add all handlers at this level with the current parameters
            for handler in &self.handlers {
                results.push(PathTrieMatch {
                    handler: handler.clone(),
                    params: current_params.clone(),
                });
            }
            return;
        }
        
        // Add any multi-wildcard handlers at this level
        for handler in &self.multi_wildcard_handlers {
            results.push(PathTrieMatch {
                handler: handler.clone(),
                params: current_params.clone(),
            });
        }
        
        let segment = &segments[index];
        
        // Check literal children
        if let Some(child) = self.children.get(segment) {
            child.find_matches_internal(segments, index + 1, results, current_params);
        }
        
        // Check wildcard child
        if let Some(child) = &self.wildcard_child {
            child.find_matches_internal(segments, index + 1, results, current_params);
        }
        
        // Check template child
        if let Some(child) = &self.template_child {
            // If this is a template node, extract and store the parameter
            if let Some(param_name) = &self.template_param_name {
                // Save old value in case we need to restore it
                let old_value = current_params.get(param_name).cloned();
                
                // Set the parameter value from the current segment
                current_params.insert(param_name.clone(), segment.clone());
                
                // Continue matching with the updated parameters
                child.find_matches_internal(segments, index + 1, results, current_params);
                
                // Restore old value or remove the parameter
                match old_value {
                    Some(value) => {
                        current_params.insert(param_name.clone(), value);
                    },
                    None => {
                        current_params.remove(param_name);
                    }
                }
            } else {
                // No parameter name defined, just continue matching
                child.find_matches_internal(segments, index + 1, results, current_params);
            }
        }
    }
    
    /// For backward compatibility - get just the handlers without parameters
    pub fn find_handlers(&self, topic: &TopicPath) -> Vec<T> {
        self.find_matches(topic)
            .into_iter()
            .map(|m| m.handler)
            .collect()
    }
    
    /// Remove handlers that match a predicate for a specific topic path
    pub fn remove_handler<F>(&mut self, topic: &TopicPath, predicate: F) -> bool
    where
        F: Fn(&T) -> bool + Copy
    {
        let network_id = topic.network_id();
        
        // Only remove from the specific network trie
        if let Some(network_trie) = self.networks.get_mut(&network_id.to_string()) {
            return network_trie.remove_handler_internal(&topic.get_segments(), 0, predicate);
        }
        
        false
    }
    
    /// Internal recursive implementation of remove_handler
    fn remove_handler_internal<F>(&mut self, segments: &[String], index: usize, predicate: F) -> bool
    where 
        F: Fn(&T) -> bool + Copy
    {
        if index >= segments.len() {
            // We've reached the end of the path
            let initial_len = self.handlers.len();
            self.handlers.retain(|h| !predicate(h));
            return initial_len > self.handlers.len();
        }
        
        let segment = &segments[index];
        let mut removed = false;
        
        if segment == ">" {
            // Multi-wildcard - remove from multi_wildcard_handlers
            let initial_len = self.multi_wildcard_handlers.len();
            self.multi_wildcard_handlers.retain(|h| !predicate(h));
            removed = initial_len > self.multi_wildcard_handlers.len();
        } else if segment == "*" {
            // Single wildcard - delegate to wildcard child if it exists
            if let Some(child) = &mut self.wildcard_child {
                removed = child.remove_handler_internal(segments, index + 1, predicate);
            }
        } else if is_template_param(segment) {
            // Template parameter - delegate to template child if it exists
            if let Some(child) = &mut self.template_child {
                removed = child.remove_handler_internal(segments, index + 1, predicate);
            }
        } else {
            // Literal segment - delegate to the appropriate child if it exists
            if let Some(child) = self.children.get_mut(segment) {
                removed = child.remove_handler_internal(segments, index + 1, predicate);
            }
        }
        
        removed
    }
    
    /// Check if this trie is empty (has no handlers or children)
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty() 
            && self.multi_wildcard_handlers.is_empty()
            && self.children.is_empty()
            && self.wildcard_child.is_none()
            && self.template_child.is_none()
            && self.networks.is_empty()
    }
    
    /// Get the count of handlers in this trie
    pub fn handler_count(&self) -> usize {
        let mut count = self.handlers.len() + self.multi_wildcard_handlers.len();
        
        // Count handlers in children
        for (_, child) in &self.children {
            count += child.handler_count();
        }
        
        // Count handlers in wildcard child
        if let Some(child) = &self.wildcard_child {
            count += child.handler_count();
        }
        
        // Count handlers in template child
        if let Some(child) = &self.template_child {
            count += child.handler_count();
        }
        
        // Count handlers in network-specific tries
        for (_, network_trie) in &self.networks {
            count += network_trie.handler_count();
        }
        
        count
    }
} 