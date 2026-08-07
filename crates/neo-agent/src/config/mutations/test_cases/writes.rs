//! writes behavior (moved from `mutations.rs`).

use super::*;
use std::{
    fs::{self, OpenOptions},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use neo_ai::ApiType;
use tempfile::TempDir;

use super::super::add_provider;
use crate::config::{
    ProviderConfig, config_process_lock_is_available, read_file_config, update_file_config,
    update_file_config_with_lock_hook, update_file_config_with_writer,
};

#[test]
fn concurrent_config_updates_preserve_both_mutations() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    let (first_mutation_started_tx, first_mutation_started_rx) = mpsc::sync_channel(0);
    let (release_first_tx, release_first_rx) = mpsc::sync_channel(0);
    let (start_second_tx, start_second_rx) = mpsc::sync_channel(0);
    let (second_attempting_tx, second_attempting_rx) = mpsc::sync_channel(0);

    std::thread::scope(|scope| {
        let first_path = &config_path;
        scope.spawn(move || {
            update_file_config(first_path, |config| {
                config.default_model = Some("model-a".to_owned());
                first_mutation_started_tx.send(()).unwrap();
                release_first_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        let second_path = &config_path;
        scope.spawn(move || {
            start_second_rx.recv().unwrap();
            second_attempting_tx.send(()).unwrap();
            update_file_config(second_path, |config| {
                config.default_provider = Some("provider-b".to_owned());
                Ok(())
            })
            .unwrap();
        });

        first_mutation_started_rx.recv().unwrap();
        start_second_tx.send(()).unwrap();
        second_attempting_rx.recv().unwrap();
        assert!(!config_process_lock_is_available(&config_path).unwrap());
        release_first_tx.send(()).unwrap();
    });

    let config = read_file_config(&config_path).unwrap();
    assert_eq!(config.default_model.as_deref(), Some("model-a"));
    assert_eq!(config.default_provider.as_deref(), Some("provider-b"));
}

#[test]
fn failed_atomic_replace_leaves_previous_config_parseable() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "default_model = \"original\"\n").unwrap();

    let result = update_file_config_with_writer(
        &config_path,
        |config| {
            config.default_model = Some("replacement".to_owned());
            Ok(())
        },
        |file, _content| {
            file.write_all(b"default_model = ")?;
            anyhow::bail!("injected writer failure")
        },
    );

    assert!(result.is_err());
    let config = read_file_config(&config_path).unwrap();
    assert_eq!(config.default_model.as_deref(), Some("original"));
}

#[test]
fn config_lock_helper() {
    let Some(lock_path) = std::env::var_os("NEO_CONFIG_LOCK_HELPER_LOCK") else {
        return;
    };
    let ready_path = std::env::var_os("NEO_CONFIG_LOCK_HELPER_READY").unwrap();
    let release_path = std::env::var_os("NEO_CONFIG_LOCK_HELPER_RELEASE").unwrap();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .unwrap();
    lock.lock().unwrap();
    fs::write(ready_path, b"ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&release_path).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn advisory_config_lock_blocks_external_writer() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "default_model = \"original\"\n").unwrap();
    let lock_path = temp.path().join("config.toml.lock");
    let ready_path = temp.path().join("lock-ready");
    let release_path = temp.path().join("lock-release");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "config::mutations::test_cases::writes::config_lock_helper",
            "--nocapture",
        ])
        .env("NEO_CONFIG_LOCK_HELPER_LOCK", &lock_path)
        .env("NEO_CONFIG_LOCK_HELPER_READY", &ready_path)
        .env("NEO_CONFIG_LOCK_HELPER_RELEASE", &release_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !ready_path.exists() {
        fs::write(&release_path, b"release").unwrap();
        let _ = child.wait();
        panic!("lock helper did not acquire the advisory lock");
    }

    let (completed_tx, completed_rx) = mpsc::sync_channel(0);
    let (at_lock_tx, at_lock_rx) = mpsc::sync_channel(0);
    let (attempt_lock_tx, attempt_lock_rx) = mpsc::sync_channel(0);
    let blocked = std::thread::scope(|scope| {
        let update_path = &config_path;
        scope.spawn(move || {
            update_file_config_with_lock_hook(
                update_path,
                || {
                    at_lock_tx.send(()).unwrap();
                    attempt_lock_rx.recv().unwrap();
                },
                |config| {
                    config.default_provider = Some("external-waited".to_owned());
                    Ok(())
                },
            )
            .unwrap();
            let _ = completed_tx.send(());
        });

        at_lock_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        attempt_lock_tx.send(()).unwrap();
        let blocked = completed_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err();
        fs::write(&release_path, b"release").unwrap();
        completed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        blocked
    });

    assert!(child.wait().unwrap().success());
    assert!(blocked, "config update bypassed the external advisory lock");
    let config = read_file_config(&config_path).unwrap();
    assert_eq!(config.default_provider.as_deref(), Some("external-waited"));
}

#[test]
fn first_config_write_includes_runtime_defaults() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join(".neo/config.toml");

    add_provider(
        &config_path,
        "openai",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            base_url: Some("https://api.openai.test/v1".to_owned()),
            api_key: None,
            api_key_env: Some("OPENAI_API_KEY".to_owned()),
        },
    )
    .expect("add provider");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("[runtime.retry]"));
    assert!(written.contains("max_retries = 5"));
    assert!(written.contains("first_event_timeout_secs = 60"));
    assert!(written.contains("stream_idle_timeout_secs = 120"));
    assert!(written.contains("[runtime.compaction]"));
    assert!(written.contains("enabled = true"));
    assert!(written.contains("keep_recent_messages = 20"));
}

#[test]
fn config_write_drops_legacy_reasoning_effort_and_keeps_structured_reasoning() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
[runtime]
reasoning_effort = "low"

[runtime.reasoning]
mode = "effort"
effort = "high"
"#,
    );

    add_provider(
        &config_path,
        "openai",
        ProviderConfig {
            display_name: None,
            provider_type: Some(ApiType::OpenAiResponse),
            base_url: Some("https://api.openai.test/v1".to_owned()),
            api_key: None,
            api_key_env: Some("OPENAI_API_KEY".to_owned()),
        },
    )
    .expect("add provider");

    let written = fs::read_to_string(config_path).expect("read config");
    let value: toml::Value = toml::from_str(&written).expect("parse written config");
    let runtime = value
        .get("runtime")
        .and_then(toml::Value::as_table)
        .expect("runtime table");
    assert!(!runtime.contains_key("reasoning_effort"));
    assert_eq!(
        runtime
            .get("reasoning")
            .and_then(toml::Value::as_table)
            .and_then(|reasoning| reasoning.get("mode"))
            .and_then(toml::Value::as_str),
        Some("effort")
    );
    assert_eq!(
        runtime
            .get("reasoning")
            .and_then(toml::Value::as_table)
            .and_then(|reasoning| reasoning.get("effort"))
            .and_then(toml::Value::as_str),
        Some("high")
    );
}

#[test]
fn set_startup_theme_persists_valid_id_through_update_file_config() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "default_model = \"keep\"\n").unwrap();

    super::super::set_startup_theme(&config_path, "night/solarized.json").unwrap();

    let config = read_file_config(&config_path).unwrap();
    let tui = config.tui.expect("tui table persisted");
    assert_eq!(
        tui.theme.as_deref(),
        Some("night/solarized.json"),
        "logical id persisted with forward slashes"
    );
    assert_eq!(config.default_model.as_deref(), Some("keep"));
}

#[test]
fn set_startup_theme_rejects_invalid_id_without_touching_config() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "default_model = \"keep\"\n").unwrap();

    for invalid in ["../escape.json", "/abs/theme.json", "a/../b.json"] {
        assert!(
            super::super::set_startup_theme(&config_path, invalid).is_err(),
            "accepted {invalid:?}"
        );
    }

    let config = read_file_config(&config_path).unwrap();
    assert_eq!(config.default_model.as_deref(), Some("keep"));
    assert!(
        config.tui.is_none(),
        "failed validation must not write config"
    );
}

#[test]
fn failed_startup_theme_write_leaves_previous_config_unchanged() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "default_model = \"original\"\n").unwrap();

    let result = update_file_config_with_writer(
        &config_path,
        |config| {
            config.tui.get_or_insert_default().theme = Some("draft.json".to_owned());
            Ok(())
        },
        |file, _content| {
            file.write_all(b"default_model = ")?;
            anyhow::bail!("injected writer failure")
        },
    );
    assert!(result.is_err());

    let config = read_file_config(&config_path).unwrap();
    assert_eq!(config.default_model.as_deref(), Some("original"));
    assert!(
        config.tui.is_none(),
        "failed write must leave the previous config unchanged"
    );
}
