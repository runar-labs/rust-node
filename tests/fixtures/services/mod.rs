// Services for testing
pub mod base_services;      // Common base services for testing
pub mod event_publisher;    // Event publisher service
pub mod event_subscriber;   // Event subscriber service
pub mod ship_service;       // Ship service for event testing

// Re-export all services for convenience
pub use base_services::*;
pub use event_publisher::*;
pub use event_subscriber::*;
pub use ship_service::*; 