use super::*;
use crate::ToolContext;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_directory_in_skill_or_backup_paths() {
    struct Case {
        name: &'static str,
        /// Creates the on-disk state (including the symlink) and returns the
        /// input payload for the rejected invocation.
        setup: Box<dyn FnOnce(&Path, &Path) -> serde_json::Value>,
        /// Case-specific assertions after the shared rejection check.
        extra: Box<dyn FnOnce(&Path, &Path)>,
    }

    let cases = vec![
        Case {
            name: "skill_directory_symlink",
            setup: Box::new(|temp, outside| {
                std::fs::create_dir_all(temp.join("skills")).expect("mkdir skills");
                std::os::unix::fs::symlink(outside, temp.join("skills").join("linked-skill"))
                    .expect("symlink skill dir");
                json!({"name": "linked-skill", "description": "A test skill", "body": "# Body"})
            }),
            extra: Box::new(|_, _| {}),
        },
        Case {
            name: "skills_root_symlink",
            setup: Box::new(|temp, outside| {
                std::os::unix::fs::symlink(outside, temp.join("skills"))
                    .expect("symlink skills root");
                json!({"name": "new-skill", "description": "A test skill", "body": "# Body"})
            }),
            extra: Box::new(|_, _| {}),
        },
        Case {
            name: "backup_parent_symlink",
            setup: Box::new(|temp, outside| {
                let skill_dir = temp.join("skills").join("safe-skill");
                std::fs::create_dir_all(&skill_dir).expect("mkdir skill dir");
                std::fs::write(skill_dir.join("SKILL.md"), "old content").expect("write old skill");
                std::os::unix::fs::symlink(outside, temp.join("backups"))
                    .expect("symlink backup parent");
                json!({"name": "safe-skill", "description": "A test skill", "body": "# Body"})
            }),
            extra: Box::new(|temp, outside| {
                assert!(
                    !outside.join("skills").exists(),
                    "backup must not follow a symlinked backup parent"
                );
                assert_eq!(
                    std::fs::read_to_string(
                        temp.join("skills").join("safe-skill").join("SKILL.md")
                    )
                    .expect("read original skill"),
                    "old content"
                );
            }),
        },
    ];

    for case in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let input = (case.setup)(temp.path(), outside.path());
        let error = CreateSkillTool::new(temp.path())
            .execute(&make_ctx(), input)
            .await
            .expect_err("symlinked directory should be rejected");

        assert!(
            error.to_string().contains("symlinked directory"),
            "{}: {error}",
            case.name
        );
        (case.extra)(temp.path(), outside.path());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_or_dangling_skill_file() {
    struct Case {
        name: &'static str,
        /// Creates the on-disk state (including the symlink) and returns the
        /// input payload for the rejected invocation.
        setup: Box<dyn FnOnce(&Path, &Path) -> serde_json::Value>,
        /// Case-specific assertions after the shared rejection check.
        extra: Box<dyn FnOnce(&Path, &Path)>,
    }

    let cases = vec![
        Case {
            name: "symlinked_skill_file",
            setup: Box::new(|temp, outside| {
                let outside_file = outside.join("SKILL.md");
                std::fs::write(&outside_file, "outside").expect("write outside");
                let skill_dir = temp.join("skills").join("safe-skill");
                std::fs::create_dir_all(&skill_dir).expect("mkdir skill dir");
                std::os::unix::fs::symlink(&outside_file, skill_dir.join("SKILL.md"))
                    .expect("symlink skill file");
                json!({"name": "safe-skill", "description": "A test skill", "body": "# Body"})
            }),
            extra: Box::new(|_, outside| {
                assert_eq!(
                    std::fs::read_to_string(outside.join("SKILL.md")).expect("read outside"),
                    "outside"
                );
            }),
        },
        Case {
            name: "dangling_symlinked_skill_file",
            setup: Box::new(|temp, _outside| {
                let skill_dir = temp.join("skills").join("safe-skill");
                std::fs::create_dir_all(&skill_dir).expect("mkdir skill dir");
                std::os::unix::fs::symlink(temp.join("missing.md"), skill_dir.join("SKILL.md"))
                    .expect("symlink dangling skill file");
                json!({"name": "safe-skill", "description": "A test skill", "body": "# Body"})
            }),
            extra: Box::new(|_, _| {}),
        },
    ];

    for case in cases {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let input = (case.setup)(temp.path(), outside.path());
        let error = CreateSkillTool::new(temp.path())
            .execute(&make_ctx(), input)
            .await
            .expect_err("symlinked skill file should be rejected");

        assert!(
            error.to_string().contains("symlinked file"),
            "{}: {error}",
            case.name
        );
        (case.extra)(temp.path(), outside.path());
    }
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn create_skill_rejects_symlinked_sidecar_before_overwriting_skill() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let skill_dir = temp.path().join("skills").join("linked-sidecar");
    let skill_file = skill_dir.join("SKILL.md");
    let agents_dir = skill_dir.join("agents");
    fs::create_dir_all(&agents_dir).await.expect("mkdir agents");
    fs::write(&skill_file, "original")
        .await
        .expect("write original skill");
    let outside_sidecar = outside.path().join("neo.yaml");
    fs::write(&outside_sidecar, "outside")
        .await
        .expect("write outside sidecar");
    create_file_symlink(&outside_sidecar, &agents_dir.join("neo.yaml"));

    let error = CreateSkillTool::new(temp.path())
        .execute(
            &make_ctx(),
            json!({
                "name": "linked-sidecar",
                "description": "Linked sidecar",
                "body": "# Replacement",
                "host_metadata": {
                    "interface": { "display_name": "Linked Sidecar" }
                }
            }),
        )
        .await
        .expect_err("symlinked sidecar target should be rejected");

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
    assert_eq!(
        fs::read_to_string(&outside_sidecar)
            .await
            .expect("read outside sidecar"),
        "outside"
    );
    assert!(!temp.path().join("backups").exists());
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("symlink sidecar");
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) {
    std::os::windows::fs::symlink_file(target, link).expect("symlink sidecar");
}
