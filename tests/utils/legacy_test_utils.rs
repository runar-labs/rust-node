// Legacy test utilities from the old test_utils/mod.rs
// This provides backward compatibility while we migrate to the new structure

use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState, ActionMetadata, EventMetadata};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType, vmap};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid;

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
        // Simple echo behavior - returns the action and data
        let action = if request.action.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.action.clone()
        };

        // Convert data to a map with plain values (not JSON)
        let mut plain_params = std::collections::HashMap::new();
        
        // Handle data which is an Option<ValueType>
        if let Some(data_value) = &request.data {
            match data_value {
                ValueType::Map(data_map) => {
                    for (key, value) in data_map {
                        // Convert JSON values to plain values
                        let plain_value = match value {
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
                            },
                            _ => value.clone()
                        };
                        plain_params.insert(key.clone(), plain_value);
                    }
                },
                ValueType::Json(json) => {
                    if let Some(obj) = json.as_object() {
                        for (key, value) in obj {
                            // Convert JSON values to plain values
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
                    // If it's a single value (not a map), store it under "value" key
                    plain_params.insert("value".to_string(), data_value.clone());
                }
            }
        }

        // Return a response with the operation name and the parameters
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Echo service processed operation '{}'", action),
            data: Some(ValueType::Map(std::collections::HashMap::from([
                ("action".to_string(), ValueType::String(action)),
                ("data".to_string(), ValueType::Map(plain_params)),
                ("service".to_string(), ValueType::String(self.name.clone())),
            ]))),
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
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "test".to_string() },
            ActionMetadata { name: "dummy".to_string() },
        ]
    }

    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "test_event".to_string() },
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
        // Process any operation
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
        // This is a mock - we just validate the request format and return a success
        if let Some(data) = &request.data {
            // In a real implementation, we would store the document and return its ID
            let document_id = uuid::Uuid::new_v4().to_string();
            
            // Get the document content from params
            let document_content = match data {
                ValueType::Map(map) => {
                    if let Some(content) = map.get("content") {
                        format!("{:?}", content)
                    } else {
                        format!("{:?}", map)
                    }
                },
                ValueType::Json(json) => {
                    json.to_string()
                },
                _ => format!("{:?}", data)
            };
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document created with ID: {}", document_id),
                data: Some(ValueType::Map(std::collections::HashMap::from([
                    ("id".to_string(), ValueType::String(document_id)),
                    ("content".to_string(), ValueType::String(document_content)),
                ]))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Missing document data".to_string(),
                data: None,
            })
        }
    }

    /// Handle read operation
    async fn read(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // This is a mock - we just validate the request format and return a dummy document
        if let Some(data) = &request.data {
            // Extract document ID from params
            let document_id = match data {
                ValueType::Map(map) => {
                    if let Some(ValueType::String(id)) = map.get("id") {
                        id.clone()
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID".to_string(),
                            data: None,
                        });
                    }
                },
                ValueType::Json(json) => {
                    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                        id.to_string()
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID in JSON".to_string(),
                            data: None,
                        });
                    }
                },
                ValueType::String(id) => id.clone(),
                _ => {
                    return Ok(ServiceResponse {
                        status: ResponseStatus::Error,
                        message: "Invalid document ID format".to_string(),
                        data: None,
                    });
                }
            };
            
            // In a real implementation, we would fetch the document by ID
            // Mock some document content based on the ID
            let document_content = format!("This is document {} content", document_id);
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document retrieved: {}", document_id),
                data: Some(ValueType::Map(std::collections::HashMap::from([
                    ("id".to_string(), ValueType::String(document_id)),
                    ("content".to_string(), ValueType::String(document_content)),
                    ("timestamp".to_string(), ValueType::String(std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                        .to_string())),
                ]))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Missing document ID".to_string(),
                data: None,
            })
        }
    }

    /// Handle update operation
    async fn update(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // This is a mock - we just validate the request format and return a success
        if let Some(data) = &request.data {
            // Extract document ID and content from params
            let (document_id, document_content) = match data {
                ValueType::Map(map) => {
                    if let Some(ValueType::String(id)) = map.get("id") {
                        let content = if let Some(content) = map.get("content") {
                            format!("{:?}", content)
                        } else {
                            format!("{:?}", map)
                        };
                        (id.clone(), content)
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID or content".to_string(),
                            data: None,
                        });
                    }
                },
                ValueType::Json(json) => {
                    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                        let content = if let Some(content) = json.get("content") {
                            content.to_string()
                        } else {
                            json.to_string()
                        };
                        (id.to_string(), content)
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID or content in JSON".to_string(),
                            data: None,
                        });
                    }
                },
                _ => {
                    return Ok(ServiceResponse {
                        status: ResponseStatus::Error,
                        message: "Invalid document update format".to_string(),
                        data: None,
                    });
                }
            };
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document updated: {}", document_id),
                data: Some(ValueType::Map(std::collections::HashMap::from([
                    ("id".to_string(), ValueType::String(document_id)),
                    ("content".to_string(), ValueType::String(document_content)),
                    ("updated".to_string(), ValueType::Bool(true)),
                ]))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Missing document update data".to_string(),
                data: None,
            })
        }
    }

    /// Handle delete operation
    async fn delete(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // This is a mock - we just validate the request format and return a success
        if let Some(data) = &request.data {
            // Extract document ID from params
            let document_id = match data {
                ValueType::Map(map) => {
                    if let Some(ValueType::String(id)) = map.get("id") {
                        id.clone()
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID".to_string(),
                            data: None,
                        });
                    }
                },
                ValueType::Json(json) => {
                    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                        id.to_string()
                    } else {
                        return Ok(ServiceResponse {
                            status: ResponseStatus::Error,
                            message: "Invalid document ID in JSON".to_string(),
                            data: None,
                        });
                    }
                },
                ValueType::String(id) => id.clone(),
                _ => {
                    return Ok(ServiceResponse {
                        status: ResponseStatus::Error,
                        message: "Invalid document ID format".to_string(),
                        data: None,
                    });
                }
            };
            
            Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document deleted: {}", document_id),
                data: Some(ValueType::Map(std::collections::HashMap::from([
                    ("id".to_string(), ValueType::String(document_id)),
                    ("deleted".to_string(), ValueType::Bool(true)),
                ]))),
            })
        } else {
            Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: "Missing document ID".to_string(),
                data: None,
            })
        }
    }

    /// Handle query operation
    async fn query(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // In a real implementation, we would query the documents based on filter
        // This is a mock that returns a fixed set of documents
        
        // If there's a filter, we pretend to filter documents
        let filter_desc = if let Some(data) = &request.data {
            match data {
                ValueType::Map(map) => {
                    format!("Filter applied: {:?}", map)
                },
                ValueType::Json(json) => {
                    format!("Filter applied: {}", json)
                },
                _ => {
                    "No specific filter applied".to_string()
                }
            }
        } else {
            "No filter provided".to_string()
        };
        
        // Generate a few mock documents
        let doc1_id = uuid::Uuid::new_v4().to_string();
        let doc2_id = uuid::Uuid::new_v4().to_string();
        let doc3_id = uuid::Uuid::new_v4().to_string();
        
        // Create a list of mock documents
        let documents = ValueType::Map(std::collections::HashMap::from([
            (doc1_id.clone(), ValueType::Map(std::collections::HashMap::from([
                ("id".to_string(), ValueType::String(doc1_id.clone())),
                ("title".to_string(), ValueType::String("First Document".to_string())),
                ("content".to_string(), ValueType::String("Content for document 1".to_string())),
            ]))),
            (doc2_id.clone(), ValueType::Map(std::collections::HashMap::from([
                ("id".to_string(), ValueType::String(doc2_id.clone())),
                ("title".to_string(), ValueType::String("Second Document".to_string())),
                ("content".to_string(), ValueType::String("Content for document 2".to_string())),
            ]))),
            (doc3_id.clone(), ValueType::Map(std::collections::HashMap::from([
                ("id".to_string(), ValueType::String(doc3_id.clone())),
                ("title".to_string(), ValueType::String("Third Document".to_string())),
                ("content".to_string(), ValueType::String("Content for document 3".to_string())),
            ]))),
        ]));
        
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Query executed. {}", filter_desc),
            data: Some(ValueType::Map(std::collections::HashMap::from([
                ("documents".to_string(), documents),
                ("count".to_string(), ValueType::Number(3.0)),
                ("query_time".to_string(), ValueType::String("0.001s".to_string())),
            ]))),
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
    
    fn actions(&self) -> Vec<ActionMetadata> {
        vec![
            ActionMetadata { name: "test".to_string() },
            ActionMetadata { name: "dummy".to_string() },
        ]
    }

    fn events(&self) -> Vec<EventMetadata> {
        vec![
            EventMetadata { name: "test_event".to_string() },
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
        // Check if action is empty and extract it from path if needed
        let action = if request.action.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.action.clone()
        };

        match action.as_str() {
            "create" => self.create(request).await,
            "read" => self.read(request).await,
            "update" => self.update(request).await,
            "delete" => self.delete(request).await,
            "query" => self.query(request).await,
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown action: {}", action),
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