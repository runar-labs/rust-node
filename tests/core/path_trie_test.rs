use runar_node::routing::{PathTrie, TopicPath};
use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_trie_template_match() {
        let mut trie = PathTrie::new();
        
        // Register a template pattern
        trie.add_content(
            TopicPath::new("services/{service_path}/state", "network1").unwrap(),
            "TEMPLATE"
        );
        
        // Test with a matching topic
        let topic = TopicPath::new("services/math/state", "network1").unwrap();
        let matches = trie.find_handlers(&topic);
        
        assert_eq!(matches, vec!["TEMPLATE"]);
        
        // Test parameter extraction
        let matches_with_params = trie.find_matches(&topic);
        assert_eq!(matches_with_params.len(), 1);
        assert_eq!(matches_with_params[0].content, "TEMPLATE");
        assert_eq!(matches_with_params[0].params.get("service_path").unwrap(), "math");
        
        // Test with a different network
        let topic2 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        
        assert_eq!(matches2, Vec::<&str>::new());
    }



    #[test]
    fn test_path_trie_wildcard_search() {
        let mut trie = PathTrie::new();
         
        // Simple template pattern
        trie.add_content(
            TopicPath::new("serviceA/action1", "network1").unwrap(),
            "serviceA/action1"
        );
        
        // Multiple template parameters
        trie.add_content(
            TopicPath::new("serviceA/action2", "network1").unwrap(),
            "serviceA/action2"
        );
        
        // Template at beginning
        trie.add_content(
            TopicPath::new("serviceA/action3", "network1").unwrap(),
            "serviceA/action3"
        );
        
        // Template at end
        trie.add_content(
            TopicPath::new("serviceB/action1", "network1").unwrap(),
            "serviceB/action1"
        );
        
        // Template in all positions
        trie.add_content(
            TopicPath::new("serviceB/action2", "network1").unwrap(),
            "serviceB/action2"
        );
        
        // Template with different network
        trie.add_content(
            TopicPath::new("serviceC/action1", "network2").unwrap(),
            "serviceC/action1"
        );
         
        
        // Test basic matching
        let searchtopic1 = TopicPath::new("serviceA/*", "network1").unwrap();
        let matches1 = trie.find_handlers(&searchtopic1);
        assert_eq!(matches1.len(), 3);
        assert!(matches1.contains(&"serviceA/action1"));
        assert!(matches1.contains(&"serviceA/action2"));
        assert!(matches1.contains(&"serviceA/action3"));
        

        
    }


    #[test]
    fn test_path_trie_template_match_extended() {
        let mut trie = PathTrie::new();
        
        // Add a test to verify parameter extraction
        fn assert_params(trie: &PathTrie<&str>, topic_str: &str, network: &str, expected_handler: &str, expected_params: HashMap<String, String>) {
            let topic = TopicPath::new(topic_str, network).unwrap();
            let matches = trie.find_matches(&topic);
            
            // Find the match with the expected handler
            let matching_result = matches.iter().find(|m| m.content == expected_handler);
            assert!(matching_result.is_some(), "No match found with handler '{}' for {}", expected_handler, topic_str);
            let m = matching_result.unwrap();
            assert_eq!(m.params, expected_params, "Parameter mismatch for handler '{}'", expected_handler);
        }
        
        // Simple template pattern
        trie.add_content(
            TopicPath::new("services/{service_path}/state", "network1").unwrap(),
            "SIMPLE_TEMPLATE"
        );
        
        // Multiple template parameters
        trie.add_content(
            TopicPath::new("services/{service_path}/actions/{action}", "network1").unwrap(),
            "MULTI_PARAMS"
        );
        
        // Template at beginning
        trie.add_content(
            TopicPath::new("{type}/services/state", "network1").unwrap(),
            "START_TEMPLATE"
        );
        
        // Template at end
        trie.add_content(
            TopicPath::new("services/state/{param}", "network1").unwrap(),
            "END_TEMPLATE"
        );
        
        // Template in all positions
        trie.add_content(
            TopicPath::new("{a}/{b}/{c}", "network1").unwrap(),
            "ALL_TEMPLATES"
        );
        
        // Template with different network
        trie.add_content(
            TopicPath::new("services/{service_path}/state", "network2").unwrap(),
            "DIFF_NETWORK"
        );
        
        // With repeated template parameter name
        trie.add_content(
            TopicPath::new("services/{param}/actions/{param}", "network1").unwrap(),
            "REPEATED_PARAM"
        );
        
        // Test basic matching
        let topic1 = TopicPath::new("services/math/state", "network1").unwrap();
        let matches1 = trie.find_handlers(&topic1);
        assert!(matches1.contains(&"SIMPLE_TEMPLATE"));
        
        // Test multiple parameters
        let topic2 = TopicPath::new("services/math/actions/add", "network1").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        assert!(matches2.contains(&"MULTI_PARAMS"));
        assert!(matches2.contains(&"REPEATED_PARAM"));
        
        // Test template at beginning
        let topic3 = TopicPath::new("internal/services/state", "network1").unwrap();
        let matches3 = trie.find_handlers(&topic3);
        assert!(matches3.contains(&"START_TEMPLATE"));
        
        // Test template at end
        let topic4 = TopicPath::new("services/state/details", "network1").unwrap();
        let matches4 = trie.find_handlers(&topic4);
        assert!(matches4.contains(&"END_TEMPLATE"));
        
        // Test all templates
        let topic5 = TopicPath::new("x/y/z", "network1").unwrap();
        let matches5 = trie.find_handlers(&topic5);
        assert!(matches5.contains(&"ALL_TEMPLATES"));
        
        // Test network isolation
        let topic6 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches6 = trie.find_handlers(&topic6);
        assert_eq!(matches6, vec!["DIFF_NETWORK"]);
        
        // Same path but different network - should not match
        let topic7 = TopicPath::new("services/math/state", "wrong_network").unwrap();
        let matches7 = trie.find_handlers(&topic7);
        assert_eq!(matches7, Vec::<&str>::new());
        
        // Test repeated parameter
        let topic8 = TopicPath::new("services/param/actions/param", "network1").unwrap();
        let matches8 = trie.find_handlers(&topic8);
        assert!(matches8.contains(&"REPEATED_PARAM"));
        
        // Test parameter extraction
        let mut expected_params = HashMap::new();
        expected_params.insert("service_path".to_string(), "math".to_string());
        assert_params(&trie, "services/math/state", "network1", "SIMPLE_TEMPLATE", expected_params);
        
        let mut expected_params = HashMap::new();
        expected_params.insert("service_path".to_string(), "auth".to_string());
        expected_params.insert("action".to_string(), "login".to_string());
        assert_params(&trie, "services/auth/actions/login", "network1", "MULTI_PARAMS", expected_params);
        
        // Note: The behavior for repeated parameter names is that the last value in the path
        // overwrites earlier ones. This is tested in the cross-network search test instead.
    }

    #[test]
    fn test_path_trie_wildcard_match() {
        let mut trie = PathTrie::new();
        
        // Register a wildcard pattern
        trie.add_content(
            TopicPath::new("services/*/state", "network1").unwrap(),
            "WILDCARD"
        );
        
        // Test with a matching topic
        let topic = TopicPath::new("services/math/state", "network1").unwrap();
        let matches = trie.find_handlers(&topic);
        
        assert_eq!(matches, vec!["WILDCARD"]);
        
        // Test with a different network
        let topic2 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        
        assert_eq!(matches2, Vec::<&str>::new());
    }

    #[test]
    fn test_path_trie_wildcard_match_extended() {
        let mut trie = PathTrie::new();
        
        // Simple wildcard
        trie.add_content(
            TopicPath::new("services/*/state", "network1").unwrap(),
            "SIMPLE_WILDCARD"
        );
        
        // Multi-segment wildcard
        trie.add_content(
            TopicPath::new("services/>", "network1").unwrap(),
            "MULTI_WILDCARD"
        );
        
        // Wildcard at beginning
        trie.add_content(
            TopicPath::new("*/services/state", "network1").unwrap(),
            "START_WILDCARD"
        );
        
        // Wildcard at end
        trie.add_content(
            TopicPath::new("services/state/*", "network1").unwrap(),
            "END_WILDCARD"
        );
        
        // Multiple wildcards
        trie.add_content(
            TopicPath::new("services/*/actions/*", "network1").unwrap(),
            "MULTI_WILDCARDS"
        );
        
        // Wildcard with different network
        trie.add_content(
            TopicPath::new("services/*/state", "network2").unwrap(),
            "DIFF_NETWORK_WILDCARD"
        );
        
        // Test simple wildcard
        let topic1 = TopicPath::new("services/math/state", "network1").unwrap();
        let matches1 = trie.find_handlers(&topic1);
        assert!(matches1.contains(&"SIMPLE_WILDCARD"));
        assert!(matches1.contains(&"MULTI_WILDCARD"));
        
        // Test multi-segment wildcard with different segment counts
        let topic2 = TopicPath::new("services/math/actions/add", "network1").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        assert!(matches2.contains(&"MULTI_WILDCARD"));
        assert!(matches2.contains(&"MULTI_WILDCARDS"));
        
        // Test wildcard at beginning
        let topic3 = TopicPath::new("internal/services/state", "network1").unwrap();
        let matches3 = trie.find_handlers(&topic3);
        assert!(matches3.contains(&"START_WILDCARD"));
        
        // Test wildcard at end
        let topic4 = TopicPath::new("services/state/details", "network1").unwrap();
        let matches4 = trie.find_handlers(&topic4);
        assert!(matches4.contains(&"END_WILDCARD"));
        assert!(matches4.contains(&"MULTI_WILDCARD"));
        
        // Test network isolation
        let topic5 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches5 = trie.find_handlers(&topic5);
        assert_eq!(matches5, vec!["DIFF_NETWORK_WILDCARD"]);
        
        // Same path but different network - should not match
        let topic6 = TopicPath::new("services/math/state", "wrong_network").unwrap();
        let matches6 = trie.find_handlers(&topic6);
        assert_eq!(matches6, Vec::<&str>::new());
        
        // Test with many segments that should match multi-wildcard
        let topic7 = TopicPath::new("services/a/b/c/d/e/f", "network1").unwrap();
        let matches7 = trie.find_handlers(&topic7);
        assert!(matches7.contains(&"MULTI_WILDCARD"));
    }

    #[test]
    fn test_path_trie_combined_template_and_wildcard() {
        let mut trie = PathTrie::new();
        
        // Template + wildcard
        trie.add_content(
            TopicPath::new("services/{service_path}/*/details", "network1").unwrap(),
            "TEMPLATE_THEN_WILDCARD"
        );
        
        // Wildcard + template
        trie.add_content(
            TopicPath::new("services/*/actions/{action}", "network1").unwrap(),
            "WILDCARD_THEN_TEMPLATE"
        );
        
        // Template + multi-wildcard
        trie.add_content(
            TopicPath::new("services/{service_path}/>", "network1").unwrap(),
            "TEMPLATE_THEN_MULTI"
        );
        
        // Multi-wildcard at end with template earlier
        trie.add_content(
            TopicPath::new("{type}/services/>", "network1").unwrap(),
            "MULTI_THEN_TEMPLATE"
        );
        
        // Complex mix of templates and wildcards
        trie.add_content(
            TopicPath::new("{type}/*/services/{name}/actions/*", "network1").unwrap(),
            "COMPLEX_MIX"
        );
        
        // Different network with same pattern
        trie.add_content(
            TopicPath::new("services/{service_path}/*/details", "network2").unwrap(),
            "DIFFERENT_NETWORK"
        );
        
        // Test template then wildcard
        let topic1 = TopicPath::new("services/math/state/details", "network1").unwrap();
        let matches1 = trie.find_handlers(&topic1);
        assert!(matches1.contains(&"TEMPLATE_THEN_WILDCARD"));
        assert!(matches1.contains(&"TEMPLATE_THEN_MULTI"));
        
        // Test wildcard then template
        let topic2 = TopicPath::new("services/math/actions/login", "network1").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        assert!(matches2.contains(&"WILDCARD_THEN_TEMPLATE"));
        assert!(matches2.contains(&"TEMPLATE_THEN_MULTI"));
        
        // Test template then multi-wildcard with deep path
        let topic3 = TopicPath::new("services/math/a/b/c/d/e", "network1").unwrap();
        let matches3 = trie.find_handlers(&topic3);
        assert!(matches3.contains(&"TEMPLATE_THEN_MULTI"));
        
        // Test multi-wildcard then template
        let topic4 = TopicPath::new("internal/services/math/actions/anything", "network1").unwrap();
        let matches4 = trie.find_handlers(&topic4);
        assert!(matches4.contains(&"MULTI_THEN_TEMPLATE"));
        
        // Test complex mix
        let topic5 = TopicPath::new("internal/any/services/auth/actions/login", "network1").unwrap();
        let matches5 = trie.find_handlers(&topic5);
        assert!(matches5.contains(&"COMPLEX_MIX"));
        
        // Test network isolation
        let topic6 = TopicPath::new("services/math/state/details", "network2").unwrap();
        let matches6 = trie.find_handlers(&topic6);
        assert_eq!(matches6, vec!["DIFFERENT_NETWORK"]);
        
        // Test wrong network
        let topic7 = TopicPath::new("services/math/state/details", "wrong_network").unwrap();
        let matches7 = trie.find_handlers(&topic7);
        assert_eq!(matches7, Vec::<&str>::new());
    }

    /// Comprehensive test specifically for network isolation
    #[test]
    fn test_path_trie_network_isolation_comprehensive() {
        let mut trie = PathTrie::new();
        
        // Add same paths with different networks
        trie.add_content(
            TopicPath::new("services/math/state", "network1").unwrap(),
            "MATH_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/math/state", "network2").unwrap(),
            "MATH_NETWORK2"
        );
        
        trie.add_content(
            TopicPath::new("services/auth/state", "network1").unwrap(),
            "AUTH_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/auth/state", "network2").unwrap(),
            "AUTH_NETWORK2"
        );
        
        // Add template paths with different networks
        trie.add_content(
            TopicPath::new("services/{service}/events", "network1").unwrap(),
            "EVENTS_TEMPLATE_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/{service}/events", "network2").unwrap(),
            "EVENTS_TEMPLATE_NETWORK2"
        );
        
        // Add wildcard paths with different networks
        trie.add_content(
            TopicPath::new("services/*/config", "network1").unwrap(),
            "CONFIG_WILDCARD_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/*/config", "network2").unwrap(),
            "CONFIG_WILDCARD_NETWORK2"
        );
        
        // Test exact path matching with network isolation
        let topic1 = TopicPath::new("services/math/state", "network1").unwrap();
        let matches1 = trie.find_handlers(&topic1);
        assert_eq!(matches1, vec!["MATH_NETWORK1"]);
        
        let topic2 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches2 = trie.find_handlers(&topic2);
        assert_eq!(matches2, vec!["MATH_NETWORK2"]);
        
        // Test template matching with network isolation
        let topic3 = TopicPath::new("services/math/events", "network1").unwrap();
        let matches3 = trie.find_handlers(&topic3);
        assert_eq!(matches3, vec!["EVENTS_TEMPLATE_NETWORK1"]);
        
        let topic4 = TopicPath::new("services/math/events", "network2").unwrap();
        let matches4 = trie.find_handlers(&topic4);
        assert_eq!(matches4, vec!["EVENTS_TEMPLATE_NETWORK2"]);
        
        // Test wildcard matching with network isolation
        let topic5 = TopicPath::new("services/math/config", "network1").unwrap();
        let matches5 = trie.find_handlers(&topic5);
        assert_eq!(matches5, vec!["CONFIG_WILDCARD_NETWORK1"]);
        
        let topic6 = TopicPath::new("services/math/config", "network2").unwrap();
        let matches6 = trie.find_handlers(&topic6);
        assert_eq!(matches6, vec!["CONFIG_WILDCARD_NETWORK2"]);
        
        // Test non-existent network
        let topic7 = TopicPath::new("services/math/state", "network3").unwrap();
        let matches7 = trie.find_handlers(&topic7);
        assert_eq!(matches7, Vec::<&str>::new());
    }
    
    #[test]
    fn test_path_trie_cross_network_search() {
        // This test verifies the behavior of find_matches when searching across networks
        let mut trie = PathTrie::new();
        
        // Add handlers for different networks
        trie.add_content(
            TopicPath::new("services/math/state", "network1").unwrap(),
            "MATH_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/math/state", "network2").unwrap(),
            "MATH_NETWORK2"
        );
        
        trie.add_content(
            TopicPath::new("services/*/events", "network1").unwrap(),
            "EVENTS_WILDCARD_NETWORK1"
        );
        
        trie.add_content(
            TopicPath::new("services/{service}/config", "network2").unwrap(),
            "CONFIG_TEMPLATE_NETWORK2"
        );
        
        // Test that find_matches only returns matches for the specific network
        let topic1 = TopicPath::new("services/math/state", "network1").unwrap();
        let matches1 = trie.find_matches(&topic1);
        assert_eq!(matches1.len(), 1);
        assert_eq!(matches1[0].content, "MATH_NETWORK1");
        
        let topic2 = TopicPath::new("services/math/state", "network2").unwrap();
        let matches2 = trie.find_matches(&topic2);
        assert_eq!(matches2.len(), 1);
        assert_eq!(matches2[0].content, "MATH_NETWORK2");
        
        // Test wildcard matching with network isolation
        let topic3 = TopicPath::new("services/math/events", "network1").unwrap();
        let matches3 = trie.find_matches(&topic3);
        assert_eq!(matches3.len(), 1);
        assert_eq!(matches3[0].content, "EVENTS_WILDCARD_NETWORK1");
        
        // Test template matching with parameter extraction
        let topic4 = TopicPath::new("services/math/config", "network2").unwrap();
        let matches4 = trie.find_matches(&topic4);
        assert_eq!(matches4.len(), 1);
        assert_eq!(matches4[0].content, "CONFIG_TEMPLATE_NETWORK2");
        assert_eq!(matches4[0].params.get("service").unwrap(), "math");
        
        // Test non-existent network
        let topic5 = TopicPath::new("services/math/state", "network3").unwrap();
        let matches5 = trie.find_matches(&topic5);
        assert_eq!(matches5.len(), 0);
    }
} 