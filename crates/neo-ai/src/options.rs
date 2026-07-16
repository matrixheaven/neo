use std::{borrow::Cow, collections::BTreeMap, fmt, str::FromStr, time::Duration};

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum CacheRetention {
    None,
    #[default]
    Short,
    Long,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ReasoningEffort(String);

impl ReasoningEffort {
    pub const MINIMAL: &'static str = "minimal";
    pub const LOW: &'static str = "low";
    pub const MEDIUM: &'static str = "medium";
    pub const HIGH: &'static str = "high";
    pub const XHIGH: &'static str = "xhigh";
    pub const MAX: &'static str = "max";

    #[must_use]
    pub fn minimal() -> Self {
        Self(Self::MINIMAL.to_owned())
    }

    #[must_use]
    pub fn low() -> Self {
        Self(Self::LOW.to_owned())
    }

    #[must_use]
    pub fn medium() -> Self {
        Self(Self::MEDIUM.to_owned())
    }

    #[must_use]
    pub fn high() -> Self {
        Self(Self::HIGH.to_owned())
    }

    #[must_use]
    pub fn xhigh() -> Self {
        Self(Self::XHIGH.to_owned())
    }

    #[must_use]
    pub fn max() -> Self {
        Self(Self::MAX.to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidReasoningEffort;

impl fmt::Display for InvalidReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("reasoning effort cannot be empty or whitespace-only")
    }
}

impl std::error::Error for InvalidReasoningEffort {}

impl TryFrom<String> for ReasoningEffort {
    type Error = InvalidReasoningEffort;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.trim().is_empty() {
            return Err(InvalidReasoningEffort);
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for ReasoningEffort {
    type Error = InvalidReasoningEffort;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_owned())
    }
}

impl FromStr for ReasoningEffort {
    type Err = InvalidReasoningEffort;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for ReasoningEffort {
    fn schema_name() -> Cow<'static, str> {
        "ReasoningEffort".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": r"\S"
        })
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
    pub const fn effort(&self) -> Option<&ReasoningEffort> {
        match self {
            Self::Effort { effort } => Some(effort),
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
    pub cache: CacheRetention,
    pub session_id: Option<String>,
    pub metadata: RequestMetadata,
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
            cache: CacheRetention::Short,
            session_id: None,
            metadata: RequestMetadata::default(),
        }
    }
}
