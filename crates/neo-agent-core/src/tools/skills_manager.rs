use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListSkillsArgs {
    /// Whether to include built-in skills shipped with Neo in the listing.
    #[serde(default)]
    #[schemars(
        description = "Whether to include built-in skills shipped with Neo in the listing. Defaults to false."
    )]
    pub include_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoveSkillArgs {
    /// Absolute path to the skill directory to move. Must contain a SKILL.md file.
    #[schemars(
        description = "Absolute path to the skill directory to move. Must contain a SKILL.md file."
    )]
    pub source: String,
    /// Absolute path to the parent directory where the skill directory should be moved.
    #[schemars(
        description = "Absolute path to the parent directory where the skill directory should be moved."
    )]
    pub destination_parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSkillArgs {
    /// Name for the new skill. Used as the directory name under ~/.neo/skills/.
    #[schemars(
        description = "Name for the new skill. Used as the directory name under ~/.neo/skills/."
    )]
    pub name: String,
    /// Short description of what the skill does. Stored in the skill frontmatter.
    #[schemars(
        description = "Short description of what the skill does. Stored in the skill frontmatter."
    )]
    pub description: String,
    /// Skill type: prompt, inline, or flow. Defaults to prompt.
    #[serde(default)]
    #[schemars(description = "Skill type: prompt, inline, or flow. Defaults to 'prompt'.")]
    pub skill_type: String,
    /// Markdown body of the skill. Must include valid YAML frontmatter followed by Markdown content.
    #[schemars(
        description = "Markdown body of the skill. Must include valid YAML frontmatter followed by Markdown content."
    )]
    pub body: String,
}

pub struct ListSkillsTool {
    project_dir: PathBuf,
    user_home: Option<PathBuf>,
    extra_dirs: Vec<PathBuf>,
}

impl ListSkillsTool {
    #[must_use]
    pub fn new(
        project_dir: impl Into<PathBuf>,
        user_home: Option<PathBuf>,
        extra_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            project_dir: project_dir.into(),
            user_home,
            extra_dirs,
        }
    }
}

impl Tool for ListSkillsTool {
    fn name(&self) -> &'static str {
        "ListSkills"
    }

    fn description(&self) -> &'static str {
        "List all discoverable skills by tier (project, user, extra, builtin) with their names and filesystem paths.\n\n\
         Use this tool to inspect which skills are available in the current environment before invoking one with the Skill tool or a slash command. Skills are discovered from:\n\
         - project: .neo/skills/\n\
         - user: ~/.neo/skills/\n\
         - extra: directories listed in config\n\
         - builtin: skills shipped with Neo (only when include_builtin=true)\n\n\
         The output groups skills by tier and shows each skill's name and absolute path."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<ListSkillsArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let args = serde_json::from_value::<ListSkillsArgs>(input).map_err(|err| {
            ToolError::InvalidInput {
                tool: "ListSkills".to_owned(),
                message: err.to_string(),
            }
        });
        let project_dir = self.project_dir.clone();
        let user_home = self.user_home.clone();
        let extra_dirs = self.extra_dirs.clone();
        Box::pin(async move {
            let _args = args?;
            let mut tiers: HashMap<&'static str, Vec<(String, PathBuf)>> = HashMap::new();
            let project_skills =
                discover_skills_in(&project_dir.join(".neo").join("skills")).await?;
            if !project_skills.is_empty() {
                tiers.insert("project", project_skills);
            }
            if let Some(home) = user_home {
                let user_skills = discover_skills_in(&home.join("skills")).await?;
                if !user_skills.is_empty() {
                    tiers.insert("user", user_skills);
                }
            }
            for dir in extra_dirs {
                let extra_skills = discover_skills_in(&dir).await?;
                if !extra_skills.is_empty() {
                    tiers.insert("extra", extra_skills);
                }
            }
            let mut lines = Vec::new();
            for (tier, skills) in tiers {
                lines.push(format!("[{tier}]"));
                for (name, path) in skills {
                    lines.push(format!("  {name}: {}", path.display()));
                }
            }
            Ok(ToolResult::ok(lines.join("\n")))
        })
    }
}

pub struct CreateSkillTool {
    user_home: PathBuf,
}

impl CreateSkillTool {
    #[must_use]
    pub fn new(user_home: impl Into<PathBuf>) -> Self {
        Self {
            user_home: user_home.into(),
        }
    }
}

impl Tool for CreateSkillTool {
    fn name(&self) -> &'static str {
        "CreateSkill"
    }

    fn description(&self) -> &'static str {
        "Create a new skill under ~/.neo/skills/<name>/SKILL.md.\n\n\
         The `body` must include valid YAML frontmatter followed by Markdown content. The frontmatter must at minimum declare `name`, `description`, and `type` (prompt, inline, or flow).\n\n\
         If a skill with the same name already exists, the existing file is backed up under ~/.neo/backups/skills/<timestamp>/<name>/SKILL.md before being overwritten.\n\n\
         After creation, the skill can be activated via the Skill tool or the /skill:<name> slash command."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<CreateSkillArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let args = serde_json::from_value::<CreateSkillArgs>(input).map_err(|err| {
            ToolError::InvalidInput {
                tool: "CreateSkill".to_owned(),
                message: err.to_string(),
            }
        });
        let user_home = self.user_home.clone();
        Box::pin(async move {
            let args = args?;
            let skill_type = if args.skill_type.is_empty() {
                "prompt"
            } else {
                &args.skill_type
            };
            let content = format!(
                "---\nname: {}\ndescription: {}\ntype: {}\n---\n\n{}",
                args.name, args.description, skill_type, args.body
            );
            // Validate frontmatter by parsing it.
            let (frontmatter, _) = crate::skills::split_frontmatter(&content).ok_or_else(|| {
                ToolError::InvalidInput {
                    tool: "CreateSkill".to_owned(),
                    message: "skill body is missing YAML frontmatter".to_owned(),
                }
            })?;
            let _manifest: crate::skills::SkillManifest = serde_yaml::from_str(frontmatter)
                .map_err(|err| ToolError::InvalidInput {
                    tool: "CreateSkill".to_owned(),
                    message: format!("invalid skill frontmatter: {err}"),
                })?;

            let skill_dir = user_home.join("skills").join(&args.name);
            fs::create_dir_all(&skill_dir)
                .await
                .map_err(ToolError::Io)?;
            let path = skill_dir.join("SKILL.md");

            // Backup existing file before overwrite.
            if path.exists() {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let backup_dir = user_home
                    .join("backups")
                    .join("skills")
                    .join(format!("{timestamp}"))
                    .join(&args.name);
                fs::create_dir_all(&backup_dir)
                    .await
                    .map_err(ToolError::Io)?;
                fs::copy(&path, backup_dir.join("SKILL.md"))
                    .await
                    .map_err(ToolError::Io)?;
            }

            fs::write(&path, content).await.map_err(ToolError::Io)?;
            Ok(ToolResult::ok(format!(
                "Created skill at {}",
                path.display()
            )))
        })
    }
}

pub struct MoveSkillTool {
    backup_home: PathBuf,
}

impl MoveSkillTool {
    #[must_use]
    pub fn new(backup_home: impl Into<PathBuf>) -> Self {
        Self {
            backup_home: backup_home.into(),
        }
    }
}

impl Tool for MoveSkillTool {
    fn name(&self) -> &'static str {
        "MoveSkill"
    }

    fn description(&self) -> &'static str {
        "Move a skill directory into a parent bundle, creating timestamped backups of every modified directory.\n\n\
         The `source` path must be an absolute skill directory containing a SKILL.md file. The skill directory is moved under `destination_parent`, preserving its directory name.\n\n\
         Before the move, a timestamped backup of the source directory is created under ~/.neo/backups/skills/<timestamp>/. If the destination already exists, the move is rejected and no changes are made."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<MoveSkillArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let args =
            serde_json::from_value::<MoveSkillArgs>(input).map_err(|err| ToolError::InvalidInput {
                tool: "MoveSkill".to_owned(),
                message: err.to_string(),
            });
        let backup_home = self.backup_home.clone();
        Box::pin(async move {
            let args = args?;
            let source = PathBuf::from(&args.source);
            if !source.exists() {
                return Ok(ToolResult::error(format!(
                    "source path does not exist: {}",
                    source.display()
                )));
            }
            if !source.join("SKILL.md").exists() {
                return Ok(ToolResult::error(format!(
                    "source is not a skill directory (no SKILL.md): {}",
                    source.display()
                )));
            }
            let parent = PathBuf::from(&args.destination_parent);
            fs::create_dir_all(&parent).await.map_err(ToolError::Io)?;
            let destination =
                parent.join(source.file_name().ok_or_else(|| ToolError::InvalidInput {
                    tool: "MoveSkill".to_owned(),
                    message: "source has no directory name".to_owned(),
                })?);

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_dir = backup_home
                .join("backups")
                .join("skills")
                .join(format!("{timestamp}"))
                .join(source.parent().unwrap_or(Path::new("")));
            fs::create_dir_all(&backup_dir)
                .await
                .map_err(ToolError::Io)?;
            let backup_target = backup_dir.join(source.file_name().unwrap());
            copy_dir(&source, &backup_target)
                .await
                .map_err(ToolError::Io)?;

            if destination.exists() {
                return Ok(ToolResult::error(format!(
                    "destination already exists: {}",
                    destination.display()
                )));
            }
            fs::rename(&source, &destination)
                .await
                .map_err(ToolError::Io)?;

            Ok(ToolResult::ok(format!(
                "Moved {} -> {}\nBackup: {}",
                source.display(),
                destination.display(),
                backup_target.display()
            )))
        })
    }
}

async fn discover_skills_in(dir: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let mut result = Vec::new();
    if !dir.exists() {
        return Ok(result);
    }
    let mut entries = fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_dir() {
            collect_skills(&path, dir, &mut result).await?;
        }
    }
    Ok(result)
}

async fn collect_skills(
    path: &Path,
    root: &Path,
    result: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    if path.join("SKILL.md").exists() {
        let relative = path.strip_prefix(root).unwrap_or(path);
        let name = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .filter(|component| component != "skills")
            .collect::<Vec<_>>()
            .join("/");
        result.push((name, path.to_path_buf()));
    }
    let mut entries = fs::read_dir(path).await?;
    while let Some(entry) = entries.next_entry().await? {
        let child = entry.path();
        if child.is_dir() {
            Box::pin(collect_skills(&child, root, result)).await?;
        }
    }
    Ok(())
}

async fn copy_dir(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination).await?;
    let mut entries = fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source_path = entry.path();
        let dest_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            Box::pin(copy_dir(&source_path, &dest_path)).await?;
        } else {
            fs::copy(&source_path, &dest_path).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use serde_json::json;

    fn make_ctx() -> ToolContext {
        ToolContext::new(std::env::current_dir().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn list_skills_discovers_project_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skills_dir = temp.path().join(".neo").join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).await.expect("mkdir");
        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: test\ntype: prompt\n---\n\nbody",
        )
        .await
        .expect("write");

        let tool = ListSkillsTool::new(temp.path(), None, Vec::new());
        let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("[project]"));
        assert!(result.content.contains("my-skill"));
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
        assert!(content.contains("type: prompt"));
    }

    #[tokio::test]
    async fn create_skill_supports_custom_type() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());
        let result = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "inline-skill",
                    "description": "An inline skill",
                    "skill_type": "inline",
                    "body": "# Body"
                }),
            )
            .await
            .expect("execute");
        assert!(!result.is_error);

        let path = temp
            .path()
            .join("skills")
            .join("inline-skill")
            .join("SKILL.md");
        let content = fs::read_to_string(&path).await.expect("read");
        assert!(content.contains("type: inline"));
    }

    #[tokio::test]
    async fn move_skill_moves_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("skills").join("to-move");
        fs::create_dir_all(&source).await.expect("mkdir");
        fs::write(source.join("SKILL.md"), "skill content")
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
        assert!(dest_parent.join("to-move").join("SKILL.md").exists());
    }

    #[test]
    fn tool_descriptions_are_non_empty() {
        assert!(
            !ListSkillsTool::new(".", None, Vec::new())
                .description()
                .is_empty()
        );
        assert!(!CreateSkillTool::new(".").description().is_empty());
        assert!(!MoveSkillTool::new(".").description().is_empty());
    }
}
