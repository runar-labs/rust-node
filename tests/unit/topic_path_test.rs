use anyhow::Result;
use runar_node_new::routing::TopicPath;

/// Comprehensive test suite for TopicPath
///
/// INTENTION: Verify that TopicPath correctly handles path parsing, manipulation,
/// and validation according to documented requirements. Covers all methods and edge cases.
#[cfg(test)]
mod topic_path_tests {
    use super::*;

    /// Test TopicPath::new() constructor with various valid inputs
    #[test]
    fn test_new_valid_paths() {
        // Test with network_id prefix
        let path = TopicPath::new("main:auth/login", "default").expect("Valid path");
        assert_eq!(path.network_id(), "main");
        assert_eq!(path.service_path(), "auth");
        assert_eq!(path.get_last_segment(), "login");
        assert_eq!(path.as_str(), "main:auth/login");
        assert_eq!(path.action_path(), "auth/login");

        // Test without network_id (uses default)
        let path = TopicPath::new("auth/login", "default").expect("Valid path");
        assert_eq!(path.network_id(), "default");
        assert_eq!(path.service_path(), "auth");
        assert_eq!(path.get_last_segment(), "login");
        assert_eq!(path.as_str(), "default:auth/login");
        assert_eq!(path.action_path(), "auth/login");

        // Test with just service name
        let path = TopicPath::new("auth", "default").expect("Valid path");
        assert_eq!(path.network_id(), "default");
        assert_eq!(path.service_path(), "auth");
        assert_eq!(path.as_str(), "default:auth");
        assert_eq!(path.action_path(), "");
        
        // Test with multiple path segments
        let path = TopicPath::new("main:auth/users/details", "default").expect("Valid path");
        assert_eq!(path.network_id(), "main");
        assert_eq!(path.service_path(), "auth"); 
        assert_eq!(path.as_str(), "main:auth/users/details");
        assert_eq!(path.action_path(), "auth/users/details");
    }

    /// Test TopicPath::new() constructor with invalid inputs
    #[test]
    fn test_new_invalid_paths() {
        // Empty path
        assert!(TopicPath::new("", "default").is_err());
        
        // Multiple colons
        assert!(TopicPath::new("main:auth:login", "default").is_err());
        
        // Empty network ID
        assert!(TopicPath::new(":auth/login", "default").is_err());
    }

    /// Test TopicPath::new_service() constructor
    #[test]
    fn test_new_service() {
        let path = TopicPath::new_service("main", "auth");
        assert_eq!(path.network_id(), "main");
        assert_eq!(path.service_path(), "auth");
        assert_eq!(path.as_str(), "main:auth");
        assert_eq!(path.action_path(), "");
        // Service-only paths don't have an action/event segment
        assert_eq!(path.get_segments().len(), 1);
    }

    /// Test TopicPath::child() for creating child paths
    #[test]
    fn test_child() {
        // Create a base path and add a child
        let base = TopicPath::new("main:auth", "default").expect("Valid path");
        let child = base.child("login").expect("Valid child path");
        
        assert_eq!(child.as_str(), "main:auth/login");
        assert_eq!(child.network_id(), "main");
        assert_eq!(child.service_path(), "auth");
        assert_eq!(child.action_path(), "auth/login");
        
        // Add another child
        let nested_child = child.child("advanced").expect("Valid nested child");
        assert_eq!(nested_child.as_str(), "main:auth/login/advanced");
 
        
        // Test invalid child (with slash)
        assert!(base.child("invalid/segment").is_err());
    }

    /// Test TopicPath::parent() for creating parent paths
    #[test]
    fn test_parent() {
        // Create a nested path
        let path = TopicPath::new("main:auth/users/details", "default").expect("Valid path");
        
        // Get parent (one level up)
        let parent = path.parent().expect("Valid parent");
        assert_eq!(parent.as_str(), "main:auth/users");
        // The service_path should remain the same as the original path (just the service name)
        assert_eq!(parent.service_path(), "auth");
        
        // Get grandparent (two levels up)
        let grandparent = parent.parent().expect("Valid grandparent");
        assert_eq!(grandparent.as_str(), "main:auth");
        
        // Cannot get parent of root path
        assert!(grandparent.parent().is_err());
        
        // Cannot get parent of service-only path
        let service_only = TopicPath::new("main:service", "default").expect("Valid path");
        assert!(service_only.parent().is_err());
    }

    /// Test TopicPath::starts_with() for path prefix matching
    #[test]
    fn test_starts_with() {
        let path = TopicPath::new("main:auth/users/list", "default").expect("Valid path");
        
        // Test with matching prefixes
        let prefix1 = TopicPath::new("main:auth", "default").expect("Valid path");
        let prefix2 = TopicPath::new("main:auth/users", "default").expect("Valid path");
        
        assert!(path.starts_with(&prefix1));
        assert!(path.starts_with(&prefix2));
        
        // Test with non-matching prefixes
        let different_network = TopicPath::new("other:auth/users", "default").expect("Valid path");
        let different_service = TopicPath::new("main:payments", "default").expect("Valid path");
        
        assert!(!path.starts_with(&different_network));
        assert!(!path.starts_with(&different_service));
    }

    /// Test TopicPath::get_segments() for path segment extraction
    #[test]
    fn test_get_segments() {
        // Simple path
        let path1 = TopicPath::new("main:auth/login", "default").expect("Valid path");
        let segments1 = path1.get_segments();
        assert_eq!(segments1, vec!["auth", "login"]);
        
        // Complex path with multiple segments
        let path2 = TopicPath::new("main:auth/users/profile/edit", "default").expect("Valid path");
        let segments2 = path2.get_segments();
        assert_eq!(segments2, vec!["auth", "users", "profile", "edit"]);
        
        // Path with service name only
        let path3 = TopicPath::new("main:auth", "default").expect("Valid path");
        let segments3 = path3.get_segments();
        assert_eq!(segments3, vec!["auth"]);
    }

    /// Test TopicPath::get_last_segment() for extracting the action or event
    #[test]
    fn test_get_last_segment() {
        // Simple path
        let path1 = TopicPath::new("main:auth/login", "default").expect("Valid path");
        assert_eq!(path1.get_last_segment(), "login");
        
        // Complex path with multiple segments
        let path2 = TopicPath::new("main:auth/users/profile/edit", "default").expect("Valid path");
        assert_eq!(path2.get_last_segment(), "edit");
        
        // Path with service name only
        let path3 = TopicPath::new("main:auth", "default").expect("Valid path");
        assert_eq!(path3.get_last_segment(), "auth");
    }

    /// Test TopicPath::test_default() helper for tests
    #[test]
    fn test_default_helper() {
        let path = TopicPath::test_default("auth/login");
        assert_eq!(path.network_id(), "default");
        assert_eq!(path.service_path(), "auth");
        assert_eq!(path.get_last_segment(), "login");
        assert_eq!(path.as_str(), "default:auth/login");
    }

    /// Test consistency between methods
    #[test]
    fn test_method_consistency() {
        let path = TopicPath::new("main:service/action", "default").expect("Valid path");
        
        // The service_path should return just the service name (first segment)
        assert_eq!(path.service_path(), "service");
        
        // The segments should include all parts
        let segments = path.get_segments();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], "service");
        assert_eq!(segments[1], "action");
        
        // The last segment should be the action
        assert_eq!(path.get_last_segment(), "action");
        
        // The action_path should include everything after the network ID
        assert_eq!(path.action_path(), "service/action");
    }

    /// Test with unusual but valid paths
    #[test]
    fn test_unusual_paths() {
        // Network ID with special characters
        let path1 = TopicPath::new("test-network_01:service", "default").expect("Valid path");
        assert_eq!(path1.network_id(), "test-network_01");
        assert_eq!(path1.service_path(), "service");
        
        // Service path with special characters
        let path2 = TopicPath::new("main:my-service_01", "default").expect("Valid path");
        assert_eq!(path2.service_path(), "my-service_01");
        
        // Long paths with many segments
        let path3 = TopicPath::new("main:service/a/b/c/d/e/f", "default").expect("Valid path");
        assert_eq!(path3.get_last_segment(), "f");
        assert_eq!(path3.get_segments().len(), 7);
    }

    /// Test service paths with embedded slashes
    #[test]
    fn test_service_paths_with_slashes() {
        // Test with internal service path using $ prefix
        let path = TopicPath::new("test_network:$registry/services/list", "default").expect("Valid path");
        
        // The service_path should be "$registry" - first segment only
        assert_eq!(path.service_path(), "$registry");
        
        // Verify the action_path includes the complete path after network ID
        assert_eq!(path.action_path(), "$registry/services/list");
        
        // Test parameters extraction with path templates
        let template = "services/{service_path}/state";
        let path = TopicPath::new("test_network:$registry/services/math/state", "default").expect("Valid path");
        
        let params = path.extract_params(template);
        
        // This will fail because segment counts don't match:
        // - Path segments: ["$registry", "services", "math", "state"]
        // - Template segments: ["services", "{service_path}", "state"]
        assert!(params.is_err());
    }

    /// Test with internal registry service path using $ prefix
    #[test]
    fn test_registry_service_paths() {
        // Create a service with $ prefix path
        let service_path = "$registry";
        
        // Register action with path "services/list" 
        let action_path = "services/list";
        
        // The full action path that should be constructed when handling requests
        let expected_full_path = "test_network:$registry/services/list";
        
        // Simulate how paths should be handled 
        let path = TopicPath::new(&format!("test_network:{}/{}", service_path, action_path), "default")
            .expect("Valid path");
        
        assert_eq!(path.as_str(), expected_full_path);
        
        // Test with template pattern for service state
        let template = "services/{service_path}/state";
        
        // Get a service state path that matches the template
        let service_state_path = TopicPath::new(
            "test_network:services/math/state", 
            "default"
        ).expect("Valid path");
        
        // Extract parameters from the path
        let params = service_state_path.extract_params(template).expect("Template should match");
        assert_eq!(params.get("service_path"), Some(&"math".to_string()));
    }
} 