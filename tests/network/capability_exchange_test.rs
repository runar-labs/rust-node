use anyhow::Result;
use serde_json::{self, json};

use runar_node::network::capabilities::{ServiceCapability, ActionCapability};

#[test]
fn test_service_capability_serialization() -> Result<()> {
    // Create a service capability with multiple actions
    let capability = ServiceCapability {
        network_id: "test-network".to_string(),
        service_path: "MathService".to_string(),
        name: "Math".to_string(), 
        version: "1.0.0".to_string(),
        description: "Math service for testing".to_string(),
        actions: vec![
            ActionCapability {
                name: "add".to_string(),
                description: "Add two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
            ActionCapability {
                name: "subtract".to_string(),
                description: "Subtract two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
            ActionCapability {
                name: "multiply".to_string(),
                description: "Multiply two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
            ActionCapability {
                name: "divide".to_string(),
                description: "Divide two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
        ],
        events: vec![],
    };
    
    // Serialize the capability to JSON (this is what would happen in announce_local_services)
    let json_str = serde_json::to_string(&capability)?;
    
    // Verify the JSON contains all expected actions
    let expected_actions = ["add", "subtract", "multiply", "divide"];
    for action in &expected_actions {
        assert!(
            json_str.contains(&format!("\"name\":\"{}\"", action)),
            "Action '{}' not found in JSON representation", 
            action
        );
    }
    
    // Deserialize back to ServiceCapability (simulating what a receiving node would do)
    let deserialized: ServiceCapability = serde_json::from_str(&json_str)?;
    
    // Verify all actions were preserved
    assert_eq!(deserialized.actions.len(), capability.actions.len());
    for expected_action in &expected_actions {
        assert!(
            deserialized.actions.iter().any(|a| &a.name == expected_action),
            "Action '{}' missing after deserialization", 
            expected_action
        );
    }
    
    // Verify correct service metadata
    assert_eq!(deserialized.service_path, "MathService");
    assert_eq!(deserialized.network_id, "test-network");
    assert_eq!(deserialized.name, "Math");
    
    Ok(())
} 