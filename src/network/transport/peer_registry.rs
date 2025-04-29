// Peer Registry Component
//
// INTENTION: Maintain a registry of known peers in the network,
// track their status, and provide lookup capabilities to find specific
// peers based on identifiers or network.

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, SystemTime};
use anyhow::Result;

use super::PeerId;
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
    // REMOVED DO NOT PUT BACK - the is is in info.peer_id 
    // Public key of the peer (used as the primary identifier)
    // pub public_key: Option<String>,
}

impl PeerEntry {
    /// Create a new peer entry from node information and network ID
    pub fn new(info: NodeInfo, network_id: String) -> Self {
        let mut networks = HashSet::new();
        // Use the provided network_id, not from info.peer_id
        networks.insert(network_id);
        
        Self {
            info,
            status: PeerStatus::Discovered,
            status_changed: SystemTime::now(),
            connection_attempts: 0,
            networks,
            metadata: HashMap::new(), 
        }
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
    /// Peers indexed by their peer_id
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
    pub fn add_peer(&self, info: NodeInfo) -> Result<()> {
        // Use public key from the peer_id as the unique identifier
        let peer_id_str = info.peer_id.public_key.clone();
        
        // First check if any of the networks have reached their limit
        {
            let network_index = self.network_index.read().unwrap();
            
            for network_id in &info.network_ids {
                if let Some(network_peers) = network_index.get(network_id) {
                    // Only check networks where this peer isn't already registered
                    if !network_peers.contains(&peer_id_str) && 
                       network_peers.len() >= self.options.max_peers_per_network {
                        anyhow::bail!("Maximum number of peers reached for network {}", network_id);
                    }
                }
            }
        }
        
        // If we've passed the limit checks, now we can modify the data structures
        let mut peers = self.peers.write().unwrap();
        let mut network_index = self.network_index.write().unwrap();

        // Add the peer to all networks
        for network_id in &info.network_ids {
            // Get or create the network's peer set
            let network_peers = network_index
                .entry(network_id.clone())
                .or_insert_with(HashSet::new);
                
            // Add to the network index
            network_peers.insert(peer_id_str.clone());
        }
        
        // Add or update peer
        if let Some(existing_peer) = peers.get_mut(&peer_id_str) {
            // Update existing peer
            existing_peer.info.last_seen = info.last_seen;
            // Only update address and capabilities if they exist
            if !info.address.is_empty() {
                existing_peer.info.address = info.address;
            }
            existing_peer.info.capabilities.extend(info.capabilities);
            
            // Update network IDs in the peer entry
            for network_id in &info.network_ids {
                existing_peer.add_network(network_id.clone());
            }
        } else {
            // Create new peer entry with all its networks
            let mut peer_entry = PeerEntry::new(info.clone(), info.network_ids[0].clone());
            
            // Add remaining networks if there are more than one
            for network_id in info.network_ids.iter().skip(1) {
                peer_entry.add_network(network_id.clone());
            }
            
            peers.insert(peer_id_str, peer_entry);
        }
            
        Ok(())
    }

    /// Update a peer's status
    pub fn update_peer_status(&self, id: &PeerId, status: PeerStatus, public_key: Option<&str>) -> Result<()> {
        let peer_id = Self::get_peer_id(public_key, &id.public_key);
        
        let mut peers = self.peers.write().unwrap();
        
        if let Some(peer) = peers.get_mut(&peer_id) {
            peer.set_status(status);
            Ok(())
        } else {
            anyhow::bail!("Peer not found: {}", peer_id)
        }
    }

    /// Update a peer's last seen time and other information
    pub fn update_peer(&self, info: NodeInfo, public_key: Option<&str>, network_id: String) -> Result<()> {
        let peer_id = Self::get_peer_id(public_key, &info.peer_id.public_key);
        
        let mut peers = self.peers.write().unwrap();
        let mut network_index = self.network_index.write().unwrap();
        
        // Ensure the peer is in the network index using the passed network_id
        let network_peers = network_index
            .entry(network_id.clone())
            .or_insert_with(HashSet::new);
        network_peers.insert(peer_id.clone());
        
        if let Some(peer) = peers.get_mut(&peer_id) {
            // Update peer
            peer.info.last_seen = info.last_seen;
            peer.add_network(network_id.clone());
            // Only update address and capabilities if they exist
            if !info.address.is_empty() {
                peer.info.address = info.address;
            }
            peer.info.capabilities.extend(info.capabilities);
            Ok(())
        } else {
            anyhow::bail!("Peer not found for update: {}", peer_id)
        }
    }

    /// Find a peer by its node identifier and optional public key
    pub fn find_peer(&self, id: &PeerId, public_key: Option<&str>) -> Option<PeerEntry> {
        let peer_id = Self::get_peer_id(public_key, &id.public_key);
        
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
    pub fn remove_peer(&self, id: &PeerId, public_key: Option<&str>) -> Result<()> {
        let peer_id = Self::get_peer_id(public_key, &id.public_key);
        
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