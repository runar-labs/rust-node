# Test Fixtures

This directory contains fixtures for testing the Runar Labs node implementation.

## Directory Structure

- `direct_api/`: Contains modern service implementations using the direct API approach
- `macro_based/`: DEPRECATED - Contains legacy macro-based service implementations

## Migration Path

We are transitioning from macro-based service implementations to direct API implementations for better maintainability, clearer code, and reduced complexity.

### Direct API Services

The `direct_api/` directory contains service implementations that follow these architectural principles:

1. **Service Boundaries**: Services have well-defined boundaries with clear responsibilities
2. **Clean Separation of Concerns**: Each service has a single responsibility
3. **Request-Based Communication**: All service interactions use the request/response pattern
4. **Event-Driven Design**: Event publishing and subscription use clean pub/sub patterns

### Event Services

Following our architecture guidelines, event handling follows a clean separation of concerns with:

1. **EventPublisherService**: Responsible solely for publishing events
2. **EventSubscriberService**: Responsible solely for subscribing to and storing events

Using this pattern improves testability and follows the architectural guidelines by separating the concerns of publishing and listening to events.

## Deprecation Notice

The `macro_based/` fixtures will be removed in a future update. All tests should be migrated to use the direct API implementations.
