use anyhow::{Result, anyhow};
use runar_common::logging::{Component, Logger};
use runar_common::types::{JsonValue, ValueType};
use runar_node::network::{NetworkConfig, NetworkMessage, NodeConfig, PeerInfo};
use runar_node::network::discovery::{DiscoveryOptions, MulticastDiscovery, NodeDiscovery, NodeInfo};
use runar_node::network::transport::{NetworkTransport, PeerId, QuicTransport, QuicTransportOptions};
use runar_node::routing::TopicPath;
use runar_node::services::{ActionContext, ActionHandler, ServiceAction, ServiceEvent, ServiceRegistry, ServiceResponse};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio::time::sleep;
use uuid::Uuid;

/// Tests for remote action invocation between nodes
///
/// These tests verify the functionality of remote action calls between two nodes
/// using the network layer with actual Node instances.
#[cfg(test)]
mod remote_action_tests {
    use super::*;

    /// Test for remote action calls between two nodes
    ///
    /// INTENTION: Create two Node instances with network enabled, they shuold  discover and connect to each other
    /// each node shouo dhave one math sercie with differente path.. so we can call it from each node and test
    /// the remove calls
    #[tokio::test]
    async fn test_remote_action_call() -> Result<()> {
        
        

        Ok(())
    }
} 