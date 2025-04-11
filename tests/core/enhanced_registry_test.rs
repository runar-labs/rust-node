use std::collections::{HashMap, BTreeMap};
use std::hash::{Hash, Hasher};
use anyhow::Result;
use tokio::sync::Mutex;
use std::sync::Arc;
use std::future::Future;
use std::pin::Pin;

use runar_node::routing::TopicPath;
use runar_node::services::EventContext;
use runar_common::types::ValueType;
use runar_common::logging::{Component, Logger};

/// Enhanced registry implementation that optimizes lookup of wildcards and templates
/// 
/// This is an experimental implementation for testing advanced path matching techniques
/// without modifying the core code. Once verified, similar techniques could be applied
/// to the core implementation.
pub struct EnhancedSubscriptionRegistry<T: Clone> {
    /// Exact matches (no wildcards or templates) - fastest lookup with O(1) complexity
    exact_matches: HashMap<TopicPath, Vec<T>>,
    
    /// Template patterns (containing {param}) - indexed by segment count for faster filtering
    /// BTreeMap allows us to query by segment count efficiently
    template_patterns: BTreeMap<usize, Vec<(TopicPath, Vec<T>)>>,
    
    /// Patterns with single wildcards (* only) - similarly indexed by segment count
    single_wildcard_patterns: BTreeMap<usize, Vec<(TopicPath, Vec<T>)>>,
    
    /// Patterns with multi-segment wildcards (> wildcards)
    /// Multi-wildcards can match varying segment counts, so indexed differently
    multi_wildcard_patterns: Vec<(TopicPath, Vec<T>)>,
    
    /// Secondary index by first segment for faster filtering
    /// This helps us quickly filter out paths with different first segments
    first_segment_index: HashMap<String, Vec<(TopicPath, Vec<T>)>>,
}

impl<T: Clone> Default for EnhancedSubscriptionRegistry<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to check if a segment is a template parameter
fn is_template_param(segment: &str) -> bool {
    segment.starts_with("{") && segment.ends_with("}")
}

impl<T: Clone> EnhancedSubscriptionRegistry<T> {
    pub fn new() -> Self {
        EnhancedSubscriptionRegistry {
            exact_matches: HashMap::new(),
            template_patterns: BTreeMap::new(),
            single_wildcard_patterns: BTreeMap::new(),
            multi_wildcard_patterns: Vec::new(),
            first_segment_index: HashMap::new(),
        }
    }
    
    /// Add a subscription to the registry
    pub fn add(&mut self, topic: TopicPath, handler: T) {
        let segments = topic.get_segments();
        let segment_count = segments.len();
        
        // Check if this is a pattern (has wildcards or template params)
        let has_wildcards = segments.iter().any(|s| s == "*" || s == ">");
        let has_templates = segments.iter().any(|s| is_template_param(s));
        
        if has_wildcards {
            // Handle wildcard patterns
            if segments.iter().any(|s| s == ">") {
                // Multi-segment wildcard pattern
                self.multi_wildcard_patterns.push((topic.clone(), vec![handler]));
            } else {
                // Single-segment wildcard pattern (*)
                let pattern_entry = self.single_wildcard_patterns
                    .entry(segment_count)
                    .or_insert_with(Vec::new);
                    
                // Check if we already have this pattern
                if let Some((_, handlers)) = pattern_entry.iter_mut()
                    .find(|(p, _)| p.as_str() == topic.as_str()) {
                    handlers.push(handler);
                } else {
                    pattern_entry.push((topic.clone(), vec![handler]));
                }
            }
        } else if has_templates {
            // Handle template patterns
            let template_entry = self.template_patterns
                .entry(segment_count)
                .or_insert_with(Vec::new);
                
            // Check if we already have this pattern
            if let Some((_, handlers)) = template_entry.iter_mut()
                .find(|(p, _)| p.as_str() == topic.as_str()) {
                handlers.push(handler.clone());
            } else {
                template_entry.push((topic.clone(), vec![handler.clone()]));
            }
            
            // Also index by first segment if it's not a template
            if !segments.is_empty() && !is_template_param(&segments[0]) && segments[0] != "*" {
                let first_segment = segments[0].to_string();
                let first_segment_entry = self.first_segment_index
                    .entry(first_segment)
                    .or_insert_with(Vec::new);
                first_segment_entry.push((topic.clone(), vec![handler]));
            }
        } else {
            // Exact match (no wildcards or templates)
            let handlers = self.exact_matches
                .entry(topic.clone())
                .or_insert_with(Vec::new);
            handlers.push(handler);
        }
    }
    
    /// Find all handlers that match the given topic
    pub fn find_matches(&self, topic: &TopicPath) -> Vec<T> {
        let mut results = Vec::new();
        let segments = topic.get_segments();
        let segment_count = segments.len();
        
        // 1. Check exact matches - O(1)
        if let Some(handlers) = self.exact_matches.get(topic) {
            results.extend(handlers.clone());
        }
        
        // 2. Check templates with matching segment count - O(k) where k is usually small
        if let Some(templates) = self.template_patterns.get(&segment_count) {
            // First try to filter by first segment if available
            let _first_segment = if !segments.is_empty() {
                Some(segments[0].to_string())
            } else {
                None
            };
            
            for (pattern, handlers) in templates {
                // Check if pattern matches
                if self.matches_template_pattern(pattern, topic) {
                    results.extend(handlers.clone());
                }
            }
        }
        
        // 3. Check single wildcards with matching segment count
        if let Some(wildcards) = self.single_wildcard_patterns.get(&segment_count) {
            for (pattern, handlers) in wildcards {
                if pattern.matches(topic) {
                    results.extend(handlers.clone());
                }
            }
        }
        
        // 4. Check multi-wildcards (can match variable segment counts)
        for (pattern, handlers) in &self.multi_wildcard_patterns {
            if pattern.matches(topic) {
                results.extend(handlers.clone());
            }
        }
        
        results
    }
    
    /// Custom template pattern matching with optimizations
    fn matches_template_pattern(&self, pattern: &TopicPath, topic: &TopicPath) -> bool {
        // Network IDs must match
        if pattern.network_id() != topic.network_id() {
            return false;
        }
        
        let pattern_segments = pattern.get_segments();
        let topic_segments = topic.get_segments();
        
        // Segment counts must match
        if pattern_segments.len() != topic_segments.len() {
            return false;
        }
        
        // Check each segment
        for (p_seg, t_seg) in pattern_segments.iter().zip(topic_segments.iter()) {
            if p_seg == t_seg {
                // Exact match
                continue;
            } else if p_seg == "*" {
                // Single wildcard
                continue;
            } else if is_template_param(p_seg) {
                // Template parameter
                continue;
            } else {
                // No match
                return false;
            }
        }
        
        true
    }
    
    /// Remove a subscription with the specified topic
    pub fn remove(&mut self, topic: &TopicPath) -> bool {
        let segments = topic.get_segments();
        let segment_count = segments.len();
        let mut removed = false;
        
        // Check if this is a pattern
        let has_wildcards = segments.iter().any(|s| s == "*" || s == ">");
        let has_templates = segments.iter().any(|s| is_template_param(s));
        
        if has_wildcards {
            if segments.iter().any(|s| s == ">") {
                // Remove from multi-wildcards
                let initial_len = self.multi_wildcard_patterns.len();
                self.multi_wildcard_patterns.retain(|(p, _)| p.as_str() != topic.as_str());
                removed = initial_len != self.multi_wildcard_patterns.len();
            } else {
                // Remove from single wildcards
                if let Some(patterns) = self.single_wildcard_patterns.get_mut(&segment_count) {
                    let initial_len = patterns.len();
                    patterns.retain(|(p, _)| p.as_str() != topic.as_str());
                    removed = initial_len != patterns.len();
                    
                    // Clean up empty entries
                    if patterns.is_empty() {
                        self.single_wildcard_patterns.remove(&segment_count);
                    }
                }
            }
        } else if has_templates {
            // Remove from templates
            if let Some(patterns) = self.template_patterns.get_mut(&segment_count) {
                let initial_len = patterns.len();
                patterns.retain(|(p, _)| p.as_str() != topic.as_str());
                removed = initial_len != patterns.len();
                
                // Clean up empty entries
                if patterns.is_empty() {
                    self.template_patterns.remove(&segment_count);
                }
            }
            
            // Also clean up first segment index
            if !segments.is_empty() && !is_template_param(&segments[0]) {
                let first_segment = segments[0].to_string();
                if let Some(entries) = self.first_segment_index.get_mut(&first_segment) {
                    entries.retain(|(p, _)| p.as_str() != topic.as_str());
                    if entries.is_empty() {
                        self.first_segment_index.remove(&first_segment);
                    }
                }
            }
        } else {
            // Remove from exact matches
            removed = self.exact_matches.remove(topic).is_some();
        }
        
        removed
    }
    
    /// Get all subscriptions in the registry
    pub fn get_all_subscriptions(&self) -> Vec<(TopicPath, Vec<T>)> {
        let mut result = Vec::new();
        
        // Collect exact matches
        for (topic, handlers) in &self.exact_matches {
            result.push((topic.clone(), handlers.clone()));
        }
        
        // Collect template patterns
        for patterns in self.template_patterns.values() {
            for (topic, handlers) in patterns {
                result.push((topic.clone(), handlers.clone()));
            }
        }
        
        // Collect single wildcard patterns
        for patterns in self.single_wildcard_patterns.values() {
            for (topic, handlers) in patterns {
                result.push((topic.clone(), handlers.clone()));
            }
        }
        
        // Collect multi-wildcard patterns
        for (topic, handlers) in &self.multi_wildcard_patterns {
            result.push((topic.clone(), handlers.clone()));
        }
        
        result
    }
    
    /// Path trie implementation for optimized matching
    pub fn to_path_trie(&self) -> PathTrie<T> {
        let mut trie = PathTrie::new();
        
        // Add exact matches
        for (topic, handlers) in &self.exact_matches {
            trie.add(topic.clone(), handlers.clone());
        }
        
        // Add template patterns
        for patterns in self.template_patterns.values() {
            for (topic, handlers) in patterns {
                trie.add(topic.clone(), handlers.clone());
            }
        }
        
        // Add single wildcard patterns
        for patterns in self.single_wildcard_patterns.values() {
            for (topic, handlers) in patterns {
                trie.add(topic.clone(), handlers.clone());
            }
        }
        
        // Add multi-wildcard patterns
        for (topic, handlers) in &self.multi_wildcard_patterns {
            trie.add(topic.clone(), handlers.clone());
        }
        
        trie
    }
}

/// Optimized path trie structure for faster matching
/// 
/// This provides a hierarchical structure that can efficiently match paths
/// by walking down the segments one at a time, similar to how a file system works.
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

impl<T: Clone> PathTrie<T> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_exact_match() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add some handlers
        registry.add(TopicPath::new("main:services/math", "default").unwrap(), "MATH");
        registry.add(TopicPath::new("main:services/auth", "default").unwrap(), "AUTH");
        
        // Test exact matches
        let topic1 = TopicPath::new("main:services/math", "default").unwrap();
        let topic2 = TopicPath::new("main:services/auth", "default").unwrap();
        let topic3 = TopicPath::new("main:services/unknown", "default").unwrap();
        
        let matches1 = registry.find_matches(&topic1);
        let matches2 = registry.find_matches(&topic2);
        let matches3 = registry.find_matches(&topic3);
        
        assert_eq!(matches1, vec!["MATH"]);
        assert_eq!(matches2, vec!["AUTH"]);
        assert_eq!(matches3, Vec::<&str>::new());
    }
    
    #[test]
    fn test_template_match() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add template pattern
        registry.add(
            TopicPath::new("main:services/{service_path}/state", "default").unwrap(),
            "TEMPLATE"
        );
        
        // Test matches
        let topic1 = TopicPath::new("main:services/math/state", "default").unwrap();
        let topic2 = TopicPath::new("main:services/auth/state", "default").unwrap();
        let topic3 = TopicPath::new("main:services/math/config", "default").unwrap();
        
        let matches1 = registry.find_matches(&topic1);
        let matches2 = registry.find_matches(&topic2);
        let matches3 = registry.find_matches(&topic3);
        
        assert_eq!(matches1, vec!["TEMPLATE"]);
        assert_eq!(matches2, vec!["TEMPLATE"]);
        assert_eq!(matches3, Vec::<&str>::new());
    }
    
    #[test]
    fn test_wildcard_match() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add wildcard patterns
        registry.add(
            TopicPath::new("main:services/*/state", "default").unwrap(),
            "SINGLE_WILDCARD"
        );
        registry.add(
            TopicPath::new("main:events/>", "default").unwrap(),
            "MULTI_WILDCARD"
        );
        
        // Test single wildcard matches
        let topic1 = TopicPath::new("main:services/math/state", "default").unwrap();
        let topic2 = TopicPath::new("main:services/auth/state", "default").unwrap();
        let topic3 = TopicPath::new("main:services/math/config", "default").unwrap();
        
        let matches1 = registry.find_matches(&topic1);
        let matches2 = registry.find_matches(&topic2);
        let matches3 = registry.find_matches(&topic3);
        
        assert_eq!(matches1, vec!["SINGLE_WILDCARD"]);
        assert_eq!(matches2, vec!["SINGLE_WILDCARD"]);
        assert_eq!(matches3, Vec::<&str>::new());
        
        // Test multi-wildcard matches
        let topic4 = TopicPath::new("main:events/user/created", "default").unwrap();
        let topic5 = TopicPath::new("main:events/system/started", "default").unwrap();
        let topic6 = TopicPath::new("main:services/math/events", "default").unwrap();
        
        let matches4 = registry.find_matches(&topic4);
        let matches5 = registry.find_matches(&topic5);
        let matches6 = registry.find_matches(&topic6);
        
        assert_eq!(matches4, vec!["MULTI_WILDCARD"]);
        assert_eq!(matches5, vec!["MULTI_WILDCARD"]);
        assert_eq!(matches6, Vec::<&str>::new());
    }
    
    #[test]
    fn test_multiple_matches() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add various handlers that could all match a single path
        registry.add(
            TopicPath::new("main:services/math/actions/add", "default").unwrap(),
            "EXACT"
        );
        registry.add(
            TopicPath::new("main:services/{service_path}/actions/{action}", "default").unwrap(),
            "TEMPLATE"
        );
        registry.add(
            TopicPath::new("main:services/*/actions/*", "default").unwrap(),
            "WILDCARD"
        );
        registry.add(
            TopicPath::new("main:services/>", "default").unwrap(),
            "MULTI_WILDCARD"
        );
        
        // Test path that should match all patterns
        let topic = TopicPath::new("main:services/math/actions/add", "default").unwrap();
        let matches = registry.find_matches(&topic);
        
        // Check that we have all matches (order may vary)
        assert_eq!(matches.len(), 4);
        assert!(matches.contains(&"EXACT"));
        assert!(matches.contains(&"TEMPLATE"));
        assert!(matches.contains(&"WILDCARD"));
        assert!(matches.contains(&"MULTI_WILDCARD"));
    }
    
    #[test]
    fn test_network_isolation() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add handlers for different networks
        registry.add(
            TopicPath::new("main:services/math", "default").unwrap(),
            "MAIN"
        );
        registry.add(
            TopicPath::new("test:services/math", "default").unwrap(),
            "TEST"
        );
        
        // Test network isolation
        let topic1 = TopicPath::new("main:services/math", "default").unwrap();
        let topic2 = TopicPath::new("test:services/math", "default").unwrap();
        
        let matches1 = registry.find_matches(&topic1);
        let matches2 = registry.find_matches(&topic2);
        
        assert_eq!(matches1, vec!["MAIN"]);
        assert_eq!(matches2, vec!["TEST"]);
    }
    
    #[test]
    fn test_remove() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add handlers
        registry.add(
            TopicPath::new("main:services/math", "default").unwrap(),
            "EXACT"
        );
        registry.add(
            TopicPath::new("main:services/{service_path}", "default").unwrap(),
            "TEMPLATE"
        );
        registry.add(
            TopicPath::new("main:services/*", "default").unwrap(),
            "WILDCARD"
        );
        registry.add(
            TopicPath::new("main:services/>", "default").unwrap(),
            "MULTI_WILDCARD"
        );
        
        // Remove handlers
        let removed1 = registry.remove(&TopicPath::new("main:services/math", "default").unwrap());
        let removed2 = registry.remove(&TopicPath::new("main:services/{service_path}", "default").unwrap());
        let removed3 = registry.remove(&TopicPath::new("main:services/*", "default").unwrap());
        let removed4 = registry.remove(&TopicPath::new("main:services/>", "default").unwrap());
        
        assert!(removed1);
        assert!(removed2);
        assert!(removed3);
        assert!(removed4);
        
        // Check that all handlers are removed
        let topic = TopicPath::new("main:services/math", "default").unwrap();
        let matches = registry.find_matches(&topic);
        assert_eq!(matches, Vec::<&str>::new());
    }
    
    #[test]
    fn test_path_trie() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add various handlers
        registry.add(
            TopicPath::new("main:services/math/add", "default").unwrap(),
            "EXACT"
        );
        registry.add(
            TopicPath::new("main:services/{service_path}/state", "default").unwrap(),
            "TEMPLATE"
        );
        registry.add(
            TopicPath::new("main:services/*/events", "default").unwrap(),
            "WILDCARD"
        );
        registry.add(
            TopicPath::new("main:events/>", "default").unwrap(),
            "MULTI_WILDCARD"
        );
        
        // Convert to path trie
        let trie = registry.to_path_trie();
        
        // Test exact match
        let topic1 = TopicPath::new("main:services/math/add", "default").unwrap();
        let matches1 = trie.find_matches(&topic1);
        assert_eq!(matches1, vec!["EXACT"]);
        
        // Test template match
        let topic2 = TopicPath::new("main:services/math/state", "default").unwrap();
        let matches2 = trie.find_matches(&topic2);
        assert_eq!(matches2, vec!["TEMPLATE"]);
        
        // Test wildcard match
        let topic3 = TopicPath::new("main:services/math/events", "default").unwrap();
        let matches3 = trie.find_matches(&topic3);
        assert_eq!(matches3, vec!["WILDCARD"]);
        
        // Test multi-wildcard match
        let topic4 = TopicPath::new("main:events/user/created", "default").unwrap();
        let matches4 = trie.find_matches(&topic4);
        assert_eq!(matches4, vec!["MULTI_WILDCARD"]);
    }
    
    #[test]
    fn test_complex_trie_scenario() {
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add handlers for a realistic service architecture
        registry.add(
            TopicPath::new("main:services/math/actions/add", "default").unwrap(),
            "ADD"
        );
        registry.add(
            TopicPath::new("main:services/math/actions/subtract", "default").unwrap(),
            "SUBTRACT"
        );
        registry.add(
            TopicPath::new("main:services/{service_path}/actions/{action}", "default").unwrap(),
            "ANY_ACTION"
        );
        registry.add(
            TopicPath::new("main:services/*/events/started", "default").unwrap(),
            "SERVICE_STARTED"
        );
        registry.add(
            TopicPath::new("main:services/*/events/*", "default").unwrap(),
            "ANY_EVENT"
        );
        registry.add(
            TopicPath::new("main:services/auth/users/{user_id}", "default").unwrap(),
            "USER_INFO"
        );
        registry.add(
            TopicPath::new("main:stats/>", "default").unwrap(),
            "STATS"
        );
        
        // Convert to path trie
        let trie = registry.to_path_trie();
        
        // Test various paths
        let topic1 = TopicPath::new("main:services/math/actions/add", "default").unwrap();
        let matches1 = trie.find_matches(&topic1);
        assert_eq!(matches1.len(), 2);
        assert!(matches1.contains(&"ADD"));
        assert!(matches1.contains(&"ANY_ACTION"));
        
        let topic2 = TopicPath::new("main:services/auth/events/started", "default").unwrap();
        let matches2 = trie.find_matches(&topic2);
        assert_eq!(matches2.len(), 2);
        assert!(matches2.contains(&"SERVICE_STARTED"));
        assert!(matches2.contains(&"ANY_EVENT"));
        
        let topic3 = TopicPath::new("main:services/auth/users/123", "default").unwrap();
        let matches3 = trie.find_matches(&topic3);
        assert_eq!(matches3, vec!["USER_INFO"]);
        
        let topic4 = TopicPath::new("main:stats/cpu/usage", "default").unwrap();
        let matches4 = trie.find_matches(&topic4);
        assert_eq!(matches4, vec!["STATS"]);
    }
    
    #[test]
    fn test_performance_comparison() {
        const NUM_PATTERNS: usize = 100;
        const NUM_LOOKUPS: usize = 1000;
        
        // Create a registry with many patterns
        let mut registry = EnhancedSubscriptionRegistry::new();
        
        // Add patterns
        for i in 0..NUM_PATTERNS {
            // Exact paths
            registry.add(
                TopicPath::new(&format!("main:services/service{}/action{}", i, i), "default").unwrap(),
                format!("EXACT_{}", i)
            );
            
            // Template paths
            registry.add(
                TopicPath::new(&format!("main:services/service{}/{{action}}", i), "default").unwrap(),
                format!("TEMPLATE_{}", i)
            );
            
            // Wildcard paths
            registry.add(
                TopicPath::new(&format!("main:services/service{}/*", i), "default").unwrap(),
                format!("WILDCARD_{}", i)
            );
        }
        
        // Convert to path trie
        let trie = registry.to_path_trie();
        
        // Time standard registry lookups
        let start_time = std::time::Instant::now();
        for i in 0..NUM_LOOKUPS {
            let topic = TopicPath::new(
                &format!("main:services/service{}/action{}", i % NUM_PATTERNS, i % NUM_PATTERNS),
                "default"
            ).unwrap();
            let _ = registry.find_matches(&topic);
        }
        let registry_time = start_time.elapsed();
        
        // Time path trie lookups
        let start_time = std::time::Instant::now();
        for i in 0..NUM_LOOKUPS {
            let topic = TopicPath::new(
                &format!("main:services/service{}/action{}", i % NUM_PATTERNS, i % NUM_PATTERNS),
                "default"
            ).unwrap();
            let _ = trie.find_matches(&topic);
        }
        let trie_time = start_time.elapsed();
        
        println!("Registry lookup time: {:?}", registry_time);
        println!("Path trie lookup time: {:?}", trie_time);
        
        // The trie should generally be faster, but this is primarily for informational purposes
        // so we don't assert on specific performance metrics which could vary by environment
    }
} 