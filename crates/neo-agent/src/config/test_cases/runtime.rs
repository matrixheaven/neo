//! runtime behavior (moved from `mod.rs`).

use super::*;

use crate::config::{
    AppConfig, ConfigOverrides, PermissionMode, RuntimeCompactionConfig, RuntimeConfig,
};

#[test]
fn config_defaults_to_ask_permission_mode() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    assert_eq!(config.permission_mode, PermissionMode::Ask);
}

#[test]
fn config_defaults_shell_limits() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 8);
    assert_eq!(config.runtime.shell.max_command_parallelism, 4);
    assert_eq!(config.runtime.shell.max_command_descendant_processes, 32);
    assert_eq!(config.runtime.shell.max_command_memory_percent, 25);
    assert_eq!(config.runtime.shell.max_output_bytes, 65_536);
    assert_eq!(config.runtime.shell.max_background_log_bytes, 10_485_760);
    let task_suffix = std::path::Path::new("agents").join("main").join("tasks");
    assert!(
        config
            .runtime
            .shell_runtime
            .runtime_root()
            .ends_with(task_suffix)
    );
}

#[test]
fn config_reuses_process_shell_runtime_root() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let first = load_config(config_path.clone(), project_dir.clone());
    let second = load_config(config_path, project_dir);

    assert_eq!(
        first.runtime.shell_runtime.runtime_root(),
        second.runtime.shell_runtime.runtime_root()
    );
}

#[test]
fn config_loads_shell_limit_overrides() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r"
[runtime.shell]
max_active_commands = 1
max_command_parallelism = 2
",
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 1);
    assert_eq!(config.runtime.shell.max_command_parallelism, 2);
}

#[test]
fn runtime_shell_uses_canonical_per_command_limits() {
    let (_temp, config_path, project_dir) = temp_project_config(
        "[runtime.shell]\n\
             max_active_commands = 6\n\
             max_command_parallelism = 8\n\
             max_command_descendant_processes = 40\n\
             max_command_memory_percent = 30\n",
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 6);
    assert_eq!(config.runtime.shell.max_command_parallelism, 8);
    assert_eq!(config.runtime.shell.max_command_descendant_processes, 40);
    assert_eq!(config.runtime.shell.max_command_memory_percent, 30);
}

#[test]
fn runtime_shell_rejects_removed_limit_names() {
    for key in [
        "max_parallelism",
        "max_descendant_processes",
        "max_tree_memory_percent",
    ] {
        let input = format!("[runtime.shell]\n{key} = 1\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("removed key was accepted");
        let message = format!("{error:#}");
        assert!(message.contains(key), "{message}");
        assert!(message.contains("active_commands"), "{message}");
        assert!(message.contains("command_parallelism"), "{message}");
    }
}

#[test]
fn runtime_shell_rejects_removed_timeout_keys() {
    for key in ["foreground_timeout_secs", "background_timeout_secs"] {
        let input = format!("[runtime.shell]\n{key} = 1\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("removed timeout key was accepted");
        let message = format!("{error:#}");
        assert!(message.contains(key), "{message}");
        assert!(message.contains("active_commands"), "{message}");
        assert!(message.contains("command_parallelism"), "{message}");
    }
}

#[test]
fn config_allows_capacity_larger_than_per_command_memory_percent() {
    let (_temp, config_path, project_dir) =
        temp_project_config("[runtime.shell]\nmax_active_commands = 51\n");
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 51);
    assert_eq!(config.runtime.shell.max_command_memory_percent, 25);
}

#[test]
fn config_defaults_to_enabled_runtime_compaction() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    let compaction = config.runtime.compaction.expect("compaction default");
    assert!(compaction.enabled);
    assert_eq!(compaction.keep_recent_messages, 20);
}

#[test]
fn runtime_reasoning_uses_structured_config_and_migrates_legacy_effort() {
    let parsed: crate::config::types::FileConfig = toml::from_str(
        r#"
            [runtime]
            reasoning_effort = "high"
            replay_reasoning = true
            "#,
    )
    .expect("parse legacy config");

    let runtime = super::super::loader::runtime_from_file_for_tests(parsed.runtime);
    assert_eq!(
        runtime.reasoning,
        neo_ai::ReasoningSelection::Effort {
            effort: neo_ai::ReasoningEffort::high(),
        }
    );
}

#[test]
fn runtime_retry_defaults_and_loads_explicit_values() {
    let config = super::super::loader::runtime_from_file_for_tests(Some(
        crate::config::types::FileRuntimeConfig {
            retry: Some(crate::config::types::FileRuntimeRetryConfig {
                max_retries: Some(100),
                first_event_timeout_secs: Some(7),
                stream_idle_timeout_secs: Some(11),
            }),
            ..crate::config::types::FileRuntimeConfig::default()
        },
    ));

    assert_eq!(config.retry.max_retries, 100);
    assert_eq!(config.retry.first_event_timeout_secs, 7);
    assert_eq!(config.retry.stream_idle_timeout_secs, 11);

    let defaults = RuntimeConfig::default().retry;
    assert_eq!(defaults.max_retries, 5);
    assert_eq!(defaults.first_event_timeout_secs, 60);
    assert_eq!(defaults.stream_idle_timeout_secs, 120);
}

#[test]
fn runtime_table_without_compaction_keeps_compaction_enabled() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r"
[runtime]
temperature = 0.2
",
    );
    let config = load_config(config_path, project_dir);
    let compaction = config.runtime.compaction.expect("compaction default");
    assert!(compaction.enabled);
    assert_eq!(compaction.keep_recent_messages, 20);
}

#[test]
fn config_loads_permission_mode_auto() {
    let (_temp, config_path, project_dir) = temp_project_config("permission_mode = \"auto\"\n");
    let config = load_config(config_path, project_dir);
    assert_eq!(config.permission_mode, PermissionMode::Auto);
}
