// Root module for the runar-node-new crate
//
// INTENTION: Define the core interfaces and implementations for the node runtime
// including services, routing, and network communication capabilities.

//! # Runar Node
//!
//! The Runar Node library provides a flexible service-oriented architecture for building
//! distributed applications. It allows services to communicate through a publish-subscribe
//! pattern and request-response interactions.
//!
//! ## API Ergonomics
//!
//! Throughout the codebase, string parameters have been designed for maximum flexibility 
//! using Rust's `impl Into<String>` pattern. This means you can pass string parameters in
//! various forms:
//!
//! ```rust,no_run
//! # use runar_node::node::{Node, NodeConfig};
//! # use runar_node::NodeDelegate;
//! # use runar_common::types::ValueType;
//! # use anyhow::Result;
//! #
//! # async fn example() -> Result<()> {
//! # let node = Node::new(NodeConfig::new("default", "default")).await?;
//! # let data = ValueType::Null;
//! # let id = "user123";
//! // All of these are valid
//! node.publish("status/updated".to_string(), data.clone()).await?;
//! node.publish(String::from("status/updated"), data.clone()).await?;
//! node.publish(format!("status/{}", id), data).await?;
//!
//! let topic = "notifications";
//! # let callback = Box::new(|ctx, data| Box::pin(async move { Ok(()) }) as _);
//! node.subscribe(topic.to_string(), callback).await?;
//! # Ok(())
//! # }
//! ```
//!
//! This flexibility extends to all public APIs in the codebase including:
//! - Node API methods (publish, subscribe, request)
//! - TopicPath constructors and methods
//! - ServiceRequest constructors 
//! - LifecycleContext registration methods
//!
//! ## Core Components
//!
//! - **Node**: The central component that manages services and handles routing
//! - **TopicPath**: Represents a hierarchical path for addressing services and actions
//! - **Service Registry**: Manages service registration, action handlers, and event subscriptions
//! - **EventContext/RequestContext**: Contextual information for handling events and requests
//!
//! ## Example Usage
//!
//! ```rust,no_run
//! use runar_node::node::{Node, NodeConfig};
//! use runar_node::NodeDelegate;
//! use runar_common::types::ValueType;
//! use anyhow::Result;
//!
//! async fn example() -> Result<()> {
//!     // Create a new node
//!     let node = Node::new(NodeConfig::new("default", "default")).await?;
//!     
//!     // Make a request to a service
//!     let response = node.request("auth/login".to_string(), ValueType::Null).await?;
//!     
//!     // Publish an event
//!     node.publish("user/profile/updated".to_string(), ValueType::Null).await?;
//!     
//!     Ok(())
//! }
//! ```

// Public modules
pub mod node;
pub mod services;
pub mod routing;
pub mod network;
pub mod config;
pub mod utils;

// Re-export the main types from the node module
pub use node::{Node, NodeConfig};

// Re-export the main types from the services module
pub use services::{
    ActionHandler, EventContext, LifecycleContext, NodeDelegate, PublishOptions,
    RequestContext, RegistryDelegate, ServiceRequest, ServiceResponse, SubscriptionOptions
};
pub use services::service_registry::ServiceRegistry;
pub use services::abstract_service::{AbstractService, ActionMetadata, CompleteServiceMetadata, ServiceState};

// Re-export the main types from the routing module
pub use routing::TopicPath;

// Re-export the main types from the network module
pub use network::{
    NetworkTransport, PeerId, TransportOptions, NetworkMessage, 
    TransportFactory, NetworkMessageType,
    NodeDiscovery, NodeInfo, DiscoveryOptions
};
// Re-export peer registry types from transport
pub use network::transport::{PeerRegistry, PeerStatus, PeerEntry};

// Re-export common types
pub use runar_common::types::ValueType;

// Re-export common macros for convenience
pub use runar_common::vmap;

// Version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NAME: &str = env!("CARGO_PKG_NAME"); 