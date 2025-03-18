use kagi_common::ServiceInfo;

#[test]
fn test_service_info_trait() {
    // Define a simple test service
    struct TestService {
        name: String,
        path: String,
        description: String,
        version: String,
    }

    impl TestService {
        fn new(name: &str, path: &str, description: &str, version: &str) -> Self {
            Self {
                name: name.to_string(),
                path: path.to_string(),
                description: description.to_string(),
                version: version.to_string(),
            }
        }
    }

    impl ServiceInfo for TestService {
        fn service_name(&self) -> &str {
            &self.name
        }

        fn service_path(&self) -> &str {
            &self.path
        }

        fn service_description(&self) -> &str {
            &self.description
        }

        fn service_version(&self) -> &str {
            &self.version
        }
    }

    // Create a test service
    let test_service = TestService::new(
        "test-service",
        "test/endpoint",
        "A test service",
        "1.0.0",
    );
    
    // Check service info
    assert_eq!(test_service.service_name(), "test-service");
    assert_eq!(test_service.service_path(), "test/endpoint");
    assert_eq!(test_service.service_description(), "A test service");
    assert_eq!(test_service.service_version(), "1.0.0");
} 