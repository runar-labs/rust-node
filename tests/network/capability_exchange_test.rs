use serde_json;
use runar_node::network::capabilities::{ServiceCapability, ActionCapability, EventCapability};
use anyhow::Result;

#[tokio::test]
async fn test_service_capability_serialization() -> Result<()> {
    // Create a service capability with multiple actions
    let capability = ServiceCapability {
        network_id: "test-network".to_string(),
        service_path: "MathService".to_string(),
        name: "Math Service".to_string(),
        version: "1.0.0".to_string(),
        description: "Provides math operations".to_string(),
        actions: vec![
            ActionCapability {
                name: "add".to_string(),
                description: "Adds two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
            ActionCapability {
                name: "subtract".to_string(),
                description: "Subtracts two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            },
            ActionCapability {
                name: "multiply".to_string(),
                description: "Multiplies two numbers".to_string(),
                params_schema: None,
                result_schema: None,
            }
        ],
        events: vec![
            EventCapability {
                topic: "calculation-done".to_string(),
                description: "Triggered when a calculation is completed".to_string(),
                data_schema: None,
            }
        ],
    };

    // Serialize to JSON
    let json = serde_json::to_string_pretty(&capability)?;
    
    // Verify JSON contains all expected actions
    assert!(json.contains("\"name\": \"add\""));
    assert!(json.contains("\"name\": \"subtract\""));
    assert!(json.contains("\"name\": \"multiply\""));
    assert!(json.contains("\"topic\": \"calculation-done\""));
    
    // Deserialize back to a service capability
    let deserialized: ServiceCapability = serde_json::from_str(&json)?;
    
    // Verify the structure
    assert_eq!(deserialized.service_path, "MathService");
    assert_eq!(deserialized.network_id, "test-network");
    assert_eq!(deserialized.name, "Math Service");
    assert_eq!(deserialized.actions.len(), 3);
    assert_eq!(deserialized.events.len(), 1);
    
    // Check that all actions are preserved
    assert!(deserialized.actions.iter().any(|a| a.name == "add"));
    assert!(deserialized.actions.iter().any(|a| a.name == "subtract"));
    assert!(deserialized.actions.iter().any(|a| a.name == "multiply"));
    
    // Check that event is preserved
    assert_eq!(deserialized.events[0].topic, "calculation-done");
    
    Ok(())
} 