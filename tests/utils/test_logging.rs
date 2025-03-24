// Utilities for configuring logging in tests
use runar_node::util::logging::{Component, configure_test_logging};

/// Configure minimal logging for tests
pub fn setup_test_logging() {
    // Configure logging for tests with minimal output
    let _ = configure_test_logging();
}

/// Configure verbose logging for tests
pub fn setup_verbose_test_logging() {
    // Configure verbose logging for tests with debug level
    let _ = configure_test_logging();
    // Set env vars for more verbose logging if needed
    std::env::set_var("RUST_LOG", "debug");
} 