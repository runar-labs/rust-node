use anyhow::Result;
use runar_node::node::{Node, NodeConfig};
use runar_node::{RequestContext, ValueType};
use runar_node::services::abstract_service::AbstractService;
use tempfile::TempDir;

use crate::fixtures::macro_based::auth_service::MacroAuthService;
use crate::fixtures::macro_based::document_service::MacroDocumentService;
use crate::fixtures::macro_based::event_emitter_service::MacroEventEmitterService;
use crate::fixtures::macro_based::event_listener_service::MacroEventListenerService;
use crate::utils::node_utils::{create_test_node, add_service_to_node};

/// Create a test node with the modern service architecture
pub async fn create_modern_test_node() -> Result<(
    Node,
    TempDir,
    NodeConfig,
    MacroAuthService,
    MacroDocumentService,
    MacroEventEmitterService,
    MacroEventListenerService,
)> {
    // Create a test node
    let (mut node, temp_dir, config) = create_test_node().await?;
    
    // Create the services with appropriate names
    let auth_service = MacroAuthService::new("test_auth");
    let document_service = MacroDocumentService::new("test_docs");
    let event_emitter = MacroEventEmitterService::new("test_emitter");
    let event_listener = MacroEventListenerService::new("test_listener");
    
    // Register the services with the node
    add_service_to_node(&mut node, auth_service.clone()).await?;
    add_service_to_node(&mut node, document_service.clone()).await?;
    add_service_to_node(&mut node, event_emitter.clone()).await?;
    add_service_to_node(&mut node, event_listener.clone()).await?;
    
    // Start the node
    node.start().await?;
    
    Ok((node, temp_dir, config, auth_service, document_service, event_emitter, event_listener))
}

/// Helper function to process a valid event with the macro-based emitter service
pub async fn process_valid_event(
    node: &Node,
    event_emitter: &MacroEventEmitterService,
    data: &str
) -> Result<ValueType> {
    // Process the valid event using the node.request pattern according to guidelines
    node.request(
        format!("{}/process_valid", event_emitter.name()),
        data.to_string()
    ).await
}

/// Helper function to process an invalid event with the macro-based emitter service
pub async fn process_invalid_event(
    node: &Node,
    event_emitter: &MacroEventEmitterService,
    data: &str
) -> Result<ValueType> {
    // Process the invalid event using the node.request pattern according to guidelines
    node.request(
        format!("{}/process_invalid", event_emitter.name()),
        data.to_string()
    ).await
}

/// Helper function to check if a valid event was received by the listener service
pub async fn check_valid_event_received(
    node: &Node,
    event_listener: &MacroEventListenerService,
    data_pattern: &str
) -> Result<bool> {
    // Query the valid events using the node.request pattern according to guidelines
    let response = node.request(
        format!("{}/query_valid_events", event_listener.name()),
        runar_node::vmap!{}
    ).await?;
    
    // Check if events were received with the given data pattern using vmap! macro
    if let Some(events) = runar_node::vmap!(response, "events" => None::<Vec<ValueType>>) {
        for event in events {
            let data = runar_node::vmap!(event, "data" => "");
            if data.contains(data_pattern) {
                return Ok(true);
            }
        }
    }
    
    Ok(false)
}

/// Helper function to check if an invalid event was received by the listener service
pub async fn check_invalid_event_received(
    node: &Node,
    event_listener: &MacroEventListenerService,
    data_pattern: &str
) -> Result<bool> {
    // Query the invalid events using the node.request pattern according to guidelines
    let response = node.request(
        format!("{}/query_invalid_events", event_listener.name()),
        runar_node::vmap!{}
    ).await?;
    
    // Check if events were received with the given data pattern using vmap! macro
    if let Some(events) = runar_node::vmap!(response, "events" => None::<Vec<ValueType>>) {
        for event in events {
            let data = runar_node::vmap!(event, "data" => "");
            if data.contains(data_pattern) {
                return Ok(true);
            }
        }
    }
    
    Ok(false)
}
