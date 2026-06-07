use std::{fs, path::Path};

use neo_extensions::{
    ExtensionDiscovery, ExtensionInstallError, ExtensionInstaller, ExtensionStatus,
};

#[test]
fn install_from_local_directory_persists_source_and_update_preserves_status() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    write_manifest(&source, "echo", "0.1.0");

    let root = dir.path().join(".neo/extensions");
    let state_path = dir.path().join(".neo/extensions-state.toml");
    let registry_path = dir.path().join(".neo/extensions-sources.toml");
    let installer = ExtensionInstaller::new(&root, &state_path, &registry_path);

    let initial_install = installer.install(&source).unwrap();
    assert_eq!(initial_install.manifest.id, "echo");
    assert_eq!(initial_install.manifest.version, "0.1.0");
    assert_eq!(initial_install.source, source.display().to_string());
    assert_eq!(
        initial_install.manifest_path,
        root.join("echo/neo-extension.toml")
    );

    installer.lifecycle().disable(&root, "echo").unwrap();
    write_manifest(&source, "echo", "0.2.0");

    let updated_install = installer.update("echo").unwrap();
    assert_eq!(updated_install.manifest.version, "0.2.0");
    assert_eq!(updated_install.source, source.display().to_string());

    let status = installer.lifecycle().status(&root, "echo").unwrap();
    assert_eq!(status.status, ExtensionStatus::Disabled);

    let discovered = ExtensionDiscovery::new(&root).discover().unwrap();
    assert_eq!(discovered[0].manifest.version, "0.2.0");

    let registry = fs::read_to_string(registry_path).unwrap();
    assert!(registry.contains("[extensions.echo.source]"));
    assert!(registry.contains("type = \"local_path\""));
    assert!(registry.contains(source.to_string_lossy().as_ref()));
}

#[test]
fn installation_uninstall_removes_installed_directory_and_source_registry_entry() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    write_manifest(&source, "echo", "0.1.0");

    let root = dir.path().join(".neo/extensions");
    let state_path = dir.path().join(".neo/extensions-state.toml");
    let registry_path = dir.path().join(".neo/extensions-sources.toml");
    let installer = ExtensionInstaller::new(&root, &state_path, &registry_path);

    installer.install(&source).unwrap();
    assert!(root.join("echo/neo-extension.toml").exists());

    let uninstalled = installer.uninstall("echo").unwrap();

    assert_eq!(uninstalled.id, "echo");
    assert_eq!(uninstalled.root, root.join("echo"));
    assert!(!root.join("echo").exists());

    let discovered = ExtensionDiscovery::new(&root).discover().unwrap();
    assert!(discovered.is_empty());

    let registry = fs::read_to_string(registry_path).unwrap();
    assert!(!registry.contains("[extensions.echo"));
    assert!(!registry.contains(source.to_string_lossy().as_ref()));
}

#[test]
fn installation_uninstall_rejects_uninstalled_extension_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".neo/extensions");
    let state_path = dir.path().join(".neo/extensions-state.toml");
    let registry_path = dir.path().join(".neo/extensions-sources.toml");
    let installer = ExtensionInstaller::new(&root, &state_path, &registry_path);

    let error = installer.uninstall("missing").unwrap_err();

    assert!(matches!(
        error,
        ExtensionInstallError::NotInstalled { id } if id == "missing"
    ));
}

#[test]
fn installation_uninstall_does_not_remove_paths_outside_extension_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("extensions");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("sentinel"), "keep").unwrap();

    let state_path = dir.path().join(".neo/extensions-state.toml");
    let registry_path = dir.path().join(".neo/extensions-sources.toml");
    let installer = ExtensionInstaller::new(&root, &state_path, &registry_path);

    let error = installer.uninstall("../outside").unwrap_err();

    assert!(matches!(
        error,
        ExtensionInstallError::OutsideExtensionRoot { id, .. } if id == "../outside"
    ));
    assert!(outside.join("sentinel").exists());
}

fn write_manifest(root: &Path, id: &str, version: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(
        root.join("neo-extension.toml"),
        format!(
            r#"
id = "{id}"
name = "Echo"
version = "{version}"

[runner]
command = "python3"
"#
        ),
    )
    .unwrap();
}
