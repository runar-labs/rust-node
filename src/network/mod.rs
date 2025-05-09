// Network Transport Module
//
// This module provides network transport capabilities for the Runar system.
// It includes traits and implementations for network communication between nodes.

pub mod discovery;
pub mod transport;

pub use discovery::{
    DiscoveryListener, DiscoveryOptions, MemoryDiscovery, MulticastDiscovery, NodeDiscovery,
    NodeInfo,
};
pub use runar_common::types::{ActionMetadata, EventMetadata, ServiceMetadata};
pub use transport::{
    MessageHandler, NetworkMessage, NetworkMessageType, NetworkTransport, PeerEntry, PeerId,
    PeerRegistry, PeerStatus, QuicTransport, QuicTransportOptions, TransportFactory,
    TransportOptions,
};

// Implementation modules should be imported directly when needed:
// use runar_node::network::discovery::multicast_discovery::MulticastDiscovery;
// use runar_node::network::transport::quic_transport::{QuicTransport, QuicTransportOptions};
