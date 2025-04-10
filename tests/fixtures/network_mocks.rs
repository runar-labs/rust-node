use anyhow::Result;
use async_trait::async_trait;
use runar_common::Logger;
use runar_node::network::transport::{NetworkTransport, NetworkMessage, PeerId, TransportFactory, MessageHandler, PeerRegistry, NetworkError, ConnectionCallback};
use runar_node::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use std::sync::Arc;
use std::net::SocketAddr;

/// A minimal mock transport that does nothing
pub struct MockTransport {
    peer_registry: PeerRegistry,
    node_id: PeerId,
}

impl MockTransport {
    /// Create a new mock transport
    pub fn new(node_id: PeerId) -> Self {
        Self {
            peer_registry: PeerRegistry::new(),
            node_id,
        }
    }
}

#[async_trait]
impl NetworkTransport for MockTransport {
    async fn initialize(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn start(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn stop(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    fn is_running(&self) -> bool {
        false
    }
    
    fn get_local_address(&self) -> String {
        "127.0.0.1:0".to_string()
    }
    
    fn get_local_node_id(&self) -> PeerId {
        self.node_id.clone()
    }
    
    async fn connect(&self, _node_id: PeerId, _address: SocketAddr) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn disconnect(&self, _node_id: PeerId) -> Result<(), NetworkError> {
        Ok(())
    }
    
    fn is_connected(&self, _node_id: PeerId) -> bool {
        false
    }
    
    async fn send_message(&self, _message: NetworkMessage) -> Result<(), NetworkError> {
        Ok(())
    }
    
    fn register_message_handler(&self, _handler: MessageHandler) -> Result<()> {
        Ok(())
    }
    
    fn set_connection_callback(&self, _callback: ConnectionCallback) -> Result<()> {
        Ok(())
    }
    
    fn get_connected_nodes(&self) -> Vec<PeerId> {
        Vec::new()
    }
    
    async fn send_request(&self, _message: NetworkMessage) -> Result<NetworkMessage, NetworkError> {
        Err(NetworkError::TransportError("Not implemented".to_string()))
    }
    
    async fn handle_message(&self, _message: NetworkMessage) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn start_discovery(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn stop_discovery(&self) -> Result<(), NetworkError> {
        Ok(())
    }
    
    async fn register_discovered_node(&self, _node_id: PeerId) -> Result<(), NetworkError> {
        Ok(())
    }
    
    fn get_discovered_nodes(&self) -> Vec<PeerId> {
        Vec::new()
    }
    
    fn set_node_discovery(&self, _discovery: Box<dyn NodeDiscovery>) -> Result<()> {
        Ok(())
    }
    
    fn complete_pending_request(&self, _correlation_id: String, _response: NetworkMessage) -> Result<(), NetworkError> {
        Ok(())
    }
}

/// A transport factory that creates mock transports
pub struct MockTransportFactory;

#[async_trait]
impl TransportFactory for MockTransportFactory {
    type Transport = MockTransport;
    
    async fn create_transport(&self, node_id: PeerId, _logger: Logger) -> Result<Self::Transport> {
        Ok(MockTransport::new(node_id))
    }
}

/// A minimal mock discovery service that does nothing
pub struct MockDiscovery;

#[async_trait]
impl NodeDiscovery for MockDiscovery {
    async fn init(&self, _options: DiscoveryOptions) -> Result<()> {
        Ok(())
    }
    
    async fn start_announcing(&self, _info: NodeInfo) -> Result<()> {
        Ok(())
    }
    
    async fn stop_announcing(&self) -> Result<()> {
        Ok(())
    }
    
    async fn register_node(&self, _node_info: NodeInfo) -> Result<()> {
        Ok(())
    }
    
    async fn update_node(&self, _node_info: NodeInfo) -> Result<()> {
        Ok(())
    }
    
    async fn discover_nodes(&self, _network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        Ok(vec![])
    }
    
    async fn find_node(&self, _network_id: &str, _node_id: &str) -> Result<Option<NodeInfo>> {
        Ok(None)
    }
    
    async fn set_discovery_listener(&self, _listener: DiscoveryListener) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// Function that creates a mock discovery service
pub fn create_mock_discovery_factory() -> Box<dyn FnOnce(Arc<tokio::sync::RwLock<Option<Box<dyn NetworkTransport>>>>, Logger) -> Result<Box<dyn NodeDiscovery>> + Send + Sync + 'static> {
    Box::new(|_, _| {
        Ok(Box::new(MockDiscovery))
    })
} 