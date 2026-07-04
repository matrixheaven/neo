use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::skills::{SkillSource, SkillStore, SkillStoreHandle, discovery};
use crate::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};

type SkillStoreReloader = Arc<dyn Fn() -> Result<SkillStore, String> + Send + Sync>;

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
    /// Markdown body of the skill. Do not include YAML frontmatter.
    #[schemars(
        description = "Markdown body of the skill. Do not include YAML frontmatter; CreateSkill generates frontmatter from name, description, and skill_type."
    )]
    pub body: String,
}

pub struct ListSkillsTool {
    neo_home: Option<PathBuf>,
    extra_dirs: Vec<PathBuf>,
}

impl ListSkillsTool {
    #[must_use]
    pub fn new(neo_home: Option<PathBuf>, extra_dirs: Vec<PathBuf>) -> Self {
        Self {
            neo_home,
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
        let neo_home = self.neo_home.clone();
        let extra_dirs = self.extra_dirs.clone();
        Box::pin(async move {
            let args = args?;
            let mut user_dirs = Vec::new();
            if let Some(home) = neo_home {
                user_dirs.extend(discovery::user_skill_dirs(&home));
            }
            let builtin = if args.include_builtin {
                crate::skills::builtin::builtin_skills().map_err(|err| ToolError::InvalidInput {
                    tool: "ListSkills".to_owned(),
                    message: err.to_string(),
                })?
            } else {
                Vec::new()
            };
            let store = SkillStore::load(&user_dirs, &extra_dirs, builtin).map_err(|err| {
                ToolError::InvalidInput {
                    tool: "ListSkills".to_owned(),
                    message: err.to_string(),
                }
            })?;
            let mut lines = Vec::new();
            for (source, tier) in [
                (SkillSource::User, "user"),
                (SkillSource::Extra, "extra"),
                (SkillSource::Builtin, "builtin"),
            ] {
                let mut skills = store
                    .iter()
                    .filter(|skill| skill.source == source)
                    .collect::<Vec<_>>();
                if skills.is_empty() {
                    continue;
                }
                skills.sort_by(|left, right| left.name.cmp(&right.name));
                lines.push(format!("[{tier}]"));
                for skill in skills {
                    lines.push(format!("  {}: {}", skill.name, skill.root.display()));
                }
            }
            Ok(ToolResult::ok(lines.join("\n")))
        })
    }
}

pub struct CreateSkillTool {
    user_home: PathBuf,
    skill_store: Option<SkillStoreHandle>,
    reload: Option<SkillStoreReloader>,
}

impl CreateSkillTool {
    #[must_use]
    pub fn new(user_home: impl Into<PathBuf>) -> Self {
        Self {
            user_home: user_home.into(),
            skill_store: None,
            reload: None,
        }
    }

    #[must_use]
    pub fn with_skill_store_reload(
        mut self,
        skill_store: SkillStoreHandle,
        reload: impl Fn() -> Result<SkillStore, String> + Send + Sync + 'static,
    ) -> Self {
        self.skill_store = Some(skill_store);
        self.reload = Some(Arc::new(reload));
        self
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
         The skill file generated by this tool includes valid YAML frontmatter followed by the \
         Markdown body you provide. Generated file example:\n\n\
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
         - body: Markdown body only. Do not include YAML frontmatter; this tool generates \
         frontmatter from name, description, and skill_type."
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
        let skill_store = self.skill_store.clone();
        let reload = self.reload.clone();
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
            let reload_message =
                reload_shared_skill_store("CreateSkill", skill_store.as_ref(), reload.as_ref())?;
            Ok(ToolResult::ok(format!(
                "Created skill at {}{}",
                path.display(),
                reload_message
            )))
        })
    }
}

pub struct MoveSkillTool {
    backup_home: PathBuf,
    skill_store: Option<SkillStoreHandle>,
    reload: Option<SkillStoreReloader>,
}

impl MoveSkillTool {
    #[must_use]
    pub fn new(backup_home: impl Into<PathBuf>) -> Self {
        Self {
            backup_home: backup_home.into(),
            skill_store: None,
            reload: None,
        }
    }

    #[must_use]
    pub fn with_skill_store_reload(
        mut self,
        skill_store: SkillStoreHandle,
        reload: impl Fn() -> Result<SkillStore, String> + Send + Sync + 'static,
    ) -> Self {
        self.skill_store = Some(skill_store);
        self.reload = Some(Arc::new(reload));
        self
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
        let skill_store = self.skill_store.clone();
        let reload = self.reload.clone();
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

            if destination.exists() {
                return Ok(ToolResult::error(format!(
                    "destination already exists: {}",
                    destination.display()
                )));
            }

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_dir = backup_home
                .join("backups")
                .join("skills")
                .join(format!("{timestamp}"));
            fs::create_dir_all(&backup_dir)
                .await
                .map_err(ToolError::Io)?;
            let backup_target = backup_dir.join(source.file_name().unwrap());
            if paths_refer_to_same_location(&source, &backup_target).await? {
                return Ok(ToolResult::error(format!(
                    "backup target resolves to source path: {}",
                    backup_target.display()
                )));
            }
            copy_dir(&source, &backup_target)
                .await
                .map_err(ToolError::Io)?;

            fs::rename(&source, &destination)
                .await
                .map_err(ToolError::Io)?;
            let reload_message =
                reload_shared_skill_store("MoveSkill", skill_store.as_ref(), reload.as_ref())?;

            Ok(ToolResult::ok(format!(
                "Moved {} -> {}\nBackup: {}{}",
                source.display(),
                destination.display(),
                backup_target.display(),
                reload_message
            )))
        })
    }
}

fn reload_shared_skill_store(
    tool: &str,
    skill_store: Option<&SkillStoreHandle>,
    reload: Option<&SkillStoreReloader>,
) -> Result<String, ToolError> {
    let (Some(skill_store), Some(reload)) = (skill_store, reload) else {
        return Ok(String::new());
    };
    let store = reload().map_err(|message| ToolError::InvalidInput {
        tool: tool.to_owned(),
        message: format!("failed to reload skill store: {message}"),
    })?;
    let count = store.len();
    skill_store.replace(store);
    Ok(format!(
        "\nSkill store reloaded ({count} skills available)."
    ))
}

async fn paths_refer_to_same_location(left: &Path, right: &Path) -> io::Result<bool> {
    if left == right {
        return Ok(true);
    }
    let left = fs::canonicalize(left).await?;
    match fs::canonicalize(right).await {
        Ok(right) => Ok(left == right),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
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
    use crate::skills::{SkillStore, SkillStoreHandle};
    use serde_json::json;

    fn make_ctx() -> ToolContext {
        let dir = tempfile::tempdir().unwrap();
        ToolContext::new(dir.path()).unwrap()
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
    async fn list_skills_includes_builtin_when_requested() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());

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
            "---\nname: test-skill\ndescription: nested\ntype: prompt\n---\n\nbody",
        )
        .await
        .expect("write");

        let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());
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
        assert!(
            !content.contains(
                "---\nname: test-skill\ndescription: A test skill\ntype: prompt\n---\n\n---"
            ),
            "CreateSkill body is plain Markdown and must not be treated as a second frontmatter block"
        );
    }

    #[test]
    fn create_skill_schema_describes_plain_markdown_body() {
        let schema = CreateSkillTool::new(".").input_schema();
        let body_description = schema["properties"]["body"]["description"]
            .as_str()
            .expect("body description");

        assert!(body_description.contains("Do not include YAML frontmatter"));
        assert!(!body_description.contains("Must include valid YAML frontmatter"));
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
    async fn create_skill_reloads_shared_skill_store() {
        let temp = tempfile::tempdir().expect("tempdir");
        let user_skills = temp.path().join("skills");
        let handle = SkillStoreHandle::new(
            SkillStore::load(std::slice::from_ref(&user_skills), &[], Vec::new())
                .expect("initial store"),
        );
        let reload_root = user_skills.clone();
        let tool =
            CreateSkillTool::new(temp.path()).with_skill_store_reload(handle.clone(), move || {
                SkillStore::load(std::slice::from_ref(&reload_root), &[], Vec::new())
                    .map_err(|err| err.to_string())
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
    async fn move_skill_moves_directory_without_losing_content() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("skills").join("to-move");
        fs::create_dir_all(&source).await.expect("mkdir");
        let original =
            "---\nname: to-move\ndescription: test\ntype: prompt\n---\n\nskill content\n";
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
