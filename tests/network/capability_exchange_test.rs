use std::time::SystemTime;

use serde_json;
use runar_common::types::{ServiceMetadata, ActionMetadata, EventMetadata};
use anyhow::Result;

#[tokio::test]
async fn test_service_capability_serialization() -> Result<()> {
    // Create a service capability with multiple actions
    let capability = ServiceMetadata {
        network_id: "test-network".to_string(),
        service_path: "test-service".to_string(),
        registration_time: SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        last_start_time: Some(SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()),
        name: "math-service".to_string(),
        version: "1.0.0".to_string(),
        description: "Provides math operations".to_string(),
        actions: vec![
            ActionMetadata {
                name: "add".to_string(),
                description: "Adds two numbers".to_string(),
                input_schema: None,
                output_schema: None,
            },
            ActionMetadata {
                name: "subtract".to_string(),
                description: "Subtracts two numbers".to_string(),
                input_schema: None,
                output_schema: None,
            },
            ActionMetadata {
                name: "multiply".to_string(),
                description: "Multiplies two numbers".to_string(),
                input_schema: None,
                output_schema: None,
            }
        ],
        events: vec![
            EventMetadata {
                path: "calculation-done".to_string(),
                description: "Triggered when a calculation is completed".to_string(),
                data_schema: None,
            }
        ],
    };

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&capability)?;
    
    // Verify JSON contains all expected actions
    assert!(json.contains("\"path\": \"add\""));
    assert!(json.contains("\"path\": \"subtract\""));
    assert!(json.contains("\"path\": \"multiply\""));
    assert!(json.contains("\"path\": \"calculation-done\""));
    
    // Deserialize back to a service metadata
    let deserialized: ServiceMetadata = serde_json::from_str(&json)?;
    
    // Verify the structure
    assert_eq!(deserialized.name, "math-service");
    assert_eq!(deserialized.version, "1.0.0");
    assert_eq!(deserialized.description, "Provides math operations");
    assert_eq!(deserialized.actions.len(), 3);
    assert_eq!(deserialized.events.len(), 1);
    assert_eq!(deserialized.actions[0].name, "add");
    assert_eq!(deserialized.actions[1].name, "subtract");
    assert_eq!(deserialized.actions[2].name, "multiply");
    assert_eq!(deserialized.events[0].path, "calculation-done");
    
    Ok(())
}