use anyhow::{anyhow, Result};
use async_trait::async_trait;
use runar_node::node::{Node, NodeConfig};
use runar_node::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType, vmap};
use runar_node::util::logging::{info_log, debug_log, error_log, Component, configure_test_logging};
use serde_json::json;
use std::sync::Mutex;
use tempfile::tempdir;
use tokio::time::timeout;
use uuid::Uuid;

use std::sync::Arc;
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::Once;

mod test_utils;
use test_utils::{create_test_node, MockDocumentStore, SimpleTestService};

// Use this to initialize the logger only once
static INIT_LOGGER: Once = Once::new();

// Helper function for logging
async fn log_info(message: &str) {
    info_log(Component::Test, message).await;
}

// Initialize logging once at the beginning of tests
fn init_logging() {
    INIT_LOGGER.call_once(|| {
        configure_test_logging();
    });
}

/// Authentication service that demonstrates service-to-service communication
struct AuthService {
    name: String,
    path: String,
    users: Arc<RwLock<HashMap<String, ValueType>>>,
    tokens: Arc<RwLock<HashMap<String, String>>>,
}

impl AuthService {
    /// Create a new AuthService
    pub fn new(name: &str) -> Self {
        // Create a HashMap for users
        let mut users = HashMap::new();
        
        // Add test users using json! macro
        users.insert(
            "admin".to_string(),
            ValueType::Json(json!({
                "username": "admin",
                "password": "password123",
                "role": "admin"
            }))
        );
        
        users.insert(
            "user".to_string(),
            ValueType::Json(json!({
                "username": "user",
                "password": "user123",
                "role": "user"
            }))
        );
        
        // Add testuser account for test_service_to_service_communication test
        users.insert(
            "testuser".to_string(),
            ValueType::Json(json!({
                "username": "testuser",
                "password": "password123",
                "role": "user"
            }))
        );
        
        Self {
            name: name.to_string(),
            path: format!("/auth/{}", name),
            users: Arc::new(RwLock::new(users)),
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Login a user with username and password
    async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<ServiceResponse> {
        log_info(&format!("AuthService::login - username: {}, password: {}", username, password)).await;
        
        // Log first, then acquire the lock
        log_info("Checking user credentials").await;
        
        // Simulate user lookup
        let user_value = {
            let users = self.users.read().unwrap();
            // Copy the user value to avoid holding the lock across an await point
            users.get(username).cloned()
        };
        
        if let Some(user_value) = user_value {
            log_info(&format!("AuthService::login - found user: {:?}", user_value)).await;
            // Extract password and check it
            if let ValueType::Json(user_data) = user_value {
                let stored_password = user_data.get("password").and_then(|v| v.as_str());
                log_info(&format!("AuthService::login - stored_password: {:?}", stored_password)).await;
                if let Some(pwd) = stored_password {
                    if pwd == password {
                        // Generate token
                        let token = uuid::Uuid::new_v4().to_string();
                        
                        // Store token
                        let mut tokens = self.tokens.write().unwrap();
                        tokens.insert(token.clone(), username.to_string());
                        
                        // Return success with token
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Success,
                            message: "Login successful".to_string(),
                            data: Some(ValueType::Json(json!({
                                "token": token
                            }))),
                        });
                    }
                }
            }
        }
        
        // Return error for invalid credentials
        log_info("AuthService::login - invalid credentials").await;
        Ok(ServiceResponse {
            status: ResponseStatus::Error,
            message: "Invalid username or password".to_string(),
            data: None,
        })
    }

    /// Logout a user by invalidating their token
    async fn logout(&self, _context: &RequestContext, token: &str) -> Result<ServiceResponse> {
        // Just simulate token invalidation - in a real service,
        // we would delete the token from a database
        if !token.is_empty() {
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: "Logout successful".to_string(),
                data: None,
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Invalid token".to_string(),
                data: None,
            })
        }
    }

    async fn validate_token(&self, token: &str) -> Result<ServiceResponse> {
        // Look up token
        let tokens = self.tokens.read().unwrap();
        if let Some(username) = tokens.get(token) {
            // Token is valid
            return Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: "Token is valid".to_string(),
                data: Some(ValueType::Json(json!({
                    "username": username
                }))),
            });
        }
        
        // Token not found
        Ok(ServiceResponse {
            status: ResponseStatus::Error,
            message: "Invalid or expired token".to_string(),
            data: None,
        })
    }
}

#[async_trait]
impl AbstractService for AuthService {
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
        "Authentication service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn operations(&self) -> Vec<String> {
        vec!["login".to_string(), "validateToken".to_string(), "logout".to_string()]
    }

    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        log_info(&format!("[{}] Initializing auth service", self.name)).await;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        log_info(&format!("[{}] Starting auth service", self.name)).await;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        log_info(&format!("[{}] Stopping auth service", self.name)).await;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        log_info(&format!(
            "[{}] Processing request: {}",
            self.name, request.operation
        )).await;

        log_info(&format!("AuthService::handle_request - request: {:?}", request)).await;

        // Check if operation is empty and extract it from path if needed
        let operation = if request.operation.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.operation.clone()
        };

        log_info(&format!("AuthService::handle_request - operation: {}", operation)).await;

        match operation.as_str() {
            "login" => {
                log_info("AuthService::handle_request - login operation").await;
                if let Some(params) = &request.params {
                    log_info(&format!("AuthService::handle_request - params: {:?}", params)).await;
                    
                    // Try to extract username and password from Map type
                    if let ValueType::Map(map) = params {
                        if let (Some(ValueType::String(username)), Some(ValueType::String(password))) = 
                            (map.get("username"), map.get("password")) {
                            log_info(&format!("AuthService::handle_request - username: {}, password: {}", username, password)).await;
                            return self.login(username, password).await;
                        }
                    }
                }
                // Return a proper error response instead of an Err
                log_info("AuthService::handle_request - invalid login parameters").await;
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid username or password".to_string(),
                    data: None,
                })
            },
            "validateToken" => {
                if let Some(params) = &request.params {
                    if let ValueType::Json(json_value) = params {
                        let token = json_value.get("token")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Missing token"))?;

                        return self.validate_token(token).await;
                    }
                }
                // Return a proper error response instead of an Err
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for validateToken".to_string(),
                    data: None,
                })
            }
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown operation: {}", operation),
                data: None,
            }),
        }
    }
}

/// Test the Node API with a simple service
#[tokio::test]
async fn test_node_api_with_simple_service() -> Result<()> {
    // Initialize test logging
    init_logging();
    
    // Set a timeout for the test
    let timeout_duration = std::time::Duration::from_secs(10);

    let _result = timeout(timeout_duration, async {
        // Create a test node
        let (mut node, _temp_dir, _node_config) = create_test_node().await?;

        // Create and add the simple test service
        let test_service = SimpleTestService::new("testService");
        node.add_service(test_service).await?;

        // Test basic service request
        let echo_params = vmap! {
            "message" => "Hello from test!",
            "value" => 42
        };

        node.start().await?;  

        let echo_result = node.request("testService/echo", echo_params).await?;

        // Print the response for debugging
        log_info(&format!("Echo result: {:?}", echo_result)).await;
        if let Some(data) = &echo_result.data {
            log_info(&format!("Response data: {:?}", data)).await;
            
            // Use match instead of .get()
            match data {
                ValueType::Map(map) => {
                    if let Some(received_params) = map.get("received_params") {
                        log_info(&format!("Received params: {:?}", received_params)).await;
                        
                        if let ValueType::Map(params_map) = received_params {
                            log_info(&format!("Params map: {:?}", params_map)).await;
                            for (k, v) in params_map {
                                log_info(&format!("Key: {}, Value: {:?}", k, v)).await;
                            }
                        }
                    }
                },
                _ => {}
            }
        }

        // Verify request was successful
        assert_eq!(
            echo_result.status,
            ResponseStatus::Success,
            "Echo request should succeed"
        );

        // Verify response has data
        assert!(echo_result.data.is_some(), "Response should have data");

        // Verify message in response
        if let Some(data) = echo_result.data {
            // Use match instead of .get()
            match data {
                ValueType::Map(map) => {
                    if let Some(message) = map.get("message") {
                        if let ValueType::String(msg_str) = message {
                            assert!(
                                msg_str.contains("echo"),
                                "Message should contain operation name"
                            );
                        }
                    }
                    
                    if let Some(received_params) = map.get("received_params") {
                        log_info(&format!("Received params: {:?}", received_params)).await;
                    }
                },
                _ => {}
            }
        }

        Ok::<(), anyhow::Error>(())
    })
    .await??;

    Ok(())
}

/// Test the service to service communication
#[tokio::test]
async fn test_service_to_service_communication() -> Result<()> {
    // Initialize test logging
    init_logging();
    
    // Set a timeout for the test
    let timeout_duration = std::time::Duration::from_secs(10);

    let _result = timeout(timeout_duration, async {
        // Create a test node
        let (mut node, _temp_dir, _node_config) = create_test_node().await?;

        // Create the MockDocumentStore (which will simulate a user store)
        let document_store = MockDocumentStore::new("documents");
        node.add_service(document_store).await?;

        // Create the AuthService
        let auth_service = AuthService::new("auth");
        node.add_service(auth_service).await?;

        // Start the node
        node.start_services().await?;

        // Login the user - the AuthService has a test user already added
        let login_params = vmap! {
            "username" => "testuser",
            "password" => "password123"
        };

        log_info(&format!("Sending login request with params: {:?}", login_params)).await;
        let login_result = node.request("auth/login", login_params).await?;
        log_info(&format!("Login result: {:?}", login_result)).await;

        // Verify login was successful
        assert_eq!(
            login_result.status,
            ResponseStatus::Success,
            "Login should succeed"
        );

        // Extract the session token
        let session_token = if let Some(ValueType::Json(token_data)) = &login_result.data {
            token_data.get("token").and_then(|t| t.as_str()).unwrap_or("").to_string()
        } else {
            "".to_string()
        };

        assert!(!session_token.is_empty(), "Session token should not be empty");

        // Now test document store operations using the token
        let doc_params = vmap! {
            "id" => "doc123",
            "collection" => "users",
            "token" => session_token
        };

        let doc_result = node.request("documents/read", doc_params).await?;
        assert_eq!(doc_result.status, ResponseStatus::Success);

        Ok::<(), anyhow::Error>(())
    })
    .await??;

    Ok(())
}

/// Test failed authentication
#[tokio::test]
async fn test_failed_authentication() -> Result<()> {
    // Initialize test logging
    init_logging();
    
    // Set a timeout for the test
    let timeout_duration = std::time::Duration::from_secs(10);

    let _result = timeout(timeout_duration, async {
        // Create a test node
        let (mut node, _temp_dir, _node_config) = create_test_node().await?;

        // Create the MockDocumentStore (which will simulate a user store)
        let document_store = MockDocumentStore::new("documentStore");
        node.add_service(document_store).await?;

        // Create the AuthService
        let auth_service = AuthService::new("auth");
        node.add_service(auth_service).await?;

        // Try to login with invalid credentials
        let login_params = vmap! {
            "username" => "testuser",
            "password" => "wrong_password"
        };

        let login_result = node.request("auth/login", login_params).await?;

        // Verify login failed
        assert_eq!(
            login_result.status,
            ResponseStatus::Error,
            "Login should fail with wrong password"
        );

        // Try to login with non-existent user
        let login_params = vmap! {
            "username" => "nonexistent",
            "password" => "password123"
        };

        let login_result = node.request("auth/login", login_params).await?;

        // Verify login failed
        assert_eq!(
            login_result.status,
            ResponseStatus::Error,
            "Login should fail with non-existent user"
        );

        Ok::<(), anyhow::Error>(())
    })
    .await??;

    Ok(())
}

#[tokio::test]
async fn test_node_api() -> Result<()> {
    // Initialize test logging
    init_logging();
    
    // ... existing code ...
    
    Ok(())
}

#[tokio::test]
async fn test_auth_service() -> Result<()> {
    // Initialize test logging
    init_logging();
    
    let (mut node, _temp_dir, _node_config) = create_test_node().await?;

    // Create an auth service
    let auth_service = AuthService::new("auth");
    
    // Create a document store service
    let doc_store = MockDocumentStore::new("documents");
    
    // Add services to the node
    node.add_service(auth_service).await?;
    node.add_service(doc_store).await?;
    
    // Start the node
    node.start_services().await?;
    
    // Test login with invalid credentials
    let params = json!({
        "username": "testuser",
        "password": "wrongpassword"
    });
    
    let response = node.request("auth/login", params).await?;
    assert_eq!(response.status, ResponseStatus::Error);
    assert_eq!(response.message, "Invalid username or password");
    
    // Test document store read operation
    let params = json!({
        "id": "doc123",
        "collection": "users"
    });
    
    let response = node.request("documents/read", params).await?;
    assert_eq!(response.status, ResponseStatus::Success);
    
    // Stop the node
    node.stop().await?;
    
    Ok(())
}
