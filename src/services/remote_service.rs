// Remote Service Implementation
//
// INTENTION: Implement a proxy service that represents a service running on a remote node.
// This service forwards requests to the remote node and returns responses, making
// remote services appear as local services to the node.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use bincode;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::network::transport::{
    NetworkMessage, NetworkMessagePayloadItem, NetworkTransport, PeerId,
};
use crate::routing::TopicPath;
use crate::services::abstract_service::AbstractService;
use crate::services::{ActionHandler, LifecycleContext};
use runar_common::logging::Logger;
use runar_common::types::{ActionMetadata, ArcValueType, SerializerRegistry, ServiceMetadata};

/// Represents a service running on a remote node
#[derive(Clone)]
pub struct RemoteService {
    /// Service metadata
    pub name: String,
    pub service_topic: TopicPath,
    pub version: String,
    pub description: String,
    /// Network ID for this service
    pub network_id: String,

    /// Remote peer information
    peer_id: PeerId,
    /// Network transport wrapped in RwLock
    network_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,

    serializer: Arc<RwLock<SerializerRegistry>>,

    /// Service capabilities
    actions: Arc<RwLock<HashMap<String, ActionMetadata>>>,

    /// Logger instance
    logger: Arc<Logger>,

    /// Local node identifier (for sending messages)
    local_node_id: PeerId,

    /// Pending requests awaiting responses
    pending_requests:
        Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<Option<ArcValueType>>>>>>,

    /// Request timeout in milliseconds
    request_timeout_ms: u64,
}

impl RemoteService {
    /// Create a new RemoteService instance
    pub fn new(
        name: String,
        service_topic: TopicPath,
        version: String,
        description: String,
        peer_id: PeerId,
        network_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,
        serializer: Arc<RwLock<SerializerRegistry>>,
        local_node_id: PeerId,
        pending_requests:
        Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<Option<ArcValueType>>>>>>,
        logger: Arc<Logger>,
        request_timeout_ms: u64,
    ) -> Self {
        Self {
            name,
            service_topic,
            version,
            description,
            peer_id,
            network_transport,
            serializer: serializer,
            actions: Arc::new(RwLock::new(HashMap::new())),
            logger,
            local_node_id,
            pending_requests,
            request_timeout_ms,
            network_id: String::new(),
        }
    }

    /// Create RemoteService instances from service metadata
    ///
    /// INTENTION: Parse service metadata from a discovered node and create
    /// RemoteService instances for each service.
    pub async fn create_from_capabilities(
        peer_id: PeerId,
        capabilities: Vec<ServiceMetadata>,
        network_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,
        serializer: Arc<RwLock<SerializerRegistry>>,
        pending_requests:
        Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<Option<ArcValueType>>>>>>,
        logger: Arc<Logger>,
        local_node_id: PeerId,
        request_timeout_ms: u64,
    ) -> Result<Vec<Arc<RemoteService>>> {
        
        logger.info(format!(
            "Creating RemoteServices from {} service metadata entries",
            capabilities.len()
        ));

        // Make sure we have a valid transport
        let transport_guard = network_transport.read().await;
        if transport_guard.is_none() {
            return Err(anyhow!("Network transport not available"));
        }

        // Create remote services for each service metadata
        let mut remote_services = Vec::new();

        for service_metadata in capabilities {
            // Create a topic path using the service name as the path
            let service_path = match TopicPath::new(&service_metadata.name, &service_metadata.network_id) {
                Ok(path) => path,
                Err(e) => {
                    logger.error(format!(
                        "Invalid service path '{}': {}",
                        service_metadata.name, e
                    ));
                    continue;
                }
            };

            // Create the remote service
            let service = Arc::new(Self::new(
                service_metadata.name.clone(),
                service_path,
                service_metadata.version.clone(),
                service_metadata.description.clone(),
                peer_id.clone(),
                network_transport.clone(),
                serializer.clone(),
                local_node_id.clone(),
                pending_requests.clone(),
                logger.clone(),
                request_timeout_ms,
            ));

            // Add actions to the service
            for action in service_metadata.actions {
                service.add_action(action.name.clone(), action).await?;
            }
            // Add service to the result list
            remote_services.push(service);
        }


        logger.info(format!(
            "Created {} RemoteService instances",
            remote_services.len()
        ));
        Ok(remote_services)
    }

    /// Get the remote peer identifier for this service
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Get the network identifier for this service path
    pub fn network_id(&self) -> String {
        self.service_topic.network_id()
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
            // let service_clone = service.clone();
            let action = action_name.clone();
            
            // Handle the Result explicitly instead of using the ? operator
            let action_topic_path = match service.service_topic.new_action_topic(&action) {
                Ok(path) => path,
                Err(e) => return Box::pin(async move { Err(anyhow::anyhow!(e)) })
            };

            // Clone all necessary fields before the async block
            let peer_id = service.peer_id.clone();
            let local_node_id = service.local_node_id.clone();
            let pending_requests = service.pending_requests.clone();
            let network_transport = service.network_transport.clone();
            let serializer = service.serializer.clone();
            let request_timeout_ms = service.request_timeout_ms;
                
            Box::pin(async move {
                // Generate a unique request ID
                let request_id = Uuid::new_v4().to_string();

                // Create a channel for receiving the response
                let (tx, rx) = tokio::sync::oneshot::channel();

                // Store the response channel
                pending_requests
                    .write()
                    .await
                    .insert(request_id.clone(), tx);                

                let serializer = serializer.read().await;
                // Serialize the parameters and convert from Arc<[u8]> to Vec<u8>
                let payload_vec: Vec<u8> = match if let Some(params) = params {
                    serializer.serialize_value(&params)
                } else {
                    serializer.serialize_value(&ArcValueType::null())
                } {
                    Ok(bytes) => bytes.to_vec(),  // Convert Arc<[u8]> to Vec<u8>
                    Err(e) => return Err(anyhow::anyhow!("Serialization error: {}", e)),
                };

                // Create the network message
                let message = NetworkMessage {
                    source: local_node_id.clone(),
                    destination: peer_id.clone(),
                    message_type: "Request".to_string(),
                    payloads: vec![NetworkMessagePayloadItem::new(
                        action_topic_path.as_str().to_string(),
                        payload_vec,
                        request_id.clone(),
                    )],
                };

                // Send the request
                if let Some(transport) = &*network_transport.read().await {
                    if let Err(e) = transport.send_message(message).await {
                        // Clean up the pending request
                        pending_requests
                            .write()
                            .await
                            .remove(&request_id);
                        return Err(anyhow::anyhow!("Failed to send request: {}", e));
                    }
                } else {
                    return Err(anyhow::anyhow!("Network transport not available"));
                }

                // Wait for the response with a timeout
                match tokio::time::timeout(
                    std::time::Duration::from_millis(request_timeout_ms),
                    rx,
                )
                .await
                {
                    Ok(Ok(Ok(response))) => Ok(response),
                    Ok(Ok(Err(e))) => Err(anyhow::anyhow!("Remote service error: {}", e)),
                    Ok(Err(_)) => {
                        // Clean up the pending request
                        pending_requests
                            .write()
                            .await
                            .remove(&request_id);
                        Err(anyhow::anyhow!("Response channel closed"))
                    }
                    Err(_) => {
                        // Clean up the pending request
                        pending_requests
                            .write()
                            .await
                            .remove(&request_id);
                        Err(anyhow::anyhow!("Request timeout"))
                    }
                }
            })
        })
    }
 
    /// Get a list of available actions this service can handle
    ///
    /// INTENTION: Provide a way to identify all actions that this remote service
    /// can handle, to be used during initialization for registering handlers.
    pub async fn get_available_actions(&self) -> Vec<String> {
        let actions = self.actions.read().await;
        actions.keys().cloned().collect()
    }

    /// Initialize the remote service and register its handlers
    ///
    /// INTENTION: Handle service initialization and register all available
    /// action handlers with the provided context.
    pub async fn init(&self, context: crate::services::RemoteLifecycleContext) -> Result<()> {
        // Get available actions
        let action_names = self.get_available_actions().await;

        // Register each action handler
        for action_name in action_names {
            if let Ok(action_topic_path) = self.service_topic.new_action_topic(&action_name) {
                // Create handler for this action
                let handler = self.create_action_handler(action_name.clone());

                context
                    .register_remote_action_handler(&action_topic_path, handler)
                    .await?;
            } else {
                self.logger.warn(format!(
                    "Failed to create topic path for action: {}/{}",
                    self.service_topic, action_name
                ));
            }
        }

        Ok(())
    }

    pub async fn stop(&self, context: crate::services::RemoteLifecycleContext) -> Result<()> {
        let action_names = self.get_available_actions().await;
    
        for action_name in action_names {
            if let Ok(action_topic_path) = self.service_topic.new_action_topic(&action_name) {
                context
                    .remove_remote_action_handler(&action_topic_path)
                    .await?;
            } else {
                self.logger.warn(format!(
                    "Failed to create topic path for action: {}/{}",
                    self.service_topic, action_name
                ));
            }
        }
    
        Ok(())
    }
    
}

#[async_trait]
impl AbstractService for RemoteService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        self.service_topic.as_str()
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn network_id(&self) -> Option<String> {
        Some(self.service_topic.network_id())
    }

    async fn init(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need initialization since they're just proxies
        self.logger.info(format!(
            "Initialized remote service proxy for {}",
            self.service_topic
        ));
        Ok(())
    }

    async fn start(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need to be started
        self.logger.info(format!(
            "Started remote service proxy for {}",
            self.service_topic
        ));
        Ok(())
    }

    async fn stop(&self, _context: LifecycleContext) -> Result<()> {
        // Remote services don't need to be stopped
        self.logger.info(format!(
            "Stopped remote service proxy for {}",
            self.service_topic
        ));
        Ok(())
    }
}
