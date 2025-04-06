// Peer Registry Component
//
// INTENTION: Maintain a registry of known peers in the network,
// track their status, and provide lookup capabilities to find specific
// peers based on identifiers or network.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, SystemTime};
use anyhow::Result;

use super::NodeIdentifier;
use crate::network::discovery::NodeInfo;

/// Status of a peer in the registry
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerStatus {
    /// Peer is known but not connected
    Discovered,
    /// Connection to peer is being established
    Connecting,
    /// Peer is connected and ready for communication
    Connected,
    /// Connection to peer is being closed
    Disconnecting,
    /// Peer was previously connected but is now disconnected
    Disconnected,
}

/// Information about a peer, including its connection status and networks it participates in
#[derive(Debug, Clone)]
pub struct PeerEntry {
    /// Basic node information
    pub info: NodeInfo,
    /// Current status of the peer
    pub status: PeerStatus,
    /// Last status change timestamp
    pub status_changed: SystemTime,
    /// Connection attempts if in Connecting state
    pub connection_attempts: u32,
    /// Networks this peer participates in
    pub networks: HashSet<String>,
    /// Additional metadata about the peer (arbitrary key-value pairs)
    pub metadata: HashMap<String, String>,
    /// Public key of the peer (used as the primary identifier)
    pub public_key: Option<String>,
}

impl PeerEntry {
    /// Create a new peer entry from node information
    pub fn new(info: NodeInfo) -> Self {
        let mut networks = HashSet::new();
        networks.insert(info.identifier.network_id.clone());
        
        Self {
            info,
            status: PeerStatus::Discovered,
            status_changed: SystemTime::now(),
            connection_attempts: 0,
            networks,
            metadata: HashMap::new(),
            public_key: None,
        }
    }

    /// Create a new peer entry with a public key
    pub fn with_public_key(info: NodeInfo, public_key: String) -> Self {
        let mut entry = Self::new(info);
        entry.public_key = Some(public_key);
        entry
    }

    /// Update the peer's status
    pub fn set_status(&mut self, status: PeerStatus) {
        self.status = status;
        self.status_changed = SystemTime::now();
    }

    /// Add a network to this peer's list of networks
    pub fn add_network(&mut self, network_id: String) {
        self.networks.insert(network_id);
    }

    /// Add or update metadata for this peer
    pub fn set_metadata(&mut self, key: String, value: String) {
        self.metadata.insert(key, value);
    }
}

/// Options for the peer registry
#[derive(Debug, Clone)]
pub struct PeerRegistryOptions {
    /// Maximum number of peers to track per network
    pub max_peers_per_network: usize,
    /// How long to keep peers in the registry without activity
    pub peer_ttl: Duration,
    /// How often to run cleanup of stale peers
    pub cleanup_interval: Duration,
}

impl Default for PeerRegistryOptions {
    fn default() -> Self {
        Self {
            max_peers_per_network: 100,
            peer_ttl: Duration::from_secs(3600), // 1 hour
            cleanup_interval: Duration::from_secs(300), // 5 minutes
        }
    }
}

/// Registry of known peers in the network
pub struct PeerRegistry {
    /// Peers indexed by their public key (when available) or node_id
    peers: RwLock<HashMap<String, PeerEntry>>,
    /// Index of network_id to peer_ids for efficient lookup
    network_index: RwLock<HashMap<String, HashSet<String>>>,
    /// Configuration options
    options: PeerRegistryOptions,
}

impl PeerRegistry {
    /// Create a new peer registry with default options
    pub fn new() -> Self {
        Self::with_options(PeerRegistryOptions::default())
    }

    /// Create a new peer registry with custom options
    pub fn with_options(options: PeerRegistryOptions) -> Self {
        Self {
            peers: RwLock::new(HashMap::new()),
            network_index: RwLock::new(HashMap::new()),
            options,
        }
    }

    /// Get primary identifier for a peer
    fn get_peer_id(public_key: Option<&str>, node_id: &str) -> String {
        // Use public key as ID if available, otherwise use node_id
        public_key.map_or_else(|| node_id.to_string(), |pk| pk.to_string())
    }

    /// Add a peer to the registry
    pub fn add_peer(&self, info: NodeInfo, public_key: Option<String>) -> Result<()> {
        let network_id = info.identifier.network_id.clone();
        let node_id = info.identifier.node_id.clone();
        let peer_id = Self::get_peer_id(public_key.as_deref(), &node_id);
        
        let mut peers = self.peers.write().unwrap();
        let mut network_index = self.network_index.write().unwrap();
        
        // Get or create the network's peer set
        let network_peers = network_index
            .entry(network_id.clone())
            .or_insert_with(HashSet::new);
            
        // Check if we've reached the limit for this network
        if !network_peers.contains(&peer_id) && 
           network_peers.len() >= self.options.max_peers_per_network {
            anyhow::bail!("Maximum number of peers reached for network {}", network_id);
        }
        
        // Add to the network index
        network_peers.insert(peer_id.clone());
        
        // Add or update peer
        if let Some(existing_peer) = peers.get_mut(&peer_id) {
            // Update existing peer
            existing_peer.add_network(network_id);
            existing_peer.info.last_seen = info.last_seen;
            // Only update address and capabilities if they exist
            if !info.address.is_empty() {
                existing_peer.info.address = info.address;
            }
            existing_peer.info.capabilities.extend(info.capabilities);
        } else {
            // Create new peer
            let peer_entry = match public_key {
                Some(pk) => PeerEntry::with_public_key(info, pk),
                None => PeerEntry::new(info),
            };
            peers.insert(peer_id, peer_entry);
        }
            
        Ok(())
    }

    /// Update a peer's status
    pub fn update_peer_status(&self, id: &NodeIdentifier, status: PeerStatus, public_key: Option<&str>) -> Result<()> {
        let peer_id = Self::get_peer_id(public_key, &id.node_id);
        
        let mut peers = self.peers.write().unwrap();
        
        if let Some(peer) = peers.get_mut(&peer_id) {
            peer.set_status(status);
            Ok(())
        } else {
            anyhow::bail!("Peer not found: {}", peer_id)
        }
    }

    /// Update a peer's last seen time and other information
    pub fn update_peer(&self, info: NodeInfo, public_key: Option<&str>) -> Result<()> {
        let network_id = info.identifier.network_id.clone();
        let peer_id = Self::get_peer_id(public_key, &info.identifier.node_id);
        
        let mut peers = self.peers.write().unwrap();
        let mut network_index = self.network_index.write().unwrap();
        
        // Ensure the peer is in the network index
        let network_peers = network_index
            .entry(network_id.clone())
            .or_insert_with(HashSet::new);
        network_peers.insert(peer_id.clone());
        
        if let Some(peer) = peers.get_mut(&peer_id) {
            // Update peer
            peer.info.last_seen = info.last_seen;
            peer.add_network(network_id);
            // Only update address and capabilities if they exist
            if !info.address.is_empty() {
                peer.info.address = info.address;
            }
            peer.info.capabilities.extend(info.capabilities);
            Ok(())
        } else {
            anyhow::bail!("Peer not found: {}", peer_id)
        }
    }

    /// Find a peer by its node identifier and optional public key
    pub fn find_peer(&self, id: &NodeIdentifier, public_key: Option<&str>) -> Option<PeerEntry> {
        let peer_id = Self::get_peer_id(public_key, &id.node_id);
        
        let peers = self.peers.read().unwrap();
        peers.get(&peer_id).cloned()
    }

    /// Find a peer by its public key
    pub fn find_peer_by_public_key(&self, public_key: &str) -> Option<PeerEntry> {
        let peers = self.peers.read().unwrap();
        peers.get(public_key).cloned()
    }

    /// Find all peers in a specific network
    pub fn find_peers_by_network(&self, network_id: &str) -> Vec<PeerEntry> {
        let peers = self.peers.read().unwrap();
        let network_index = self.network_index.read().unwrap();
        
        if let Some(peer_ids) = network_index.get(network_id) {
            peer_ids.iter()
                .filter_map(|id| peers.get(id).cloned())
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Find all peers with a specific status
    pub fn find_peers_by_status(&self, status: PeerStatus) -> Vec<PeerEntry> {
        let peers = self.peers.read().unwrap();
        peers.values()
            .filter(|p| p.status == status)
            .cloned()
            .collect()
    }

    /// Get all known peers
    pub fn get_all_peers(&self) -> Vec<PeerEntry> {
        let peers = self.peers.read().unwrap();
        peers.values().cloned().collect()
    }

    /// Remove a peer from the registry
    pub fn remove_peer(&self, id: &NodeIdentifier, public_key: Option<&str>) -> Result<()> {
        let peer_id = Self::get_peer_id(public_key, &id.node_id);
        
        let mut peers = self.peers.write().unwrap();
        let mut network_index = self.network_index.write().unwrap();
        
        // Remove from networks
        if let Some(peer) = peers.get(&peer_id) {
            for network in &peer.networks {
                if let Some(network_peers) = network_index.get_mut(network) {
                    network_peers.remove(&peer_id);
                }
            }
        }
        
        // Remove peer
        if peers.remove(&peer_id).is_some() {
            Ok(())
        } else {
            anyhow::bail!("Peer not found: {}", peer_id)
        }
    }

    /// Clean up stale peers that haven't been seen recently
    pub fn cleanup_stale_peers(&self) -> usize {
        let now = SystemTime::now();
        let mut removed_count = 0;
        
        // Identify stale peers
        let stale_keys: Vec<String> = {
            let peers = self.peers.read().unwrap();
            
            peers.iter()
                .filter(|(_, peer)| {
                    // Check if the peer has been seen within TTL
                    match peer.info.last_seen.elapsed() {
                        Ok(elapsed) => elapsed > self.options.peer_ttl,
                        Err(_) => false, // Future timestamp, keep it
                    }
                })
                .map(|(key, _)| key.clone())
                .collect()
        };
        
        // Remove them
        if !stale_keys.is_empty() {
            let mut peers = self.peers.write().unwrap();
            let mut network_index = self.network_index.write().unwrap();
            
            for key in stale_keys {
                if let Some(peer) = peers.get(&key) {
                    // Remove from each network index
                    for network in &peer.networks {
                        if let Some(network_peers) = network_index.get_mut(network) {
                            network_peers.remove(&key);
                        }
                    }
                }
                
                // Remove peer
                peers.remove(&key);
                removed_count += 1;
            }
            
            // Clean up empty networks
            let empty_networks: Vec<String> = network_index
                .iter()
                .filter(|(_, peers)| peers.is_empty())
                .map(|(network, _)| network.clone())
                .collect();
                
            for network in empty_networks {
                network_index.remove(&network);
            }
        }
        
        removed_count
    }
} 