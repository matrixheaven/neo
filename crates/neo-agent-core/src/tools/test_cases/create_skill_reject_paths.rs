use super::*;
use crate::ToolContext;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[tokio::test]
async fn create_skill_rejects_resource_path_escape() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "references/../escaped.md",
                        "content": "escaped"
                    }
                ]
            }),
        )
        .await
        .expect_err("resource path escapes must fail");

    assert!(error.to_string().contains("invalid resource path"));
    assert!(
        !temp
            .path()
            .join("skills")
            .join("bad-resource")
            .join("escaped.md")
            .exists()
    );
    assert!(!temp.path().join("skills").join("bad-resource").exists());
}

#[tokio::test]
async fn create_skill_rejects_resource_outside_canonical_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "docs/guide.md",
                        "content": "guide"
                    }
                ]
            }),
        )
        .await
        .expect_err("unsupported resource dir must fail");

    assert!(error.to_string().contains("references, scripts, or assets"));
}

#[tokio::test]
async fn create_skill_rejects_absolute_resource_path() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let absolute_resource_path = outside.path().join("guide.md");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": absolute_resource_path.to_string_lossy(),
                        "content": "guide"
                    }
                ]
            }),
        )
        .await
        .expect_err("absolute resource path must fail");

    assert!(error.to_string().contains("invalid resource path"));
    assert!(!absolute_resource_path.exists());
}

#[tokio::test]
async fn create_skill_rejects_skill_md_as_resource() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "bad-resource",
                "description": "Bad resource",
                "body": "# Bad",
                "resources": [
                    {
                        "path": "references/SKILL.md",
                        "content": "not a nested skill"
                    }
                ]
            }),
        )
        .await
        .expect_err("SKILL.md resources must fail");

    assert!(error.to_string().contains("SKILL.md"));
}

#[tokio::test]
async fn create_skill_rejects_windows_hostile_resource_path_components() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    for path in [
        "references/bad:name.md",
        "references/bad\tname.md",
        "references/trailing-space.md ",
        "references/trailing-space /guide.md",
    ] {
        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "bad-resource",
                    "description": "Bad resource",
                    "body": "# Bad",
                    "resources": [
                        {
                            "path": path,
                            "content": "bad"
                        }
                    ]
                }),
            )
            .await
            .expect_err("Windows-hostile resource path must fail");

        assert!(
            error.to_string().contains("invalid resource path"),
            "{path}: {error}"
        );
    }
    assert!(!temp.path().join("skills").join("bad-resource").exists());
}

#[tokio::test]
async fn create_skill_rejects_path_like_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "../escaped",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("path-like names should be invalid input");

    assert!(error.to_string().contains("invalid skill name"));
    assert!(
        !temp.path().join("escaped").exists(),
        "invalid skill name must not write outside the skills directory"
    );
}

#[tokio::test]
async fn create_skill_rejects_windows_reserved_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "con",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("reserved names should be invalid input");

    assert!(error.to_string().contains("reserved Windows device name"));
}

#[tokio::test]
async fn create_skill_rejects_invalid_host_metadata_without_side_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let interface_only = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "interface-only",
                "description": "Interface metadata only",
                "body": "# Interface Only",
                "host_metadata": {
                    "interface": { "display_name": "Interface Only" }
                }
            }),
        )
        .await
        .expect("interface-only metadata should be valid");
    assert!(!interface_only.is_error);

    for (name, host_metadata) in [
        ("empty-metadata", json!({})),
        (
            "multiline-dependency",
            json!({
                "dependencies": [{ "type": "mcp", "value": "bad\nvalue" }]
            }),
        ),
    ] {
        let skill_dir = temp.path().join("skills").join(name);
        fs::create_dir_all(&skill_dir).await.expect("mkdir skill");
        let skill_file = skill_dir.join("SKILL.md");
        fs::write(&skill_file, "original")
            .await
            .expect("write original skill");

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": name,
                    "description": "Rejected metadata",
                    "body": "# Replacement",
                    "host_metadata": host_metadata
                }),
            )
            .await
            .expect_err("invalid metadata should be rejected");
        assert!(
            error.to_string().contains("host_metadata"),
            "error should identify host metadata: {error}"
        );
        assert_eq!(
            fs::read_to_string(&skill_file)
                .await
                .expect("read original skill"),
            "original"
        );
    }

    let legacy_dir = temp.path().join("skills").join("legacy-input");
    fs::create_dir_all(&legacy_dir)
        .await
        .expect("mkdir legacy skill");
    let legacy_file = legacy_dir.join("SKILL.md");
    fs::write(&legacy_file, "original")
        .await
        .expect("write legacy skill");
    let mut legacy_input = json!({
        "name": "legacy-input",
        "description": "Legacy input",
        "body": "# Replacement"
    });
    let retired_field = ["skill", "_type"].concat();
    legacy_input
        .as_object_mut()
        .expect("object input")
        .insert(retired_field.clone(), json!("prompt"));
    let error = tool
        .execute(&make_ctx(), legacy_input)
        .await
        .expect_err("retired CreateSkill field should be rejected");
    assert!(error.to_string().contains(&retired_field), "{error}");
    assert_eq!(
        fs::read_to_string(&legacy_file)
            .await
            .expect("read legacy skill"),
        "original"
    );
    assert!(
        !temp.path().join("backups").exists(),
        "rejected metadata must not create backups"
    );
}

#[tokio::test]
async fn create_skill_rejects_non_file_sidecar_before_overwriting_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("blocked-sidecar");
    let skill_file = skill_dir.join("SKILL.md");
    fs::create_dir_all(skill_dir.join("agents").join("neo.yaml"))
        .await
        .expect("create directory at sidecar target");
    fs::write(&skill_file, "original")
        .await
        .expect("write original skill");

    let error = CreateSkillTool::new(temp.path())
        .execute(
            &make_ctx(),
            json!({
                "name": "blocked-sidecar",
                "description": "Blocked sidecar",
                "body": "# Replacement",
                "host_metadata": {
                    "interface": { "display_name": "Blocked Sidecar" }
                }
            }),
        )
        .await
        .expect_err("directory sidecar target should be rejected");

    assert!(
        error
            .to_string()
            .contains("non-regular host metadata target"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(&skill_file)
            .await
            .expect("read original skill"),
        "original"
    );
    assert!(
        !temp.path().join("backups").exists(),
        "preflight failure must happen before backup"
    );
}
