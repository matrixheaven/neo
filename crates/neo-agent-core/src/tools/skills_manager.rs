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
    #[serde(default)]
    pub include_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MoveSkillArgs {
    pub source: String,
    pub destination_parent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateSkillArgs {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub skill_type: String,
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
        "List all discoverable skills by tier (project, user, extra, builtin) with their names and filesystem paths."
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
        "Create a new skill under ~/.neo/skills/<name>/SKILL.md. The body must include valid YAML frontmatter followed by Markdown content."
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
        "Move a skill directory into a parent bundle, creating timestamped backups of every modified directory. The source path must be an absolute skill directory containing a SKILL.md file."
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
