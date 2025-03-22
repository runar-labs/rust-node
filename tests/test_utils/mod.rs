use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType, vmap};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Utility function to create ValueType::Number from integers (replacement for ValueType::Int)
pub fn value_int(value: i64) -> ValueType {
    ValueType::Number(value as f64)
}

/// A minimal test service that just responds with a simple message
/// This is useful for basic service registration/discovery tests
pub struct SimpleTestService {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
}

impl SimpleTestService {
    /// Create a new SimpleTestService
    pub fn new(name: &str) -> Self {
        SimpleTestService {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
        }
    }

    /// Process any operation generically - this service just echoes back the operation and parameters
    async fn process_any_operation(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Simple echo behavior - returns the operation and parameters
        let operation = if request.operation.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.operation.clone()
        };

        // Convert params to a map with plain values (not JSON)
        let mut plain_params = std::collections::HashMap::new();
        
        // Handle params which is an Option<ValueType>
        if let Some(params_value) = request.params {
            match params_value {
                ValueType::Map(params_map) => {
                    for (key, value) in params_map {
                        // Convert JSON values to plain values
                        let plain_value = match &value {
                            ValueType::Json(json) => {
                                if let Some(s) = json.as_str() {
                                    ValueType::String(s.to_string())
                                } else if let Some(n) = json.as_f64() {
                                    ValueType::Number(n)
                                } else if let Some(b) = json.as_bool() {
                                    ValueType::Bool(b)
                                } else {
                                    value.clone()
                                }
                            }
                            _ => value.clone(),
                        };
                        plain_params.insert(key.clone(), plain_value);
                    }
                },
                ValueType::Json(json_obj) => {
                    if let Some(obj) = json_obj.as_object() {
                        for (key, value) in obj {
                            let plain_value = if let Some(s) = value.as_str() {
                                ValueType::String(s.to_string())
                            } else if let Some(n) = value.as_f64() {
                                ValueType::Number(n)
                            } else if let Some(b) = value.as_bool() {
                                ValueType::Bool(b)
                            } else {
                                ValueType::Json(value.clone())
                            };
                            plain_params.insert(key.clone(), plain_value);
                        }
                    }
                },
                _ => {
                    // Handle other ValueType variants if needed
                }
            }
        }

        let response_data = vmap! {
            "message" => format!("SimpleTestService received operation: {}", operation),
            "received_params" => ValueType::Map(plain_params),
            "timestamp" => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs().to_string()
        };

        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Request processed successfully for operation: {}", operation),
            data: Some(response_data),
        })
    }
}

#[async_trait::async_trait]
impl AbstractService for SimpleTestService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        *self.state.lock().unwrap()
    }

    fn description(&self) -> &str {
        "Simple Test Service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn operations(&self) -> Vec<String> {
        vec!["echo".to_string(), "ping".to_string()]
    }

    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // This service just echoes back any operation, so we use a generic handler
        self.process_any_operation(request).await
    }
}

/// A mock database service for testing
/// This simulates a document store with key operations (create, read, update, delete)
pub struct MockDocumentStore {
    name: String,
    path: String,
    state: Mutex<ServiceState>,
}

impl MockDocumentStore {
    /// Create a new MockDocumentStore
    pub fn new(name: &str) -> Self {
        MockDocumentStore {
            name: name.to_string(),
            path: name.to_string(),
            state: Mutex::new(ServiceState::Created),
        }
    }

    /// Handle create operation
    async fn create(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let collection = request
            .get_param("collection")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let document = request.get_param("document").unwrap_or(ValueType::Null);

        // Generate a UUID for the document
        let id = uuid::Uuid::new_v4().to_string();

        // Add ID to document
        let mut document_map = document.clone();
        if let ValueType::Map(ref mut map) = document_map {
            map.insert("_id".to_string(), ValueType::String(id.clone()));
        }

        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!(
                "Document created in collection {} with ID: {}",
                collection, id
            ),
            data: Some(document_map),
        })
    }

    /// Handle read operation
    async fn read(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let collection = request
            .get_param("collection")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let filter = request.get_param("filter").unwrap_or(ValueType::Null);

        // Check if we're looking for a user with a specific name
        let mut has_match = false;
        if let ValueType::Map(filter_map) = &filter {
            if let Some(name_value) = filter_map.get("name") {
                let name_str = match name_value {
                    ValueType::String(s) => Some(s.as_str()),
                    ValueType::Json(json) => {
                        if let Some(s) = json.as_str() {
                            Some(s)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if let Some(name) = name_str {
                    if name == "testuser" {
                        has_match = true;
                    }
                }
            }
        }

        if has_match {
            // Return a mock user
            let user = vmap! {
                "_id" => uuid::Uuid::new_v4().to_string(),
                "name" => "testuser",
                "email" => "test@example.com",
                "age" => 30,
                "password" => "password123"
            };

            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Found 1 document in collection {}", collection),
                data: Some(ValueType::Array(vec![user])),
            })
        } else {
            // Return empty array
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Found 0 documents in collection {}", collection),
                data: Some(ValueType::Array(vec![])),
            })
        }
    }

    /// Handle update operation
    async fn update(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let collection = request
            .get_param("collection")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let filter = request.get_param("filter").unwrap_or(ValueType::Null);
        let document = request.get_param("document").unwrap_or(ValueType::Null);

        // Extract ID from filter or generate a new one
        let id = if let ValueType::Map(filter_map) = &filter {
            if let Some(id_value) = filter_map.get("_id") {
                if let Some(id_str) = id_value.as_str() {
                    id_str.to_string()
                } else {
                    uuid::Uuid::new_v4().to_string()
                }
            } else {
                uuid::Uuid::new_v4().to_string()
            }
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!(
                "Document updated in collection {} with ID: {}",
                collection, id
            ),
            data: Some(document),
        })
    }

    /// Handle delete operation
    async fn delete(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let collection = request
            .get_param("collection")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let filter = request.get_param("filter").unwrap_or(ValueType::Null);

        // Extract ID from filter or generate a new one
        let id = if let ValueType::Map(filter_map) = &filter {
            if let Some(id_value) = filter_map.get("_id") {
                if let Some(id_str) = id_value.as_str() {
                    id_str.to_string()
                } else {
                    uuid::Uuid::new_v4().to_string()
                }
            } else {
                uuid::Uuid::new_v4().to_string()
            }
        } else {
            uuid::Uuid::new_v4().to_string()
        };

        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!(
                "Document deleted from collection {} with ID: {}",
                collection, id
            ),
            data: None,
        })
    }

    /// Handle query operation
    async fn query(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        let collection = request
            .get_param("collection")
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        let query = request
            .get_param("query")
            .unwrap_or(ValueType::Null);

        // Simulate a query operation
        let results = vec![
            vmap! {
                "id" => uuid::Uuid::new_v4().to_string(),
                "name" => "Sample Document 1",
                "created_at" => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs().to_string()
            },
            vmap! {
                "id" => uuid::Uuid::new_v4().to_string(),
                "name" => "Sample Document 2",
                "created_at" => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs().to_string()
            }
        ];

        let response_data = vmap! {
            "collection" => collection,
            "query" => query,
            "results" => ValueType::Array(results),
            "count" => 2
        };

        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: "Query executed successfully".to_string(),
            data: Some(response_data),
        })
    }
}

#[async_trait::async_trait]
impl AbstractService for MockDocumentStore {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        *self.state.lock().unwrap()
    }

    fn description(&self) -> &str {
        "Mock Document Store for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }
    
    fn operations(&self) -> Vec<String> {
        vec![
            "create".to_string(),
            "read".to_string(),
            "update".to_string(),
            "delete".to_string(),
            "query".to_string(),
        ]
    }

    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract operation from path if operation is empty
        let operation = if request.operation.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.operation.clone()
        };

        // Handle document store operations
        match operation.as_str() {
            "create" => self.create(request).await,
            "read" => self.read(request).await,
            "update" => self.update(request).await,
            "delete" => self.delete(request).await,
            "query" => self.query(request).await,
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown operation: {}", operation),
                data: None,
            }),
        }
    }
}

/// Test helper function to create a temporary node for testing
pub async fn create_test_node() -> Result<(
    runar_node::node::Node,
    tempfile::TempDir,
    runar_node::node::NodeConfig,
)> {
    // Create a temporary directory for node storage
    let temp_dir = tempfile::tempdir()?;
    let node_path = temp_dir.path().to_str().unwrap();
    let network_id = "test-network";

    // Create a NodeConfig for the node
    let node_config =
        runar_node::node::NodeConfig::new(network_id, node_path, &format!("{}/db", node_path));

    // Create and initialize the node
    let node_config_clone = node_config.clone();
    let mut node = runar_node::node::Node::new(node_config_clone).await?;
    node.init().await?;

    Ok((node, temp_dir, node_config))
}
