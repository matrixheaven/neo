use super::*;
use crate::ToolContext;
use crate::skills::SkillStore;
use crate::skills::SkillStoreHandle;
use crate::skills::discovery;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

fn list_skills_tool(neo_home: Option<PathBuf>) -> ListSkillsTool {
    let user_dirs = neo_home
        .as_deref()
        .map(discovery::user_skill_dirs)
        .unwrap_or_default();
    let store = SkillStore::load(
        &user_dirs,
        &[],
        crate::skills::builtin::builtin_skills().expect("builtin skills"),
    );
    ListSkillsTool::new(SkillStoreHandle::new(store))
}

#[tokio::test]
async fn list_skills_discovers_user_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skills_dir = temp.path().join("skills").join("my-skill");
    fs::create_dir_all(&skills_dir).await.expect("mkdir");
    fs::write(
        skills_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: test\n---\n\nbody",
    )
    .await
    .expect("write");

    let tool = list_skills_tool(Some(temp.path().to_path_buf()));
    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("[user]"));
    assert!(result.content.contains("my-skill"));
}

#[tokio::test]
async fn list_skills_includes_builtin_when_requested() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = list_skills_tool(Some(temp.path().to_path_buf()));

    let result = tool
        .execute(&make_ctx(), json!({"include_builtin": true}))
        .await
        .expect("execute");

    assert!(!result.is_error);
    assert!(result.content.contains("[builtin]"));
    assert!(result.content.contains("self-evo"));
    assert!(result.content.contains("sub-skill"));
}

#[tokio::test]
async fn list_skills_reads_the_shared_skill_store_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("shared-skill");
    fs::create_dir_all(&skill_dir).await.expect("mkdir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: shared-skill\ndescription: test\n---\n\nbody",
    )
    .await
    .expect("write");
    let handle = SkillStoreHandle::default();
    let tool = ListSkillsTool::new(handle.clone());
    handle.replace(SkillStore::load(
        &[],
        &[temp.path().to_path_buf()],
        Vec::new(),
    ));

    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");

    assert!(result.content.contains("[extra]"));
    assert!(result.content.contains("shared-skill"));
}

#[tokio::test]
async fn list_skills_reports_invocation_names_for_nested_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp
        .path()
        .join("skills")
        .join("superpowers")
        .join("skills")
        .join("test-skill");
    fs::create_dir_all(&skill_dir).await.expect("mkdir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: nested\n---\n\nbody",
    )
    .await
    .expect("write");

    let tool = list_skills_tool(Some(temp.path().to_path_buf()));
    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");

    assert!(!result.is_error);
    assert!(result.content.contains("test-skill:"));
    assert!(
        !result.content.contains("superpowers/skills/test-skill:"),
        "ListSkills should show the name accepted by the Skill tool: {}",
        result.content
    );
}

#[tokio::test]
async fn list_skills_summarizes_non_empty_resource_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("resourceful");
    fs::create_dir_all(skill_dir.join("assets"))
        .await
        .expect("mkdir assets");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir references");
    fs::create_dir_all(skill_dir.join("scripts"))
        .await
        .expect("mkdir scripts");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: resourceful\ndescription: test\n---\n\nbody",
    )
    .await
    .expect("write skill");
    fs::write(skill_dir.join("assets").join("template.md"), "template")
        .await
        .expect("write asset");
    fs::write(skill_dir.join("references").join("guide.md"), "guide")
        .await
        .expect("write reference");
    fs::write(skill_dir.join("scripts").join("check.py"), "print('ok')\n")
        .await
        .expect("write script");

    let tool = list_skills_tool(Some(temp.path().to_path_buf()));
    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");

    assert!(!result.is_error);
    assert!(result.content.contains(&format!(
        "  resourceful: {} [references,scripts,assets]",
        skill_dir.display()
    )));
    assert!(!result.content.contains("guide.md"), "{}", result.content);
    assert!(!result.content.contains("check.py"), "{}", result.content);
    assert!(
        !result.content.contains("template.md"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn list_skills_omits_empty_resource_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("quiet-skill");
    fs::create_dir_all(skill_dir.join("references"))
        .await
        .expect("mkdir empty references");
    fs::create_dir_all(skill_dir.join("scripts"))
        .await
        .expect("mkdir scripts");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: quiet-skill\ndescription: test\n---\n\nbody",
    )
    .await
    .expect("write skill");
    fs::write(skill_dir.join("scripts").join("check.py"), "print('ok')\n")
        .await
        .expect("write script");

    let tool = list_skills_tool(Some(temp.path().to_path_buf()));
    let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");

    assert!(!result.is_error);
    assert!(
        result
            .content
            .contains(&format!("  quiet-skill: {} [scripts]", skill_dir.display()))
    );
    assert!(
        !result.content.contains("[references,scripts]"),
        "{}",
        result.content
    );
}
