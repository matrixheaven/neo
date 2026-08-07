//! providers behavior (moved from `mutations.rs`).

use super::*;
use std::fs::{self};

use neo_ai::ApiType;
use tempfile::TempDir;

use super::super::{
    add_provider_from_catalog_entry, list_providers, refresh_provider_models_from_catalog_entry,
    remove_provider,
};
use crate::config::{ModelConfig, ProviderConfig};

#[test]
fn list_providers_formats_text_and_json_entries() {
    let temp = TempDir::new().expect("temp dir");
    let mut config = test_config(temp.path());
    config.default_model = "openai/gpt-4.1".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            base_url: Some("https://api.openai.test/v1".to_owned()),
            api_key: Some("secret".to_owned()),
            api_key_env: None,
        },
    );
    config.models.insert(
        "openai/gpt-4.1".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-4.1".to_owned(),
            ..ModelConfig::default()
        },
    );

    let text = list_providers(&config, false).expect("provider text");
    assert!(text.contains("openai"));
    assert!(text.contains("type=openai_response"));
    assert!(text.contains("models=1"));
    assert!(text.contains("cred=api_key"));
    assert!(text.contains("current"));
    assert!(text.contains("Default model: openai/gpt-4.1"));

    let json_output = list_providers(&config, true).expect("provider json");
    let value: serde_json::Value = serde_json::from_str(&json_output).expect("json");
    assert_eq!(
        value,
        serde_json::json!({
            "providers": [{
                "id": "openai",
                "type": "openai_response",
                "base_url": "https://api.openai.test/v1",
                "credential": "api_key",
                "model_count": 1,
                "default": true,
            }],
            "default_model": "openai/gpt-4.1",
        })
    );
}

#[test]
fn remove_provider_drops_models_and_default_for_that_provider_only() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.test/v1"

[providers.anthropic]
type = "anthropic"
base_url = "https://api.anthropic.test"

[models."openai/gpt-4.1"]
provider = "openai"
model = "gpt-4.1"

[models."anthropic/sonnet"]
provider = "anthropic"
model = "claude-sonnet-4"
"#,
    );

    let message = remove_provider(&config_path, "openai").expect("remove provider");
    assert_eq!(message, "removed provider 'openai' and its models\n");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(!written.contains("[providers.openai]"));
    assert!(written.contains("[providers.anthropic]"));
    assert!(!written.contains("[models.\"openai/gpt-4.1\"]"));
    assert!(written.contains("[models.\"anthropic/sonnet\"]"));
    assert!(!written.contains("default_model"));
}

#[test]
fn add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join(".neo/config.toml");

    let message = super::super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAi),
            base_url: Some("https://gateway.example.com/v1".to_owned()),
            api_key: None,
            api_key_env: Some("ACME_API_KEY".to_owned()),
        },
        vec![(
            "acme/qwen2.5-coder-32b-instruct".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "qwen2.5-coder-32b-instruct".to_owned(),
                max_context_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                capabilities: vec![
                    "streaming".to_owned(),
                    "tools".to_owned(),
                    "reasoning".to_owned(),
                ],
                reasoning: neo_ai::ReasoningCapability::Effort {
                    values: vec![
                        neo_ai::ReasoningEffort::low(),
                        neo_ai::ReasoningEffort::medium(),
                        neo_ai::ReasoningEffort::high(),
                    ],
                    disable_supported: true,
                },
                display_name: Some("Qwen 2.5 Coder 32B".to_owned()),
            },
        )],
        None,
    )
    .expect("add custom endpoint provider");

    assert_eq!(message, "added provider 'acme' with 1 model\n");
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(written.contains("type = \"openai\""), "{written}");
    assert!(
        written.contains("api_key_env = \"ACME_API_KEY\""),
        "{written}"
    );
    assert!(
        written.contains("[models.\"acme/qwen2.5-coder-32b-instruct\"]"),
        "{written}"
    );
    assert!(written.contains("max_context_tokens = 128000"), "{written}");
    assert!(written.contains("max_output_tokens = 8192"), "{written}");
    assert!(written.contains("type = \"effort\""), "{written}");
    assert!(
        written.contains("default_model = \"acme/qwen2.5-coder-32b-instruct\""),
        "{written}"
    );
    assert!(written.contains("default_provider = \"acme\""), "{written}");
}

#[test]
fn add_custom_endpoint_provider_replaces_existing_provider_models_only() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
default_model = "other/keep"

[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
    );

    super::super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::Google),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
            api_key: Some("local".to_owned()),
            api_key_env: None,
        },
        vec![(
            "acme/gemini-custom".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "models/gemini-custom".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        )],
        None,
    )
    .expect("replace custom endpoint provider");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(
        written.contains("default_model = \"other/keep\""),
        "{written}"
    );
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(written.contains("type = \"google\""), "{written}");
    assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
    assert!(
        written.contains("[models.\"acme/gemini-custom\"]"),
        "{written}"
    );
    assert!(written.contains("[models.\"other/keep\"]"), "{written}");
}

#[test]
fn add_custom_endpoint_provider_accepts_empty_models_and_does_not_invent_default() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
    );

    let message = super::super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::Google),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
            api_key: Some("local".to_owned()),
            api_key_env: None,
        },
        Vec::new(),
        None,
    )
    .expect("replace custom endpoint provider without models");

    assert_eq!(message, "added provider 'acme' with 0 models\n");
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(written.contains("type = \"google\""), "{written}");
    assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
    assert!(written.contains("[models.\"other/keep\"]"), "{written}");
    assert!(!written.contains("default_model"), "{written}");
    assert!(!written.contains("default_provider"), "{written}");
}

#[test]
fn add_custom_endpoint_provider_rejects_explicit_default_outside_supplied_aliases() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
    );

    let err = super::super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::Google),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
            api_key: Some("local".to_owned()),
            api_key_env: None,
        },
        vec![(
            "acme/gemini-custom".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "models/gemini-custom".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        )],
        Some("acme/missing"),
    )
    .expect_err("invalid explicit default should be rejected");

    assert!(
        err.to_string()
            .contains("default model 'acme/missing' is not one of the supplied model aliases"),
        "{err}"
    );
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(!written.contains("[providers.acme]"), "{written}");
    assert!(
        !written.contains("[models.\"acme/gemini-custom\"]"),
        "{written}"
    );
    assert!(!written.contains("default_model"), "{written}");
    assert!(written.contains("[providers.other]"), "{written}");
    assert!(written.contains("[models.\"other/keep\"]"), "{written}");
}

#[test]
fn add_custom_endpoint_provider_invalidated_provider_default_uses_first_alias() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
default_model = "acme/old"
default_provider = "acme"

[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"
"#,
    );

    super::super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::Google),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
            api_key: Some("local".to_owned()),
            api_key_env: None,
        },
        vec![(
            "acme/gemini-custom".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "models/gemini-custom".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        )],
        None,
    )
    .expect("replace custom endpoint provider");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(
        written.contains("default_model = \"acme/gemini-custom\""),
        "{written}"
    );
    assert!(written.contains("default_provider = \"acme\""), "{written}");
    assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
    assert!(
        written.contains("[models.\"acme/gemini-custom\"]"),
        "{written}"
    );
}

#[test]
fn remove_provider_clears_unqualified_default_alias_owned_by_provider() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
default_model = "fast"
default_provider = "openai"

[providers.openai]
type = "openai_response"

[models.fast]
provider = "openai"
model = "gpt-4.1"
"#,
    );

    let message = remove_provider(&config_path, "openai").expect("remove provider");
    assert_eq!(message, "removed provider 'openai' and its models\n");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(!written.contains("default_model"));
    assert!(!written.contains("[models.fast]"));
}

#[test]
fn refresh_provider_models_preserves_provider_and_surviving_default() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_header(
        temp.path(),
        r#"
default_model = "openai/custom-large"
default_provider = "openai"

[providers.openai]
display_name = "My OpenAI"
type = "openai"
base_url = "https://gateway.example/v1"
api_key = "keep-me"

[providers.other]
type = "openai"

[models."openai/custom-large"]
provider = "openai"
model = "gpt-large"

[models."openai/old"]
provider = "openai"
model = "old"

[models."other/stays"]
provider = "other"
model = "stays"

[runtime.reasoning]
mode = "effort"
effort = "high"
"#,
    );

    let message =
        refresh_provider_models_from_catalog_entry(&config_path, "openai", &catalog_entry())
            .expect("refresh provider models");

    assert_eq!(message, "refreshed provider 'openai' with 2 models\n");
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(
        written.contains("display_name = \"My OpenAI\""),
        "{written}"
    );
    assert!(written.contains("base_url = \"https://gateway.example/v1\""));
    assert!(written.contains("api_key = \"keep-me\""), "{written}");
    assert!(written.contains("default_model = \"openai/gpt-large\""));
    assert!(written.contains("default_provider = \"openai\""));
    assert!(written.contains("effort = \"high\""), "{written}");
    assert!(written.contains("[models.\"openai/gpt-small\"]"));
    assert!(written.contains("[models.\"openai/gpt-large\"]"));
    assert!(!written.contains("[models.\"openai/old\"]"));
    assert!(written.contains("[models.\"other/stays\"]"));
    assert!(written.contains("[providers.other]"));
}

#[test]
fn refresh_provider_models_falls_back_with_automatic_reasoning() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_header(
        temp.path(),
        r#"
default_model = "openai/removed"
default_provider = "openai"

[providers.openai]
type = "openai_response"

[models."openai/removed"]
provider = "openai"
model = "removed"

[runtime.reasoning]
mode = "effort"
effort = "max"
"#,
    );

    refresh_provider_models_from_catalog_entry(&config_path, "openai", &catalog_entry())
        .expect("refresh provider models");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("default_model = \"openai/gpt-large\""));
    assert!(written.contains("default_provider = \"openai\""));
    assert!(written.contains("effort = \"medium\""), "{written}");
    assert!(!written.contains("effort = \"max\""), "{written}");
}

#[test]
fn add_provider_from_catalog_entry_replaces_existing_provider_models() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_header(
        temp.path(),
        r#"
default_model = "openai/old"

[providers.openai]
type = "openai"
base_url = "https://old.example/v1"

[models."openai/old"]
provider = "openai"
model = "old"

[models."other/stays"]
provider = "other"
model = "stays"
"#,
    );
    let entry = catalog_entry();

    let message = add_provider_from_catalog_entry(
        &config_path,
        "openai",
        &entry,
        Some("inline-key"),
        Some("gpt-large"),
    )
    .expect("import provider");

    assert_eq!(
        message,
        "imported provider 'openai' with 2 models from models.dev\n"
    );
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("default_model = \"openai/gpt-large\""));
    assert!(written.contains("default_provider = \"openai\""));
    assert!(written.contains("[providers.openai]"));
    assert!(written.contains("type = \"openai_response\""));
    assert!(written.contains("api_key = \"inline-key\""));
    assert!(written.contains("[models.\"openai/gpt-small\"]"));
    assert!(written.contains("[models.\"openai/gpt-large\"]"));
    let written_toml: toml::Value = toml::from_str(&written).expect("parse written config");
    let reasoning = written_toml
        .get("models")
        .and_then(toml::Value::as_table)
        .and_then(|models| models.get("openai/gpt-large"))
        .and_then(toml::Value::as_table)
        .and_then(|model| model.get("reasoning"))
        .and_then(toml::Value::as_table)
        .expect("typed model reasoning");
    assert_eq!(
        reasoning.get("type").and_then(toml::Value::as_str),
        Some("combined")
    );
    assert_eq!(
        reasoning
            .get("effort")
            .and_then(toml::Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .collect::<Vec<_>>()
            }),
        Some(vec!["low", "medium", "high"])
    );
    assert_eq!(
        reasoning.get("toggle").and_then(toml::Value::as_bool),
        Some(true)
    );
    let budget = reasoning
        .get("budget")
        .and_then(toml::Value::as_table)
        .expect("typed combined budget");
    assert_eq!(
        budget.get("min").and_then(toml::Value::as_integer),
        Some(128)
    );
    assert_eq!(
        budget.get("max").and_then(toml::Value::as_integer),
        Some(24_576)
    );
    assert_eq!(
        reasoning
            .get("disable_supported")
            .and_then(toml::Value::as_bool),
        Some(true)
    );
    assert!(written.contains("[models.\"other/stays\"]"));
    assert!(!written.contains("[models.\"openai/old\"]"));
    assert!(!written.contains("OPENAI_API_KEY"));
}

/// Regression: importing a *new* provider must update `default_provider` to
/// match the new default model alias (`<provider>/<model>`). Otherwise the
/// model label formatter (`{default_provider}/{default_model}`) stitches the
/// stale provider onto the new alias, producing e.g.
/// `deepseek/minimax-.../MiniMax-M2`.
#[test]
fn add_provider_syncs_default_provider_to_new_provider() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_header(
        temp.path(),
        r#"
default_model = "deepseek/old"
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
base_url = "https://deepseek.example/v1"

[models."deepseek/old"]
provider = "deepseek"
model = "old"
"#,
    );
    let entry = catalog_entry();

    let message = add_provider_from_catalog_entry(
        &config_path,
        "openai",
        &entry,
        Some("inline-key"),
        Some("gpt-large"),
    )
    .expect("import provider");

    assert_eq!(
        message,
        "imported provider 'openai' with 2 models from models.dev\n"
    );
    let written = fs::read_to_string(config_path).expect("read config");
    // The new provider's default alias and provider must be consistent so
    // the label is `openai/gpt-large`, not `deepseek/openai/gpt-large`.
    assert!(written.contains("default_model = \"openai/gpt-large\""));
    assert!(written.contains("default_provider = \"openai\""));
    assert!(!written.contains("default_provider = \"deepseek\""));
}
