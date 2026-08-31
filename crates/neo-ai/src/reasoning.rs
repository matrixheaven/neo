use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ReasoningBudget, ReasoningCapability, ReasoningEffort, ReasoningSelection};

const DEFAULT_BUDGET_TOKENS: u32 = 8_192;

/// Canonical effort-name ladder used to interpret reasoning intensity names
/// against provider-specific effort values. Position 0 is the cheapest level,
/// position 5 the most expensive.
const CANONICAL_EFFORT_LADDER: [&str; 6] = [
    ReasoningEffort::MINIMAL,
    ReasoningEffort::LOW,
    ReasoningEffort::MEDIUM,
    ReasoningEffort::HIGH,
    ReasoningEffort::XHIGH,
    ReasoningEffort::MAX,
];

/// Reasoning intensity policy for auxiliary model calls (session titles,
/// compaction summaries). Auxiliary calls never participate in the
/// interactive chat reasoning selection.
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
pub enum AuxReasoning {
    /// Cheapest controllable level: explicit reasoning disable when the
    /// capability allows it, otherwise the first (cheapest) declared effort,
    /// otherwise the toggle/budget fallback.
    #[default]
    Auto,
    /// Always request disabled reasoning (historical behavior). Providers that
    /// reject explicit disables surface the error to the caller, where the
    /// auxiliary executor may retry once with the cheapest declared effort.
    Off,
    /// Request one specific effort value. Provider-declared values match
    /// exactly (case-insensitive fallback); canonical ladder names map to the
    /// nearest declared value; anything else falls back to [`AuxReasoning::Auto`].
    Effort(String),
}

impl AuxReasoning {
    fn parse(value: &str) -> Option<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed.eq_ignore_ascii_case("auto") {
            Some(Self::Auto)
        } else if trimmed.eq_ignore_ascii_case("off") {
            Some(Self::Off)
        } else {
            Some(Self::Effort(trimmed.to_owned()))
        }
    }
}

impl Serialize for AuxReasoning {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Off => serializer.serialize_str("off"),
            Self::Effort(value) => serializer.serialize_str(value),
        }
    }
}

impl<'de> Deserialize<'de> for AuxReasoning {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| {
            serde::de::Error::custom(
                "aux reasoning must be \"auto\", \"off\", or a non-empty effort value",
            )
        })
    }
}

/// Result of resolving a requested effort name against declared values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortResolution {
    /// The requested name is a declared value.
    Exact(ReasoningEffort),
    /// The requested name is a canonical ladder name mapped to the nearest
    /// declared value.
    Nearest(ReasoningEffort),
    /// The requested name cannot be ranked against the declared values.
    Unresolved,
}

/// Wire-ready reasoning plan for one auxiliary request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxReasoningPlan {
    pub selection: ReasoningSelection,
    /// Whether the request should ask the provider to disable reasoning on
    /// the wire. Only meaningful together with `selection`.
    pub disable_reasoning: bool,
    /// True when the requested intensity was not an exact declared value
    /// (nearest mapping or fallback to auto).
    pub coerced: bool,
}

fn plan(selection: ReasoningSelection, disable_reasoning: bool, coerced: bool) -> AuxReasoningPlan {
    AuxReasoningPlan {
        selection,
        disable_reasoning,
        coerced,
    }
}

/// Resolve one requested effort name against provider-declared values.
///
/// Declared values are ordered cheapest-first: the catalog and inline TOML
/// list order is the intensity contract. Exact matches win; canonical ladder
/// names (`minimal`..`max`) map by ladder position to the nearest declared
/// value, so arbitrary names such as `5k/10k/20k` or `normal/high/ultra`
/// resolve without knowing the provider's naming scheme.
#[must_use]
pub fn resolve_requested_effort(values: &[ReasoningEffort], requested: &str) -> EffortResolution {
    let requested = requested.trim();
    if requested.is_empty() || values.is_empty() {
        return EffortResolution::Unresolved;
    }
    if let Some(effort) = values.iter().find(|effort| effort.as_str() == requested) {
        return EffortResolution::Exact(effort.clone());
    }
    if let Some(effort) = values
        .iter()
        .find(|effort| effort.as_str().eq_ignore_ascii_case(requested))
    {
        return EffortResolution::Exact(effort.clone());
    }
    let Some(ladder_index) = CANONICAL_EFFORT_LADDER
        .iter()
        .position(|name| name.eq_ignore_ascii_case(requested))
    else {
        return EffortResolution::Unresolved;
    };
    let last = values.len() - 1;
    // Round-half-up of ladder_index / 5 * last without floating point. The
    // fraction never ties for a five-step ladder with integer positions.
    let index = (ladder_index * last * 2 + 5) / 10;
    EffortResolution::Nearest(values[index.min(last)].clone())
}

/// Build the wire-ready reasoning plan for one auxiliary request.
#[must_use]
pub fn aux_reasoning_selection(
    capability: &ReasoningCapability,
    policy: &AuxReasoning,
) -> AuxReasoningPlan {
    match policy {
        AuxReasoning::Off => plan(ReasoningSelection::Off, true, false),
        AuxReasoning::Auto => auto_plan(capability, false),
        AuxReasoning::Effort(requested) => {
            let values = capability_effort_values(capability);
            match resolve_requested_effort(values, requested) {
                EffortResolution::Exact(effort) => {
                    plan(ReasoningSelection::Effort { effort }, false, false)
                }
                EffortResolution::Nearest(effort) => {
                    plan(ReasoningSelection::Effort { effort }, false, true)
                }
                EffortResolution::Unresolved => auto_plan(capability, true),
            }
        }
    }
}

fn capability_effort_values(capability: &ReasoningCapability) -> &[ReasoningEffort] {
    match capability {
        ReasoningCapability::Effort { values, .. }
        | ReasoningCapability::Combined { effort: values, .. } => values,
        ReasoningCapability::None
        | ReasoningCapability::Toggle { .. }
        | ReasoningCapability::BudgetTokens { .. } => &[],
    }
}

fn auto_plan(capability: &ReasoningCapability, coerced: bool) -> AuxReasoningPlan {
    if capability.disable_supported() {
        return plan(ReasoningSelection::Off, true, coerced);
    }
    if let Some(effort) = capability_effort_values(capability).first() {
        return plan(
            ReasoningSelection::Effort {
                effort: effort.clone(),
            },
            false,
            coerced,
        );
    }
    match capability {
        ReasoningCapability::Toggle { .. } => plan(ReasoningSelection::On, false, coerced),
        ReasoningCapability::BudgetTokens { min, max, .. } => {
            plan(budget_selection(*min, *max), false, coerced)
        }
        ReasoningCapability::Combined { toggle, budget, .. } => {
            if *toggle {
                plan(ReasoningSelection::On, false, coerced)
            } else if let Some(budget) = budget {
                plan(budget_selection(budget.min, budget.max), false, coerced)
            } else {
                plan(ReasoningSelection::Off, true, coerced)
            }
        }
        ReasoningCapability::None | ReasoningCapability::Effort { .. } => {
            plan(ReasoningSelection::Off, true, coerced)
        }
    }
}

/// Choose Neo's default reasoning selection for one model capability.
#[must_use]
pub fn automatic_reasoning_selection(capability: &ReasoningCapability) -> ReasoningSelection {
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
    Some(ReasoningSelection::Effort { effort })
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

#[cfg(test)]
#[path = "test_cases/reasoning.rs"]
mod tests;
