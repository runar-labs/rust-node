// Network tests
//
// This module contains tests for the network components of Runar Node.
// Note: Some of these tests are in the process of being updated for API compatibility.
// See the following for more details:
// - ../../../rust-docs/specs/under_construction/test_network.md
// - README.md
// - UPDATE_TESTS.md

// Working tests - these tests have been updated to work with the current API
pub mod capability_exchange_test;
pub mod quic_transport_test;
pub mod multicast_discovery_test;
pub mod discovery_test;

// Tests being updated - these tests are in the process of being fixed for API compatibility
// pub mod network_test;
// pub mod remote_action_test; 