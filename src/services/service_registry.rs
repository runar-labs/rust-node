use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use log::{debug, error, info};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::RwLock;
use serde_json::Value;
use crate::vmap;
use tokio::time::timeout;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration as StdDuration;

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
use crate::util::logging::{debug_log, error_log, info_log, warn_log, Component};
// Remove the common module imports
// use crate::common::async_cache::AsyncCache;
// use crate::common::service::{
//     Event, EventCallback, EventData, EventDispatcher,
// };
// Define the necessary types here
type EventData = ValueType;
type EventCallback = Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>;
struct Event {
    topic: String,
    data: EventData,
}

/// Type definition for action handlers
pub type ActionHandler = dyn Fn(&RequestContext, &ValueType) -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> + Send + Sync;

/// Type definition for process handlers
pub type ProcessHandler = dyn Fn(&RequestContext, &str, &ValueType) -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> + Send + Sync;

/// Type definition for subscription handlers
pub type SubscriptionHandler = dyn Fn(ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync;

/// Entry for an action handler in the registry
#[derive(Clone)]
pub struct ActionHandlerEntry {
    /// The service that owns this action
    pub service: String,
    /// The name of the action
    pub name: String,
    /// Timeout for this action
    pub timeout: StdDuration,
    /// The handler function
    pub handler: Arc<ActionHandler>,
}

/// Registry for process handlers - maps service_name to handler functions
#[derive(Clone)]
pub struct ProcessHandlerEntry {
    pub service: String,
    pub timeout: StdDuration,
    pub handler: Arc<dyn Fn(&RequestContext, &str, &ValueType) -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> + Send + Sync>,
}

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

    /// Storage for action handlers
    action_handlers: RwLock<HashMap<(String, String), ActionHandlerEntry>>,
    
    /// Storage for process handlers
    process_handlers: RwLock<HashMap<String, ProcessHandlerEntry>>,
    
    /// Storage for subscriptions
    subscription_handlers: RwLock<HashMap<(String, String), Arc<dyn Fn(ValueType) -> Result<()> + Send + Sync>>>,
    
    /// Storage for publications
    publication_topics: RwLock<HashMap<String, HashSet<String>>>,

    /// Cache for service lookups
    services_cache: AsyncCache<String, Arc<dyn AbstractService + Send + Sync>>,

    /// Events that are waiting for a subscription
    pending_events: Arc<Mutex<Vec<(String, EventData)>>>,
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
            action_handlers: RwLock::new(HashMap::new()),
            process_handlers: RwLock::new(HashMap::new()),
            subscription_handlers: RwLock::new(HashMap::new()),
            publication_topics: RwLock::new(HashMap::new()),
            services_cache: AsyncCache::new(StdDuration::from_secs(300)),
            pending_events: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

// Implement the ServiceRegistry trait from server.rs
#[async_trait]
impl crate::server::ServiceRegistry for ServiceRegistry {
    async fn register_service(&self, service: Arc<dyn crate::server::Service>) -> Result<()> {
        // Get the service name
        let service_name = service.name().to_string();
        
        info_log(
            Component::Registry,
            &format!("Registering server service: {}", service_name)
        ).await;
        
        // Create an adapter that wraps the Server::Service as an AbstractService
        let adapter = ServerServiceAdapter::new(service);
        
        // Store in our services map
        let mut services = self.services.write().await;
        services.insert(service_name.clone(), Arc::new(adapter));
        
        Ok(())
    }
    
    async fn get_service(&self, name: &str) -> Option<Arc<dyn crate::server::Service>> {
        // Try to get the service from our registry
        if let Some(abstract_service) = self.get_service(name).await {
            // Check if it's a ServerServiceAdapter
            if let Some(adapter) = ServerServiceAdapter::from_abstract(abstract_service.clone()) {
                return Some(adapter.inner_service());
            }
        }
        None
    }
}

/// Adapter to convert between Service and AbstractService
struct ServerServiceAdapter {
    inner: Arc<dyn crate::server::Service>,
}

impl ServerServiceAdapter {
    fn new(service: Arc<dyn crate::server::Service>) -> Self {
        Self { inner: service }
    }
    
    fn inner_service(&self) -> Arc<dyn crate::server::Service> {
        self.inner.clone()
    }
    
    fn from_abstract(service: Arc<dyn AbstractService>) -> Option<Arc<Self>> {
        // Try a downcast to our adapter type
        let type_id = std::any::TypeId::of::<ServerServiceAdapter>();
        
        // This is a simplification - in reality we'd need a proper downcast mechanism
        // For now we'll just check if the name follows our convention
        let name = service.name();
        if name.starts_with("server_adapter_") {
            // We'd return the actual adapter here
            // but since we can't dynamically downcast easily, we'll return None
            None
        } else {
            None
        }
    }
}

#[async_trait]
impl AbstractService for ServerServiceAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }
    
    fn path(&self) -> &str {
        // We don't have a path in the Service trait, so use the name
        self.inner.name()
    }
    
    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        self.inner.handle_request(request).await
    }
    
    async fn init(&mut self, _context: &RequestContext) -> Result<()> {
        // No initialization in the Service trait
        Ok(())
    }
    
    // Added missing methods
    fn state(&self) -> ServiceState {
        ServiceState::Running
    }
    
    fn description(&self) -> &str {
        "Server service adapter"
    }
    
    fn metadata(&self) -> ServiceMetadata {
        // Create default metadata
        ServiceMetadata {
            name: self.inner.name().to_string(),
            path: self.inner.name().to_string(),
            description: "Server service adapter".to_string(),
            version: "1.0".to_string(),
            operations: vec![],
            state: ServiceState::Running,
        }
    }
    
    async fn start(&mut self) -> Result<()> {
        // No start in the Service trait
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        // No stop in the Service trait
        Ok(())
    }
}

// Implement the NodeRequestHandler trait
#[async_trait::async_trait]
impl NodeRequestHandler for ServiceRegistry {
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        // Create a request context with default settings
        let context = RequestContext::new_with_option(
            path.clone(), 
            Some(params.clone()),
            Arc::new(NodeRequestHandlerImpl::new(Arc::new(self.clone()))),
        );
        
        // Create a service request
        let request = ServiceRequest {
            request_id: Some(uuid::Uuid::new_v4().to_string()),
            path: path.clone(),
            operation: path.split('/').last().unwrap_or("").to_string(),
            params: Some(params),
            request_context: Arc::new(context),
        };
        
        // Process the request
        self.handle_request(request).await
    }
    
    async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        debug_log(
            Component::Service,
            &format!("Publishing to topic: {}", topic)
        ).await;
        
        // Find all subscribers for this topic
        let subscribers = {
            let subs = self.event_subscribers.read().await;
            if let Some(subs_vec) = subs.get(&topic) {
                subs_vec.clone()
            } else {
                Vec::new()
            }
        };
        
        // Invoke each subscriber's callback
        for service_name in subscribers {
            let callbacks = self.event_callbacks.read().await;
            if let Some(callbacks_vec) = callbacks.get(&service_name) {
                for (callback_topic, callback) in callbacks_vec {
                    if callback_topic == &topic {
                        if let Err(e) = callback(data.clone()) {
                            error_log(
                                Component::Service,
                                &format!(
                                    "Error invoking callback for topic {} on service {}: {:?}",
                                    topic, service_name, e
                                ),
                            ).await;
                        }
                    }
                }
            }
        }
        
        Ok(())
    }
    
    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
    ) -> Result<String> {
        // Default options
        let options = SubscriptionOptions {
            ttl: None,
            max_triggers: None,
            once: false,
            id: None,
        };
        
        // Use the options version
        self.subscribe_with_options(topic, callback, options).await
    }
    
    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        debug_log(
            Component::Service,
            &format!("Subscribing to topic: {} with options: {:?}", topic, options)
        ).await;
        
        // Generate a unique subscription ID
        let subscription_id = options.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        
        // Make up a service name from the subscription ID
        let service_name = format!("subscription_{}", subscription_id);
        
        // Store the callback
        {
            let mut callbacks = self.event_callbacks.write().await;
            let service_callbacks = callbacks.entry(service_name.clone()).or_insert_with(Vec::new);
            service_callbacks.push((topic.clone(), callback));
        }
        
        // Add to subscribers
        {
            let mut subscribers = self.event_subscribers.write().await;
            let topic_subscribers = subscribers.entry(topic.clone()).or_insert_with(Vec::new);
            topic_subscribers.push(service_name);
        }
        
        // Handle expiration logic if needed
        if let Some(duration) = options.ttl {
            let registry = self.clone();
            let topic_clone = topic.clone();
            let sub_id = subscription_id.clone();
            
            // Spawn a task to expire the subscription
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                if let Err(e) = registry.unsubscribe_wrapper(topic_clone, sub_id).await {
                    error_log(
                        Component::Service,
                        &format!("Error unsubscribing expired subscription: {:?}", e)
                    ).await;
                }
            });
        }
        
        // Handle one-time subscriptions
        if options.once {
            // No need to do anything here - we'll unsubscribe after the first event
        }
        
        // Handle max triggers
        if let Some(max_triggers) = options.max_triggers {
            // Could track this with a counter, but for now we'll just log it
            debug_log(
                Component::Service,
                &format!("Subscription has max_triggers set to {}", max_triggers)
            ).await;
        }
        
        Ok(subscription_id)
    }
    
    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()> {
        // Convert the subscription_id to the service name format
        let service_name = if let Some(id) = subscription_id {
            format!("subscription_{}", id)
        } else {
            // If no ID is provided, we can't know which service to unsubscribe
            return Err(anyhow!("Subscription ID is required for unsubscribing"));
        };
        
        // Use our internal unsubscribe implementation
        self.unsubscribe(&service_name, &topic, subscription_id).await
    }
    
    fn list_services(&self) -> Vec<String> {
        // Create a blocking task to get services
        let registry = self.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();
        
        rt.block_on(async {
            let services_lock = registry.services.read().await;
            services_lock.keys().cloned().collect()
        })
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
            action_handlers: RwLock::new(HashMap::new()),
            process_handlers: RwLock::new(HashMap::new()),
            subscription_handlers: RwLock::new(HashMap::new()),
            publication_topics: RwLock::new(HashMap::new()),
            services_cache: AsyncCache::new(StdDuration::from_secs(300)),
            pending_events: Arc::new(Mutex::new(Vec::new())),
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
            action_handlers: RwLock::new(HashMap::new()),
            process_handlers: RwLock::new(HashMap::new()),
            subscription_handlers: RwLock::new(HashMap::new()),
            publication_topics: RwLock::new(HashMap::new()),
            services_cache: AsyncCache::new(StdDuration::from_secs(300)),
            pending_events: Arc::new(Mutex::new(Vec::new())),
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
        let peer_id_clone = peer_id.clone();
        self.remove_remote_subscriptions(peer_id_clone).await;

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
        }

        Ok(())
    }

    /// Register all action handlers from the runtime registry
    pub async fn register_runtime_actions(&self) -> Result<()> {
        // Get all action handlers from the runtime registry
        let action_handlers = crate::init::get_action_handlers();

        info_log(
            Component::Registry,
            &format!("Registering {} runtime action handlers", action_handlers.len())
        ).await;
        
        let mut handlers_map = self.action_handlers.write().await;
        
        for handler in action_handlers {
            debug_log(
                    Component::Registry,
                &format!("Registering runtime action handler: {} for service {}", handler.name, handler.service)
            ).await;
            
            // Store the handler in our improved map
            let key = (handler.service.clone(), handler.name.clone());
            
            // We need to create a wrapper function that makes sure the lifetimes match
            // by cloning the values so they're owned by the closure
            let handler_fn = handler.handler;
            let wrapper_handler = move |context: &RequestContext, params: &ValueType| -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> {
                // Clone the context and params to ensure correct lifetimes
                let context_clone = context.clone();
                let params_clone = params.clone();
                
                // Return a boxed future that calls the original handler with the cloned values
                Box::pin(async move {
                    handler_fn(&context_clone, params_clone).await
                })
            };
            
            // Create an entry for this handler with Arc wrapped handler
            let entry = ActionHandlerEntry {
                service: handler.service.clone(),
                name: handler.name.clone(),
                timeout: handler.timeout,
                handler: Arc::new(wrapper_handler),
            };
            
            handlers_map.insert(key, entry);
            
            // Also find the service and associate this action with it if it exists
            if let Some(_service) = self.get_service(&handler.service).await {
                debug_log(
                    Component::Registry,
                    &format!("Associated action '{}' with service '{}'", handler.name, handler.service)
                ).await;
            }
        }
        
        info_log(
                    Component::Registry,
            &format!("Registered {} runtime action handlers", handlers_map.len())
        ).await;
        
        Ok(())
    }
    
    /// Register all process handlers from the runtime registry
    pub async fn register_runtime_processes(&self) -> Result<()> {
        // Get all process handlers from the runtime registry
        let process_handlers = crate::init::get_process_handlers();
        
        info_log(
            Component::Registry,
            &format!("Registering {} runtime process handlers", process_handlers.len())
        ).await;
        
        let mut handlers_map = self.process_handlers.write().await;
        
        for handler in process_handlers {
            debug_log(
                Component::Registry,
                &format!("Registering runtime process handler for service: {}", handler.service)
            ).await;
            
            // Store the handler in our improved map
            let key = handler.service.clone();
            
            // We need to create a wrapper function that ensures correct lifetimes
            let handler_fn = handler.handler;
            let wrapper_handler = move |context: &RequestContext, operation: &str, params: &ValueType| -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> {
                // Clone all inputs to ensure correct lifetimes
                let context_clone = context.clone();
                let operation_clone = operation.to_string();
                let params_clone = params.clone();
                
                // Return a boxed future that calls the original handler with the cloned values
                Box::pin(async move {
                    handler_fn(&context_clone, &operation_clone, &params_clone).await
                })
            };
            
            // Create an entry for this handler with Arc wrapped handler
            let entry = ProcessHandlerEntry {
                service: handler.service.clone(),
                timeout: std::time::Duration::from_secs(30), // Default timeout
                handler: Arc::new(wrapper_handler),
            };
            
            handlers_map.insert(key, entry);
            
            // Also find the service and associate this process handler with it if it exists
            if let Some(_service) = self.get_service(&handler.service).await {
            debug_log(
                Component::Registry,
                    &format!("Associated process handler with service '{}'", handler.service)
                ).await;
            }
        }
        
                info_log(
                    Component::Registry,
            &format!("Registered {} runtime process handlers", handlers_map.len())
        ).await;

        Ok(())
    }

    /// Register all event subscriptions from the runtime registry
    pub async fn register_runtime_subscriptions(&self) -> Result<()> {
        // Get all event subscriptions from the runtime registry
        let subscriptions = crate::init::get_subscriptions();
        
        info_log(
            Component::Registry,
            &format!("Registering {} runtime subscriptions", subscriptions.len())
        ).await;
        
        for subscription in subscriptions {
        debug_log(
                Component::Registry,
                &format!(
                    "Registering runtime subscription for topic '{}' in service '{}'",
                    subscription.topic, subscription.service
                )
            ).await;
            
            // Convert the async handler to a synchronous callback
            let handler_fn = subscription.handler;
            
            // Create a callback that correctly handles async
            let callback = Box::new(move |payload: ValueType| -> Result<()> {
                // Create a new runtime to execute the future
                let rt = tokio::runtime::Runtime::new()?;
                
                // Execute the future using the runtime
                rt.block_on(async {
                    let fut = handler_fn(payload);
                    fut.await
                })
            }) as Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>;
            
            // Store the handler reference separately for later use
            {
                let mut subscription_handlers = self.subscription_handlers.write().await;
                let key = (subscription.service.clone(), subscription.topic.clone());
                
                // Can't store the callback directly as it's not clonable
                // Instead, we'll create a wrapper function that recreates the logic
                let wrapper = Arc::new(move |payload: ValueType| -> Result<()> {
                    // Create a new runtime to execute the future
                    let rt = tokio::runtime::Runtime::new()?;
                    
                    // Execute the future using the runtime  
                    rt.block_on(async {
                        let fut = handler_fn(payload);
                        fut.await
                    })
                });
                
                subscription_handlers.insert(key, wrapper);
            }
            
            // Create subscription options
            let options = SubscriptionOptions {
                ttl: None,
                max_triggers: None,
                once: false,
                id: None,
            };
            
            // Also subscribe to the topic so we receive events
            self.subscribe_with_options(
                subscription.topic.clone(), 
                callback,
                options
            ).await?;
            
            debug_log(
                Component::Registry,
                &format!(
                    "Successfully registered subscription for topic '{}' in service '{}'",
                    subscription.topic, subscription.service
                )
            ).await;
        }
        
        info_log(
            Component::Registry,
            "Completed registering runtime subscriptions"
        ).await;
        
        Ok(())
    }
    
    /// Register all publication info from the runtime registry
    pub async fn register_runtime_publications(&self) -> Result<()> {
        // Get all publication info from the runtime registry
        let publications = crate::init::get_publications();
        
        info_log(
            Component::Registry,
            &format!("Registering {} runtime publications", publications.len())
        ).await;
        
        let mut pub_map = self.publication_topics.write().await;
        
        for publication in publications {
        debug_log(
            Component::Registry,
            &format!(
                    "Registering runtime publication for topic '{}' from service '{}'",
                    publication.topic, publication.service
                )
            ).await;
            
            // Associate this publication topic with the service
            let topics = pub_map.entry(publication.service.clone())
                .or_insert_with(HashSet::new);
            topics.insert(publication.topic.clone());
            
            // Note: We don't need to do anything more with publications
            // since they are just metadata about what topics a service publishes to
            // The actual publishing happens through the RequestContext
        }
        
        info_log(
                        Component::Registry,
            &format!("Registered {} services with publication topics", pub_map.len())
        ).await;
        
        Ok(())
    }
    
    /// Initialize all runtime-registered handlers and subscriptions
    pub async fn init_runtime_registrations(&self) -> Result<()> {
        // Register all handlers and subscriptions from the runtime registry
        info_log(
                Component::Registry,
            "Initializing all runtime registrations (actions, processes, subscriptions, publications)"
        ).await;
        
        // Perform each registration step
        self.register_runtime_actions().await?;
        self.register_runtime_processes().await?;
        self.register_runtime_subscriptions().await?;
        self.register_runtime_publications().await?;

        info_log(
            Component::Registry,
            "Successfully initialized all runtime registrations"
        ).await;
        
        Ok(())
    }

    /// Call an action handler directly - useful for bypassing the service request system
    pub async fn call_action_handler(
        &self,
        service: &str,
        action: &str,
        context: &RequestContext,
        params: &ValueType
    ) -> Result<ServiceResponse> {
        let handler_map = self.action_handlers.read().await;
        
        // Look up the handler by service and action name
        if let Some(entry) = handler_map.get(&(service.to_string(), action.to_string())) {
            // Call the handler
                        debug_log(
                            Component::Registry,
                &format!("Directly calling action handler '{}' for service '{}'", action, service)
            ).await;
            
            // Execute with timeout
            match timeout(entry.timeout, (entry.handler)(context, params)).await {
                Ok(result) => result,
                Err(_) => {
                        error_log(
                            Component::Registry,
                        &format!("Action handler '{}' for service '{}' timed out after {:?}", 
                            action, service, entry.timeout)
                    ).await;
                    
                    Err(anyhow!("Action handler timed out"))
                    }
                }
            } else {
            // Handler not found
            debug_log(
                    Component::Registry,
                &format!("Action handler '{}' not found for service '{}'", action, service)
            ).await;
            
            // Check if the service exists and try to call it through the regular mechanism
            if let Some(service_obj) = self.get_service(service).await {
                debug_log(
                Component::Registry,
                    &format!("Falling back to service.handle_request() for action '{}'", action)
                ).await;
                
                // Create a service request
                let request = ServiceRequest {
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    path: format!("{}/{}", service_obj.path(), action),
                    operation: action.to_string(),
                    params: Some(params.clone()),
                    request_context: Arc::new(context.clone()),
                };
                
                // Call through the service's handle_request method
                service_obj.handle_request(request).await
        } else {
                Err(anyhow!("Action handler and service not found"))
            }
        }
    }
    
    /// Call a process handler directly - useful for bypassing the service request system
    pub async fn call_process_handler(
        &self,
        service: &str,
        operation: &str,
        context: &RequestContext,
        params: &ValueType
    ) -> Result<ServiceResponse> {
        let handler_map = self.process_handlers.read().await;
        
        // Look up the handler by service name
        if let Some(entry) = handler_map.get(service) {
            // Call the handler
        debug_log(
                Component::Registry,
                &format!("Directly calling process handler for service '{}' operation '{}'", service, operation)
            ).await;
            
            // Execute with timeout
            match timeout(entry.timeout, (entry.handler)(context, operation, params)).await {
                Ok(result) => result,
                Err(_) => {
                    error_log(
                        Component::Registry,
                        &format!("Process handler for service '{}' operation '{}' timed out after {:?}", 
                            service, operation, entry.timeout)
                    ).await;
                    
                    Err(anyhow!("Process handler timed out"))
                    }
                }
            } else {
            // Handler not found
                debug_log(
                Component::Registry,
                &format!("Process handler not found for service '{}'", service)
            ).await;
            
            // Check if the service exists and try to call it through the regular mechanism
            if let Some(service_obj) = self.get_service(service).await {
                            debug_log(
                    Component::Registry,
                    &format!("Falling back to service.handle_request() for operation '{}'", operation)
                ).await;

            // Create a service request
            let request = ServiceRequest {
                    request_id: Some(uuid::Uuid::new_v4().to_string()),
                    path: format!("{}/{}", service_obj.path(), operation),
                    operation: operation.to_string(),
                    params: Some(params.clone()),
                    request_context: Arc::new(context.clone()),
                };
                
                // Call through the service's handle_request method
                service_obj.handle_request(request).await
                    } else {
                Err(anyhow!("Process handler and service not found"))
            }
        }
    }
    
    /// Get a map of all registered action handlers for a service
    pub async fn get_action_handlers_for_service(&self, service: &str) -> HashMap<String, ActionHandlerEntry> {
        let handler_map = self.action_handlers.read().await;
        
        let mut result = HashMap::new();
        for ((service_name, action_name), entry) in handler_map.iter() {
            if service_name == service {
                result.insert(action_name.clone(), entry.clone());
            }
        }
        
        result
    }
    
    /// Get a map of all registered process handlers
    pub async fn get_process_handlers(&self) -> HashMap<String, ProcessHandlerEntry> {
        let handler_map = self.process_handlers.read().await;
        handler_map.clone()
    }

    /// Handle a service request
    pub async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // First, parse the path to get the service name and action/operation
        let path_parts: Vec<&str> = request.path.split('/').collect();
        
        if path_parts.len() < 2 {
            error_log(
            Component::Registry,
                &format!("Invalid path format: {}. Expected service/action", request.path)
            ).await;
            return Err(anyhow!("Invalid path format: {}. Expected service/action", request.path));
        }
        
        let service_name = path_parts[0];
        let action_name = path_parts[1];
        
        debug_log(
            Component::Registry,
            &format!("Processing request for service '{}', action '{}'", service_name, action_name)
        ).await;
        
        // Extract the context and parameters
        let context = &request.request_context;
        let params = request.params.as_ref().unwrap_or(&ValueType::Null);
        
        // Try to find a direct handler first for better performance
        // Check if we have a registered action handler
        {
            let action_handlers = self.action_handlers.read().await;
            let key = (service_name.to_string(), action_name.to_string());
            
            if let Some(entry) = action_handlers.get(&key) {
                debug_log(
                    Component::Registry,
                    &format!("Found direct action handler for '{}' in service '{}'", action_name, service_name)
                ).await;
                
                // Use the direct handler
                return match timeout(entry.timeout, (entry.handler)(context.as_ref(), params)).await {
                    Ok(result) => result,
                    Err(_) => {
                        error_log(
                            Component::Registry,
                            &format!("Action handler '{}' for service '{}' timed out after {:?}", 
                                action_name, service_name, entry.timeout)
                        ).await;
                        
                        Err(anyhow!("Action handler timed out"))
                    }
                };
            }
        }
        
        // Check if we have a registered process handler
        {
            let process_handlers = self.process_handlers.read().await;
            
            if let Some(entry) = process_handlers.get(service_name) {
                debug_log(
            Component::Registry,
                    &format!("Found direct process handler for service '{}'", service_name)
                ).await;
                
                // Use the direct handler
                return match timeout(entry.timeout, (entry.handler)(context.as_ref(), action_name, params)).await {
                    Ok(result) => result,
                    Err(_) => {
            error_log(
                Component::Registry,
                            &format!("Process handler for service '{}' operation '{}' timed out after {:?}", 
                                service_name, action_name, entry.timeout)
                        ).await;
                        
                        Err(anyhow!("Process handler timed out"))
                    }
                };
            }
        }
        
        // If no direct handler found, fall back to the traditional service lookup
        debug_log(
                                    Component::Registry,
            &format!("No direct handler found, falling back to service lookup for '{}'", service_name)
                                ).await;
        
        // Get the service from the registry
        if let Some(service) = self.get_service(service_name).await {
            // Call handle_request on the service
            service.handle_request(request).await
                            } else {
            // Service not found
                                error_log(
                                    Component::Registry,
                &format!("Service not found: {}", service_name)
                                ).await;
            
            Err(anyhow!("Service not found: {}", service_name))
        }
    }

    /// Get a service by name
    pub async fn get_service(&self, name: &str) -> Option<Arc<dyn AbstractService>> {
        // First check our cache
        if let Some(service) = self.services_cache.get(&name.to_string()).await {
            return Some(service);
        }
        
        // Then check our registry
        let services = self.services.read().await;
        if let Some(service) = services.get(name) {
            // Cache this for future lookups
            self.services_cache.set(name.to_string(), service.clone()).await;
            return Some(service.clone());
        }
        
        None
    }

    /// Get a service by path
    pub async fn get_service_by_path(&self, path: &str) -> Option<Arc<dyn AbstractService>> {
        // Split the path and get the first part as the service name
        let parts: Vec<&str> = path.split('/').collect();
        if parts.is_empty() {
            return None;
        }
        
        self.get_service(parts[0]).await
    }

    /// Get all services in the registry
    pub async fn get_all_services(&self) -> Vec<Arc<dyn AbstractService>> {
        let services = self.services.read().await;
        services.values().cloned().collect()
    }

    /// Register a remote Service
    pub async fn register_service(&self, service: Arc<dyn AbstractService>) -> Result<()> {
        let service_name = service.name().to_string();
        
            debug_log(
            Component::Registry,
            &format!("Registering service: {}", service_name)
        ).await;
        
        // Store in our services map
        let mut services = self.services.write().await;
        services.insert(service_name, service);

        Ok(())
    }

    /// Handle a Service Message (from P2P)
    pub async fn handle_message(&self, peer_id: PeerId, message_bytes: &[u8]) -> Result<()> {
        let message: P2PMessage = bincode::deserialize(message_bytes)?;
        
        debug_log(
            Component::Registry,
            &format!("Handling message from peer {:?}: {:?}", peer_id, message)
        ).await;
        
        // Process based on message type
        match message {
            P2PMessage::Request { request_id, path, params } => {
                // Handle service request
                // Parse the path to get service and operation
                let parts: Vec<&str> = path.split('/').collect();
                if parts.len() < 2 {
                    return Err(anyhow!("Invalid path format: {}", path));
                }
                
                let service = parts[0].to_string();
                let operation = parts[1].to_string();
                
                if let Some(service_obj) = self.get_service(&service).await {
                    // Create a context for the request
                    let context = RequestContext::new_with_option(
                        path.clone(),
                        Some(params.clone()),
                        Arc::new(NodeRequestHandlerImpl::new(Arc::new(self.clone()))),
                    );
                    
                    // Create a service request
                    let request = ServiceRequest {
                        request_id: Some(request_id.clone()),
                        path,
                        operation,
                        params: Some(params),
                        request_context: Arc::new(context),
                    };
                    
                    // Process the request
                    match service_obj.handle_request(request).await {
                        Ok(response) => {
                            // Send response back via P2P
                            if let Some(delegate) = self.p2p_delegate.read().await.as_ref() {
                                // Need to wrap with response
                                let response_msg = P2PMessage::Response {
                                    request_id,
                                    response,
                                };
                                
                                // Serialize and send - need to use the delegate's method
                                let serialized = bincode::serialize(&response_msg)?;
                                delegate.send_message(peer_id, String::from_utf8_lossy(&serialized).to_string()).await?;
                            }
                        }
                        Err(e) => {
                            error_log(
                                Component::Registry,
                                &format!("Error processing request: {}", e)
                            ).await;
                            
                            // Send error response
                            let error_response = ServiceResponse {
                                status: crate::services::ResponseStatus::Error,
                                message: format!("Error processing request: {}", e),
                                data: None,
                            };
                            
                            if let Some(delegate) = self.p2p_delegate.read().await.as_ref() {
                                let serialized = bincode::serialize(&error_response)?;
                                delegate.send_message(peer_id, String::from_utf8_lossy(&serialized).to_string()).await?;
                            }
                        }
                    }
                } else {
                    // Service not found
                    let error_response = ServiceResponse {
                        status: crate::services::ResponseStatus::Error,
                        message: format!("Service not found: {}", path),
                        data: None,
                    };
                    
                    // Send error response
                    if let Some(delegate) = self.p2p_delegate.read().await.as_ref() {
                        let serialized = bincode::serialize(&error_response)?;
                        delegate.send_message(peer_id, String::from_utf8_lossy(&serialized).to_string()).await?;
                    }
                }
            }
            P2PMessage::Response { request_id, response } => {
                // This would be handled by the P2P service directly
                debug_log(
                    Component::Registry,
                    &format!("Got service response for request {}", request_id)
                ).await;
            }
            P2PMessage::Event { topic, data } => {
                // Handle incoming events from remote peers
                debug_log(
                    Component::Registry,
                    &format!("Got service event for topic {}", topic)
                ).await;
                
                // Forward to local subscribers
                self.publish(topic, data).await?;
            }
            P2PMessage::ServiceDiscovery { services } => {
                // Register remote services from this peer
                self.register_remote_services(peer_id, services).await?;
            }
            P2PMessage::ConnectNotification { peer_id: _, address: _ } => {
                // Handle connection notification
                debug_log(
                    Component::Registry,
                    &format!("Got connection notification from peer {:?}", peer_id)
                ).await;
            }
            _ => {
                // Unknown message type
                debug_log(
                    Component::Registry,
                    &format!("Unknown message type received")
                ).await;
            }
        }
        
        Ok(())
    }
    
    /// Remove all remote subscriptions for a peer
    pub async fn remove_remote_subscriptions(&self, peer_id: PeerId) -> Result<()> {
        debug_log(
            Component::Registry,
            &format!("Removing all remote subscriptions for peer {:?}", peer_id)
        ).await;
        
        let mut remote_subs = self.remote_subscriptions.write().await;
        remote_subs.remove(&peer_id);
        
        Ok(())
    }
    
    // Helper method that takes a reference
    pub async fn unregister_peer(&self, peer_id: PeerId) -> Result<()> {
        let peer_id_str = peer_id.to_string();
        debug_log(
            Component::Service,
            &format!("Unregistering peer: {}", peer_id_str)
        );

        let peer_id_clone = peer_id.clone();
        // First, remove any remote subscriptions from this peer
        let _ = self.remove_remote_subscriptions(peer_id_clone).await;
        
        // Then remove the peer from connected peers
        let mut remote_peers = self.remote_peers.write().await;
        remote_peers.remove(&peer_id);
        
        Ok(())
    }

    /// Helper method to handle unsubscribe requests
    pub async fn unsubscribe_helper(&self, topic: String, subscription_id: Option<&str>) -> Result<()> {
        // Convert the subscription_id to the service name format
        let service_name = if let Some(id) = subscription_id {
            format!("subscription_{}", id)
                    } else {
            // If no ID is provided, we can't know which service to unsubscribe
            return Err(anyhow!("Subscription ID is required for unsubscribing"));
        };
        
        // Use our internal unsubscribe implementation
        self.unsubscribe(&service_name, &topic, subscription_id).await
    }

    /// Fix for the signature mismatch in the unsubscribe method
    /// This method is used in the callback expiration handler
    pub async fn unsubscribe_wrapper(&self, topic_clone: String, sub_id: String) -> Result<()> {
        // Convert the String to &str for the subscription_id
        self.unsubscribe(&format!("subscription_{}", sub_id), &topic_clone, Some(&sub_id)).await
    }
}

// Simple AsyncCache implementation
struct AsyncCache<K, V> {
    cache: Arc<Mutex<HashMap<K, V>>>,
    ttl: StdDuration,
}

impl<K, V> AsyncCache<K, V> 
where 
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
{
    fn new(ttl: StdDuration) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl,
        }
    }

    async fn get(&self, key: &K) -> Option<V> 
    where 
        K: Clone,
        V: Clone,
    {
        let cache = self.cache.lock().unwrap();
        cache.get(key).cloned()
    }

    async fn set(&self, key: K, value: V) {
        let mut cache = self.cache.lock().unwrap();
        cache.insert(key, value);
    }
}