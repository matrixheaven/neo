use std::{fs, path::Path};

use neo_extensions::{ExtensionDiscovery, ExtensionInstaller, ExtensionStatus};

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
