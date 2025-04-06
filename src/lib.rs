// Root module for the runar-node-new crate
//
// This crate implements a new version of the ServiceRegistry and related
// functionality using a test-driven approach.

pub mod services;
pub mod routing;
pub mod node;

// Re-export common types
pub use services::service_registry::ServiceRegistry;
pub use services::{NodeRequestHandler, RequestContext, ServiceRequest, ServiceResponse, ResponseStatus};
pub use services::abstract_service::{AbstractService, ServiceState};
pub use routing::TopicPath; 
pub use node::{Node, NodeConfig}; 