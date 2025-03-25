use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::{debug, error, info};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use serde_json::Value;
use crate::vmap;

use crate::db::SqliteDatabase;
use crate::node::NodeRequestHandlerImpl;
use crate::p2p::crypto::PeerId;
use crate::p2p::service::P2PRemoteServiceDelegate;
use crate::p2p::transport::{P2PMessage, P2PServiceInfo};
use crate::services::abstract_service::{ServiceMetadata, ServiceState};
use crate::services::remote::{P2PTransport, RemoteService};
use crate::services::{
    AbstractService, NodeRequestHandler, RequestContext, ServiceRequest, ServiceResponse,
    SubscriptionOptions, ValueType,
};
use runar_common::utils::logging::{debug_log, error_log, info_log, warn_log, Component};

/// A type representing a callback function for event subscribers
type EventCallback = Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>;

/// ServiceRegistry is responsible for registering, discovering, and managing services
/// It combines the functionality of the previous ServiceRegistry and ServiceManager
pub struct ServiceRegistry {
    /// The network ID
    network_id: String,

    /// The path of the registry
    path: String,

    /// The services managed by this registry
    services: RwLock<HashMap<String, Arc<dyn AbstractService>>>,

    /// The database connection (optional)
    db: Option<Arc<SqliteDatabase>>,

    /// The node handler for sending requests to other services
    node_handler: Option<Arc<dyn NodeRequestHandler + Send + Sync>>,

    /// Event subscribers with topic as the key
    event_subscribers: RwLock<HashMap<String, Vec<String>>>,

    /// Callback map for event subscribers
    event_callbacks: RwLock<HashMap<String, Vec<(String, EventCallback)>>>,

    /// Remote peer map - peer_id to list of services offered by that peer
    remote_peers: RwLock<HashMap<PeerId, Vec<String>>>,

    /// P2P delegate for communicating with remote peers
    p2p_delegate: RwLock<Option<Arc<P2PRemoteServiceDelegate>>>,

    /// Remote subscriptions map - peer_id to set of topics they are subscribed to
    remote_subscriptions: RwLock<HashMap<PeerId, HashSet<String>>>,

    /// Service info cache
    service_info_cache: RwLock<HashMap<String, P2PServiceInfo>>,

    /// Node handler lock for interior mutability
    node_handler_lock: RwLock<Option<Arc<dyn NodeRequestHandler + Send + Sync>>>,
}

// Implement Clone manually for ServiceRegistry
impl Clone for ServiceRegistry {
    fn clone(&self) -> Self {
        ServiceRegistry {
            network_id: self.network_id.clone(),
            path: self.path.clone(),
            services: RwLock::new(HashMap::new()),
            db: self.db.clone(),
            node_handler: self.node_handler.clone(),
            event_subscribers: RwLock::new(HashMap::new()),
            event_callbacks: RwLock::new(HashMap::new()),
            remote_peers: RwLock::new(HashMap::new()),
            p2p_delegate: RwLock::new(None),
            remote_subscriptions: RwLock::new(HashMap::new()),
            service_info_cache: RwLock::new(HashMap::new()),
            node_handler_lock: RwLock::new(None),
        }
    }
}

impl ServiceRegistry {
    /// Create a new registry with the given network ID
    pub fn new(network_id: &str) -> Self {
        let path = format!("internal/registry");

        ServiceRegistry {
            network_id: network_id.to_string(),
            path,
            services: RwLock::new(HashMap::new()),
            db: None,
            node_handler: None,
            event_subscribers: RwLock::new(HashMap::new()),
            event_callbacks: RwLock::new(HashMap::new()),
            remote_peers: RwLock::new(HashMap::new()),
            p2p_delegate: RwLock::new(None),
            remote_subscriptions: RwLock::new(HashMap::new()),
            service_info_cache: RwLock::new(HashMap::new()),
            node_handler_lock: RwLock::new(None),
        }
    }

    /// Create a new registry with a database connection
    pub fn new_with_db(network_id: &str, db: Arc<SqliteDatabase>) -> Self {
        let path = format!("internal/registry");

        ServiceRegistry {
            network_id: network_id.to_string(),
            path,
            services: RwLock::new(HashMap::new()),
            db: Some(db),
            node_handler: None,
            event_subscribers: RwLock::new(HashMap::new()),
            event_callbacks: RwLock::new(HashMap::new()),
            remote_peers: RwLock::new(HashMap::new()),
            p2p_delegate: RwLock::new(None),
            remote_subscriptions: RwLock::new(HashMap::new()),
            service_info_cache: RwLock::new(HashMap::new()),
            node_handler_lock: RwLock::new(None),
        }
    }

    /// Set the node handler
    pub fn set_node_handler(&mut self, node_handler: Arc<dyn NodeRequestHandler + Send + Sync>) {
        self.node_handler = Some(node_handler);
    }

    /// Set the node handler asynchronously (doesn't require mutable access)
    pub async fn set_node_handler_async(
        &self,
        node_handler: Arc<dyn NodeRequestHandler + Send + Sync>,
    ) {
        // Use the shared RwLock directly
        let mut node_handler_write = self.node_handler_lock.write().await;
        *node_handler_write = Some(node_handler);
    }

    /// Set the P2P transport to use for remote services
    pub async fn set_p2p_transport(&self, transport: Arc<dyn P2PTransport>) {
        debug_log(
            Component::Service,
            "Setting P2P transport for service registry",
        )
        .await;

        // Create a P2P delegate with the provided transport
        let p2p_delegate =
            P2PRemoteServiceDelegate::new(None, &self.network_id, self.db.clone().unwrap())
                .await
                .expect("Failed to create P2P delegate");

        // Store the delegate
        let p2p_delegate = Arc::new(p2p_delegate);
        let mut delegate_guard = self.p2p_delegate.write().await;
        *delegate_guard = Some(p2p_delegate.clone());

        // Register a message handler to process incoming messages
        let registry_clone = self.clone();
        p2p_delegate
            .register_message_handler(move |message_event| {
                let registry = registry_clone.clone();
                let peer_id = message_event.peer_id.clone();
                let message_bytes = message_event.message.clone().into_bytes();

                tokio::spawn(async move {
                    debug_log(
                        Component::Service,
                        &format!("Processing message from peer {:?}", peer_id),
                    )
                    .await;

                    if let Err(e) = registry.handle_message(peer_id, &message_bytes).await {
                        error_log(
                            Component::Service,
                            &format!("Error handling message: {}", e),
                        )
                        .await;
                    }
                });
            })
            .await;

        // We need to create an owned copy to call start() on it
        // since we can't get a mutable reference to it inside the Arc
        let p2p_delegate_clone = p2p_delegate.clone();
        let p2p_delegate_data = p2p_delegate_clone.as_ref().clone();
        // Use tokio::spawn to create a new task that owns the delegaate
        tokio::spawn(async move {
            let mut delegate_obj = p2p_delegate_data;
            if let Err(e) = delegate_obj.start().await {
                error_log(
                    Component::Service,
                    &format!("Failed to start P2P delegate: {}", e),
                )
                .await;
            }
        });
    }

    /// Register remote services from a peer
    pub async fn register_remote_services(
        &self,
        peer_id: PeerId,
        service_infos: Vec<P2PServiceInfo>,
    ) -> Result<()> {
        debug_log(
            Component::ServiceRegistry,
            &format!(
                "Registering {} remote services from peer {:?}",
                service_infos.len(),
                peer_id
            ),
        )
        .await;

        let mut remote_peers = self.remote_peers.write().await;
        let mut peer_services = Vec::new();

        for service_info in service_infos {
            debug_log(
                Component::ServiceRegistry,
                &format!(
                    "Registering remote service: name={}, path={}, operations={:?}",
                    service_info.name, service_info.path, service_info.operations
                ),
            )
            .await;

            peer_services.push(service_info.name.clone());

            // Create remote service proxy and store service info
            let mut service_info_cache = self.service_info_cache.write().await;
            let cache_key = format!("{}:{}", peer_id, service_info.name);
            service_info_cache.insert(cache_key, service_info.clone());
        }

        // Store all services for this peer
        debug_log(
            Component::ServiceRegistry,
            &format!(
                "Storing {} services for peer {:?}: {:?}",
                peer_services.len(),
                peer_id,
                peer_services
            ),
        )
        .await;

        remote_peers.insert(peer_id.clone(), peer_services);
        Ok(())
    }

    /// Remove all services from a remote peer when it disconnects
    pub async fn remove_remote_services(&self, peer_id: &PeerId) -> Result<()> {
        info_log(
            Component::Registry,
            &format!("Removing remote services from peer {:?}", peer_id),
        )
        .await;

        let mut remote_peers = self.remote_peers.write().await;

        // Get the list of services for this peer
        if let Some(peer_services) = remote_peers.remove(peer_id) {
            let mut services = self.services.write().await;

            // Remove each service
            for service_name in &peer_services {
                services.remove(service_name);
                debug_log(
                    Component::Registry,
                    &format!(
                        "Removed remote service '{}' from peer {:?}",
                        service_name, peer_id
                    ),
                )
                .await;
            }
        }

        // Also remove any remote subscriptions from this peer
        self.remove_remote_subscriptions(peer_id).await;

        Ok(())
    }

    /// Register a remote subscription from a peer for a specific topic
    pub async fn register_remote_subscription(&self, peer_id: PeerId, topic: String) -> Result<()> {
        debug_log(
            Component::Registry,
            &format!(
                "Registering remote subscription for peer {:?} on topic '{}'",
                peer_id, topic
            ),
        )
        .await;

        let mut remote_subs = self.remote_subscriptions.write().await;

        // Create entry for this peer if it doesn't exist
        if !remote_subs.contains_key(&peer_id) {
            remote_subs.insert(peer_id.clone(), HashSet::new());
        }

        // Add the topic to this peer's subscriptions
        if let Some(topics) = remote_subs.get_mut(&peer_id) {
            topics.insert(topic.clone());
        }

        Ok(())
    }

    /// Unregister a remote subscription from a peer for a specific topic
    pub async fn unregister_remote_subscription(
        &self,
        peer_id: &PeerId,
        topic: &str,
    ) -> Result<()> {
        debug_log(
            Component::Registry,
            &format!(
                "Unregistering remote subscription for peer {:?} on topic '{}'",
                peer_id, topic
            ),
        )
        .await;

        let mut remote_subs = self.remote_subscriptions.write().await;

        if let Some(topics) = remote_subs.get_mut(peer_id) {
            topics.remove(topic);
        }

        Ok(())
    }

    /// Unsubscribe from a topic
    pub async fn unsubscribe(
        &self,
        service_name: &str,
        topic: &str,
        subscription_id: Option<&str>,
    ) -> Result<()> {
        debug_log(
            Component::Service,
            &format!("ServiceRegistry::unsubscribe - Topic: {}", topic),
        )
        .await;

        // Track if all subscriptions for this topic were removed
        let mut all_subscriptions_removed = false;

        // If a subscription ID is provided, remove only that specific subscription
        if let Some(id) = subscription_id {
            let mut callbacks = self.event_callbacks.write().await;
            if let Some(callbacks_for_service) = callbacks.get_mut(service_name) {
                callbacks_for_service.retain(|(callback_topic, _)| callback_topic != topic);

                // If no more callbacks for this service, remove the service entry
                if callbacks_for_service.is_empty() {
                    callbacks.remove(service_name);

                    // Also remove the service from the subscribers list
                    let mut subscribers = self.event_subscribers.write().await;
                    if let Some(subscribers_for_topic) = subscribers.get_mut(topic) {
                        subscribers_for_topic.retain(|s| s != service_name);

                        // If no more subscribers, remove the topic entry
                        if subscribers_for_topic.is_empty() {
                            subscribers.remove(topic);
                            all_subscriptions_removed = true;
                        }
                    }
                }
            }
        } else {
            // Remove all subscriptions for this service and topic
            let mut callbacks = self.event_callbacks.write().await;
            if let Some(callbacks_for_service) = callbacks.get_mut(service_name) {
                callbacks_for_service.retain(|(callback_topic, _)| callback_topic != topic);

                // If no more callbacks, remove the service entry
                if callbacks_for_service.is_empty() {
                    callbacks.remove(service_name);
                }
            }

            // Remove the service from the subscribers list
            let mut subscribers = self.event_subscribers.write().await;
            if let Some(subscribers_for_topic) = subscribers.get_mut(topic) {
                subscribers_for_topic.retain(|s| s != service_name);

                // If no more subscribers, remove the topic entry
                if subscribers_for_topic.is_empty() {
                    subscribers.remove(topic);
                    all_subscriptions_removed = true;
                }
            }
        }

        // If this is not an internal topic and all subscriptions were removed,
        // propagate the unsubscription to peers
        if all_subscriptions_removed && !topic.starts_with("internal/") {
            // TODO: In the future, we should implement a proper propagate_unsubscription_to_peers method
            // For now, we log that we would propagate this
            debug_log(
                Component::Registry,
                &format!(
                    "Would propagate unsubscription for topic '{}' to peers",
                    topic
                ),
            )
            .await;
        }

        debug_log(
            Component::Service,
            &format!("ServiceRegistry::unsubscribe - Done: {}", topic),
        )
        .await;

        Ok(())
    }

    /// Remove all remote subscriptions for a peer (when it disconnects)
    pub async fn remove_remote_subscriptions(&self, peer_id: &PeerId) {
        debug_log(
            Component::Registry,
            &format!("Removing all remote subscriptions for peer {:?}", peer_id),
        )
        .await;

        let mut remote_subs = self.remote_subscriptions.write().await;
        remote_subs.remove(peer_id);
    }

    /// Register a service with the registry
    pub async fn register_service(&self, service: Arc<dyn AbstractService>) -> Result<()> {
        let mut services = self.services.write().await;
        let service_name = service.name().to_string();

        info_log(
            Component::Registry,
            &format!(
                "Registering service: {} at {}",
                service_name,
                service.path()
            ),
        )
        .await;

        if services.contains_key(&service_name) {
            return Err(anyhow!("Service '{}' is already registered", service_name));
        }

        services.insert(service_name.clone(), service.clone());

        // After registering the service locally, schedule a debounced update
        // instead of immediately propagating to connected peers
        drop(services); // Release the lock before async operation

        // Use tokio::spawn to avoid blocking the caller
        let registry_clone = self.clone();
        tokio::spawn(async move {
            // Simple debounce mechanism: delay for 500ms
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Propagate all services at once
            if let Err(e) = registry_clone.propagate_all_services_to_peers().await {
                error_log(
                    Component::Registry,
                    &format!("Failed to propagate services: {}", e),
                )
                .await;
            }
        });

        Ok(())
    }

    /// Get a service by name
    pub async fn get_service(&self, name: impl AsRef<str>) -> Option<Arc<dyn AbstractService>> {
        let services = self.services.read().await;
        services.get(name.as_ref()).cloned()
    }

    /// Get a service by path
    pub async fn get_service_by_path(&self, path: &str) -> Option<Arc<dyn AbstractService>> {
        let services = self.services.read().await;

        // Check for direct matches first
        for service in services.values() {
            if service.path() == path {
                debug_log(
                    Component::Registry,
                    &format!(
                        "Found service match by direct path: {} -> {}",
                        path,
                        service.name()
                    ),
                )
                .await;
                return Some(service.clone());
            }
        }

        // Check for matches with the network ID prefix
        let full_path = if !path.starts_with(&self.network_id) {
            format!("{}/{}", self.network_id, path)
        } else {
            path.to_string()
        };

        for service in services.values() {
            if service.path() == full_path {
                debug_log(
                    Component::Registry,
                    &format!(
                        "Found service match by prefixed path: {} -> {}",
                        path,
                        service.name()
                    ),
                )
                .await;
                return Some(service.clone());
            }
            // Also check if the path is just the service name
            if service.name() == path {
                debug_log(
                    Component::Registry,
                    &format!(
                        "Found service match by name: {} -> {}",
                        path,
                        service.name()
                    ),
                )
                .await;
                return Some(service.clone());
            }
        }

        // Check if the path begins with a service path
        for service in services.values() {
            let service_path = service.path();
            if path.starts_with(&format!("{}/", service_path)) {
                debug_log(
                    Component::Registry,
                    &format!(
                        "Found service match by path prefix: {} -> {}",
                        path,
                        service.name()
                    ),
                )
                .await;
                return Some(service.clone());
            }
        }

        None
    }

    /// List all services
    pub async fn list_services(&self) -> Vec<String> {
        let services = self.services.read().await;
        services.keys().cloned().collect()
    }

    /// Get all services
    pub async fn get_all_services(&self) -> Vec<Arc<dyn AbstractService>> {
        let services = self.services.read().await;
        services.values().cloned().collect()
    }

    /// Handle a service request
    pub async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        debug!("Processing request: {:?}", request);
        
        // Get the service
        let service = self.get_service_by_path(&request.path).await;
        
        // If service exists, delegate the request
        match service {
            Some(service) => {
                debug!("Service found: {}", service.name());
                service.handle_request(request).await
            }
            None => {
                // Service not found, check if this is a remote service
                debug!("Local service not found, checking remote services");
                
                // Parse path to get service name
                let path_parts: Vec<&str> = request.path.split('/').collect();
                let service_name = path_parts.last().unwrap_or(&"");
                
                // Check if any remote peers have this service
                let mut remote_peers = Vec::new();
                {
                    let remote_peers_map = self.remote_peers.read().await;
                    for (peer_id, services) in remote_peers_map.iter() {
                        if services.contains(&service_name.to_string()) {
                            remote_peers.push(peer_id.clone());
                        }
                    }
                }
                
                if !remote_peers.is_empty() {
                    debug!("Found remote peer with service: {}", service_name);
                    
                    // Get the first remote peer with this service
                    let peer_id = remote_peers[0].clone();
                    
                    // Get the P2P transport
                    if let Some(p2p) = self.get_p2p_transport().await {
                        // Forward the request to the remote peer
                        debug!("Forwarding request to remote peer: {:?}", peer_id);
                        
                        // Extract params
                        let params = request.data.unwrap_or(vmap! {});
                        
                        // Send the request
                        match p2p.send_request(peer_id, request.path, params).await {
                            Ok(response) => Ok(response),
                            Err(e) => {
                                error!("Error forwarding request to remote peer: {}", e);
                                Ok(ServiceResponse::error(format!(
                                    "Error forwarding request to remote peer: {}",
                                    e
                                )))
                            }
                        }
                    } else {
                        error!("P2P transport not available");
                        Ok(ServiceResponse::error("P2P transport not available"))
                    }
                } else {
                    debug!("Service not found locally or remotely: {}", request.path);
                    Ok(ServiceResponse::error(format!("Service not found: {}", request.path)))
                }
            }
        }
    }

    /// Get the database connection
    pub fn get_db(&self) -> Option<Arc<SqliteDatabase>> {
        self.db.clone()
    }

    /// Propagate an event to all peers that have subscribed to the topic
    async fn propagate_event_to_peers(&self, topic: &str, data: &ValueType) -> Result<()> {
        // Get remote subscriptions for this topic
        let remote_subscribers = {
            let subscriptions = self.remote_subscriptions.read().await;
            let mut subscribers = Vec::new();

            for (peer_id, topics) in subscriptions.iter() {
                if topics.contains(&topic.to_string()) {
                    subscribers.push(peer_id.clone());
                }
            }

            subscribers
        };

        info_log(
            Component::Registry,
            &format!(
                "Propagating event for topic {} to {} remote peers",
                topic,
                remote_subscribers.len()
            ),
        )
        .await;

        if remote_subscribers.is_empty() {
            debug_log(
                Component::Registry,
                &format!("No remote peers subscribed to topic {}", topic),
            )
            .await;
            return Ok(());
        }

        // Get the P2P delegate to send events
        let delegate_guard = self.p2p_delegate.read().await;
        if let Some(delegate) = &*delegate_guard {
            for peer_id in remote_subscribers {
                info_log(
                    Component::Registry,
                    &format!("Publishing event for topic {} to peer {:?}", topic, peer_id),
                )
                .await;

                // Publish the event to the peer
                match delegate
                    .publish_event(peer_id.clone(), topic.to_string(), data.clone())
                    .await
                {
                    Ok(_) => {
                        info_log(
                            Component::Registry,
                            &format!(
                                "Successfully published event for topic {} to peer {:?}",
                                topic, peer_id
                            ),
                        )
                        .await;
                    }
                    Err(e) => {
                        error_log(
                            Component::Registry,
                            &format!(
                                "Failed to publish event for topic {} to peer {:?}: {}",
                                topic, peer_id, e
                            ),
                        )
                        .await;
                    }
                }
            }
        } else {
            warn_log(
                Component::Registry,
                "No P2P delegate available to propagate events",
            )
            .await;
        }

        Ok(())
    }

    /// Subscribe to events on a topic
    pub async fn subscribe(
        &self,
        service_name: String,
        topic: String,
        callback: EventCallback,
    ) -> Result<String> {
        // Default options
        let mut options = SubscriptionOptions::new();
        options.ensure_id();

        self.subscribe_with_options(service_name, topic, callback, options)
            .await
    }

    /// Subscribe to events on a topic with options
    pub async fn subscribe_with_options(
        &self,
        service_name: String,
        topic: String,
        callback: EventCallback,
        options: SubscriptionOptions,
    ) -> Result<String> {
        debug_log(
            Component::Service,
            &format!("ServiceRegistry::subscribe - Topic: {}", topic),
        )
        .await;

        // Make sure we have an ID
        let mut options = options;
        options.ensure_id();
        let subscription_id = options.id.clone().unwrap();

        // Store the subscription
        {
            let mut subscribers = self.event_subscribers.write().await;
            let subscribers_for_topic = subscribers.entry(topic.clone()).or_insert_with(Vec::new);
            if !subscribers_for_topic.contains(&service_name) {
                subscribers_for_topic.push(service_name.clone());
            }
        }

        // Store the callback with the subscription ID
        {
            let mut callbacks = self.event_callbacks.write().await;
            let callbacks_for_service = callbacks
                .entry(service_name.clone())
                .or_insert_with(Vec::new);

            // Wrap the callback in another closure that handles max_triggers and once
            let wrapper_callback: EventCallback = if options.max_triggers.is_some()
                || options.ttl.is_some()
            {
                let subscription_id_clone = subscription_id.clone();
                let topic_clone = topic.clone();
                let service_name_clone = service_name.clone();
                let registry = self.clone();
                let max_triggers = options.max_triggers;
                let trigger_count = Arc::new(RwLock::new(0));

                Box::new(move |payload: ValueType| {
                    let subscription_id_clone = subscription_id_clone.clone();
                    let topic_clone = topic_clone.clone();
                    let service_name_clone = service_name_clone.clone();
                    let registry = registry.clone();
                    let trigger_count = trigger_count.clone();
                    let callback = &callback;

                    // Call the original callback
                    let result = callback(payload);

                    // If we have max_triggers set, check if we should unsubscribe
                    if let Some(max_triggers) = max_triggers {
                        tokio::spawn(async move {
                            let mut count = trigger_count.write().await;
                            *count += 1;

                            if *count >= max_triggers {
                                // Unsubscribe
                                if let Err(e) = registry
                                    .unsubscribe(
                                        &service_name_clone,
                                        &topic_clone,
                                        Some(&subscription_id_clone),
                                    )
                                    .await
                                {
                                    error_log(
                                        Component::Service,
                                        &format!("Error unsubscribing after max triggers: {}", e),
                                    )
                                    .await;
                                }
                            }
                        });
                    }

                    result
                })
            } else {
                callback
            };

            // Store the callback for this service with the topic as the key
            callbacks_for_service.push((topic.clone(), wrapper_callback));
        }

        // If TTL is set, schedule a task to unsubscribe after the TTL
        if let Some(ttl) = options.ttl {
            let subscription_id_clone = subscription_id.clone();
            let topic_clone = topic.clone();
            let service_name_clone = service_name.clone();
            let registry = self.clone();

            tokio::spawn(async move {
                tokio::time::sleep(ttl).await;

                // Unsubscribe after TTL
                if let Err(e) = registry
                    .unsubscribe(
                        &service_name_clone,
                        &topic_clone,
                        Some(&subscription_id_clone),
                    )
                    .await
                {
                    error_log(
                        Component::Service,
                        &format!("Error unsubscribing after TTL: {}", e),
                    );
                }
            });
        }

        // If this is not an internal topic, propagate to peers
        // Internal topics (starting with "internal/") are kept local to this node only
        if !topic.starts_with("internal/") {
            self.propagate_subscription_to_peers(&topic).await?;
        }

        debug_log(
            Component::Service,
            &format!("ServiceRegistry::subscribe - Done: {}", topic),
        );

        // Return the subscription ID
        Ok(subscription_id)
    }

    /// Propagate a subscription to all connected peers
    async fn propagate_subscription_to_peers(&self, topic: &str) -> Result<usize> {
        // We no longer need a separate mechanism for subscription propagation
        // since service info contains all operations and events
        debug_log(
            Component::Registry,
            &format!(
                "Subscription propagation for topic '{}' is handled via service propagation",
                topic
            ),
        )
        .await;

        // Just delegate to propagate_all_services_to_peers
        self.propagate_all_services_to_peers().await?;

        // Return 1 to indicate success (we don't know the exact count of propagated peers)
        Ok(1)
    }

    /// Propagate all services to all connected peers at once
    async fn propagate_all_services_to_peers(&self) -> Result<()> {
        // Get the P2P transport
        let p2p_transport = {
            let transport = self.p2p_delegate.read().await;
            match transport.clone() {
                Some(t) => t,
                None => {
                    // If no P2P transport is set, this is fine - we just don't propagate service info
                    debug_log(
                        Component::Registry,
                        "No P2P transport set, skipping service info propagation",
                    )
                    .await;
                    return Ok(());
                }
            }
        };

        // Get all connected peers
        let remote_peers = self.remote_peers.read().await;
        if remote_peers.is_empty() {
            debug_log(
                Component::Registry,
                "No connected peers, skipping service info propagation",
            )
            .await;
            return Ok(());
        }

        // Get all local services
        let all_services = self.get_all_services().await;
        if all_services.is_empty() {
            debug_log(Component::Registry, "No local services to propagate").await;
            return Ok(());
        }

        // Create a list of P2PServiceInfo objects from our services
        let service_infos: Vec<P2PServiceInfo> = all_services
            .iter()
            .map(|service| P2PServiceInfo {
                name: service.name().to_string(),
                path: service.path().to_string(),
                operations: service.metadata().operations,
            })
            .collect();

        info_log(
            Component::Registry,
            &format!(
                "Propagating {} services to {} peers",
                service_infos.len(),
                remote_peers.len()
            ),
        )
        .await;

        let mut success_count = 0;

        // Send a single updateNode message to each peer with all services
        for peer_id in remote_peers.keys() {
            // We need to cast the p2p_transport to the concrete type
            if let Some(p2p_transport_concrete) = p2p_transport
                .as_any()
                .downcast_ref::<crate::p2p::transport::P2PTransport>(
            ) {
                // Create a ServiceDiscovery message with all services
                let params = serde_json::json!({
                    "action": "internal/registry/updateNode",
                    "services": service_infos,
                });

                // Send the updateNode request to the peer
                match p2p_transport_concrete
                    .share_services(peer_id.clone(), service_infos.clone())
                    .await
                {
                    Ok(_) => {
                        debug_log(
                            Component::Registry,
                            &format!("Successfully shared all services with peer {:?}", peer_id),
                        )
                        .await;
                        success_count += 1;
                    }
                    Err(e) => {
                        error_log(
                            Component::Registry,
                            &format!("Failed to share services with peer {:?}: {}", peer_id, e),
                        )
                        .await;
                    }
                }
            } else {
                error_log(
                    Component::Registry,
                    "Failed to downcast P2P transport to concrete type",
                )
                .await;
            }
        }

        if success_count > 0 {
            info_log(
                Component::Registry,
                &format!(
                    "Successfully shared all services with {} out of {} peers",
                    success_count,
                    remote_peers.len()
                ),
            )
            .await;
        } else {
            warn_log(
                Component::Registry,
                "Failed to share services with any peer",
            )
            .await;
        }

        Ok(())
    }

    /// Propagate service information to all connected peers
    /// This method is kept for backward compatibility but delegates to propagate_all_services_to_peers
    async fn propagate_service_info_to_peers(
        &self,
        _service: Arc<dyn AbstractService>,
    ) -> Result<()> {
        // Simply delegate to the batch method
        self.propagate_all_services_to_peers().await
    }

    pub async fn handle_message(&self, peer_id: PeerId, message: &[u8]) -> Result<()> {
        debug_log(
            Component::Service,
            &format!("Handling message from peer {:?}", peer_id),
        )
        .await;

        // Try to parse as a P2PMessage
        if let Ok(message_str) = String::from_utf8(message.to_vec()) {
            debug_log(
                Component::Service,
                &format!("Message content: {}", message_str),
            )
            .await;

            if let Ok(message) = serde_json::from_str::<P2PMessage>(&message_str) {
                match message {
                    P2PMessage::Request {
                        request_id,
                        path,
                        params,
                    } => {
                        debug_log(
                            Component::Service,
                            &format!("Processing request {} for path {}", request_id, path),
                        )
                        .await;
                        self.process_remote_request(peer_id, request_id, &path, params)
                            .await?;
                    }
                    P2PMessage::Event { topic, data } => {
                        debug_log(
                            Component::Service,
                            &format!("Processing event for topic {}", topic),
                        )
                        .await;
                        self.handle_event(&topic, &data).await;
                    }
                    P2PMessage::ServiceDiscovery { services } => {
                        debug_log(
                            Component::Service,
                            &format!(
                                "Processing service discovery with {} services from peer {:?}",
                                services.len(),
                                peer_id
                            ),
                        )
                        .await;

                        // Register each service from the remote peer
                        for service_info in services {
                            debug_log(
                                Component::Service,
                                &format!(
                                    "Registering remote service '{}' from peer {:?}",
                                    service_info.name, peer_id
                                ),
                            )
                            .await;

                            // Register the remote service in our registry
                            // Clone peer_id before passing it to avoid moving it
                            self.register_remote_service(peer_id.clone(), service_info)
                                .await?;
                        }
                        return Ok(());
                    }
                    _ => {
                        debug_log(
                            Component::Service,
                            "Message is not a request or event, ignoring",
                        )
                        .await;
                    }
                }
            } else {
                debug_log(
                    Component::Service,
                    "Failed to parse message as P2PMessage, trying alternative formats",
                )
                .await;

                // Try to parse as a raw JSON that might be an event
                if let Ok(json_value) = serde_json::from_str::<serde_json::Value>(&message_str) {
                    if let (Some(topic), Some(data)) =
                        (json_value.get("topic"), json_value.get("data"))
                    {
                        if let Some(topic_str) = topic.as_str() {
                            debug_log(
                                Component::Service,
                                &format!("Detected raw event message for topic {}", topic_str),
                            )
                            .await;

                            // Convert data to ValueType
                            if let Ok(data_value) = serde_json::from_value(data.clone()) {
                                debug_log(
                                    Component::Service,
                                    &format!("Processing raw event for topic {}", topic_str),
                                )
                                .await;
                                self.handle_event(topic_str, &data_value).await;
                                return Ok(());
                            }
                        }
                    }
                }
            }
        } else {
            warn_log(
                Component::Service,
                &format!("Unhandled message from peer {:?}", peer_id),
            )
            .await;
        }

        Ok(())
    }

    async fn process_remote_request(
        &self,
        peer_id: PeerId,
        request_id: String,
        path: &str,
        params: ValueType,
    ) -> Result<()> {
        // Implementation of remote request handling
        // Get the service from the path
        let service = self.get_service_by_path(path).await;
        if let Some(service) = service {
            // Create a request context
            let source = format!("remote:{}:{}", base64::encode(peer_id.0), path);

            // Create a NodeRequestHandlerImpl using a clone of this ServiceRegistry
            let registry_arc = Arc::new(self.clone());
            let node_handler = Arc::new(NodeRequestHandlerImpl::new(registry_arc));

            let context = Arc::new(RequestContext::new(
                source,
                vmap! {},
                node_handler,
            ));

            // Create a service request
            let request = ServiceRequest {
                request_id: Some(request_id.clone()),
                path: path.to_string(),
                operation: path.split('/').last().unwrap_or("").to_string(),
                params: Some(params),
                request_context: context,
            };

            // Process the request
            match service.handle_request(request).await {
                Ok(response) => {
                    // Send the response back to the peer
                    let response_message = P2PMessage::Response {
                        request_id,
                        response,
                    };

                    // Get the P2P delegate
                    let delegate_guard = self.p2p_delegate.read().await;
                    if let Some(delegate) = &*delegate_guard {
                        // Convert to JSON string
                        let message_str = serde_json::to_string(&response_message)?;

                        // Send the response
                        delegate.send_message(peer_id, message_str).await?;
                    } else {
                        return Err(anyhow!("P2P delegate not available"));
                    }
                }
                Err(e) => {
                    error_log(
                        Component::Service,
                        &format!("Error processing remote request: {}", e),
                    )
                    .await;

                    // Send an error response
                    let error_response = ServiceResponse::error(format!("Error: {}", e));
                    let response_message = P2PMessage::Response {
                        request_id,
                        response: error_response,
                    };

                    // Get the P2P delegate
                    let delegate_guard = self.p2p_delegate.read().await;
                    if let Some(delegate) = &*delegate_guard {
                        // Convert to JSON string
                        let message_str = serde_json::to_string(&response_message)?;

                        // Send the response
                        delegate.send_message(peer_id, message_str).await?;
                    } else {
                        return Err(anyhow!("P2P delegate not available"));
                    }
                }
            }
        } else {
            // Service not found
            let error_response = ServiceResponse::error(format!("Service not found: {}", path));
            let response_message = P2PMessage::Response {
                request_id,
                response: error_response,
            };

            // Get the P2P delegate
            let delegate_guard = self.p2p_delegate.read().await;
            if let Some(delegate) = &*delegate_guard {
                // Convert to JSON string
                let message_str = serde_json::to_string(&response_message)?;

                // Send the response
                delegate.send_message(peer_id, message_str).await?;
            } else {
                return Err(anyhow!("P2P delegate not available"));
            }
        }

        Ok(())
    }

    async fn handle_event(&self, topic: &str, data: &ValueType) {
        info_log(
            Component::Registry,
            &format!("Handling event for topic {}", topic),
        )
        .await;

        // DIAGNOSTIC: Log entry into handle_event
        println!("DIAG-REGISTRY: handle_event called for topic: {}", topic);
        println!("DIAG-REGISTRY: event data: {:?}", data);

        // First, get the list of subscribers for this topic
        let subscribers_for_direct_match = {
            let subscribers_guard = self.event_subscribers.read().await;

            // DIAGNOSTIC: Log the current subscribers map in more detail
            println!("DIAG-REGISTRY: Current event_subscribers map:");
            for (topic_key, service_list) in subscribers_guard.iter() {
                println!(
                    "DIAG-REGISTRY:   Topic '{}' has subscribers: {:?}",
                    topic_key, service_list
                );
            }

            if let Some(subs) = subscribers_guard.get(topic) {
                // DIAGNOSTIC: Log if we found subscribers for this topic
                println!(
                    "DIAG-REGISTRY: Found {} direct subscribers for topic {}: {:?}",
                    subs.len(),
                    topic,
                    subs
                );
                subs.clone()
            } else {
                println!(
                    "DIAG-REGISTRY: No direct subscribers found for topic {}",
                    topic
                );
                Vec::new()
            }
        };

        // Log the event_callbacks map to help diagnose the issue
        {
            let callbacks_guard = self.event_callbacks.read().await;
            println!("DIAG-REGISTRY: Current event_callbacks map:");
            for (service_name, callbacks) in callbacks_guard.iter() {
                println!("DIAG-REGISTRY:   Service '{}' has callbacks:", service_name);
                for (callback_topic, _) in callbacks {
                    println!("DIAG-REGISTRY:     Topic: '{}'", callback_topic);
                }
            }
        }

        // Process direct match subscribers
        let mut callback_count = 0;
        for service_name in &subscribers_for_direct_match {
            // DIAGNOSTIC: Log which service we're executing callbacks for
            println!(
                "DIAG-REGISTRY: Executing callbacks for service {} on topic {}",
                service_name, topic
            );

            // Execute callbacks that match this topic
            let cb_executed = self
                .execute_callback_for_topic(service_name, topic, topic, data)
                .await;
            if cb_executed {
                callback_count += 1;
                println!(
                    "DIAG-REGISTRY: Successfully executed callback for service {} on topic {}",
                    service_name, topic
                );
            } else {
                println!(
                    "DIAG-REGISTRY: No callback executed for service {} on topic {}",
                    service_name, topic
                );
            }
        }

        // Find wildcard subscribers
        let wildcard_matches = {
            let subscribers_guard = self.event_subscribers.read().await;
            let mut matches = Vec::new();

            for (pattern, service_names) in subscribers_guard.iter() {
                if pattern.ends_with("/*") {
                    let prefix = &pattern[0..pattern.len() - 1]; // Remove the '*'
                    if topic.starts_with(prefix) {
                        // For each matching pattern, collect all (pattern, service_name) pairs
                        for service_name in service_names {
                            matches.push((pattern.clone(), service_name.clone()));
                        }
                    }
                }
            }

            matches
        };

        // Process wildcard matches
        for (pattern, service_name) in wildcard_matches {
            // Execute callbacks for this wildcard match
            let cb_executed = self
                .execute_callback_for_topic(&service_name, &pattern, topic, data)
                .await;
            if cb_executed {
                callback_count += 1;
            }
        }

        info_log(
            Component::Registry,
            &format!("Executed {} callbacks for topic {}", callback_count, topic),
        )
        .await;

        // Propagate to remote peers that are subscribed to this topic
        if let Err(e) = self.propagate_event_to_peers(topic, data).await {
            error_log(
                Component::Registry,
                &format!("Error propagating event to peers: {}", e),
            )
            .await;
        }
    }

    // Helper method to execute a callback for a specific topic
    async fn execute_callback_for_topic(
        &self,
        service_name: &str,
        callback_topic: &str,
        actual_topic: &str,
        data: &ValueType,
    ) -> bool {
        println!("DIAG-EXEC: Executing callback for service '{}', callback_topic '{}', actual_topic '{}'",
            service_name, callback_topic, actual_topic);

        // Get read lock on event_callbacks
        let event_callbacks = self.event_callbacks.read().await;

        // Print the current callbacks map for debugging
        println!("DIAG-EXEC: Current callbacks map:");
        for (service, callbacks) in event_callbacks.iter() {
            println!(
                "DIAG-EXEC:   Service '{}' has {} callbacks:",
                service,
                callbacks.len()
            );
            for (topic, _) in callbacks {
                println!("DIAG-EXEC:     Topic: '{}'", topic);
            }
        }

        // Check for callbacks for this service
        let mut callback_executed = false;
        if let Some(service_callbacks) = event_callbacks.get(service_name) {
            println!(
                "DIAG-EXEC: Found service '{}' with {} callbacks",
                service_name,
                service_callbacks.len()
            );

            // Check each callback to see if we have one for the specified topic
            for (topic, callback) in service_callbacks {
                println!(
                    "DIAG-EXEC: Checking if topic '{}' matches callback_topic '{}'",
                    topic, callback_topic
                );

                if topic == callback_topic {
                    println!("DIAG-EXEC: Topic match found! Executing callback");

                    // Execute the callback
                    match callback(data.clone()) {
                        Ok(_) => {
                            println!("DIAG-EXEC: Callback executed successfully");
                            callback_executed = true;
                        }
                        Err(e) => {
                            println!("DIAG-EXEC: Callback execution failed: {}", e);

                            // Log the error with additional context
                            if callback_topic.contains('*') {
                                error_log(
                                    Component::Registry,
                                    &format!(
                                        "Error executing callback for service '{}' on topic '{}' (matched wildcard '{}'): {}",
                                        service_name, actual_topic, callback_topic, e
                                    ),
                                ).await;
                            } else {
                                error_log(
                                    Component::Registry,
                                    &format!(
                                        "Error executing callback for service '{}' on topic '{}': {}",
                                        service_name, actual_topic, e
                                    ),
                                ).await;
                            }
                        }
                    }
                }
            }

            if !callback_executed {
                println!(
                    "DIAG-EXEC: No matching callback found for service '{}' and topic '{}'",
                    service_name, callback_topic
                );
            }
        } else {
            println!(
                "DIAG-EXEC: Service '{}' not found in callbacks map",
                service_name
            );
            println!(
                "DIAG-EXEC: Available services: {:?}",
                event_callbacks.keys().collect::<Vec<_>>()
            );
        }

        callback_executed
    }

    // Fix the type mismatch in get_p2p_transport
    async fn get_p2p_transport(&self) -> Option<Arc<dyn P2PTransport>> {
        self.p2p_delegate.read().await.as_ref().map(|delegate| {
            let transport: Arc<dyn P2PTransport> = delegate.clone();
            transport
        })
    }

    /// Get all callbacks for a topic
    pub async fn get_callbacks_for_topic(&self, topic: &str) -> Vec<(String, EventCallback)> {
        let callbacks = self.event_callbacks.read().await;
        let mut result = Vec::new();

        // Process callbacks with matching topics
        self.collect_matching_callbacks(&callbacks, topic, &mut result)
            .await;

        // Process wildcard patterns
        for (pattern, _) in callbacks.iter() {
            if pattern.ends_with("/*") {
                let prefix = &pattern[0..pattern.len() - 1]; // Remove the '*'
                if topic.starts_with(prefix) {
                    self.collect_matching_callbacks(&callbacks, pattern, &mut result)
                        .await;
                }
            }
        }

        result
    }

    // Helper method to collect callbacks
    async fn collect_matching_callbacks(
        &self,
        callbacks: &HashMap<String, Vec<(String, EventCallback)>>,
        pattern: &str,
        result: &mut Vec<(String, EventCallback)>,
    ) {
        if let Some(callbacks_for_pattern) = callbacks.get(pattern) {
            for (callback_id, _) in callbacks_for_pattern {
                // Create a new callback that will look up the actual callback at runtime
                let callback_id_clone = callback_id.clone();
                let pattern_clone = pattern.to_string();
                let registry = self.clone();

                let new_callback: EventCallback = Box::new(move |value| {
                    // This is not ideal in terms of performance, but it solves the lifetime issue
                    match futures::executor::block_on(registry.call_callback(
                        &pattern_clone,
                        &callback_id_clone,
                        value.clone(),
                    )) {
                        Ok(()) => Ok(()),
                        Err(e) => Err(anyhow::anyhow!("Error calling callback: {}", e)),
                    }
                });

                result.push((callback_id.clone(), new_callback));
            }
        }
    }

    // Helper method to call a callback at runtime
    async fn call_callback(
        &self,
        pattern: &str,
        callback_id: &str,
        value: ValueType,
    ) -> Result<()> {
        let callbacks = self.event_callbacks.read().await;
        if let Some(callbacks_for_pattern) = callbacks.get(pattern) {
            for (id, callback) in callbacks_for_pattern {
                if id == callback_id {
                    return callback(value);
                }
            }
        }

        Err(anyhow::anyhow!("Callback not found"))
    }

    /// Get the current node handler
    pub async fn get_node_handler(&self) -> Option<Arc<dyn NodeRequestHandler + Send + Sync>> {
        let node_handler_read = self.node_handler_lock.read().await;
        node_handler_read.clone()
    }

    /// Publish an event to the node_handler
    pub async fn publish(&self, topic: &str, data: ValueType) -> Result<()> {
        // DIAGNOSTIC: Log entry into publish
        println!("DIAG-REGISTRY: publish called for topic: {}", topic);

        // Get the list of subscribers for this topic
        let subscribers = {
            let subscribers = self.event_subscribers.read().await;
            if let Some(subs) = subscribers.get(topic) {
                println!(
                    "DIAG-REGISTRY: Found {} subscribers for topic {}",
                    subs.len(),
                    topic
                );
                subs.clone()
            } else {
                println!("DIAG-REGISTRY: No subscribers found for topic {}", topic);
                Vec::new()
            }
        };

        // If there are no subscribers, we're done
        if subscribers.is_empty() {
            debug_log(
                Component::ServiceRegistry,
                &format!("No subscribers found for topic {}", topic),
            );
            return Ok(());
        }

        // Process each subscriber by calling handle_event which is void/doesn't return anything
        self.handle_event(topic, &data).await;

        Ok(())
    }

    /// Register a remote service discovered from a peer
    pub async fn register_remote_service(
        &self,
        peer_id: PeerId,
        service_info: P2PServiceInfo,
    ) -> Result<()> {
        debug_log(
            Component::Registry,
            &format!(
                "Registering remote service '{}' from peer {:?}",
                service_info.name, peer_id
            ),
        )
        .await;

        // Get the P2P delegate
        let p2p_delegate = match self.p2p_delegate.read().await.as_ref() {
            Some(delegate) => delegate.clone(),
            None => {
                return Err(anyhow!(
                    "No P2P delegate available for remote service registration"
                ));
            }
        };

        // Create a remote service adapter
        let remote_service = crate::services::remote::RemoteService::new(
            service_info.name.clone(),
            service_info.path.clone(),
            peer_id.clone(), // Clone the peer_id to avoid moving it
            service_info.operations.clone(),
            p2p_delegate,
        );

        // Register the remote service
        let service_arc = Arc::new(remote_service);

        // Add to the services map
        let mut services = self.services.write().await;

        // Check if service already exists
        if services.contains_key(&service_info.name) {
            debug_log(
                Component::Registry,
                &format!(
                    "Remote service '{}' already exists in registry",
                    service_info.name
                ),
            );
            return Ok(());
        }

        // Insert the service
        services.insert(service_info.name.clone(), service_arc);

        info_log(
            Component::Registry,
            &format!(
                "Successfully registered remote service '{}' from peer {:?}",
                service_info.name,
                peer_id.clone() // Clone the peer_id here too
            ),
        )
        .await;

        Ok(())
    }

    // Add a new method to get the event subscribers (for debugging)
    pub async fn get_event_subscribers(&self) -> tokio::sync::RwLockReadGuard<'_, HashMap<String, Vec<String>>> {
        self.event_subscribers.read().await
    }
}

/// Implementation of the AbstractService trait for ServiceRegistry
#[async_trait]
impl AbstractService for ServiceRegistry {
    fn name(&self) -> &str {
        "registry"
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        ServiceState::Running
    }

    fn description(&self) -> &str {
        "Service Registry"
    }

    fn metadata(&self) -> ServiceMetadata {
        ServiceMetadata {
            name: self.name().to_string(),
            path: self.path().to_string(),
            state: self.state(),
            description: self.description().to_string(),
            operations: vec![
                "register".to_string(),
                "unregister".to_string(),
                "list".to_string(),
                "find".to_string(),
            ],
            version: "1.0".to_string(),
        }
    }

    async fn init(&mut self, _ctx: &RequestContext) -> Result<()> {
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        match request.action.as_str() {
            "register" => {
                // Register a service
                // This would normally be more complex - for now, just return success
                Ok(ServiceResponse::success::<String>(
                    "Service registered".to_string(),
                    None,
                ))
            }
            "unregister" => {
                // Unregister a service
                // This would normally be more complex - for now, just return success
                Ok(ServiceResponse::success::<String>(
                    format!("Service '{}' unregistered", request.data.as_ref().unwrap().get("name").unwrap().as_str().unwrap()),
                    None,
                ))
            }
            "list" => {
                // List all services
                let services = self.list_services().await;
                
                Ok(ServiceResponse::success(
                    "Services listed".to_string(),
                    Some(ValueType::Array(
                        services.into_iter().map(|s| ValueType::String(s)).collect()
                    )),
                ))
            }
            "find" => {
                // Find a service by name
                if let Some(data) = &request.data {
                    if let Some(ValueType::String(name)) = data.get("name") {
                        if let Some(service) = self.get_service(&name).await {
                            // Service found, return success with service info
                            let service_info = service.get_info().await?;
                            return Ok(ServiceResponse::success(
                                format!("Service '{}' found", name),
                                Some(service_info),
                            ));
                        } else {
                            return Ok(ServiceResponse::error(format!("Service '{}' not found", name)));
                        }
                    }
                }
                
                Ok(ServiceResponse::error("Missing service name parameter"))
            }
            _ => {
                Ok(ServiceResponse::error(format!("Unknown operation: {}", request.action)))
            }
        }
    }
}

/// Registry Service for service discovery and management
/// This is a system service that provides information about available services
pub struct RegistryService {
    /// Name of the service
    name: String,
    /// Path of the service
    path: String,
    /// Current state
    state: std::sync::Mutex<ServiceState>,
    /// The database
    db: Arc<SqliteDatabase>,
}

impl RegistryService {
    /// Create a new registry service
    pub fn new(db: Arc<SqliteDatabase>, network_id: &str) -> Self {
        RegistryService {
            name: "registry".to_string(),
            path: format!("{}/registry", network_id),
            state: std::sync::Mutex::new(ServiceState::Created),
            db,
        }
    }

    /// Register a service
    pub async fn register_service(&self, service: Arc<dyn AbstractService>) -> Result<()> {
        // Register the service in the database
        // For now, just log the registration
        log::info!("Registering service: {}", service.name());
        Ok(())
    }
    
    pub async fn get_service_by_path(&self, path: &str) -> Option<Arc<dyn AbstractService>> {
        // In a real implementation, this would query the database
        // For now, just return None
        None
    }
}

#[async_trait]
impl AbstractService for RegistryService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn state(&self) -> ServiceState {
        *self.state.lock().unwrap()
    }

    fn metadata(&self) -> ServiceMetadata {
        ServiceMetadata {
            name: self.name().to_string(),
            path: self.path().to_string(),
            state: self.state(),
            description: self.description().to_string(),
            operations: vec![
                "register".to_string(),
                "unregister".to_string(),
                "list".to_string(),
                "get".to_string(),
            ],
            version: "1.0".to_string(),
        }
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
        // Extract the operation from the request
        let operation = &request.action;
        
        match operation.as_str() {
            "register" => {
                // Register a service
                if let Some(data) = &request.data {
                    if let Some(ValueType::Map(service_data)) = data.get("service") {
                        // In a real implementation, this would create and register the service
                        
                        Ok(ServiceResponse::success(
                            "Service registered".to_string(),
                            Some(ValueType::Map(service_data.clone())),
                        ))
                    } else {
                        Ok(ServiceResponse::error("Missing or invalid service data"))
                    }
                } else {
                    Ok(ServiceResponse::error("Missing parameters"))
                }
            }
            "unregister" => {
                // Unregister a service
                if let Some(data) = &request.data {
                    if let Some(ValueType::String(name)) = data.get("name") {
                        // In a real implementation, this would unregister the service
                        
                        Ok(ServiceResponse::success::<String>(
                            format!("Service '{}' unregistered", name),
                            None,
                        ))
                    } else {
                        Ok(ServiceResponse::error("Missing or invalid name parameter"))
                    }
                } else {
                    Ok(ServiceResponse::error("Missing parameters"))
                }
            }
            "list" => {
                // List all services
                // In a real implementation, this would query the registry
                
                let services = vec![
                    ValueType::String("registry".to_string()),
                    ValueType::String("node_info".to_string()),
                ];
                
                Ok(ServiceResponse::success(
                    "Services listed".to_string(),
                    Some(ValueType::Array(services)),
                ))
            }
            "get" => {
                // Get information about a specific service
                if let Some(data) = &request.data {
                    if let Some(ValueType::String(name)) = data.get("name") {
                        // In a real implementation, this would query the registry
                        
                        let mut service_info = HashMap::new();
                        service_info.insert("name".to_string(), ValueType::String(name.clone()));
                        service_info.insert("path".to_string(), ValueType::String(format!("/{}", name)));
                        service_info.insert("state".to_string(), ValueType::String("Running".to_string()));
                        
                        Ok(ServiceResponse::success(
                            format!("Service '{}' info", name),
                            Some(ValueType::Map(service_info)),
                        ))
                    } else {
                        Ok(ServiceResponse::error("Missing or invalid name parameter"))
                    }
                } else {
                    Ok(ServiceResponse::error("Missing parameters"))
                }
            }
            _ => {
                Ok(ServiceResponse::error(format!("Unknown operation: {}", operation)))
            }
        }
    }

    fn description(&self) -> &str {
        "Registry service for managing service registrations"
    }
}
