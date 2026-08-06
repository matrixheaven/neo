use super::*;
use crate::ToolContext;
use crate::skills::SkillStore;
use crate::skills::SkillStoreHandle;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[tokio::test]
async fn create_skill_backs_up_existing_resource_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    fs::write(skill_dir.join("references").join("old.md"), "old reference")
        .await
        .expect("write old reference");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated",
                "resources": [
                    {
                        "path": "references/new.md",
                        "content": "new reference"
                    }
                ]
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("old.md"))
            .await
            .expect("read preserved resource"),
        "old reference"
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("new.md"))
            .await
            .expect("read new resource"),
        "new reference"
    );

    let backup_root = temp.path().join("backups").join("skills");
    let backup_skill = std::fs::read_dir(&backup_root)
        .expect("read backups")
        .map(|entry| entry.expect("backup entry").path().join("existing-skill"))
        .find(|path| path.join("SKILL.md").is_file())
        .expect("backup skill dir");
    assert_eq!(
        fs::read_to_string(backup_skill.join("SKILL.md"))
            .await
            .expect("read backup skill"),
        "old skill"
    );
    assert_eq!(
        fs::read_to_string(backup_skill.join("references").join("old.md"))
            .await
            .expect("read backup resource"),
        "old reference"
    );
}

#[tokio::test]
async fn create_skill_rejects_resource_directory_target_before_overwriting_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(skill_dir.join("references").join("guide.md"))
        .await
        .expect("mkdir resource target");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated",
                "resources": [
                    {
                        "path": "references/guide.md",
                        "content": "new reference"
                    }
                ]
            }),
        )
        .await
        .expect_err("directory resource target should fail");

    assert!(error.to_string().contains("resource target is a directory"));
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md"))
            .await
            .expect("read original skill"),
        "old skill"
    );
}

#[tokio::test]
async fn create_skill_rejects_conflicting_resource_paths_before_overwriting_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated",
                "resources": [
                    {
                        "path": "references/foo",
                        "content": "file"
                    },
                    {
                        "path": "references/foo/bar.md",
                        "content": "nested file"
                    }
                ]
            }),
        )
        .await
        .expect_err("conflicting resource paths should fail");

    assert!(
        error
            .to_string()
            .contains("conflicts with another resource")
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md"))
            .await
            .expect("read original skill"),
        "old skill"
    );
    assert!(
        !skill_dir.join("references").exists(),
        "validation should fail before writing resources"
    );
}

#[tokio::test]
async fn create_skill_rejects_case_insensitive_resource_path_conflicts() {
    for (skill_name, first_path, second_path, expected_message) in [
        (
            "case-duplicate",
            "references/Guide.md",
            "references/guide.md",
            "duplicates another resource",
        ),
        (
            "case-ancestor",
            "references/Foo",
            "references/foo/bar.md",
            "conflicts with another resource",
        ),
    ] {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join("skills").join(skill_name);
        fs::create_dir_all(&skill_dir)
            .await
            .expect("mkdir skill dir");
        fs::write(skill_dir.join("SKILL.md"), "old skill")
            .await
            .expect("write old skill");
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": skill_name,
                    "description": "Updated skill",
                    "body": "# Updated",
                    "resources": [
                        {
                            "path": first_path,
                            "content": "first"
                        },
                        {
                            "path": second_path,
                            "content": "second"
                        }
                    ]
                }),
            )
            .await
            .expect_err("case-insensitive resource path conflict should fail");

        assert!(
            error.to_string().contains(expected_message),
            "{skill_name}: {error}"
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md"))
                .await
                .expect("read original skill"),
            "old skill"
        );
        assert!(
            !skill_dir.join("references").exists(),
            "validation should fail before writing resources"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_resource_target_before_overwriting_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    let resource_dir = skill_dir.join("references");
    fs::create_dir_all(&resource_dir)
        .await
        .expect("mkdir references");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    let outside_file = outside.path().join("guide.md");
    fs::write(&outside_file, "outside")
        .await
        .expect("write outside");
    std::os::unix::fs::symlink(&outside_file, resource_dir.join("guide.md"))
        .expect("symlink resource target");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated",
                "resources": [
                    {
                        "path": "references/guide.md",
                        "content": "new reference"
                    }
                ]
            }),
        )
        .await
        .expect_err("symlinked resource target should fail");

    assert!(error.to_string().contains("symlinked file"));
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md"))
            .await
            .expect("read original skill"),
        "old skill"
    );
    assert_eq!(
        fs::read_to_string(outside_file)
            .await
            .expect("read outside file"),
        "outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_backup_preserves_executable_resource_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    let script_path = skill_dir.join("scripts").join("check.py");
    fs::create_dir_all(script_path.parent().expect("script parent"))
        .await
        .expect("mkdir scripts");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    fs::write(&script_path, "print('old')\n")
        .await
        .expect("write script");
    let mut permissions = stdfs::metadata(&script_path)
        .expect("script metadata")
        .permissions();
    permissions.set_mode(permissions.mode() | 0o100);
    stdfs::set_permissions(&script_path, permissions).expect("chmod script");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let backup_root = temp.path().join("backups").join("skills");
    let backup_script = std::fs::read_dir(&backup_root)
        .expect("read backups")
        .map(|entry| {
            entry
                .expect("backup entry")
                .path()
                .join("existing-skill")
                .join("scripts")
                .join("check.py")
        })
        .find(|path| path.is_file())
        .expect("backup script");
    let mode = stdfs::metadata(backup_script)
        .expect("backup script metadata")
        .permissions()
        .mode();
    assert_ne!(
        mode & 0o100,
        0,
        "backup should preserve owner executable bit"
    );
}

#[tokio::test]
async fn create_skill_preserves_unmentioned_existing_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::write(skill_dir.join("SKILL.md"), "old skill")
        .await
        .expect("write old skill");
    fs::write(skill_dir.join("references").join("keep.md"), "keep me")
        .await
        .expect("write kept reference");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# Updated"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("keep.md"))
            .await
            .expect("read kept reference"),
        "keep me"
    );
}

#[tokio::test]
async fn create_skill_creates_unique_backup_directories_for_rapid_overwrites() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    fs::write(skill_dir.join("SKILL.md"), "old content")
        .await
        .expect("write old skill");
    let tool = CreateSkillTool::new(temp.path());

    let first = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "First update",
                "body": "# First"
            }),
        )
        .await
        .expect("first execute");
    assert!(!first.is_error);

    let second = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Second update",
                "body": "# Second"
            }),
        )
        .await
        .expect("second execute");
    assert!(!second.is_error);

    let backup_root = temp.path().join("backups").join("skills");
    let mut backup_contents = Vec::new();
    for entry in std::fs::read_dir(&backup_root).expect("read backup root") {
        let backup_skill = entry.expect("backup entry").path().join("existing-skill");
        if backup_skill.join("SKILL.md").is_file() {
            backup_contents.push(
                fs::read_to_string(backup_skill.join("SKILL.md"))
                    .await
                    .expect("read backup skill"),
            );
        }
    }

    assert_eq!(
        backup_contents.len(),
        2,
        "rapid overwrites should create distinct backup directories"
    );
    assert!(
        backup_contents
            .iter()
            .any(|content| content == "old content")
    );
    assert!(
        backup_contents
            .iter()
            .any(|content| content.contains("# First"))
    );
}

#[tokio::test]
async fn create_skill_reloads_shared_skill_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let user_skills = temp.path().join("skills");
    let handle = SkillStoreHandle::new(SkillStore::load(
        std::slice::from_ref(&user_skills),
        &[],
        Vec::new(),
    ));
    let reload_root = user_skills.clone();
    let tool =
        CreateSkillTool::new(temp.path()).with_skill_store_reload(handle.clone(), move || {
            Ok(SkillStore::load(
                std::slice::from_ref(&reload_root),
                &[],
                Vec::new(),
            ))
        });

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "fresh-skill",
                "description": "Freshly available",
                "body": "# Fresh\n\nUse me now."
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert!(
        handle.get("fresh-skill").is_some(),
        "created skill should be immediately visible through the shared store"
    );
    assert!(
        result.content.contains("Skill store reloaded"),
        "tool result should tell the model the reload happened: {}",
        result.content
    );
}

#[tokio::test]
async fn create_skill_reports_durable_write_when_reload_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path())
        .with_skill_store_reload(SkillStoreHandle::default(), || {
            Err("reload unavailable".to_owned())
        });

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "written-not-reloaded",
                "description": "Durable write reporting",
                "body": "# Written"
            }),
        )
        .await
        .expect("reload failure should be returned as a tool result");

    assert!(result.is_error);
    for expected in [
        "Created skill at",
        "Backup: none",
        "Resources: none",
        "Host metadata: not present",
        "reload unavailable",
        "package files were written",
        "active skill store was not updated",
    ] {
        assert!(result.content.contains(expected), "{}", result.content);
    }
    assert!(
        temp.path()
            .join("skills/written-not-reloaded/SKILL.md")
            .is_file()
    );
}
