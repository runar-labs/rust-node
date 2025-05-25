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

use crate::network::transport::PeerId;
use crate::routing::{PathTrie, TopicPath};
use std::str::FromStr;
use crate::services::abstract_service::{AbstractService, ServiceState};
use runar_common::types::schemas::{ActionMetadata, EventMetadata, ServiceMetadata};
use crate::services::{
    ActionHandler, EventContext, RegistryDelegate, RemoteService, ServiceResponse,
    SubscriptionOptions,
};
use runar_common::logging::Logger;
use runar_common::types::ArcValueType;

/// Type definition for event callbacks
///
/// INTENTION: Define a type that can handle event notifications asynchronously.
/// The callback takes an event context and payload and returns a future that
/// resolves once the event has been processed.
pub type EventCallback = Arc<
    dyn Fn(Arc<EventContext>, ArcValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Type definition for event handler
///
/// INTENTION: Provide a sharable type similar to ActionHandler that can be referenced
/// by multiple subscribers and cloned as needed. This fixes lifetime issues by using Arc.
pub type EventHandler = Arc<
    dyn Fn(Arc<EventContext>, ArcValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Import Future trait for use in type definition
use std::future::Future;

/// Future returned by service operations
pub type ServiceFuture = Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>>;

/// Type for event subscription callbacks
pub type EventSubscriber = Arc<
    dyn Fn(
            Arc<EventContext>,
            Option<ArcValueType>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Type for action registration function
pub type ActionRegistrar = Arc<
    dyn Fn(
            &str,
            &str,
            ActionHandler,
            Option<ActionMetadata>,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

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
    /// Timestamp when the service was registered (in seconds since UNIX epoch)
    pub registration_time: u64,
    /// Timestamp when the service was last started (in seconds since UNIX epoch)
    /// This is None if the service has never been started
    pub last_start_time: Option<u64>,
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
            .field("registration_time", &self.registration_time)
            .field("last_start_time", &self.last_start_time)
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
    /// Store both the handler and the original registration topic path for parameter extraction
    local_action_handlers: RwLock<PathTrie<(ActionHandler, TopicPath, Option<ActionMetadata>)>>,

    //reverse index where we store the events that a service listens to
    local_events_by_service: RwLock<PathTrie<Vec<EventMetadata>>>,

    /// Remote action handlers organized by path (using PathTrie instead of HashMap)
    remote_action_handlers: RwLock<PathTrie<Vec<ActionHandler>>>,

    /// Local event subscriptions (using PathTrie instead of WildcardSubscriptionRegistry)
    local_event_subscriptions: RwLock<PathTrie<Vec<(String, EventCallback, Option<EventMetadata>)>>>,

    /// Remote event subscriptions (using PathTrie instead of WildcardSubscriptionRegistry)
    remote_event_subscriptions: RwLock<PathTrie<Vec<(String, EventCallback)>>>,

    /// Map subscription IDs back to TopicPath for efficient unsubscription
    /// (Single HashMap for both local and remote subscriptions)
    subscription_id_to_topic_path: RwLock<HashMap<String, TopicPath>>,

    /// Map subscription IDs back to the service TopicPath for efficient unsubscription
    subscription_id_to_service_topic_path: RwLock<HashMap<String, TopicPath>>,

    /// Local services registry (using PathTrie instead of HashMap)
    local_services: RwLock<PathTrie<Arc<ServiceEntry>>>,

    local_services_list: RwLock<HashMap<TopicPath, Arc<ServiceEntry>>>,
    
    /// Remote services registry (using PathTrie instead of HashMap)
    remote_services: RwLock<PathTrie<Vec<Arc<RemoteService>>>>,

    /// Remote services registry indexed by peer ID
    remote_services_by_peer: RwLock<HashMap<PeerId, HashSet<String>>>,

    /// Service lifecycle states - moved from Node
    service_states_by_service_path: Arc<RwLock<HashMap<String, ServiceState>>>,

    /// Logger instance
    logger: Arc<Logger>,
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
            local_events_by_service: RwLock::new(PathTrie::new()),
            remote_action_handlers: RwLock::new(PathTrie::new()),
            local_event_subscriptions: RwLock::new(PathTrie::new()),
            remote_event_subscriptions: RwLock::new(PathTrie::new()),
            subscription_id_to_topic_path: RwLock::new(HashMap::new()),
            subscription_id_to_service_topic_path: RwLock::new(HashMap::new()),
            local_services: RwLock::new(PathTrie::new()),
            local_services_list: RwLock::new(HashMap::new()),
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
    pub fn new(logger: Arc<Logger>) -> Self {
        Self {
            local_action_handlers: RwLock::new(PathTrie::new()),
            local_events_by_service: RwLock::new(PathTrie::new()),
            remote_action_handlers: RwLock::new(PathTrie::new()),
            local_event_subscriptions: RwLock::new(PathTrie::new()),
            remote_event_subscriptions: RwLock::new(PathTrie::new()),
            subscription_id_to_topic_path: RwLock::new(HashMap::new()),
            subscription_id_to_service_topic_path: RwLock::new(HashMap::new()),
            local_services: RwLock::new(PathTrie::new()),
            local_services_list: RwLock::new(HashMap::new()),
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
        Self::new(Arc::new(Logger::new_root(
            runar_common::Component::Registry,
            "global",
        )))
    }

    /// Register a local service
    ///
    /// INTENTION: Register a local service implementation for use by the node.
    pub async fn register_local_service(&self, service: Arc<ServiceEntry>) -> Result<()> {
        let service_entry = service.clone();
        let service_topic = service_entry.service_topic.clone();
        self.logger
            .info(format!("Registering local service: {}", service_topic));

        // Store the service in the local services registry
        self.local_services
            .write()
            .await
            .add_content(service_topic.clone(), service);

        self.update_service_state(&service_entry.service.path(), service_entry.service_state)
            .await?;

        self.local_services_list
            .write()
            .await
            .insert(service_topic, service_entry.clone());

        Ok(())
    }

    /// Register a remote service
    ///
    /// INTENTION: Register a service that exists on a remote node, making it available for local requests.
    pub async fn register_remote_service(&self, service: Arc<RemoteService>) -> Result<()> {
        let service_topic = service.service_topic.clone();
        let service_path = service.path().to_string();
        let peer_id = service.peer_id().clone();

        self.logger.info(format!(
            "Registering remote service: {} from peer: {}",
            service_path, peer_id
        ));

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
                services.add_content(service_topic, vec![service]);
            } else {
                // Get the existing services for this topic
                let mut existing_services = matches[0].content.clone();

                // Check if this service is already registered
                if !existing_services.iter().any(|s| s.path() == service.path()) {
                    existing_services.push(service);

                    // Update the services in the trie
                    services.add_content(service_topic, existing_services);
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
        metadata: Option<ActionMetadata>,
    ) -> Result<()> {
        self.logger.debug(format!(
            "Registering local action handler for: {}",
            topic_path
        ));

        // Store in the new local action handlers trie with the original topic path for parameter extraction
        self.local_action_handlers
            .write()
            .await
            .add_content(topic_path.clone(), (handler, topic_path.clone(), metadata));

        Ok(())
    }

    /// Register a remote action handler
    ///
    /// INTENTION: Register a handler for a specific action path that exists on a remote node.
    pub async fn register_remote_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        remote_service: Arc<RemoteService>,
    ) -> Result<()> {
        self.logger.debug(format!(
            "Registering remote action handler for: {}",
            topic_path.as_str()
        ));

        // Store the handler in remote_action_handlers using PathTrie
        {
            let mut handlers_trie = self.remote_action_handlers.write().await;
            let matches = handlers_trie.find_matches(topic_path);

            if matches.is_empty() {
                // No handlers yet for this path
                handlers_trie.add_content(topic_path.clone(), vec![handler.clone()]);
            } else {
                // Get existing handlers and add the new one
                let mut existing_handlers = matches[0].content.clone();
                existing_handlers.push(handler.clone());

                // Update the handlers in the trie
                handlers_trie.add_content(topic_path.clone(), existing_handlers);
            }
        }

        // Store the remote service reference if needed
        {
            let service_topic_path = TopicPath::new(
                &topic_path.service_path(),
                &topic_path.network_id()
            ).map_err(|e| anyhow::anyhow!("Failed to create service topic path: {}", e))?;
            let mut services = self.remote_services.write().await;
            let matches = services.find_matches(&service_topic_path);

            if matches.is_empty() {
                // No existing services for this topic
                services.add_content(service_topic_path.clone(), vec![remote_service]);
            } else {
                // Get the existing services for this topic
                let mut existing_services = matches[0].content.clone();

                // Check if this service is already registered
                if !existing_services
                    .iter()
                    .any(|s| s.peer_id() == remote_service.peer_id())
                {
                    existing_services.push(remote_service);

                    // Update the services in the trie
                    services.add_content(service_topic_path.clone(), existing_services);
                }
            }
        }

        Ok(())
    }

    /// Get a local action handler only
    ///
    /// INTENTION: Retrieve a handler for a specific action path that will be executed locally.
    /// Now returns both the handler and the original registration topic path for parameter extraction.
    pub async fn get_local_action_handler(
        &self,
        topic_path: &TopicPath,
    ) -> Option<(ActionHandler, TopicPath)> {
        let handlers_trie = self.local_action_handlers.read().await;
        let matches = handlers_trie.find_matches(topic_path);

        if !matches.is_empty() {
            let (handler, topic_path, _metadata) = matches[0].content.clone();
            return Some((handler, topic_path));
        } else {
            None
        }
    }

    /// Get all remote action handlers for a path (for load balancing)
    ///
    /// INTENTION: Retrieve all handlers for a specific action path that exist on remote nodes.
    /// Get all remote action handlers for a path (for load balancing)
    ///
    /// INTENTION: Retrieve all handlers for a specific action path that exist on remote nodes.
    /// Returns a flattened vector of all matching handlers across all matching topic patterns.
    pub async fn get_remote_action_handlers(&self, topic_path: &TopicPath) -> Vec<ActionHandler> {
        let handlers_trie = self.remote_action_handlers.read().await;
        let matches = handlers_trie.find_matches(topic_path);

        // Flatten all matches into a single vector of handlers
        matches.into_iter()
            .flat_map(|mat| mat.content.clone())
            .collect()
    }

    /// Get an action handler for a specific topic path
    ///
    /// INTENTION: Look up the appropriate action handler for a given topic path
    /// supporting both local and remote handlers.
    pub async fn get_action_handler(&self, topic_path: &TopicPath) -> Option<ActionHandler> {
        // First try local handlers
        if let Some((handler, _)) = self.get_local_action_handler(topic_path).await {
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
        metadata: Option<EventMetadata>,
    ) -> Result<String> {
        let subscription_id = Uuid::new_v4().to_string();

        // Store in local event subscriptions using PathTrie
        {
            let mut subscriptions = self.local_event_subscriptions.write().await;
            let matches = subscriptions.find_matches(topic_path);

            if matches.is_empty() {
                // No existing subscriptions for this topic
                subscriptions.add_content(
                    topic_path.clone(),
                    vec![(subscription_id.clone(), callback.clone(), metadata.clone())],
                );
            } else {
                // Add to existing subscriptions
                let mut updated_subscriptions = matches[0].content.clone();
                updated_subscriptions.push((subscription_id.clone(), callback.clone(), metadata.clone()));

                // First remove the old handlers, then add the updated ones
                subscriptions.remove_handler(topic_path, |_| true);
                subscriptions.add_content(topic_path.clone(), updated_subscriptions);
            }
        }

        // Store the mapping from ID to TopicPath in the combined HashMap
        {
            let mut id_map = self.subscription_id_to_topic_path.write().await;
            id_map.insert(subscription_id.clone(), topic_path.clone());
        }

        let service_topic = TopicPath::new(&topic_path.service_path(), &topic_path.network_id()).unwrap();
        
        // store metadata in local_events_by_service
        if metadata.is_some() {
            let metadata = metadata.unwrap();
            
            
            
            let mut events = self.local_events_by_service.write().await;
            let matches = events.find_matches(&service_topic);

            
            if matches.is_empty() {
                // No existing events for this service
                //add a vec with one item -> topic_path indexed by the source_service_path
                events.add_content(
                    service_topic.clone(),
                    vec![metadata],
                );
            } else {
                // Add to existing events
                let mut updated_events = matches[0].content.clone();
                updated_events.push(metadata);

                // First remove the old handlers, then add the updated ones
                events.remove_handler(&service_topic, |_| true);
                events.add_content(service_topic.clone(), updated_events);
            }
        }

        //store in subscription_id_to_service_topic_path
        {
            let mut id_map = self.subscription_id_to_service_topic_path.write().await;
            id_map.insert(subscription_id.clone(), service_topic.clone());
        }
        

        Ok(subscription_id)
    }

    /// Register remote event subscription
    ///
    /// INTENTION: Register a callback to be invoked when events are published from remote nodes.
    pub async fn register_remote_event_subscription(
        &self,
        topic_path: &TopicPath,
        callback: EventCallback,
    ) -> Result<String> {
        let subscription_id = Uuid::new_v4().to_string();

        // Store in remote event subscriptions using PathTrie
        {
            let mut subscriptions = self.remote_event_subscriptions.write().await;
            let matches = subscriptions.find_matches(topic_path);

            if matches.is_empty() {
                // No existing subscriptions for this topic
                subscriptions.add_content(
                    topic_path.clone(),
                    vec![(subscription_id.clone(), callback.clone())],
                );
            } else {
                // Add to existing subscriptions
                let mut updated_subscriptions = matches[0].content.clone();
                updated_subscriptions.push((subscription_id.clone(), callback.clone()));

                // First remove the old handlers, then add the updated ones
                subscriptions.remove_handler(topic_path, |_| true);
                subscriptions.add_content(topic_path.clone(), updated_subscriptions);
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
    pub async fn get_local_event_subscribers(
        &self,
        topic_path: &TopicPath,
    ) -> Vec<(String, EventCallback)> {
        let subscriptions = self.local_event_subscriptions.read().await;
        let matches = subscriptions.find_matches(topic_path);

        let mut result = Vec::new();
        for match_item in matches {
            // Each item in content is a (String, EventCallback, Option<EventMetadata>) tuple
            for (subscription_id, callback, _metadata) in match_item.content.clone() {
                result.push((subscription_id, callback));
            }
        }
        result
    }

    /// Get remote event subscribers
    ///
    /// INTENTION: Find all remote subscribers for a specific event topic.
    pub async fn get_remote_event_subscribers(
        &self,
        topic_path: &TopicPath,
    ) -> Vec<(String, EventCallback)> {
        let subscriptions = self.remote_event_subscriptions.read().await;
        let matches = subscriptions.find_matches(topic_path);

        // Flatten all matches into a single vector
        let mut result = Vec::new();
        for match_item in matches {
            // Each item in content is already a (String, EventCallback) tuple
            for (subscription_id, callback) in match_item.content.clone() {
                result.push((subscription_id, callback));
            }
        }
        result
    }

    //FIX: this mnethods should reveive a topiPAth instead of a string for service_path
    /// Update service state
    ///
    /// INTENTION: Track the lifecycle state of a service.
    pub async fn update_service_state(
        &self,
        service_path: &str,
        state: ServiceState,
    ) -> Result<()> {
        self.logger.debug(format!(
            "Updating service state for {}: {:?}",
            service_path, state
        ));
        let mut states = self.service_states_by_service_path.write().await;
        states.insert(service_path.to_string(), state);
        Ok(())
    }

    /// Get all service states
    ///
    /// INTENTION: Retrieve the current state of all services.
    pub async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states_by_service_path.read().await.clone()
    }

    
    /// Get metadata for all events under a specific service path
    ///
    /// INTENTION: Retrieve metadata for all events registered under a service path.
    /// This is useful for service discovery and introspection.
    pub async fn get_events_metadata(
        &self,
        search_path: &TopicPath,
    ) -> Vec<EventMetadata> {
 
        // Search in the events trie local_event_handlers
        let events = self.local_events_by_service.read().await;
        let matches = events.find_matches(&search_path);
        
        // Collect all events that match the service path
        let mut result = Vec::new();
        
        for match_item in matches {
            // Extract the topic path from the match
            let event_topic_list = &match_item.content;
            
            //iterate event_topic_list
            for event_metadata in event_topic_list {
                result.push(event_metadata.clone());
            } 
        }
        
        result
    }
    

    /// Get metadata for all actions under a specific service path
    ///
    /// INTENTION: Retrieve metadata for all actions registered under a service path.
    /// This is useful for service discovery and introspection.
    pub async fn get_actions_metadata(
        &self,
        search_path: &TopicPath,
    ) -> Vec<ActionMetadata> {
 
        // Search in the actions trie local_action_handlers
        let actions = self.local_action_handlers.read().await;
        let matches = actions.find_matches(&search_path);
        
        // Collect all actions that match the service path
        let mut result = Vec::new();
        
        for match_item in matches {
            // Extract the topic path from the match
            let (_, _, metadata) = &match_item.content;
            if let Some(metadata) = metadata {
                result.push(metadata.clone());
            }
        }
        
        result
    }

    /// Get all local services
    ///
    /// INTENTION: Provide access to all registered local services, allowing the
    /// Node to directly interact with them for lifecycle operations like initialization,
    /// starting, and stopping. This preserves the Node's responsibility for service
    /// lifecycle management while keeping the Registry focused on registration.
    pub async fn get_local_services(&self) -> HashMap<TopicPath, Arc<ServiceEntry>> {
        self.local_services_list.read().await.clone()
    }
    
    /// Get metadata for all events under a specific event path
    ///
    /// INTENTION: Retrieve metadata for all events registered under a service path.
    /// This is useful for service discovery and introspection.
    // pub async fn get_events_metadata_internal(
    //     &self,
    //     search_path: &TopicPath,
    // ) -> Vec<EventMetadata> {
    //     // Search in the events trie
    //     let event_subscriptions = self.local_event_subscriptions.read().await;
    //     let event_matches = event_subscriptions.find_matches(search_path);
        
    //     // Collect all events that match the service path
    //     let mut result = Vec::new();
    //     let mut seen_paths = HashSet::new();
        
    //     // For event subscriptions, we need to handle the Vec<(String, EventCallback)> structure
    //     for match_item in event_matches {
    //         // Each handler is a Vec of (subscription_id, callback) pairs
    //         for (_, _callback) in &match_item.handler {
    //             // We can use the match path since event handlers don't store the original topic path
    //             let path_str = search_path.as_str().replace("/*", "");
                
    //             // Only add each event path once, no need to add duplicates for multiple subscribers
    //             if !seen_paths.contains(&path_str) {
    //                 seen_paths.insert(path_str.clone());
                    
    //                 // Create metadata for this event
    //                 result.push(EventMetadata {
    //                     path: path_str.clone(),
    //                     description: format!("Event handler for {}", path_str),
    //                     data_schema: None, // We don't have schema information in the handler
    //                 });
    //             }
    //         }
    //     }
        
    //     result
    // }

    /// Helper method to get all network IDs from the registry
    /// 
    /// INTENTION: Collect all unique network IDs from registered services
    async fn get_all_network_ids(&self) -> Vec<String> {
        // Start with the default network
        let mut network_ids = vec!["default".to_string()];
        
        // Add network IDs from local services
        let local_service_states = self.service_states_by_service_path.read().await;
        
        // Extract network IDs from service paths
        for service_path in local_service_states.keys() {
            // Try to parse the service path to extract the network ID
            // Format is typically "network_id:service/path"
            if let Some(network_id) = service_path.split(':').next() {
                let network_id = network_id.to_string();
                if !network_ids.contains(&network_id) && !network_id.is_empty() {
                    network_ids.push(network_id);
                }
            }
        }
        
        network_ids
    }
    
    pub async fn unsubscribe_local(&self, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!(
            "Attempting to unsubscribe local subscription ID: {}",
            subscription_id
        ));

        // Find the TopicPath associated with the subscription ID
        let topic_path_option = {
            let id_map = self.subscription_id_to_topic_path.read().await;
            id_map.get(subscription_id).cloned()
        };

        if let Some(topic_path) = topic_path_option {
            self.logger.debug(format!(
                "Found topic path '{}' for subscription ID: {}",
                topic_path.as_str(),
                subscription_id
            ));
            let mut subscriptions = self.local_event_subscriptions.write().await;

            // Find current subscriptions for this topic
            let matches = subscriptions.find_matches(&topic_path);

            if !matches.is_empty() {
                // Get the first match (should be the only one for exact path)
                let mut updated_subscriptions = Vec::new();

                // Create a new list without the subscription we want to remove
                for (id, callback,metadata) in matches[0].content.clone() {
                    if id != subscription_id {
                        updated_subscriptions.push((id, callback, metadata));
                    }
                }

                // Remove the old list and add the updated one
                let removed = subscriptions.remove_handler(&topic_path, |_| true);

                if !updated_subscriptions.is_empty() {
                    // If we still have subscriptions for this topic, add them back
                    subscriptions.add_content(topic_path.clone(), updated_subscriptions);
                }

                if removed {
                    // Also remove from the ID map
                    {
                        let mut id_to_topic_path_map =
                            self.subscription_id_to_topic_path.write().await;
                        id_to_topic_path_map.remove(subscription_id);
                    }
                    self.logger.debug(format!(
                        "Successfully unsubscribed from topic: {} with ID: {}",
                        topic_path.as_str(),
                        subscription_id
                    ));
                    
                } else {
                    // This case might happen if the subscription was already removed concurrently
                    let msg = format!("Subscription handler not found for topic path {} and ID {}, although ID was mapped. Potential race condition?", topic_path.as_str(), subscription_id);
                    self.logger.warn(msg.clone());
                    return Err(anyhow!(msg))
                }

                //remove from service topic path map
                let service_topic_path = self.subscription_id_to_service_topic_path.read().await;
                let service_topic_path = service_topic_path.get(subscription_id).cloned();
        
                if let Some(service_topic_path) = service_topic_path {
                    self.logger.debug(format!(
                        "Unsubscribing from service topic : {} at service path: {}",
                        topic_path.as_str(),
                        service_topic_path.as_str()
                    ));
                    let mut events = self.local_events_by_service.write().await;
                    let events_by_service = events.find_matches(&service_topic_path);
                    if !events_by_service.is_empty() {
                        for match_item in events_by_service {  
                            let mut updated_list = Vec::new();
                            let items = match_item.content.clone();
                            for event_metadata in items {
                                if event_metadata.path == topic_path.as_str() {
                                    continue;
                                }
                                updated_list.push(event_metadata);
                            }
                            events.remove_handler(&service_topic_path, |_| true);
                            if !updated_list.is_empty() {
                                events.add_content(service_topic_path.clone(), updated_list);
                            }
                        }
                    }
                    

                }
                Ok(())
            } else {
                // No subscriptions found for this topic
                let msg = format!(
                    "No subscriptions found for topic path {} and ID {}",
                    topic_path.as_str(),
                    subscription_id
                );
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }

            
        } else {
            let msg = format!(
                "No topic path found mapping to subscription ID: {}. Cannot unsubscribe.",
                subscription_id
            );
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }


        
    }

    /// Unsubscribe from a remote event subscription using only the subscription ID.
    ///
    /// INTENTION: Remove a specific subscription by ID from the remote event subscriptions,
    /// providing a simpler API that doesn't require the original topic.
    pub async fn unsubscribe_remote(&self, subscription_id: &str) -> Result<()> {
        self.logger.debug(format!(
            "Attempting to unsubscribe remote subscription ID: {}",
            subscription_id
        ));

        // Find the TopicPath associated with the subscription ID
        let topic_path_option = {
            let id_map = self.subscription_id_to_topic_path.read().await;
            id_map.get(subscription_id).cloned()
        };

        if let Some(topic_path) = topic_path_option {
            self.logger.debug(format!(
                "Found topic path '{}' for subscription ID: {}",
                topic_path.as_str(),
                subscription_id
            ));
            let mut subscriptions = self.remote_event_subscriptions.write().await;

            // Find current subscriptions for this topic
            let matches = subscriptions.find_matches(&topic_path);

            if !matches.is_empty() {
                // Get the first match (should be the only one for exact path)
                let mut updated_subscriptions = Vec::new();

                // Create a new list without the subscription we want to remove
                for (id, callback) in matches[0].content.clone() {
                    if id != subscription_id {
                        updated_subscriptions.push((id, callback));
                    }
                }

                // Remove the old list and add the updated one
                let removed = subscriptions.remove_handler(&topic_path, |_| true);

                if !updated_subscriptions.is_empty() {
                    // If we still have subscriptions for this topic, add them back
                    subscriptions.add_content(topic_path.clone(), updated_subscriptions);
                }

                if removed {
                    // Also remove from the ID map
                    {
                        let mut id_map = self.subscription_id_to_topic_path.write().await;
                        id_map.remove(subscription_id);
                    }
                    self.logger.debug(format!(
                        "Successfully unsubscribed from remote topic: {} with ID: {}",
                        topic_path.as_str(),
                        subscription_id
                    ));
                    Ok(())
                } else {
                    // This case might happen if the subscription was already removed concurrently
                    let msg = format!("Subscription handler not found for remote topic path {} and ID {}, although ID was mapped. Potential race condition?", topic_path.as_str(), subscription_id);
                    self.logger.warn(msg.clone());
                    Err(anyhow!(msg))
                }
            } else {
                // No subscriptions found for this topic
                let msg = format!(
                    "No subscriptions found for remote topic path {} and ID {}",
                    topic_path.as_str(),
                    subscription_id
                );
                self.logger.warn(msg.clone());
                Err(anyhow!(msg))
            }
        } else {
            let msg = format!(
                "No topic path found mapping to remote subscription ID: {}. Cannot unsubscribe.",
                subscription_id
            );
            self.logger.warn(msg.clone());
            Err(anyhow!(msg))
        }
    }

    /// Get metadata for all services with an option to filter internal services
    ///
    /// INTENTION: Retrieve metadata for all registered services with the option
    /// to exclude internal services (those with paths starting with $)
    pub async fn get_all_service_metadata(
        &self,
        include_internal_services: bool,
    ) -> HashMap<String, ServiceMetadata> {
        let mut result = HashMap::new();
        let local_services = self.get_local_services().await;

        // Iterate through all services
        for (_, service_entry) in local_services {
            let service = &service_entry.service;
            let path_str = service.path().to_string();

            // Skip internal services if not included
            if !include_internal_services && path_str.starts_with("$") {
                continue;
            }


            // Create metadata using individual getter methods from the service
            result.insert(
                path_str,
                ServiceMetadata {
                    network_id: service.network_id().unwrap_or_default().to_string(),
                    service_path: service.path().to_string(),
                    name: service.name().to_string(),
                    version: service.version().to_string(),
                    description: service.description().to_string(),
                    actions: Vec::new(), // TODO: Populate with actual actions
                    events: Vec::new(),  // TODO: Populate with actual events
                    registration_time: service_entry.registration_time,
                    last_start_time: service_entry.last_start_time,
                },
            );
        }

        result
    }

}

#[async_trait::async_trait]
impl crate::services::RegistryDelegate for ServiceRegistry {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_states_by_service_path.read().await.clone()
    }


    
    // This method is now implemented as a public method above
    async fn get_actions_metadata(
        &self,
        service_topic_path: &TopicPath,
    ) -> Vec<ActionMetadata> {
        // Delegate to the public implementation
        self.get_actions_metadata(service_topic_path).await
    }


    /// Get metadata for a specific service
    ///
    /// INTENTION: Retrieve comprehensive metadata for a service, including its actions and events.
    /// This is useful for service discovery and introspection.
    async fn get_service_metadata(
        &self,
        topic_path: &TopicPath,
    ) -> Option<ServiceMetadata> {
        // Find service in the local services trie
        let services = self.local_services.read().await;
        let matches = services.find_matches(topic_path);

        if !matches.is_empty() {
            let service_entry = &matches[0].content;
            let service = service_entry.service.clone();
            let search_path = format!("{}/*", &service.path());
            let network_id_string = topic_path.network_id();
            let service_topic_path = TopicPath::new(
                search_path.as_str(),
                &network_id_string,
            ).unwrap();

            // Get actions metadata for this service - create a wildcard path 
             let actions = self.get_actions_metadata(&service_topic_path).await;
            
            // Get events metadata for this service - create a wildcard path
             let events = self.get_events_metadata(&service_topic_path).await;

            // Create metadata using individual getter methods
            return Some(ServiceMetadata {
                network_id: network_id_string,
                service_path: service.path().to_string(),
                name: service.name().to_string(),
                version: service.version().to_string(),
                description: service.description().to_string(),
                actions,
                events,
                registration_time: service_entry.registration_time,
                last_start_time: service_entry.last_start_time,
            });
        }

        None
    }



    /// Get metadata for all registered services with an option to filter internal services
    async fn get_all_service_metadata(
        &self,
        include_internal_services: bool,
    ) -> HashMap<String, ServiceMetadata> {
        self.get_all_service_metadata(include_internal_services)
            .await
    }

    /// Register a remote action handler
    async fn register_remote_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        remote_service: Arc<RemoteService>,
    ) -> Result<()> {
        // This is just a proxy to the instance method
        self.register_remote_action_handler(topic_path, handler, remote_service)
            .await
    }
}
