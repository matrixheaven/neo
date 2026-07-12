use std::{collections::BTreeMap, time::Duration};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ReasoningEffort {
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

impl ReasoningEffort {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReasoningSelection {
    #[default]
    Off,
    On,
    Effort {
        effort: ReasoningEffort,
    },
    BudgetTokens {
        budget_tokens: u32,
    },
}

impl ReasoningSelection {
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        !matches!(self, Self::Off)
    }

    #[must_use]
    pub const fn effort(&self) -> Option<ReasoningEffort> {
        match self {
            Self::Effort { effort } => Some(*effort),
            Self::Off | Self::On | Self::BudgetTokens { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReasoningBudget {
    pub min: Option<u32>,
    pub max: Option<u32>,
}

impl ReasoningBudget {
    #[must_use]
    pub const fn contains(&self, budget_tokens: u32) -> bool {
        if let Some(min) = self.min
            && budget_tokens < min
        {
            return false;
        }
        if let Some(max) = self.max
            && budget_tokens > max
        {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningCapability {
    #[default]
    None,
    Toggle {
        disable_supported: bool,
    },
    Effort {
        values: Vec<ReasoningEffort>,
        disable_supported: bool,
    },
    BudgetTokens {
        min: Option<u32>,
        max: Option<u32>,
        disable_supported: bool,
    },
    Combined {
        toggle: bool,
        effort: Vec<ReasoningEffort>,
        budget: Option<ReasoningBudget>,
        disable_supported: bool,
    },
}

impl ReasoningCapability {
    #[must_use]
    pub fn supports_reasoning(&self) -> bool {
        match self {
            Self::None => false,
            Self::Toggle { .. } | Self::BudgetTokens { .. } => true,
            Self::Effort { values, .. } => !values.is_empty(),
            Self::Combined {
                toggle,
                effort,
                budget,
                ..
            } => *toggle || !effort.is_empty() || budget.is_some(),
        }
    }

    #[must_use]
    pub fn supports(&self, selection: &ReasoningSelection) -> bool {
        match selection {
            ReasoningSelection::Off => matches!(self, Self::None) || self.disable_supported(),
            ReasoningSelection::On => match self {
                Self::Toggle { .. } => true,
                Self::Combined { toggle, .. } => *toggle,
                Self::None | Self::Effort { .. } | Self::BudgetTokens { .. } => false,
            },
            ReasoningSelection::Effort { effort } => match self {
                Self::Effort { values, .. } | Self::Combined { effort: values, .. } => {
                    values.contains(effort)
                }
                Self::None | Self::Toggle { .. } | Self::BudgetTokens { .. } => false,
            },
            ReasoningSelection::BudgetTokens { budget_tokens } => match self {
                Self::BudgetTokens { min, max, .. } => ReasoningBudget {
                    min: *min,
                    max: *max,
                }
                .contains(*budget_tokens),
                Self::Combined { budget, .. } => budget
                    .as_ref()
                    .is_some_and(|budget| budget.contains(*budget_tokens)),
                Self::None | Self::Toggle { .. } | Self::Effort { .. } => false,
            },
        }
    }

    #[must_use]
    pub const fn disable_supported(&self) -> bool {
        match self {
            Self::None => true,
            Self::Toggle { disable_supported }
            | Self::Effort {
                disable_supported, ..
            }
            | Self::BudgetTokens {
                disable_supported, ..
            }
            | Self::Combined {
                disable_supported, ..
            } => *disable_supported,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RequestMetadata {
    values: BTreeMap<String, String>,
}

impl RequestMetadata {
    #[must_use]
    pub fn from_pairs<const N: usize>(pairs: [(&str, &str); N]) -> Self {
        Self {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.values
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub headers: BTreeMap<String, String>,
    #[schemars(skip)]
    pub timeout: Option<Duration>,
    pub reasoning: ReasoningSelection,
    pub replay_reasoning: bool,
    pub retries: Option<u32>,
    pub cache: CacheRetention,
    pub session_id: Option<String>,
    pub metadata: RequestMetadata,
    /// Cancellation token for the HTTP retry loop's backoff sleep.
    /// Set by the runtime so retries abort promptly on user cancellation.
    #[serde(skip)]
    #[schemars(skip)]
    pub cancel_token: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            headers: BTreeMap::new(),
            timeout: None,
            reasoning: ReasoningSelection::Off,
            replay_reasoning: true,
            retries: Some(2),
            cache: CacheRetention::Short,
            session_id: None,
            metadata: RequestMetadata::default(),
            cancel_token: None,
        }
    }
}
