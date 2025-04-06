// Service Registry Module
//
// INTENTION:
// This module provides action handler and event subscription management capabilities for the node.
// It acts as a central registry for action handlers and event subscriptions, enabling the node to
// find he correct subscribers and actions handlers.. THE Registry does not CALL ANY CALLBACKS/Handler directly
//.. this is NODEs functions.
//
// ARCHITECTURAL PRINCIPLES:
// 1. Handler Registration - Manages registration of action handlers
// 2. Event Subscription  Registration - Manages registration of event handlers 
// 3. Network Isolation - Respects network boundaries for handlers and subscriptions
// 4. Path Consistency - ALL Registry APIs use TopicPath objects for proper validation
//    and consistent path handling, NEVER raw strings
//
// IMPORTANT NOTE:
// The Registry should focus solely on managing action handlers and subscriptions.
// It should NOT handle service discovery or lifecycle - that's the responsibility of the Node.
// Request routing and handling is also the Node's responsibility.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use log::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

use runar_common::utils::logging::{debug_log, info_log, Component};
use runar_common::logging::Logger;
use runar_common::types::ValueType;
use crate::routing::TopicPath;
use crate::services::abstract_service::{CompleteServiceMetadata, ServiceState, ActionMetadata, AbstractService};
use crate::services::{
    NodeRequestHandler, RequestContext, ServiceRequest, ServiceResponse, SubscriptionOptions, 
    ActionHandler, LifecycleContext, ArcContextLogging, EventContext
};

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
    /// Action handlers organized by network and then by path
    action_handlers: RwLock<HashMap<String, HashMap<String, ActionHandler>>>,
    /// Event subscribers organized by topic
    event_subscribers: RwLock<HashMap<String, Vec<String>>>,
    /// Callbacks map for event handlers
    event_callbacks: RwLock<HashMap<String, Vec<(String, EventCallback)>>>,
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
            action_handlers: RwLock::new(HashMap::new()),
            event_subscribers: RwLock::new(HashMap::new()),
            event_callbacks: RwLock::new(HashMap::new()),
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
            action_handlers: RwLock::new(HashMap::new()),
            event_subscribers: RwLock::new(HashMap::new()),
            event_callbacks: RwLock::new(HashMap::new()),
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
    
    /// Subscribe to a topic
    ///
    /// INTENTION: Register a callback to be notified when events are published
    /// to a specific topic. This is the primary way for components to receive
    /// events from the system.
    ///
    /// Returns a unique subscription ID that can be used for unsubscribing.
    pub async fn subscribe(
        &self,
        topic: &TopicPath,
        callback: Arc<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        self.subscribe_with_options(topic, callback, SubscriptionOptions::default()).await
    }
    
    /// Subscribe to a topic with options
    ///
    /// INTENTION: Register a callback with specific options for controlling
    /// how events are delivered. This extended version of subscribe allows
    /// for more fine-grained control of subscription behavior.
    ///
    /// Note: Currently, options are not used, but the parameter exists for
    /// future extension.
    pub async fn subscribe_with_options(
        &self,
        topic: &TopicPath,
        callback: Arc<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        _options: SubscriptionOptions,
    ) -> Result<String> {
        // Generate a unique subscription ID
        let subscription_id = uuid::Uuid::new_v4().to_string();
        
        self.logger.debug(format!("Subscribing to topic '{}' with ID '{}'", topic.as_str(), subscription_id));
        
        // Update the subscribers map - we need to store which subscription IDs exist for each topic
        {
            let mut subscribers = self.event_subscribers.write().await;
            
            // Ensure we have an entry for this topic
            if !subscribers.contains_key(topic.as_str()) {
                subscribers.insert(topic.as_str().to_string(), Vec::new());
            }
            
            // Get the vector for this topic and add the subscription ID
            if let Some(subscriber_list) = subscribers.get_mut(topic.as_str()) {
                subscriber_list.push(subscription_id.clone());
            }
        }
        
        // Store the callback in the callbacks map
        {
            let mut callbacks = self.event_callbacks.write().await;
            
            // Create entry if not exists
            if !callbacks.contains_key(topic.as_str()) {
                callbacks.insert(topic.as_str().to_string(), Vec::new());
            }
            
            // Add the callback
            if let Some(cb_list) = callbacks.get_mut(topic.as_str()) {
                cb_list.push((subscription_id.clone(), callback));
            }
        }
        
        // Return the subscription ID
        Ok(subscription_id)
    }
    
    /// Unsubscribe from a topic
    ///
    /// INTENTION: Remove a subscription from a topic, either by specific
    /// subscription ID or all subscriptions for the topic if no ID is provided.
    ///
    /// ARCHITECTURAL PRINCIPLE:
    /// Subscriptions should be explicitly managed, with the ability to clean up
    /// when no longer needed to prevent resource leaks and unwanted callbacks.
    pub async fn unsubscribe(&self, topic: &TopicPath, subscription_id: Option<&str>) -> Result<()> {
        match subscription_id {
            Some(id) => {
                self.logger.debug(format!("Unsubscribing from topic '{}' with ID '{}'", topic.as_str(), id));
                
                // Remove from subscribers map
                {
                    let mut subscribers = self.event_subscribers.write().await;
                    if let Some(sub_list) = subscribers.get_mut(topic.as_str()) {
                        sub_list.retain(|sub_id| sub_id != id);
                    }
                }
                
                // Remove from callbacks map
                {
                    let mut callbacks = self.event_callbacks.write().await;
                    if let Some(cb_list) = callbacks.get_mut(topic.as_str()) {
                        cb_list.retain(|(sub_id, _)| sub_id != id);
                    }
                }
            },
            None => {
                self.logger.debug(format!("Unsubscribing from all callbacks for topic '{}'", topic.as_str()));
                
                // Remove entire topic
                {
                    let mut subscribers = self.event_subscribers.write().await;
                    subscribers.remove(topic.as_str());
                }
                
                {
                    let mut callbacks = self.event_callbacks.write().await;
                    callbacks.remove(topic.as_str());
                }
            }
        }
        
        Ok(())
    }
    
    /// Register an action handler
    ///
    /// INTENTION: Register a handler function for a specific service action.
    /// This enables services to dynamically register handlers at runtime.
    pub async fn register_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        metadata: Option<ActionMetadata>
    ) -> Result<()> {
        // Use action_path directly instead of manually constructing it
        let action_path = topic_path.action_path();
        let network_id = topic_path.network_id();

        // Debug output using proper logger
        self.logger.debug(format!("DEBUG_REGISTER_ACTION: action_path={}, topic_path={}", 
                  action_path, topic_path.as_str()));
        
        // Print detailed debug information
        self.logger.info(format!("Registering action: action_path={}, topic_path={}", 
                              action_path, topic_path.as_str()));
        
        // Store the handler in the appropriate network and path
        {
            let mut handlers = self.action_handlers.write().await;
            
            // Ensure we have a map for this network
            if !handlers.contains_key(&network_id) {
                handlers.insert(network_id.to_string(), HashMap::new());
            }
            
            // Add the handler to the network's map
            if let Some(network_handlers) = handlers.get_mut(&network_id) {
                self.logger.debug(format!("Registering action handler for '{}' on network '{}'", action_path, network_id));
                // Use the action_path directly as the key for consistency
                network_handlers.insert(action_path.clone(), handler);
                
                // Debug output of registered handlers using proper logger
                self.logger.debug(format!("Registered handlers for network {}: {:?}", 
                         network_id, network_handlers.keys().collect::<Vec<_>>()));
            }
        }
        
        // Log the registration
        self.logger.info(format!("Registered action handler for '{}' on network '{}'", action_path, network_id));
        
        Ok(())
    }

    /// Get an action handler for a specific topic path
    ///
    /// INTENTION: Find the action handler registered for a specific topic path.
    /// This is used by the Node when routing requests to the appropriate handler.
    /// A valid topic path MUST include an action, not just a service name.
    pub async fn get_action_handler(
        &self,
        topic_path: &TopicPath
    ) -> Option<ActionHandler> {
        // Use action_path directly instead of manually constructing it
        let action_path = topic_path.action_path();
        
        // Debug output with proper logger
        self.logger.debug(format!("Looking for handler: topic_path={}, action_path={}, network_id={}",
                 topic_path.as_str(), action_path, topic_path.network_id()));
        
        // Access the action handlers registry
        let handlers = self.action_handlers.read().await;
        
        // Debug output of all available handlers using proper logger
        if let Some(network_handlers) = handlers.get(&topic_path.network_id()) {
            self.logger.debug(format!("Available handlers for network {}: {:?}", 
                     topic_path.network_id(), network_handlers.keys().collect::<Vec<_>>()));
        }
        
        // Try to find the handler in the network's handlers map
        if let Some(network_handlers) = handlers.get(&topic_path.network_id()) {
            // First check for direct match
            if let Some(handler) = network_handlers.get(&action_path) {
                self.logger.debug(format!("Found direct handler match for '{}'", action_path));
                return Some(handler.clone());
            }
            
            // If no direct match, check for template patterns with path parameters
            for (registered_path, handler) in network_handlers.iter() {
                // For paths with parameter placeholders like {service_path}
                if registered_path.contains('{') {
                    // Get the segments of both paths
                    let registered_segments: Vec<&str> = registered_path.split('/').collect();
                    let requested_segments: Vec<&str> = action_path.split('/').collect();
                    
                    // Strict segment count matching - must have exactly the same number of segments
                    if registered_segments.len() != requested_segments.len() {
                        self.logger.debug(format!("Template '{}' has {} segments, but request '{}' has {} segments - not a match", 
                            registered_path, registered_segments.len(), action_path, requested_segments.len()));
                        continue;
                    }
                    
                    // If segment count matches, we might have a template match
                    let mut matches = true;
                    
                    for (i, reg_segment) in registered_segments.iter().enumerate() {
                        // If this segment is a parameter (wrapped in {}), it matches anything
                        if reg_segment.starts_with('{') && reg_segment.ends_with('}') {
                            // Parameter matches any value in this position
                            continue;
                        } else if reg_segment != &requested_segments[i] {
                            // Literal segment must match exactly
                            matches = false;
                            self.logger.debug(format!("Template '{}' segment '{}' doesn't match request '{}' segment '{}'", 
                                registered_path, reg_segment, action_path, requested_segments[i]));
                            break;
                        }
                    }
                    
                    if matches {
                        // Extract parameters from the registered path template and the actual request path
                        let mut params = HashMap::new();
                        
                        for (i, reg_segment) in registered_segments.iter().enumerate() {
                            if reg_segment.starts_with('{') && reg_segment.ends_with('}') {
                                // Extract parameter name (remove the {} brackets)
                                let param_name = &reg_segment[1..reg_segment.len()-1];
                                
                                // Store the parameter value from the requested path
                                params.insert(param_name.to_string(), requested_segments[i].to_string());
                            }
                        }
                        
                        // Use proper logger
                        self.logger.debug(format!("Found template handler for '{}' matching template '{}' with params: {:?}", 
                                 action_path, registered_path, params));
                                 
                        // Create a wrapper handler that populates the path_params
                        let original_handler = handler.clone();
                        let handler_with_params: ActionHandler = Arc::new(move |params_data, mut context| {
                            let handler_clone = original_handler.clone();
                            let path_params = params.clone();
                            
                            Box::pin(async move {
                                // Update the context with path parameters
                                context.path_params = path_params;
                                
                                // Call the original handler with updated context
                                handler_clone(params_data, context).await
                            })
                        });
                        
                        return Some(handler_with_params);
                    }
                }
            }
        }
        
        // Use proper logger for not found case
        self.logger.debug(format!("No handler found for '{}', available keys: {:?}", action_path, 
                 handlers.get(&topic_path.network_id())
                 .map(|m| m.keys().collect::<Vec<_>>())
                 .unwrap_or_default()));
        
        None
    }
    
    /// List all registered action handler paths
    ///
    /// INTENTION: Get a list of all registered action handler paths.
    /// This is useful for debugging and introspection.
    pub async fn list_action_handlers(&self) -> Vec<String> {
        let handlers = self.action_handlers.read().await;
        let mut result = Vec::new();
        
        for (network_id, network_handlers) in handlers.iter() {
            for handler_key in network_handlers.keys() {
                result.push(format!("{}:{}", network_id, handler_key));
            }
        }
        
        result
    }

    /// Get callback for a specific subscription
    ///
    /// INTENTION: Get the callback for a specific subscription ID.
    /// This is used by the Node when executing callbacks, to avoid having
    /// to hold locks across await points.
    ///
    /// Returns true if the callback was found and executed, false otherwise.
    pub async fn has_subscriber(&self, topic: &TopicPath, subscription_id: &str) -> bool {
        let callbacks = self.event_callbacks.read().await;
        
        // Check if we have this topic
        if let Some(cb_list) = callbacks.get(topic.as_str()) {
            // Find the matching subscription
            for (id, _) in cb_list {
                if id == subscription_id {
                    return true;
                }
            }
        }
        
        false
    }

    /// Get event handlers for a topic
    ///
    /// INTENTION: Get all the event handlers registered for a specific topic.
    /// This is used by the Node when publishing events, so it can execute the handlers directly.
    ///
    /// ARCHITECTURAL PRINCIPLE:
    /// The Registry only provides the handler information, but does NOT execute handlers.
    /// Handler execution is the responsibility of the Node.
    ///
    /// Returns subscription IDs and their handlers for the topic.
    pub async fn get_event_handlers(&self, topic: &TopicPath) -> Vec<(String, EventHandler)> {
        let callbacks = self.event_callbacks.read().await;
        let mut result = Vec::new();
        
        // Check if we have any subscribers for this exact topic
        if let Some(cb_list) = callbacks.get(topic.as_str()) {
            // For each callback, clone the ID and the Arc callback
            for (id, callback) in cb_list {
                result.push((id.clone(), callback.clone()));
            }
        }
        
        result
    }
} 