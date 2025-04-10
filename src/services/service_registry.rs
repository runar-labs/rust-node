// Service Registry Module
//
// INTENTION:
// This module provides action handler and event subscription management capabilities for the node.
// It acts as a central registry for action handlers and event subscriptions, enabling the node to
// find the correct subscribers and actions handlers. THE Registry does not CALL ANY CALLBACKS/Handler directly
//.. this is NODEs functions.
//
// ARCHITECTURAL PRINCIPLES:
// 1. Handler Registration - Manages registration of action handlers
// 2. Event Subscription Registration - Manages registration of event handlers 
// 3. Network Isolation - Respects network boundaries for handlers and subscriptions
// 4. Path Consistency - ALL Registry APIs use TopicPath objects for proper validation
//    and consistent path handling, NEVER raw strings
// 5. Separate Storage - Local and remote handlers are stored separately for clear responsibility
//
// IMPORTANT NOTE:
// The Registry should focus solely on managing action handlers and subscriptions.
// It should NOT handle service discovery or lifecycle - that's the responsibility of the Node.
// Request routing and handling is also the Node's responsibility.

use anyhow::{anyhow, Result};
use log::debug;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use runar_common::logging::Logger;
use runar_common::types::ValueType;
use crate::routing::{TopicPath, WildcardSubscriptionRegistry};
use crate::services::abstract_service::{ActionMetadata, AbstractService, ServiceState, CompleteServiceMetadata};
use crate::services::{
    ServiceResponse, SubscriptionOptions, 
    ActionHandler, EventContext, RemoteService,
    RegistryDelegate
};
use crate::network::transport::PeerId;

/// Type definition for event callbacks
///
/// INTENTION: Define a type that can handle event notifications asynchronously.
/// The callback takes an event context and payload and returns a future that
/// resolves once the event has been processed.
pub type EventCallback = Arc<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Type definition for event handler
///
/// INTENTION: Provide a sharable type similar to ActionHandler that can be referenced
/// by multiple subscribers and cloned as needed. This fixes lifetime issues by using Arc.
pub type EventHandler = Arc<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Import Future trait for use in type definition
use std::future::Future;

/// Future returned by service operations
pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>>;

/// Type for event subscription callbacks
pub type EventSubscriber = Arc<dyn Fn(Arc<EventContext>, Option<ValueType>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Type for action registration function
pub type ActionRegistrar = Arc<dyn Fn(&str, &str, ActionHandler, Option<ActionMetadata>) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

/// Service registry for managing services and their handlers
///
/// INTENTION: Provide a centralized registry for action handlers and event subscriptions.
/// This ensures consistent handling of service operations and enables service routing.
///
/// ARCHITECTURAL PRINCIPLE:
/// Service discovery and routing should be centralized for consistency and
/// to ensure proper service isolation.
pub struct ServiceRegistry {
    /// Local action handlers organized by path
    local_action_handlers: RwLock<HashMap<TopicPath, ActionHandler>>,
    
    /// Remote action handlers organized by path, with multiple handlers per path
    remote_action_handlers: RwLock<HashMap<TopicPath, Vec<ActionHandler>>>,
    
    /// Legacy action handlers organized by network and then by path 
    /// Will be deprecated once migration is complete
    action_handlers: RwLock<HashMap<String, HashMap<String, ActionHandler>>>,
    
    /// Local event subscriptions using wildcard registry
    local_event_subscriptions: RwLock<WildcardSubscriptionRegistry<(String, EventCallback)>>,
    
    /// Remote event subscriptions using wildcard registry
    remote_event_subscriptions: RwLock<WildcardSubscriptionRegistry<(String, EventCallback)>>,
    
    /// REMOVED - dont remote.. just remore change codfe that is using it
    //event_subscriptions: RwLock<WildcardSubscriptionRegistry<(String, EventCallback)>>,
    
    /// Local services registry
    local_services: RwLock<HashMap<String, Arc<dyn AbstractService>>>,
    
    /// Remote services registry indexed by service path
    remote_services: RwLock<HashMap<TopicPath, Vec<Arc<RemoteService>>>>,
    
    /// Legacy remote services registry - will be deprecated
    legacy_remote_services: RwLock<HashMap<String, Vec<Arc<RemoteService>>>>,
    
    /// Remote services registry indexed by peer ID
    peer_services: RwLock<HashMap<PeerId, HashSet<String>>>,
    
    /// Service lifecycle states - moved from Node
    service_states: Arc<RwLock<HashMap<String, ServiceState>>>,
    
    /// Logger instance
    logger: Logger,
}

impl Clone for ServiceRegistry {
    fn clone(&self) -> Self {
        // Note: We create new RwLocks with new HashMaps inside
        // WARNING: This implementation CREATES EMPTY REGISTRY MAPS
        // This means that any handlers registered on the original registry
        // will NOT be available in the cloned registry
        debug!("WARNING: ServiceRegistry clone was called - creating empty registry!");
        
        ServiceRegistry {
            local_action_handlers: RwLock::new(HashMap::new()),
            remote_action_handlers: RwLock::new(HashMap::new()),
            action_handlers: RwLock::new(HashMap::new()),
            local_event_subscriptions: RwLock::new(WildcardSubscriptionRegistry::new()),
            remote_event_subscriptions: RwLock::new(WildcardSubscriptionRegistry::new()), 
            local_services: RwLock::new(HashMap::new()),
            remote_services: RwLock::new(HashMap::new()),
            legacy_remote_services: RwLock::new(HashMap::new()),
            peer_services: RwLock::new(HashMap::new()),
            service_states: Arc::new(RwLock::new(HashMap::new())),
            logger: self.logger.clone(),
        }
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new_with_default_logger()
    }
}

impl ServiceRegistry {
    /// Create a new registry with a provided logger
    ///
    /// INTENTION: Initialize a new registry with a logger provided by the parent
    /// component (typically the Node). This ensures proper logger hierarchy.
    pub fn new(logger: Logger) -> Self {
        Self {
            local_action_handlers: RwLock::new(HashMap::new()),
            remote_action_handlers: RwLock::new(HashMap::new()),
            action_handlers: RwLock::new(HashMap::new()),
            local_event_subscriptions: RwLock::new(WildcardSubscriptionRegistry::new()),
            remote_event_subscriptions: RwLock::new(WildcardSubscriptionRegistry::new()),
             
            local_services: RwLock::new(HashMap::new()),
            remote_services: RwLock::new(HashMap::new()),
            legacy_remote_services: RwLock::new(HashMap::new()),
            peer_services: RwLock::new(HashMap::new()),
            service_states: Arc::new(RwLock::new(HashMap::new())),
            logger,
        }
    }
    
    /// Create a new registry with a default root logger
    ///
    /// INTENTION: Create a registry with a default logger when no parent logger
    /// is available. This is primarily used for testing or standalone usage.
    pub fn new_with_default_logger() -> Self {
        Self::new(Logger::new_root(runar_common::Component::Registry, "global"))
    }
    
    /// Register a local service
    ///
    /// INTENTION: Register a local service implementation for use by the node.
    pub async fn register_local_service(&self, service: Arc<dyn AbstractService>) -> Result<()> {
        let service_path = service.path().to_string();
        self.logger.info(format!("Registering local service: {}", service_path));
        
        // Store the service in the local services registry
        self.local_services.write().await.insert(service_path, service);
        
        Ok(())
    }
    
    /// Register a remote service
    ///
    /// INTENTION: Register a service that exists on a remote node, making it available for local requests.
    pub async fn register_remote_service(&self, service: Arc<RemoteService>) -> Result<()> {
        let service_path = service.path().to_string();
        let peer_id = service.peer_id().clone();
        
        self.logger.info(format!("Registering remote service: {} from peer: {}", service_path, peer_id));
        
        // Add to legacy remote services registry indexed by service path
        {
            let mut remote_services = self.legacy_remote_services.write().await;
            let services = remote_services.entry(service_path.clone()).or_insert_with(Vec::new);
            services.push(service.clone());
        }
        
        // Try to convert the service path to a TopicPath using the service's network ID
        match TopicPath::new(&service_path, service.network_id()) {
            Ok(topic_path) => {
                // Add to new remote services registry indexed by TopicPath
                let mut remote_services = self.remote_services.write().await;
                let services = remote_services.entry(topic_path).or_insert_with(Vec::new);
                services.push(service.clone());
            },
            Err(e) => {
                self.logger.warn(format!("Failed to create TopicPath for remote service: {}, error: {}", service_path, e));
            }
        }
        
        // Add to peer services registry indexed by peer ID
        {
            let mut peer_services = self.peer_services.write().await;
            let services = peer_services.entry(peer_id).or_insert_with(HashSet::new);
            services.insert(service_path);
        }
        
        Ok(())
    }
    
    /// Register a local action handler
    ///
    /// INTENTION: Register a handler for a specific action path that will be executed locally.
    pub async fn register_local_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        metadata: Option<ActionMetadata>
    ) -> Result<()> {
        self.logger.debug(format!("Registering local action handler for: {}", topic_path.as_str()));
        
        // Store in the new local action handlers map
        {
            let mut handlers = self.local_action_handlers.write().await;
            handlers.insert(topic_path.clone(), handler.clone());
        }
        
        // Also store in the legacy action handlers map (double storage during transition)
        {
            let mut handlers = self.action_handlers.write().await;
            let network_handlers = handlers
                .entry(topic_path.network_id().to_string())
                .or_insert_with(HashMap::new);
            
            network_handlers.insert(topic_path.action_path().to_string(), handler);
        }
        
        Ok(())
    }
    
    /// Register a remote action handler
    ///
    /// INTENTION: Register a handler for a specific action path that exists on a remote node.
    pub async fn register_remote_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        remote_service: Arc<RemoteService>
    ) -> Result<()> {
        self.logger.debug(format!("Registering remote action handler for: {}", topic_path.as_str()));
        
        // Store the handler in remote_action_handlers
        {
            let mut handlers = self.remote_action_handlers.write().await;
            let entry = handlers.entry(topic_path.clone()).or_insert_with(Vec::new);
            entry.push(handler.clone());
        }
        
        // Store the remote service reference if needed
        {
            let mut services = self.remote_services.write().await;
            let entry = services.entry(topic_path.clone()).or_insert_with(Vec::new);
            if !entry.iter().any(|s| s.peer_id() == remote_service.peer_id()) {
                entry.push(remote_service);
            }
        }
        
        // Also store in the legacy action handlers map (double storage during transition)
        {
            let mut handlers = self.action_handlers.write().await;
            let network_handlers = handlers
                .entry(topic_path.network_id().to_string())
                .or_insert_with(HashMap::new);
            
            network_handlers.insert(topic_path.action_path().to_string(), handler);
        }
        
        Ok(())
    }
    
    /// Get a local action handler only
    ///
    /// INTENTION: Retrieve a handler for a specific action path that will be executed locally.
    pub async fn get_local_action_handler(&self, topic_path: &TopicPath) -> Option<ActionHandler> {
        let handlers = self.local_action_handlers.read().await;
        handlers.get(topic_path).cloned()
    }
    
    /// Get all remote action handlers for a path (for load balancing)
    ///
    /// INTENTION: Retrieve all handlers for a specific action path that exist on remote nodes.
    pub async fn get_remote_action_handlers(&self, topic_path: &TopicPath) -> Vec<ActionHandler> {
        let handlers = self.remote_action_handlers.read().await;
        handlers.get(topic_path).cloned().unwrap_or_default()
    }
    
    /// Get an action handler for a specific topic path
    ///
    /// INTENTION: Look up the appropriate action handler for a given topic path
    /// supporting both local and remote handlers.
    pub async fn get_action_handler(&self, topic_path: &TopicPath) -> Option<ActionHandler> {
        // First try local handlers
        if let Some(handler) = self.local_action_handlers.read().await.get(topic_path) {
            return Some(handler.clone());
        }
        
        // Then check remote handlers
        let remote_handlers = self.remote_action_handlers.read().await;
        if let Some(handlers) = remote_handlers.get(topic_path) {
            if !handlers.is_empty() {
                // For backward compatibility, just return the first one
                // The Node will apply proper load balancing when using get_remote_action_handlers directly
                return Some(handlers[0].clone());
            }
        }
        
        None
    }
    
    /// Stable API - DO NOT CHANGE UNLES ASKED EXPLICITLY!
    /// Register local event subscription
    ///
    /// INTENTION: Register a callback to be invoked when events are published locally.
    pub async fn register_local_event_subscription(
        &self, 
        topic_path: &TopicPath,
        callback: EventCallback,
        options: SubscriptionOptions
    ) -> Result<String> {
        let subscription_id = Uuid::new_v4().to_string();
        //TODO handle optoion
        // Store in local event subscriptions
        {
            let mut subscriptions = self.local_event_subscriptions.write().await;
            subscriptions.add(topic_path.clone(), (subscription_id.clone(), callback.clone()));
        }
         
        Ok(subscription_id)
    }
    
    /// Register remote event subscription
    ///
    /// INTENTION: Register a callback to be invoked when events are published from remote nodes.
    pub async fn register_remote_event_subscription(
        &self, 
        topic_path: &TopicPath,
        callback: EventCallback
    ) -> Result<String> {
        let subscription_id = Uuid::new_v4().to_string();
        
        // Store in remote event subscriptions
        {
            let mut subscriptions = self.remote_event_subscriptions.write().await;
            subscriptions.add(topic_path.clone(), (subscription_id.clone(), callback.clone()));
        }
         
        Ok(subscription_id)
    }
    
    /// Get local event subscribers
    ///
    /// INTENTION: Find all local subscribers for a specific event topic.
    pub async fn get_local_event_subscribers(&self, topic_path: &TopicPath) -> Vec<(String, EventCallback)> {
        let subscriptions = self.local_event_subscriptions.read().await;
        subscriptions.find_matches(topic_path)
    }
    
    /// Get remote event subscribers
    ///
    /// INTENTION: Find all remote subscribers for a specific event topic.
    pub async fn get_remote_event_subscribers(&self, topic_path: &TopicPath) -> Vec<(String, EventCallback)> {
        let subscriptions = self.remote_event_subscriptions.read().await;
        subscriptions.find_matches(topic_path)
    }
     
     //FIX: this mnethods should reveive a topiPAth instead of a string for service_path
    /// Update service state
    ///
    /// INTENTION: Track the lifecycle state of a service.
    pub async fn update_service_state(&self, service_path: &str, state: ServiceState) -> Result<()> {
        self.logger.debug(format!("Updating service state for {}: {:?}", service_path, state));
        let mut states = self.service_states.write().await;
        states.insert(service_path.to_string(), state);
        Ok(())
    }
    
    //FIX rename to get_all_service_states
    /// Get all service states
    ///
    /// INTENTION: Retrieve the current state of all services.
    pub async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states.read().await.clone()
    }
    
    //REMOVED DONT PUT BACK - any code tha breaks need to be changed
    // pub async fn get_all_action_handlers(&self, topic_path: &TopicPath) -> Vec<ActionHandler> {
    
    /// Get all local services
    ///
    /// INTENTION: Provide access to all registered local services, allowing the
    /// Node to directly interact with them for lifecycle operations like initialization,
    /// starting, and stopping. This preserves the Node's responsibility for service
    /// lifecycle management while keeping the Registry focused on registration.
    pub async fn get_local_services(&self) -> HashMap<String, Arc<dyn AbstractService>> {
        self.local_services.read().await.clone()
    }
    
    /// Unsubscribe from a local event subscription
    ///
    /// INTENTION: Remove a specific subscription by ID from the local event subscriptions.
    pub async fn unsubscribe_local(&self, topic: &str, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!("Unsubscribing from local topic: {} with ID: {}", topic, subscription_id));
        
        // Convert string to TopicPath with default network for compatibility
        if let Ok(topic_path) = TopicPath::new(topic, "default") {
            let mut local_subscriptions = self.local_event_subscriptions.write().await;
            
            // Remove the specific subscription using the predicate function
            let removed = local_subscriptions.remove_handler(&topic_path, |handler| {
                let (id, _) = handler;
                id == subscription_id
            });
             
            if removed {
                self.logger.debug(format!("Successfully unsubscribed from topic: {} with ID: {}", topic, subscription_id));
                Ok(())
            } else {
                let msg = format!("No subscription found for topic: {} with ID: {}", topic, subscription_id);
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }
        } else {
            let msg = format!("Invalid topic path: {}", topic);
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }
    }
    
    /// Unsubscribe from a remote event subscription
    ///
    /// INTENTION: Remove a specific subscription by ID from the remote event subscriptions.
    pub async fn unsubscribe_remote(&self, topic: &str, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!("Unsubscribing from remote topic: {} with ID: {}", topic, subscription_id));
        
        // Convert string to TopicPath with default network for compatibility
        if let Ok(topic_path) = TopicPath::new(topic, "default") {
            let mut remote_subscriptions = self.remote_event_subscriptions.write().await;
            
            // Remove the specific subscription using the predicate function
            let removed = remote_subscriptions.remove_handler(&topic_path, |handler| {
                let (id, _) = handler;
                id == subscription_id
            });
             
            if removed {
                self.logger.debug(format!("Successfully unsubscribed from remote topic: {} with ID: {}", topic, subscription_id));
                Ok(())
            } else {
                let msg = format!("No remote subscription found for topic: {} with ID: {}", topic, subscription_id);
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }
        } else {
            let msg = format!("Invalid topic path: {}", topic);
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }
    }
    
}

#[async_trait::async_trait]
impl crate::services::RegistryDelegate for ServiceRegistry {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states.read().await.clone()
    }
    
    /// Get metadata for a specific service
    async fn get_service_metadata(&self, service_path: &str) -> Option<CompleteServiceMetadata> {
        // First try looking up a local service
        let services = self.local_services.read().await;
        if let Some(service) = services.get(service_path) {
            let states = self.service_states.read().await;
            let state = states.get(service_path).cloned().unwrap_or(ServiceState::Unknown);
            
            // Create timestamp for registration
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            // Create metadata using individual getter methods
            return Some(CompleteServiceMetadata {
                name: service.name().to_string(),
                path: service_path.to_string(),
                version: service.version().to_string(),
                description: service.description().to_string(),
                registered_actions: HashMap::new(), // Would need more complex logic to populate
                registered_events: HashMap::new(),  // Would need more complex logic to populate
                current_state: state,
                registration_time: now,
                last_start_time: None,
            });
        }
        
        // If not found, it might be a remote service - this would need to be implemented
        // based on how remote service metadata is stored
        None
    }
    
    /// Get metadata for all registered services
    async fn get_all_service_metadata(&self) -> HashMap<String, CompleteServiceMetadata> {
        // For now, just return local services
        let mut result = HashMap::new();
        let services = self.local_services.read().await;
        let states = self.service_states.read().await;
        
        // Create timestamp for registration
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        for (path, service) in services.iter() {
            let state = states.get(path).cloned().unwrap_or(ServiceState::Unknown);
            
            // Create metadata using individual getter methods
            result.insert(path.clone(), CompleteServiceMetadata {
                name: service.name().to_string(),
                path: path.clone(),
                version: service.version().to_string(),
                description: service.description().to_string(),
                registered_actions: HashMap::new(), // Would need more complex logic to populate
                registered_events: HashMap::new(),  // Would need more complex logic to populate
                current_state: state,
                registration_time: now,
                last_start_time: None,
            });
        }
        
        result
    }
    
    /// List all services
    fn list_services(&self) -> Vec<String> {
        // For now, just return local services
        // This would need to add remote services in a real implementation
        let services = match self.local_services.try_read() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        
        services.keys().cloned().collect()
    }
    
    /// Register a remote action handler
    async fn register_remote_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        remote_service: Arc<RemoteService>
    ) -> Result<()> {
        // This is just a proxy to the instance method
        self.register_remote_action_handler(topic_path, handler, remote_service).await
    }
} 