use std::{fs, path::Path};

use neo_extensions::{
    ExtensionLifecycleStatus, ExtensionLifecycleStore, ExtensionStatus, LifecycleStateSource,
};

#[test]
fn lifecycle_defaults_discovered_extensions_to_enabled_without_writing_state() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path().join("extensions/echo/neo-extension.toml"),
        "echo",
        "Echo",
    );
    let state_path = dir.path().join(".neo/extensions-state.toml");
    let store = ExtensionLifecycleStore::new(&state_path);

    let statuses = store.statuses(dir.path().join("extensions")).unwrap();

    assert_eq!(
        statuses,
        vec![ExtensionLifecycleStatus {
            id: "echo".into(),
            name: "Echo".into(),
            version: "1.0.0".into(),
            manifest_path: dir.path().join("extensions/echo/neo-extension.toml"),
            status: ExtensionStatus::Enabled,
            source: LifecycleStateSource::Default,
        }]
    );
    assert!(
        !state_path.exists(),
        "reading default lifecycle status must not create fake state"
    );
}

#[test]
fn lifecycle_disable_and_enable_are_persisted_atomically() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path().join("extensions/echo/neo-extension.toml"),
        "echo",
        "Echo",
    );
    let root = dir.path().join("extensions");
    let state_path = dir.path().join(".neo/extensions-state.toml");
    let store = ExtensionLifecycleStore::new(&state_path);

    let disabled = store.disable(&root, "echo").unwrap();

    assert_eq!(disabled.status, ExtensionStatus::Disabled);
    assert_eq!(disabled.source, LifecycleStateSource::StateFile);
    let state = fs::read_to_string(&state_path).unwrap();
    assert!(state.contains("[extensions.echo]"));
    assert!(state.contains("enabled = false"));
    assert!(
        fs::read_dir(state_path.parent().unwrap())
            .unwrap()
            .all(|entry| entry.unwrap().path() != state_path.with_extension("toml.tmp")),
        "temporary state file should be cleaned up after commit"
    );

    let fresh_store = ExtensionLifecycleStore::new(&state_path);
    assert_eq!(
        fresh_store.status(&root, "echo").unwrap().status,
        ExtensionStatus::Disabled
    );

    let enabled = fresh_store.enable(&root, "echo").unwrap();
    assert_eq!(enabled.status, ExtensionStatus::Enabled);
    assert_eq!(enabled.source, LifecycleStateSource::StateFile);
    assert!(
        fs::read_to_string(&state_path)
            .unwrap()
            .contains("enabled = true")
    );
}

#[test]
fn lifecycle_rejects_unknown_extension_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path().join("extensions/echo/neo-extension.toml"),
        "echo",
        "Echo",
    );
    let store = ExtensionLifecycleStore::new(dir.path().join(".neo/extensions-state.toml"));

    let err = store
        .status(dir.path().join("extensions"), "missing")
        .unwrap_err();

    assert!(err.to_string().contains("extension \"missing\" not found"));
}

#[test]
fn lifecycle_reports_disabled_extensions_as_not_callable() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(
        dir.path().join("extensions/echo/neo-extension.toml"),
        "echo",
        "Echo",
    );
    let root = dir.path().join("extensions");
    let store = ExtensionLifecycleStore::new(dir.path().join(".neo/extensions-state.toml"));
    store.disable(&root, "echo").unwrap();

    let err = store.ensure_enabled(&root, "echo").unwrap_err();

    assert!(err.to_string().contains("extension \"echo\" is disabled"));
}

fn write_manifest(path: impl AsRef<Path>, id: &str, name: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            r#"
id = "{id}"
name = "{name}"
version = "1.0.0"

[runner]
command = "extension"
"#
        ),
    )
    .unwrap();
}
