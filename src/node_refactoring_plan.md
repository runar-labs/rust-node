## Action Handler Registry

Similar to the event subscription system, we should implement an action handler registry. This will allow services to register action handlers during initialization, rather than implementing a monolithic `handle_action` method.

### Current Implementation
Currently, services must implement the `handle_action` method which dispatches to different logic based on the action name:

```rust
async fn handle_action(
    &self,
    action: &str,
    params: Option<ValueType>,
    context: RequestContext,
) -> Result<ServiceResponse> {
    match action {
        "add" => self.handle_add(params, context).await,
        "subtract" => self.handle_subtract(params, context).await,
        // More action handling...
        _ => Ok(ServiceResponse::error(404, &format!("Unknown action: {}", action))),
    }
}
```

### Proposed Implementation

1. Extend `ServiceRegistry` to store action handlers in addition to event handlers:
   ```rust
   // In ServiceRegistry
   action_handlers: Arc<RwLock<HashMap<String, HashMap<String, ActionHandler>>>>
   ```

2. Define an `ActionHandler` type:
   ```rust
   type ActionHandler = Arc<dyn Fn(Option<ValueType>, RequestContext) -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> + Send + Sync>;
   ```

3. Add a `register_action` method to the initialization context:
   ```rust
   // In LifecycleContext
   pub async fn register_action<F>(&self, action_name: &str, handler: F) -> Result<()> 
   where F: Fn(Option<ValueType>, RequestContext) -> Pin<Box<dyn Future<Output = Result<ServiceResponse>> + Send>> + Send + Sync + 'static
   {
       // Register with the service registry
   }
   ```

4. Update services to register action handlers during initialization:
   ```rust
   async fn init(&self, context: LifecycleContext) -> Result<()> {
       // Register action handlers
       context.register_action("add", |params, ctx| Box::pin(self.handle_add(params, ctx))).await?;
       context.register_action("subtract", |params, ctx| Box::pin(self.handle_subtract(params, ctx))).await?;
       
       // Other initialization...
       Ok(())
   }
   
   // Action handler methods
   async fn handle_add(&self, params: Option<ValueType>, context: RequestContext) -> Result<ServiceResponse> {
       // Implementation...
   }
   ```

5. Update the `NodeRequestHandler` to use the action handler registry:
   ```rust
   async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
       // Look up the action handler and invoke it
   }
   ```

This approach has several advantages:
- Consistent pattern between event handlers and action handlers
- No need for a large `match` statement in `handle_action`
- Better separation of concerns
- Easier to test individual action handlers
- Action handlers can be registered dynamically during initialization

### Migration Strategy

1. Implement the action handler registry in `ServiceRegistry`
2. Update the `LifecycleContext` to include `register_action`
3. Modify the `Node` implementation to use the action handler registry
4. Keep the existing `handle_action` method on the `AbstractService` trait for backward compatibility
5. Gradually migrate services to the new pattern
6. Eventually mark the `handle_action` method as deprecated

Note: This change will require significant refactoring but will result in a more consistent and maintainable codebase.

### Architectural Boundary Clarification

It's important to maintain proper architectural boundaries between components:

1. **ServiceRegistry Responsibilities**:
   - Store and manage service references
   - Provide service lookup functionality
   - Store action handler references
   - Provide methods to register and look up action handlers
   - Track event subscriptions

2. **ServiceRegistry NON-Responsibilities**:
   - ❌ Invoking action handlers directly
   - ❌ Handling service requests
   - ❌ Publishing events

3. **Node Responsibilities**:
   - ✅ Invoking action handlers
   - ✅ Handling service requests
   - ✅ Publishing events
   - Managing the request/response lifecycle

### Implementation Correction

The current implementation incorrectly places `invoke_action_handler` in the `ServiceRegistry`. This violates the architectural boundaries by making the registry responsible for handling requests.

**Correction Plan**:

1. Modify the `ServiceRegistry` to only provide lookup of action handlers:
   ```rust
   // In ServiceRegistry
   pub async fn get_action_handler(
       &self,
       service_path: &str,
       action_name: &str
   ) -> Option<ActionHandler> {
       // Return the handler if found, otherwise None
   }
   ```

2. Update the `Node` to handle invoking action handlers:
   ```rust
   // In Node implementation
   async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
       // Extract request details
       let service_path = &request.path;
       let action = &request.action_or_event;
       
       // Try to get the action handler
       if let Some(handler) = self.service_registry.get_action_handler(service_path, action).await {
           // Node is responsible for invoking the handler
           let context = RequestContext::new_with_logger(/*...*/);
           return handler(request.data, context).await;
       }
       
       // No handler found, return error
       return ServiceResponse::error(404, &format!("No handler for {}/{}", service_path, action));
   }
   ```

This ensures that:
1. The ServiceRegistry only manages registration and lookup
2. The Node maintains responsibility for request handling
3. The architectural boundaries are properly respected

### Simplified Implementation - NO BACKWARD COMPATIBILITY

Based on the project guidelines, we are implementing a clean architecture without backward compatibility. This means:

1. We completely remove the old approach of getting services and calling `handle_action`
2. No fallback mechanisms, no dual paths in the code
3. All services must register action handlers explicitly

**Implementation Changes**:

1. The `ServiceRegistry` *only* stores and provides lookup for action handlers:
   ```rust
   pub async fn get_action_handler(&self, service_path: &TopicPath, action_name: &str) -> Option<ActionHandler>
   ```

2. The `Node.handle_request` implementation is simplified to only one code path:
   ```rust
   async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
       // Create TopicPath from the string path for proper validation
       let topic_path = TopicPath::new(&request.path)?;
       
       // Get action handler from registry
       if let Some(handler) = self.service_registry.get_action_handler(&topic_path, &request.action_or_event).await {
           // Invoke the handler
           return handler(request.data, request.context).await;
       }
       
       // No handler found - error
       Ok(ServiceResponse::error(404, &format!("No handler for {}/{}", request.path, request.action_or_event)))
   }
   ```

3. The `AbstractService` trait's `handle_action` method will eventually be removed

4. Improved Action Registration with Delegate Pattern:
   
   Rather than passing the entire ServiceRegistry to LifecycleContext, we use a delegate pattern:

   ```rust
   // Define an action registrar type
   pub type ActionRegistrar = Arc<dyn Fn(&str, &str, ActionHandler) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;
   
   // Update LifecycleContext to use the action registrar
   pub struct LifecycleContext {
       // ...other fields
       action_registrar: Option<ActionRegistrar>,
   }
   
   // Node creates a LifecycleContext with an appropriate registrar function
   pub fn create_lifecycle_context(&self, service_path: &str) -> LifecycleContext {
       let registry = self.service_registry.clone();
       let registrar = Arc::new(move |service_path, action_name, handler| {
           let registry_clone = registry.clone();
           Box::pin(async move {
               // Parse the service path to a TopicPath for validation
               let topic_path = TopicPath::new(service_path)?;
               registry_clone.register_action(&topic_path, action_name, handler).await
           })
       });
       
       LifecycleContext::new_with_registrar(/*...args*/, registrar)
   }
   ```

5. Service initialization flow changes:
   - The Node no longer explicitly registers services with the ServiceRegistry
   - Instead, services register their action handlers during initialization
   - This simplifies the service registration flow and respects proper boundaries

6. All ServiceRegistry APIs use TopicPath objects, not strings:
   - All path parameters must be TopicPath objects, ensuring proper validation
   - This enforces consistency and type safety across the codebase
   - Node acts as adapter, converting string paths to TopicPath objects when needed
   - APIs updated to use TopicPath:
     ```rust
     // Before
     registry.register_action(service_path_string, action_name, handler);
     registry.get_action_handler(service_path_string, action_name);
     registry.subscribe(topic_string, callback);
     
     // After
     registry.register_action(&topic_path, action_name, handler);
     registry.get_action_handler(&topic_path, action_name);
     registry.subscribe(&topic_path, callback);
     ```

This simplified approach aligns with our architectural principles by:
- Maintaining clear boundaries between components
- Using a delegate pattern to avoid unnecessary dependencies
- Following proper dependency injection principles
- Making the code more maintainable and modular
- Ensuring proper validation of all paths through TopicPath objects

This simplified approach aligns with our architectural principles by maintaining clear boundaries between components and making the code more maintainable.

## Unifying Node Delegate Interfaces

Current Status:
- EventContext has separate fields for event_dispatcher and request_handler
- This creates unnecessary duplication and complexity
- The Node needs to implement multiple traits

Plan:
- Create a unified NodeDelegate trait that combines EventDispatcher and NodeRequestHandler
- Replace the separate handler fields in EventContext with a single node_delegate
- Have Node implement this single trait
- Simplify method signatures to use direct parameters instead of complex objects
- Update all references to use the new unified interface

Benefits:
- Simplified context structures 
- Cleaner, more maintainable API
- Reduced interface duplication
- More consistent parameter patterns

Implementation Steps:
1. Create the NodeDelegate trait
2. Update EventContext to use the new trait
3. Update Node to implement the new trait
4. Update doctest examples
5. Run tests to verify changes

## Important Implementation Guidelines

When implementing these changes, the following principles MUST be followed:

1. **Preserve API Stability**: Never modify existing public APIs without explicit instruction
   - Do not add new methods without approval (e.g., no publish_simple())
   - Do not change method signatures
   - Do not remove methods without explicit direction
   
2. **Follow Documented Intentions**:
   - Each component and method has clearly documented INTENTION
   - Respect these intentions when making changes
   - Read intention documentation before modifying code
   - If the intention is unclear, ask before making changes

3. **Maintain API Consistency**:
   - Similar operations should have similar method signatures
   - Parameters should be consistent across related methods
   - Return types should be consistent across related methods

4. **Avoid Duplication**:
   - Don't create separate methods for similar functionality 
   - Prefer to enhance existing methods rather than creating new ones
   - Consider delegation patterns for specialized behavior

These guidelines help maintain a clean, consistent API while still enabling the necessary architectural improvements. 