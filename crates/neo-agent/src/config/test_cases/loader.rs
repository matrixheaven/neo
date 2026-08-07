//! loader behavior (moved from `mod.rs`).

use super::*;
use std::fs;

use neo_agent_core::QueueMode;
use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId};
use tempfile::TempDir;

use crate::config::{AppConfig, ConfigOverrides, PermissionMode, ThemeResolution};
use crate::themes::ThemeId;
use crate::trust::{ProjectTrustState, ProjectTrustStore};

#[test]
fn no_config_file_shows_unconfigured_label() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: false,
        auto: false,
        trust_store: None,
        project_dir: Some(project_dir),
    })
    .expect("load config without file");
    assert!(!config.config_file_exists);
    assert_eq!(
        config.default_model_label(),
        "No configured providers/models"
    );
}

#[test]
fn config_defaults_follow_up_queue_to_all() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.follow_up_queue_mode, QueueMode::All);
}

#[test]
fn config_loads_builtin_workflow_definitions() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    let builtins = config
        .workflow_definitions
        .list(neo_agent_core::workflow::WorkflowListScope::Builtin)
        .expect("list built-in workflows");

    assert_eq!(
        builtins
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["code-review", "deep-research", "large-refactor"]
    );
}

#[test]
fn cli_permission_flags_override_config_permission_mode() {
    let cases = [
        ("yolo", true, false, PermissionMode::Yolo),
        ("auto", false, true, PermissionMode::Auto),
    ];

    for (name, yolo, auto, expected) in cases {
        let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"ask\"\n");
        let config = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo,
            auto,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config");
        assert_eq!(config.permission_mode, expected, "case {name}");
    }
}

#[test]
fn scoped_models_matches_globs_against_qualified_and_model_ids() {
    let openai = ModelSpec {
        provider: ProviderId("openai".to_owned()),
        model: "gpt-4.1".to_owned(),
        api: ApiKind::OpenAiResponse,
        capabilities: ModelCapabilities::tool_chat(),
    };
    let anthropic = ModelSpec {
        provider: ProviderId("anthropic".to_owned()),
        model: "claude-sonnet-4".to_owned(),
        api: ApiKind::AnthropicMessages,
        capabilities: ModelCapabilities::tool_chat(),
    };

    let models = [openai, anthropic];
    let scoped = super::super::scoped_models(
        models.iter(),
        &["openai/gpt-*".to_owned(), "claude-??????-4:high".to_owned()],
    );

    assert_eq!(
        scoped
            .iter()
            .map(|model| format!("{}/{}", model.provider.0, model.model))
            .collect::<Vec<_>>(),
        vec!["openai/gpt-4.1", "anthropic/claude-sonnet-4"]
    );
}

#[test]
fn config_trust_is_not_required_for_directory_without_inputs() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir.clone());
    assert!(config.project_trusted);
    assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
}

#[test]
fn config_trust_is_unknown_when_inputs_exist_without_decision() {
    let (temp, config_path, project_dir) = temp_project_config("");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
    let store = ProjectTrustStore::new(temp.path().join("trust.json"));

    let config = load_config_with_store(config_path, project_dir.clone(), store);

    assert!(!config.project_trusted);
    assert!(matches!(
        config.project_trust,
        ProjectTrustState::Unknown { .. }
    ));
}

#[test]
fn config_trust_is_trusted_when_store_approves_current_dir() {
    let (temp, config_path, project_dir) = temp_project_config("");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
    let store = ProjectTrustStore::new(temp.path().join("trust.json"));
    store.set(&project_dir, Some(true)).expect("approve");

    let config = load_config_with_store(config_path, project_dir.clone(), store);

    assert!(config.project_trusted);
    assert_eq!(
        config.project_trust,
        ProjectTrustState::Trusted {
            target: project_dir.canonicalize().expect("canonicalize"),
        }
    );
}

#[test]
fn config_trust_is_untrusted_when_store_denies_current_dir() {
    let (temp, config_path, project_dir) = temp_project_config("");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
    let store = ProjectTrustStore::new(temp.path().join("trust.json"));
    store.set(&project_dir, Some(false)).expect("deny");

    let config = load_config_with_store(config_path, project_dir.clone(), store);

    assert!(!config.project_trusted);
    assert_eq!(
        config.project_trust,
        ProjectTrustState::Untrusted {
            target: project_dir.canonicalize().expect("canonicalize"),
        }
    );
}

#[test]
fn config_yolo_sets_not_required_and_untrusted() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: true,
        auto: false,
        trust_store: None,
        project_dir: Some(project_dir),
    })
    .expect("load config");

    assert!(!config.project_trusted);
    assert_eq!(config.project_trust, ProjectTrustState::NotRequired);
}

#[test]
fn config_explicit_theme_resolves_and_is_never_fuzzy() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let themes_dir = temp.path().join("themes");
    fs::create_dir_all(&themes_dir).expect("create themes");
    fs::write(
        themes_dir.join("night.json"),
        r##"{"name": "Night", "colors": {"brand": "#123456"}}"##,
    )
    .expect("write theme");
    // A sorted-first sibling with a different name must NOT be selected for
    // the explicit id, and a partial-name reference must not fuzzy-match.
    fs::write(
        themes_dir.join("aaa.json"),
        r##"{"name": "Aaa", "colors": {"brand": "#654321"}}"##,
    )
    .expect("write sibling");
    fs::write(&config_path, config_with_theme("", "night.json")).expect("write config");
    fs::create_dir_all(temp.path().join("project")).expect("create project");

    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: false,
        auto: false,
        trust_store: None,
        project_dir: Some(temp.path().join("project")),
    })
    .expect("load config");

    assert_eq!(config.theme.name, "Night");
    assert_eq!(
        config.theme.id.as_ref().map(ThemeId::as_str),
        Some("night.json")
    );
    assert!(
        matches!(config.theme_resolution, ThemeResolution::Explicit(_)),
        "explicit id must not fall back to discovery"
    );
}

#[test]
fn config_invalid_explicit_theme_uses_default_with_diagnostic() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let themes_dir = temp.path().join("themes");
    fs::create_dir_all(&themes_dir).expect("create themes");
    fs::write(
        themes_dir.join("aaa.json"),
        r##"{"name": "Aaa", "colors": {}}"##,
    )
    .expect("write sibling");
    fs::write(&config_path, config_with_theme("", "../escape.json")).expect("write config");
    fs::create_dir_all(temp.path().join("project")).expect("create project");

    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: false,
        auto: false,
        trust_store: None,
        project_dir: Some(temp.path().join("project")),
    })
    .expect("load config");

    assert_eq!(config.theme.name, "default");
    assert!(config.theme.id.is_none());
    let diagnostic = config.theme_resolution.diagnostic().expect("diagnostic");
    assert!(diagnostic.contains("../escape.json"), "{diagnostic}");
    assert!(
        !matches!(config.theme_resolution, ThemeResolution::Discovered(_)),
        "an explicit invalid id must never re-enter sorted-first discovery"
    );
}

#[test]
fn config_missing_explicit_theme_uses_default_with_diagnostic() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let themes_dir = temp.path().join("themes");
    fs::create_dir_all(&themes_dir).expect("create themes");
    fs::write(
        themes_dir.join("aaa.json"),
        r##"{"name": "Aaa", "colors": {}}"##,
    )
    .expect("write sibling");
    fs::write(&config_path, config_with_theme("", "missing.json")).expect("write config");
    fs::create_dir_all(temp.path().join("project")).expect("create project");

    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: false,
        auto: false,
        trust_store: None,
        project_dir: Some(temp.path().join("project")),
    })
    .expect("load config");

    assert_eq!(config.theme.name, "default");
    let diagnostic = config.theme_resolution.diagnostic().expect("diagnostic");
    assert!(diagnostic.contains("missing.json"), "{diagnostic}");
    assert!(
        !matches!(config.theme_resolution, ThemeResolution::Discovered(_)),
        "a missing explicit id must never fall back to another JSON file"
    );
}

#[test]
fn config_absent_theme_keeps_sorted_first_discovery() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let themes_dir = temp.path().join("themes");
    fs::create_dir_all(&themes_dir).expect("create themes");
    fs::write(
        themes_dir.join("zz.json"),
        r##"{"name": "Zed", "colors": {}}"##,
    )
    .expect("write theme");
    fs::write(
        themes_dir.join("aa.json"),
        r##"{"name": "Alpha", "colors": {"brand": "#010203"}}"##,
    )
    .expect("write theme");
    fs::write(&config_path, "default_model = \"x\"\n").expect("write config");
    fs::create_dir_all(temp.path().join("project")).expect("create project");

    let config = AppConfig::load(ConfigOverrides {
        config_path: Some(config_path),
        yolo: false,
        auto: false,
        trust_store: None,
        project_dir: Some(temp.path().join("project")),
    })
    .expect("load config");

    assert_eq!(config.theme.name, "Alpha");
    assert!(
        config.theme.id.is_none(),
        "discovered themes carry no explicit id"
    );
    assert!(matches!(
        config.theme_resolution,
        ThemeResolution::Discovered(_)
    ));
    assert!(config.theme_resolution.diagnostic().is_none());
}
