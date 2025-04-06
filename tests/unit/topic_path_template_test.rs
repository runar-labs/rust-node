use std::collections::HashMap;
use runar_node::routing::TopicPath;

#[test]
fn test_extract_params_from_template() {
    // A template pattern for our Registry Service paths
    let template = "services/{service_path}/state";
    
    // An actual path that matches the template
    let path = TopicPath::new("main:services/math/state", "default").expect("Valid path");
    
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