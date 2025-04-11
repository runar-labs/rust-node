use std::time::Instant;
use runar_node::routing::TopicPath;
use runar_node::routing::WildcardSubscriptionRegistry;

mod test_helpers {
    use super::*;
    use std::collections::{HashMap, BTreeMap};
    
    /// Import the EnhancedSubscriptionRegistry directly
    pub use crate::core::enhanced_registry_test::{EnhancedSubscriptionRegistry, PathTrie};
}

use test_helpers::EnhancedSubscriptionRegistry;

#[test]
fn benchmark_comparison_with_original_registry() {
    const NUM_PATTERNS: usize = 100;
    const NUM_LOOKUPS: usize = 1000;
    
    println!("Setting up benchmark with {} patterns and {} lookups", NUM_PATTERNS, NUM_LOOKUPS);
    
    // Create both registry types
    let mut original_registry = WildcardSubscriptionRegistry::<String>::new();
    let mut enhanced_registry = EnhancedSubscriptionRegistry::<String>::new();
    
    // Register handlers - mix of exact, templates, and wildcards
    println!("Registering handlers...");
    for i in 0..NUM_PATTERNS {
        // Exact paths
        let exact_path = format!("main:services/service{}/action{}", i, i);
        let exact_topic = TopicPath::new(&exact_path, "default").unwrap();
        let handler_name = format!("exact_{}", i);
        
        original_registry.add(exact_topic.clone(), handler_name.clone());
        enhanced_registry.add(exact_topic, handler_name);
        
        // Template paths (only in enhanced registry)
        if i % 5 == 0 {
            let template_path = format!("main:services/service{}/{{action}}", i);
            let template_topic = TopicPath::new(&template_path, "default").unwrap();
            let handler_name = format!("template_{}", i);
            
            enhanced_registry.add(template_topic, handler_name);
        }
        
        // Single wildcard paths
        if i % 10 == 0 {
            let wildcard_path = format!("main:services/service{}/*", i);
            let wildcard_topic = TopicPath::new(&wildcard_path, "default").unwrap();
            let handler_name = format!("wildcard_{}", i);
            
            original_registry.add(wildcard_topic.clone(), handler_name.clone());
            enhanced_registry.add(wildcard_topic, handler_name);
        }
        
        // Multi-wildcard paths
        if i % 50 == 0 {
            let multi_wildcard_path = format!("main:services/service{}>", i);
            let multi_wildcard_topic = TopicPath::new(&multi_wildcard_path, "default").unwrap();
            let handler_name = format!("multi_wildcard_{}", i);
            
            original_registry.add(multi_wildcard_topic.clone(), handler_name.clone());
            enhanced_registry.add(multi_wildcard_topic, handler_name);
        }
    }
    
    // Create trie from enhanced registry
    println!("Converting to path trie...");
    let trie = enhanced_registry.to_path_trie();
    
    // Generate lookup topics
    println!("Generating lookup topics...");
    let mut lookup_topics = Vec::with_capacity(NUM_LOOKUPS);
    for i in 0..NUM_LOOKUPS {
        let service_num = i % NUM_PATTERNS;
        let action_num = i % (NUM_PATTERNS / 2); // Mix up the matches
        
        let path = format!("main:services/service{}/action{}", service_num, action_num);
        let topic = TopicPath::new(&path, "default").unwrap();
        lookup_topics.push(topic);
    }
    
    // Benchmark original registry
    println!("Benchmarking original registry...");
    let start_time = Instant::now();
    let mut original_matches = 0;
    for topic in &lookup_topics {
        let matches = original_registry.find_matches(topic);
        original_matches += matches.len();
    }
    let original_time = start_time.elapsed();
    
    // Benchmark enhanced registry
    println!("Benchmarking enhanced registry...");
    let start_time = Instant::now();
    let mut enhanced_matches = 0;
    for topic in &lookup_topics {
        let matches = enhanced_registry.find_matches(topic);
        enhanced_matches += matches.len();
    }
    let enhanced_time = start_time.elapsed();
    
    // Benchmark trie
    println!("Benchmarking path trie...");
    let start_time = Instant::now();
    let mut trie_matches = 0;
    for topic in &lookup_topics {
        let matches = trie.find_matches(topic);
        trie_matches += matches.len();
    }
    let trie_time = start_time.elapsed();
    
    // Print results
    println!("Results:");
    println!("Original registry: {:?} for {} matches", original_time, original_matches);
    println!("Enhanced registry: {:?} for {} matches", enhanced_time, enhanced_matches);
    println!("Path trie:         {:?} for {} matches", trie_time, trie_matches);
    println!("Speed improvement (enhanced vs original): {:.2}x", original_time.as_micros() as f64 / enhanced_time.as_micros() as f64);
    println!("Speed improvement (trie vs original): {:.2}x", original_time.as_micros() as f64 / trie_time.as_micros() as f64);
    
    // Make sure the results are correct (trie and enhanced should have more matches due to templates)
    assert!(enhanced_matches >= original_matches, "Enhanced registry should find at least as many matches as original");
    assert_eq!(enhanced_matches, trie_matches, "Trie should find same number of matches as enhanced registry");
} 