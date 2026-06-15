use std::collections::BTreeMap;

use neo_agent_core::{AgentEvent, ToolResult};

use crate::ToolStatusKind;
use crate::transcript::{ToolCallComponent, ToolCallState};

#[derive(Debug, Default)]
pub struct StreamingController {
    tools: BTreeMap<String, ToolCallComponent>,
    streaming_args: BTreeMap<String, String>,
}

impl StreamingController {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::ToolCallStarted { id, name, .. } => {
                self.ensure_tool(id, name, None);
            }
            AgentEvent::ToolCallArgumentsDelta {
                id, json_fragment, ..
            } => {
                let args = self.streaming_args.entry(id.clone()).or_default();
                args.push_str(&json_fragment);
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.update_call(Some(args.clone()));
                }
            }
            AgentEvent::ToolCallFinished { tool_call, .. } => {
                let args = tool_call.arguments.to_string();
                self.streaming_args
                    .insert(tool_call.id.clone(), args.clone());
                self.ensure_tool(tool_call.id.clone(), tool_call.name, Some(args));
            }
            AgentEvent::ToolExecutionStarted {
                id,
                name,
                arguments,
                ..
            } => {
                let args = self
                    .streaming_args
                    .get(&id)
                    .cloned()
                    .unwrap_or_else(|| arguments.to_string());
                self.ensure_tool(id, name, Some(args));
            }
            AgentEvent::ToolExecutionUpdate {
                id, partial_result, ..
            } => {
                if let Some(tool) = self.tools.get_mut(&id) {
                    tool.append_progress(partial_result.content);
                }
            }
            AgentEvent::ToolExecutionFinished {
                id, name, result, ..
            } => {
                self.finish_tool(id, name, result);
            }
            _ => {}
        }
    }

    fn ensure_tool(&mut self, id: String, name: String, arguments: Option<String>) {
        let tool = self.tools.entry(id.clone()).or_insert_with(|| {
            ToolCallComponent::new(ToolCallState {
                id,
                name,
                arguments: None,
                result: None,
                details: None,
                status: ToolStatusKind::Running,
                exit_code: None,
            })
        });
        if arguments.is_some() {
            tool.update_call(arguments);
        }
    }

    fn finish_tool(&mut self, id: String, name: String, result: ToolResult) {
        let tool = self.tools.entry(id.clone()).or_insert_with(|| {
            ToolCallComponent::new(ToolCallState {
                id,
                name,
                arguments: None,
                result: None,
                details: None,
                status: ToolStatusKind::Running,
                exit_code: None,
            })
        });
        tool.set_result(Some(result.content), result.details, result.is_error, None);
    }

    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    #[must_use]
    pub fn tool(&self, id: &str) -> Option<&ToolCallComponent> {
        self.tools.get(id)
    }

    pub fn tool_mut(&mut self, id: &str) -> Option<&mut ToolCallComponent> {
        self.tools.get_mut(id)
    }
}
