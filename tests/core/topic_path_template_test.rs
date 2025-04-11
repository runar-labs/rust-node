use std::collections::HashMap;
use runar_node::routing::TopicPath;
use runar_node::routing::TemplatePathKey;

#[test]
fn test_extract_params_from_template() {
    // A template pattern for our Registry Service paths
    let template = "services/{service_path}/state";
    
    // An actual path that matches the template
    let path: TopicPath = TopicPath::new("services/math/state", "main").expect("Valid path");
    
    // Extract parameters from the path
    let params = path.extract_params(template).expect("Template should match");
    
    // Verify the extracted parameter
    assert_eq!(params.get("service_path"), Some(&"math".to_string()));
    
    // Try another path
    let path2 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
    let params2 = path2.extract_params(template).expect("Template should match");
    assert_eq!(params2.get("service_path"), Some(&"auth".to_string()));
    
    // A path that doesn't match the segment count
    let non_matching1 = TopicPath::new("main:services/math", "default").expect("Valid path");
    assert!(non_matching1.extract_params(template).is_err());
    
    // A path that doesn't match the literal segments
    let non_matching2 = TopicPath::new("main:users/math/profile", "default").expect("Valid path");
    assert!(non_matching2.extract_params(template).is_err());
}

#[test]
fn test_hash_keys_with_template() {
    // A template pattern for our Registry Service paths
    let template = "services/{service_path}";
    let match_value = "services/math";
    let not_match = "services/{service_path}/something_else";
    let network_id = "main";
   
    let template_path = TopicPath::new(template, network_id).expect("Valid path");
    let match_value_path = TopicPath::new(match_value, network_id).expect("Valid path");
    let not_match_path = TopicPath::new(not_match, network_id).expect("Valid path");
    
    // Wrap in TemplatePathKey
    let template_key = TemplatePathKey(template_path);
    let match_value_key = TemplatePathKey(match_value_path);
    let not_match_key = TemplatePathKey(not_match_path);

    let mut hash_map = HashMap::new();
    hash_map.insert(template_key, "VALUE");
   
    let result = hash_map.get(&match_value_key);
    assert_eq!(result, Some(&"VALUE"));

    let result = hash_map.get(&not_match_key);
    assert_eq!(result, None);
}

#[test]
fn test_matches_template() {
    let template = "services/{service_path}/state";
    
    // Paths that should match
    let path1 = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    let path2 = TopicPath::new("main:services/auth/state", "default").expect("Valid path");
    
    assert!(path1.matches_template(template));
    assert!(path2.matches_template(template));
    
    // Paths that shouldn't match
    let path3 = TopicPath::new("main:services/math", "default").expect("Valid path");
    let path4 = TopicPath::new("main:users/auth/profile", "default").expect("Valid path");
    
    assert!(!path3.matches_template(template));
    assert!(!path4.matches_template(template));
}

#[test]
fn test_from_template() {
    let template = "services/{service_path}/state";
    
    // Create parameters
    let mut params = HashMap::new();
    params.insert("service_path".to_string(), "math".to_string());
    
    // Create a path from the template
    let path = TopicPath::from_template(template, params, "main").expect("Valid template");
    
    // Verify the created path
    assert_eq!(path.as_str(), "main:services/math/state");
    assert_eq!(path.service_path(), "services");
    assert_eq!(path.network_id(), "main");
    
    // Multiple parameters test
    let template2 = "{service_type}/{service_name}/{action}";
    
    let mut params2 = HashMap::new();
    params2.insert("service_type".to_string(), "internal".to_string());
    params2.insert("service_name".to_string(), "registry".to_string());
    params2.insert("action".to_string(), "list".to_string());
    
    let path2 = TopicPath::from_template(template2, params2, "main").expect("Valid template");
    assert_eq!(path2.as_str(), "main:internal/registry/list");
}

#[test]
fn test_registry_service_use_case() {
    // Template for our registry service paths
    let list_template = "services/list";
    let service_template = "services/{service_path}";
    let state_template = "services/{service_path}/state";
    let actions_template = "services/{service_path}/actions";
    let events_template = "services/{service_path}/events";
    
    // Test matching for various paths
    let list_path = TopicPath::new("main:services/list", "default").expect("Valid path");
    assert!(list_path.matches_template(list_template));
    
    let service_path = TopicPath::new("main:services/math", "default").expect("Valid path");
    assert!(service_path.matches_template(service_template));
    
    let state_path = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    assert!(state_path.matches_template(state_template));
    
    // Extract service path from a request
    let params = state_path.extract_params(state_template).expect("Template should match");
    assert_eq!(params.get("service_path"), Some(&"math".to_string()));
    
    // Create a path for a specific service's actions
    let mut params = HashMap::new();
    params.insert("service_path".to_string(), "auth".to_string());
    
    let actions_path = TopicPath::from_template(actions_template, params, "main").expect("Valid template");
    assert_eq!(actions_path.as_str(), "main:services/auth/actions");
}

#[test]
fn debug_hash_and_eq() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    // A template pattern for our Registry Service paths
    let template = "services/{service_path}";
    let match_value = "services/math";
    let network_id = "main";
   
    let template_path = TopicPath::new(template, network_id).expect("Valid path");
    let match_value_path = TopicPath::new(match_value, network_id).expect("Valid path");

    // Print these paths for debugging
    println!("Template path: {}", template_path);
    println!("Match value path: {}", match_value_path);
    
    // Calculate hash values
    let mut hasher1 = DefaultHasher::new();
    template_path.hash(&mut hasher1);
    let template_hash = hasher1.finish();
    
    let mut hasher2 = DefaultHasher::new();
    match_value_path.hash(&mut hasher2);
    let match_value_hash = hasher2.finish();
    
    println!("Template path hash: {:x}", template_hash);
    println!("Match value hash: {:x}", match_value_hash);
    println!("Hash equality: {}", template_hash == match_value_hash);
    println!("TopicPath equality: {}", template_path == match_value_path);
    
    // The problem is that the current implementation only considers paths equal
    // if they have exactly the same segments. It doesn't handle template matching
    // where {service_path} would match "math".
    
    // This assert will fail with the current implementation
    assert_eq!(template_path, match_value_path, "Template path and match value path should be equal for HashMap lookups");
}

#[test]
fn test_template_path_key() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use runar_node::routing::TemplatePathKey;
    
    // Create a template path and a concrete path that should match
    let template = "services/{service_path}";
    let match_value = "services/math";
    let network_id = "main";
   
    let template_path = TopicPath::new(template, network_id).expect("Valid path");
    let match_value_path = TopicPath::new(match_value, network_id).expect("Valid path");
    
    // Wrap in TemplatePathKey
    let template_key = TemplatePathKey(template_path);
    let match_value_key = TemplatePathKey(match_value_path);
    
    // Print for debugging
    println!("Template path: {}", template_key);
    println!("Match value path: {}", match_value_key);
    
    // Calculate hash values
    let mut hasher1 = DefaultHasher::new();
    template_key.hash(&mut hasher1);
    let template_hash = hasher1.finish();
    
    let mut hasher2 = DefaultHasher::new();
    match_value_key.hash(&mut hasher2);
    let match_value_hash = hasher2.finish();
    
    println!("Template path hash: {:x}", template_hash);
    println!("Match value hash: {:x}", match_value_hash);
    println!("Hash equality: {}", template_hash == match_value_hash);
    println!("TemplatePathKey equality: {}", template_key == match_value_key);
    
    // These should now be equal
    assert_eq!(template_key, match_value_key);
    
    // Test with HashMap
    let mut hash_map = HashMap::new();
    hash_map.insert(template_key, "VALUE");
   
    let result = hash_map.get(&match_value_key);
    assert_eq!(result, Some(&"VALUE"));
}

#[test]
fn test_template_path_key_with_wildcards() {
    use runar_node::routing::TemplatePathKey;
    
    // Create test paths
    let network_id = "main";
    
    // Single-segment wildcard
    let wildcard_path = TopicPath::new("services/*/state", network_id).expect("Valid wildcard path");
    let match_path1 = TopicPath::new("services/math/state", network_id).expect("Valid path");
    let match_path2 = TopicPath::new("services/auth/state", network_id).expect("Valid path");
    let non_match = TopicPath::new("services/math/config", network_id).expect("Valid path");
    
    // Wrap in TemplatePathKey
    let wildcard_key = TemplatePathKey(wildcard_path);
    let match_key1 = TemplatePathKey(match_path1);
    let match_key2 = TemplatePathKey(match_path2);
    let non_match_key = TemplatePathKey(non_match);
    
    // Test equality
    assert_eq!(wildcard_key, match_key1);
    assert_eq!(wildcard_key, match_key2);
    assert_ne!(wildcard_key, non_match_key);
    
    // Test with HashMap
    let mut hash_map = HashMap::new();
    hash_map.insert(wildcard_key, "WILDCARD_VALUE");
    
    // Since our hash implementation makes template paths and wildcards 
    // hash the same as concrete paths, we should be able to look up
    // using the concrete path
    let result1 = hash_map.get(&match_key1);
    assert_eq!(result1, Some(&"WILDCARD_VALUE"));
    
    let result2 = hash_map.get(&match_key2);
    assert_eq!(result2, Some(&"WILDCARD_VALUE"));
}

#[test]
fn test_simplified_template_key() {
    use runar_node::routing::TemplatePathKey;
    
    // Create paths we want to test
    let template = "services/{service_path}";
    let match_value = "services/math";
    let network_id = "main";
   
    let template_path = TopicPath::new(template, network_id).expect("Valid path");
    let match_value_path = TopicPath::new(match_value, network_id).expect("Valid path");
    
    // Create keys for direct comparison
    let template_key = TemplatePathKey(template_path.clone());
    let match_value_key = TemplatePathKey(match_value_path.clone());
    
    // Print normalized values
    // println!("Template key normalized: {}", template_key.normalize_for_hash());
    // println!("Match key normalized: {}", match_value_key.normalize_for_hash());
    
    // Check equality directly
    // println!("Direct equality check: {}", template_key == match_value_key);
    assert_eq!(template_key, match_value_key);
    
    // Debug the HashMap
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    
    // Calculate hash values for both keys
    let mut hasher1 = DefaultHasher::new();
    template_key.hash(&mut hasher1);
    let hash1 = hasher1.finish();
    
    let mut hasher2 = DefaultHasher::new();
    match_value_key.hash(&mut hasher2);
    let hash2 = hasher2.finish();
    
    println!("Template key hash: {:x}", hash1);
    println!("Match key hash: {:x}", hash2);
    println!("Hash equality: {}", hash1 == hash2);
    
    // Try directly getting with the same key instance
    let mut hash_map = HashMap::new();
    
    // First insert with a key instance
    let key1 = TemplatePathKey(template_path);
    hash_map.insert(key1, "VALUE");
    
    // Then use the SAME KEY INSTANCE to get
    let key1_clone = TemplatePathKey(match_value_path);
    let result = hash_map.get(&key1_clone);
    
    println!("Lookup result with same key: {:?}", result);
    assert_eq!(result, Some(&"VALUE"));
}

#[test]
fn test_normalized_template_matching() {
    let template_key = TopicPath::new("main:services/{service_path}", "default").unwrap();
    let match_value_key = TopicPath::new("main:services/math", "default").unwrap();
    
    // println!("Template key normalized: {}", template_key.normalize_for_hash());
    // println!("Match key normalized: {}", match_value_key.normalize_for_hash());
    
    let template_matches = template_key.matches(&match_value_key);
    let value_matches = match_value_key.matches(&template_key);
    
    assert!(!template_matches, "Template shouldn't match concrete path when tested in this direction");
    assert!(value_matches, "Concrete path should match template");
} 