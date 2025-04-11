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
use crate::routing::{TopicPath, PathTrie};
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

/// Enum to distinguish between local and remote items
pub enum LocationType {
    Local,
    Remote,
}

#[derive(Clone)]
pub struct ServiceEntry {
    /// The service instance
    pub service: Arc<dyn AbstractService>,
    /// service topic path
    pub service_topic: TopicPath,
    //service state
    pub service_state: ServiceState,
}

impl std::fmt::Debug for ServiceEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEntry")
            .field("name", &self.service.name())
            .field("path", &self.service.path())
            .field("version", &self.service.version())
            .field("description", &self.service.description())
            .field("state", &self.service_state)
            .field("topic", &self.service_topic)
            .finish()
    }
}

/// Service registry for managing services and their handlers
///
/// INTENTION: Provide a centralized registry for action handlers and event subscriptions.
/// This ensures consistent handling of service operations and enables service routing.
///
/// ARCHITECTURAL PRINCIPLE:
/// Service discovery and routing should be centralized for consistency and
/// to ensure proper service isolation.
pub struct ServiceRegistry {
    /// Local action handlers organized by path (using PathTrie instead of HashMap)
    local_action_handlers: RwLock<PathTrie<ActionHandler>>,
    
    /// Remote action handlers organized by path (using PathTrie instead of HashMap)
    remote_action_handlers: RwLock<PathTrie<Vec<ActionHandler>>>,
     
    /// Local event subscriptions (using PathTrie instead of WildcardSubscriptionRegistry)
    local_event_subscriptions: RwLock<PathTrie<Vec<(String, EventCallback)>>>,
    
    /// Remote event subscriptions (using PathTrie instead of WildcardSubscriptionRegistry)
    remote_event_subscriptions: RwLock<PathTrie<Vec<(String, EventCallback)>>>,
    
    /// Map subscription IDs back to TopicPath for efficient unsubscription
    /// (Single HashMap for both local and remote subscriptions)
    subscription_id_to_topic_path: RwLock<HashMap<String, TopicPath>>,
    
    /// Local services registry (using PathTrie instead of HashMap)
    local_services: RwLock<PathTrie<Arc<ServiceEntry>>>,
    
    /// Remote services registry (using PathTrie instead of HashMap)
    remote_services: RwLock<PathTrie<Vec<Arc<RemoteService>>>>,
     
    /// Remote services registry indexed by peer ID
    remote_services_by_peer: RwLock<HashMap<PeerId, HashSet<String>>>,
    
    /// Service lifecycle states - moved from Node
    service_states_by_service_path: Arc<RwLock<HashMap<String, ServiceState>>>,
    
    /// Logger instance
    logger: Logger,
}

impl Clone for ServiceRegistry {
    fn clone(&self) -> Self {
        // Note: We create new RwLocks with new PathTrie inside
        // WARNING: This implementation CREATES EMPTY REGISTRY MAPS
        // This means that any handlers registered on the original registry
        // will NOT be available in the cloned registry
        debug!("WARNING: ServiceRegistry clone was called - creating empty registry!");
        
        ServiceRegistry {
            local_action_handlers: RwLock::new(PathTrie::new()),
            remote_action_handlers: RwLock::new(PathTrie::new()), 
            local_event_subscriptions: RwLock::new(PathTrie::new()),
            remote_event_subscriptions: RwLock::new(PathTrie::new()),
            subscription_id_to_topic_path: RwLock::new(HashMap::new()),
            local_services: RwLock::new(PathTrie::new()),
            remote_services: RwLock::new(PathTrie::new()), 
            remote_services_by_peer: RwLock::new(HashMap::new()),
            service_states_by_service_path: Arc::new(RwLock::new(HashMap::new())),
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
            local_action_handlers: RwLock::new(PathTrie::new()),
            remote_action_handlers: RwLock::new(PathTrie::new()), 
            local_event_subscriptions: RwLock::new(PathTrie::new()),
            remote_event_subscriptions: RwLock::new(PathTrie::new()),
            subscription_id_to_topic_path: RwLock::new(HashMap::new()),
            local_services: RwLock::new(PathTrie::new()),
            remote_services: RwLock::new(PathTrie::new()), 
            remote_services_by_peer: RwLock::new(HashMap::new()),
            service_states_by_service_path: Arc::new(RwLock::new(HashMap::new())),
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
    pub async fn register_local_service(&self, service: Arc<ServiceEntry>) -> Result<()> {
        let service_entry = service.clone();
        let service_topic = service_entry.service_topic.clone();
        self.logger.info(format!("Registering local service: {}", service_topic));
        
        // Store the service in the local services registry
        self.local_services.write().await.add_handler(service_topic, service);
          
        self.update_service_state(&service_entry.service.path(), service_entry.service_state).await?;

        Ok(())
    }
    
    /// Register a remote service
    ///
    /// INTENTION: Register a service that exists on a remote node, making it available for local requests.
    pub async fn register_remote_service(&self, service: Arc<RemoteService>) -> Result<()> {
        let service_topic = service.service_topic.clone();
        let service_path  = service.path().to_string();
        let peer_id = service.peer_id().clone();
        
        self.logger.info(format!("Registering remote service: {} from peer: {}", service_path, peer_id));
         
        // Add to peer services registry indexed by peer ID
        {
            let mut peer_services = self.remote_services_by_peer.write().await;
            let services = peer_services.entry(peer_id).or_insert_with(HashSet::new);
            services.insert(service_path);
        }

        // Add to remote services using PathTrie
        {
            let mut services = self.remote_services.write().await;
            let matches = services.find_matches(&service_topic);
            
            if matches.is_empty() {
                // No existing services for this topic
                services.add_handler(service_topic, vec![service]);
            } else {
                // Get the existing services for this topic
                let mut existing_services = matches[0].clone();
                
                // Check if this service is already registered
                if !existing_services.iter().any(|s| s.path() == service.path()) {
                    existing_services.push(service);
                    
                    // Update the services in the trie
                    services.add_handler(service_topic, existing_services);
                }
            }
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
        self.logger.debug(format!("Registering local action handler for: {}", topic_path ));
        
        // Store in the new local action handlers trie
        self.local_action_handlers.write().await.add_handler(topic_path.clone(), handler);
          
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
        
        // Store the handler in remote_action_handlers using PathTrie
        {
            let mut handlers_trie = self.remote_action_handlers.write().await;
            let matches = handlers_trie.find_matches(topic_path);
            
            if matches.is_empty() {
                // No handlers yet for this path
                handlers_trie.add_handler(topic_path.clone(), vec![handler.clone()]);
            } else {
                // Get existing handlers and add the new one
                let mut existing_handlers = matches[0].clone();
                existing_handlers.push(handler.clone());
                
                // Update the handlers in the trie
                handlers_trie.add_handler(topic_path.clone(), existing_handlers);
            }
        }
        
        // Store the remote service reference if needed
        {
            let mut services = self.remote_services.write().await;
            let matches = services.find_matches(topic_path);
            
            if matches.is_empty() {
                // No existing services for this topic
                services.add_handler(topic_path.clone(), vec![remote_service]);
            } else {
                // Get the existing services for this topic
                let mut existing_services = matches[0].clone();
                
                // Check if this service is already registered
                if !existing_services.iter().any(|s| s.peer_id() == remote_service.peer_id()) {
                    existing_services.push(remote_service);
                    
                    // Update the services in the trie
                    services.add_handler(topic_path.clone(), existing_services);
                }
            }
        }
         
        Ok(())
    }
    
    /// Get a local action handler only
    ///
    /// INTENTION: Retrieve a handler for a specific action path that will be executed locally.
    pub async fn get_local_action_handler(&self, topic_path: &TopicPath) -> Option<ActionHandler> {
        let handlers_trie = self.local_action_handlers.read().await;
        let matches = handlers_trie.find_matches(topic_path);
        
        if !matches.is_empty() {
            Some(matches[0].clone())
        } else {
            None
        }
    }
    
    /// Get all remote action handlers for a path (for load balancing)
    ///
    /// INTENTION: Retrieve all handlers for a specific action path that exist on remote nodes.
    pub async fn get_remote_action_handlers(&self, topic_path: &TopicPath) -> Vec<ActionHandler> {
        let handlers_trie = self.remote_action_handlers.read().await;
        let matches = handlers_trie.find_matches(topic_path);
        
        if !matches.is_empty() {
            matches[0].clone()
        } else {
            Vec::new()
        }
    }
    
    /// Get an action handler for a specific topic path
    ///
    /// INTENTION: Look up the appropriate action handler for a given topic path
    /// supporting both local and remote handlers.
    pub async fn get_action_handler(&self, topic_path: &TopicPath) -> Option<ActionHandler> {
        // First try local handlers
        if let Some(handler) = self.get_local_action_handler(topic_path).await {
            return Some(handler);
        }
        
        // Then check remote handlers
        let remote_handlers = self.get_remote_action_handlers(topic_path).await;
        if !remote_handlers.is_empty() {
            // For backward compatibility, just return the first one
            // The Node will apply proper load balancing when using get_remote_action_handlers directly
            return Some(remote_handlers[0].clone());
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
        //TODO handle option
        
        // Store in local event subscriptions using PathTrie
        {
            let mut subscriptions = self.local_event_subscriptions.write().await;
            let matches = subscriptions.find_matches(topic_path);
            
            if matches.is_empty() {
                // No existing subscriptions for this topic
                subscriptions.add_handler(topic_path.clone(), vec![(subscription_id.clone(), callback.clone())]);
            } else {
                // Add to existing subscriptions
                let mut existing_subscriptions = matches[0].clone();
                existing_subscriptions.push((subscription_id.clone(), callback.clone()));
                
                // Update the subscriptions in the trie
                subscriptions.add_handler(topic_path.clone(), existing_subscriptions);
            }
        }
        
        // Store the mapping from ID to TopicPath in the combined HashMap
        {
            let mut id_map = self.subscription_id_to_topic_path.write().await;
            id_map.insert(subscription_id.clone(), topic_path.clone());
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
        
        // Store in remote event subscriptions using PathTrie
        {
            let mut subscriptions = self.remote_event_subscriptions.write().await;
            let matches = subscriptions.find_matches(topic_path);
            
            if matches.is_empty() {
                // No existing subscriptions for this topic
                subscriptions.add_handler(topic_path.clone(), vec![(subscription_id.clone(), callback.clone())]);
            } else {
                // Add to existing subscriptions
                let mut existing_subscriptions = matches[0].clone();
                existing_subscriptions.push((subscription_id.clone(), callback.clone()));
                
                // Update the subscriptions in the trie
                subscriptions.add_handler(topic_path.clone(), existing_subscriptions);
            }
        }
        
        // Store the mapping from ID to TopicPath in the combined HashMap
        {
            let mut id_map = self.subscription_id_to_topic_path.write().await;
            id_map.insert(subscription_id.clone(), topic_path.clone());
        }
         
        Ok(subscription_id)
    }
    
    /// Get local event subscribers
    ///
    /// INTENTION: Find all local subscribers for a specific event topic.
    pub async fn get_local_event_subscribers(&self, topic_path: &TopicPath) -> Vec<(String, EventCallback)> {
        let subscriptions = self.local_event_subscriptions.read().await;
        let matches = subscriptions.find_matches(topic_path);
        
        // Flatten all matches into a single vector
        let mut result = Vec::new();
        for subscription_list in matches {
            result.extend(subscription_list.clone());
        }
        
        result
    }
    
    /// Get remote event subscribers
    ///
    /// INTENTION: Find all remote subscribers for a specific event topic.
    pub async fn get_remote_event_subscribers(&self, topic_path: &TopicPath) -> Vec<(String, EventCallback)> {
        let subscriptions = self.remote_event_subscriptions.read().await;
        let matches = subscriptions.find_matches(topic_path);
        
        // Flatten all matches into a single vector
        let mut result = Vec::new();
        for subscription_list in matches {
            result.extend(subscription_list.clone());
        }
        
        result
    }
     
     //FIX: this mnethods should reveive a topiPAth instead of a string for service_path
    /// Update service state
    ///
    /// INTENTION: Track the lifecycle state of a service.
    pub async fn update_service_state(&self, service_path: &str, state: ServiceState) -> Result<()> {
        self.logger.debug(format!("Updating service state for {}: {:?}", service_path, state));
        let mut states = self.service_states_by_service_path.write().await;
        states.insert(service_path.to_string(), state);
        Ok(())
    }
    
    //FIX rename to get_all_service_states
    /// Get all service states
    ///
    /// INTENTION: Retrieve the current state of all services.
    pub async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states_by_service_path.read().await.clone()
    }
    
    //REMOVED DONT PUT BACK - any code tha breaks need to be changed
    // pub async fn get_all_action_handlers(&self, topic_path: &TopicPath) -> Vec<ActionHandler> {
    
    /// Get all local services
    ///
    /// INTENTION: Provide access to all registered local services, allowing the
    /// Node to directly interact with them for lifecycle operations like initialization,
    /// starting, and stopping. This preserves the Node's responsibility for service
    /// lifecycle management while keeping the Registry focused on registration.
    pub async fn get_local_services(&self) -> HashMap<TopicPath, Arc<ServiceEntry>> {
        let trie = self.local_services.read().await;
        
        // Convert PathTrie results to the expected HashMap format
        let mut result = HashMap::new();
        
        // Use a dummy path to get all handlers - will be ignored in find_matches
        // since we're getting all entries
        let dummy_path = TopicPath::new_service("default", "dummy");
        
        // Find matches will return all handlers in the trie
        let services = trie.find_matches(&dummy_path);
        
        // Group services by their topic path
        for service in services {
            result.insert(service.service_topic.clone(), service);
        }
        
        result
    }
    
    /// Unsubscribe from an event subscription
    ///
    /// INTENTION: Remove a specific subscription by ID, providing a simpler API 
    /// that doesn't require the original topic.
    pub async fn unsubscribe_local(&self, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!("Attempting to unsubscribe local subscription ID: {}", subscription_id));
        
        // Find the TopicPath associated with the subscription ID
        let topic_path_option = {
            let id_map = self.subscription_id_to_topic_path.read().await;
            id_map.get(subscription_id).cloned()
        };
        
        if let Some(topic_path) = topic_path_option {
            self.logger.debug(format!("Found topic path '{}' for subscription ID: {}", topic_path.as_str(), subscription_id));
            let mut subscriptions = self.local_event_subscriptions.write().await;
            
            // Find current subscriptions for this topic
            let matches = subscriptions.find_matches(&topic_path);
            
            if !matches.is_empty() {
                // Get the first match (should be the only one for exact path)
                let mut updated_subscriptions = Vec::new();
                
                // Create a new list without the subscription we want to remove
                for (id, callback) in matches[0].clone() {
                    if id != subscription_id {
                        updated_subscriptions.push((id, callback));
                    }
                }
                
                // Remove the old list and add the updated one
                let removed = subscriptions.remove_handler(&topic_path, |_| true);
                
                if !updated_subscriptions.is_empty() {
                    // If we still have subscriptions for this topic, add them back
                    subscriptions.add_handler(topic_path.clone(), updated_subscriptions);
                }
                
                if removed {
                    // Also remove from the ID map
                    {
                        let mut id_to_topic_path_map = self.subscription_id_to_topic_path.write().await;
                        id_to_topic_path_map.remove(subscription_id);
                    }
                    self.logger.debug(format!("Successfully unsubscribed from topic: {} with ID: {}", topic_path.as_str(), subscription_id));
                    Ok(())
                } else {
                    // This case might happen if the subscription was already removed concurrently
                    let msg = format!("Subscription handler not found for topic path {} and ID {}, although ID was mapped. Potential race condition?", topic_path.as_str(), subscription_id);
                    self.logger.warn(msg.clone());
                    Err(anyhow!(msg))
                }
            } else {
                // No subscriptions found for this topic
                let msg = format!("No subscriptions found for topic path {} and ID {}", topic_path.as_str(), subscription_id);
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }
        } else {
            let msg = format!("No topic path found mapping to subscription ID: {}. Cannot unsubscribe.", subscription_id);
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }
    }
    
    /// Unsubscribe from a remote event subscription using only the subscription ID.
    ///
    /// INTENTION: Remove a specific subscription by ID from the remote event subscriptions,
    /// providing a simpler API that doesn't require the original topic.
    pub async fn unsubscribe_remote(&self, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!("Attempting to unsubscribe remote subscription ID: {}", subscription_id));
        
        // Find the TopicPath associated with the subscription ID
        let topic_path_option = {
            let id_map = self.subscription_id_to_topic_path.read().await;
            id_map.get(subscription_id).cloned()
        };
        
        if let Some(topic_path) = topic_path_option {
            self.logger.debug(format!("Found topic path '{}' for subscription ID: {}", topic_path.as_str(), subscription_id));
            let mut subscriptions = self.remote_event_subscriptions.write().await;
            
            // Find current subscriptions for this topic
            let matches = subscriptions.find_matches(&topic_path);
            
            if !matches.is_empty() {
                // Get the first match (should be the only one for exact path)
                let mut updated_subscriptions = Vec::new();
                
                // Create a new list without the subscription we want to remove
                for (id, callback) in matches[0].clone() {
                    if id != subscription_id {
                        updated_subscriptions.push((id, callback));
                    }
                }
                
                // Remove the old list and add the updated one
                let removed = subscriptions.remove_handler(&topic_path, |_| true);
                
                if !updated_subscriptions.is_empty() {
                    // If we still have subscriptions for this topic, add them back
                    subscriptions.add_handler(topic_path.clone(), updated_subscriptions);
                }
                
                if removed {
                    // Also remove from the ID map
                    {
                        let mut id_map = self.subscription_id_to_topic_path.write().await;
                        id_map.remove(subscription_id);
                    }
                    self.logger.debug(format!("Successfully unsubscribed from remote topic: {} with ID: {}", topic_path.as_str(), subscription_id));
                    Ok(())
                } else {
                    // This case might happen if the subscription was already removed concurrently
                    let msg = format!("Subscription handler not found for remote topic path {} and ID {}, although ID was mapped. Potential race condition?", topic_path.as_str(), subscription_id);
                    self.logger.warn(msg.clone());
                    Err(anyhow!(msg))
                }
            } else {
                // No subscriptions found for this topic
                let msg = format!("No subscriptions found for remote topic path {} and ID {}", topic_path.as_str(), subscription_id);
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }
        } else {
            let msg = format!("No topic path found mapping to remote subscription ID: {}. Cannot unsubscribe.", subscription_id);
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }
    }
    
}

#[async_trait::async_trait]
impl crate::services::RegistryDelegate for ServiceRegistry {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states_by_service_path.read().await.clone()
    }
    
    /// Get metadata for a specific service
    async fn get_service_metadata(&self, service_path: &TopicPath) -> Option<CompleteServiceMetadata> {
        // Find service in the local services trie
        let services = self.local_services.read().await;
        let matches = services.find_matches(service_path);
        
        if !matches.is_empty() {
            let service_entry = &matches[0];
            let service = service_entry.service.clone();
            let states = self.service_states_by_service_path.read().await;
            let service_path_str = service_path.service_path();
            let state = states.get(&service_path_str).cloned().unwrap_or(ServiceState::Unknown);
             
            // Create timestamp for registration
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            
            // Create metadata using individual getter methods
            return Some(CompleteServiceMetadata {
                name: service.name().to_string(),
                path: service.path().to_string(),
                version: service.version().to_string(),
                description: service.description().to_string(),
                //TODO
                registered_actions: HashMap::new(),  
                //TODO
                registered_events: HashMap::new(),  
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
        let states = self.service_states_by_service_path.read().await;
        
        // Create timestamp for registration
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        
        // Use a dummy path to get all handlers
        let dummy_path = TopicPath::new_service("default", "dummy");
        let service_entries = services.find_matches(&dummy_path);
        
        for service_entry in service_entries {
            let service = &service_entry.service;
            let path_str = service.path().to_string();
            let state = states.get(&path_str).cloned().unwrap_or(ServiceState::Unknown);
            
            // Create metadata using individual getter methods from the service
            result.insert(path_str, CompleteServiceMetadata {
                name: service.name().to_string(),
                path: service.path().to_string(),
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