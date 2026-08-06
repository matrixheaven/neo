use super::*;
use crate::ToolContext;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_skill_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let skills_dir = temp.path().join("skills");
    fs::create_dir_all(&skills_dir).await.expect("mkdir skills");
    std::os::unix::fs::symlink(outside.path(), skills_dir.join("linked-skill"))
        .expect("symlink skill dir");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "linked-skill",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("symlinked skill directories should be invalid input");

    assert!(error.to_string().contains("symlinked directory"));
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_skills_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    std::os::unix::fs::symlink(outside.path(), temp.path().join("skills"))
        .expect("symlink skills root");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "new-skill",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("symlinked skills root should be invalid input");

    assert!(error.to_string().contains("symlinked directory"));
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_backup_parent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let skill_dir = temp.path().join("skills").join("safe-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    fs::write(skill_dir.join("SKILL.md"), "old content")
        .await
        .expect("write old skill");
    std::os::unix::fs::symlink(outside.path(), temp.path().join("backups"))
        .expect("symlink backup parent");
    let tool = CreateSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "safe-skill",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("symlinked backup parent should be invalid input");

    assert!(error.to_string().contains("symlinked directory"));
    assert!(
        !outside.path().join("skills").exists(),
        "backup must not follow a symlinked backup parent"
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("SKILL.md"))
            .await
            .expect("read original skill"),
        "old content"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_symlinked_skill_file_without_following_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_file = outside.path().join("SKILL.md");
    std::fs::write(&outside_file, "outside").expect("write outside");
    let skill_dir = temp.path().join("skills").join("safe-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    std::os::unix::fs::symlink(&outside_file, skill_dir.join("SKILL.md"))
        .expect("symlink skill file");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "safe-skill",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("symlinked skill file should be invalid input");

    assert!(error.to_string().contains("symlinked file"));
    assert_eq!(
        std::fs::read_to_string(outside_file).expect("read outside"),
        "outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn create_skill_rejects_dangling_symlinked_skill_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("safe-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    std::os::unix::fs::symlink(temp.path().join("missing.md"), skill_dir.join("SKILL.md"))
        .expect("symlink dangling skill file");
    let tool = CreateSkillTool::new(temp.path());
    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "safe-skill",
                "description": "A test skill",
                "body": "# Body"
            }),
        )
        .await
        .expect_err("dangling symlinked skill file should be invalid input");

    assert!(error.to_string().contains("symlinked file"));
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
