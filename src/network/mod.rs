// Network Transport Module
//
// This module provides network transport capabilities for the Runar system.
// It includes traits and implementations for network communication between nodes.

pub mod transport;
pub mod discovery;

pub use transport::{
    NetworkTransport, NetworkMessage, NodeIdentifier, TransportOptions, 
    TransportFactory, NetworkMessageType, MessageHandler,
    PeerRegistry, PeerStatus, PeerEntry
};
pub use discovery::{NodeDiscovery, DiscoveryOptions, NodeInfo};

// Implementation modules should be imported directly when needed:
// use runar_node::network::discovery::multicast_discovery::MulticastDiscovery;
// use runar_node::network::transport::quic_transport::{QuicTransport, QuicTransportOptions}; 