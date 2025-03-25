# Test Utilities

This directory contains consolidated utilities for testing the runar-node project. The goal is to provide a unified set of tools that make it easier to write tests.

## Directory Structure

- `mod.rs` - Main module file that re-exports all utilities
- `event_utils.rs` - Utilities for event testing (topics, subscriptions, publishing)
- `node_utils.rs` - Core utilities for working with Node instances
- `test_helpers.rs` - General test helpers for service requests, responses, etc.
- `test_logging.rs` - Logging utilities for tests
- `modernized_test_utils.rs` - Modern test utilities using the newer macro-based services
- `legacy_test_utils.rs` - Legacy test utilities migrated from the old test_utils/mod.rs

## Migration Status

The utilities in this directory are part of a consolidation effort to merge:
- `/tests/test_utils/` (legacy monolithic utilities)
- `/tests/utils/` (newer modular utilities)

The `legacy_test_utils.rs` file contains code migrated from the original `test_utils/mod.rs` to maintain backward compatibility during the transition. New tests should use the modular utilities in this directory instead of the legacy ones.

## Best Practices

When writing tests:

1. Use the modular utilities where possible instead of the legacy ones
2. Keep service testing separate from utilities
3. Create new utility modules for specific testing needs rather than extending existing ones
4. Ensure all utilities have proper documentation

## Example Usage

```rust
use crate::utils::node_utils::create_test_node;
use crate::utils::event_utils::create_event_data;

#[tokio::test]
async fn test_event_publishing() {
    // Create a test node
    let (node, _temp_dir) = create_test_node("test-network").await.unwrap();
    
    // Create event data
    let event_data = create_event_data("test", "test_service", None);
    
    // Use the node for testing...
}
``` 