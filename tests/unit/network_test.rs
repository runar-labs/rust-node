// Network Module Tests
//
// Tests for the network components including transport and discovery

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use std::net::SocketAddr;
use async_trait::async_trait;

use runar_node::network::transport::{NetworkTransport, NetworkMessage, NodeIdentifier, MessageHandler};
use runar_node::network::discovery::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use runar_common::types::ValueType;
use runar_common::Logger;
use runar_node::network::transport::peer_registry::PeerRegistry;

// Mock implementation of NetworkTransport for testing
struct MockTransport {
    node_id: NodeIdentifier,
    sent_messages: RwLock<Vec<NetworkMessage>>,
    message_sink: Option<mpsc::Sender<NetworkMessage>>,
    logger: Logger,
    peer_registry: Arc<PeerRegistry>,
}

impl MockTransport {
    fn new(network_id: &str, node_id: &str) -> Self {
        Self {
            node_id: NodeIdentifier::new(network_id.to_string(), node_id.to_string()),
            sent_messages: RwLock::new(Vec::new()),
            message_sink: None,
            logger: Logger::new_root(runar_common::Component::Network, node_id),
            peer_registry: Arc::new(PeerRegistry::new()),
        }
    }
    
    fn set_message_sink(&mut self, sink: mpsc::Sender<NetworkMessage>) {
        self.message_sink = Some(sink);
    }
    
    async fn get_sent_messages(&self) -> Vec<NetworkMessage> {
        self.sent_messages.read().await.clone()
    }
}

#[async_trait]
impl NetworkTransport for MockTransport {
    async fn init(&self) -> Result<()> {
        Ok(())
    }
    
    async fn connect(&self, _node_info: &NodeInfo) -> Result<()> {
        Ok(())
    }
    
    async fn send(&self, message: NetworkMessage) -> Result<()> {
        // Save the message
        self.sent_messages.write().await.push(message.clone());
        
        // Forward to message sink if set
        if let Some(sink) = &self.message_sink {
            if let Err(e) = sink.send(message.clone()).await {
                self.logger.error(format!("Failed to send message to sink: {}", e));
            }
        }
        
        Ok(())
    }
    
    fn register_handler(&self, _handler: MessageHandler) -> Result<()> {
        // We're not using handlers directly in this mock
        // Messages will be sent to the message_sink
        Ok(())
    }
    
    fn peer_registry(&self) -> &PeerRegistry {
        &self.peer_registry
    }
    
    fn local_node_id(&self) -> &NodeIdentifier {
        &self.node_id
    }
    
    async fn local_address(&self) -> Option<SocketAddr> {
        // Use a fixed address for mock transport
        Some("127.0.0.1:8090".parse().unwrap())
    }
    
    async fn shutdown(&self) -> Result<()> {
        self.logger.info("Shutting down Mock transport");
        Ok(())
    }
}

// Mock implementation of NodeDiscovery for testing
struct MockDiscovery {
    nodes: RwLock<HashMap<String, Vec<NodeInfo>>>,
    listeners: RwLock<Vec<DiscoveryListener>>,
    logger: Logger,
}

impl MockDiscovery {
    fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            listeners: RwLock::new(Vec::new()),
            logger: Logger::new_root(runar_common::Component::Network, "mock-discovery"),
        }
    }
}

#[async_trait]
impl NodeDiscovery for MockDiscovery {
    async fn init(&self, _options: DiscoveryOptions) -> Result<()> {
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    
    async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        let network_id = node_info.identifier.network_id.clone();
        
        if let Some(network_nodes) = nodes.get_mut(&network_id) {
            network_nodes.push(node_info.clone());
        } else {
            nodes.insert(network_id, vec![node_info.clone()]);
        }
        
        // Notify listeners
        for listener in self.listeners.read().await.iter() {
            listener(node_info.clone());
        }
        
        Ok(())
    }
    
    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        self.listeners.write().await.push(listener);
        Ok(())
    }
    
    async fn start_announcing(&self, _node_info: NodeInfo) -> Result<()> {
        Ok(())
    }
    
    async fn stop_announcing(&self) -> Result<()> {
        // Implementation not needed for tests
        Ok(())
    }
    
    async fn update_node(&self, _node_info: NodeInfo) -> Result<()> {
        // Implementation not needed for tests
        Ok(())
    }
    
    async fn discover_nodes(&self, network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        let nodes = self.nodes.read().await;
        
        if let Some(id) = network_id {
            if let Some(network_nodes) = nodes.get(id) {
                Ok(network_nodes.clone())
            } else {
                Ok(Vec::new())
            }
        } else {
            // Collect all nodes from all networks
            let mut all_nodes = Vec::new();
            for nodes_vec in nodes.values() {
                all_nodes.extend(nodes_vec.clone());
            }
            Ok(all_nodes)
        }
    }
    
    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>> {
        let nodes = self.nodes.read().await;
        
        if let Some(network_nodes) = nodes.get(network_id) {
            Ok(network_nodes.iter()
                .find(|node| node.identifier.node_id == node_id)
                .cloned())
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;
    
    #[tokio::test]
    async fn test_transport_send_message() -> Result<()> {
        let transport = MockTransport::new("test-network", "node-1");
        
        let message = NetworkMessage {
            source: NodeIdentifier::new("test-network".to_string(), "node-1".to_string()),
            destination: Some(NodeIdentifier::new("test-network".to_string(), "node-2".to_string())),
            message_type: "Request".to_string(),
            payload: ValueType::String("test payload".to_string()),
            correlation_id: Some("test-correlation-id".to_string()),
        };
        
        transport.send(message.clone()).await?;
        
        let sent_messages = transport.get_sent_messages().await;
        assert_eq!(sent_messages.len(), 1);
        assert_eq!(sent_messages[0].message_type, "Request");
        assert_eq!(sent_messages[0].correlation_id, Some("test-correlation-id".to_string()));
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_discovery_register_node() -> Result<()> {
        let discovery = MockDiscovery::new();
        let (tx, mut rx) = mpsc::channel::<NodeInfo>(10);
        
        // Set a discovery listener
        discovery.set_discovery_listener(Box::new(move |node_info| {
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Err(e) = tx.send(node_info).await {
                    eprintln!("Channel send error: {}", e);
                }
            });
        })).await?;
        
        let node_info = NodeInfo {
            identifier: NodeIdentifier::new("test-network".to_string(), "node-1".to_string()),
            address: "localhost:8080".to_string(),
            capabilities: vec!["request".to_string(), "event".to_string()],
            last_seen: SystemTime::now(),
        };
        
        // Register node
        discovery.register_node(node_info.clone()).await?;
        
        // Check if listener was notified
        if let Some(received_node_info) = rx.recv().await {
            assert_eq!(received_node_info.identifier.node_id, "node-1");
            assert_eq!(received_node_info.address, "localhost:8080");
        } else {
            return Err(anyhow!("Discovery listener was not notified"));
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_transport_handler() -> Result<()> {
        // Create a channel
        let (tx, mut rx) = mpsc::channel(10);
        
        // Create the transport
        let mut transport = MockTransport::new("test-network", "node-1");
        
        // Set message sink
        transport.set_message_sink(tx);
        
        // Create a test message
        let message = NetworkMessage {
            source: NodeIdentifier::new("test-network".to_string(), "node-2".to_string()),
            destination: Some(NodeIdentifier::new("test-network".to_string(), "node-1".to_string())),
            message_type: "Request".to_string(),
            payload: ValueType::String("test payload".to_string()),
            correlation_id: Some("test-correlation-id".to_string()),
        };
        
        // Send the message
        transport.send(message.clone()).await?;
        
        // Check if the message was received through the channel
        if let Some(received) = rx.recv().await {
            assert_eq!(received.source.node_id, "node-2");
            assert_eq!(received.message_type, "Request");
            assert_eq!(received.correlation_id, Some("test-correlation-id".to_string()));
        } else {
            return Err(anyhow!("No message received"));
        }
        
        Ok(())
    }
} 