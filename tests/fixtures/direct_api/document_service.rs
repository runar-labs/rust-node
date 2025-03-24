use anyhow::{anyhow, Result};
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState};
use runar_node::services::ResponseStatus;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// A document storage service for testing
/// This implementation uses the Node API directly without macros
pub struct DocumentService {
    name: String,
    path: String,
    documents: Arc<RwLock<HashMap<String, ValueType>>>,
}

impl Clone for DocumentService {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            path: self.path.clone(),
            documents: self.documents.clone(),
        }
    }
}

impl DocumentService {
    /// Create a new DocumentService
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            path: format!("/documents/{}", name),
            documents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a document
    pub async fn create(&self, id: &str, data: ValueType) -> Result<ServiceResponse> {
        let mut documents = self.documents.write().unwrap();
        
        // Check if document already exists
        if documents.contains_key(id) {
            return Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Document with ID {} already exists", id),
                data: None,
            });
        }
        
        // Insert the document
        documents.insert(id.to_string(), data.clone());
        
        // Return success response
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Document created with ID: {}", id),
            data: Some(ValueType::Json(json!({
                "id": id,
                "data": data
            }))),
        })
    }

    /// Read a document
    pub async fn read(&self, id: &str) -> Result<ServiceResponse> {
        let documents = self.documents.read().unwrap();
        
        // Look up the document
        if let Some(data) = documents.get(id) {
            // Return the document
            return Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document found with ID: {}", id),
                data: Some(ValueType::Json(json!({
                    "id": id,
                    "data": data
                }))),
            });
        }
        
        // Document not found
        Ok(ServiceResponse {
            status: ResponseStatus::Error,
            message: format!("Document with ID {} not found", id),
            data: None,
        })
    }

    /// Update a document
    pub async fn update(&self, id: &str, data: ValueType) -> Result<ServiceResponse> {
        let mut documents = self.documents.write().unwrap();
        
        // Check if document exists
        if !documents.contains_key(id) {
            return Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Document with ID {} not found", id),
                data: None,
            });
        }
        
        // Update the document
        documents.insert(id.to_string(), data.clone());
        
        // Return success response
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Document updated with ID: {}", id),
            data: Some(ValueType::Json(json!({
                "id": id,
                "data": data
            }))),
        })
    }

    /// Delete a document
    pub async fn delete(&self, id: &str) -> Result<ServiceResponse> {
        let mut documents = self.documents.write().unwrap();
        
        // Check if document exists and remove it
        if documents.remove(id).is_some() {
            return Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Document deleted with ID: {}", id),
                data: Some(ValueType::Json(json!({
                    "id": id
                }))),
            });
        }
        
        // Document not found
        Ok(ServiceResponse {
            status: ResponseStatus::Error,
            message: format!("Document with ID {} not found", id),
            data: None,
        })
    }

    /// Query documents
    pub async fn query(&self, filter: Option<ValueType>) -> Result<ServiceResponse> {
        let documents = self.documents.read().unwrap();
        
        // If no filter is provided, return all documents
        if filter.is_none() {
            let result: HashMap<String, ValueType> = documents.clone();
            return Ok(ServiceResponse {
                status: ResponseStatus::Success,
                message: format!("Found {} documents", result.len()),
                data: Some(ValueType::Json(json!({
                    "documents": result
                }))),
            });
        }
        
        // Simple filtering based on exact match of fields
        let mut result = HashMap::new();
        
        if let Some(ValueType::Json(filter_json)) = filter {
            for (id, doc) in documents.iter() {
                if let ValueType::Json(doc_json) = doc {
                    // Check if all filter criteria match
                    let mut matches = true;
                    for (key, value) in filter_json.as_object().unwrap() {
                        if let Some(doc_value) = doc_json.get(key) {
                            if doc_value != value {
                                matches = false;
                                break;
                            }
                        } else {
                            matches = false;
                            break;
                        }
                    }
                    
                    if matches {
                        result.insert(id.clone(), doc.clone());
                    }
                }
            }
        }
        
        // Return filtered results
        Ok(ServiceResponse {
            status: ResponseStatus::Success,
            message: format!("Found {} documents matching criteria", result.len()),
            data: Some(ValueType::Json(json!({
                "documents": result
            }))),
        })
    }
}

#[async_trait]
impl AbstractService for DocumentService {
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
        "Document storage service for testing"
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
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Check if operation is empty and extract it from path if needed
        let operation = if request.operation.is_empty() && request.path.contains("/") {
            request.path.split("/").last().unwrap_or("").to_string()
        } else {
            request.operation.clone()
        };

        match operation.as_str() {
            "create" => {
                if let Some(params) = &request.params {
                    if let ValueType::Json(json_value) = params {
                        let id = json_value.get("id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Missing id"))?;
                        let data = json_value.get("data")
                            .ok_or_else(|| anyhow!("Missing data"))?;
                        
                        return self.create(id, ValueType::Json(data.clone())).await;
                    } else if let ValueType::Map(map) = params {
                        if let (Some(ValueType::String(id)), Some(data)) = 
                            (map.get("id"), map.get("data")) {
                            return self.create(id, data.clone()).await;
                        }
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for create".to_string(),
                    data: None,
                })
            },
            "read" => {
                if let Some(params) = &request.params {
                    if let ValueType::Json(json_value) = params {
                        let id = json_value.get("id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Missing id"))?;
                        
                        return self.read(id).await;
                    } else if let ValueType::Map(map) = params {
                        if let Some(ValueType::String(id)) = map.get("id") {
                            return self.read(id).await;
                        }
                    } else if let ValueType::String(id) = params {
                        return self.read(id).await;
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for read".to_string(),
                    data: None,
                })
            },
            "update" => {
                if let Some(params) = &request.params {
                    if let ValueType::Json(json_value) = params {
                        let id = json_value.get("id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Missing id"))?;
                        let data = json_value.get("data")
                            .ok_or_else(|| anyhow!("Missing data"))?;
                        
                        return self.update(id, ValueType::Json(data.clone())).await;
                    } else if let ValueType::Map(map) = params {
                        if let (Some(ValueType::String(id)), Some(data)) = 
                            (map.get("id"), map.get("data")) {
                            return self.update(id, data.clone()).await;
                        }
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for update".to_string(),
                    data: None,
                })
            },
            "delete" => {
                if let Some(params) = &request.params {
                    if let ValueType::Json(json_value) = params {
                        let id = json_value.get("id")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| anyhow!("Missing id"))?;
                        
                        return self.delete(id).await;
                    } else if let ValueType::Map(map) = params {
                        if let Some(ValueType::String(id)) = map.get("id") {
                            return self.delete(id).await;
                        }
                    } else if let ValueType::String(id) = params {
                        return self.delete(id).await;
                    }
                }
                
                Ok(ServiceResponse {
                    status: ResponseStatus::Error,
                    message: "Invalid parameters for delete".to_string(),
                    data: None,
                })
            },
            "query" => {
                let filter = request.params.clone();
                return self.query(filter).await;
            },
            _ => Ok(ServiceResponse {
                status: ResponseStatus::Error,
                message: format!("Unknown operation: {}", operation),
                data: None,
            }),
        }
    }
}
