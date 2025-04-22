// Test module for runar_node
//
// This module organizes tests for the runar_node crate

// Core tests (services, routing, node)
pub mod core;

// Network tests (transport, discovery) 
// Note: Some network tests are in the process of being updated for API compatibility.
// The following tests are currently working:
// - capability_exchange_test.rs (tests serialization of service capabilities)
// - network_standalone_test.rs (separate test file not included here)
// 
// See rust-docs/specs/under_construction/test_network.md for details on the current status and update plan.
// See network/README.md and network/UPDATE_TESTS.md for more information.
pub mod network;

// Test fixtures
pub mod fixtures;

// Integration tests
pub mod integration;

// Re-enable specific unit tests later if needed 