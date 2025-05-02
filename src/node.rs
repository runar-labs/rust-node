// Node Implementation
//
// This module provides the Node which is the primary entry point for the Runar system.
// The Node is responsible for managing the service registry, handling requests, and
// coordinating event publishing and subscriptions.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::future::Future;
use std::io::Read;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::{oneshot, RwLock};
use uuid::Uuid;
use async_trait::async_trait;
use std::fmt::Debug;
use runar_common::types::ValueType;
use runar_common::logging::{Logger, Component};
use env_logger;
use chrono;
use rcgen;
use rustls;
use rustls::{Certificate, PrivateKey};
use std::net::SocketAddr;
use socket2;

use crate::network::discovery::multicast_discovery::PeerInfo;
use crate::network::{
    discovery::{MulticastDiscovery, NodeDiscovery, NodeInfo, DiscoveryOptions, DEFAULT_MULTICAST_ADDR},
    ServiceCapability, ActionCapability
};
use crate::network::transport::{
    NetworkTransport, NetworkMessage, MessageCallback, PeerId, MessageHandler, QuicTransport, QuicTransportOptions
};
use crate::routing::TopicPath;
use crate::services::{
    RemoteLifecycleContext,NodeDelegate, RequestContext, ServiceResponse, SubscriptionOptions, 
    RegistryDelegate, PublishOptions, ActionHandler, EventContext
};
use crate::services::abstract_service::{
    AbstractService, ActionMetadata, CompleteServiceMetadata, 
    ServiceState
};
use crate::services::registry_info::RegistryService;
use crate::services::service_registry::{ServiceRegistry, ServiceEntry};
use crate::services::remote_service::RemoteService;
use crate::TransportOptions;

// Type alias for BoxFuture
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Type alias for the discovery factory closure
type DiscoveryFactoryFn = Box<dyn FnOnce(Arc<RwLock<Option<Box<dyn NetworkTransport>>>>, Logger) -> Result<Box<dyn NodeDiscovery>> + Send + Sync + 'static>;

/// Define the load balancing strategy trait
///
/// INTENTION: Provide a common interface for different load balancing
/// strategies that can be plugged into the Node for selecting remote handlers.
pub trait LoadBalancingStrategy: Send + Sync {
    /// Select a handler from the list of handlers
    ///
    /// This method is called when a remote request needs to be routed to one
    /// of multiple available handlers. The implementation should choose a handler
    /// based on its strategy (round-robin, random, weighted, etc.)
    fn select_handler(&self, handlers: &[ActionHandler], context: &RequestContext) -> usize;
}

/// Simple round-robin load balancer
/// 
/// INTENTION: Provide a basic load balancing strategy that distributes
/// requests evenly across all available handlers in a sequential fashion.
#[derive(Debug)]
pub struct RoundRobinLoadBalancer {
    /// The current index for round-robin selection
    current_index: Arc<AtomicUsize>,
}

impl RoundRobinLoadBalancer {
    /// Create a new round-robin load balancer
    pub fn new() -> Self {
        RoundRobinLoadBalancer {
            current_index: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl LoadBalancingStrategy for RoundRobinLoadBalancer {
    fn select_handler(&self, handlers: &[ActionHandler], _context: &RequestContext) -> usize {
        if handlers.is_empty() {
            return 0;
        }
        
        // Get the next index in a thread-safe way
        let index = self.current_index
            .fetch_add(1, Ordering::SeqCst) % handlers.len();
            
        index
    }
}

/// Node Configuration
///
/// INTENTION: Provide configuration options for a Node instance
#[derive(Clone, Debug)]
pub struct NodeConfig {
    /// Node ID (required) - Builder method will either use provided ID or generate one
    pub node_id: String,
    
    /// Primary network ID this node belongs to
    pub default_network_id: String,
    
    /// Additional network IDs this node participates in
    pub network_ids: Vec<String>,
    
    /// Network configuration (None = no networking features)
    pub network_config: Option<NetworkConfig>,
    
    /// Logging configuration options
    pub logging_config: Option<LoggingConfig>,
    
    //FIX: move this to the network config.. local sercvies shuold not have timeout checks.
    /// Request timeout in milliseconds
    pub request_timeout_ms: u64,
}

/// Logging configuration options
#[derive(Clone, Debug)]
pub struct LoggingConfig {
    /// Default log level for all components
    pub default_level: LogLevel,
    
    /// Component-specific log levels that override the default
    pub component_levels: HashMap<ComponentKey, LogLevel>,
}

/// Component key for logging configuration
/// This provides Hash and Eq implementations for identifying components
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKey {
    Node,
    Registry,
    Service,
    Database,
    Network,
    System,
    Custom(String),
}

impl From<Component> for ComponentKey {
    fn from(component: Component) -> Self {
        match component {
            Component::Node => ComponentKey::Node,
            Component::Registry => ComponentKey::Registry,
            Component::Service => ComponentKey::Service,
            Component::Database => ComponentKey::Database,
            Component::Network => ComponentKey::Network,
            Component::NetworkDiscovery => ComponentKey::Network,
            Component::System => ComponentKey::System,
            Component::Custom(name) => ComponentKey::Custom(name.to_string()),
        }
    }
}

/// Log levels matching standard Rust log crate levels
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Off,
}

impl LogLevel {
    /// Convert to log::LevelFilter
    pub fn to_level_filter(&self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
            LogLevel::Off => log::LevelFilter::Off,
        }
    }
}

impl LoggingConfig {
    /// Create a new logging configuration with default settings
    pub fn new() -> Self {
        Self {
            default_level: LogLevel::Info,
            component_levels: HashMap::new(),
        }
    }
    
    /// Create a default logging configuration with Info level for all components
    pub fn default_info() -> Self {
        Self {
            default_level: LogLevel::Info,
            component_levels: HashMap::new(),
        }
    }
    
    /// Set the default log level
    pub fn with_default_level(mut self, level: LogLevel) -> Self {
        self.default_level = level;
        self
    }
    
    /// Set a log level for a specific component
    pub fn with_component_level(mut self, component: Component, level: LogLevel) -> Self {
        self.component_levels.insert(ComponentKey::from(component), level);
        self
    }
    
    /// Apply this logging configuration
    /// 
    /// INTENTION: Configure the global logger solely based on the settings in this
    /// LoggingConfig object. Ignore all environment variables.
    pub fn apply(&self) {
        // Create a new env_logger builder
        let mut builder = env_logger::Builder::new();
        
        // Configure the log level from this config
        builder.filter_level(match self.default_level {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn, 
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
            LogLevel::Off => log::LevelFilter::Off,
        });
        
        // Add a custom formatter to match the desired log format
        builder.format(|buf, record| {
            use std::io::Write;
            
            // Simplified timestamp format without T and Z
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            
            // Format the log level with brackets
            let level = format!("[{}]", record.level());
            
            // Extract the content (which includes our custom prefix format)
            let content = format!("{}", record.args());
            
            // Write the final log message
            writeln!(buf, "[{}] {} {}", timestamp, level, content)
        });
        
        // Initialize the logger, ignoring any errors (in case it's already initialized)
        let _ = builder.try_init();
    }
}

/// Network configuration options
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    /// Load balancing strategy (defaults to round-robin)
    pub load_balancer: Arc<RoundRobinLoadBalancer>,
    
    /// Transport configuration
    pub transport_type: TransportType,
    
    /// Base transport options
    pub transport_options: TransportOptions,
    
    /// QUIC transport options (fully configured)
    pub quic_options: Option<QuicTransportOptions>,
    
    /// Discovery configuration
    pub discovery_providers: Vec<DiscoveryProviderConfig>,
    pub discovery_options: Option<DiscoveryOptions>,
    
    /// Advanced options
    pub connection_timeout_ms: u32,
    pub request_timeout_ms: u32,
    pub max_connections: u32,
    pub max_message_size: usize,
    pub max_chunk_size: usize,
}

impl std::fmt::Display for NetworkConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NetworkConfig: transport:{:?} bind_address:{} msg_size:{}/{}KB timeout:{}ms",
            self.transport_type,
            self.transport_options.bind_address,
            self.max_message_size / 1024,
            self.max_chunk_size / 1024,
            self.connection_timeout_ms
        )?;

        if let Some(discovery_options) = &self.discovery_options {
            if discovery_options.use_multicast {
                write!(
                    f,
                    " multicast discovery interval:{}ms timeout:{}ms",
                    discovery_options.announce_interval.as_millis(),
                    discovery_options.discovery_timeout.as_millis()
                )?;
            }

            write!(f, " ttl:{}s", discovery_options.node_ttl.as_secs())?;
        }

        Ok(())
    }
}

/// Transport type enum
#[derive(Clone, Debug)]
pub enum TransportType {
    Quic,
    // Add other transport types as needed
}

/// Discovery provider configuration using proper typed options instead of string hashmaps
#[derive(Clone, Debug)]
pub enum DiscoveryProviderConfig {
    /// Multicast discovery configuration
    Multicast(MulticastDiscoveryOptions),
    
    /// Static discovery configuration
    Static(StaticDiscoveryOptions),
    
    // Other discovery types can be added here as needed
}

/// Options specific to multicast discovery
#[derive(Clone, Debug)]
pub struct MulticastDiscoveryOptions {
    /// Multicast group address
    pub multicast_group: String,
    
    /// Announcement interval 
    pub announce_interval: Duration,
    
    /// Discovery timeout
    pub discovery_timeout: Duration,
    
    /// Time-to-live for discovered nodes
    pub node_ttl: Duration,
    
    /// Whether to use multicast for discovery
    pub use_multicast: bool,
    
    /// Whether to only discover on local network
    pub local_network_only: bool,
}

/// Options specific to static discovery
#[derive(Clone, Debug)]
pub struct StaticDiscoveryOptions {
    /// List of static node addresses
    pub node_addresses: Vec<String>,
    
    /// Refresh interval for checking static nodes
    pub refresh_interval: Duration,
}

impl DiscoveryProviderConfig {
    pub fn default_multicast() -> Self {
        DiscoveryProviderConfig::Multicast(MulticastDiscoveryOptions {
            multicast_group: DEFAULT_MULTICAST_ADDR.to_string(),
            announce_interval: Duration::from_secs(30),
            discovery_timeout: Duration::from_secs(30),
            node_ttl: Duration::from_secs(60),
            use_multicast: true,
            local_network_only: true,
        })
    }
    
    pub fn default_static(addresses: Vec<String>) -> Self {
        DiscoveryProviderConfig::Static(StaticDiscoveryOptions {
            node_addresses: addresses,
            refresh_interval: Duration::from_secs(60),
        })
    }
}


impl NetworkConfig {
    /// Creates a new network configuration with default settings.
    pub fn new() -> Self {
        Self {
            load_balancer: Arc::new(RoundRobinLoadBalancer::new()),
            transport_type: TransportType::Quic,
            transport_options: TransportOptions::default(),
            quic_options: None,
            discovery_providers: Vec::new(),
            discovery_options: Some(DiscoveryOptions::default()),
            connection_timeout_ms: 60000,
            request_timeout_ms: 10000,
            max_connections: 100,
            max_message_size: 1024 * 1024 * 10, // 10MB default
            max_chunk_size: 1024 * 1024 * 10, // 10MB default
        }
    }
    
    /// Create a new NetworkConfig with default QUIC transport settings
    /// 
    /// This is a convenience method that sets up a NetworkConfig with:
    /// - Default QUIC transport
    /// - Auto port selection
    /// - Localhost binding
    /// - Secure defaults for connection handling
    pub fn with_quic(validate_certificates: bool) -> Self {
        // Default QUIC configuration 
        let mut quic_options = QuicTransportOptions::new().with_verify_certificates(validate_certificates);

        // Generate self-signed certificates if none are provided
        if quic_options.certificates.is_none() && quic_options.cert_path.is_none() {
            match generate_self_signed_cert() {
                Ok((cert, key)) => {
                    quic_options = quic_options
                        .with_certificates(vec![cert])
                        .with_private_key(key);
                },
                Err(e) => {
                    // Using static logging since we don't have a logger instance here
                    log::error!("Failed to generate self-signed certificate: {}", e);
                    // Continue without certs, will fail later when actually used
                }
            }
        }
 
        Self {
            load_balancer: Arc::new(RoundRobinLoadBalancer::new()),
            transport_type: TransportType::Quic, 
            transport_options: TransportOptions::default(),
            quic_options: Some(quic_options),
            discovery_providers: Vec::new(), // No providers by default
            discovery_options: None, // No discovery options by default (disabled)
            connection_timeout_ms: 30000,
            request_timeout_ms: 30000,
            max_connections: 100,
            max_message_size: 1024 * 1024, // 1MB
            max_chunk_size: 1024 * 1024, // 1MB
        }
    }
    
    pub fn with_transport_type(mut self, transport_type: TransportType) -> Self {
        self.transport_type = transport_type;
        self
    }
     
    pub fn with_quic_options(mut self, options: QuicTransportOptions) -> Self {
        self.quic_options = Some(options);
        self
    }
    
    pub fn with_discovery_provider(mut self, provider: DiscoveryProviderConfig) -> Self {
        self.discovery_providers.push(provider);
        self
    }
    
    pub fn with_discovery_options(mut self, options: DiscoveryOptions) -> Self {
        self.discovery_options = Some(options);
        self
    }
    
    /// Enable multicast discovery with default settings
    /// 
    /// This is a convenience method that configures:
    /// - Default multicast discovery provider
    /// - Default discovery options
    pub fn with_multicast_discovery(mut self) -> Self {
        // Clear existing providers and add the default multicast provider
        self.discovery_providers.clear();
        self.discovery_providers.push(DiscoveryProviderConfig::default_multicast());
        
        // Use default discovery options
        self.discovery_options = Some(DiscoveryOptions::default());
        
        self
    }
}

impl NodeConfig {
    /// Create a new configuration with the specified node ID and network ID
    pub fn new(node_id: impl Into<String>, default_network_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            default_network_id: default_network_id.into(),
            network_ids: Vec::new(),
            network_config: None,
            logging_config: Some(LoggingConfig::default_info()), // Default to Info logging
            request_timeout_ms: 30000, // 30 seconds
        }
    }
    
    /// Generate a node ID if not provided
    pub fn new_with_generated_id(default_network_id: impl Into<String>) -> Self {
        let node_id = Uuid::new_v4().to_string();
        Self::new(node_id, default_network_id)
    }
    
    /// Add network configuration
    pub fn with_network_config(mut self, config: NetworkConfig) -> Self {
        self.network_config = Some(config);
        self
    }

    /// Add logging configuration
    pub fn with_logging_config(mut self, config: LoggingConfig) -> Self {
        self.logging_config = Some(config);
        self
    }

    /// Add additional network IDs
    pub fn with_additional_networks(mut self, network_ids: Vec<String>) -> Self {
        self.network_ids = network_ids;
        self
    }
    
    /// Set the request timeout in milliseconds
    pub fn with_request_timeout(mut self, timeout_ms: u64) -> Self {
        self.request_timeout_ms = timeout_ms;
        self
    }
}

// Implement Display for NodeConfig to enable logging it directly
impl std::fmt::Display for NodeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "NodeConfig: node_id:{} network:{} request_timeout:{}ms",
            self.node_id,
            self.default_network_id,
            self.request_timeout_ms
        )?;
        
        // Add network configuration details if available
        if let Some(network_config) = &self.network_config {
            write!(f, " {}", network_config)?;
        }
        
        Ok(())
    }
}

/// The Node is the main entry point for the application
/// 
/// INTENTION: Provide a high-level interface for services to communicate
/// with each other, for registering and discovering services, and for
/// managing the lifecycle of services.
pub struct Node {
    /// The network ID for this node
    pub(crate) network_id: String,

    //network_ids that this node participates in.
    pub(crate) network_ids: Vec<String>,

    /// The node ID for this node
    pub(crate) peer_id: PeerId,

    /// Configuration for this node
    pub(crate) config: NodeConfig,

    /// The service registry for this node
    pub(crate) service_registry: Arc<ServiceRegistry>,

    /// Logger instance
    pub(crate) logger: Logger,
    
    /// Flag indicating if the node is running
    pub(crate) running: AtomicBool,
    
    /// Flag indicating if this node supports networking
    /// This is set when networking is enabled in the config
    pub(crate) supports_networking: bool,

    /// Network transport for connecting to remote nodes
    pub(crate) network_transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,

    /// Load balancer for selecting remote handlers
    pub(crate) load_balancer: Arc<RwLock<dyn LoadBalancingStrategy>>,

    /// Default load balancer to use if none is provided
    pub(crate) default_load_balancer: RoundRobinLoadBalancer,

    /// Pending requests waiting for responses, keyed by correlation ID
    pub(crate) event_payloads: Arc<RwLock<HashMap<String, oneshot::Sender<Result<ServiceResponse>>>>>,
}

// Implementation for Node
impl Node {
    /// Create a new Node with the given configuration
    ///
    /// INTENTION: Initialize a new Node with the specified configuration, setting up
    /// all the necessary components and internal state. This is the primary
    /// entry point for creating a Node instance.
    ///
    /// This constructor does not start services - call start() separately
    /// after registering services.
    pub async fn new(config: NodeConfig) -> Result<Self> {
        let node_id = config.node_id.clone();
        let logger = Logger::new_root(Component::Node, &node_id);
        
        // Apply logging configuration (default to Info level if none provided)
        if let Some(logging_config) = &config.logging_config {
            logging_config.apply();
            logger.debug("Applied custom logging configuration");
        } else {
            // Apply default Info logging when no configuration is provided
            let default_config = LoggingConfig::default_info();
            default_config.apply();
            logger.debug("Applied default Info logging configuration");
        }
        
        // Clone fields before moving config
        let default_network_id = config.default_network_id.clone();
        //stgore this in the node struct.. will be used later features..
        let network_ids = config.network_ids.clone();
        let networking_enabled = config.network_config.is_some();
        
        let mut network_ids = network_ids.clone();
        network_ids.push(default_network_id.clone());
        network_ids.dedup();    

        logger.info(format!("Initializing node '{}' in network '{}'...", node_id, default_network_id));
        
        let service_registry = Arc::new(ServiceRegistry::new(logger.clone()));
        let peer_id = PeerId::new(node_id.clone());
        // Create the node (with network fields now included)
        let mut node = Self {
            network_id: default_network_id,
            network_ids,
            peer_id,
            config,
            logger: logger.clone(),
            service_registry,
            running: AtomicBool::new(false),
            supports_networking: networking_enabled,
            network_transport: Arc::new(RwLock::new(None)),
            load_balancer: Arc::new(RwLock::new(RoundRobinLoadBalancer::new())),
            default_load_balancer: RoundRobinLoadBalancer::new(),
            event_payloads: Arc::new(RwLock::new(HashMap::new())),
        };
        
        // Register the registry service
        let registry_service = RegistryService::new(
            logger.clone(),
            Arc::new(node.clone()) as Arc<dyn RegistryDelegate>,
        );
        
        // Add the registry service to the node
        node.add_service(registry_service).await?;
        
        Ok(node)
    }
    
    /// Add a service to this node
    ///
    /// 1: validate service path    
    /// 2: create topic path
    /// 3: create service entry
    /// 4: register service
    /// 5: update service state to initialized
    /// 
    /// INTENTION: Register a service with this node, making its actions available
    /// for requests and allowing it to receive events. This method initializes the
    /// service but does not start it - services are started when the node is started.
    pub async fn add_service<S: AbstractService + 'static>(&mut self, service: S) -> Result<()> {
        let service_path = service.path();
        let service_name = service.name();
        let default_network_id = self.network_id.to_string();
        let service_network_id = match service.network_id() {
            Some(id) => id,
            None => default_network_id,  
        };

        self.logger.info(format!("Adding service '{}' to node using path {}", service_name, service_path));
        self.logger.debug(format!("network id {}", service_network_id ));   
        
        let registry = Arc::clone(&self.service_registry);
        // Create a proper topic path for the service
        let service_topic = match crate::routing::TopicPath::new(service_path, &service_network_id) {
            Ok(tp) => tp,
            Err(e) => {
                self.logger.error(format!("Failed to create topic path for service name:{} path:{} error:{}", service_name, service_path, e));
                registry.update_service_state(service_path, ServiceState::Error).await?;
                return Err(anyhow!("Failed to create topic path for service {}: {}", service_name, e));
            }
        };

        // Create a lifecycle context for initialization
        let init_context = crate::services::LifecycleContext::new(
            &service_topic, 
            self.logger.clone().with_component(runar_common::Component::Service)
        ).with_node_delegate(Arc::new(self.clone()));
        
        // Initialize the service using the context
        if let Err(e) = service.init(init_context).await {
            self.logger.error(format!("Failed to initialize service: {}, error: {}", service_name, e));
            registry.update_service_state(service_path, ServiceState::Error).await?;
            return Err(anyhow!("Failed to initialize service: {}", e));
        }
        registry.update_service_state(service_path, ServiceState::Initialized).await?;

        // Service initialized successfully, create the ServiceEntry and register it
        let service_entry = ServiceEntry {
            service: Arc::new(service),
            service_topic,
            service_state: ServiceState::Initialized,
        };        
        registry.register_local_service(Arc::new(service_entry)).await?;
        
        Ok(())
    }
 
    /// Start the Node and all registered services
    ///
    /// INTENTION: Initialize the Node's internal systems and start all registered services.
    /// This method:
    /// 1. Checks if the Node is already started to ensure idempotency
    /// 2. Get all local services from the registry 
    /// 3. Initialize and start each service
    /// 4. Update service state to running
    /// 5. Start networking if enabled
    ///
    /// When network functionality is added, this will also advertise services to the network.
    pub async fn start(&mut self) -> Result<()> {
        self.logger.info("Starting node...");
        
        if self.running.load(Ordering::SeqCst) {
            self.logger.warn("Node already running");
            return Ok(());
        }
        
        // Get services directly from the registry
        let registry = Arc::clone(&self.service_registry);
        let local_services = registry.get_local_services().await;
        
        // start each service
        for (service_topic, service_entry) in local_services {
            self.logger.info(format!("Initializing service: {}", service_topic));
            
            let service = service_entry.service.clone();
            let service_path = service.path();

            // Create a lifecycle context for starting
            let start_context = crate::services::LifecycleContext::new(
                &service_topic, 
                self.logger.clone().with_component(runar_common::Component::Service)
            );
            
            // Start the service using the context
            if let Err(e) = service.start(start_context).await {
                self.logger.error(format!("Failed to start service: {}, error: {}", service_topic, e));
                registry.update_service_state(service_path, ServiceState::Error).await?;
                continue;
            }
            
            registry.update_service_state(service_path, ServiceState::Running).await?;
        }
        
        // Start networking if enabled
        if self.supports_networking {
            if let Err(e) = self.start_networking().await {
                self.logger.error(format!("Failed to start networking components: {}", e));
                return Err(e); 
            }
        }
        
        self.logger.info("Node started successfully");
        self.running.store(true, Ordering::SeqCst);
        
        Ok(())
    }
    
    /// Stop the Node and all registered services
    ///
    /// INTENTION: Gracefully stop the Node and all registered services. This method:
    /// 1. Transitions the Node to the Stopping state
    /// 2. Stops all registered services in the reverse order they were started
    /// 3. Updates the service state in the metadata as each service stops
    /// 4. Handles any errors during service shutdown
    /// 5. Transitions the Node to the Stopped state
    pub async fn stop(&mut self) -> Result<()> {
        self.logger.info("Stopping node...");
        
        if !self.running.load(Ordering::SeqCst) {
            self.logger.warn("Node already stopped");
            return Ok(());
        }
        
        self.running.store(false, Ordering::SeqCst);
        
        // Get services directly and stop them
        let registry = Arc::clone(&self.service_registry);
        let local_services = registry.get_local_services().await;
        
        self.logger.info("Stopping services...");
        // Stop each service
        for (service_topic, service_entry) in local_services {
            self.logger.info(format!("Stopping service: {}", service_topic));
            
            // Extract the service from the entry
            let service = service_entry.service.clone();
            let service_path = service.path();
            
            // Create a lifecycle context for stopping
            let stop_context = crate::services::LifecycleContext::new(
                &service_topic, 
                self.logger.clone().with_component(runar_common::Component::Service)
            );
            
            // Stop the service using the context
            if let Err(e) = service.stop(stop_context).await {
                self.logger.error(format!("Failed to stop service: {}, error: {}", service_topic, e));
                continue;
            }
            
            registry.update_service_state(service_path, ServiceState::Stopped).await?;
        }

        self.logger.info("Stopping networking...");

         // Shut down networking if enabled
         if self.supports_networking {
            if let Err(e) = self.shutdown_network().await {
                self.logger.error(format!("Error shutting down network: {}", e));
            }
        }
        
        self.logger.info("Node stopped successfully");
        
        Ok(())
    }

    /// Starts the networking components (transport and discovery).
    /// This should be called internally as part of the node.start process.
    async fn start_networking(&self) -> Result<()> {
        self.logger.info("Starting networking components...");
        
        if !self.supports_networking {
            self.logger.info("Networking is disabled, skipping network initialization");
            return Ok(());
        }
        
        // Get the configuration
        let config = &self.config;
        let network_config = config.network_config.as_ref()
            .ok_or_else(|| anyhow!("Network configuration is required"))?;
        
        // Log the network configuration
        self.logger.info(format!("Network config: {}", network_config));
        
        // Initialize the network transport
        if self.network_transport.read().await.is_none() {
            self.logger.info("Initializing network transport...");
            
            // Create network transport using the factory pattern based on transport_type
            let node_identifier =   self.peer_id.clone();
            let transport = self.create_transport(network_config, node_identifier).await?;
            
            self.logger.info("Initializing network transport layer...");
            transport.start().await.map_err(|e| anyhow!("Failed to initialize transport: {}", e))?;
            
            // Store the transport
            let mut transport_guard = self.network_transport.write().await;
            *transport_guard = Some(transport);
        }
         
        
        // Initialize discovery if enabled
        if let Some(discovery_options) = &network_config.discovery_options {
            self.logger.info("Initializing node discovery providers...");
            
            // Check if any providers are configured
            if network_config.discovery_providers.is_empty() {
                return Err(anyhow!("No discovery providers configured"));
            }
 
            // Iterate through all discovery providers and initialize each one
            for provider_config in &network_config.discovery_providers {
                // Create a discovery provider instance
                let provider_type = format!("{:?}", provider_config);
                
                match self.create_discovery_provider(
                    provider_config,
                    Some(discovery_options.clone())
                ).await {
                    Ok(discovery) => {
                        // Configure discovery listener for this provider
                        let node_clone = self.clone();
                        let provider_type_clone = provider_type.clone();
                        
                        discovery.set_discovery_listener(Box::new(move |peer_info| {
                            let node = node_clone.clone();
                            let provider_type_clone = provider_type_clone.clone();
                            
                            tokio::spawn(async move {
                                if let Err(e) = node.handle_discovered_node(peer_info).await {
                                    node.logger.error(format!("Failed to handle node discovered by {} provider: {}", 
                                        provider_type_clone, e));
                                }
                            });
                        })).await?;
                        
                        // Start announcing on this provider
                        self.logger.info(format!("Starting to announce on {:?} discovery provider", provider_type));
                        discovery.start_announcing().await?;
                    },
                    Err(e) => {
                        // Log the error but continue with other providers
                        self.logger.error(format!("Failed to initialize {:?} discovery provider: {}", 
                            provider_type, e));
                        // Don't return immediately, try other providers
                    }
                }
            }
        }
        
        self.logger.info("Networking started successfully");
        
        Ok(())
    }

    /// Create a transport instance based on the transport type in the config
    async fn create_transport(&self, network_config: &NetworkConfig, node_id: PeerId) -> Result<Box<dyn NetworkTransport>> {
        match network_config.transport_type {
            TransportType::Quic => {
                // If QUIC is specified, use the QUIC transport
                if let Some(_quic_options) = &network_config.quic_options {
                    self.logger.debug("Creating QUIC transport");
                    
                    // Clone the NetworkConfig to avoid reference issues
                    let owned_config = network_config.clone();
                    
                    let transport = QuicTransport::new(
                        node_id,
                        owned_config,
                        self.logger.clone()
                    );
                    
                    self.logger.debug("QUIC transport created");
                    Ok(Box::new(transport))
                } else {
                    return Err(anyhow!("QUIC transport type specified but no QUIC options provided"));
                }
            }
        }
    }

    /// Create a discovery provider based on the provider type
    async fn create_discovery_provider(
        &self, 
        provider_config: &DiscoveryProviderConfig,
        discovery_options: Option<DiscoveryOptions>
    ) -> Result<Box<dyn NodeDiscovery>> {

        let node_info = self.get_local_node_info().await?;

        match provider_config {
            DiscoveryProviderConfig::Multicast(options) => {
                self.logger.info("Creating MulticastDiscovery provider with config options");
                // Use .await to properly wait for the async initialization
                let discovery = MulticastDiscovery::new(node_info , discovery_options.unwrap_or_default(),self.logger.with_component(Component::NetworkDiscovery)).await?;
                Ok(Box::new(discovery) as Box<dyn NodeDiscovery>)
            },
            DiscoveryProviderConfig::Static(options) => {
                self.logger.info("Static discovery provider configured");
                // Implement static discovery when needed
                Err(anyhow!("Static discovery provider not yet implemented"))
            },
            // Add other discovery types as they're implemented
        }
    }

    /// Handle discovery of a new node
    pub async fn handle_discovered_node(&self, peer_info: PeerInfo) -> Result<()> {
        // Skip if networking is not enabled
        if !self.supports_networking {
            self.logger.warn("Received node discovery event but networking is disabled");
            return Ok(());
        }

        let local_node_info = self.get_local_node_info().await?;

        let peer_public_key = peer_info.public_key.clone(); 
        
        self.logger.info(format!("Discovery listener found node: {}", peer_public_key));
        
        let transport = self.network_transport.read().await;
        if let Some(transport) = transport.as_ref() {
             
            // Store the discovered node using the transport
            if let Ok(_) = transport.connect_node(peer_info.clone(), local_node_info).await {
                self.logger.info(format!("connected to node: {} - will", peer_public_key));
                


                // // Always connect to the peer for now
                // let should_connect = true; // In future: perform public/private key checks
                
                // if should_connect {
                     
                //     // Try to connect to the peer
                //     if let Ok(addr) = address.parse::<SocketAddr>() {
                //         match transport.connect(peer_id.clone(), addr).await {
                //             Ok(_) => {
                //                 self.logger.info(format!("Connected to peer: {}", peer_id));
                                
                //                 // Process capabilities AFTER successful connection
                //                 let _ = self.process_remote_capabilities(
                //                     node_info.clone()
                //                 ).await;
                //             },
                //             Err(e) => self.logger.warn(format!("Failed to connect to peer: {} - Error: {}", 
                //                 peer_id, e))
                //         }
                //     } else {
                //         self.logger.warn(format!("Invalid address format for peer: {} - {}", 
                //             peer_id, address));
                //     }
                // }
            } else {
                self.logger.warn(format!("Failed to add node to registry: {}", peer_public_key));
            }
        } else {
            self.logger.warn("No transport available to handle discovered node");
        }
        
        Ok(())
    }

    /// Handle a network message
    async fn handle_network_message(&self, message: NetworkMessage) -> Result<()> {
        // Skip if networking is not enabled
        if !self.supports_networking {
            self.logger.warn("Received network message but networking is disabled");
            return Ok(());
        }
        
        self.logger.debug(format!("Received network message: {:?}", message));
        
        // Match on message type
        match message.message_type.as_str() {
            "Request" => self.handle_network_request(message).await,
            "Response" => self.handle_network_response(message).await,
            "Event" => self.handle_network_event(message).await,
            // "Discovery" => self.handle_network_discovery(message).await,
            _ => {
                self.logger.warn(format!("Unknown message type: {}", message.message_type));
                Ok(())
            }
        }
    }

    /// Handle a network request
    async fn handle_network_request(&self, message: NetworkMessage) -> Result<()> {
        // Skip if networking is not enabled
        if !self.supports_networking {
            self.logger.warn("Received network request but networking is disabled");
            return Ok(());
        }
        
        self.logger.info(format!("Handling network request from {}", message.source));
        
        // Assume requests have one payload for now. If batching is allowed, this needs iteration.
        if message.payloads.is_empty() {
            return Err(anyhow!("Received request message with no payloads"));
        }
        let (topic, params, correlation_id) = message.payloads[0].clone(); // Clone to own the values
        
        // Get local node ID (remains the same)
        let network_transport = self.network_transport.read().await;
        let local_peer_id = self.peer_id.clone();
        drop(network_transport); // Drop read lock
        
        // Process the request locally using extracted topic and params
        self.logger.debug(format!("Processing network request for topic: {}", topic));
        match self.request(topic.as_str(), params).await {
            Ok(response) => {
                // Create response message - destination is the original source
                let response_message = NetworkMessage {
                    source: local_peer_id , // Source is now self
                    destination: message.source.clone(), // Destination is the original request source
                    message_type: "Response".to_string(),
                    // Response payload tuple: Use original topic, response data, and original correlation_id
                    payloads: vec![(topic, response.data.unwrap_or(ValueType::Null), correlation_id)],
                };
                
                // Check if networking is still enabled before trying to send response
                if !self.supports_networking {
                    self.logger.warn("Can't send response - networking is disabled");
                    return Ok(());
                }
                
                // Send the response via transport
                let transport_guard = self.network_transport.read().await;
                if let Some(transport) = transport_guard.as_ref() {
                    if let Err(e) = transport.send_message(response_message).await {
                        self.logger.error(format!("Failed to send response message: {}", e));
                        // Consider returning error or just logging?
                    } else {
                        self.logger.debug("Sent response message to remote node");
                    }
                } else {
                    self.logger.warn("No network transport available to send response");
                }
            },
            Err(e) => {
                // Create error response message - destination is the original source
                let error_payload = ValueType::Map(HashMap::from([
                    ("error".to_string(), ValueType::Bool(true)),
                    ("message".to_string(), ValueType::String(e.to_string())),
                ]));
                let response_message = NetworkMessage {
                    source: local_peer_id , // Source is self
                    destination: message.source.clone(), // Destination is the original request source
                    message_type: "Error".to_string(), // Use Error type
                    // Error payload tuple: Use original topic, error details, and original correlation_id
                    payloads: vec![(topic, error_payload, correlation_id)], 
                };
                
                // Check if networking is still enabled before trying to send error response
                if !self.supports_networking {
                    self.logger.warn("Can't send error response - networking is disabled");
                    return Ok(());
                }
                
                // Send the error response via transport
                let transport_guard = self.network_transport.read().await;
                if let Some(transport) = transport_guard.as_ref() {
                    if let Err(e) = transport.send_message(response_message).await {
                        self.logger.error(format!("Failed to send error response message: {}", e));
                    } else {
                        self.logger.debug(format!("Sent error response to remote node: {}", e));
                    }
                } else {
                    self.logger.warn("No network transport available to send error response");
                }
            }
        }
        
        Ok(())
    }

    /// Handle a network response
    async fn handle_network_response(&self, message: NetworkMessage) -> Result<()> {
        // Skip if networking is not enabled
        if !self.supports_networking {
            self.logger.warn("Received network response but networking is disabled");
            return Ok(());
        }
        
        self.logger.debug(format!("Handling network response: {:?}", message));
        
        // Extract payloads and handle them
        for (topic, payload_data, correlation_id) in message.payloads {
            // Only process if we have an actual correlation ID
            self.logger.debug(format!("Processing response for topic {}, correlation ID: {}", topic, correlation_id));
            
            // Find any pending response handlers
            if let Some(response_sender) = self.event_payloads.write().await.remove(&correlation_id) {
                self.logger.debug(format!("Found response handler for correlation ID: {}", correlation_id));
                
                // Create a success response
                let response = ServiceResponse::ok(payload_data);
                
                // Send the response through the oneshot channel
                match response_sender.send(Ok(response)) {
                    Ok(_) => self.logger.debug(format!("Successfully sent response for correlation ID: {}", correlation_id)),
                    Err(e) => self.logger.error(format!("Failed to send response data: {:?}", e)),
                }
            } else {
                self.logger.warn(format!("No response handler found for correlation ID: {}", correlation_id));
            }
        }
        
        Ok(())
    }

    /// Handle a network event
    async fn handle_network_event(&self, message: NetworkMessage) -> Result<()> {
        // Skip if networking is not enabled
        if !self.supports_networking {
            self.logger.warn("Received network event but networking is disabled");
            return Ok(());
        }
        
        self.logger.debug(format!("Handling network event: {:?}", message));
        
        // Process each payload separately
        for (topic, payload, _) in message.payloads {
            // Skip processing if topic is empty
            if topic.is_empty() {
                self.logger.warn("Received event with empty topic, skipping");
                continue;
            }
            
            // Create topic path
            let topic_path = match TopicPath::new(&topic, &self.network_id) {
                Ok(tp) => tp,
                Err(e) => {
                    self.logger.error(format!("Invalid topic path for event: {}", e));
                    continue;
                }
            };
            
            // Create proper event context
            let event_context = Arc::new(EventContext::new(
                &topic_path,
                self.logger.clone().with_component(Component::Service)
            ));
            
            // Get subscribers for this topic
            let subscribers = self.service_registry.get_local_event_subscribers(&topic_path).await;
            
            if subscribers.is_empty() {
                self.logger.debug(format!("No subscribers found for topic: {}", topic));
                continue;
            }
            
            // Notify all subscribers
            for (_subscription_id, callback) in subscribers {
                let ctx = event_context.clone();
                let payload_clone = payload.clone();
                
                // Invoke callback. errors are logged but not propagated to avoid affecting other subscribers
                let result = callback(ctx, payload_clone).await;
                if let Err(e) = result {
                    self.logger.error(format!("Error in subscriber callback: {}", e));
                }
            }
        }
        
        Ok(())
    }
 

    /// Handle a request for a specific action - Stable API DO NOT CHANGE UNLESS EXPLICITLY ASKED TO DO SO!
    ///
    /// INTENTION: Route a request to the appropriate action handler,
    /// first checking local handlers and then remote handlers.
    /// Apply load balancing when multiple remote handlers are available.
    /// 
    /// This is the central request routing mechanism for the Node.
    pub async fn request(&self, path: impl Into<String>, payload: ValueType) -> Result<ServiceResponse> {
        let path_string = path.into();
        let topic_path = match TopicPath::new(&path_string, &self.network_id) {
            Ok(tp) => tp,
            Err(e) => return Err(anyhow!("Failed to parse topic path: {} : {}", path_string, e)),
        };

        self.logger.debug(format!("Processing request: {}", topic_path));
        
        // First check for local handlers
        if let Some((handler, registration_path)) = self.service_registry.get_local_action_handler(&topic_path).await {
            self.logger.debug(format!("Executing local handler for: {}", topic_path));
            
            // Create request context
            let mut context = RequestContext::new(&topic_path, self.logger.clone());
            
            // Extract parameters using the original registration path
            if let Ok(params) = topic_path.extract_params(&registration_path.action_path()) {
                // Populate the path_params in the context
                context.path_params = params;
                self.logger.debug(format!("Extracted path parameters: {:?}", context.path_params));
            }
            
            // Execute the handler and return result
            return handler(Some(payload), context).await;
        }
        
        // If no local handler found, look for remote handlers
        let remote_handlers = self.service_registry.get_remote_action_handlers(&topic_path).await;
        if !remote_handlers.is_empty() {
            self.logger.debug(format!("Found {} remote handlers for: {}", remote_handlers.len(), topic_path));
            
            // Apply load balancing strategy to select a handler
            let load_balancer = self.load_balancer.read().await;
            let handler_index = load_balancer.select_handler(&remote_handlers, &RequestContext::new(&topic_path, self.logger.clone()));
            
            // Get the selected handler
            let handler = &remote_handlers[handler_index];
            
            self.logger.debug(format!("Selected remote handler {} of {} for: {}", 
                handler_index + 1, remote_handlers.len(), topic_path));
            
            // Create request context
            let context = RequestContext::new(&topic_path, self.logger.clone());
            
            // For remote handlers, we don't have the registration path
            // In the future, we should enhance the remote handler registry to include registration paths
            
            // Execute the selected handler
            return handler(Some(payload), context).await;
        }
        
        // No handler found
        Err(anyhow!("No handler found for action: {}", topic_path))
    }
     

    /// Publish with options - Helper method to implement the publish_with_options functionality
    async fn publish_with_options(&self, topic: impl Into<String>, data: ValueType, options: PublishOptions) -> Result<()> {
        let topic_string = topic.into();
        // Check for valid topic path
        let topic_path = match TopicPath::new(&topic_string, &self.network_id) {
            Ok(tp) => tp,
            Err(e) => return Err(anyhow!("Invalid topic path: {}", e)),
        };
        
        // Create the logger for this operation
        let event_logger = self.logger.with_action_path(topic_path.action_path());
        
        // Publish to local subscribers
        let local_subscribers = self.service_registry.get_local_event_subscribers(&topic_path).await;
        for (subscription_id, callback) in local_subscribers {
            // Create an event context for this subscriber
            let event_context = Arc::new(EventContext::new(&topic_path, event_logger.clone()));
            
            // Execute the callback with correct arguments
            if let Err(e) = callback(event_context, data.clone()).await {
                self.logger.error(format!("Error in local event handler for {}: {}", topic_string, e));
            }
        }
        
        // Broadcast to remote nodes if requested and network is available
        if options.broadcast && self.supports_networking {
            if let Some(transport) = &*self.network_transport.read().await {
                // Log message since we can't implement send yet
                self.logger.debug(format!("Would broadcast event {} to network", topic_string));
            }
        }
        
        Ok(())
    }

    /// Handle remote node capabilities
    ///
    /// INTENTION: Process capabilities from a remote node by creating
    /// RemoteService instances and making them available locally.
    async fn process_remote_capabilities(
        &self,
        node_info: NodeInfo,
    ) -> Result<Vec<Arc<RemoteService>>> {
        let capabilities = node_info.capabilities.clone();
        self.logger.info(format!(
            "Processing {} capabilities from node {}",
            capabilities.len(),
            node_info.peer_id
        ));
        
        // Check if capabilities is empty
        if capabilities.is_empty() {
            self.logger.info("Received empty capabilities list.");
            return Ok(Vec::new()); // Nothing to process
        }

        // Get the local node ID
        let local_peer_id = self.peer_id.clone();
        
        // Create RemoteService instances directly
        let remote_services = match RemoteService::create_from_capabilities(
            node_info.peer_id.clone(),
            capabilities,
            self.network_transport.clone(),
            self.logger.clone(), // Pass logger directly
            local_peer_id,
            self.config.request_timeout_ms,
        ).await {
            Ok(services) => services,
            Err(e) => {
                self.logger.error(format!("Failed to create remote services from capabilities: {}", e));
                return Err(e);
            }
        };

        // Register each service and initialize it to register its handlers
        for service in &remote_services {
            // Register the service instance with the registry
            if let Err(e) = self.service_registry.register_remote_service(service.clone()).await {
                self.logger.error(format!("Failed to register remote service '{}': {}", service.path(), e));
                continue; // Skip initialization if registration fails
            }
            
            // Create RemoteLifecycleContext for the service to register its handlers
            // The context needs a reference back to the registry (as RegistryDelegate)
            // The Node itself implements RegistryDelegate
             let registry_delegate: Arc<dyn RegistryDelegate + Send + Sync> = Arc::new(self.clone());

            // The TopicPath for the context should represent the service itself
            let service_topic_path = TopicPath::new(service.path(), &self.network_id)
                .map_err(|e| anyhow!("Failed to create TopicPath for remote service init: {}", e))?; 
            
            // Pass TopicPath by reference
            let context = RemoteLifecycleContext::new(&service_topic_path, self.logger.clone())
                 .with_registry_delegate(registry_delegate);

            // Initialize the service - this triggers handler registration via the context
             if let Err(e) = service.init(context).await {
                 self.logger.error(format!("Failed to initialize remote service '{}' (handler registration): {}", service.path(), e));
             }
        }
        
        self.logger.info(format!(
            "Successfully processed {} remote services from node {}", 
            remote_services.len(), node_info.peer_id
        ));

        Ok(remote_services)
    }
 
    /// Create a remote action handler
    fn create_remote_action_handler(
        &self,
        peer_id: PeerId,
        topic_path: TopicPath,
        transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,
        logger: Logger,
        local_node_id: PeerId,
        timeout_ms: Option<u64>,
    ) -> ActionHandler {
        Arc::new(move |params, _context| {
            // Clone things we need to move into the async block
            let transport = transport.clone();
            let topic_path = topic_path.clone();
            let peer_id = peer_id.clone();
            let local_node_id = local_node_id.clone();
            let logger = logger.clone();
            let timeout_ms = timeout_ms.unwrap_or(30000);

            // Create a future for the remote call
            Box::pin(async move {
                // Generate a unique correlation ID for this request
                let correlation_id = format!("{}", Uuid::new_v4());
                
                // Create a channel for the response
                let (sender, receiver) = oneshot::channel();
                
                // Create network message
                let message = NetworkMessage {
                    source: local_node_id.clone(),
                    destination: peer_id.clone(), 
                    message_type: "Request".to_string(),
                    payloads: vec![(topic_path.as_str().to_string(), params.unwrap_or(ValueType::Null), correlation_id.clone())],
                };
                
                // Store the pending request
                // Create a pending requests map if needed
                let pending_map = Arc::new(RwLock::new(HashMap::<String, oneshot::Sender<Result<ServiceResponse>>>::new()));
                pending_map.write().await.insert(correlation_id.clone(), sender);
                
                // Send the request
                if let Some(transport_guard) = &*transport.read().await {
                    // Create a message handler to intercept responses
                    let response_handler = {
                        let pending_map = pending_map.clone();
                        let logger = logger.clone();
                        Box::new(move |response: NetworkMessage| {
                            // Clone the required response data before moving it into the tokio::spawn
                            let pending_map = pending_map.clone();
                            
                            // Process each payload in the response
                            for (_topic, payload_data, correlation_id) in response.payloads {
                                // We need to spawn a task to handle this asynchronously
                                // since message callbacks must be sync to satisfy NetworkTransport trait
                                let pending_map_clone = pending_map.clone(); // Clone for the spawned task
                                let response_message_type = response.message_type.clone(); // Clone message type
                                
                                tokio::spawn(async move {
                                    let mut pending = pending_map_clone.write().await;
                                    if let Some(sender) = pending.remove(&correlation_id) {
                                        // Convert message to service response based on the message type
                                        let service_response = if response_message_type == "Response" {
                                            // Check if payload exists and is not Null
                                            match &payload_data {
                                                ValueType::Null => ServiceResponse {
                                                    status: 200,
                                                    data: None,
                                                    error: None,
                                                },
                                                _ => ServiceResponse {
                                                    status: 200,
                                                    data: Some(payload_data),
                                                    error: None,
                                                }
                                            }
                                        } else if response_message_type == "Error" {
                                            ServiceResponse {
                                                status: 400,
                                                data: None,
                                                error: Some(format!("Remote error: {:?}", payload_data)),
                                            }
                                        } else {
                                            ServiceResponse {
                                                status: 400,
                                                data: None, 
                                                error: Some(format!("Unexpected message type received: {}", response_message_type)),
                                            }
                                        };
                                        
                                        // Send the response through the oneshot channel
                                        let _ = sender.send(Ok(service_response));
                                    }
                                });
                            }
        
        Ok(())
                        }) as MessageHandler
                    };
                    
                    // Register our handler
                    if let Err(e) = transport_guard.register_message_handler(response_handler) {
                        logger.error(format!("Failed to register message handler: {}", e));
                        return Err(anyhow!("Failed to register message handler: {}", e));
                    }
                    
                    // Now send the message
                    match transport_guard.send_message(message).await {
                        Ok(_) => {
                            logger.debug(format!("Sent request to {} with correlation ID {}", peer_id, correlation_id));
                        },
                        Err(e) => {
                            logger.error(format!("Failed to send request: {}", e));
                            return Err(anyhow!("Failed to send request: {}", e));
                        }
                    }
                } else {
                    return Err(anyhow!("Network transport not available"));
                }
                
                // Wait for the response with timeout
                match tokio::time::timeout(Duration::from_millis(timeout_ms), receiver).await {
                    Ok(response) => {
                        match response {
                            Ok(result) => result,
                            Err(_) => {
                                logger.warn("Response channel closed");
                                Err(anyhow!("Response channel closed"))
                            }
                        }
                    },
                    Err(_) => {
                        // Timeout occurred
                        logger.warn(format!("Request to {} timed out after {}ms", peer_id, timeout_ms));
                        Err(anyhow!("Request to {} timed out after {}ms", peer_id, timeout_ms))
                    }
                }
            })
        })
    }

    /// Collect capabilities of all local services
    ///
    /// INTENTION: Gather capability information from all local services.
    /// This includes service metadata and all registered actions.
    /// 
    pub async fn collect_local_service_capabilities(&self) -> Result<Vec<ServiceCapability>> {
        // Get all local services
        let service_paths = self.service_registry.get_local_services().await;
        if service_paths.is_empty() {
            return Ok(Vec::new());
        }
        
        // Build capability information for each service
        let mut capabilities = Vec::new();
        
        for (service_path, service_entry) in service_paths {
            let service = &service_entry.service;
            // Skip internals services:
            // -$registry service - 
            if service.path().contains("$registry") {
                continue;
            }
            
            // Get the service actions from registry
            let meta = self.service_registry.get_service_metadata(&service_path).await;
            
            // Convert registered actions to ActionCapability objects
            let mut actions = Vec::new();
            
            if let Some(metadata) = meta {
                // Get actions from service metadata (which should have all registered handlers)
                for action in metadata.registered_actions.values() {
                    actions.push(ActionCapability {
                        name: action.name.clone(),
                        description: action.description.clone(),
                        params_schema: action.parameters_schema.clone(),
                        result_schema: action.return_schema.clone(),
                    });
                }
                self.logger.debug(format!(
                    "Found {} registered actions for service {} in metadata",
                    actions.len(), service_path
                ));
            }
   
            // Create a complete capability object
            let capability = ServiceCapability {
                network_id: match service.network_id() {
                    Some(id) => id.to_string(),
                    None => self.network_id.clone(),
                },
                service_path: service.path().to_string(),
                name: service.name().to_string(),
                version: service.version().to_string(),
                description: service.description().to_string(),
                actions,
                //TODO: add events
                events: vec![],
            };
            
            self.logger.debug(format!(
                "Created capability for service {} with network_id={} and {} actions: {:?}", 
                capability.service_path, 
                capability.network_id,
                capability.actions.len(),
                capability.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>()
            ));
            
            capabilities.push(capability);
        }
        
        // Log all capabilities collected
        self.logger.info(format!("Collected {} service capabilities", capabilities.len()));
        for cap in &capabilities {
            self.logger.debug(format!(
                "Capability: service={}, network_id={}, actions={:?}", 
                cap.service_path, 
                cap.network_id, 
                cap.actions.iter().map(|a| a.name.clone()).collect::<Vec<_>>()
            ));
        }
        
        Ok(capabilities)
    }

    /// Get the node's public network address
    ///
    /// This retrieves the address that other nodes should use to connect to this node.
    async fn get_node_address(&self) -> Result<String> {
        // If networking is disabled, return empty string
        if !self.supports_networking {
            return Ok(String::new());
        }
        
        // First, try to get the address from the network transport if available
        let transport_guard = self.network_transport.read().await;
        if let Some(transport) = transport_guard.as_ref() {
            let address = transport.get_local_address();
            if !address.is_empty() {
                return Ok(address);
            }
        }
        
        // If transport is not available or didn't provide an address,
        // try to get the address from the network config
        if let Some(network_config) = &self.config.network_config {
            return Ok(network_config.transport_options.bind_address.to_string());
        }
        
        // If networking is disabled or no address is available, return empty string
        Ok(String::new())
    }
    
    /// Get information about the local node
    ///
    /// INTENTION: Create a complete NodeInfo structure for this node,
    /// including its network IDs, address, and capabilities.
    pub async fn get_local_node_info(&self) -> Result<NodeInfo> {
        let mut address = self.get_node_address().await?;
        
        // Check if address starts with 0.0.0.0 and replace with a usable IP address
        if address.starts_with("0.0.0.0") {
            // Try to get a real network interface IP address
            if let Ok(ip) = self.get_non_loopback_ip() {
                address = address.replace("0.0.0.0", &ip);
                self.logger.debug(format!("Replaced 0.0.0.0 with network interface IP: {}", ip));
            } else {
                // Fall back to localhost if we can't get a real IP
                address = address.replace("0.0.0.0", "127.0.0.1");
                self.logger.debug("Replaced 0.0.0.0 with localhost (127.0.0.1)");
            }
        }
        
        let node_info = NodeInfo {
            peer_id: self.peer_id.clone(),
            network_ids: self.network_ids.clone(),
            addresses: vec![address],
            capabilities: self.collect_local_service_capabilities().await?,
            last_seen: std::time::SystemTime::now(),
        };
        
        Ok(node_info)
    }

    /// Get a non-loopback IP address from the local network interfaces
    fn get_non_loopback_ip(&self) -> Result<String> {
        use std::net::{IpAddr, SocketAddr};
        use socket2::{Socket, Domain, Type};
        
        // Create a UDP socket
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, None)?;
        
        // "Connect" to a public IP (doesn't actually send anything)
        // This forces the OS to choose the correct network interface
        let addr: SocketAddr = "8.8.8.8:80".parse()?;
        socket.connect(&addr.into())?;
        
        // Get the local address associated with the socket
        let local_addr = socket.local_addr()?;
        let ip = match local_addr.as_socket_ipv4() {
            Some(addr) => addr.ip().to_string(),
            None => return Err(anyhow!("Failed to get IPv4 address")),
        };
        
        self.logger.debug(format!("Discovered local network interface IP: {}", ip));
        Ok(ip)
    }

    /// Shutdown the network components
    async fn shutdown_network(&self) -> Result<()> {
        // Early return if networking is disabled
        if !self.supports_networking {
            self.logger.debug("Network shutdown skipped - networking is disabled");
            return Ok(());
        }
        
        self.logger.info("Shutting down network components");
        
        // For simplicity during the refactoring, just log the intention
        // We would actually shut down the discovery and transport here
        self.logger.info("Stopping discovery and transport services");
        
        // Discovery would need to be shut down properly
        let transport_guard = self.network_transport.read().await;
        if let Some(discovery_service) = transport_guard.as_ref() {
            // The NetworkTransport trait doesn't have a shutdown method
            // so we're commenting this out to avoid errors
            // discovery.shutdown().await?;
            
            // Instead, we'll just log that we would shut it down
            self.logger.info("Would shut down discovery and transport");
        }
        
        Ok(())
    }

    /// Create a message handler for network messages
    fn create_message_handler(&self) -> MessageCallback {
        let node_arc = self.clone();
        
        Arc::new(move |message: NetworkMessage| {
            let node = node_arc.clone();
            
            Box::pin(async move {
                // Quick check if networking is disabled
                if !node.supports_networking {
                    node.logger.warn("Received network message but networking is disabled");
                    return Ok(());
                }
                
                if let Err(e) = node.handle_network_message(message).await {
                    // Errors are handled and logged in handle_network_message
                    // Here we just log that message handling failed
                    node.logger.error(format!("Failed to handle network message: {}", e));
                }
                Ok(())
            })
        })
    }

    async fn subscribe(
        &self,
        topic: impl Into<String>,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        let topic_string = topic.into();
        self.logger.debug(format!("Subscribing to topic: {}", topic_string));
        let options = SubscriptionOptions::default(); 
        // Convert topic String to TopicPath and callback Box to Arc
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic string for subscribe: {}", e))?; 
        self.service_registry.register_local_event_subscription(&topic_path, callback.into(), options).await
    }

    async fn subscribe_with_options(
        &self,
        topic: impl Into<String>,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        let topic_string = topic.into();
        self.logger.debug(format!("Subscribing to topic with options: {}", topic_string));
        // Convert topic String to TopicPath
        let topic_path = TopicPath::new(&topic_string, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic string for subscribe_with_options: {}", e))?; 
        self.service_registry.register_local_event_subscription(&topic_path, callback.into(), options).await
    }

    async fn unsubscribe(&self,  subscription_id: Option<&str>) -> Result<()> {
       // let topic_string = topic.into();
        if let Some(id) = subscription_id {
            self.logger.debug(format!("Unsubscribing from  with ID: {}",   id));
            // Directly forward to service registry's method
            let registry = self.service_registry.clone();
            match registry.unsubscribe_local(id).await {
                Ok(_) => {
                    self.logger.debug(format!(
                        "Successfully unsubscribed locally from   id {}",
                         id
                    ));
                    Ok(())
                }
                Err(e) => {
                    self.logger.error(format!("Failed to unsubscribe locally from  with id {}: {}",   id, e));
                    Err(anyhow!("Failed to unsubscribe locally: {}", e))
                }
            }
        } else {
            Err(anyhow!("Subscription ID is required"))
        }
    }
}

#[async_trait]
impl NodeDelegate for Node {
    async fn request(&self, path: String, params: ValueType) -> Result<ServiceResponse> {
        // Delegate directly to our implementation using the path string and params
        self.request(path, params).await
    }

    async fn publish(&self, topic: String, data: ValueType) -> Result<()> {
        // Create default options
        let options = PublishOptions {
            broadcast: true,
            guaranteed_delivery: false,
            retention_seconds: None,
            target: None,
        };
        
        self.publish_with_options(topic, data, options).await
    }

    async fn subscribe(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
    ) -> Result<String> {
        self.logger.debug(format!("Subscribing to topic: {}", topic));
        let options = SubscriptionOptions::default(); 
        // Convert topic String to TopicPath and callback Box to Arc
        let topic_path = TopicPath::new(&topic, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic string for subscribe: {}", e))?; 
        self.service_registry.register_local_event_subscription(&topic_path, callback.into(), options).await
    }

    async fn subscribe_with_options(
        &self,
        topic: String,
        callback: Box<dyn Fn(Arc<EventContext>, ValueType) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>,
        options: SubscriptionOptions,
    ) -> Result<String> {
        self.logger.debug(format!("Subscribing to topic with options: {}", topic));
        // Convert topic String to TopicPath
        let topic_path = TopicPath::new(&topic, &self.network_id)
            .map_err(|e| anyhow!("Invalid topic string for subscribe_with_options: {}", e))?; 
        self.service_registry.register_local_event_subscription(&topic_path, callback.into(), options).await
    }

    async fn unsubscribe(&self, subscription_id: Option<&str>) -> Result<()> {
        if let Some(id) = subscription_id {
            self.logger.debug(format!("Unsubscribing from with ID: {}", id));
            // Directly forward to service registry's method
            let registry = self.service_registry.clone();
            match registry.unsubscribe_local(id).await {
                Ok(_) => {
                    self.logger.debug(format!(
                        "Successfully unsubscribed locally from  with id {}",
                        id
                    ));
                    Ok(())
                }
                Err(e) => {
                    self.logger.error(format!("Failed to unsubscribe locally from  with id {}: {}", id, e));
                    Err(anyhow!("Failed to unsubscribe locally: {}", e))
                }
            }

        } else {
            Err(anyhow!("Subscription ID is required"))
        }
    }

    /// Register an action handler for a specific path
    ///
    /// INTENTION: Allow services to register handlers for actions through the NodeDelegate.
    /// This consolidates all node interactions through a single interface.
    async fn register_action_handler(&self, topic_path: &TopicPath, handler: ActionHandler, metadata: Option<ActionMetadata>) -> Result<()> {
        self.service_registry.register_local_action_handler(topic_path, handler, metadata).await
    }
}

#[async_trait]
impl RegistryDelegate for Node {
    /// Get all service states
    async fn get_all_service_states(&self) -> HashMap<String, ServiceState> {
        self.service_registry.get_all_service_states().await
    }
    
    /// Get metadata for a specific service
    async fn get_service_metadata(&self, service_path: &TopicPath) -> Option<CompleteServiceMetadata> {
        self.service_registry.get_service_metadata(service_path).await
    }
    
    /// Get metadata for all registered services with an option to filter internal services
    async fn get_all_service_metadata(&self, include_internal_services: bool) -> HashMap<String, CompleteServiceMetadata> {
        self.service_registry.get_all_service_metadata(include_internal_services).await
    }
    
    /// Register a remote action handler
    ///
    /// INTENTION: Delegates to the service registry to register a remote action handler.
    /// This allows RemoteLifecycleContext to register handlers without direct access
    /// to the service registry.
    async fn register_remote_action_handler(
        &self,
        topic_path: &TopicPath,
        handler: ActionHandler,
        remote_service: Arc<RemoteService>
    ) -> Result<()> {
        // Delegate to the service registry
        self.service_registry.register_remote_action_handler(topic_path, handler, remote_service).await
    }
}

/// Implementation of discovery listener that forwards node information to Node
struct NodeDiscoveryListener {
    logger: Logger,
    transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>,
}

impl NodeDiscoveryListener {
    fn new(logger: Logger, transport: Arc<RwLock<Option<Box<dyn NetworkTransport>>>>) -> Self {
        Self { logger, transport }
    }
    
    async fn handle_discovered_node(&self, discovery_msg: PeerInfo) -> Result<()> {
        self.logger.info(format!("Discovery listener found node: {}", discovery_msg.public_key));
        
        // For now, just log discovery events
        // The actual connection logic will be handled by the Node
        self.logger.info(format!("Node discovery event for: {}", discovery_msg.public_key));
        
        // We'll implement proper connection logic elsewhere to avoid the
        // linter errors related to mutability and type mismatches
        Ok(())
    }
}

// Implement Clone for Node
impl Clone for Node {
    fn clone(&self) -> Self {
        Self {
            network_id: self.network_id.clone(),
            network_ids: self.network_ids.clone(),
            peer_id: self.peer_id.clone(),
            config: self.config.clone(),
            service_registry: self.service_registry.clone(),
            logger: self.logger.clone(),
            running: AtomicBool::new(self.running.load(Ordering::SeqCst)),
            supports_networking: self.supports_networking,
            network_transport: self.network_transport.clone(),
            load_balancer: self.load_balancer.clone(),
            default_load_balancer: RoundRobinLoadBalancer::new(),
            event_payloads: self.event_payloads.clone(),
        }
    }
}

/// Generate a self-signed certificate for testing/development
fn generate_self_signed_cert() -> Result<(Certificate, PrivateKey)> {
    // Generate self-signed certificates for development/testing
    // In production, these should be replaced with proper certificates
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()])?;
    let cert_der = cert.serialize_der()?;
    let priv_key = cert.serialize_private_key_der();
    let priv_key = PrivateKey(priv_key);
    let certificate = Certificate(cert_der);
    
    Ok((certificate, priv_key))
} 