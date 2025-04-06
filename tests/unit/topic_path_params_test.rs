use std::collections::HashMap;
use runar_node::routing::TopicPath;
use runar_node::services::RequestContext;
use runar_common::logging::{Logger, Component};

#[test]
fn test_extract_params() {
    // Create a topic path
    let topic_path = TopicPath::new("services/math/state", "test_network").unwrap();
    
    // Extract parameters using template
    let template = "services/{service_path_param}/state";
    let params = topic_path.extract_params(template).unwrap();
    
    // Verify parameters were extracted correctly
    assert_eq!(params.get("service_path_param"), Some(&"math".to_string()));
    
    // Try a non-matching template
    let non_matching_template = "users/{user_id}/profile";
    let result = topic_path.extract_params(non_matching_template);
    
    // Verify the template did not match
    assert!(result.is_err());
}

#[test]
fn test_matches_template() {
    // Create a topic path
    let topic_path = TopicPath::new("services/math/state", "test_network").unwrap();
    
    // Check matching template
    let template = "services/{service_path_param}/state";
    assert!(topic_path.matches_template(template));
    
    // Check non-matching template
    let non_matching_template = "users/{user_id}/profile";
    assert!(!topic_path.matches_template(non_matching_template));
}

#[test]
fn test_multiple_parameters() {
    // Create a topic path with multiple parameters
    let topic_path = TopicPath::new("services/math/actions/add", "test_network").unwrap();
    
    // Extract multiple parameters
    let template = "services/{service_path_param}/actions/{action_name}";
    let params = topic_path.extract_params(template).unwrap();
    
    // Verify multiple parameters were extracted correctly
    assert_eq!(params.get("service_path_param"), Some(&"math".to_string()));
    assert_eq!(params.get("action_name"), Some(&"add".to_string()));
}

#[test]
fn test_request_context_with_extracted_params() {
    let logger = Logger::new_root(Component::Service, "test-node");
    
    // Create a topic path for the context
    let topic_path = TopicPath::new("services/math/state", "test_network").unwrap();
    let mut context = RequestContext::new(&topic_path, logger);
    
    // Extract parameters and set them in the context
    let template = "services/{service_path_param}/state";
    let params = topic_path.extract_params(template).unwrap();
    context.path_params = params;
    
    // Verify the context now contains the parameters
    assert_eq!(context.path_params.get("service_path_param"), Some(&"math".to_string()));
}

#[test]
fn test_from_template() {
    // Create parameters map
    let mut params = HashMap::new();
    params.insert("service_path".to_string(), "math".to_string());
    params.insert("action".to_string(), "add".to_string());
    
    // Create a topic path from template
    let template = "services/{service_path}/actions/{action}";
    let topic_path = TopicPath::from_template(template, params, "test_network").unwrap();
    
    // Verify the generated path
    assert_eq!(topic_path.as_str(), "test_network:services/math/actions/add");
} 