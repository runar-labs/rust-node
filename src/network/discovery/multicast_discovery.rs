// Multicast-based Node Discovery
//
// INTENTION: Provide an implementation of the NodeDiscovery trait that uses
// UDP multicast to discover nodes on the local network. This implementation
// periodically broadcasts announcements and listens for announcements from
// other nodes.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Serialize, Deserialize};
use socket2::{Socket, Domain, Type, Protocol};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use tokio::sync::{RwLock, Mutex, mpsc::{self, Sender}};
use std::time::{Duration, SystemTime};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time;
use bincode;

use super::{NodeDiscovery, NodeInfo, DiscoveryOptions, DiscoveryListener};
use crate::network::transport::PeerId;

// Default multicast address and port
const DEFAULT_MULTICAST_ADDR: &str = "239.255.42.98";
const DEFAULT_MULTICAST_PORT: u16 = 45678;

// Message formats for multicast communication
#[derive(Debug, Clone, Serialize, Deserialize)]
enum MulticastMessage {
    // Node announces its presence
    Announce(NodeInfo),
    // Node requests others to announce themselves
    DiscoveryRequest(String), // network_id
    // Node is leaving the network
    Goodbye(PeerId),
}

impl MulticastMessage {
    // Helper to get the sender ID if the message contains it
    fn sender_id(&self) -> Option<&PeerId> {
        match self {
            MulticastMessage::Announce(info) => Some(&info.peer_id),
            MulticastMessage::Goodbye(id) => Some(id),
            MulticastMessage::DiscoveryRequest(_) => None, // Requests don't inherently have a sender ID in this structure
        }
    }
    
    // Helper to set sender ID (useful for sender task)
    // Consumes self and returns a new message, might need adjustment based on usage
    fn with_sender_id(mut self, sender: PeerId) -> Self {
         match &mut self {
            MulticastMessage::Announce(ref mut info) => info.peer_id = sender,
            MulticastMessage::Goodbye(ref mut id) => *id = sender,
            MulticastMessage::DiscoveryRequest(_) => {}, // No ID field
        }
        self
    }
}

// Define DiscoveryCallback type alias for clarity
type DiscoveryCallback = Box<dyn Fn(NodeInfo) + Send + Sync>;

/// Multicast-based node discovery implementation
pub struct MulticastDiscovery {
    options: Arc<RwLock<DiscoveryOptions>>, 
    nodes: Arc<RwLock<HashMap<String, NodeInfo>>>, 
    local_node: Arc<RwLock<Option<NodeInfo>>>, 
    socket: Arc<UdpSocket>,
    listeners: Arc<RwLock<Vec<DiscoveryListener>>>, 
    tx: Arc<Mutex<Option<Sender<MulticastMessage>>>>, 
    // Task fields
    listener_task: Mutex<Option<JoinHandle<()>>>, 
    sender_task: Mutex<Option<JoinHandle<()>>>,   
    announce_task: Mutex<Option<JoinHandle<()>>>, 
    cleanup_task: Mutex<Option<JoinHandle<()>>>,  
    // Discovery callback field 
    discovery_callback: RwLock<Option<DiscoveryCallback>>, 
    // Multicast address field
    multicast_addr: Arc<Mutex<SocketAddr>>, 
}

impl MulticastDiscovery {
    /// Create a new multicast discovery instance
    pub async fn new(options: DiscoveryOptions) -> Result<Self> {
        // Parse multicast group - handle both formats: "239.255.42.98" and "239.255.42.98:45678"
        let (multicast_addr, port) = if options.multicast_group.contains(':') {
            // Parse as a SocketAddr "IP:PORT"
            let addr: SocketAddr = options.multicast_group.parse()
                .map_err(|e| anyhow!("Invalid multicast address format: {}", e))?;
            (addr.ip(), addr.port())
        } else {
            // Parse as just an IP, use default port
            let ip: Ipv4Addr = options.multicast_group.parse()
                .map_err(|e| anyhow!("Invalid multicast address: {}", e))?;
            (IpAddr::V4(ip), DEFAULT_MULTICAST_PORT)
        };

        // Ensure it's a valid multicast address
        if let IpAddr::V4(ipv4) = multicast_addr {
            if !ipv4.is_multicast() {
                return Err(anyhow!("Not a valid multicast IPv4 address: {}", ipv4));
            }
        } else {
            return Err(anyhow!("Multicast address must be IPv4"));
        }

        // Create socket address
        let socket_addr = SocketAddr::new(multicast_addr, port);
        
        // Create UDP socket with proper configuration
        let socket = Self::create_multicast_socket(socket_addr).await?;
        println!("Successfully created multicast socket with address: {}", socket_addr);

        Ok(Self {
            options: Arc::new(RwLock::new(options)),
            nodes: Arc::new(RwLock::new(HashMap::new())), 
            local_node: Arc::new(RwLock::new(None)), 
            socket: Arc::new(socket),
            listeners: Arc::new(RwLock::new(Vec::new())), 
            tx: Arc::new(Mutex::new(None)), 
            multicast_addr: Arc::new(Mutex::new(socket_addr)),
            listener_task: Mutex::new(None), 
            sender_task: Mutex::new(None), 
            announce_task: Mutex::new(None),
            cleanup_task: Mutex::new(None),
            discovery_callback: RwLock::new(None),
        })
    }
    
    /// Create a new multicast discovery with a specific address and port
    pub async fn with_address(addr: &str, port: u16) -> Result<Self> {
        // Create default options and modify the multicast group
        let mut options = DiscoveryOptions::default();
        options.multicast_group = format!("{}:{}", addr, port);
        
        // Create instance with the specified options
        Self::new(options).await
    }
    
    /// Create and configure a multicast socket
    async fn create_multicast_socket(addr: SocketAddr) -> Result<UdpSocket> {
        // Extract IP and port
        let multicast_ip = match addr.ip() {
            IpAddr::V4(ip) => ip,
            _ => return Err(anyhow!("Only IPv4 multicast is supported")),
        };
        
        let port = addr.port();
        
        // Create a socket with socket2 for low-level configuration
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        
        // Configure socket for multicast
        socket.set_reuse_address(true)?;
        
        // On some platforms, set_reuse_port may not be available, try it but don't fail if not
        let _ = socket.set_reuse_port(true);
        
        // Set multicast TTL (how many network hops multicast packets can traverse)
        socket.set_multicast_ttl_v4(2)?;
        
        // Allow loopback (receive our own multicast packets)
        socket.set_multicast_loop_v4(true)?;
        
        // Bind to the port with any address (0.0.0.0)
        let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        socket.bind(&bind_addr.into())?;
        
        // Join the multicast group
        socket.join_multicast_v4(&multicast_ip, &Ipv4Addr::UNSPECIFIED)?;
        
        // Convert to std socket and then to tokio socket
        let std_socket: std::net::UdpSocket = socket.into();
        std_socket.set_nonblocking(true)?;
        
        // Create tokio UDP socket
        let udp_socket = UdpSocket::from_std(std_socket)?;
        
        println!("Created multicast socket bound to {}:{} and joined multicast group {}", 
                 Ipv4Addr::UNSPECIFIED, port, multicast_ip);
        
        Ok(udp_socket)
    }
    
    /// Start the receive task for listening to multicast messages
    fn start_listener_task(&self) -> JoinHandle<()> {
        let socket = Arc::clone(&self.socket);
        let nodes = Arc::clone(&self.nodes);
        let listeners = Arc::clone(&self.listeners);
        let local_node = Arc::clone(&self.local_node); 
        let socket_for_process = Arc::clone(&self.socket); 

        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            
            loop {
                match socket.recv_from(&mut buf).await {
                    Ok((len, src)) => {
                        println!("Received multicast message from {}, size: {}", src, len);
                        match bincode::deserialize::<MulticastMessage>(&buf[..len]) {
                            Ok(message) => {
                                let local_node_guard = local_node.read().await; 
                                // Use helper method to check sender ID
                                let mut skip = false;
                                if let Some(local_info) = local_node_guard.as_ref() {
                                    if let Some(sender) = message.sender_id() {
                                        if sender == &local_info.peer_id {
                                            skip = true; // Skip message from self
                                            println!("Skipping message from self");
                                        }
                                    }
                                }
                                drop(local_node_guard); 

                                if !skip {
                                    Self::process_message(message, src, &nodes, &listeners, &socket_for_process, &local_node).await;
                                }
                            },
                            Err(e) => println!("Failed to deserialize multicast message: {}", e),
                        }
                    },
                    Err(e) => {
                        println!("Failed to receive multicast message: {}", e);
                        // Brief pause to avoid tight loop on error
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        })
    }
    
    /// Start the announce task for periodically announcing our presence
    fn start_announce_task(&self, tx: Sender<MulticastMessage>, info: NodeInfo, interval: Duration) -> JoinHandle<()> {
         tokio::spawn(async move {
            let mut ticker = time::interval(interval);
            let mut current_info = info.clone();

            loop {
                ticker.tick().await;
                // Update last_seen timestamp
                current_info.last_seen = SystemTime::now();
                // Send announcement
                println!("Sending announcement for node {}", current_info.peer_id);
                if tx.send(MulticastMessage::Announce(current_info.clone())).await.is_err() {
                    println!("Failed to send periodic announcement, channel closed.");
                    break; // Stop task if channel is closed
                }
            }
        })
    }
    
    /// Start a task that periodically cleans up stale nodes
    fn start_cleanup_task(&self, nodes: Arc<RwLock<HashMap<String, NodeInfo>>>, ttl: Duration) -> JoinHandle<()> {
         tokio::spawn(async move {
            let mut interval = time::interval(Duration::from_secs(60)); // Check every 60s
            loop {
                interval.tick().await;
                Self::cleanup_stale_nodes(&nodes, ttl).await;
            }
        })
    }
    
    /// Task to send outgoing messages (announcements, requests)
    fn start_sender_task(&self) -> (JoinHandle<()>, Sender<MulticastMessage>) {
        let (tx, mut rx) = mpsc::channel::<MulticastMessage>(100);
        let socket = Arc::clone(&self.socket);
        let local_node = Arc::clone(&self.local_node);
        let multicast_addr_arc = Arc::clone(&self.multicast_addr); 

        let task = tokio::spawn(async move {
            let target_addr = *multicast_addr_arc.lock().await;
            
            while let Some(mut message) = rx.recv().await { 
                let local_node_guard = local_node.read().await; 
                if let Some(info) = local_node_guard.as_ref() {
                    match &mut message { 
                        MulticastMessage::Announce(ref mut node_info) => node_info.peer_id = info.peer_id.clone(),
                        MulticastMessage::DiscoveryRequest(_) => { /* No sender ID needed */ }, 
                        MulticastMessage::Goodbye(ref mut id) => *id = info.peer_id.clone(),
                    }
                }
                drop(local_node_guard); 
                
                match bincode::serialize(&message) {
                    Ok(data) => {
                        println!("Sending multicast message to {}, size: {}", target_addr, data.len());
                        if let Err(e) = socket.send_to(&data, target_addr).await {
                            println!("Failed to send multicast message: {}", e);
                        }
                    },
                    Err(e) => println!("Failed to serialize multicast message: {}", e),
                }
            }
        });

        (task, tx)
    }

    /// Process a received multicast message
    async fn process_message(
        message: MulticastMessage,
        src: SocketAddr, 
        nodes: &Arc<RwLock<HashMap<String, NodeInfo>>>, 
        listeners: &Arc<RwLock<Vec<DiscoveryListener>>>, 
        socket: &Arc<UdpSocket>, 
        local_node: &Arc<RwLock<Option<NodeInfo>>> 
    ) {
        match message {
            // Announce: Store info and notify listeners
            MulticastMessage::Announce(info) => {
                println!("Processing announce message from {}", info.peer_id);
                
                // Store the node info
                let key = info.peer_id.to_string();
                {
                    let mut nodes_write = nodes.write().await; 
                    nodes_write.insert(key, info.clone());
                }
                
                // Notify listeners
                {
                    let listeners_read = listeners.read().await; 
                    for listener in listeners_read.iter() {
                        println!("Notifying listener about node {}", info.peer_id);
                        listener(info.clone());
                    }
                }
                
                // Automatically respond with our own info to facilitate bidirectional discovery
                let local_node_guard = local_node.read().await;
                if let Some(local_info) = local_node_guard.as_ref() {
                    // Only respond if this is a different node
                    if local_info.peer_id != info.peer_id {
                        println!("Auto-responding to announcement with our own info: {}", local_info.peer_id);
                        let response_msg = MulticastMessage::Announce(local_info.clone());
                        if let Ok(data) = bincode::serialize(&response_msg) {
                            // Small delay to avoid collision
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            // Respond directly to the sender
                            if let Err(e) = socket.send_to(&data, src).await {
                                println!("Failed to send auto-response to {}: {}", src, e);
                            }
                        }
                    }
                }
            },
            // Discovery Request: Respond with local node info if appropriate
            MulticastMessage::DiscoveryRequest(network_id) => {
                println!("Processing discovery request for network {}", network_id);
                let local_node_guard = local_node.read().await;
                if let Some(local_info) = local_node_guard.as_ref() {
                    // Respond only if the request matches one of our network IDs
                    if local_info.network_ids.contains(&network_id) {
                        println!("Responding to discovery request with our info: {}", local_info.peer_id);
                        let response_msg = MulticastMessage::Announce(local_info.clone());
                        if let Ok(data) = bincode::serialize(&response_msg) {
                            // Small delay to avoid collision
                            tokio::time::sleep(Duration::from_millis(50)).await;
                            // Respond directly to the sender using the main socket
                            if let Err(e) = socket.send_to(&data, src).await {
                                println!("Failed to send discovery response to {}: {}", src, e);
                            }
                        } else {
                            println!("Failed to serialize discovery response");
                        }
                    }
                }
            },
            // Goodbye: Remove node
            MulticastMessage::Goodbye(identifier) => {
                println!("Processing goodbye message from {}", identifier);
                let key = identifier.to_string();
                let mut nodes_write = nodes.write().await; 
                nodes_write.remove(&key);
            },
        }
    }
    
    // cleanup_stale_nodes needs to be async due to lock
    async fn cleanup_stale_nodes(nodes: &Arc<RwLock<HashMap<String, NodeInfo>>>, ttl: Duration) {
        let mut nodes_map = nodes.write().await;
        let stale_keys: Vec<String> = nodes_map.iter()
            .filter_map(|(key, info)| {
                info.last_seen.elapsed()
                    .ok()
                    .filter(|elapsed| *elapsed > ttl)
                    .map(|_| key.clone())
            })
            .collect();
        for key in stale_keys {
            println!("Removing stale node: {}", key);
            nodes_map.remove(&key);
        }
    }
}

#[async_trait]
impl NodeDiscovery for MulticastDiscovery {
    async fn init(&self, options: DiscoveryOptions) -> Result<()> {
        println!("Initializing MulticastDiscovery with options: {:?}", options);
        *self.options.write().await = options.clone();
        
        // Re-parse the multicast address from options to ensure it's valid
        let (multicast_addr, port) = if options.multicast_group.contains(':') {
            // Parse as a SocketAddr "IP:PORT"
            let addr: SocketAddr = options.multicast_group.parse()
                .map_err(|e| anyhow!("Invalid multicast address format: {}", e))?;
            (addr.ip(), addr.port())
        } else {
            // Parse as just an IP, use default port
            let ip: Ipv4Addr = options.multicast_group.parse()
                .map_err(|e| anyhow!("Invalid multicast address: {}", e))?;
            (IpAddr::V4(ip), DEFAULT_MULTICAST_PORT)
        };
        
        // Create valid socket address and store it
        let socket_addr = SocketAddr::new(multicast_addr, port);
        *self.multicast_addr.lock().await = socket_addr;
        println!("Using multicast address: {}", socket_addr);

        let listener_handle = self.start_listener_task();
        *self.listener_task.lock().await = Some(listener_handle); 

        // Call start_sender_task and store results
        let (sender_handle, tx) = self.start_sender_task(); 
        *self.sender_task.lock().await = Some(sender_handle); 
        *self.tx.lock().await = Some(tx); 
        
        // Start cleanup task
        let cleanup_handle = self.start_cleanup_task(Arc::clone(&self.nodes), options.node_ttl); 
        *self.cleanup_task.lock().await = Some(cleanup_handle);
        
        Ok(())
    }
    
    async fn start_announcing(&self, info: NodeInfo) -> Result<()> {
        println!("Starting to announce node: {}", info.peer_id);
        *self.local_node.write().await = Some(info.clone());
        
        let tx_opt = self.tx.lock().await;
        let tx = match tx_opt.as_ref() {
            Some(tx_channel) => tx_channel.clone(),
            None => return Err(anyhow!("Discovery sender task not initialized")),
        };
        drop(tx_opt);
        
        let interval = {
            let options_guard = self.options.read().await;
            options_guard.announce_interval
        };
        
        let task = self.start_announce_task(tx.clone(), info.clone(), interval);
        *self.announce_task.lock().await = Some(task);

        // Send initial announcement and return Result
        println!("Sending initial announcement");
        tx.send(MulticastMessage::Announce(info)).await.map_err(|e| anyhow!("Failed to send initial announcement: {}", e))?;
        
        Ok(())
    }
    
    async fn stop_announcing(&self) -> Result<()> {
        // Send goodbye message
        let info_opt = self.local_node.read().await; 
        if let Some(info) = info_opt.as_ref() {
            println!("Sending goodbye message for node: {}", info.peer_id);
            let message = MulticastMessage::Goodbye(info.peer_id.clone());
            let tx_opt = self.tx.lock().await; 
             if let Some(ref tx) = *tx_opt {
                 // Ignore error if channel is already closed during shutdown
                 let _ = tx.send(message).await;
             }
        }
        drop(info_opt); 
        
        // Stop announce task
        if let Some(task) = self.announce_task.lock().await.take() {
            task.abort();
        }
        
        Ok(())
    }
    
    async fn discover_nodes(&self, network_id: Option<&str>) -> Result<Vec<NodeInfo>> {
        // Send discovery request for the specified network or all known networks
        let target_network = {
            let info_guard = self.local_node.read().await;
            network_id.map(|s| s.to_string())
                .or_else(|| {
                    // Use the first network ID from local info as default, or "default"
                    info_guard.as_ref().and_then(|info| info.network_ids.first().cloned())
                })
                .unwrap_or_else(|| "default".to_string())
        };
        
        println!("Sending discovery request for network: {}", target_network);
        {
            let tx_guard = self.tx.lock().await; 
            if let Some(ref tx) = *tx_guard {
                let _ = tx.send(MulticastMessage::DiscoveryRequest(target_network.clone())).await;
            }
        }

        // Allow some time for responses (adjust as needed)
        let timeout_duration = {
            let options_guard = self.options.read().await;
            options_guard.discovery_timeout
        };
        println!("Waiting {} ms for discovery responses", timeout_duration.as_millis());
        tokio::time::sleep(timeout_duration).await;

        // Return current state of discovered nodes, filtering by network_id
        let nodes_read = self.nodes.read().await;
        let nodes_vec: Vec<NodeInfo> = nodes_read.values()
            .filter(|info| info.network_ids.contains(&target_network))
            .cloned()
            .collect();
        
        println!("Discovered {} nodes in network {}", nodes_vec.len(), target_network);
        Ok(nodes_vec)
    }
    
    async fn set_discovery_listener(&self, listener: DiscoveryListener) -> Result<()> {
        println!("Adding discovery listener");
        self.listeners.write().await.push(listener);
        Ok(())
    }
    
    async fn shutdown(&self) -> Result<()> {
        println!("Shutting down MulticastDiscovery");
        // Send goodbye message
        let info_opt = self.local_node.read().await; 
        if let Some(info) = info_opt.as_ref() {
            let message = MulticastMessage::Goodbye(info.peer_id.clone());
            let tx_opt = self.tx.lock().await; 
             if let Some(ref tx) = *tx_opt {
                 // Ignore error if channel is already closed during shutdown
                 let _ = tx.send(message).await;
             }
        }
        drop(info_opt); 
        
        // Stop tasks
        if let Some(task) = self.listener_task.lock().await.take() { task.abort(); }
        if let Some(task) = self.sender_task.lock().await.take() { task.abort(); }
        if let Some(task) = self.announce_task.lock().await.take() { task.abort(); }
        if let Some(task) = self.cleanup_task.lock().await.take() { task.abort(); }
        
        // Clear sender channel
        if let Some(_tx_sender) = self.tx.lock().await.take() {
            // Sender is dropped here, closing the channel
        }
        
        // Clear internal state
        self.nodes.write().await.clear();
        *self.local_node.write().await = None;
        
        Ok(())
    }

    async fn register_node(&self, node_info: NodeInfo) -> Result<()> {
        println!("Manually registering node: {}", node_info.peer_id);
        let key = node_info.peer_id.to_string();
        let mut nodes = self.nodes.write().await;
        nodes.insert(key, node_info.clone());
        
        // Notify listeners
        drop(nodes); // Release lock before notifying
        let listeners = self.listeners.read().await;
        for listener in listeners.iter() {
            listener(node_info.clone());
        }
        
        Ok(())
    }

    async fn update_node(&self, node_info: NodeInfo) -> Result<()> {
        println!("Updating node: {}", node_info.peer_id);
        // Same implementation as register_node for this implementation
        self.register_node(node_info).await
    }

    async fn find_node(&self, network_id: &str, node_id: &str) -> Result<Option<NodeInfo>> {
        let nodes_read = self.nodes.read().await;
        // Create PeerId using only node_id
        let key = PeerId::new(node_id.to_string()).to_string();
        let node = nodes_read.get(&key).cloned();
        
        if let Some(ref info) = node {
            println!("Found node {}/{} in local registry", network_id, node_id);
        } else {
            println!("Node {}/{} not found in local registry", network_id, node_id);
        }
        
        Ok(node)
    }
} 