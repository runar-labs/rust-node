#[cfg(test)]
mod tests {
    use anyhow::Result;
    use runar_node::{RequestContext, ValueType};
    use runar_node::util::logging::{Component, debug_log, info_log, warn_log, error_log};
    use runar_node::node::DummyNodeRequestHandler;
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use std::collections::HashMap;
    use log::{debug, info, warn, error};
    use std::sync::Once;

    // Static initialization to ensure logger is only set up once
    static INIT_LOGGER: Once = Once::new();

    // Mock logger for testing
    struct MockLogger {
        logs: Arc<Mutex<Vec<(String, String, HashMap<String, String>)>>>,
    }

    impl MockLogger {
        fn new() -> Self {
            Self {
                logs: Arc::new(Mutex::new(Vec::new())),
            }
        }

        async fn log(&self, level: &str, message: &str, fields: HashMap<String, String>) {
            let mut logs = self.logs.lock().await;
            logs.push((level.to_string(), message.to_string(), fields));
        }

        async fn get_logs(&self) -> Vec<(String, String, HashMap<String, String>)> {
            let logs = self.logs.lock().await;
            logs.clone()
        }

        async fn clear_logs(&self) {
            let mut logs = self.logs.lock().await;
            logs.clear();
        }
    }

    // Test-specific custom context
    struct TestContext {
        request_id: String,
        network_id: String,
        node_id: String,
        peer_id: Option<String>,
    }

    impl TestContext {
        fn new(request_id: &str, network_id: &str, node_id: &str) -> Self {
            Self {
                request_id: request_id.to_string(),
                network_id: network_id.to_string(),
                node_id: node_id.to_string(),
                peer_id: None,
            }
        }

        fn with_peer_id(mut self, peer_id: &str) -> Self {
            self.peer_id = Some(peer_id.to_string());
            self
        }
    }

    // Setup test environment
    async fn setup_test_environment() -> Arc<MockLogger> {
        let logger = Arc::new(MockLogger::new());
        
        // Configure logging for tests only once
        INIT_LOGGER.call_once(|| {
            runar_node::util::logging::configure_test_logging();
        });
        
        logger
    }

    #[tokio::test]
    async fn test_basic_logging() -> Result<()> {
        let logger = setup_test_environment().await;
        logger.clear_logs().await;
        
        // Test basic logging using standard log macros
        info!("Test message");
        
        // Test with component-based logging
        debug_log(Component::Test, "Debug test message").await;
        info_log(Component::Test, "Info test message").await;
        warn_log(Component::Test, "Warning test message").await;
        error_log(Component::Test, "Error test message").await;
        
        // Wait a moment for async logging to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_context_aware_logging() -> Result<()> {
        let _logger = setup_test_environment().await;
        
        // Create a request context with a dummy node handler
        let node_handler = Arc::new(DummyNodeRequestHandler {});
        let context = RequestContext::new(
            "test/path",
            ValueType::Map(HashMap::new()),
            node_handler
        );
        
        // Log with context information
        info_log(Component::Test, &format!("Context-aware message with path: {}", 
            context.path)).await;
        
        // Wait for async logging
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_custom_fields() -> Result<()> {
        let _logger = setup_test_environment().await;
        
        // Create a context
        let context = TestContext::new(
            "req-12345",
            "net-67890",
            "node-abcde"
        ).with_peer_id("peer-xyz");
        
        // Log with context and custom fields
        info_log(Component::Test, &format!(
            "Message with custom fields - req: {}, net: {}, node: {}, peer: {}, duration_ms: {}, status: {}, count: {}",
            context.request_id,
            context.network_id,
            context.node_id,
            context.peer_id.as_deref().unwrap_or("none"),
            42,
            "success",
            100
        )).await;
        
        // Wait for async logging
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        Ok(())
    }

    // Test for non-async function logging
    #[test]
    fn test_sync_logging() -> Result<()> {
        // This is a synchronous function, so we're testing that the logging macros
        // work correctly in a non-async context
        
        // Create a context
        let context = TestContext::new(
            "req-sync",
            "net-sync",
            "node-sync"
        );
        
        // These should not panic or cause compilation errors
        debug!("Debug in sync context - req: {}", context.request_id);
        info!("Info in sync context - net: {}", context.network_id);
        warn!("Warning in sync context - node: {}", context.node_id);
        error!("Error in sync context - peer: {}", context.peer_id.as_deref().unwrap_or("none"));
        
        Ok(())
    }
} 