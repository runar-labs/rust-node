use crate::p2p::crypto::PeerId;
use crate::services::abstract_service::{AbstractService, ServiceMetadata, ServiceState};
use crate::services::{RequestContext, ServiceRequest, ServiceResponse, ValueType};
use crate::util::logging::{debug_log, error_log, info_log, warn_log, Component};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// P2P Transport interface to decouple RemoteService from the specific P2P implementation
#[async_trait]
pub trait P2PTransport: Send + Sync {
    /// Send a request to a remote peer and wait for the response
    async fn send_request(
        &self,
        peer_id: PeerId,
        path: String,
        params: ValueType,
    ) -> Result<ServiceResponse>;

    /// Publish an event to a remote peer
    async fn publish_event(&self, peer_id: PeerId, topic: String, data: ValueType) -> Result<()>;

    /// Send a request to a remote peer with metadata and wait for the response
    async fn send_request_with_metadata(
        &self,
        peer_id: PeerId,
        path: String,
        params: ValueType,
        metadata: Option<HashMap<String, ValueType>>,
    ) -> Result<ServiceResponse>;

    /// Publish an event to a remote peer with metadata
    async fn publish_event_with_metadata(
        &self,
        peer_id: PeerId,
        topic: String,
        data: ValueType,
        metadata: Option<HashMap<String, ValueType>>,
    ) -> Result<()>;

    /// Get a reference to self as Any for downcasting
    fn as_any(&self) -> &dyn std::any::Any;
}

/// RemoteService represents a service that exists on a remote peer
/// All requests are forwarded to the remote peer via the P2P layer
pub struct RemoteService {
    /// The name of the service
    name: String,

    /// The path at which the service is available
    path: String,

    /// The ID of the peer hosting this service
    peer_id: PeerId,

    /// Available operations on this service
    operations: Vec<String>,

    /// Current state of the service
    state: Mutex<ServiceState>,

    /// Service uptime
    uptime: Instant,

    /// P2P Transport for sending messages to the peer
    p2p_transport: Arc<dyn P2PTransport>,
}

impl RemoteService {
    /// Create a new RemoteService
    pub fn new(
        name: String,
        path: String,
        peer_id: PeerId,
        operations: Vec<String>,
        p2p_transport: Arc<dyn P2PTransport>,
    ) -> Self {
        info_log(
            Component::Service,
            &format!(
                "Creating RemoteService: name={}, peer_id={:?}",
                name, peer_id
            ),
        );

        RemoteService {
            name,
            path,
            peer_id,
            operations,
            state: Mutex::new(ServiceState::Created),
            uptime: Instant::now(),
            p2p_transport,
        }
    }

    /// Get the peer ID of the remote peer hosting this service
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
}

#[async_trait]
impl AbstractService for RemoteService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        *self.state.lock().unwrap()
    }

    fn description(&self) -> &str {
        "Remote service proxy"
    }
    
    fn version(&self) -> &str {
        "1.0.0"
    }
    
    fn operations(&self) -> Vec<String> {
        self.operations.clone()
    }

    async fn init(&mut self, _ctx: &RequestContext) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Initialized;
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Running;
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        *self.state.lock().unwrap() = ServiceState::Stopped;
        Ok(())
    }

    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        debug_log(
            Component::Service,
            &format!(
                "RemoteService processing request: service={}, path={}",
                self.name, request.path
            ),
        );

        // Forward the request to the remote peer via P2P, including metadata if available
        if let Some(metadata) = request.metadata {
            self.p2p_transport.send_request_with_metadata(
                self.peer_id.clone(),
                request.path,
                request.params.unwrap_or(ValueType::Null),
                Some(metadata),
            ).await
        } else {
            self.p2p_transport.send_request(
                self.peer_id.clone(),
                request.path,
                request.params.unwrap_or(ValueType::Null),
            ).await
        }
    }
}
