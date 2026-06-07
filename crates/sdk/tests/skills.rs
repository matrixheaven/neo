use std::{fs, path::Path};

use neo_sdk::{ResourceKind, SkillLoadOptions, SkillManifest, SkillResource, load_skill};

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

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
