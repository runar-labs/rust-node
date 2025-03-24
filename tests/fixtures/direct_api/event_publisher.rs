use anyhow::Result;
use async_trait::async_trait;
use runar_node::services::abstract_service::{AbstractService, ServiceState};
use runar_node::services::types::ValueType;
use runar_node::{RequestContext, ServiceRequest, ServiceResponse};
use runar_node::routing::TopicPath;
use std::sync::Arc;

/// A direct API service for publishing events
/// Follows the clean separation of concerns architecture
#[derive(Clone)]
pub struct EventPublisherService {
    name: String,
    context: Option<Arc<RequestContext>>,
}

impl EventPublisherService {
    /// Create a new EventPublisherService
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            context: None,
        }
    }

    /// Publish an event with a full path
    pub async fn publish_with_full_path(&self, payload: String, context: &RequestContext) -> Result<ServiceResponse> {
        // Use the complete topic path format: service/topic
        let topic = format!("{}/events/test", self.name);
        context.publish(&topic, ValueType::String(payload)).await?;
        
        Ok(ServiceResponse::success("Published event with full path", Some(ValueType::String(format!("Published to {}", topic)))))
    }

    /// Publish an event with topic only
    pub async fn publish_with_topic_only(&self, payload: String, context: &RequestContext) -> Result<ServiceResponse> {
        // Use just the topic name format
        let topic = "test_event";
        context.publish(topic, ValueType::String(payload)).await?;
        
        Ok(ServiceResponse::success("Published event with topic only", Some(ValueType::String(format!("Published to {}", topic)))))
    }

    /// Publish a global event
    pub async fn publish_to_global(&self, payload: String, context: &RequestContext) -> Result<ServiceResponse> {
        // Use global event format
        let topic = "/global_event";
        context.publish(topic, ValueType::String(payload)).await?;
        
        Ok(ServiceResponse::success("Published global event", Some(ValueType::String(format!("Published to {}", topic)))))
    }
}

#[async_trait]
impl AbstractService for EventPublisherService {
    fn name(&self) -> &str {
        &self.name
    }

    fn path(&self) -> &str {
        "publisher"
    }

    fn state(&self) -> ServiceState {
        ServiceState::Started
    }

    fn description(&self) -> &str {
        "Direct API event publisher service for testing"
    }

    fn version(&self) -> &str {
        "1.0"
    }

    fn operations(&self) -> Vec<String> {
        vec![
            "publish/full_path".to_string(),
            "publish/topic_only".to_string(),
            "publish/global".to_string(),
        ]
    }

    async fn init(&mut self, context: &RequestContext) -> Result<()> {
        self.context = Some(Arc::new(context.clone()));
        Ok(())
    }

    async fn start(&mut self) -> Result<()> {
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    async fn handle_request(&self, request: ServiceRequest) -> Result<ServiceResponse> {
        // Extract the context from the request
        let context = &request.context;
        
        // Process based on the operation path
        match request.path.as_str() {
            "publish/full_path" => {
                if let Some(data) = &request.data {
                    let payload = match data {
                        ValueType::String(s) => s.clone(),
                        ValueType::Map(map) => {
                            // Either extract "payload" key or use the whole data serialized
                            if let Some(ValueType::String(p)) = map.get("payload") {
                                p.clone()
                            } else {
                                format!("{:?}", map)
                            }
                        },
                        _ => format!("{:?}", data)
                    };
                    self.publish_with_full_path(payload, &context).await
                } else {
                    Err(anyhow::anyhow!("Missing payload data"))
                }
            },
            "publish/topic_only" => {
                if let Some(data) = &request.data {
                    let payload = match data {
                        ValueType::String(s) => s.clone(),
                        ValueType::Map(map) => {
                            // Either extract "payload" key or use the whole data serialized
                            if let Some(ValueType::String(p)) = map.get("payload") {
                                p.clone()
                            } else {
                                format!("{:?}", map)
                            }
                        },
                        _ => format!("{:?}", data)
                    };
                    self.publish_with_topic_only(payload, &context).await
                } else {
                    Err(anyhow::anyhow!("Missing payload data"))
                }
            },
            "publish/global" => {
                if let Some(data) = &request.data {
                    let payload = match data {
                        ValueType::String(s) => s.clone(),
                        ValueType::Map(map) => {
                            // Either extract "payload" key or use the whole data serialized
                            if let Some(ValueType::String(p)) = map.get("payload") {
                                p.clone()
                            } else {
                                format!("{:?}", map)
                            }
                        },
                        _ => format!("{:?}", data)
                    };
                    self.publish_to_global(payload, &context).await
                } else {
                    Err(anyhow::anyhow!("Missing payload data"))
                }
            },
            _ => Err(anyhow::anyhow!("Unknown operation: {}", request.path)),
        }
    }
}
