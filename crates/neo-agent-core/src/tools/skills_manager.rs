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
    user_home: Option<PathBuf>,
    extra_dirs: Vec<PathBuf>,
}

impl ListSkillsTool {
    #[must_use]
    pub fn new(user_home: Option<PathBuf>, extra_dirs: Vec<PathBuf>) -> Self {
        Self {
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
        "List all discoverable skills by tier (user, extra, builtin) with their names and \
         filesystem paths.\n\n\
         Use this tool to inspect which skills are available in the current environment before \
         invoking one with the Skill tool or a slash command.\n\n\
         Skill discovery tiers (in priority order):\n\
         1. user: Skills in ~/.neo/skills/ — created by the user or the CreateSkill tool. These \
         take highest priority when multiple skills share a name.\n\
         2. extra: Skills in directories listed in the config's extra_skill_dirs setting. Useful \
         for team-shared skill directories.\n\
         3. builtin: Skills shipped with Neo (e.g. sub-skill, self-evo). These are extracted into \
         ~/.neo/skills/.builtin/ on startup. Only included in the listing when \
         include_builtin=true.\n\n\
         Output format:\n\
         Skills are grouped by tier and each entry shows the skill name and its absolute \
         filesystem path. Skills discovered at a higher tier shadow lower-tier skills with the \
         same name.\n\n\
         After identifying a skill, activate it via:\n\
         - The Skill tool (programmatic invocation).\n\
         - The /skill:<name> slash command (manual invocation in the TUI).\n\n\
         Parameters:\n\
         - include_builtin: When true, also list built-in skills shipped with Neo. Defaults to \
         false to keep the listing focused on user-managed skills."
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
        let user_home = self.user_home.clone();
        let extra_dirs = self.extra_dirs.clone();
        Box::pin(async move {
            let _args = args?;
            let mut tiers: HashMap<&'static str, Vec<(String, PathBuf)>> = HashMap::new();
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
        "Create a new skill under ~/.neo/skills/<name>/SKILL.md for reuse in future sessions.\n\n\
         When to use:\n\
         - After completing a complex, multi-step task whose workflow should be preserved.\n\
         - When the user explicitly asks to save a procedure as a skill.\n\
         - When an error was overcome and the resolution should be recorded.\n\n\
         When NOT to use:\n\
         - For trivial one-off tasks that are unlikely to recur.\n\
         - For information that is already documented in AGENTS.md or project docs.\n\n\
         The skill file must include valid YAML frontmatter followed by Markdown content. Example:\n\n\
         ---\n\
         name: deploy-staging\n\
         description: Deploys the app to staging. Use when the user asks to deploy or push to the staging environment.\n\
         type: prompt\n\
         whenToUse: When the user asks to deploy to staging, push to staging, or update the staging environment.\n\
         ---\n\n\
         # Deploy to Staging\n\n\
         ## Steps\n\
         1. Run `cargo build --release`\n\
         2. ...\n\n\
         Frontmatter fields:\n\
         - name (required): Skill identifier, must match the directory name.\n\
         - description (required): One-line summary of what the skill does.\n\
         - type (required): One of \"prompt\" (injected as a context message before the user's \
         message), \"inline\" (expanded directly into the prompt), or \"flow\" (multi-step \
         interactive workflow).\n\
         - whenToUse (recommended): Natural language trigger description for automatic skill selection.\n\n\
         If a skill with the same name already exists, the existing file is backed up under \
         ~/.neo/backups/skills/<timestamp>/<name>/SKILL.md before being overwritten.\n\n\
         After creation, the skill can be activated via the Skill tool or the /skill:<name> slash command.\n\n\
         Parameters:\n\
         - name: Directory name for the skill under ~/.neo/skills/.\n\
         - description: Short description of what the skill does.\n\
         - skill_type: \"prompt\", \"inline\", or \"flow\". Defaults to \"prompt\".\n\
         - body: Full Markdown content including YAML frontmatter and the skill body."
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
        "Move a skill directory into a parent bundle directory, creating timestamped backups of \
         every affected directory.\n\n\
         When to use:\n\
         - To group related skills under a shared parent directory (a \"bundle\"). A bundle is \
         simply a directory under ~/.neo/skills/ that contains multiple skill subdirectories, e.g. \
         ~/.neo/skills/deploy-bundle/deploy-staging/ and \
         ~/.neo/skills/deploy-bundle/deploy-prod/.\n\
         - To reorganize skills after they have been created.\n\n\
         When NOT to use:\n\
         - To rename a skill (create a new one and delete the old one instead).\n\
         - To move a skill to a different machine or workspace.\n\n\
         Parameters:\n\
         - source: Absolute path to the skill directory to move. Must contain a SKILL.md file.\n\
         - destination_parent: Absolute path to the parent directory where the skill directory \
         should be moved. The skill's directory name is preserved under this parent.\n\n\
         Behavior:\n\
         - Before the move, a timestamped backup of the source directory is created under \
         ~/.neo/backups/skills/<timestamp>/.\n\
         - If the destination already exists (a skill with the same name already lives under \
         destination_parent), the move is rejected and no changes are made.\n\
         - Returns the new absolute path of the moved skill directory.\n\n\
         After the move, the skill is discovered from its new location on the next skill scan. No \
         manual re-registration is needed."
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
    async fn list_skills_discovers_user_skills() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skills_dir = temp.path().join("skills").join("my-skill");
        fs::create_dir_all(&skills_dir).await.expect("mkdir");
        fs::write(
            skills_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: test\ntype: prompt\n---\n\nbody",
        )
        .await
        .expect("write");

        let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());
        let result = tool.execute(&make_ctx(), json!({})).await.expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("[user]"));
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
            !ListSkillsTool::new(None, Vec::new())
                .description()
                .is_empty()
        );
        assert!(!CreateSkillTool::new(".").description().is_empty());
        assert!(!MoveSkillTool::new(".").description().is_empty());
    }
}
