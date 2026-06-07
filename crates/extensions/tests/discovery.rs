use std::{fs, path::Path};

use neo_extensions::{ExtensionDiscovery, ExtensionManifest, ExtensionTransport};

#[test]
fn discovers_manifests_in_deterministic_order() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("zeta/neo-extension.toml"),
        r#"
id = "zeta"
name = "Zeta"
version = "0.1.0"

[runner]
command = "zeta-bin"
args = ["--serve"]
"#,
    );
    write(
        dir.path().join("alpha/neo-extension.toml"),
        r#"
id = "alpha"
name = "Alpha"
version = "1.0.0"
description = "Alpha extension"

[runner]
command = "alpha-bin"
"#,
    );

    let discovered = ExtensionDiscovery::new(dir.path()).discover().unwrap();

    assert_eq!(
        discovered
            .iter()
            .map(|item| item.manifest.id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "zeta"]
    );
    assert_eq!(discovered[0].manifest.name, "Alpha");
    assert_eq!(
        discovered[1].manifest,
        ExtensionManifest {
            id: "zeta".into(),
            name: "Zeta".into(),
            version: "0.1.0".into(),
            description: None,
            transport: ExtensionTransport::Stdio {
                command: "zeta-bin".into(),
                args: vec!["--serve".into()],
                env: vec![],
            },
        }
    );
}

#[test]
fn rejects_duplicate_extension_ids() {
    let dir = tempfile::tempdir().unwrap();
    write_manifest(dir.path().join("one/neo-extension.toml"), "same", "One");
    write_manifest(dir.path().join("two/neo-extension.toml"), "same", "Two");

    let err = ExtensionDiscovery::new(dir.path()).discover().unwrap_err();

    assert!(err.to_string().contains("duplicate extension id"));
}

fn write_manifest(path: impl AsRef<Path>, id: &str, name: &str) {
    write(
        path,
        &format!(
            r#"
id = "{id}"
name = "{name}"
version = "1.0.0"

[runner]
command = "extension"
"#
        ),
    );
}

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
