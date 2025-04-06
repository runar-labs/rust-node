# Service Registry Redesign Plan

## Overview

The ServiceRegistry is a critical component in the runar-node framework that manages service registration, lookup, and event subscription/publishing. This document outlines the plan for redesigning the ServiceRegistry to improve its architecture, address existing issues, and enhance maintainability.

## Objectives

1. Create a network-agnostic ServiceRegistry that does not maintain a default network ID
2. Move responsibility for network ID management to the Node
3. Organize services by network ID to maintain proper isolation
4. Improve the service lookup API to prioritize path-based lookups
5. Rename `get_service_by_topic_path` to `get_service` as the default way to get a service
6. Update test fixtures to separate name and path parameters to avoid confusion
7. Begin fresh implementation in a new crate using test-driven development

## Current Issues

From our analysis of the existing code, we identified the following issues:

1. The ServiceRegistry has a `default_network_id` field, creating unnecessary state
2. Service lookup methods are inconsistent, with multiple ways to get services
3. There's confusion between service name and path in the test fixtures
4. Tests use a deprecated API (`with_default_network`)

## New Design

### ServiceRegistry

The new ServiceRegistry:
- Will not store a default network ID
- Will organize services by network ID -> service path -> service
- Will provide a clear API for service registration and lookup
- Will simplify event subscription and publishing

### Service Lookup

We'll address the TODOs from the test file:
1. Rename `get_service_by_topic_path` to `get_service` as the default way to get a service
2. Add name and path parameters to test fixtures to avoid confusion

### API Changes

1. `ServiceRegistry::new()` - Create a new registry without a default network ID
2. `register_service(network_id, service)` - Register a service with a specific network ID
3. `get_service(network_id, service_path)` - Get a service by network ID and path (new default)
4. `get_service_by_topic_path(topic_path)` - Alternative lookup method using a TopicPath

## Test Improvements

1. Updated MathService test fixture to take both name and path parameters
2. Comprehensive test suite to validate all aspects of the new design
3. Tests now explicitly specify network IDs for service registration and lookup

## Implementation Plan

1. ✅ Create new crate `rust-node-new` to start fresh
2. ✅ Implement core types and interfaces 
3. ✅ Implement the new ServiceRegistry design
4. ✅ Create test fixtures with proper name/path separation
5. ✅ Write comprehensive tests for all functionality
6. ✅ Document design decisions and API

## Progress

### Completed Items

- Created new crate structure with appropriate modules
- Implemented core service types and interfaces
- Created `TopicPath` for routing and path handling
- Implemented new `ServiceRegistry` with network isolation
- Created improved test fixtures with name/path separation
- Implemented comprehensive test suite
- Fixed TODOs from original test file

### Next Steps

- Run the test suite to validate the implementation
- Add integration tests with actual service communication
- Eventually migrate the rest of the codebase to use the new ServiceRegistry
- Update any components that interact with ServiceRegistry to use the new API

## Future Considerations

- Optimize service lookup for performance
- Add support for service discovery across networks
- Improve error handling and reporting
- Enhance event delivery guarantees
- Add metrics and monitoring capabilities 