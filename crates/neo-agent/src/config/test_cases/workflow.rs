//! workflow behavior (moved from `mod.rs`).

use super::*;

use crate::config::{AppConfig, ConfigOverrides};

#[test]
fn workflow_machine_defaults_are_host_occupancy_limits() {
    let (_temp, config_path, project_dir) = temp_project_config("");
    let config = load_config(config_path, project_dir);
    let limits = &config.runtime.workflow;
    let defaults = neo_agent_core::workflow::WorkflowLimits::default();

    assert_eq!(limits.lua_source_bytes, defaults.lua_source_bytes);
    assert_eq!(limits.manifest_bytes, defaults.manifest_bytes);
    assert_eq!(limits.lua_vm_memory_bytes, defaults.lua_vm_memory_bytes);
    assert_eq!(limits.pause_hook_interval, defaults.pause_hook_interval);
    assert_eq!(
        limits.max_uninterrupted_instructions,
        defaults.max_uninterrupted_instructions
    );
    assert_eq!(limits.journal_record_bytes, defaults.journal_record_bytes);
    assert_eq!(limits.journal_total_bytes, defaults.journal_total_bytes);
    assert_eq!(limits.artifact_record_bytes, defaults.artifact_record_bytes);
    assert_eq!(limits.artifact_total_bytes, defaults.artifact_total_bytes);
    assert_eq!(limits.global_storage_bytes, defaults.global_storage_bytes);
    assert_eq!(limits.pending_record_bytes, defaults.pending_record_bytes);
    assert_eq!(
        limits.task_output_page_bytes,
        defaults.task_output_page_bytes
    );
    assert_eq!(limits.max_active_vms, defaults.max_active_vms);
    assert_eq!(limits.max_active_workers, defaults.max_active_workers);
    assert_eq!(limits.max_active_executors, defaults.max_active_executors);
    assert_eq!(limits.swarm_concurrency, defaults.swarm_concurrency);
    assert_eq!(config.workflow_runtime.limits(), *limits);
    assert_eq!(
        config
            .workflow_runtime
            .admission()
            .occupancy()
            .active_workers,
        0
    );
}

#[test]
fn workflow_task_output_limit_is_capped_by_tool_output_limit() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r#"
[runtime.shell]
max_output_bytes = 32768

[runtime.workflow]
task_output_page_bytes = 65536
"#,
    );

    let config = load_config(config_path, project_dir);

    assert_eq!(config.runtime.shell.max_output_bytes, 32_768);
    assert_eq!(config.runtime.workflow.task_output_page_bytes, 32_768);
    assert_eq!(
        config.workflow_runtime.limits().task_output_page_bytes,
        32_768
    );
}

#[test]
fn workflow_machine_limits_map_all_v2_fields() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r"
[runtime.workflow]
lua_source_bytes = 2048
manifest_bytes = 1024
lua_vm_memory_bytes = 4096
pause_hook_interval = 123
max_uninterrupted_instructions = 456
journal_record_bytes = 8192
journal_total_bytes = 16384
artifact_record_bytes = 2048
artifact_total_bytes = 32768
global_storage_bytes = 65536
pending_record_bytes = 4096
task_output_page_bytes = 512
max_active_vms = 2
max_active_workers = 3
max_active_executors = 5
swarm_concurrency = 12
",
    );
    let config = load_config(config_path, project_dir);
    let limits = &config.runtime.workflow;

    assert_eq!(limits.lua_source_bytes, 2048);
    assert_eq!(limits.manifest_bytes, 1024);
    assert_eq!(limits.lua_vm_memory_bytes, 4096);
    assert_eq!(limits.pause_hook_interval, 123);
    assert_eq!(limits.max_uninterrupted_instructions, 456);
    assert_eq!(limits.journal_record_bytes, 8192);
    assert_eq!(limits.journal_total_bytes, 16384);
    assert_eq!(limits.artifact_record_bytes, 2048);
    assert_eq!(limits.artifact_total_bytes, 32768);
    assert_eq!(limits.global_storage_bytes, 65536);
    assert_eq!(limits.pending_record_bytes, 4096);
    assert_eq!(limits.task_output_page_bytes, 512);
    assert_eq!(limits.max_active_vms, 2);
    assert_eq!(limits.max_active_workers, 3);
    assert_eq!(limits.max_active_executors, 5);
    assert_eq!(limits.swarm_concurrency, 12);
    assert_eq!(config.workflow_runtime.limits(), *limits);
    assert_eq!(
        config
            .workflow_runtime
            .admission()
            .limits()
            .max_active_workers,
        3
    );
}

#[test]
fn workflow_machine_rejects_invalid_limits() {
    let invalid = [
        ("lua_source_bytes", "0"),
        ("manifest_bytes", "0"),
        ("lua_vm_memory_bytes", "0"),
        ("pause_hook_interval", "0"),
        ("pause_hook_interval", "4294967296"),
        ("max_uninterrupted_instructions", "0"),
        ("journal_record_bytes", "0"),
        ("journal_total_bytes", "0"),
        ("artifact_record_bytes", "0"),
        ("artifact_total_bytes", "0"),
        ("global_storage_bytes", "0"),
        ("pending_record_bytes", "0"),
        ("task_output_page_bytes", "0"),
        ("max_active_vms", "0"),
        ("max_active_workers", "0"),
        ("max_active_executors", "0"),
        ("swarm_concurrency", "0"),
    ];
    for (key, value) in invalid {
        let input = format!("[runtime.workflow]\n{key} = {value}\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("invalid workflow limit was accepted");
        let message = format!("{error:#}");
        assert!(message.contains(key), "{key}={value}: {message}");
    }

    for key in ["max_concurrency", "projected_usage", "token_cap"] {
        let input = format!("[runtime.workflow]\n{key} = 1\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("unknown workflow limit was accepted");
        assert!(format!("{error:#}").contains(key), "{key}: {error:#}");
    }

    #[cfg(target_pointer_width = "32")]
    {
        let (_temp, config_path, project_dir) =
            temp_project_config("[runtime.workflow]\nlua_vm_memory_bytes = 4294967296\n");
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("platform-sized workflow limit was accepted");
        assert!(
            format!("{error:#}").contains("lua_vm_memory_bytes"),
            "{error:#}"
        );
    }
}

#[test]
fn workflow_live_state_keeps_runtime_limits_consistent() {
    let (_current_temp, current_path, current_project) =
        temp_project_config("[runtime.workflow]\nswarm_concurrency = 7\nmax_active_workers = 2\n");
    let current = load_config(current_path, current_project);
    let (_next_temp, next_path, next_project) =
        temp_project_config("[runtime.workflow]\nswarm_concurrency = 9\nmax_active_workers = 5\n");
    let mut next = load_config(next_path, next_project);

    next.inherit_live_state(&current);

    assert_eq!(
        next.workflow_runtime.limits(),
        current.workflow_runtime.limits()
    );
    assert_eq!(next.runtime.workflow, current.workflow_runtime.limits());
}
