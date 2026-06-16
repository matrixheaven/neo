use crate::{AgentContext, AgentMessage};

#[async_trait::async_trait]
pub trait DynamicInjector: Send {
    async fn inject(&mut self, context: &AgentContext) -> Option<AgentMessage>;
}
