use std::collections::HashMap;
use runar_node_new::routing::TopicPath;
use runar_node_new::services::RequestContext;
use runar_node_new::services::path_processor::{process_template, matches_template};
use runar_common::logging::{Logger, Component};

#[test]
fn test_process_template() {
    let logger = Logger::new_root(Component::Service, "test-node");
    
    // Create a topic path for the context
    let topic_path = TopicPath::new("services/math/state", "test_network").unwrap();
    let mut context = RequestContext::new(&topic_path, logger);
    
    // Process template
    let template = "services/{service_path_param}/state";
    let result = process_template(&mut context, template).unwrap();
    
    // Verify the template was matched and parameters extracted
    assert!(result);
    assert_eq!(context.path_params.get("service_path_param"), Some(&"math".to_string()));
    
    // Try a non-matching template
    let topic_path2 = TopicPath::new("services/math/state", "test_network").unwrap();
    let mut context2 = RequestContext::new(&topic_path2, Logger::new_root(Component::Service, "test-node"));
    
    let non_matching_template = "users/{user_id}/profile";
    let result2 = process_template(&mut context2, non_matching_template).unwrap();
    
    // Verify the template did not match
    assert!(!result2);
}

#[test]
fn test_matches_template() {
    let logger = Logger::new_root(Component::Service, "test-node");
    
    // Create a topic path for the context
    let topic_path = TopicPath::new("services/math/state", "test_network").unwrap();
    let context = RequestContext::new(&topic_path, logger);
    
    // Check matching template
    let template = "services/{service_path_param}/state";
    assert!(matches_template(&context, template));
    
    // Check non-matching template
    let non_matching_template = "users/{user_id}/profile";
    assert!(!matches_template(&context, non_matching_template));
}

#[test]
fn test_multiple_parameters() {
    let logger = Logger::new_root(Component::Service, "test-node");
    
    // Create a topic path for the context with multiple parameters
    let topic_path = TopicPath::new("services/math/actions/add", "test_network").unwrap();
    let mut context = RequestContext::new(&topic_path, logger);
    
    // Process template with multiple parameters
    let template = "services/{service_path_param}/actions/{action_name}";
    let result = process_template(&mut context, template).unwrap();
    
    // Verify the template was matched and multiple parameters extracted
    assert!(result);
    assert_eq!(context.path_params.get("service_path_param"), Some(&"math".to_string()));
    assert_eq!(context.path_params.get("action_name"), Some(&"add".to_string()));
}

#[test]
fn test_context_without_topic_path() {
    let logger = Logger::new_root(Component::Service, "test-node");
    
    // Create a simple service path topic path
    let topic_path = TopicPath::new("services/math", "test_network").unwrap();
    let mut context = RequestContext::new(&topic_path, logger.clone());
    
    // Override with None to test without a topic path
    context.topic_path = None;
    
    // Process template without a topic path
    let template = "services/{service_path_param}/state";
    let result = process_template(&mut context, template).unwrap();
    
    // Verify the function returns false when no topic path is present
    assert!(!result);
    assert!(context.path_params.is_empty());
    
    // Similarly, matches_template should return false
    assert!(!matches_template(&context, template));
} 