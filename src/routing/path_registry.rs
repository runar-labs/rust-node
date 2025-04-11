use std::collections::HashMap;
use crate::routing::TopicPath;

/// Helper function to check if a segment is a template parameter
fn is_template_param(segment: &str) -> bool {
    segment.starts_with("{") && segment.ends_with("}")
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
    
    /// Handlers for multi-wildcard (>) at this level
    multi_wildcard_handlers: Vec<T>,
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
            multi_wildcard_handlers: Vec::new(),
        }
    }
    
    /// Add a topic path and its handlers to the trie
    pub fn add(&mut self, topic: TopicPath, handlers: Vec<T>) {
        self.add_internal(&topic.get_segments(), &topic.network_id(), 0, handlers);
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
    fn add_internal(&mut self, segments: &[String], network_id: &str, index: usize, handlers: Vec<T>) {
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
                child.add_internal(segments, network_id, index + 1, handlers);
            }
        } else if is_template_param(segment) {
            // Template parameter - create/get template child and continue
            if self.template_child.is_none() {
                self.template_child = Some(Box::new(PathTrie::new()));
            }
            
            if let Some(child) = &mut self.template_child {
                child.add_internal(segments, network_id, index + 1, handlers);
            }
        } else {
            // Literal segment - create/get child and continue
            let child = self.children
                .entry(segment.clone())
                .or_insert_with(PathTrie::new);
                
            child.add_internal(segments, network_id, index + 1, handlers);
        }
    }
    
    /// Find all handlers that match the given topic
    pub fn find_matches(&self, topic: &TopicPath) -> Vec<T> {
        let segments = topic.get_segments();
        let mut results = Vec::new();
        
        self.find_matches_internal(&segments, &topic.network_id(), 0, &mut results);
        
        results
    }
    
    /// Internal recursive implementation of find_matches
    fn find_matches_internal(&self, segments: &[String], network_id: &str, index: usize, results: &mut Vec<T>) {
        if index >= segments.len() {
            // We've reached the end of the path
            results.extend(self.handlers.clone());
            return;
        }
        
        // Add any multi-wildcard handlers at this level
        results.extend(self.multi_wildcard_handlers.clone());
        
        let segment = &segments[index];
        
        // Check literal children
        if let Some(child) = self.children.get(segment) {
            child.find_matches_internal(segments, network_id, index + 1, results);
        }
        
        // Check wildcard child
        if let Some(child) = &self.wildcard_child {
            child.find_matches_internal(segments, network_id, index + 1, results);
        }
        
        // Check template child
        if let Some(child) = &self.template_child {
            child.find_matches_internal(segments, network_id, index + 1, results);
        }
    }
    
    /// Remove handlers that match a predicate for a specific topic path
    pub fn remove_handler<F>(&mut self, topic: &TopicPath, predicate: F) -> bool
    where
        F: Fn(&T) -> bool + Copy
    {
        self.remove_handler_internal(&topic.get_segments(), &topic.network_id(), 0, predicate)
    }
    
    /// Internal recursive implementation of remove_handler
    fn remove_handler_internal<F>(&mut self, segments: &[String], network_id: &str, index: usize, predicate: F) -> bool
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
                removed = child.remove_handler_internal(segments, network_id, index + 1, predicate);
            }
        } else if is_template_param(segment) {
            // Template parameter - delegate to template child if it exists
            if let Some(child) = &mut self.template_child {
                removed = child.remove_handler_internal(segments, network_id, index + 1, predicate);
            }
        } else {
            // Literal segment - delegate to the appropriate child if it exists
            if let Some(child) = self.children.get_mut(segment) {
                removed = child.remove_handler_internal(segments, network_id, index + 1, predicate);
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
        
        count
    }
} 