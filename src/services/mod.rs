use anyhow::{anyhow, Result};
use base64;
use log::info;
use serde::{Deserialize, Serialize};
use serde_bytes;
use serde_json::{json, Value};
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid;
use crate::vmap;
use crate::vmap_opt;

use crate::db::SqliteDatabase;
use crate::node::NodeConfig;

// Export the abstract service module
pub mod abstract_service;
// Export the sqlite service module
pub mod sqlite;
// Export the node info service module
pub mod node_info;
// Export the registry service module
pub mod service_registry;
// Export the registry info module
pub mod registry_info;
// Export the utils module
pub mod utils;
// Export the remote service module
pub mod remote;
// Export the anonymous subscriber module
pub mod anonymous_subscriber;
// Export the manager module
pub mod manager;
// Export the distributed registry module (for macros)
pub mod distributed_registry;

// Re-export service types
pub use abstract_service::{AbstractService, ServiceMetadata, ServiceState};
pub use anonymous_subscriber::AnonymousSubscriberService;
pub use node_info::NodeInfoService;
pub use remote::{P2PTransport, RemoteService};
pub use sqlite::SqliteService;
pub use manager::*;

// Re-export distributed registry types (for macros)
pub use distributed_registry::{
    ActionHandler, 
    ProcessHandler, 
    EventSubscription, 
    PublicationInfo,
    ACTION_REGISTRY,
    PROCESS_REGISTRY,
    SUBSCRIPTION_REGISTRY,
    PUBLICATION_REGISTRY,
};

#[cfg(feature = "distributed_slice")]
pub use distributed_registry::distributed_slice;

/// Handler for node requests - used to make service calls and handle events
#[async_trait::async_trait]
pub trait NodeRequestHandler: Send + Sync {
    /// Make a request to a service
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse>;

    /// Publish an event
    async fn publish(&self, topic: String, data: ValueType) -> Result<()>;

    /// Subscribe to events on a topic
    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
    ) -> Result<String>;

    /// Subscribe to events with options
    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(ValueType) -> Result<()> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String>;

    /// Unsubscribe from events
    async fn unsubscribe(&self, topic: String, subscription_id: Option<&str>) -> Result<()>;

    /// List available services
    fn list_services(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Options for subscription management
#[derive(Debug, Clone)]
pub struct SubscriptionOptions {
    /// Optional time-to-live for the subscription (None means no expiration)
    pub ttl: Option<std::time::Duration>,

    /// Maximum number of times this subscription should be triggered before auto-unsubscribing
    /// None means no limit
    pub max_triggers: Option<usize>,

    /// Whether this is a one-time subscription that should be removed after first trigger
    pub once: bool,

    /// A unique ID for this subscription (auto-generated if not provided)
    pub id: Option<String>,
}

impl Default for SubscriptionOptions {
    fn default() -> Self {
        Self {
            ttl: None,
            max_triggers: None,
            once: false,
            id: None,
        }
    }
}

impl SubscriptionOptions {
    /// Create a new set of subscription options
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a time-to-live for the subscription
    pub fn with_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.ttl = Some(ttl);
        self
    }

    /// Set a maximum number of triggers for the subscription
    pub fn with_max_triggers(mut self, max_triggers: usize) -> Self {
        self.max_triggers = Some(max_triggers);
        self
    }

    /// Set this subscription to trigger only once
    pub fn once(mut self) -> Self {
        self.once = true;
        self.max_triggers = Some(1);
        self
    }

    /// Set a custom ID for this subscription
    pub fn with_id(mut self, id: String) -> Self {
        self.id = Some(id);
        self
    }

    /// Generate a random ID if one is not already set
    pub fn ensure_id(&mut self) {
        if self.id.is_none() {
            self.id = Some(uuid::Uuid::new_v4().to_string());
        }
    }
}

/// Response status for service responses
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseStatus {
    /// Success
    Success,
    /// Error
    Error,
}

/// A generic value type that can represent different kinds of data
/// This allows us to pass native data structures through the Node API
/// and only serialize to JSON when needed at transport boundaries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ValueType {
    /// JSON value (for compatibility with external services)
    Json(Value),
    /// HashMap of string keys to ValueType values
    Map(HashMap<String, ValueType>),
    /// Vector of ValueType values
    Array(Vec<ValueType>),
    /// String value
    String(String),
    /// Numeric value
    Number(f64),
    /// Boolean value
    Bool(bool),
    /// Null/None value
    Null,
    /// Binary data
    #[serde(with = "serde_bytes")]
    Bytes(Vec<u8>),
    /// Raw struct data (serialized on demand)
    #[serde(skip)]
    Struct(Arc<dyn SerializableStruct + Send + Sync + 'static>),
}

/// Trait for types that can be stored in a ValueType::Struct
pub trait SerializableStruct: std::fmt::Debug {
    /// Convert to a HashMap representation
    fn to_map(&self) -> Result<HashMap<String, ValueType>>;

    /// Convert to JSON Value
    fn to_json_value(&self) -> Result<Value>;

    /// Get the type name (for debugging)
    fn type_name(&self) -> &'static str;

    /// Clone the struct (required since we can't directly clone a dyn trait)
    fn clone_box(&self) -> Box<dyn SerializableStruct + Send + Sync + 'static>;
}

// We need to create our own Clone impl for Box<dyn SerializableStruct>
// to avoid orphan rule violations
impl Clone for Box<dyn SerializableStruct + Send + Sync + 'static> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

// Define our own wrapper to avoid the orphan rule violation for Arc cloning
pub struct StructArc(pub Box<dyn SerializableStruct + Send + Sync + 'static>);

impl Clone for StructArc {
    fn clone(&self) -> Self {
        StructArc(self.0.clone())
    }
}

/// Implementation for any type that implements Serialize and Debug
impl<T> SerializableStruct for T
where
    T: std::fmt::Debug + serde::Serialize + Clone + Send + Sync + 'static,
{
    fn to_map(&self) -> Result<HashMap<String, ValueType>> {
        // Convert to JSON first
        let json = serde_json::to_value(self)?;

        // Then convert JSON to map
        match json {
            Value::Object(map) => {
                let mut value_map = HashMap::new();
                for (key, value) in map {
                    value_map.insert(key, ValueType::from_json(value));
                }
                Ok(value_map)
            }
            _ => Err(anyhow!("Expected a JSON object, got: {:?}", json)),
        }
    }

    fn to_json_value(&self) -> Result<Value> {
        serde_json::to_value(self).map_err(|e| anyhow!("Serialization error: {}", e))
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }

    fn clone_box(&self) -> Box<dyn SerializableStruct + Send + Sync + 'static> {
        Box::new(self.clone())
    }
}

impl ValueType {
    /// Create a ValueType from a JSON Value
    pub fn from_json(value: Value) -> Self {
        ValueType::Json(value)
    }

    /// Convert this ValueType to a JSON Value
    pub fn to_json(&self) -> Value {
        match self {
            ValueType::Json(value) => value.clone(),
            ValueType::Map(map) => {
                let mut json_map = serde_json::Map::new();
                for (key, value) in map {
                    json_map.insert(key.clone(), value.to_json());
                }
                Value::Object(json_map)
            }
            ValueType::Array(array) => {
                let json_array = array.iter().map(|v| v.to_json()).collect();
                Value::Array(json_array)
            }
            ValueType::String(s) => Value::String(s.clone()),
            ValueType::Number(n) => {
                if let Some(f) = serde_json::Number::from_f64(*n) {
                    Value::Number(f)
                } else {
                    Value::Null
                }
            }
            ValueType::Bool(b) => Value::Bool(*b),
            ValueType::Null => Value::Null,
            ValueType::Bytes(b) => {
                // Base64 encode binary data for JSON representation
                let base64 = base64::encode(b);
                Value::String(base64)
            }
            ValueType::Struct(s) => {
                // Serialize the struct to JSON on demand
                match s.to_json_value() {
                    Ok(v) => v,
                    Err(_) => Value::Null,
                }
            }
        }
    }

    /// Convert this ValueType to a HashMap if possible
    pub fn to_map(&self) -> Result<HashMap<String, ValueType>> {
        match self {
            ValueType::Map(map) => Ok(map.clone()),
            ValueType::Json(Value::Object(obj)) => {
                let mut map = HashMap::new();
                for (k, v) in obj {
                    map.insert(k.clone(), ValueType::from_json(v.clone()));
                }
                Ok(map)
            }
            ValueType::Struct(s) => s.to_map(),
            _ => Err(anyhow!("Cannot convert {:?} to HashMap", self)),
        }
    }

    /// Get a reference to a map if this ValueType is a Map
    pub fn as_map(&self) -> Option<&HashMap<String, ValueType>> {
        match self {
            ValueType::Map(map) => Some(map),
            _ => None,
        }
    }

    /// Get a reference to an array if this ValueType is an Array
    pub fn as_array(&self) -> Option<&Vec<ValueType>> {
        match self {
            ValueType::Array(array) => Some(array),
            _ => None,
        }
    }

    /// Get a reference to a string if this ValueType is a String
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ValueType::String(s) => Some(s),
            _ => None,
        }
    }

    /// Get an integer if this ValueType is an Integer
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ValueType::Number(n) => Some(*n as i64),
            _ => None,
        }
    }

    /// Get a float if this ValueType is a Float
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ValueType::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Get a boolean if this ValueType is a Boolean
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ValueType::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Get a reference to bytes if this ValueType is Bytes
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            ValueType::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Get a map value by key
    pub fn get(&self, key: &str) -> Option<ValueType> {
        match self {
            ValueType::Map(map) => map.get(key).cloned(),
            ValueType::Json(Value::Object(map)) => {
                // Create a ValueType from the JSON value
                map.get(key).map(|v| ValueType::from_json(v.clone()))
            }
            ValueType::Struct(s) => {
                // Try to convert to map first
                match s.to_map() {
                    Ok(map) => map.get(key).cloned(),
                    Err(_) => None,
                }
            }
            _ => None,
        }
    }

    /// Get a reference to a map value by key without cloning (unsafe, only use for readonly)
    pub fn get_ref(&self, key: &str) -> Option<&ValueType> {
        match self {
            ValueType::Map(map) => map.get(key),
            _ => None,
        }
    }
}

/// Convenient conversions from Rust types to ValueType
impl From<String> for ValueType {
    fn from(s: String) -> Self {
        ValueType::String(s)
    }
}

impl From<&str> for ValueType {
    fn from(s: &str) -> Self {
        ValueType::String(s.to_string())
    }
}

impl From<i64> for ValueType {
    fn from(i: i64) -> Self {
        ValueType::Number(i as f64)
    }
}

impl From<i32> for ValueType {
    fn from(i: i32) -> Self {
        ValueType::Number(i as f64)
    }
}

impl From<f64> for ValueType {
    fn from(f: f64) -> Self {
        ValueType::Number(f)
    }
}

impl From<bool> for ValueType {
    fn from(b: bool) -> Self {
        ValueType::Bool(b)
    }
}

impl From<Vec<u8>> for ValueType {
    fn from(b: Vec<u8>) -> Self {
        ValueType::Bytes(b)
    }
}

impl From<HashMap<String, ValueType>> for ValueType {
    fn from(map: HashMap<String, ValueType>) -> Self {
        ValueType::Map(map)
    }
}

impl From<Vec<ValueType>> for ValueType {
    fn from(array: Vec<ValueType>) -> Self {
        ValueType::Array(array)
    }
}

/// Implementation of From<Value> for ValueType
impl From<Value> for ValueType {
    fn from(value: Value) -> Self {
        match value {
            Value::String(s) => ValueType::String(s),
            Value::Number(n) => {
                if n.is_i64() {
                    ValueType::Number(n.as_i64().unwrap_or(0) as f64)
                } else if n.is_u64() {
                    ValueType::Number(n.as_u64().unwrap_or(0) as f64)
                } else {
                    ValueType::Number(n.as_f64().unwrap_or(0.0))
                }
            }
            Value::Bool(b) => ValueType::Bool(b),
            Value::Array(arr) => {
                let mut vec = Vec::new();
                for item in arr {
                    vec.push(ValueType::from(item));
                }
                ValueType::Array(vec)
            }
            Value::Object(obj) => {
                let mut map = HashMap::new();
                for (k, v) in obj {
                    map.insert(k, ValueType::from(v));
                }
                ValueType::Map(map)
            }
            Value::Null => ValueType::Null,
        }
    }
}

impl Default for ValueType {
    fn default() -> Self {
        ValueType::Null
    }
}

/// Service request - represents a request to a service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRequest {
    /// The path of the service
    pub path: String,
    /// The operation to perform on the service
    pub operation: String,
    /// The parameters for the operation
    pub params: Option<ValueType>,
    /// Request ID for tracing
    pub request_id: Option<String>,
    /// Request context object (not serialized)
    #[serde(skip)]
    pub request_context: Arc<RequestContext>,
}

impl ServiceRequest {
    /// Create a new service request
    pub fn new<P: Into<String>, O: Into<String>, V: Into<ValueType>>(
        path: P,
        operation: O,
        params: V,
        request_context: Arc<RequestContext>,
    ) -> Self {
        let path_str = path.into();
        let op_str = operation.into();
        let params_value = params.into();

        ServiceRequest {
            path: path_str,
            operation: op_str,
            params: Some(params_value),
            request_id: None,
            request_context,
        }
    }

    /// Get a parameter from the request
    pub fn get_param(&self, key: &str) -> Option<ValueType> {
        self.params.as_ref().and_then(|v| match v {
            ValueType::Map(map) => map.get(key).cloned(),
            ValueType::Json(json) => json.get(key).map(|v| ValueType::Json(v.clone())),
            _ => None,
        })
    }

    /// Get a parameter reference from the request
    pub fn get_param_ref(&self, key: &str) -> Option<&ValueType> {
        self.params.as_ref().and_then(|v| match v {
            ValueType::Map(map) => map.get(key),
            _ => None,
        })
    }

    /// Convert to JSON params
    pub fn to_json_params(&self) -> Value {
        json!({
            "path": self.path,
            "operation": self.operation,
            "params": self.params.as_ref().map(|v| v.to_json()),
            "request_id": self.request_id,
        })
    }
}

/// Service response - represents a response from a service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceResponse {
    /// Status of the response (success or error)
    pub status: ResponseStatus,
    /// Message describing the response
    pub message: String,
    /// The response data
    pub data: Option<ValueType>,
}

impl ServiceResponse {
    /// Create a success response with data
    pub fn success<T: Into<ValueType>>(message: String, data: Option<T>) -> Self {
        ServiceResponse {
            status: ResponseStatus::Success,
            message,
            data: data.map(|d| d.into()),
        }
    }

    /// Create an error response with a message
    pub fn error<T: Into<String>>(message: T) -> Self {
        ServiceResponse {
            status: ResponseStatus::Error,
            message: message.into(),
            data: None,
        }
    }
}

/// Service Manager - manages services and routes requests
/// This implementation is being refactored to use ServiceRegistry
/// in the future and will eventually be deprecated
pub struct ServiceManager {
    /// Map of service name to service implementation
    services: Arc<RwLock<HashMap<String, Arc<dyn AbstractService>>>>,
    /// The network ID
    network_id: String,
    /// Database connection
    db: Arc<SqliteDatabase>,
    /// Node configuration
    config: Arc<NodeConfig>,
    /// Node handler for making requests
    node_handler: Option<Arc<dyn NodeRequestHandler + Send + Sync>>,
}

impl ServiceManager {
    /// Create a new ServiceManager
    pub fn new(db: Arc<SqliteDatabase>, config: Arc<NodeConfig>) -> Self {
        ServiceManager {
            db,
            network_id: config.network_id.clone(),
            services: Arc::new(RwLock::new(HashMap::new())),
            config,
            node_handler: None,
        }
    }

    /// Create a new ServiceManager from a reference to NodeConfig
    pub fn new_from_ref(db: Arc<SqliteDatabase>, config: &NodeConfig) -> Self {
        // Create a new Arc<NodeConfig> from the reference by cloning
        let config_clone = config.clone();
        let config_arc = Arc::new(config_clone);

        ServiceManager {
            db,
            network_id: config.network_id.clone(),
            services: Arc::new(RwLock::new(HashMap::new())),
            config: config_arc,
            node_handler: None,
        }
    }

    /// Get the network ID
    pub fn network_id(&self) -> &str {
        &self.network_id
    }

    /// Get the node configuration
    pub fn config(&self) -> Arc<NodeConfig> {
        self.config.clone()
    }

    /// Get the database connection
    pub fn get_db(&self) -> &Arc<SqliteDatabase> {
        &self.db
    }

    /// Initialize services
    pub async fn init_services(&mut self) -> Result<()> {
        info!("Initializing services for network ID: {}", self.network_id);

        // Create a node handler for request context
        let node_handler = if let Some(handler) = &self.node_handler {
            handler.clone()
        } else {
            Arc::new(crate::node::DummyNodeRequestHandler {})
        };

        // Create a request context for initialization
        let request_context = Arc::new(RequestContext::new_with_option(
            "service_manager".to_string(),
            vmap_opt! {},
            node_handler,
        ));

        // Initialize the registry service
        let registry_service =
            crate::services::service_registry::ServiceRegistry::new(&self.network_id);
        
        // No need to initialize or start - ServiceRegistry is not an AbstractService

        // Get a reference to use later
        let registry = Arc::new(registry_service);
        
        // Set it as the node handler for the service manager
        if self.node_handler.is_none() {
            self.node_handler = Some(registry.clone());
        }

        // Initialize the SQLite service
        let mut sqlite_service = sqlite::SqliteService::new(self.db.clone(), &self.network_id);
        sqlite_service.init(&request_context).await?;
        sqlite_service.start().await?;

        // Add it to our services map
        self.register_service(Arc::new(sqlite_service)).await?;

        // Initialize the node info service
        let mut node_info_service =
            node_info::NodeInfoService::new(&self.network_id, self.config.clone());
        node_info_service.init(&request_context).await?;
        node_info_service.start().await?;

        // Add it to our services map
        self.register_service(Arc::new(node_info_service)).await?;

        // Load services from the registry database
        let services = self.list_services().await;
        log::info!("Services initialized: {}", services.join(", "));

        Ok(())
    }

    /// Register a service with the ServiceManager
    /// Note: This is being refactored to use ServiceRegistry
    pub async fn register_service(&self, service: Arc<dyn AbstractService>) -> Result<()> {
        let mut services = self.services.write().await;
        let service_name = service.name().to_string();

        info!(
            "Registering service: {} at {}",
            service_name,
            service.path()
        );

        if services.contains_key(&service_name) {
            return Err(anyhow!("Service '{}' is already registered", service_name));
        }

        services.insert(service_name, service);

        Ok(())
    }

    /// Get a service by name
    pub async fn get_service(&self, name: &str) -> Option<Arc<dyn AbstractService>> {
        let services = self.services.read().await;
        services.get(name).cloned()
    }

    /// Get a service by path
    pub async fn get_service_by_path(&self, path: &str) -> Option<Arc<dyn AbstractService>> {
        let services = self.services.read().await;

        // Check for direct matches first
        for service in services.values() {
            if service.path() == path {
                return Some(service.clone());
            }
        }

        // Check for matches with the network ID prefix
        let full_path = if !path.starts_with(&self.network_id) {
            format!("{}/{}", self.network_id, path)
        } else {
            path.to_string()
        };

        for service in services.values() {
            if service.path() == full_path {
                return Some(service.clone());
            }
            // Also check if the path is just the service name
            if service.name() == path {
                return Some(service.clone());
            }
        }

        None
    }

    /// List all services
    pub async fn list_services(&self) -> Vec<String> {
        let services = self.services.read().await;
        services.keys().cloned().collect()
    }

    /// Get all services
    pub async fn get_all_services(&self) -> Vec<Arc<dyn AbstractService>> {
        let services = self.services.read().await;
        services.values().cloned().collect()
    }

    /// Handle a service request
    pub async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Get the service from the path
        let service_path = request.path.clone();
        let service = self.get_service_by_path(&service_path).await;

        match service {
            Some(service) => {
                // Forward the request to the service
                service.handle_request(request).await
            }
            None => {
                // Service not found
                Ok(ServiceResponse::error(format!(
                    "Service not found: {}",
                    service_path
                )))
            }
        }
    }

    /// Clone the ServiceManager
    pub fn clone(&self) -> Self {
        ServiceManager {
            db: self.db.clone(),
            network_id: self.network_id.clone(),
            services: self.services.clone(),
            config: self.config.clone(),
            node_handler: self.node_handler.clone(),
        }
    }

    /// Create a service manager from a NodeConfig
    pub fn create_from_node_config(config: Arc<NodeConfig>, db: Arc<SqliteDatabase>) -> Self {
        ServiceManager {
            db,
            network_id: config.network_id.to_string(),
            services: Arc::new(RwLock::new(HashMap::new())),
            config,
            node_handler: None,
        }
    }

    /// Create a service manager from a NodeConfig (owned version)
    pub fn create_from_config(config: &NodeConfig, db: Arc<SqliteDatabase>) -> Self {
        let config_arc = Arc::new(config.clone());
        ServiceManager {
            db,
            network_id: config.network_id.to_string(),
            services: Arc::new(RwLock::new(HashMap::new())),
            config: config_arc,
            node_handler: None,
        }
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueType::String(s) => write!(f, "{}", s),
            ValueType::Number(n) => write!(f, "{}", n),
            ValueType::Bool(b) => write!(f, "{}", b),
            ValueType::Null => write!(f, "null"),
            ValueType::Json(json) => write!(f, "{}", json),
            ValueType::Map(_) => write!(f, "{{...}}"),
            ValueType::Array(_) => write!(f, "[...]"),
            ValueType::Bytes(_) => write!(f, "<binary>"),
            ValueType::Struct(_) => write!(f, "<struct>"),
        }
    }
}

/// Wrapper type for external types to allow implementing traits for them
pub struct ValueWrapper<T>(pub T);

/// Trait for types that can be converted to ValueType, defined in our crate
pub trait IntoValueType {
    /// Convert to ValueType
    fn into_value_type(self) -> ValueType;
}

// Implement for primitive types to avoid orphan rule violations
impl IntoValueType for &str {
    fn into_value_type(self) -> ValueType {
        ValueType::String(self.to_string())
    }
}

impl IntoValueType for String {
    fn into_value_type(self) -> ValueType {
        ValueType::String(self)
    }
}

impl IntoValueType for i64 {
    fn into_value_type(self) -> ValueType {
        ValueType::Number(self as f64)
    }
}

impl IntoValueType for f64 {
    fn into_value_type(self) -> ValueType {
        ValueType::Number(self)
    }
}

impl IntoValueType for bool {
    fn into_value_type(self) -> ValueType {
        ValueType::Bool(self)
    }
}

/// Convenience macro for creating a ValueType::Map from key-value pairs
///
/// # Examples
///
/// ```
/// use kagi_node::vmap;
/// let map = vmap! {
///     "name" => "John Doe",
///     "age" => 30,
///     "is_active" => true,
///     "stats" => vmap! {
///         "visits" => 42,
///         "likes" => 123
///     }
/// };
/// ```
#[macro_export]
macro_rules! vmap {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut map = std::collections::HashMap::new();
        $(
            map.insert($key.to_string(), $value.into());
        )*
        $crate::services::ValueType::Map(map)
    }};
    () => {{
        let map = std::collections::HashMap::new();
        $crate::services::ValueType::Map(map)
    }};
}

// Helper macro for creating a vmap that returns Option<ValueType>
#[macro_export]
macro_rules! vmap_opt {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let mut map = std::collections::HashMap::new();
        $(
            map.insert($key.to_string(), $value.into());
        )*
        Some($crate::services::ValueType::Map(map))
    }};
    () => {{
        let map = std::collections::HashMap::new();
        Some($crate::services::ValueType::Map(map))
    }};
}

/// Helper function to convert a value to ValueType
/// This is used by the vmap! macro
pub fn to_value_type<T: Serialize>(value: T) -> ValueType {
    // Special case for &str or String
    if let Ok(s) = serde_json::to_value(&value) {
        if s.is_string() {
            if let Value::String(string_val) = s {
                return ValueType::String(string_val);
            }
        }
    }

    // Use serde_json to convert the value to a JSON Value first
    match serde_json::to_value(value) {
        Ok(Value::String(s)) => ValueType::String(s),
        Ok(Value::Number(n)) => {
            if let Some(f) = n.as_f64() {
                ValueType::Number(f)
            } else {
                ValueType::Number(n.as_i64().unwrap_or(0) as f64)
            }
        }
        Ok(Value::Bool(b)) => ValueType::Bool(b),
        Ok(Value::Array(arr)) => {
            let values = arr.into_iter().map(|v| ValueType::Json(v)).collect();
            ValueType::Array(values)
        }
        Ok(Value::Object(obj)) => {
            let mut map = HashMap::new();
            for (k, v) in obj {
                // Recursively convert each value using its appropriate type
                map.insert(
                    k,
                    match v {
                        Value::String(s) => ValueType::String(s),
                        Value::Number(n) => {
                            if let Some(f) = n.as_f64() {
                                ValueType::Number(f)
                            } else {
                                ValueType::Number(n.as_i64().unwrap_or(0) as f64)
                            }
                        }
                        Value::Bool(b) => ValueType::Bool(b),
                        Value::Array(arr) => {
                            let values = arr
                                .into_iter()
                                .map(|item| match item {
                                    Value::String(s) => ValueType::String(s),
                                    Value::Number(n) => {
                                        if let Some(f) = n.as_f64() {
                                            ValueType::Number(f)
                                        } else {
                                            ValueType::Number(n.as_i64().unwrap_or(0) as f64)
                                        }
                                    }
                                    Value::Bool(b) => ValueType::Bool(b),
                                    Value::Object(obj) => ValueType::Json(Value::Object(obj)),
                                    Value::Array(arr) => ValueType::Json(Value::Array(arr)),
                                    Value::Null => ValueType::Null,
                                })
                                .collect();
                            ValueType::Array(values)
                        }
                        Value::Object(nested_obj) => {
                            let mut nested_map = HashMap::new();
                            for (nested_k, nested_v) in nested_obj {
                                nested_map.insert(
                                    nested_k,
                                    match nested_v {
                                        Value::String(s) => ValueType::String(s),
                                        Value::Number(n) => {
                                            if let Some(f) = n.as_f64() {
                                                ValueType::Number(f)
                                            } else {
                                                ValueType::Number(n.as_i64().unwrap_or(0) as f64)
                                            }
                                        }
                                        Value::Bool(b) => ValueType::Bool(b),
                                        Value::Object(obj) => ValueType::Json(Value::Object(obj)),
                                        Value::Array(arr) => ValueType::Json(Value::Array(arr)),
                                        Value::Null => ValueType::Null,
                                    },
                                );
                            }
                            ValueType::Map(nested_map)
                        }
                        Value::Null => ValueType::Null,
                    },
                );
            }
            ValueType::Map(map)
        }
        Ok(Value::Null) => ValueType::Null,
        Err(_) => ValueType::Null,
    }
}

/// Helper function to check if a value can be cast to a specific type
fn option_as<U: 'static>(value: &dyn Any) -> Option<&U> {
    value.downcast_ref::<U>()
}

/// Utility macro for creating a ValueType from a json! macro
#[macro_export]
macro_rules! vjson {
    ($($json:tt)+) => {
        $crate::services::ValueType::from(serde_json::json!($($json)+))
    };
}

/// Request context for service requests
///
/// It allows services to make requests to other services, publish events, and subscribe to events
/// without having direct access to the node instance.
#[derive(Clone)]
pub struct RequestContext {
    /// The service making the request (source)
    pub path: String,
    /// Data associated with the request
    pub data: ValueType,
    /// Node handler for making requests to other services
    #[doc(hidden)]
    pub node_handler: Arc<dyn NodeRequestHandler + Send + Sync>,
}

impl std::fmt::Debug for RequestContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestContext")
            .field("path", &self.path)
            .field("data", &self.data)
            .field("node_handler", &"<node_handler>")
            .finish()
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self {
            path: "default".to_string(),
            data: ValueType::Null,
            node_handler: Arc::new(crate::node::DummyNodeRequestHandler {}),
        }
    }
}

impl RequestContext {
    /// Create a new RequestContext
    pub fn new<P: Into<String>, V: Into<ValueType>>(
        path: P,
        data: V,
        node_handler: Arc<dyn NodeRequestHandler + Send + Sync>,
    ) -> Self {
        RequestContext {
            path: path.into(),
            data: data.into(),
            node_handler,
        }
    }

    /// Create a new RequestContext with optional ValueType
    pub fn new_with_option<P: Into<String>>(
        path: P,
        data: Option<ValueType>,
        node_handler: Arc<dyn NodeRequestHandler + Send + Sync>,
    ) -> Self {
        RequestContext {
            path: path.into(),
            data: data.unwrap_or_else(|| ValueType::Map(HashMap::new())),
            node_handler,
        }
    }

    /// Create a RequestContext for a child request
    pub fn new_child<P: Into<String>, V: Into<ValueType>>(&self, path: P, data: V) -> Self {
        RequestContext {
            path: path.into(),
            data: data.into(),
            node_handler: self.node_handler.clone(),
        }
    }

    /// Get a context value
    pub fn get(&self, key: &str) -> Option<ValueType> {
        match &self.data {
            ValueType::Map(map) => map.get(key).cloned(),
            ValueType::Json(json) => json.get(key).map(|v| ValueType::Json(v.clone())),
            _ => None,
        }
    }

    /// Set a context value
    pub fn set(&mut self, key: &str, value: ValueType) {
        match &mut self.data {
            ValueType::Map(map) => {
                map.insert(key.to_string(), value);
            }
            _ => {
                let mut map = HashMap::new();
                map.insert(key.to_string(), value);
                self.data = ValueType::Map(map);
            }
        }
    }

    /// Publish an event to a topic
    pub async fn publish<T: Into<String>, V: Into<ValueType>>(&self, topic: T, data: V) -> Result<()> {
        self.node_handler.publish(topic.into(), data.into()).await
    }

    /// Subscribe to events on a topic
    pub async fn subscribe<T: Into<String>>(&self, topic: T, callback: impl Fn(ValueType) -> Result<()> + Send + Sync + 'static) -> Result<String> {
        self.node_handler.subscribe(
            topic.into(),
            Box::new(callback),
        ).await
    }

    /// Subscribe to events with additional options
    pub async fn subscribe_with_options<T: Into<String>>(
        &self, 
        topic: T,
        callback: impl Fn(ValueType) -> Result<()> + Send + Sync + 'static,
        options: SubscriptionOptions,
    ) -> Result<String> {
        self.node_handler.subscribe_with_options(
            topic.into(),
            Box::new(callback),
            options,
        ).await
    }

    /// Unsubscribe from events
    pub async fn unsubscribe<T: Into<String>>(&self, topic: T, subscription_id: Option<&str>) -> Result<()> {
        self.node_handler.unsubscribe(topic.into(), subscription_id).await
    }

    /// Subscribe to an event once (auto-unsubscribes after first trigger)
    pub async fn once<T: Into<String>>(&self, topic: T, callback: impl Fn(ValueType) -> Result<()> + Send + Sync + 'static) -> Result<String> {
        let options = SubscriptionOptions::new().once();
        self.subscribe_with_options(topic, callback, options).await
    }
}

impl From<ValueType> for serde_json::Value {
    fn from(value: ValueType) -> Self {
        match value {
            ValueType::String(s) => serde_json::Value::String(s),
            ValueType::Number(n) => {
                if let Some(num) = serde_json::Number::from_f64(n) {
                    serde_json::Value::Number(num)
                } else {
                    serde_json::Value::Null
                }
            },
            ValueType::Bool(b) => serde_json::Value::Bool(b),
            ValueType::Array(arr) => serde_json::Value::Array(arr.into_iter().map(|v| v.into()).collect()),
            ValueType::Map(map) => serde_json::Value::Object(map.into_iter().map(|(k, v)| (k, v.into())).collect()),
            ValueType::Json(v) => v,
            ValueType::Null => serde_json::Value::Null,
            ValueType::Bytes(b) => serde_json::Value::String(base64::encode(&b)),
            ValueType::Struct(s) => s.to_json_value().unwrap_or(serde_json::Value::Null),
        }
    }
}

impl From<f32> for ValueType {
    fn from(value: f32) -> Self {
        ValueType::Number(value as f64)
    }
}

/* Commenting out duplicate definitions
#[async_trait::async_trait]
pub trait AbstractService: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn path(&self) -> &str;
    fn state(&self) -> &str;
    fn description(&self) -> &str;
    fn metadata(&self) -> ServiceMetadata;
    async fn init(&mut self) -> Result<()>;
    async fn start(&mut self) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    async fn handle_request(&self, ctx: RequestContext) -> Result<ServiceResponse>;
}

#[derive(Debug, Clone)]
pub struct ServiceMetadata {
    pub operations: Vec<String>,
    pub description: String,
}

impl ServiceMetadata {
    pub fn new(operations: Vec<String>, description: String) -> Self {
        Self {
            operations,
            description,
        }
    }
}
*/
