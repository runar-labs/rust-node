// Abstract Service Definition Module
//
// INTENTION:
// This module defines the core AbstractService trait that all services must implement,
// along with its associated types and enumerations. It establishes the foundation
// for service implementation, lifecycle management, and communication patterns.
//
// ARCHITECTURAL PRINCIPLES:
// 1. Interface-First Design - All services must implement a consistent interface
// 2. Lifecycle Management - Services follow a predictable lifecycle (init, start, stop)
// 3. Consistent Communication - All services use the same request/response patterns
// 4. Self-Describing Services - Services provide metadata about their capabilities
// 5. Asynchronous Operations - All service methods are async for performance
//
// This module is a foundational element of the service architecture, defining
// how all services behave and interact with the system.

use anyhow::Result;
use std::fmt;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

use crate::services::LifecycleContext;
use runar_common::types::ValueType;

/// Represents a service's current state
///
/// INTENTION: Track the lifecycle stage of a service
/// for proper initialization and operational management.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ServiceState {
    /// Service is created but not initialized
    Created,
    /// Service has been initialized
    Initialized,
    /// Service is running
    Running,
    /// Service has been stopped
    Stopped,
    /// Service has been paused
    Paused,
    /// Service has encountered an error
    Error,
    /// Service state is unknown
    Unknown,
}

impl fmt::Display for ServiceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceState::Created => write!(f, "Created"),
            ServiceState::Initialized => write!(f, "Initialized"),
            ServiceState::Running => write!(f, "Running"),
            ServiceState::Stopped => write!(f, "Stopped"),
            ServiceState::Paused => write!(f, "Paused"),
            ServiceState::Error => write!(f, "Error"),
            ServiceState::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Action metadata with parameter and return type schemas
///
/// INTENTION: Provide additional information about service actions,
/// including their parameters and return types, which can be used for
/// automatic documentation generation and API validation.
#[derive(Debug, Clone)]
pub struct ActionMetadata {
    /// The name of the action
    pub name: String,
    /// A description of what the action does
    pub description: String,
    /// Schema for the action parameters
    pub parameters_schema: Option<ValueType>,
    /// Schema for the action return type
    pub return_schema: Option<ValueType>,
}

/// Event metadata with parameter schema
///
/// INTENTION: Provide additional information about service events,
/// including their data schema, which can be used for automatic
/// documentation generation and API validation.
#[derive(Debug, Clone)]
pub struct EventMetadata {
    /// The name of the event
    pub name: String,
    /// A description of what the event signifies
    pub description: String,
    /// Schema for the event data
    pub data_schema: Option<ValueType>,
}

/// Complete metadata for a service, including runtime information
///
/// INTENTION: Represent the complete state of a service, including
/// both static metadata and runtime information. This is maintained by
/// the Node, not individual services, simplifying service implementations.
#[derive(Debug, Clone)]
pub struct CompleteServiceMetadata {
    /// Basic metadata for the service
    pub name: String,
    /// Version of the service
    pub version: String,
    /// Path to the service
    pub path: String,
    /// Description of the service
    pub description: String,
    /// Current state of the service
    pub current_state: ServiceState,
    /// List of registered actions
    pub registered_actions: HashMap<String, ActionMetadata>,
    /// List of registered events
    pub registered_events: HashMap<String, EventMetadata>,
    /// Time the service was registered (Unix timestamp in seconds)
    pub registration_time: u64,
    /// Time the service was last started (Unix timestamp in seconds)
    pub last_start_time: Option<u64>,
}

impl CompleteServiceMetadata {
    /// Create a new CompleteServiceMetadata from basic service info
    pub fn new( 
        name: String, 
        version: String, 
        path: String, 
        description: String 
    ) -> Self {
        // Get current timestamp
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
            
        // Initialize with basic service info and empty collections
        Self { 
            name,
            version,
            path,
            description, 
            current_state: ServiceState::Created,
            registered_actions: HashMap::new(),
            registered_events: HashMap::new(),
            registration_time: now,
            last_start_time: None,
        }
    }
    
    /// Update the service state
    pub fn update_state(&mut self, state: ServiceState) {
        self.current_state = state;
        
        // If service is being started, update the start time
        if state == ServiceState::Running {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
                
            self.last_start_time = Some(now);
        }
    }
    
    /// Register an action with metadata
    pub fn register_action(&mut self, metadata: ActionMetadata) {
        self.registered_actions.insert(metadata.name.clone(), metadata);
    }
    
    /// Register an event with metadata
    pub fn register_event(&mut self, metadata: EventMetadata) {
        self.registered_events.insert(metadata.name.clone(), metadata);
    }
    
    /// Get all registered actions
    pub fn get_all_actions(&self) -> Vec<ActionMetadata> {
        self.registered_actions.values().cloned().collect()
    }
    
    /// Get all registered events
    pub fn get_all_events(&self) -> Vec<EventMetadata> {
        self.registered_events.values().cloned().collect()
    }
}

/// Abstract service interface
///
/// INTENTION: Define a common interface for all services, enabling
/// uniform management of service lifecycle and request handling.
///
/// ARCHITECTURAL PRINCIPLE:
/// Services follow a consistent lifecycle pattern with well-defined
/// initialization, startup, and shutdown phases. This ensures proper
/// resource management and predictable state transitions.
#[async_trait::async_trait]
pub trait AbstractService: Send + Sync {
    
    /// Get service name
    fn name(&self) -> &str;
    
    /// Get service version
    fn version(&self) -> &str;
    
    //NOTE: path is used during service registration. avoid using it directly internaly you shuold always use TOpicPath
    /// Get service path
    fn path(&self) -> &str;
    
    /// Get service description
    fn description(&self) -> &str;

    //NOTE: network_id is used during service registration. avoid using it directly internaly you shuold always use TOpicPath
    /// Get service description
    fn network_id(&self) -> Option<String>;
     
    /// Initialize the service
    ///
    /// INTENTION: Set up the service for operation, register handlers,
    /// establish connections to dependencies, and prepare internal state.
    ///
    /// This is where services should register their action handlers using
    /// the context's registration methods. The service should not perform
    /// any active operations during initialization.
    ///
    /// Initialization errors should be propagated to enable reporting and
    /// proper error handling.
    async fn init(&self, context: LifecycleContext) -> Result<()>;
    
    /// Start the service
    ///
    /// INTENTION: Begin active operations after initialization is complete.
    /// This is where the service should start any background tasks, timers,
    /// or active processing activities.
    ///
    /// The service should be fully initialized before this method is called.
    async fn start(&self, context: LifecycleContext) -> Result<()>;
    
    /// Stop the service
    ///
    /// INTENTION: Gracefully terminate all active operations, cancel background
    /// tasks, and release resources. This method should ensure that the service
    /// can be cleanly shut down without data loss or corruption.
    async fn stop(&self, context: LifecycleContext) -> Result<()>;
} 