// Root module for the runar-node-new crate
//
// This crate implements a new version of the ServiceRegistry and related
// functionality using a test-driven approach.

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
//! ```rust
//! // All of these are valid
//! node.publish("status/updated", data).await?;
//! node.publish(String::from("status/updated"), data).await?;
//! node.publish(&format!("status/{}", id), data).await?;
//!
//! let topic = "notifications";
//! node.subscribe(topic, callback).await?;
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
//! use runar_common::types::ValueType;
//! use anyhow::Result;
//!
//! async fn example() -> Result<()> {
//!     // Create a new node
//!     let node = Node::new(NodeConfig::new("default")).await?;
//!     
//!     // Make a request to a service
//!     let response = node.request("auth/login", ValueType::Null).await?;
//!     
//!     // Publish an event
//!     node.publish("user/profile/updated", ValueType::Null).await?;
//!     
//!     Ok(())
//! }
//! ```

pub mod services;
pub mod routing;
pub mod node;

// Re-export common types
pub use services::service_registry::ServiceRegistry;
pub use services::{NodeRequestHandler, RequestContext, ServiceRequest, ServiceResponse, ResponseStatus};
pub use services::abstract_service::{AbstractService, ServiceState};
pub use routing::TopicPath; 
pub use node::{Node, NodeConfig}; 