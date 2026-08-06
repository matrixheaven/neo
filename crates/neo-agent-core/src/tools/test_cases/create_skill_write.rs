use super::*;
use crate::ToolContext;
use crate::skills::SkillStore;
use crate::skills::SkillStoreHandle;
use crate::skills::load_host_metadata;
use serde_json::json;

fn make_ctx() -> ToolContext {
    let dir = tempfile::tempdir().unwrap();
    ToolContext::new(dir.path()).unwrap()
}

#[tokio::test]
async fn create_skill_writes_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "test-skill",
                "description": "A test skill",
                "body": "# Body\n\nInstructions."
            }),
        )
        .await
        .expect("execute");
    assert!(!result.is_error);
    assert!(result.content.contains("Created skill at"));

    let path = temp
        .path()
        .join("skills")
        .join("test-skill")
        .join("SKILL.md");
    let content = fs::read_to_string(&path).await.expect("read");
    assert!(content.contains("name: test-skill"));
    assert!(
        !content.contains("---\nname: test-skill\ndescription: A test skill\n---\n\n---"),
        "CreateSkill body is plain Markdown and must not be treated as a second frontmatter block"
    );
}

#[tokio::test]
async fn create_skill_writes_resource_files() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "resource-skill",
                "description": "Use when testing resource-backed skills.",
                "body": "# Resource Skill\n\nRead `${NEO_SKILL_DIR}/references/guide.md`.",
                "resources": [
                    {
                        "path": "references/guide.md",
                        "content": "# Guide\n\nUse this reference."
                    },
                    {
                        "path": "scripts/check.py",
                        "content": "print('ok')\n",
                        "executable": true
                    },
                    {
                        "path": "assets/template.md",
                        "content": "Name: {{name}}\n"
                    }
                ]
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let skill_dir = temp.path().join("skills").join("resource-skill");
    assert_eq!(
        fs::read_to_string(skill_dir.join("references").join("guide.md"))
            .await
            .expect("read reference"),
        "# Guide\n\nUse this reference."
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("scripts").join("check.py"))
            .await
            .expect("read script"),
        "print('ok')\n"
    );
    assert_eq!(
        fs::read_to_string(skill_dir.join("assets").join("template.md"))
            .await
            .expect("read asset"),
        "Name: {{name}}\n"
    );
}

#[tokio::test]
async fn create_skill_escapes_frontmatter_fields() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());
    let description = "first line\nname: injected";
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "quoted-skill",
                "description": description,
                "body": "# Body"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let path = temp
        .path()
        .join("skills")
        .join("quoted-skill")
        .join("SKILL.md");
    let content = fs::read_to_string(&path).await.expect("read");
    let (frontmatter, _) = crate::skills::split_frontmatter(&content).expect("frontmatter");
    let manifest: crate::skills::SkillManifest =
        serde_yaml::from_str(frontmatter).expect("manifest");
    assert_eq!(manifest.name, "quoted-skill");
    assert_eq!(manifest.description, description);
}

#[test]
fn create_skill_schema_describes_plain_markdown_body() {
    let schema = CreateSkillTool::new(".").input_schema();
    let body_description = schema["properties"]["body"]["description"]
        .as_str()
        .expect("body description");

    assert!(body_description.contains("Do not include YAML frontmatter"));
    assert!(!body_description.contains("Must include valid YAML frontmatter"));
    let schema_text = schema.to_string();
    assert!(schema_text.contains("dependencies"));
    assert!(schema_text.contains("mcp"));
}

#[tokio::test]
async fn create_skill_output_uses_canonical_frontmatter() {
    let temp = tempfile::tempdir().expect("tempdir");
    let tool = CreateSkillTool::new(temp.path());
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "canonical-skill",
                "description": "Has canonical frontmatter only",
                "body": "# Body"
            }),
        )
        .await
        .expect("execute");
    assert!(!result.is_error);

    let path = temp
        .path()
        .join("skills")
        .join("canonical-skill")
        .join("SKILL.md");
    let content = fs::read_to_string(&path).await.expect("read");
    assert!(content.contains("name: canonical-skill"));
    assert!(content.contains("description: Has canonical frontmatter only"));
    assert!(!content.contains("type:"));
    assert!(!content.contains(&["skill", "_type"].concat()));
    assert!(!content.contains("slash"));
}

#[tokio::test]
async fn create_skill_overwrites_existing_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join("skills").join("existing-skill");
    fs::create_dir_all(&skill_dir)
        .await
        .expect("mkdir skill dir");
    fs::write(skill_dir.join("SKILL.md"), "old content")
        .await
        .expect("write old skill");
    let tool = CreateSkillTool::new(temp.path());

    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "existing-skill",
                "description": "Updated skill",
                "body": "# New"
            }),
        )
        .await
        .expect("execute");

    assert!(!result.is_error);
    let content = fs::read_to_string(skill_dir.join("SKILL.md"))
        .await
        .expect("read new skill");
    assert!(content.contains("Updated skill"));
    assert!(!content.contains("old content"));
}

#[tokio::test]
async fn create_skill_writes_and_preserves_typed_host_metadata() {
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
    let skill_dir = temp.path().join("skills").join("host-skill");

    // Create with host metadata.
    let result = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "host-skill",
                "description": "Has host metadata",
                "body": "# Host Skill\n\nUses metadata.",
                "host_metadata": {
                    "interface": {
                        "display_name": "Host Display",
                        "short_description": "Picker summary"
                    },
                    "dependencies": [
                        {
                            "type": "mcp",
                            "value": "myServer",
                            "description": "My MCP server"
                        }
                    ]
                }
            }),
        )
        .await
        .expect("execute");
    assert!(!result.is_error);
    assert!(
        result.content.contains("Backup: none"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Resources: none"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Host metadata: written at"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("Skill store reloaded"),
        "{}",
        result.content
    );

    let sidecar_path = skill_dir.join("agents").join("neo.yaml");
    let (metadata, diagnostics) = load_host_metadata(&skill_dir);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(metadata.display_name("host-skill"), "Host Display");
    assert_eq!(metadata.short_description(), Some("Picker summary"));
    assert_eq!(metadata.dependencies.len(), 1);
    assert_eq!(metadata.dependencies[0].value, "myServer");
    assert_eq!(
        metadata.dependencies[0].description.as_deref(),
        Some("My MCP server")
    );
    let loaded = handle
        .get("host-skill")
        .expect("created skill should be present after reload");
    assert_eq!(loaded.host_metadata, metadata);

    // Update without host_metadata — existing sidecar preserved.
    let result2 = tool
        .execute(
            &make_ctx(),
            json!({
                "name": "host-skill",
                "description": "Updated",
                "body": "# Updated"
            }),
        )
        .await
        .expect("execute2");
    assert!(!result2.is_error);
    assert!(
        result2.content.contains("Host metadata: preserved at"),
        "{}",
        result2.content
    );
    let (preserved, diagnostics) = load_host_metadata(&skill_dir);
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    assert_eq!(preserved, metadata);
    assert_eq!(
        handle
            .get("host-skill")
            .expect("updated skill should remain present after reload")
            .host_metadata,
        metadata
    );
    assert!(sidecar_path.is_file());
}
