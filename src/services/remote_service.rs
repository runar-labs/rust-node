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
        
        // In a real implementation, we would now wait for a response with the matching correlation_id
        // For now, we'll return a placeholder response
        // TODO: Implement waiting for response with timeout
        
        // Extract action name from the last segment of the action path
        let action_name = action_path.split('/').last().unwrap_or("unknown");
        
        // Mock response for now
        Ok(ServiceResponse::ok(ValueType::String(format!("Remote action '{}' called", action_name))))
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