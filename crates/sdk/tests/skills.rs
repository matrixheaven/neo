use std::{fs, path::Path};

use neo_sdk::{ResourceKind, SkillLoadOptions, SkillManifest, SkillResource, load_skill};
use serde_json::json;

#[test]
fn load_skill_parses_frontmatter_body_and_resources() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r#"---
name = "reviewer"
description = "Review repository changes"
version = "1.2.0"
entrypoint = "SKILL.md"
resources = [
  { path = "references/policy.md", kind = "text" },
  { path = "scripts/check.sh", kind = "executable" },
]
---

# Reviewer

Use focused findings.
"#,
    );
    write(
        dir.path().join("references/policy.md"),
        "Do not invent issues.\n",
    );
    write(dir.path().join("scripts/check.sh"), "#!/bin/sh\nexit 0\n");

    let skill = load_skill(dir.path(), SkillLoadOptions::default()).unwrap();

    assert_eq!(
        skill.manifest,
        SkillManifest {
            name: "reviewer".into(),
            description: "Review repository changes".into(),
            version: Some("1.2.0".into()),
            entrypoint: "SKILL.md".into(),
            resources: vec![
                SkillResource {
                    path: "references/policy.md".into(),
                    kind: ResourceKind::Text,
                },
                SkillResource {
                    path: "scripts/check.sh".into(),
                    kind: ResourceKind::Executable,
                },
            ],
        }
    );
    assert!(skill.body.contains("Use focused findings."));
    assert_eq!(
        skill.resources[0].content.as_text().unwrap(),
        "Do not invent issues.\n"
    );
    assert_eq!(skill.root, dir.path());
}

#[test]
fn load_skill_rejects_resource_escape() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r#"---
name = "escape"
description = "bad"
resources = [{ path = "../outside.txt", kind = "text" }]
---
body
"#,
    );

    let err = load_skill(dir.path(), SkillLoadOptions::default()).unwrap_err();

    assert!(err.to_string().contains("escapes skill root"));
}

#[test]
fn load_skill_accepts_direct_skill_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join("SKILL.md");
    write(
        &skill_path,
        r#"---
name = "direct"
description = "Loaded from a direct file path"
---
Use this skill directly.
"#,
    );

    let skill = load_skill(&skill_path, SkillLoadOptions::default()).unwrap();

    assert_eq!(skill.root, dir.path());
    assert_eq!(skill.manifest.name, "direct");
    assert!(skill.body.contains("Use this skill directly."));
}

#[test]
fn load_skill_reports_missing_direct_skill_file_without_appending_skill_md() {
    let dir = tempfile::tempdir().unwrap();
    let skill_path = dir.path().join("missing").join("SKILL.md");

    let err = load_skill(&skill_path, SkillLoadOptions::default()).unwrap_err();

    assert!(
        err.to_string()
            .contains(skill_path.to_string_lossy().as_ref())
    );
    assert!(!err.to_string().contains("SKILL.md/SKILL.md"));
}

#[test]
fn load_skill_accepts_crlf_frontmatter_separators() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        "---\r\nname = \"crlf\"\r\ndescription = \"Windows line endings\"\r\n---\r\nBody\r\n",
    );

    let skill = load_skill(dir.path(), SkillLoadOptions::default()).unwrap();

    assert_eq!(skill.manifest.name, "crlf");
    assert_eq!(skill.body, "Body\r\n");
}

#[test]
fn skill_manifest_serializes_with_stable_shape() {
    let manifest = SkillManifest {
        name: "shape".into(),
        description: "Stable SDK manifest".into(),
        version: None,
        entrypoint: "SKILL.md".into(),
        resources: vec![
            SkillResource {
                path: "references/guide.md".into(),
                kind: ResourceKind::Text,
            },
            SkillResource {
                path: "bin/tool".into(),
                kind: ResourceKind::Executable,
            },
        ],
    };

    let value = serde_json::to_value(&manifest).unwrap();

    assert_eq!(
        value,
        json!({
            "name": "shape",
            "description": "Stable SDK manifest",
            "entrypoint": "SKILL.md",
            "resources": [
                { "path": "references/guide.md", "kind": "text" },
                { "path": "bin/tool", "kind": "executable" }
            ]
        })
    );
}

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
