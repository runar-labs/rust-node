// Math service test fixture
//
// This is a simple service implementation used for testing the service registry.
// It implements both name and path parameters.
//
// IMPORTANT: This fixture is a critical component for testing the entire Runar service architecture.
// It should NOT be substantially modified without careful consideration of all dependent tests.
// Any changes to the service interface or semantics may break numerous tests across the codebase.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

use runar_common::types::ValueType;
use runar_node::services::abstract_service::AbstractService;
use runar_node::services::{ArcContextLogging, LifecycleContext, RequestContext, ServiceResponse};

/// A simple math service for testing
///
/// This service is used throughout the test suite to verify various aspects
/// of the service architecture, including:
///
/// - Service registration and lookup
/// - Request handling and parameter extraction
/// - Context usage for logging
/// - Service lifecycle management
///
/// USAGE NOTES:
/// This fixture is designed to be a clean implementation of the service architecture.
/// It intentionally follows all the proper patterns for context handling, logging,
/// and error handling that should be present in production services.
///
/// If refactoring the AbstractService trait or related interfaces, this fixture
/// should be updated carefully to maintain its role as the reference implementation.
pub struct MathService {
    /// Name of the service
    name: String,
    /// Version of the service
    version: String,
    /// Path of the service
    path: String,
    /// Description of the service
    description: String,
    /// Network ID of the service
    network_id: Option<String>,
    /// Counter for operations performed
    /// Note: This is used only for testing. In production services,
    /// we should avoid maintaining state when possible and use metrics
    /// for tracking operation counts.
    counter: Arc<Mutex<i32>>,
}

// Manual implementation of Clone for MathService
impl Clone for MathService {
    fn clone(&self) -> Self {
        MathService {
            name: self.name.clone(),
            version: self.version.clone(),
            path: self.path.clone(),
            description: self.description.clone(),
            network_id: self.network_id.clone(),
            counter: self.counter.clone(),
        }
    }
}

impl MathService {
    /// Create a new MathService with the specified name and path
    pub fn new(name: &str, path: &str) -> Self {
        Self {
            name: name.to_string(),
            version: "1.0.0".to_string(),
            path: path.to_string(),
            description: "Math service for testing".to_string(),
            network_id: None, //will be set by the node
            counter: Arc::new(Mutex::new(0)),
        }
    }

    /// Convenience constructor when path should match name
    pub fn new_with_same_path(name: &str) -> Self {
        Self::new(name, name.to_lowercase().as_str())
    }

    /// Add two numbers
    ///
    /// IMPORTANT: This method demonstrates proper context usage for logging.
    /// It correctly uses the context parameter for all logging operations.
    fn add(&self, a: f64, b: f64, ctx: &RequestContext) -> f64 {
        // Increment the counter
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;

        // Use the passed context for logging
        ctx.debug(format!("Adding {} + {}", a, b));

        // Perform the addition
        a + b
    }

    /// Subtract two numbers
    fn subtract(&self, a: f64, b: f64, ctx: &RequestContext) -> f64 {
        // Increment the counter
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;

        // Use the passed context for logging
        ctx.debug(format!("Subtracting {} - {}", a, b));

        // Perform the subtraction
        a - b
    }

    /// Multiply two numbers
    fn multiply(&self, a: f64, b: f64, ctx: &RequestContext) -> f64 {
        // Increment the counter
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;

        // Use the passed context for logging
        ctx.debug(format!("Multiplying {} * {}", a, b));

        // Perform the multiplication
        a * b
    }

    /// Divide two numbers
    ///
    /// IMPORTANT: This method demonstrates proper error handling in services.
    /// It checks for error conditions, logs appropriately using the context,
    /// and returns a Result that includes the error condition.
    fn divide(&self, a: f64, b: f64, ctx: &RequestContext) -> Result<f64> {
        // Check for division by zero
        if b == 0.0 {
            ctx.error(format!("Division by zero attempted: {} / {}", a, b));
            return Err(anyhow!("Division by zero"));
        }

        // Increment the counter
        let mut counter = self.counter.lock().unwrap();
        *counter += 1;

        // Use the passed context for logging
        ctx.debug(format!("Dividing {} / {}", a, b));

        // Perform the division
        Ok(a / b)
    }

    /// Get the operation counter
    pub fn get_counter(&self) -> i32 {
        *self.counter.lock().unwrap()
    }

    /// Extract the 'a' and 'b' parameters from the request data
    ///
    /// IMPORTANT: This method demonstrates proper parameter extraction and validation.
    /// It shows how to check for required parameters and handle errors appropriately.
    fn extract_parameters(
        &self,
        data: &ValueType,
        ctx: &RequestContext,
    ) -> anyhow::Result<(f64, f64)> {
        match data {
            ValueType::Map(map) => {
                // Extract 'a' parameter
                let a = if let Some(value) = map.get("a") {
                    value.as_f64().unwrap_or(0.0)
                } else {
                    ctx.error("Missing or invalid parameter 'a'".to_string());
                    return Err(anyhow!("Missing or invalid parameter 'a'"));
                };

                // Extract 'b' parameter
                let b = if let Some(value) = map.get("b") {
                    value.as_f64().unwrap_or(0.0)
                } else {
                    ctx.error("Missing or invalid parameter 'b'".to_string());
                    return Err(anyhow!("Missing or invalid parameter 'b'"));
                };

                Ok((a, b))
            }
            _ => {
                ctx.error("Expected map with 'a' and 'b' parameters".to_string());
                Err(anyhow!("Expected map with 'a' and 'b' parameters"))
            }
        }
    }

    /// Handle the add operation
    ///
    /// IMPORTANT: This demonstrates proper handler implementation for an operation.
    /// It uses the context for logging, extracts parameters, and returns appropriate responses.
    async fn handle_add(
        &self,
        params: Option<ValueType>,
        context: RequestContext,
    ) -> Result<ServiceResponse> {
        // Use the context logger directly
        context.info("Handling add operation request".to_string());

        // Get parameters or use empty map if none provided
        let data = params.unwrap_or(ValueType::Map(std::collections::HashMap::new()));

        // Extract parameters
        match self.extract_parameters(&data, &context) {
            Ok((a, b)) => {
                // Perform the addition
                let result = self.add(a, b, &context);

                // Log result with context
                context.info(format!("Addition successful: {} + {} = {}", a, b, result));

                // Return the result
                Ok(ServiceResponse::ok(ValueType::Number(result)))
            }
            Err(e) => {
                context.error(format!("Parameter extraction failed: {}", e));
                Ok(ServiceResponse::error(400, &format!("{}", e)))
            }
        }
    }

    /// Handle the subtract operation
    async fn handle_subtract(
        &self,
        params: Option<ValueType>,
        context: RequestContext,
    ) -> Result<ServiceResponse> {
        // Use the context logger directly
        context.info("Handling subtract operation request".to_string());

        // Get parameters or use empty map if none provided
        let data = params.unwrap_or(ValueType::Map(std::collections::HashMap::new()));

        // Extract parameters
        match self.extract_parameters(&data, &context) {
            Ok((a, b)) => {
                // Perform the subtraction
                let result = self.subtract(a, b, &context);

                // Log result with context
                context.info(format!(
                    "Subtraction successful: {} - {} = {}",
                    a, b, result
                ));

                // Return the result
                Ok(ServiceResponse::ok(ValueType::Number(result)))
            }
            Err(e) => {
                context.error(format!("Parameter extraction failed: {}", e));
                Ok(ServiceResponse::error(400, &format!("{}", e)))
            }
        }
    }

    /// Handle the multiply operation
    async fn handle_multiply(
        &self,
        params: Option<ValueType>,
        context: RequestContext,
    ) -> Result<ServiceResponse> {
        // Use the context logger directly
        context.info("Handling multiply operation request".to_string());

        // Get parameters or use empty map if none provided
        let data = params.unwrap_or(ValueType::Map(std::collections::HashMap::new()));

        // Extract parameters
        match self.extract_parameters(&data, &context) {
            Ok((a, b)) => {
                // Perform the multiplication
                let result = self.multiply(a, b, &context);

                // Log result with context
                context.info(format!(
                    "Multiplication successful: {} * {} = {}",
                    a, b, result
                ));

                // Return the result
                Ok(ServiceResponse::ok(ValueType::Number(result)))
            }
            Err(e) => {
                context.error(format!("Parameter extraction failed: {}", e));
                Ok(ServiceResponse::error(400, &format!("{}", e)))
            }
        }
    }

    /// Handle the divide operation
    async fn handle_divide(
        &self,
        params: Option<ValueType>,
        context: RequestContext,
    ) -> Result<ServiceResponse> {
        // Use the context logger directly
        context.info("Handling divide operation request".to_string());

        // Get parameters or use empty map if none provided
        let data = params.unwrap_or(ValueType::Map(std::collections::HashMap::new()));

        // Extract parameters
        match self.extract_parameters(&data, &context) {
            Ok((a, b)) => {
                // Perform the division with error handling
                match self.divide(a, b, &context) {
                    Ok(result) => {
                        // Log result with context
                        context.info(format!("Division successful: {} / {} = {}", a, b, result));

                        // Return the result
                        Ok(ServiceResponse::ok(ValueType::Number(result)))
                    }
                    Err(e) => {
                        context.error(format!("Division error: {}", e));
                        Ok(ServiceResponse::error(
                            400,
                            &format!("Division error: {}", e),
                        ))
                    }
                }
            }
            Err(e) => {
                context.error(format!("Parameter extraction failed: {}", e));
                Ok(ServiceResponse::error(400, &format!("{}", e)))
            }
        }
    }
}

#[async_trait]
impl AbstractService for MathService {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn network_id(&self) -> Option<String> {
        self.network_id.clone()
    }

    async fn init(&self, context: LifecycleContext) -> Result<()> {
        // Log the service information being initialized
        context.info(format!(
            "Initializing MathService with name: {}, path: {}",
            self.name, self.path
        ));

        // Create an owned copy to move into the closures
        let owned_self = self.clone();

        // Register add action
        context.info(format!("Registering 'add' action for path: {}", self.path));
        context
            .register_action(
                "add",
                Arc::new(move |params, request_ctx| {
                    let self_clone = owned_self.clone();
                    Box::pin(async move { self_clone.handle_add(params, request_ctx).await })
                }),
            )
            .await?;

        // Register subtract action with metadata
        let owned_self = self.clone();

        context.info(format!(
            "Registering 'subtract' action for path: {}",
            self.path
        ));
        context
            .register_action(
                "subtract",
                Arc::new(move |params, request_ctx| {
                    let self_clone = owned_self.clone();
                    Box::pin(async move { self_clone.handle_subtract(params, request_ctx).await })
                }),
            )
            .await?;

        // Register multiply action with metadata
        let owned_self = self.clone();

        context.info(format!(
            "Registering 'multiply' action for path: {}",
            self.path
        ));
        context
            .register_action(
                "multiply",
                Arc::new(move |params, request_ctx| {
                    let self_clone = owned_self.clone();
                    Box::pin(async move { self_clone.handle_multiply(params, request_ctx).await })
                }),
            )
            .await?;

        // Register divide action with metadata
        let owned_self = self.clone();

        context.info(format!(
            "Registering 'divide' action for path: {}",
            self.path
        ));
        context
            .register_action(
                "divide",
                Arc::new(move |params, request_ctx| {
                    let self_clone = owned_self.clone();
                    Box::pin(async move { self_clone.handle_divide(params, request_ctx).await })
                }),
            )
            .await?;

        // Log successful initialization
        context.info("MathService initialized".to_string());

        Ok(())
    }

    async fn start(&self, context: LifecycleContext) -> Result<()> {
        // Update state in a thread-safe way
        let mut counter = self.counter.lock().unwrap();
        *counter = 0; // Reset counter on start
        drop(counter); // Release the lock

        context.info("MathService started".to_string());
        Ok(())
    }

    async fn stop(&self, context: LifecycleContext) -> Result<()> {
        context.info("MathService stopped".to_string());
        Ok(())
    }
}
