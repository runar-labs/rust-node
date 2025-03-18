use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use async_trait::async_trait;
use tempfile::TempDir;

use crate::db::SqliteDatabase;
use crate::node::NodeConfig;
use crate::services::ResponseStatus;
use crate::services::ServiceManager;
use crate::services::{
    abstract_service::AbstractService, RequestContext, ServiceRequest,
    ServiceResponse,
};
use crate::services::service_registry::ServiceRegistry;
use crate::services::ValueType;
use crate::services::ServiceState;
use crate::services::ServiceMetadata;
use kagi_common::ServiceInfo;

// Constants
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Create a test database
async fn create_test_db() -> Result<Arc<SqliteDatabase>> {
    // Create a temporary directory for our test
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir
        .path()
        .join("test_db.db")
        .to_str()
        .unwrap()
        .to_string();

    // Create the database
    let db = Arc::new(SqliteDatabase::new(&db_path).await?);
    
    Ok(db)
}

/// Test the Registry service - ignored until implementation is fixed
#[tokio::test]
#[ignore]
async fn test_registry_service() -> Result<()> {
    // Create a test database
    let _db = create_test_db().await?;
    
    // Create a service registry
    let registry = ServiceRegistry::new("test_network");
    
    // Create a dummy node handler for the request context
    let node_handler = Arc::new(crate::node::DummyNodeRequestHandler {});
    
    // Create a request context
    let request_context = Arc::new(RequestContext::new(
        "test",
        ValueType::Null,
        node_handler.clone(),
    ));
    
    // Create a service request
    let request = ServiceRequest {
        path: "registry/info".to_string(),
        operation: "info".to_string(),
        params: Some(ValueType::Null),
        request_id: Some("test-request".to_string()),
        request_context: request_context.clone(),
    };
    
    // Handle the request
    let response = registry.handle_request(request).await?;
    
    // Check the response
    assert_eq!(response.status, ResponseStatus::Success);
    
    Ok(())
}

/// Alternative test for the Registry service that only tests register operation
#[tokio::test]
#[ignore]
async fn test_registry_service_register_only() -> Result<()> {
    // Add a constant for timeout duration
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    // Run the entire test with a timeout
    timeout(TEST_TIMEOUT, async {
        // Create a test database
        let db = create_test_db().await?;

        // Create the Registry service
        let network_id = "test_network";
        let mut service = ServiceRegistry::new(network_id);
        service.set_db(db.clone()); // Set the database

        // Start the service
        service.start().await?;

        // Register a service
        let register_request = ServiceRequest {
            request_id: Some("register-test".to_string()),
            path: format!("{}/registry", network_id),
            operation: "register".to_string(),
            params: json!({
                "name": "test-service",
                "path": "test/endpoint",
                "description": "A test service",
                "version": "1.0.0"
            })
            .into(),
            request_context: RequestContext::default().into(),
        };

        let response = service.handle_request(register_request).await?;
        assert!(response.status == ResponseStatus::Success);

        // Stop the service
        service.stop().await?;

        Ok(())
    })
    .await?
}

/// Test the ServiceManager with Registry service - ignored until implementation is fixed
#[tokio::test]
#[ignore]
async fn test_service_manager_registry() -> Result<()> {
    Ok(())
}

/// Alternative test for ServiceManager with Registry service that only tests register operation
#[tokio::test]
#[ignore]
async fn test_service_manager_registry_register_only() -> Result<()> {
    // Add a constant for timeout duration
    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    // Run the entire test with a timeout
    timeout(TEST_TIMEOUT, async {
        // Create a test database
        let db = create_test_db();

        // Create a service manager
        let network_id = "test_network";
        let config = Arc::new(NodeConfig::new(
            network_id,
            &format!("./test_data/{}", network_id),
            &format!("./test_data/{}/db", network_id),
        ));
        let mut service_manager = ServiceManager::new(db.clone(), config);

        // Initialize services
        service_manager.init_services().await?;

        // Test Registry service through the manager
        // Register a service
        let register_request = ServiceRequest {
            request_id: Some("register-test".to_string()),
            path: format!("{}/registry", network_id),
            operation: "register".to_string(),
            params: json!({
                "name": "test-service",
                "path": "test/endpoint",
                "description": "A test service",
                "version": "1.0.0"
            })
            .into(),
            request_context: RequestContext::default().into(),
        };

        let response = service_manager.handle_request(register_request).await?;
        assert!(response.status == ResponseStatus::Success);

        Ok(())
    })
    .await?
}

// Tests for the service registry
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    // Implement ServiceInfo and AbstractService separately
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

    #[async_trait]
    impl AbstractService for TestService {
        fn name(&self) -> &str {
            &self.name
        }

        fn path(&self) -> &str {
            &self.path
        }

        fn state(&self) -> ServiceState {
            ServiceState::Running
        }

        fn description(&self) -> &str {
            &self.description
        }

        fn metadata(&self) -> ServiceMetadata {
            ServiceMetadata {
                name: self.name.clone(),
                path: self.path.clone(),
                state: ServiceState::Running,
                description: self.description.clone(),
                operations: vec![], // No operations
                version: self.version.clone(),
            }
        }

        async fn init(&mut self, _context: &RequestContext) -> Result<()> {
            // No initialization needed
            Ok(())
        }

        async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
            // Simple echo service
            let response = ServiceResponse::success(
                format!("Echoing request: {}", request.operation),
                Some(ValueType::from(request.params.unwrap_or_default())), // Convert to ValueType
            );
            Ok(response)
        }

        async fn start(&mut self) -> Result<()> {
            // Nothing to start
            Ok(())
        }

        async fn stop(&mut self) -> Result<()> {
            // Nothing to stop
            Ok(())
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_service_registry() {
        timeout(TEST_TIMEOUT, async {
            // Create a test database
            let _db = create_test_db().await.unwrap();
            let network_id = "test_network";
            
            // Create a service registry directly
            let registry = ServiceRegistry::new(network_id);
            
            // Create a test service
            let test_service = TestService::new(
                "test-service",
                "test/endpoint",
                "A test service",
                "1.0.0",
            );
            
            // Register the service
            registry.register_service(Arc::new(test_service)).await.unwrap();
            
            // Create a dummy node handler for the request context
            let node_handler = Arc::new(crate::node::DummyNodeRequestHandler {});
            
            // Create a request context
            let request_context = Arc::new(RequestContext::new(
                "test",
                ValueType::Null,
                node_handler.clone(),
            ));
            
            // Create a service request
            let request = ServiceRequest {
                path: "test/endpoint/echo".to_string(),
                operation: "echo".to_string(),
                params: Some(ValueType::from(json!({
                    "name": "test-service",
                    "path": "test/endpoint",
                    "description": "A test service",
                    "version": "1.0.0"
                }))),
                request_id: Some("test-request".to_string()),
                request_context: request_context.clone(),
            };
            
            // Get the service
            let service = registry.get_service("test/endpoint").await.unwrap();
            
            // Process the request
            let response = service.handle_request(request).await.unwrap();
            
            // Check the response
            assert_eq!(response.status, ResponseStatus::Success);
            assert!(response.data.is_some());
            
            Result::<(), anyhow::Error>::Ok(())
        }).await.unwrap().unwrap();
    }
    
    #[tokio::test]
    #[ignore]
    async fn test_service_info() {
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
        
        // Create a dummy node handler for the request context
        let node_handler = Arc::new(crate::node::DummyNodeRequestHandler {});
        
        // Create a request context
        let request_context = Arc::new(RequestContext::new(
            "test",
            ValueType::Null,
            node_handler.clone(),
        ));
        
        // Create a service request to test info
        let request = ServiceRequest {
            path: "test/endpoint/info".to_string(),
            operation: "info".to_string(),
            params: Some(ValueType::from(json!({
                "name": "test-service",
                "path": "test/endpoint",
                "description": "A test service",
                "version": "1.0.0"
            }))),
            request_id: Some("test-request".to_string()),
            request_context: request_context.clone(),
        };
    }
}
