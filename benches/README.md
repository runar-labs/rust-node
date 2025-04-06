# Runar Node Benchmarks

This directory contains benchmark tests for the Runar Node system. These benchmarks measure various performance aspects of the node, including request processing, event publishing/subscription, memory usage, and service scaling.

## Running the Benchmarks

To run all benchmarks:

```bash
cargo bench --package runar_node
```

To run a specific benchmark group:

```bash
cargo bench --package runar_node --bench node_benchmarks -- request_processing
```

Available benchmark groups:
- `request_processing`: Measures request processing throughput under different levels of concurrency
- `event_processing`: Measures event publishing and subscription throughput
- `memory_usage`: Measures memory allocation and garbage collection performance
- `service_scaling`: Measures how the system scales with an increasing number of services

## Benchmark Configuration

The benchmarks are configured to run with a minimum sample size of 10 iterations as required by Criterion. You can adjust the sample size and other parameters in the benchmark implementation if needed.

Each benchmark creates a dedicated Runar Node instance with a specific number of services:
- Request processing benchmarks: 5 services
- Event processing benchmarks: 3 services
- Memory usage benchmarks: 2 services
- Service scaling benchmarks: up to 25 services

## Adding New Benchmarks

When adding new benchmarks:
1. Create a new function within `node_benchmarks.rs`
2. Use the `criterion_group!` macro to include it in the benchmark suite
3. Ensure proper cleanup of resources after each benchmark run
4. Include appropriate documentation comments

## Interpreting Results

Criterion will generate HTML reports in the `target/criterion` directory. These reports include:
- Throughput metrics
- Statistical analysis
- Performance comparisons with previous runs
- Visualization of benchmark results

For memory usage benchmarks, additional diagnostics are printed to the console output. 