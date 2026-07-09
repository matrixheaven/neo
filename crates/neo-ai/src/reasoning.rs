use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ApiKind, ChatMessage, ContentPart, ModelSpec, ProviderId, ReasoningBudget, ReasoningCapability,
    ReasoningEffort, ReasoningSelection,
};

const DEFAULT_BUDGET_TOKENS: u32 = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReasoningPolicy {
    #[serde(rename = "off", alias = "Off")]
    Off,
    #[serde(rename = "auto", alias = "Auto")]
    Auto,
    #[serde(rename = "minimal", alias = "Minimal")]
    Minimal,
    #[serde(rename = "low", alias = "Low")]
    Low,
    #[serde(rename = "medium", alias = "Medium")]
    Medium,
    #[serde(rename = "high", alias = "High")]
    High,
    #[serde(rename = "xhigh", alias = "XHigh")]
    XHigh,
    #[serde(rename = "max", alias = "Max")]
    Max,
}

impl ReasoningPolicy {
    #[must_use]
    pub fn resolve_for_model(self, model: &ModelSpec) -> ReasoningSelection {
        match self {
            Self::Off => ReasoningSelection::Off,
            Self::Auto => auto_selection(&model.capabilities.reasoning),
            Self::Minimal => effort_selection(ReasoningEffort::Minimal),
            Self::Low => effort_selection(ReasoningEffort::Low),
            Self::Medium => effort_selection(ReasoningEffort::Medium),
            Self::High => effort_selection(ReasoningEffort::High),
            Self::XHigh => effort_selection(ReasoningEffort::XHigh),
            Self::Max => effort_selection(ReasoningEffort::Max),
        }
    }
}

fn auto_selection(capability: &ReasoningCapability) -> ReasoningSelection {
    match capability {
        ReasoningCapability::None => ReasoningSelection::Off,
        ReasoningCapability::Toggle { .. } => ReasoningSelection::On,
        ReasoningCapability::Effort { values, .. } => {
            effort_auto_selection(values).unwrap_or(ReasoningSelection::Off)
        }
        ReasoningCapability::BudgetTokens { min, max, .. } => budget_selection(*min, *max),
        ReasoningCapability::Combined {
            toggle,
            effort,
            budget,
            ..
        } => effort_auto_selection(effort)
            .or_else(|| (*toggle).then_some(ReasoningSelection::On))
            .or_else(|| budget.as_ref().map(budget_auto_selection))
            .unwrap_or(ReasoningSelection::Off),
    }
}

fn effort_auto_selection(values: &[ReasoningEffort]) -> Option<ReasoningSelection> {
    let effort = if values.contains(&ReasoningEffort::Medium) {
        ReasoningEffort::Medium
    } else {
        *values.first()?
    };
    Some(effort_selection(effort))
}

const fn effort_selection(effort: ReasoningEffort) -> ReasoningSelection {
    ReasoningSelection::Effort { effort }
}

fn budget_auto_selection(budget: &ReasoningBudget) -> ReasoningSelection {
    budget_selection(budget.min, budget.max)
}

const fn budget_selection(min: Option<u32>, max: Option<u32>) -> ReasoningSelection {
    ReasoningSelection::BudgetTokens {
        budget_tokens: match (min, max) {
            (Some(min), _) => min,
            (None, Some(max)) => max,
            (None, None) => DEFAULT_BUDGET_TOKENS,
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningContinuation {
    pub provider: ProviderId,
    pub api: ApiKind,
}

impl ReasoningContinuation {
    #[must_use]
    pub fn matches_model(&self, model: &ModelSpec) -> bool {
        self.provider == model.provider && self.api == model.api
    }
}

#[must_use]
pub fn sanitize_reasoning_continuation(
    messages: Vec<ChatMessage>,
    origin: Option<&ReasoningContinuation>,
    target: &ModelSpec,
) -> Vec<ChatMessage> {
    if origin.is_some_and(|origin| origin.matches_model(target)) {
        return messages;
    }
    messages.into_iter().map(strip_opaque_reasoning).collect()
}

fn strip_opaque_reasoning(message: ChatMessage) -> ChatMessage {
    match message {
        ChatMessage::System { content } => ChatMessage::System {
            content: filter_opaque_reasoning(content),
        },
        ChatMessage::User { content } => ChatMessage::User {
            content: filter_opaque_reasoning(content),
        },
        ChatMessage::Assistant {
            content,
            tool_calls,
        } => ChatMessage::Assistant {
            content: filter_opaque_reasoning(content),
            tool_calls,
        },
        ChatMessage::ToolResult {
            tool_call_id,
            content,
            is_error,
        } => ChatMessage::ToolResult {
            tool_call_id,
            content: filter_opaque_reasoning(content),
            is_error,
        },
    }
}

fn filter_opaque_reasoning(content: Vec<ContentPart>) -> Vec<ContentPart> {
    content
        .into_iter()
        .filter(|part| match part {
            ContentPart::Thinking {
                signature,
                redacted,
                ..
            } => signature.is_none() && !redacted,
            ContentPart::Text { .. } | ContentPart::Image { .. } => true,
        })
        .collect()
}
