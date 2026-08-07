//! output behavior (moved from `mod.rs`).

use super::*;
use std::path::Path;

use neo_agent_core::AgentEvent;
use neo_ai::ApiType;

use super::super::models_cli::list_configured_models;
use super::super::runtime::{model_registry_for_config, select_config_model};
use crate::config::{ModelConfig, ProviderConfig};

#[test]
fn stable_json_redacts_instruction_metadata_paths_and_failure_detail() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let neo_home = temp.path().join("neo-home");
    let outside = temp.path().join("private/rules.md");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&neo_home).expect("neo home");
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let neo_home = neo_home.canonicalize().expect("canonical neo home");
    #[cfg(unix)]
    let configured_neo_home = {
        let link = temp.path().join("neo-home-link");
        std::os::unix::fs::symlink(&neo_home, &link).expect("neo home symlink");
        link
    };
    #[cfg(not(unix))]
    let configured_neo_home = neo_home.clone();
    let nested = workspace.join("crates/neo-tui");
    let epoch = neo_agent_core::instructions::InstructionEpochData {
        agent_id: "main".to_owned(),
        generation: 7,
        outcome: neo_agent_core::instructions::InstructionEpochOutcome::Blocked,
        scopes: vec![neo_agent_core::instructions::InstructionScopeData {
            display_path: nested.clone(),
            kind: neo_agent_core::instructions::InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 1_024,
        }],
        selected_bundles: vec![neo_agent_core::instructions::InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 1_024,
            byte_size: 4_096,
            source_count: 2,
            import_count: 2,
            import_paths: vec![neo_home.join("CX.md"), outside.clone()],
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: Some(neo_agent_core::instructions::InstructionFailure {
            fingerprint: "failure-fingerprint".to_owned(),
            display_path: outside,
            kind: neo_agent_core::instructions::InstructionFailureKind::MissingImport,
            detail: "PRIVATE FAILURE DETAIL".to_owned(),
        }),
        deferred_tool_ids: vec!["call-1".to_owned()],
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
    };
    let turn = super::super::PromptTurn {
        session_id: "session_00000000-0000-4000-8000-000000000607".to_owned(),
        events: vec![AgentEvent::InstructionEpoch { epoch }],
        assistant_text: String::new(),
    };
    let config = test_config(&workspace);

    let output = temp_env::with_var("NEO_HOME", Some(configured_neo_home.as_os_str()), || {
        super::super::output::stable_json_output(&turn, &config).expect("stable JSON")
    });
    let record = output
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON line"))
        .find(|value| value["type"] == "instruction_epoch")
        .expect("instruction epoch record");
    let encoded = record.to_string();

    assert_eq!(
        record["scopes"][0]["display_path"],
        Path::new("crates").join("neo-tui").display().to_string()
    );
    assert_eq!(
        record["selectedBundles"][0]["import_paths"],
        serde_json::json!([
            Path::new("$NEO_HOME").join("CX.md").display().to_string(),
            "<outside-workspace>"
        ])
    );
    assert_eq!(record["failure"]["display_path"], "<outside-workspace>");
    assert!(record["failure"].get("detail").is_none(), "{record}");
    for secret in [
        temp.path().display().to_string(),
        "PRIVATE FAILURE DETAIL".to_owned(),
        "SECRET INSTRUCTION BODY".to_owned(),
    ] {
        assert!(!encoded.contains(&secret), "leaked {secret}: {encoded}");
    }
}

#[test]
fn events_output_projects_instruction_epoch_to_display_safe_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let outside = temp.path().join("private/rules.md");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let canonical_workspace = workspace.canonicalize().expect("canonical workspace");
    let epoch = neo_agent_core::instructions::InstructionEpochData {
        agent_id: "main".to_owned(),
        generation: 9,
        outcome: neo_agent_core::instructions::InstructionEpochOutcome::Blocked,
        scopes: vec![neo_agent_core::instructions::InstructionScopeData {
            display_path: canonical_workspace.clone(),
            kind: neo_agent_core::instructions::InstructionScopeKind::WorkspaceRoot,
            revision: None,
            token_estimate: 0,
        }],
        selected_bundles: Vec::new(),
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: Some(neo_agent_core::instructions::InstructionFailure {
            fingerprint: "failure-fingerprint".to_owned(),
            display_path: outside,
            kind: neo_agent_core::instructions::InstructionFailureKind::MissingImport,
            detail: "PRIVATE FAILURE DETAIL".to_owned(),
        }),
        deferred_tool_ids: vec!["call-1".to_owned()],
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
    };
    let turn = super::super::PromptTurn {
        session_id: "session_00000000-0000-4000-8000-000000000608".to_owned(),
        events: vec![AgentEvent::InstructionEpoch { epoch }],
        assistant_text: String::new(),
    };
    let config = test_config(&workspace);

    let output = super::super::events_output(&turn, &config).expect("events output");
    let record: serde_json::Value = serde_json::from_str(output.trim()).expect("event JSON");
    let encoded = record.to_string();

    assert_eq!(record["type"], "instruction_epoch");
    assert_eq!(record["scopes"][0]["display_path"], ".");
    assert_eq!(record["failure"]["display_path"], "<outside-workspace>");
    assert!(record["failure"].get("detail").is_none(), "{record}");
    assert!(!encoded.contains("PRIVATE FAILURE DETAIL"), "{encoded}");
    assert!(!encoded.contains("SECRET INSTRUCTION BODY"), "{encoded}");
    assert!(
        !encoded.contains(&temp.path().display().to_string()),
        "{encoded}"
    );
}

#[test]
fn list_configured_models_formats_text_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "gpt-4.1".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            ..ProviderConfig::default()
        },
    );
    config.models.insert(
        "fast".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-4.1".to_owned(),
            max_context_tokens: Some(1_000_000),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            display_name: Some("GPT 4.1".to_owned()),
            ..ModelConfig::default()
        },
    );
    config.models.insert(
        "local/echo".to_owned(),
        ModelConfig {
            provider: "missing".to_owned(),
            model: "echo".to_owned(),
            capabilities: vec!["streaming".to_owned()],
            ..ModelConfig::default()
        },
    );

    let output = list_configured_models(&config, false).expect("models list");

    assert_eq!(
        output,
        concat!(
            "models:\n",
            "- fast -> openai/gpt-4.1 (openai_response default) ctx=1000000 [streaming,tools] - GPT 4.1\n",
            "- local/echo (unknown) ctx=? [streaming]\n",
        )
    );
}

#[test]
fn list_configured_models_formats_json_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "fast".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            ..ProviderConfig::default()
        },
    );
    config.models.insert(
        "fast".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-4.1".to_owned(),
            max_context_tokens: Some(1_000_000),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            display_name: Some("GPT 4.1".to_owned()),
            ..ModelConfig::default()
        },
    );

    let output = list_configured_models(&config, true).expect("models json");
    let value: serde_json::Value = serde_json::from_str(&output).expect("json output");

    assert_eq!(
        value,
        serde_json::json!({
            "models": [{
                "alias": "fast",
                "provider": "openai",
                "model": "gpt-4.1",
                "type": "openai_response",
                "capabilities": ["streaming", "tools"],
                "max_context_tokens": 1_000_000,
                "display_name": "GPT 4.1",
                "default": true,
            }],
            "default_model": "fast",
        })
    );
}

#[test]
fn select_config_model_accepts_default_model_alias() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "openai/gpt-large".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            ..ProviderConfig::default()
        },
    );
    config.models.insert(
        "openai/gpt-large".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-large".to_owned(),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            ..ModelConfig::default()
        },
    );

    let registry = model_registry_for_config(&config).expect("registry");
    let model = select_config_model(&registry, &config).expect("model resolves");

    assert_eq!(model.provider.0, "openai");
    assert_eq!(model.model, "gpt-large");
}

#[test]
fn configured_model_registry_uses_typed_reasoning_metadata() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "reasoner".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            ..ProviderConfig::default()
        },
    );
    config.models.insert(
        "reasoner".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-reasoner".to_owned(),
            capabilities: vec![
                "streaming".to_owned(),
                "tools".to_owned(),
                "reasoning".to_owned(),
            ],
            reasoning: neo_ai::ReasoningCapability::Effort {
                values: vec![
                    neo_ai::ReasoningEffort::low(),
                    neo_ai::ReasoningEffort::high(),
                ],
                disable_supported: true,
            },
            ..ModelConfig::default()
        },
    );

    let registry = model_registry_for_config(&config).expect("registry");
    let model = select_config_model(&registry, &config).expect("model resolves");

    assert_eq!(
        model.capabilities.reasoning,
        neo_ai::ReasoningCapability::Effort {
            values: vec![
                neo_ai::ReasoningEffort::low(),
                neo_ai::ReasoningEffort::high()
            ],
            disable_supported: true,
        }
    );
}

#[test]
fn select_config_model_accepts_unqualified_config_alias() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "fast".to_owned();
    config.providers.insert(
        "openai".to_owned(),
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            ..ProviderConfig::default()
        },
    );
    config.models.insert(
        "fast".to_owned(),
        ModelConfig {
            provider: "openai".to_owned(),
            model: "gpt-4.1".to_owned(),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            ..ModelConfig::default()
        },
    );

    let registry = model_registry_for_config(&config).expect("registry");
    let model = select_config_model(&registry, &config).expect("alias resolves");

    assert_eq!(model.provider.0, "openai");
    assert_eq!(model.model, "gpt-4.1");
}

#[test]
fn select_config_model_accepts_bare_default_model_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path());
    config.default_provider = "openai".to_owned();
    config.default_model = "gpt-4.1".to_owned();

    let registry = model_registry_for_config(&config).expect("registry");
    let model = select_config_model(&registry, &config).expect("builtin model resolves");

    assert_eq!(model.provider.0, "openai");
    assert_eq!(model.model, "gpt-4.1");
}
