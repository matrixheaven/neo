//! Test-path shim so plan verification can address
//! `modes::config::tests::workflow_machine_limits_map_all_v2_fields`.
//! Production config ownership remains `crate::config`.

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::TempDir;

    use crate::config::{AppConfig, ConfigOverrides};

    fn temp_project_config(content: &str) -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        fs::write(&config_path, content).expect("write config");
        let project_dir = temp.path().join("project");
        fs::create_dir_all(&project_dir).expect("create project");
        (temp, config_path, project_dir)
    }

    fn load_config(config_path: PathBuf, project_dir: PathBuf) -> AppConfig {
        AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect("load config")
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
}
