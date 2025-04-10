use serde::{Serialize, Deserialize};
use runar_common::types::ValueType;

/// Represents the capabilities of a single service advertised over the network.
///
/// INTENTION: Provide a structured way to share information about a service's
/// available actions and events during discovery.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceCapability {
    pub network_id: String,        // Network ID for this service
    pub service_path: String,      // e.g., "auth", "user/profile" - without network prefix
    pub name: String,              // Human-readable name
    pub version: String,
    pub description: String,
    pub actions: Vec<ActionCapability>,
    pub events: Vec<EventCapability>,
}

/// Represents a single action capability of a service.
///
/// INTENTION: Describe an action that can be invoked on a service, including
/// its parameters and expected result format.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionCapability {
    pub name: String, // e.g., "login", "get_details"
    pub description: String,
    // Optional schemas for validation or documentation generation
    pub params_schema: Option<ValueType>,
    pub result_schema: Option<ValueType>,
}

/// Represents a single event capability of a service.
///
/// INTENTION: Describe an event that a service might publish, including the
/// topic format and the expected data schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventCapability {
    pub topic: String, // e.g., "user_created", "status_updated"
    pub description: String,
    // Optional schema for the event data payload
    pub data_schema: Option<ValueType>,
} 