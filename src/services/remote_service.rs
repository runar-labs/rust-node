// Remote Service Implementation
//
// INTENTION: Implement a proxy service that represents a service running on a remote node.
// This service forwards requests to the remote node and returns responses, making
// remote services appear as local services to the node.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::network::transport::{NetworkMessage, NetworkTransport, NodeIdentifier};
use crate::routing::TopicPath;
use crate::services::abstract_service::{AbstractService, ActionMetadata, ServiceState};
use crate::services::{ActionHandler, EventContext, LifecycleContext, RequestContext, ServiceResponse};
use runar_common::types::ValueType;
use runar_common::logging::Logger;

/// Represents a service running on a remote node
#[derive(Clone)]
pub struct RemoteService {
    /// Service metadata
    name: String,
    service_path: TopicPath,
    service_path_str: String, // Cache the string representation
    version: String,
    description: String,
    
    /// Remote peer information
    peer_id: NodeIdentifier,
    network_transport: Arc<dyn NetworkTransport>,
    
    /// Service capabilities
    actions: Arc<RwLock<HashMap<String, ActionMetadata>>>,
    
    /// Logger instance
    logger: Logger,
    
    /// Local node identifier (for sending messages)
    local_node_id: NodeIdentifier,
    
    /// Pending requests awaiting responses
    pending_requests: Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<ServiceResponse>>>>>,
    
    /// Request timeout in milliseconds
    request_timeout_ms: u64,
}

impl RemoteService {
    /// Create a new RemoteService instance
    pub fn new(
        name: String,
        service_path: TopicPath,
        version: String,
        description: String,
        peer_id: NodeIdentifier,
        network_transport: Arc<dyn NetworkTransport>,
        local_node_id: NodeIdentifier,
        logger: Logger,
    ) -> Self {
        // Cache the service path string to avoid multiple calls
        let service_path_str = service_path.service_path().to_string();
        
        Self {
            name,
            service_path,
            service_path_str,
            version,
            description,
            peer_id,
            network_transport,
            actions: Arc::new(RwLock::new(HashMap::new())),
            logger,
            local_node_id,
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            request_timeout_ms: 30000, // Default 30 seconds timeout
        }
    }
    
    /// Get the remote peer identifier for this service
    pub fn peer_id(&self) -> &NodeIdentifier {
        &self.peer_id
    }
    
    /// Add an action to this remote service
    pub async fn add_action(&self, action_name: String, metadata: ActionMetadata) -> Result<()> {
        self.actions.write().await.insert(action_name, metadata);
        Ok(())
    }
    
    /// Create a handler for a remote action
    pub fn create_action_handler(&self, action_name: String) -> ActionHandler {
        let service = self.clone();
        
        // Create a handler that forwards requests to the remote service
        Arc::new(move |params, context| {
            let service_clone = service.clone();
            let action = action_name.clone();
            
            // Create a new TopicPath for this action using the helper method
            let action_topic_path = match service_clone.service_path.new_action_topic(&action) {
                Ok(path) => path,
                Err(e) => {
                    return Box::pin(async move {
                        Ok(ServiceResponse::error(400, format!("Invalid action path: {}", e)))
                    });
                }
            };
            
            Box::pin(async move {
                service_clone.handle_remote_action(action_topic_path, params, context).await
            })
        })
    }
    
    /// Register a response handler for incoming network messages
    ///
    /// INTENTION: Set up this service to receive responses for its requests.
    /// This should be called once when the service is created.
    pub async fn register_response_handler(&self, node: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>) -> Result<()> {
        let service_clone = self.clone();
        
        // Create a response handler that will resolve pending requests
        let handler = Box::new(move |message: NetworkMessage| {
            let service = service_clone.clone();
            
            // Only handle response messages
            if message.message_type == "Response" {
                if let Some(correlation_id) = message.correlation_id.clone() {
                    let message_clone = message.clone();
                    // Spawn a task to handle the response
                    tokio::spawn(async move {
                        // Handle the response
                        if let Err(e) = service.handle_response(&correlation_id, message_clone).await {
                            service.logger.error(format!("Error handling response: {}", e));
                        }
                    });
                }
            }
            
            Ok(())
        });
        
        // Register the handler with the transport
        if let Some(transport) = &*node.read().await {
            transport.register_handler(handler)?;
        } else {
            return Err(anyhow!("Network transport not available"));
        }
        
        Ok(())
    }
    
    /// Handle a response for a pending request
    ///
    /// INTENTION: Process an incoming response and resolve the corresponding pending request.
    async fn handle_response(&self, correlation_id: &str, message: NetworkMessage) -> Result<()> {
        // Extract the sender for this correlation ID
        let sender = {
            let mut pending = self.pending_requests.write().await;
            pending.remove(correlation_id)
        };
        
        if let Some(sender) = sender {
            // Parse the response message
            if message.message_type == "Response" {
                if let ValueType::Map(map) = &message.payload {
                    if let Some(error) = map.get("error") {
                        if let Some(true) = error.as_bool() {
                            // This is an error response
                            let error_message = map.get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("Unknown error").to_string();
                            
                            // Send the error response
                            let _ = sender.send(Ok(ServiceResponse::error(400, error_message)));
                        } else {
                            // This is a successful response
                            let data = map.get("data")
                                .unwrap_or(&ValueType::Null);
                            
                            // Send the successful response
                            let _ = sender.send(Ok(ServiceResponse::ok(data.clone())));
                        }
                    } else {
                        // Legacy format or direct data
                        let _ = sender.send(Ok(ServiceResponse::ok(message.payload)));
                    }
                } else {
                    // Not a map, just send as is
                    let _ = sender.send(Ok(ServiceResponse::ok(message.payload)));
                }
            } else {
                // Unexpected message type
                let _ = sender.send(Ok(ServiceResponse::error(
                    400, 
                    format!("Unexpected message type: {}", message.message_type)
                )));
            }
        } else {
            self.logger.warn(format!("Received response for unknown correlation ID: {}", correlation_id));
        }
        
        Ok(())
    }
    
    /// Handle a request for a remote action
    async fn handle_remote_action(
        &self, 
        action_topic_path: TopicPath, 
        params: Option<ValueType>, 
        _context: RequestContext
    ) -> Result<ServiceResponse> {
        // Generate a unique correlation ID for this request
        let correlation_id = Uuid::new_v4().to_string();
        
        // Get the full action path
        let action_path = action_topic_path.action_path();
        
        // Create a channel for the response
        let (sender, receiver) = tokio::sync::oneshot::channel();
        
        // Store the sender in the pending requests map
        {
            let mut pending = self.pending_requests.write().await;
            pending.insert(correlation_id.clone(), sender);
        }
        
        // Create a network message with the request
        let message = NetworkMessage {
            source: self.local_node_id.clone(),
            destination: Some(self.peer_id.clone()),
            message_type: "Request".to_string(),
            correlation_id: Some(correlation_id.clone()),
            topic: action_path.clone(),
            params: params.unwrap_or(ValueType::Null),
            payload: ValueType::Null, // No additional payload needed
        };
        
        self.logger.debug(format!("Sending remote request to {}: {}", self.peer_id, action_path));
        
        // Send the message to the remote node
        self.network_transport.send(message).await?;
        
        // Wait for the response with a timeout
        match tokio::time::timeout(
            std::time::Duration::from_millis(self.request_timeout_ms),
            receiver
        ).await {
            Ok(result) => {
                match result {
                    Ok(response) => response,
                    Err(_) => {
                        // The sender was dropped without sending a value
                        self.logger.error(format!("Request channel closed without response: {}", correlation_id));
                        Ok(ServiceResponse::error(500, "Request channel closed without response"))
                    }
                }
            },
            Err(_) => {
                // Timeout occurred
                // Remove the pending request since we're no longer waiting for it
                {
                    let mut pending = self.pending_requests.write().await;
                    pending.remove(&correlation_id);
                }
                
                self.logger.error(format!("Request timed out: {}", correlation_id));
                Ok(ServiceResponse::error(408, "Request timed out"))
            }
        }
    }
}

#[async_trait]
impl AbstractService for RemoteService {
    fn name(&self) -> &str {
        &self.name
    }
    
    fn path(&self) -> &str {
        &self.service_path_str
    }
    
    fn version(&self) -> &str {
        &self.version
    }
    
    fn description(&self) -> &str {
        &self.description
    }
    
    async fn init(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need initialization since they're just proxies
        self.logger.info(format!("Initialized remote service proxy for {}", self.service_path_str));
        Ok(())
    }
    
    async fn start(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need to be started
        self.logger.info(format!("Started remote service proxy for {}", self.service_path_str));
        Ok(())
    }
    
    async fn stop(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need to be stopped
        self.logger.info(format!("Stopped remote service proxy for {}", self.service_path_str));
        Ok(())
    }
} 