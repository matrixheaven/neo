use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{ModelSpec, ReasoningBudget, ReasoningCapability, ReasoningEffort, ReasoningSelection};

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
            Self::Minimal => effort_selection(ReasoningEffort::minimal()),
            Self::Low => effort_selection(ReasoningEffort::low()),
            Self::Medium => effort_selection(ReasoningEffort::medium()),
            Self::High => effort_selection(ReasoningEffort::high()),
            Self::XHigh => effort_selection(ReasoningEffort::xhigh()),
            Self::Max => effort_selection(ReasoningEffort::max()),
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
    let effort = if values
        .iter()
        .any(|effort| effort.as_str() == ReasoningEffort::MEDIUM)
    {
        ReasoningEffort::medium()
    } else {
        values.first()?.clone()
    };
    Some(effort_selection(effort))
}

fn effort_selection(effort: ReasoningEffort) -> ReasoningSelection {
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
