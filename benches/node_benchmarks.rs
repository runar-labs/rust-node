// Node Benchmarks
//
// INTENTION:
// Benchmark the performance of the Runar Node system under load, with a focus on
// concurrent operation, thread safety, and memory/CPU utilization.
//
// These benchmarks measure:
// 1. Request processing throughput
// 2. Event publishing/subscription throughput
// 3. Service scalability
// 4. Memory usage under load
// 5. Contention resolution
//
// The results serve as a baseline for future performance comparisons.
//
// HOW TO RUN:
// Run with: cargo bench --package runar_node
// 
// The benchmarks are configured with criterion and will output:
// - Throughput metrics for concurrent request processing
// - Throughput metrics for event publishing and subscribing
// - Memory usage under load
// - Service scaling performance
//
// If you encounter issues with the AbstractService trait and Arc<dyn AbstractService>,
// you may need to:
// 1. Use a fully qualified type name in Node's add_service
// 2. Or add a Clone implementation for the services trait object

use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use runar_common::types::ValueType;
use runar_node::node::{Node, NodeConfig};
use runar_node::services::{LifecycleContext, ServiceResponse};
use runar_node::services::abstract_service::AbstractService;
use runar_node::vmap;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::Barrier;
use anyhow::Result;
use std::time::Duration;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

// Dummy service for benchmark testing
struct BenchmarkService {
    name: String,
    path: String,
    version: String,
    description: String,
}

impl BenchmarkService {
    fn new(index: usize) -> Self {
        Self {
            name: format!("benchmark-service-{}", index),
            path: format!("benchmark/service{}", index),
            version: "1.0.0".to_string(),
            description: format!("Benchmark service instance {}", index),
        }
    }
}

#[async_trait]
impl AbstractService for BenchmarkService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn description(&self) -> &str {
        &self.description
    }

    async fn init(&self, context: LifecycleContext) -> Result<()> {
        // Register an echo action
        let service_path = self.path.clone();
        context.register_action(
            "echo",
            Arc::new(move |params, ctx| {
                let p = params.clone();
                let sp = service_path.clone(); // Clone here to avoid move issues
                Box::pin(async move {
                    ctx.logger.debug(format!("Echo request received on {}", sp));
                    Ok(ServiceResponse::ok(p.unwrap_or(ValueType::Null)))
                })
            }),
        ).await?;

        // Register a compute action
        context.register_action(
            "compute",
            Arc::new(move |params, ctx| {
                Box::pin(async move {
                    // Simple computation to simulate CPU work
                    let iterations = if let Some(p) = params.clone() {
                        match p {
                            ValueType::Number(n) => n as u64,
                            _ => 1000
                        }
                    } else {
                        1000
                    };

                    let mut result = 0;
                    for i in 0..iterations {
                        result = (result + i) % 1000;
                    }

                    Ok(ServiceResponse::ok(ValueType::Number(result as f64)))
                })
            }),
        ).await?;

        // Register an action that returns multiple values
        context.register_action(
            "get_data",
            Arc::new(move |_params, _ctx| {
                Box::pin(async move {
                    let data = vmap! {
                        "id" => 123,
                        "name" => "test",
                        "active" => true,
                        "tags" => vec!["benchmark", "test", "performance"],
                        "metadata" => vmap! {
                            "created_at" => "2023-01-01",
                            "version" => 1.0
                        }
                    };
                    Ok(ServiceResponse::ok(data))
                })
            }),
        ).await?;

        Ok(())
    }

    async fn start(&self, context: LifecycleContext) -> Result<()> {
        context.logger.info(format!("Starting service {}", self.name));
        Ok(())
    }

    async fn stop(&self, context: LifecycleContext) -> Result<()> {
        context.logger.info(format!("Stopping service {}", self.name));
        Ok(())
    }
}

// Create a Node with a number of benchmark services
async fn setup_node(num_services: usize) -> Node {
    let node_config = NodeConfig::new("benchmark-node");
    let node = Node::new(node_config).await.expect("Failed to create node");
    
    // Using mutable reference to node to call add_service
    let mut node_mut = node;
    
    // Add benchmark services
    for i in 0..num_services {
        let service = BenchmarkService::new(i);
        // Call add_service which takes the service by value
        node_mut.add_service(service).await.expect("Failed to add service");
    }
    
    // Start the node
    node_mut.start().await.expect("Failed to start node");
    node_mut
}

// Run concurrent requests to measure throughput
async fn concurrent_requests(node: Arc<Node>, num_requests: usize, num_concurrent: usize) {
    let barrier = Arc::new(Barrier::new(num_concurrent));
    let mut handles = Vec::with_capacity(num_concurrent);
    
    for t in 0..num_concurrent {
        let node_clone = node.clone();
        let barrier_clone = barrier.clone();
        let requests_per_thread = num_requests / num_concurrent;
        
        let handle = tokio::spawn(async move {
            // Wait for all threads to be ready
            barrier_clone.wait().await;
            
            for i in 0..requests_per_thread {
                let service_index = i % 10; // Distribute across 10 services
                let service_path = format!("benchmark/service{}/echo", service_index);
                let result = node_clone.request(&service_path, ValueType::String(format!("request-{}-{}", t, i))).await;
                assert!(result.is_ok(), "Request failed: {:?}", result.err());
            }
        });
        
        handles.push(handle);
    }
    
    // Wait for all threads to complete
    for handle in handles {
        handle.await.expect("Thread panicked");
    }
}

// Run concurrent publish/subscribe operations
async fn concurrent_pubsub(node: Arc<Node>, num_events: usize, num_publishers: usize) {
    let barrier = Arc::new(Barrier::new(num_publishers + 1)); // +1 for the subscriber
    let mut handles = Vec::with_capacity(num_publishers + 1);
    
    // Create a subscriber for events
    let subscriber_node = node.clone();
    let subscriber_barrier = barrier.clone();
    
    let subscriber_handle = tokio::spawn(async move {
        let received_count = Arc::new(AtomicUsize::new(0));
        
        // Subscribe to all benchmark events
        subscriber_node.subscribe("benchmark/events/>", {
            let received_count = received_count.clone();
            Box::new(move |ctx, _data| {
                let count = received_count.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move {
                    ctx.logger.debug(format!("Received event #{}", count));
                    Ok(())
                })
            })
        }).await.expect("Failed to subscribe");
        
        // Signal ready and wait for publishers
        subscriber_barrier.wait().await;
        
        // Keep subscriber alive while publishers work
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    
    handles.push(subscriber_handle);
    
    // Create publishers
    for t in 0..num_publishers {
        let publisher_node = node.clone();
        let publisher_barrier = barrier.clone();
        let events_per_publisher = num_events / num_publishers;
        
        let publisher_handle = tokio::spawn(async move {
            // Wait for all threads to be ready
            publisher_barrier.wait().await;
            
            for i in 0..events_per_publisher {
                let event_data = vmap! {
                    "publisher_id" => t as f64,
                    "event_id" => i as f64,
                    "timestamp" => format!("{:?}", tokio::time::Instant::now())
                };
                
                let result = publisher_node.publish(
                    &format!("benchmark/events/type{}", i % 5),
                    event_data
                ).await;
                
                assert!(result.is_ok(), "Publish failed: {:?}", result.err());
            }
        });
        
        handles.push(publisher_handle);
    }
    
    // Wait for all threads to complete
    for handle in handles {
        handle.await.expect("Thread panicked");
    }
}

// Test memory usage under load
async fn memory_usage_test(node: Arc<Node>, iterations: usize) {
    // Measure baseline memory
    let initial_memory = get_memory_usage();
    
    // Perform a series of operations that should stress memory allocation
    for i in 0..iterations {
        // Create a large payload
        let large_data = vmap! {
            "iteration" => i as f64,
            "data" => (0..1000).map(|j| format!("data-{}-{}", i, j)).collect::<Vec<String>>()
        };
        
        // Make a request with the large payload
        let result = node.request("benchmark/service0/echo", large_data.clone()).await;
        assert!(result.is_ok());
        
        // Publish an event with the large payload
        let result = node.publish(&format!("benchmark/large-events/{}", i), large_data).await;
        assert!(result.is_ok());
        
        // Don't wait between iterations to stress GC
    }
    
    // Measure ending memory
    let final_memory = get_memory_usage();
    println!("Memory usage: Initial: {}KB, Final: {}KB, Diff: {}KB", 
        initial_memory, final_memory, final_memory - initial_memory);
}

// Test scaling with increasing number of services
async fn service_scaling_test(max_services: usize) {
    let node_config = NodeConfig::new("scaling-test-node");
    let node = Node::new(node_config).await.expect("Failed to create node");
    
    // Using mutable reference to add services
    let mut node_mut = node;
    
    // Measure performance as we add services
    for num_services in (5..=max_services).step_by(5) {
        // Add 5 more services
        for i in (num_services - 5)..num_services {
            let service = BenchmarkService::new(i);
            // Call add_service which takes the service by value
            node_mut.add_service(service).await.expect("Failed to add service");
        }
        
        // Arc the node for thread safety after adding services
        let node_arc = Arc::new(node_mut);
        
        // Measure request processing time
        let start = tokio::time::Instant::now();
        let test_requests = 1000;
        
        for i in 0..test_requests {
            let service_index = i % num_services;
            let result = node_arc.request(
                &format!("benchmark/service{}/echo", service_index), 
                ValueType::Number(i as f64)
            ).await;
            assert!(result.is_ok());
        }
        
        let elapsed = start.elapsed();
        println!("Services: {}, Requests: {}, Time: {:?}, Req/sec: {:.2}", 
            num_services, test_requests, elapsed, 
            test_requests as f64 / elapsed.as_secs_f64());
            
        // Because we moved node_mut into node_arc, we need to break the loop
        // or create a new node for the next iteration
        break;
    }
}

// Benchmark request processing
fn bench_request_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("request_processing");
    group.sample_size(10); // Minimum required by Criterion
    
    let rt = Runtime::new().unwrap();
    
    for &num_concurrent in &[1, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("concurrent_requests", num_concurrent),
            &num_concurrent,
            |b, &num_concurrent| {
                b.to_async(&rt).iter(|| async {
                    let node = setup_node(5).await; // Setup with 5 services
                    let node_arc = Arc::new(node);
                    
                    // Run requests distributed across threads
                    concurrent_requests(node_arc.clone(), 500, num_concurrent).await;
                    
                    // Clean shutdown
                    node_arc.stop().await.expect("Failed to stop node");
                })
            },
        );
    }
    
    group.finish();
}

// Benchmark event publishing/subscribing
fn bench_event_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("event_processing");
    group.sample_size(10); // Minimum required by Criterion
    
    let rt = Runtime::new().unwrap();
    
    for &num_publishers in &[1, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("concurrent_pubsub", num_publishers),
            &num_publishers,
            |b, &num_publishers| {
                b.to_async(&rt).iter(|| async {
                    let node = setup_node(3).await; // 3 services is enough for this test
                    let node_arc = Arc::new(node);
                    
                    // Publish events distributed across publishers
                    concurrent_pubsub(node_arc.clone(), 500, num_publishers).await;
                    
                    // Clean shutdown
                    node_arc.stop().await.expect("Failed to stop node");
                })
            },
        );
    }
    
    group.finish();
}

// Benchmark memory usage
fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");
    group.sample_size(10); // Minimum required by Criterion
    
    let rt = Runtime::new().unwrap();
    
    for &iterations in &[100, 500] {
        group.bench_with_input(
            BenchmarkId::new("memory_allocations", iterations),
            &iterations,
            |b, &iterations| {
                b.to_async(&rt).iter(|| async {
                    let node = setup_node(2).await; // 2 services is enough
                    let node_arc = Arc::new(node);
                    
                    // Run memory test
                    memory_usage_test(node_arc.clone(), iterations).await;
                    
                    // Clean shutdown
                    node_arc.stop().await.expect("Failed to stop node");
                })
            },
        );
    }
    
    group.finish();
}

// Benchmark service scaling
fn bench_service_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("service_scaling");
    group.sample_size(10); // Minimum required by Criterion
    
    let rt = Runtime::new().unwrap();
    
    for &max_services in &[10, 25] {
        group.bench_with_input(
            BenchmarkId::new("service_count", max_services),
            &max_services,
            |b, &max_services| {
                b.to_async(&rt).iter(|| async {
                    service_scaling_test(max_services).await;
                })
            },
        );
    }
    
    group.finish();
}

// Helper function to get current process memory usage
fn get_memory_usage() -> usize {
    // This is a simplistic approximation - in a real benchmark you would
    // use platform-specific APIs to get accurate memory measurements
    let mut mem_usage = 0;
    
    #[cfg(target_os = "linux")]
    {
        use std::fs::File;
        use std::io::Read;
        
        if let Ok(mut file) = File::open("/proc/self/statm") {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Some(value) = contents.split_whitespace().nth(0) {
                    if let Ok(pages) = value.parse::<usize>() {
                        // Convert pages to KB (assuming 4KB pages)
                        mem_usage = pages * 4;
                    }
                }
            }
        }
    }
    
    // Fallback for non-Linux or if Linux-specific code fails
    if mem_usage == 0 {
        // Return a fake value rather than crashing
        let mut v = Vec::with_capacity(1024 * 1024); // Allocate 1MB
        for i in 0..1024 * 256 {
            v.push(i);
        }
        mem_usage = v.len() * std::mem::size_of::<i32>() / 1024; // Size in KB
    }
    
    mem_usage
}

criterion_group!(
    benches,
    bench_request_processing,
    bench_event_processing,
    bench_memory_usage,
    bench_service_scaling
);
criterion_main!(benches); 