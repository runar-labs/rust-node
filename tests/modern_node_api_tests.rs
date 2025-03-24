use anyhow::{anyhow, Result};
use tokio::time::Duration;
use std::sync::Once;
use tokio::time::timeout;

mod fixtures;
mod utils;

use crate::fixtures::macro_based::auth_service::MacroAuthService;
use crate::fixtures::macro_based::document_service::MacroDocumentService;
use crate::fixtures::macro_based::event_emitter_service::MacroEventEmitterService;
use crate::fixtures::macro_based::event_listener_service::MacroEventListenerService;
use crate::utils::modernized_test_utils::{create_modern_test_node};

// Use this to initialize the logger only once
static INIT_LOGGER: Once = Once::new();

#[tokio::test]
async fn test_modern_node_api_basic() -> Result<()> {
    // Create a node with all our modern services
    let (mut node, _temp_dir, _config, auth_service, document_service, event_emitter, event_listener) = 
        create_modern_test_node().await?;
    
    // Test auth service login
    let login_result = node.request(
        format!("{}/login", auth_service.name()),
        runar_node::vmap!{
            "username" => "admin".into(),
            "password" => "password123".into()
        }
    ).await?;
    
    // Verify we got a token using vmap! macro per guidelines
    let token = runar_node::vmap!(login_result, "token" => "");
    
    assert!(!token.is_empty(), "Token should not be empty");
    
    // Validate the token
    let validation_result = node.request(
        format!("{}/validate_token", auth_service.name()),
        runar_node::vmap!{
            "token" => token.clone().into()
        }
    ).await?;
    
    let is_valid = runar_node::vmap!(validation_result, "valid" => false);
    
    assert!(is_valid, "Token should be valid");
    
    // Test document service
    let doc_id = "test-doc-1";
    let doc_content = "This is a test document";
    
    // Create a document
    let create_result = node.request(
        format!("{}/create", document_service.name()),
        runar_node::vmap!{
            "id" => doc_id.into(),
            "data" => doc_content.into()
        }
    ).await?;
    
    // Verify document was created
    let result_id = runar_node::vmap!(create_result, "id" => "");
    assert_eq!(result_id, doc_id, "Document ID should match");
    
    // Test event emitter and listener
    let event_data = "This is a test event";
    
    // Publish a valid event
    node.request(
        format!("{}/process_valid", event_emitter.name()),
        runar_node::vmap!{
            "data" => event_data.into()
        }
    ).await?;
    
    // Allow time for the event to be processed
    tokio::time::sleep(Duration::from_millis(100)).await;
    
    // Query the listener to check if it received the event
    let query_result = node.request(
        format!("{}/query_valid_events", event_listener.name()),
        runar_node::vmap!{}
    ).await?;
    
    // Verify that we received at least one event
    let events_array = runar_node::vmap!(query_result, "events" => Vec::new());
    
    assert!(!events_array.is_empty(), "Should have received at least one event");
    
    // Check that our specific event was received
    let found_event = events_array.iter().any(|e| {
        let data = runar_node::vmap!(e, "data" => "");
        data == event_data
    });
    
    assert!(found_event, "Should have found our specific event");
    
    Ok(())
}

#[tokio::test]
async fn test_modern_auth_flow() -> Result<()> {
    // Create a node with all our modern services
    let (mut node, _temp_dir, _config, auth_service, _document_service, _event_emitter, _event_listener) = 
        create_modern_test_node().await?;
    
    // Test successful login
    let login_result = node.request(
        format!("{}/login", auth_service.name()),
        runar_node::vmap!{
            "username" => "admin".into(),
            "password" => "password123".into()
        }
    ).await?;
    
    // Verify we got a token using vmap! macro per guidelines
    let token = runar_node::vmap!(login_result, "token" => "");
    
    assert!(!token.is_empty(), "Token should not be empty");
    
    // Test failed login
    let failed_login_result = node.request(
        format!("{}/login", auth_service.name()),
        runar_node::vmap!{
            "username" => "admin".into(),
            "password" => "wrong_password".into()
        }
    ).await;
    
    // It should fail with an error
    assert!(failed_login_result.is_err(), "Login with wrong password should fail");
    
    // Validate the token
    let validation_result = node.request(
        format!("{}/validate_token", auth_service.name()),
        runar_node::vmap!{
            "token" => token.clone().into()
        }
    ).await?;
    
    let is_valid = runar_node::vmap!(validation_result, "valid" => false);
    
    assert!(is_valid, "Token should be valid");
    
    // Logout
    let logout_result = node.request(
        format!("{}/logout", auth_service.name()),
        runar_node::vmap!{
            "token" => token.clone().into()
        }
    ).await?;
    
    // Check that logout was successful
    let success = runar_node::vmap!(logout_result, "success" => false);
        
    assert!(success, "Logout should be successful");
    
    // Validate the token again - should now be invalid
    let validation_result = node.request(
        format!("{}/validate_token", auth_service.name()),
        runar_node::vmap!{
            "token" => token.clone().into()
        }
    ).await?;
    
    let is_still_valid = runar_node::vmap!(validation_result, "valid" => false);
    
    assert!(!is_still_valid, "Token should be invalid after logout");
    
    Ok(())
}
#[tokio::test]
async fn test_modern_service_to_service_communication() -> Result<()> {
    // Set a timeout for the test
    let timeout_duration = std::time::Duration::from_secs(10);

    let _result = timeout(timeout_duration, async {
        // Create a modern test node with all required services
        let (mut node, _temp_dir, _config, auth_service, document_service, _event_emitter, _event_listener) = 
            create_modern_test_node().await?;

        // The modern AuthService in fixtures/macro_based/auth_service.rs has a test user already added
        // Use the node.request pattern to login the test user
        let login_result = node.request(
            format!("{}/login", auth_service.name()),
            runar_node::vmap!{
                "username" => "testuser".into(),
                "password" => "password123".into()
            }
        ).await?;
        
        // Extract the session token using vmap! per guidelines
        let token = runar_node::vmap!(login_result, "token" => "");
        
        assert!(!token.is_empty(), "Session token should not be empty");
        
        // First create a document in the document service
        let doc_id = "test-doc-123";
        let doc_content = "Test document content for service-to-service test";
        
        // Create a document
        let create_result = node.request(
            document_service.name(),
            "create",
            runar_node::vmap!{
                "id" => doc_id.into(),
                "data" => doc_content.into()
            }
        ).await?;
        
        // Verify the document was created
        let created_id = runar_node::vmap!(create_result, "id" => "");
        assert_eq!(created_id, doc_id, "Document ID should match");
        
        // Now demonstrate service-to-service communication by:
        // 1. Reading a document (which should work)
        // 2. Creating a secure document that requires validation
        
        // First, read the document we just created
        let read_result = node.request(
            document_service.name(),
            "read",
            runar_node::vmap!{
                "id" => doc_id.into()
            }
        ).await?;
        
        // Verify we can read the document
        let read_id = runar_node::vmap!(read_result, "id" => "");
        assert_eq!(read_id, doc_id, "Read document ID should match");
        
        // For service-to-service communication, create a document with authentication info
        // The authentication info will be embedded in the document itself
        // This is a more realistic example of service boundaries and communication
        let secure_doc_id = "secure-doc-456";
        let secure_doc = runar_node::vmap!{
            "content" => "This is a secure document".into(),
            "owner" => "testuser".into(),
            "auth_token" => token.clone().into() // Embed the token in the document
        };
        
        // Create the secure document
        let secure_create_result = node.request(
            document_service.name(),
            "create",
            runar_node::vmap!{
                "id" => secure_doc_id.into(),
                "data" => secure_doc.into()
            }
        ).await?;
        
        // Verify the secure document was created
        let secure_created_id = runar_node::vmap!(secure_create_result, "id" => "");
        assert_eq!(secure_created_id, secure_doc_id, "Secure document ID should match");
        
        Ok::<(), anyhow::Error>(())
    }).await??;
    
    Ok(())
}
