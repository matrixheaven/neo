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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub headers: BTreeMap<String, String>,
    #[schemars(skip)]
    pub timeout: Option<Duration>,
    pub retries: Option<u32>,
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
            retries: Some(0),
            cache: CacheRetention::Short,
            session_id: None,
            metadata: RequestMetadata::default(),
        }
    }
}
