use super::*;
use crate::ToolContext;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[tokio::test]
async fn move_skill_moves_directory_without_losing_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("skills").join("to-move");
    fs::create_dir_all(&source).await.expect("mkdir");
    let original = "---\nname: to-move\ndescription: test\n---\n\nskill content\n";
    fs::write(source.join("SKILL.md"), original)
        .await
        .expect("write");

    let dest_parent = temp.path().join("bundles");
    let tool = MoveSkillTool::new(temp.path());
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "source": source.to_str().unwrap(),
                "destination_parent": dest_parent.to_str().unwrap()
            }),
        )
        .await
        .expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("Moved"));
    let moved_path = dest_parent.join("to-move").join("SKILL.md");
    assert_eq!(
        fs::read_to_string(&moved_path).await.expect("read moved"),
        original
    );

    let backup_line = result
        .content
        .lines()
        .find_map(|line| line.strip_prefix("Backup: "))
        .expect("backup line");
    let backup_target = PathBuf::from(backup_line);
    assert!(
        backup_target.starts_with(temp.path().join("backups").join("skills")),
        "backup should live under ~/.neo/backups/skills equivalent, got {}",
        backup_target.display()
    );
    assert_eq!(
        fs::read_to_string(backup_target.join("SKILL.md"))
            .await
            .expect("read backup"),
        original
    );
    assert!(!source.exists(), "source directory should have been moved");
}

#[tokio::test]
async fn move_skill_rejects_existing_destination_without_side_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let source = temp.path().join("skills").join("to-move");
    fs::create_dir_all(&source).await.expect("mkdir source");
    fs::write(source.join("SKILL.md"), "original")
        .await
        .expect("write source");
    let dest_parent = temp.path().join("bundles");
    let destination = dest_parent.join("to-move");
    fs::create_dir_all(&destination)
        .await
        .expect("mkdir destination");
    fs::write(destination.join("SKILL.md"), "existing")
        .await
        .expect("write destination");

    let tool = MoveSkillTool::new(temp.path());
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "source": source.to_str().unwrap(),
                "destination_parent": dest_parent.to_str().unwrap()
            }),
        )
        .await
        .expect("execute");

    assert!(result.is_error);
    assert_eq!(
        fs::read_to_string(source.join("SKILL.md"))
            .await
            .expect("read source"),
        "original"
    );
    assert!(
        !temp.path().join("backups").exists(),
        "rejected move must not create a backup"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn move_skill_rejects_symlinked_source_directory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_skill = outside.path().join("linked-skill");
    fs::create_dir_all(&outside_skill)
        .await
        .expect("mkdir outside skill");
    fs::write(outside_skill.join("SKILL.md"), "outside")
        .await
        .expect("write outside skill");
    let source_parent = temp.path().join("skills");
    fs::create_dir_all(&source_parent)
        .await
        .expect("mkdir source parent");
    let source = source_parent.join("linked-skill");
    std::os::unix::fs::symlink(&outside_skill, &source).expect("symlink source skill");
    let tool = MoveSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "source": source.to_str().unwrap(),
                "destination_parent": temp.path().join("bundle").to_str().unwrap()
            }),
        )
        .await
        .expect_err("symlinked source skill should be rejected");

    assert!(
        error.to_string().contains("symlinked directory"),
        "error should name symlink risk: {error}"
    );
    assert_eq!(
        fs::read_to_string(outside_skill.join("SKILL.md"))
            .await
            .expect("read outside skill"),
        "outside"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn move_skill_rejects_symlinked_source_artifacts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let source = temp.path().join("skills").join("to-move");
    fs::create_dir_all(&source).await.expect("mkdir source");
    fs::write(source.join("SKILL.md"), "original")
        .await
        .expect("write source");
    let outside_file = outside.path().join("secret.md");
    fs::write(&outside_file, "outside")
        .await
        .expect("write outside");
    std::os::unix::fs::symlink(&outside_file, source.join("linked.md"))
        .expect("symlink source artifact");
    let destination_parent = temp.path().join("bundles");
    let tool = MoveSkillTool::new(temp.path());

    let error = tool
        .execute(
            &make_ctx(),
            json!({
                "source": source.to_str().unwrap(),
                "destination_parent": destination_parent.to_str().unwrap()
            }),
        )
        .await
        .expect_err("symlinked source artifact should fail backup");

    assert!(
        error.to_string().contains("symlinked skill artifact"),
        "error should name symlink risk: {error}"
    );
    assert!(
        source.exists(),
        "source should remain in place after rejected move"
    );
    assert!(
        !destination_parent.join("to-move").exists(),
        "destination should not be created after rejected move"
    );
}
