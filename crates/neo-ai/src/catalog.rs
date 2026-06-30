//! models.dev catalog integration.
//!
//! Provides types and functions for fetching the public catalog from
//! `https://models.dev/api.json`, inferring provider wire types, and
//! converting catalog entries into neo's config format.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::ApiType;

/// Public catalog endpoint.
pub const CATALOG_URL: &str = "https://models.dev/api.json";

/// A provider entry in the models.dev catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Base API URL.
    #[serde(default)]
    pub api: Option<String>,
    /// Environment variable names for credentials.
    #[serde(default)]
    pub env: Vec<String>,
    /// NPM package name (used for wire-type inference).
    #[serde(default)]
    pub npm: Option<String>,
    /// Explicit wire type override.
    #[serde(default, rename = "type")]
    pub explicit_type: Option<String>,
    #[serde(default)]
    pub models: BTreeMap<String, CatalogModel>,
}

/// A model entry within a provider.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub limit: Option<CatalogLimit>,
    #[serde(default)]
    pub tool_call: Option<bool>,
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub interleaved: Option<InterleavedHint>,
    #[serde(default)]
    pub modalities: Option<CatalogModalities>,
}

/// Token limits from catalog.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogLimit {
    #[serde(default)]
    pub context: Option<u32>,
    #[serde(default)]
    pub output: Option<u32>,
}

/// Input/output modalities.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

/// Interleaved reasoning hint — either a bare bool or an object with a field name.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InterleavedHint {
    Bool(bool),
    Field { field: Option<String> },
}

/// A flattened model produced from a catalog entry.
#[derive(Debug, Clone)]
pub struct CatalogModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub capabilities: Vec<String>,
}

/// Result of applying a catalog provider: the config-level provider definition
/// and all the models to register.
pub struct CatalogProviderConfig {
    pub provider_type: ApiType,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub models: Vec<CatalogModelInfo>,
}

/// Fetch the full catalog from `models.dev/api.json`.
pub async fn fetch_catalog() -> Result<BTreeMap<String, CatalogEntry>, crate::error::AiError> {
    fetch_catalog_from(CATALOG_URL).await
}

/// Fetch the catalog from a custom URL.
pub async fn fetch_catalog_from(
    url: &str,
) -> Result<BTreeMap<String, CatalogEntry>, crate::error::AiError> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| crate::error::AiError::Network {
            message: e.to_string(),
        })?;

    if !resp.status().is_success() {
        return Err(crate::error::AiError::Network {
            message: format!("catalog fetch returned {}", resp.status()),
        });
    }

    resp.json::<BTreeMap<String, CatalogEntry>>()
        .await
        .map_err(|e| crate::error::AiError::Network {
            message: e.to_string(),
        })
}

/// Infer the provider wire type from catalog entry metadata.
#[must_use]
pub fn infer_api_type(entry: &CatalogEntry) -> Option<ApiType> {
    // 1. Explicit `type` field
    if let Some(t) = &entry.explicit_type
        && let Some(api) = ApiType::from_config_str(t)
    {
        return Some(api);
    }
    // 2. npm/id matching
    let npm = entry.npm.as_deref().unwrap_or("");
    let id = entry.id.as_str();
    if npm.contains("anthropic") || id.contains("claude") {
        return Some(ApiType::Anthropic);
    }
    if id.contains("vertex") {
        return Some(ApiType::Google);
    }
    if npm.contains("google") || id.contains("gemini") {
        return Some(ApiType::Google);
    }
    if npm.contains("openai") {
        return Some(ApiType::OpenAiChat);
    }
    None
}

/// Check if a model is an embedding model (should be skipped).
fn is_embedding_model(model: &CatalogModel) -> bool {
    // Filter by output modality
    if let Some(mods) = &model.modalities
        && !mods.output.is_empty()
        && !mods.output.iter().any(|m| m == "text")
    {
        return true;
    }
    // Filter by name/family
    let check = |s: &str| {
        let lower = s.to_ascii_lowercase();
        lower.contains("embedding") || lower.contains("/embed")
    };
    model.family.as_deref().is_some_and(check)
        || check(&model.id)
        || model.name.as_deref().is_some_and(check)
}

/// Extract usable models from a catalog entry.
#[must_use]
pub fn catalog_provider_models(entry: &CatalogEntry) -> Vec<CatalogModelInfo> {
    entry
        .models
        .values()
        .filter(|m| !is_embedding_model(m))
        .map(|m| CatalogModelInfo {
            id: m.id.clone(),
            name: m.name.clone(),
            max_context_tokens: m.limit.as_ref().and_then(|l| l.context),
            max_output_tokens: m.limit.as_ref().and_then(|l| l.output),
            capabilities: catalog_model_capabilities(m),
        })
        .collect()
}

/// Build capability string list from catalog model fields.
fn catalog_model_capabilities(model: &CatalogModel) -> Vec<String> {
    let mut caps = vec!["streaming".to_owned()];
    if catalog_model_supports_tools(model) {
        caps.push("tools".to_owned());
    }
    if catalog_model_supports_reasoning(model) {
        caps.push("reasoning".to_owned());
    }
    if catalog_model_accepts_images(model) {
        caps.push("images".to_owned());
    }
    caps
}

fn catalog_model_supports_tools(model: &CatalogModel) -> bool {
    model.tool_call.unwrap_or(true)
}

fn catalog_model_supports_reasoning(model: &CatalogModel) -> bool {
    model.reasoning.unwrap_or(false)
}

fn catalog_model_accepts_images(model: &CatalogModel) -> bool {
    model
        .modalities
        .as_ref()
        .is_some_and(|modalities| modalities.input.iter().any(|m| m == "image"))
}

/// Convert a catalog entry to the config-level structures.
///
/// Returns the provider type, base URL, env var, and model list.
#[must_use]
pub fn catalog_to_provider_config(
    entry: &CatalogEntry,
    api_key: Option<&str>,
) -> Option<CatalogProviderConfig> {
    let provider_type = infer_api_type(entry)?;
    let models = catalog_provider_models(entry);
    if models.is_empty() {
        return None;
    }
    Some(CatalogProviderConfig {
        provider_type,
        base_url: entry.api.clone(),
        api_key_env: if api_key.is_none() {
            entry.env.first().cloned()
        } else {
            None
        },
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_api_type_anthropic() {
        let entry = CatalogEntry {
            id: "anthropic".to_owned(),
            name: None,
            api: None,
            env: vec![],
            npm: Some("@ai-sdk/anthropic".to_owned()),
            explicit_type: None,
            models: BTreeMap::new(),
        };
        assert_eq!(infer_api_type(&entry), Some(ApiType::Anthropic));
    }

    #[test]
    fn test_infer_api_type_openai() {
        let entry = CatalogEntry {
            id: "openai".to_owned(),
            name: None,
            api: None,
            env: vec![],
            npm: Some("@ai-sdk/openai".to_owned()),
            explicit_type: None,
            models: BTreeMap::new(),
        };
        assert_eq!(infer_api_type(&entry), Some(ApiType::OpenAiChat));
    }

    #[test]
    fn test_infer_api_type_explicit() {
        let entry = CatalogEntry {
            id: "custom".to_owned(),
            name: None,
            api: None,
            env: vec![],
            npm: None,
            explicit_type: Some("openai-responses".to_owned()),
            models: BTreeMap::new(),
        };
        assert_eq!(infer_api_type(&entry), Some(ApiType::OpenAiResponses));
    }

    #[test]
    fn test_embedding_filter() {
        let model = CatalogModel {
            id: "text-embedding-3".to_owned(),
            name: None,
            family: Some("embedding".to_owned()),
            limit: None,
            tool_call: None,
            reasoning: None,
            interleaved: None,
            modalities: None,
        };
        assert!(is_embedding_model(&model));
    }

    #[test]
    fn catalog_model_capabilities_defaults_to_streaming_and_tools() {
        let model = CatalogModel {
            id: "chat".to_owned(),
            name: None,
            family: None,
            limit: None,
            tool_call: None,
            reasoning: None,
            interleaved: None,
            modalities: None,
        };

        assert_eq!(catalog_model_capabilities(&model), ["streaming", "tools"]);
    }

    #[test]
    fn catalog_model_capabilities_respects_disabled_tools_and_optional_features() {
        let model = CatalogModel {
            id: "vision-reasoning".to_owned(),
            name: None,
            family: None,
            limit: None,
            tool_call: Some(false),
            reasoning: Some(true),
            interleaved: None,
            modalities: Some(CatalogModalities {
                input: vec!["text".to_owned(), "image".to_owned()],
                output: vec!["text".to_owned()],
            }),
        };

        assert_eq!(
            catalog_model_capabilities(&model),
            ["streaming", "reasoning", "images"]
        );
    }
}
