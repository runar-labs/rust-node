/* // WebSocket Network Transport Implementation
//
// This module provides a WebSocket-based implementation of the NetworkTransport trait.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_tungstenite::{accept_async, connect_async, tungstenite::protocol::Message, WebSocketStream};
use runar_common::logging::{Component, Logger};
use serde::{Serialize, Deserialize};
use bincode;
use log::{debug, error, info, warn};
use runar_common::utils::logging::{debug_log, info_log};
use socket2::{Domain, Socket, Type};
use tokio::sync::{mpsc, Mutex, Notify};

use super::{NetworkTransport, NetworkMessage, NodeIdentifier, TransportOptions, NetworkMessageType, TransportFactory, PeerRegistry, MessageHandler};

/// WebSocket implementation of NetworkTransport
pub struct WebSocketTransport {
    /// Local address to bind to
    bind_address: SocketAddr,
    /// Node identifier for this node
    node_id: NodeIdentifier,
    /// Connected peers
    peers: Arc<RwLock<HashMap<String, WebSocketPeer>>>,
    /// Message handler callback
    handler: Arc<RwLock<Option<Arc<dyn Fn(NetworkMessage) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>>>,
    /// Logger
    logger: Logger,
    /// Peer registry
    peer_registry: Arc<PeerRegistry>,
}

/// Information about a WebSocket peer
struct WebSocketPeer {
    /// Node identifier
    node_id: NodeIdentifier,
    /// Connection address
    address: String,
    /// Last seen timestamp
    last_seen: u64,
}

/// Serializable network message for WebSocket transport
#[derive(Serialize, Deserialize, Debug, Clone)]
struct WebSocketMessage {
    /// Message type
    #[serde(rename = "type")]
    message_type: String,
    /// Source node network ID
    source_network: String,
    /// Source node ID
    source_node: String,
    /// Destination node network ID (if any)
    destination_network: Option<String>,
    /// Destination node ID (if any)
    destination_node: Option<String>,
    /// Correlation ID for request/response matching
    correlation_id: String,
    /// Topic or path
    topic: String,
    /// Message payload
    payload: serde_json::Value,
    /// Message timestamp
    timestamp: u64,
}

impl WebSocketTransport {
    /// Create a new WebSocketTransport
    pub fn new(bind_address: SocketAddr, node_id: NodeIdentifier) -> Self {
        Self {
            bind_address,
            node_id: node_id.clone(),
            peers: Arc::new(RwLock::new(HashMap::new())),
            handler: Arc::new(RwLock::new(None)),
            logger: Logger::new_root(Component::Network, &node_id.network_id),
            peer_registry: Arc::new(PeerRegistry::new()),
        }
    }

    /// Start the WebSocket server
    async fn start_server(&self) -> Result<()> {
        let addr = self.bind_address;
        let peers = self.peers.clone();
        let handler = self.handler.clone();
        let node_id = self.node_id.clone();
        let logger = self.logger.clone();

        logger.info(format!("Starting WebSocket transport server on {}", addr));

        // Start a TCP listener
        let listener = TcpListener::bind(addr).await?;

        // Handle incoming connections in a background task
        tokio::spawn(async move {
            while let Ok((stream, remote_addr)) = listener.accept().await {
                let peers = peers.clone();
                let handler = handler.clone();
                let node_id = node_id.clone();
                let logger = logger.clone();

                logger.debug(format!("New WebSocket connection from {}", remote_addr));

                // Handle each connection in its own task
                tokio::spawn(async move {
                    match handle_connection(stream, remote_addr, peers, handler, node_id, logger.clone()).await {
                        Ok(_) => (),
                        Err(e) => logger.error(format!("WebSocket connection error: {}", e)),
                    }
                });
            }
        });

        Ok(())
    }

    /// Convert WebSocketMessage to NetworkMessage
    fn convert_to_network_message(&self, msg: WebSocketMessage) -> Result<NetworkMessage> {
        // Convert string message type to enum
        let message_type = match msg.message_type.as_str() {
            "request" => NetworkMessageType::Request,
            "response" => NetworkMessageType::Response,
            "event" => NetworkMessageType::Event,
            "discovery" => NetworkMessageType::Discovery,
            "heartbeat" => NetworkMessageType::Heartbeat,
            _ => return Err(anyhow!("Unknown message type: {}", msg.message_type)),
        };

        // Convert payload from serde_json::Value to ValueType
        let payload = match serde_json::from_value(msg.payload) {
            Ok(value) => value,
            Err(e) => return Err(anyhow!("Failed to deserialize payload: {}", e)),
        };

        // Create source node identifier
        let source = NodeIdentifier::new(msg.source_network, msg.source_node);

        // Create destination node identifier (if any)
        let destination = match (msg.destination_network, msg.destination_node) {
            (Some(network), Some(node)) => Some(NodeIdentifier::new(network, node)),
            _ => None,
        };

        Ok(NetworkMessage {
            message_type,
            source,
            destination,
            correlation_id: msg.correlation_id,
            topic: msg.topic,
            payload,
            timestamp: msg.timestamp,
        })
    }

    /// Convert NetworkMessage to WebSocketMessage
    fn convert_from_network_message(&self, msg: &NetworkMessage) -> Result<WebSocketMessage> {
        // Convert enum message type to string
        let message_type = match msg.message_type {
            NetworkMessageType::Request => "request",
            NetworkMessageType::Response => "response",
            NetworkMessageType::Event => "event",
            NetworkMessageType::Discovery => "discovery",
            NetworkMessageType::Heartbeat => "heartbeat",
        };

        // Convert destination (if any)
        let (destination_network, destination_node) = match &msg.destination {
            Some(dest) => (Some(dest.network_id.clone()), Some(dest.node_id.clone())),
            None => (None, None),
        };

        // Convert payload from ValueType to serde_json::Value
        let payload = match serde_json::to_value(&msg.payload) {
            Ok(value) => value,
            Err(e) => return Err(anyhow!("Failed to serialize payload: {}", e)),
        };

        Ok(WebSocketMessage {
            message_type: message_type.to_string(),
            source_network: msg.source.network_id.clone(),
            source_node: msg.source.node_id.clone(),
            destination_network,
            destination_node,
            correlation_id: msg.correlation_id.clone(),
            topic: msg.topic.clone(),
            payload,
            timestamp: msg.timestamp,
        })
    }

    /// Connect to a peer
    async fn connect_to_peer(&self, address: &str, peer_node_id: NodeIdentifier) -> Result<()> {
        let peers = self.peers.clone();
        let handler = self.handler.clone();
        let node_id = self.node_id.clone();
        let logger = self.logger.clone();
        let address_str = address.to_string();

        self.logger.info(format!("Connecting to peer at {}", address));

        // Connect to the peer in a background task
        tokio::spawn(async move {
            let url = if address_str.starts_with("ws://") || address_str.starts_with("wss://") {
                address_str.clone()
            } else {
                format!("ws://{}", address_str)
            };

            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    logger.info(format!("Connected to peer at {}", address_str));

                    // Store peer information
                    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default().as_secs();

                    let peer = WebSocketPeer {
                        node_id: peer_node_id.clone(),
                        address: address_str.clone(),
                        last_seen: now,
                    };

                    let peer_key = format!("{}:{}", peer_node_id.network_id, peer_node_id.node_id);
                    peers.write().await.insert(peer_key, peer);

                    // Handle the WebSocket connection
                    match handle_outgoing_connection(ws_stream, peers.clone(), handler.clone(), node_id.clone(), peer_node_id.clone(), logger.clone()).await {
                        Ok(_) => (),
                        Err(e) => logger.error(format!("WebSocket connection error: {}", e)),
                    }
                }
                Err(e) => {
                    logger.error(format!("Failed to connect to peer at {}: {}", address_str, e));
                }
            }
        });

        Ok(())
    }
}

/// Handle an incoming WebSocket connection
async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    peers: Arc<RwLock<HashMap<String, WebSocketPeer>>>,
    handler: Arc<RwLock<Option<Arc<dyn Fn(NetworkMessage) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>>>,
    node_id: NodeIdentifier,
    logger: Logger,
) -> Result<()> {
    // Accept the WebSocket connection
    let ws_stream = accept_async(stream).await?;
    logger.debug(format!("WebSocket connection established with {}", addr));

    // Split the WebSocket stream
    let (mut write, mut read) = ws_stream.split();

    // Handle incoming messages
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                logger.debug(format!("Received message from {}: {}", addr, text));

                // Parse the message
                match serde_json::from_str::<WebSocketMessage>(&text) {
                    Ok(ws_msg) => {
                        // Store/update peer information
                        let peer_node_id = NodeIdentifier::new(
                            ws_msg.source_network.clone(),
                            ws_msg.source_node.clone(),
                        );
                        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default().as_secs();

                        let peer = WebSocketPeer {
                            node_id: peer_node_id.clone(),
                            address: addr.to_string(),
                            last_seen: now,
                        };

                        let peer_key = format!("{}:{}", peer_node_id.network_id, peer_node_id.node_id);
                        peers.write().await.insert(peer_key, peer);

                        // Convert to NetworkMessage
                        let transport = WebSocketTransport {
                            bind_address: addr,
                            node_id: node_id.clone(),
                            peers: peers.clone(),
                            handler: handler.clone(),
                            logger: logger.clone(),
                            peer_registry: Arc::new(PeerRegistry::new()),
                        };

                        match transport.convert_to_network_message(ws_msg) {
                            Ok(network_msg) => {
                                // Call the handler (if registered)
                                if let Some(handler_fn) = &*handler.read().await {
                                    match handler_fn(network_msg).await {
                                        Ok(_) => (),
                                        Err(e) => logger.error(format!("Error handling message: {}", e)),
                                    }
                                }
                            }
                            Err(e) => logger.error(format!("Error converting message: {}", e)),
                        }
                    }
                    Err(e) => logger.error(format!("Error parsing message: {}", e)),
                }
            }
            Ok(Message::Close(_)) => {
                logger.debug(format!("WebSocket connection closed by {}", addr));
                break;
            }
            Err(e) => {
                logger.error(format!("WebSocket error from {}: {}", addr, e));
                break;
            }
            _ => (),
        }
    }

    Ok(())
}

/// Handle an outgoing WebSocket connection
async fn handle_outgoing_connection(
    ws_stream: WebSocketStream<TcpStream>,
    peers: Arc<RwLock<HashMap<String, WebSocketPeer>>>,
    handler: Arc<RwLock<Option<Arc<dyn Fn(NetworkMessage) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>>>>,
    node_id: NodeIdentifier,
    peer_node_id: NodeIdentifier,
    logger: Logger,
) -> Result<()> {
    // Split the WebSocket stream
    let (mut write, mut read) = ws_stream.split();

    // Handle incoming messages
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                logger.debug(format!("Received message from peer {}: {}", peer_node_id.node_id, text));

                // Parse the message
                match serde_json::from_str::<WebSocketMessage>(&text) {
                    Ok(ws_msg) => {
                        // Update peer information
                        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default().as_secs();

                        let peer_key = format!("{}:{}", peer_node_id.network_id, peer_node_id.node_id);
                        if let Some(peer) = peers.write().await.get_mut(&peer_key) {
                            peer.last_seen = now;
                        }

                        // Convert to NetworkMessage
                        let transport = WebSocketTransport {
                            bind_address: "0.0.0.0:0".parse().unwrap(),
                            node_id: node_id.clone(),
                            peers: peers.clone(),
                            handler: handler.clone(),
                            logger: logger.clone(),
                            peer_registry: Arc::new(PeerRegistry::new()),
                        };

                        match transport.convert_to_network_message(ws_msg) {
                            Ok(network_msg) => {
                                // Call the handler (if registered)
                                if let Some(handler_fn) = &*handler.read().await {
                                    match handler_fn(network_msg).await {
                                        Ok(_) => (),
                                        Err(e) => logger.error(format!("Error handling message: {}", e)),
                                    }
                                }
                            }
                            Err(e) => logger.error(format!("Error converting message: {}", e)),
                        }
                    }
                    Err(e) => logger.error(format!("Error parsing message: {}", e)),
                }
            }
            Ok(Message::Close(_)) => {
                logger.debug(format!("WebSocket connection closed by peer {}", peer_node_id.node_id));
                break;
            }
            Err(e) => {
                logger.error(format!("WebSocket error from peer {}: {}", peer_node_id.node_id, e));
                break;
            }
            _ => (),
        }
    }

    Ok(())
}

#[async_trait]
impl NetworkTransport for WebSocketTransport {
    async fn init(&self, _options: TransportOptions) -> Result<()> {
        self.logger.info("Initializing WebSocket transport");
        
        // Start the WebSocket server
        self.start_server().await?;
        
        Ok(())
    }
    
    async fn connect(&self, address: &str) -> Result<NodeIdentifier> {
        self.logger.info(format!("Attempting to connect WebSocket to: {}", address));
        let url = if address.starts_with("ws://") || address.starts_with("wss://") {
            address.to_string()
        } else {
            format!("ws://{}", address)
        };

        let (ws_stream, _) = connect_async(&url)
            .await
            .context(format!("Failed to connect WebSocket to {}", address))?;
        
        self.logger.info(format!("WebSocket connected successfully to: {}", address));
        
        // TODO: Need a handshake mechanism to get the remote NodeIdentifier
        // For now, returning a placeholder based on the address
        let placeholder_id = NodeIdentifier::new(self.node_id.network_id.clone(), format!("ws-{}", address));
        
        // Spawn a task to handle the new connection
        let peers_clone = self.peers.clone();
        let handler_clone = self.handler.clone();
        let node_id_clone = self.node_id.clone();
        let logger_clone = self.logger.clone();
        let remote_node_id = placeholder_id.clone(); // Use the placeholder for now
        
        tokio::spawn(async move {
            if let Err(e) = handle_outgoing_connection(
                ws_stream, // Pass the actual ws_stream
                peers_clone, 
                handler_clone, 
                node_id_clone, 
                remote_node_id, // Pass the obtained/placeholder ID
                logger_clone
            ).await {
                error_log!(logger_clone, "Error handling outgoing WS connection to {}: {}", address, e);
            }
        });
        
        Ok(placeholder_id)
    }
    
    async fn send_message(&self, message: NetworkMessage) -> Result<()> {
        let destination = match &message.destination {
            Some(dest) => dest,
            None => return Err(anyhow!("No destination specified for message")),
        };
        
        // Get the peer
        let peer_key = format!("{}:{}", destination.network_id, destination.node_id);
        let peer_opt = self.peers.read().await.get(&peer_key).cloned();
        
        if let Some(peer) = peer_opt {
            // Connect to the peer if needed
            let address = peer.address.clone();
            let dest_id = peer.node_id.clone();
            
            // Convert message to WebSocketMessage
            let ws_message = self.convert_from_network_message(&message)?;
            let message_json = serde_json::to_string(&ws_message)?;
            
            // TODO: Actually send the message through the WebSocket connection
            // For now, log that we would send it
            self.logger.info(format!("Would send message to {}: {}", peer_key, message_json));
            
            Ok(())
        } else {
            Err(anyhow!("No peer found for destination: {}", peer_key))
        }
    }
    
    async fn broadcast_message(&self, message: NetworkMessage) -> Result<()> {
        // Convert message to WebSocketMessage
        let ws_message = self.convert_from_network_message(&message)?;
        let message_json = serde_json::to_string(&ws_message)?;
        
        // Broadcast to all peers in the same network
        let network_id = message.source.network_id.clone();
        let peers = self.peers.read().await;
        
        for (peer_key, peer) in peers.iter() {
            if peer.node_id.network_id == network_id {
                // TODO: Actually send the message to each peer
                // For now, log that we would send it
                self.logger.info(format!("Would broadcast message to {}: {}", peer_key, message_json));
            }
        }
        
        Ok(())
    }
    
    fn register_handler(&self, handler: MessageHandler) -> Result<()> {
        // The MessageHandler type alias is already Box<dyn Fn(...) ...>
        let mut handler_lock = self.handler.blocking_write(); // Still might block, consider async mutex
        *handler_lock = Some(handler); // Store the Box directly
        
        Ok(())
    }
    
    fn peer_registry(&self) -> &PeerRegistry {
        // We need to store PeerRegistry within WebSocketTransport
        // Returning a reference to the stored PeerRegistry
        &self.peer_registry 
    }

    fn local_node_id(&self) -> NodeIdentifier {
        self.node_id.clone()
    }
    
    async fn shutdown(&self) -> Result<()> {
        self.logger.info("Shutting down WebSocket transport");
        
        // Close all peer connections
        // For now, just clear the peers list
        self.peers.write().await.clear();
        
        Ok(())
    }
}

/// Factory for creating WebSocket transport instances
pub struct WebSocketTransportFactory {
    /// Address to bind to
    bind_address: SocketAddr,
}

impl WebSocketTransportFactory {
    /// Create a new WebSocketTransportFactory
    pub fn new(bind_address: SocketAddr) -> Self {
        Self {
            bind_address,
        }
    }
}

impl TransportFactory for WebSocketTransportFactory {
    fn create_transport(&self, node_id: NodeIdentifier) -> Box<dyn NetworkTransport> {
        Box::new(WebSocketTransport::new(self.bind_address, node_id))
    }
}
*/ 