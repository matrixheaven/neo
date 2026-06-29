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
    pub reasoning_effort: Option<ReasoningEffort>,
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
            reasoning_effort: None,
            replay_reasoning: true,
            retries: Some(2),
            cache: CacheRetention::Short,
            session_id: None,
            metadata: RequestMetadata::default(),
            cancel_token: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_retries_is_two() {
        let opts = RequestOptions::default();
        assert_eq!(opts.retries, Some(2));
    }
}
