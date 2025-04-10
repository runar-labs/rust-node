use anyhow::Result;
use async_trait::async_trait;
use runar_common::Logger;
use runar_node::network::transport::{NetworkTransport, NetworkMessage, PeerId, TransportFactory, MessageHandler, PeerRegistry};
use runar_node::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use std::sync::Arc;

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
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    
    async fn send(&self, _message: NetworkMessage) -> Result<()> {
        Ok(())
    }
    
    async fn connect(&self, _node_info: &NodeInfo) -> Result<()> {
        Ok(())
    }
    
    fn register_handler(&self, _handler: MessageHandler) -> Result<()> {
        Ok(())
    }
    
    async fn local_address(&self) -> Option<std::net::SocketAddr> {
        None
    }

    fn peer_registry(&self) -> &PeerRegistry {
        &self.peer_registry
    }

    fn local_node_id(&self) -> &PeerId {
        &self.node_id
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