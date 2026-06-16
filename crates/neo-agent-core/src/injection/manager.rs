use std::sync::{Arc, RwLock};

use crate::mode::PlanMode;
use crate::{AgentContext, AgentMessage};

use super::{DynamicInjector, PlanModeInjector};

#[derive(Default)]
pub struct InjectionManager {
    injectors: Vec<Box<dyn DynamicInjector>>,
}

impl InjectionManager {
    #[must_use]
    pub fn new(plan_mode: Arc<RwLock<PlanMode>>) -> Self {
        Self {
            injectors: vec![Box::new(PlanModeInjector::new(plan_mode))],
        }
    }

    pub async fn inject(&mut self, context: &AgentContext) -> Vec<AgentMessage> {
        let mut messages = Vec::new();
        for inj in &mut self.injectors {
            if let Some(msg) = inj.inject(context).await {
                messages.push(msg);
            }
        }
        messages
    }
}
