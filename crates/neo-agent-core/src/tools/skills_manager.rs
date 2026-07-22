use std::{
    collections::BTreeSet,
    fs as stdfs, io,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::skills::{SkillSource, SkillStore, SkillStoreHandle, discovery};
use crate::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};

type SkillStoreReloader = Arc<dyn Fn() -> Result<SkillStore, String> + Send + Sync>;

const RESOURCE_DIRS: &[&str] = &["references", "scripts", "assets"];
const MAX_RESOURCE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_RESOURCE_BYTES: usize = 1024 * 1024;

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
pub struct CreateSkillResource {
    /// Relative path under references/, scripts/, or assets/.
    #[schemars(
        description = "Relative resource path under references/, scripts/, or assets/. Absolute paths, '..', and SKILL.md are rejected."
    )]
    pub path: String,
    /// UTF-8 text content for the resource file.
    #[schemars(description = "UTF-8 text content for the resource file.")]
    pub content: String,
    /// Request executable permissions where supported. Intended for scripts/.
    #[serde(default)]
    #[schemars(
        description = "Request executable permissions where supported. Intended for scripts/."
    )]
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
    /// Markdown body of the skill. Do not include YAML frontmatter.
    #[schemars(
        description = "Markdown body of the skill. Do not include YAML frontmatter; CreateSkill generates frontmatter from name and description."
    )]
    pub body: String,
    /// Optional text resources to write under references/, scripts/, or assets/.
    #[serde(default)]
    #[schemars(
        description = "Optional text resources to create under references/, scripts/, or assets/."
    )]
    pub resources: Vec<CreateSkillResource>,
    /// Optional Neo host metadata for agents/neo.yaml sidecar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(
        description = "Optional typed host metadata: interface (display_name, short_description) and/or MCP dependencies."
    )]
    pub host_metadata: Option<CreateSkillHostMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillHostMetadata {
    #[serde(default)]
    pub interface: Option<CreateSkillInterface>,
    #[serde(default)]
    pub dependencies: Vec<CreateSkillDependency>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillInterface {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub short_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateSkillDependency {
    #[serde(rename = "type")]
    pub dependency_type: CreateSkillDependencyType,
    pub value: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CreateSkillDependencyType {
    Mcp,
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
         filesystem path, plus an optional [references,scripts,assets] suffix when those \
         top-level resource directories are non-empty. Skills discovered at a higher tier shadow \
         lower-tier skills with the same name.\n\n\
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
            let store = SkillStore::load(&user_dirs, &extra_dirs, builtin);
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
                    let resources = skill_resource_summary(&skill.root)
                        .map_or_else(String::new, |summary| format!(" {summary}"));
                    let display = skill.display_name();
                    let label = if display != skill.name.as_str() {
                        format!(" ({display})")
                    } else {
                        String::new()
                    };
                    let mut entry = format!(
                        "  {}{}: {}{}",
                        skill.name,
                        label,
                        skill.root.display(),
                        resources
                    );
                    if let Some(short) = skill.short_description() {
                        entry.push_str(&format!(" — {short}"));
                    }
                    if !skill.host_metadata.dependencies.is_empty() {
                        let deps: Vec<_> = skill
                            .host_metadata
                            .dependencies
                            .iter()
                            .map(|d| d.value.as_str())
                            .collect();
                        entry.push_str(&format!("  [needs: {}]", deps.join(", ")));
                    }
                    lines.push(entry);
                }
            }
            let diags = store.diagnostics();
            if !diags.is_empty() {
                lines.push(String::new());
                for d in diags {
                    lines.push(format!("⚠ {}: {}", d.path.display(), d.message));
                }
            }
            Ok(ToolResult::ok(lines.join("\n")))
        })
    }
}

fn skill_resource_summary(skill_root: &Path) -> Option<String> {
    let dirs = RESOURCE_DIRS
        .iter()
        .copied()
        .filter(|dir| resource_dir_has_entries(&skill_root.join(dir)))
        .collect::<Vec<_>>();
    if dirs.is_empty() {
        None
    } else {
        Some(format!("[{}]", dirs.join(",")))
    }
}

fn resource_dir_has_entries(path: &Path) -> bool {
    let Ok(mut entries) = stdfs::read_dir(path) else {
        return false;
    };
    entries.next().is_some_and(|entry| entry.is_ok())
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
         ---\n\n\
         # Deploy to Staging\n\n\
         ## Steps\n\
         1. Run `cargo build --release`\n\
         2. ...\n\n\
         Frontmatter fields:\n\
         - name (required): Skill identifier, must match the directory name.\n\
         - description (required): One-line summary of what the skill does.\n\
         If a skill with the same name already exists, the existing skill directory is backed up \
         under ~/.neo/backups/skills/<timestamp>/<name>/ before being overwritten.\n\n\
         After creation, the skill can be activated via the Skill tool or the /skill:<name> slash command.\n\n\
         Parameters:\n\
         - name: Directory name for the skill under ~/.neo/skills/.\n\
         - description: Short description of what the skill does.\n\
         - body: Markdown body only. Do not include YAML frontmatter; this tool generates \
         frontmatter from name and description.\n\
         - host_metadata: Optional Neo UI labels and typed MCP server dependencies for agents/neo.yaml.\n\
         - resources: Optional UTF-8 text files under references/, scripts/, or assets/. \
         Resource paths must be relative and cannot target SKILL.md."
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
            validate_skill_name(&args.name)?;
            let resources = validate_resources(&args.resources)?;
            let sidecar_yaml = prepare_host_metadata(args.host_metadata.as_ref())?;
            let frontmatter = CreateSkillFrontmatter {
                name: &args.name,
                description: &args.description,
            };
            let frontmatter =
                serde_yaml::to_string(&frontmatter).map_err(|err| ToolError::InvalidInput {
                    tool: "CreateSkill".to_owned(),
                    message: format!("invalid skill frontmatter: {err}"),
                })?;
            let content = format!("---\n{frontmatter}---\n\n{}", args.body);

            let skills_root = ensure_safe_home_subdirectory(&user_home, Path::new("skills"))?;
            let skill_name = Path::new(&args.name);
            let skill_dir_path = skills_root.join(skill_name);
            let skill_dir_existed = match stdfs::symlink_metadata(&skill_dir_path) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(ToolError::Io(error)),
            };
            let skill_dir =
                ensure_safe_child_directory(&skills_root, skill_name).map_err(ToolError::Io)?;
            let path = skill_dir.join("SKILL.md");
            reject_reparse_or_symlink_if_present(&path).map_err(ToolError::Io)?;
            let agents_dir = skill_dir.join("agents");
            let sidecar_path = agents_dir.join("neo.yaml");
            if sidecar_yaml.is_some() {
                preflight_sidecar_target(&agents_dir, &sidecar_path).map_err(ToolError::Io)?;
            }

            for resource in &resources {
                preflight_resource_file(&skill_dir, resource).map_err(ToolError::Io)?;
            }

            let backup_path = if skill_dir_existed {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let backup_child = PathBuf::from("backups").join("skills");
                let backup_root = ensure_safe_home_subdirectory(&user_home, &backup_child)?;
                let backup_id = format!("{timestamp}-{}", Uuid::new_v4());
                let timestamp_dir =
                    ensure_safe_child_directory(&backup_root, Path::new(&backup_id))
                        .map_err(ToolError::Io)?;
                let backup_dir = timestamp_dir.join(&args.name);
                reject_reparse_or_symlink_if_present(&backup_dir).map_err(ToolError::Io)?;
                if let Err(error) = copy_dir(&skill_dir, &backup_dir).await {
                    let _ = fs::remove_dir_all(&backup_dir).await;
                    return Err(ToolError::Io(error));
                }
                Some(backup_dir)
            } else {
                None
            };

            write_file_atomic(&path, content.as_bytes()).map_err(ToolError::Io)?;

            if let Some(sidecar_yaml) = sidecar_yaml {
                ensure_safe_child_directory(&skill_dir, Path::new("agents"))
                    .map_err(ToolError::Io)?;
                write_file_atomic(&sidecar_path, sidecar_yaml.as_bytes()).map_err(ToolError::Io)?;
            }

            for resource in &resources {
                write_resource_file(&skill_dir, resource).map_err(ToolError::Io)?;
            }
            let backup_message = backup_path
                .as_ref()
                .map_or_else(|| "none".to_owned(), |backup| backup.display().to_string());
            let resource_message = if resources.is_empty() {
                "none".to_owned()
            } else {
                resources
                    .iter()
                    .map(|resource| resource.relative_path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let sidecar_message = if args.host_metadata.is_some() {
                format!("written at {}", sidecar_path.display())
            } else if sidecar_path.is_file() {
                format!("preserved at {}", sidecar_path.display())
            } else {
                "not present".to_owned()
            };
            let report = format!(
                "Created skill at {}\nBackup: {}\nResources: {}\nHost metadata: {}",
                path.display(),
                backup_message,
                resource_message,
                sidecar_message
            );
            match reload_shared_skill_store("CreateSkill", skill_store.as_ref(), reload.as_ref()) {
                Ok(reload_message) => Ok(ToolResult::ok(format!("{report}{reload_message}"))),
                Err(error) => Ok(ToolResult::error(format!(
                    "{report}\nSkill store reload failed: {error}\nThe package files were written, but the active skill store was not updated."
                ))),
            }
        })
    }
}

fn prepare_host_metadata(
    input: Option<&CreateSkillHostMetadata>,
) -> Result<Option<String>, ToolError> {
    let Some(input) = input else {
        return Ok(None);
    };
    let metadata = crate::skills::SkillHostMetadata {
        interface: input
            .interface
            .as_ref()
            .map(|interface| crate::skills::SkillInterface {
                display_name: interface.display_name.clone(),
                short_description: interface.short_description.clone(),
            }),
        dependencies: input
            .dependencies
            .iter()
            .map(|dependency| crate::skills::SkillToolDependency {
                value: dependency.value.clone(),
                description: dependency.description.clone(),
            })
            .collect(),
    };
    let metadata = crate::skills::metadata::validate_host_metadata(
        metadata,
        Path::new("CreateSkill.host_metadata"),
    )
    .map_err(|diagnostics| invalid_create_skill_input(diagnostics.join("; ")))?;
    if metadata.is_empty() {
        return Err(invalid_create_skill_input(
            "host_metadata must contain a non-empty interface field or MCP dependency".to_owned(),
        ));
    }
    let yaml = crate::skills::serialize_host_metadata(&metadata).ok_or_else(|| {
        invalid_create_skill_input("host_metadata could not be serialized".to_owned())
    })?;
    Ok(Some(yaml))
}

fn preflight_sidecar_target(agents_dir: &Path, sidecar_path: &Path) -> io::Result<()> {
    match stdfs::symlink_metadata(agents_dir) {
        Ok(_) => validate_safe_directory(agents_dir)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    match stdfs::symlink_metadata(sidecar_path) {
        Ok(metadata) if is_reparse_or_symlink(&metadata) || !metadata.is_file() => {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing non-regular host metadata target {}",
                    sidecar_path.display()
                ),
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_safe_directory(path: &Path) -> Result<(), ToolError> {
    ensure_safe_directory_tree(path).map_err(ToolError::Io)
}

fn ensure_safe_home_subdirectory(home: &Path, child: &Path) -> Result<PathBuf, ToolError> {
    ensure_safe_directory(home)?;
    ensure_safe_child_directory(home, child).map_err(ToolError::Io)
}

#[derive(Serialize)]
struct CreateSkillFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
}

#[derive(Debug, Clone)]
struct ValidatedResource {
    relative_path: PathBuf,
    content: String,
    executable: bool,
}

fn validate_skill_name(name: &str) -> Result<(), ToolError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(invalid_create_skill_input(
            "skill name must not be empty".to_owned(),
        ));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(invalid_create_skill_input(format!(
            "invalid skill name {name:?}: use lowercase letters, digits, '.', '_' or '-', starting with a letter or digit"
        )));
    }
    if !chars
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-'))
    {
        return Err(invalid_create_skill_input(format!(
            "invalid skill name {name:?}: use lowercase letters, digits, '.', '_' or '-'"
        )));
    }
    if name.ends_with('.') {
        return Err(invalid_create_skill_input(format!(
            "invalid skill name {name:?}: trailing dots are not portable"
        )));
    }
    let reserved_prefix = name.split('.').next().unwrap_or(name);
    if is_windows_reserved_basename(reserved_prefix) {
        return Err(invalid_create_skill_input(format!(
            "invalid skill name {name:?}: reserved Windows device name"
        )));
    }
    Ok(())
}

fn validate_resources(
    resources: &[CreateSkillResource],
) -> Result<Vec<ValidatedResource>, ToolError> {
    let mut total_bytes = 0usize;
    let mut planned_paths = BTreeSet::new();
    let mut validated = Vec::with_capacity(resources.len());
    for resource in resources {
        let content_bytes = resource.content.len();
        if content_bytes > MAX_RESOURCE_BYTES {
            return Err(invalid_create_skill_input(format!(
                "resource content too large for {:?}: {} bytes exceeds {} bytes",
                resource.path, content_bytes, MAX_RESOURCE_BYTES
            )));
        }
        total_bytes = total_bytes.checked_add(content_bytes).ok_or_else(|| {
            invalid_create_skill_input("total resource content is too large".to_owned())
        })?;
        if total_bytes > MAX_TOTAL_RESOURCE_BYTES {
            return Err(invalid_create_skill_input(format!(
                "total resource content too large: {total_bytes} bytes exceeds {MAX_TOTAL_RESOURCE_BYTES} bytes"
            )));
        }
        let relative_path = validate_resource_path(&resource.path)?;
        let planned_key = planned_resource_path_key(&relative_path);
        validate_planned_resource_path(&relative_path, &planned_key, &planned_paths)?;
        planned_paths.insert(planned_key);
        validated.push(ValidatedResource {
            relative_path,
            content: resource.content.clone(),
            executable: resource.executable,
        });
    }
    Ok(validated)
}

fn validate_planned_resource_path(
    relative_path: &Path,
    planned_key: &[String],
    planned_paths: &BTreeSet<Vec<String>>,
) -> Result<(), ToolError> {
    for planned_path in planned_paths {
        if planned_key == planned_path {
            return Err(invalid_resource_path(
                &relative_path.display().to_string(),
                "path duplicates another resource",
            ));
        }
        if planned_key.starts_with(planned_path) || planned_path.starts_with(planned_key) {
            return Err(invalid_resource_path(
                &relative_path.display().to_string(),
                "path conflicts with another resource path",
            ));
        }
    }
    Ok(())
}

fn planned_resource_path_key(relative_path: &Path) -> Vec<String> {
    relative_path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_ascii_lowercase),
            Component::CurDir
            | Component::Prefix(_)
            | Component::RootDir
            | Component::ParentDir => None,
        })
        .collect()
}

fn validate_resource_path(raw: &str) -> Result<PathBuf, ToolError> {
    if raw.is_empty() {
        return Err(invalid_resource_path(raw, "path must not be empty"));
    }
    if raw.split(['/', '\\']).any(str::is_empty) {
        return Err(invalid_resource_path(
            raw,
            "path contains an empty component",
        ));
    }
    if Path::new(raw).is_absolute() {
        return Err(invalid_resource_path(raw, "path must be relative"));
    }

    let mut components = Vec::new();
    for part in raw.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." {
            return Err(invalid_resource_path(
                raw,
                "path contains an unsafe component",
            ));
        }
        if part.ends_with('.') {
            return Err(invalid_resource_path(
                raw,
                "path component must not end with a dot",
            ));
        }
        if part.ends_with(' ') {
            return Err(invalid_resource_path(
                raw,
                "path component must not end with a space",
            ));
        }
        if part.chars().any(|ch| {
            ch.is_ascii_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        }) {
            return Err(invalid_resource_path(
                raw,
                "path component contains a Windows-illegal character",
            ));
        }
        let reserved_prefix = part.split('.').next().unwrap_or(part);
        if is_windows_reserved_basename(reserved_prefix) {
            return Err(invalid_resource_path(
                raw,
                "path contains a reserved Windows device name",
            ));
        }
        components.push(part.to_owned());
    }

    if components.len() < 2 {
        return Err(invalid_resource_path(
            raw,
            "path must include a file under a resource directory",
        ));
    }
    if !RESOURCE_DIRS.contains(&components[0].as_str()) {
        return Err(invalid_resource_path(
            raw,
            "path must start with references, scripts, or assets",
        ));
    }
    if components
        .last()
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
    {
        return Err(invalid_resource_path(
            raw,
            "resource path must not target SKILL.md",
        ));
    }

    let mut path = PathBuf::new();
    for component in components {
        path.push(component);
    }
    Ok(path)
}

fn invalid_resource_path(raw: &str, reason: &str) -> ToolError {
    invalid_create_skill_input(format!("invalid resource path {raw:?}: {reason}"))
}

fn is_windows_reserved_basename(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    )
}

fn invalid_create_skill_input(message: String) -> ToolError {
    ToolError::InvalidInput {
        tool: "CreateSkill".to_owned(),
        message,
    }
}

fn write_resource_file(skill_dir: &Path, resource: &ValidatedResource) -> io::Result<()> {
    preflight_resource_file(skill_dir, resource)?;
    let path = skill_dir.join(&resource.relative_path);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path has no parent directory: {}", path.display()),
        )
    })?;
    let relative_parent = parent.strip_prefix(skill_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource parent escapes skill directory: {}",
                parent.display()
            ),
        )
    })?;
    ensure_safe_child_directory(skill_dir, relative_parent)?;
    write_file_atomic(&path, resource.content.as_bytes())?;
    apply_resource_executable(&path, resource.executable)
}

fn preflight_resource_file(skill_dir: &Path, resource: &ValidatedResource) -> io::Result<()> {
    let path = skill_dir.join(&resource.relative_path);
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource path has no parent directory: {}", path.display()),
        )
    })?;
    let relative_parent = parent.strip_prefix(skill_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "resource parent escapes skill directory: {}",
                parent.display()
            ),
        )
    })?;
    preflight_resource_parent(skill_dir, relative_parent)?;
    reject_reparse_or_symlink_if_present(&path)?;
    match stdfs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("resource target is a directory: {}", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn preflight_resource_parent(skill_dir: &Path, relative_parent: &Path) -> io::Result<()> {
    validate_safe_directory(skill_dir)?;
    let mut current = skill_dir.to_path_buf();
    for component in relative_parent.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                match stdfs::symlink_metadata(&current) {
                    Ok(_) => validate_safe_directory(&current)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "refusing unsafe resource parent: {}",
                        relative_parent.display()
                    ),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn apply_resource_executable(path: &Path, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !executable {
        return Ok(());
    }
    let metadata = stdfs::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o100);
    stdfs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn apply_resource_executable(_path: &Path, _executable: bool) -> io::Result<()> {
    Ok(())
}

fn write_file_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    ensure_existing_safe_directory_tree(parent)?;
    reject_reparse_or_symlink_if_present(path)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    let file = temp.as_file_mut();
    file.write_all(content)?;
    file.sync_all()?;
    temp.persist(path).map(|_| ()).map_err(|error| error.error)
}

fn reject_reparse_or_symlink_if_present(path: &Path) -> io::Result<()> {
    let metadata = match stdfs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to write through symlinked skill file {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn ensure_safe_directory_tree(path: &Path) -> io::Result<()> {
    stdfs::create_dir_all(path)?;
    validate_safe_directory(path)
}

fn ensure_existing_safe_directory_tree(path: &Path) -> io::Result<()> {
    validate_safe_directory(path)
}

fn ensure_safe_child_directory(parent: &Path, child: &Path) -> io::Result<PathBuf> {
    validate_safe_directory(parent)?;
    let mut current = parent.to_path_buf();
    for component in child.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                match stdfs::symlink_metadata(&current) {
                    Ok(_) => validate_safe_directory(&current)?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        stdfs::create_dir(&current)?;
                        validate_safe_directory(&current)?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("refusing unsafe child path: {}", child.display()),
                ));
            }
        }
    }
    Ok(current)
}

fn validate_safe_directory(path: &Path) -> io::Result<()> {
    let metadata = stdfs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to write through symlinked skill directory {}",
                path.display()
            ),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("skill path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}

fn ensure_path_absent(path: &Path) -> io::Result<()> {
    match stdfs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("path already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn validate_regular_file(path: &Path) -> io::Result<()> {
    let metadata = stdfs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to read symlinked skill file {}", path.display()),
        ));
    }
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("skill path is not a file: {}", path.display()),
        ));
    }
    Ok(())
}

fn copy_file_safely(source: &Path, destination: &Path) -> io::Result<u64> {
    validate_regular_file(source)?;
    let source_metadata = stdfs::metadata(source)?;
    reject_reparse_or_symlink_if_present(destination)?;
    ensure_path_absent(destination)?;
    let mut input = stdfs::File::open(source)?;
    let mut output = stdfs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let bytes = io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    drop(output);
    copy_file_permissions(&source_metadata, destination)?;
    Ok(bytes)
}

#[cfg(unix)]
fn copy_file_permissions(source_metadata: &stdfs::Metadata, destination: &Path) -> io::Result<()> {
    stdfs::set_permissions(destination, source_metadata.permissions())
}

#[cfg(not(unix))]
fn copy_file_permissions(
    _source_metadata: &stdfs::Metadata,
    _destination: &Path,
) -> io::Result<()> {
    Ok(())
}

fn is_reparse_or_symlink(metadata: &stdfs::Metadata) -> bool {
    metadata.file_type().is_symlink() || platform_reparse_point(metadata)
}

#[cfg(windows)]
fn platform_reparse_point(metadata: &stdfs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn platform_reparse_point(_metadata: &stdfs::Metadata) -> bool {
    false
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
            match stdfs::symlink_metadata(&source) {
                Ok(_) => ensure_existing_safe_directory_tree(&source).map_err(ToolError::Io)?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(ToolResult::error(format!(
                        "source path does not exist: {}",
                        source.display()
                    )));
                }
                Err(error) => return Err(ToolError::Io(error)),
            }
            let source_skill_file = source.join("SKILL.md");
            match validate_regular_file(&source_skill_file) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Ok(ToolResult::error(format!(
                        "source is not a skill directory (no SKILL.md): {}",
                        source.display()
                    )));
                }
                Err(error) => return Err(ToolError::Io(error)),
            }
            let parent = PathBuf::from(&args.destination_parent);
            ensure_safe_directory(&parent)?;
            let destination =
                parent.join(source.file_name().ok_or_else(|| ToolError::InvalidInput {
                    tool: "MoveSkill".to_owned(),
                    message: "source has no directory name".to_owned(),
                })?);

            match stdfs::symlink_metadata(&destination) {
                Ok(_) => {
                    return Ok(ToolResult::error(format!(
                        "destination already exists: {}",
                        destination.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(ToolError::Io(error)),
            }

            if paths_refer_to_same_location(&source, &destination).await? {
                return Ok(ToolResult::error(format!(
                    "destination resolves to source path: {}",
                    destination.display()
                )));
            }

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let backup_child = PathBuf::from("backups").join("skills");
            let backup_root = ensure_safe_home_subdirectory(&backup_home, &backup_child)?;
            let backup_dir =
                ensure_safe_child_directory(&backup_root, Path::new(&format!("{timestamp}")))
                    .map_err(ToolError::Io)?;
            let backup_target = backup_dir.join(source.file_name().unwrap());
            ensure_path_absent(&backup_target).map_err(ToolError::Io)?;
            if paths_refer_to_same_location(&source, &backup_target).await? {
                return Ok(ToolResult::error(format!(
                    "backup target resolves to source path: {}",
                    backup_target.display()
                )));
            }
            if let Err(error) = copy_dir(&source, &backup_target).await {
                let _ = fs::remove_dir_all(&backup_target).await;
                return Err(ToolError::Io(error));
            }

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
    ensure_existing_safe_directory_tree(source)?;
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", destination.display()),
        )
    })?;
    ensure_existing_safe_directory_tree(parent)?;
    ensure_path_absent(destination)?;
    stdfs::create_dir(destination)?;
    validate_safe_directory(destination)?;
    let mut entries = fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source_path = entry.path();
        let dest_path = destination.join(entry.file_name());
        let metadata = stdfs::symlink_metadata(&source_path)?;
        if is_reparse_or_symlink(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to copy symlinked skill artifact {}",
                    source_path.display()
                ),
            ));
        }
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            Box::pin(copy_dir(&source_path, &dest_path)).await?;
        } else if file_type.is_file() {
            copy_file_safely(&source_path, &dest_path)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to copy non-file skill artifact {}",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use crate::skills::{SkillStore, SkillStoreHandle, load_host_metadata};
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
            "---\nname: my-skill\ndescription: test\n---\n\nbody",
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
            "---\nname: test-skill\ndescription: nested\n---\n\nbody",
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

        let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());
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

        let tool = ListSkillsTool::new(Some(temp.path().to_path_buf()), Vec::new());
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

    #[tokio::test]
    async fn extract_builtin_skills_refreshes_stale_builtin_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let builtin_skill_dir = temp.path().join("skills").join(".builtin").join("self-evo");
        fs::create_dir_all(&builtin_skill_dir)
            .await
            .expect("mkdir builtin skill dir");
        let skill_path = builtin_skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            "---\nname: self-evo\ndescription: stale\ndisableModelInvocation: true\n---\n\nSTALE_MARKER\n",
        )
        .await
        .expect("write stale builtin");

        crate::skills::builtin::extract_builtin_skills(&temp.path().join("skills"))
            .expect("extract built-ins");

        let content = fs::read_to_string(skill_path)
            .await
            .expect("read refreshed builtin");
        assert!(content.contains("No-argument invocation is not a scope"));
        assert!(!content.contains("STALE_MARKER"), "{content}");
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
    async fn create_skill_rejects_resource_path_escape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "bad-resource",
                    "description": "Bad resource",
                    "body": "# Bad",
                    "resources": [
                        {
                            "path": "references/../escaped.md",
                            "content": "escaped"
                        }
                    ]
                }),
            )
            .await
            .expect_err("resource path escapes must fail");

        assert!(error.to_string().contains("invalid resource path"));
        assert!(
            !temp
                .path()
                .join("skills")
                .join("bad-resource")
                .join("escaped.md")
                .exists()
        );
        assert!(!temp.path().join("skills").join("bad-resource").exists());
    }

    #[tokio::test]
    async fn create_skill_rejects_resource_outside_canonical_dirs() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "bad-resource",
                    "description": "Bad resource",
                    "body": "# Bad",
                    "resources": [
                        {
                            "path": "docs/guide.md",
                            "content": "guide"
                        }
                    ]
                }),
            )
            .await
            .expect_err("unsupported resource dir must fail");

        assert!(error.to_string().contains("references, scripts, or assets"));
    }

    #[tokio::test]
    async fn create_skill_rejects_absolute_resource_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let absolute_resource_path = outside.path().join("guide.md");
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "bad-resource",
                    "description": "Bad resource",
                    "body": "# Bad",
                    "resources": [
                        {
                            "path": absolute_resource_path.to_string_lossy(),
                            "content": "guide"
                        }
                    ]
                }),
            )
            .await
            .expect_err("absolute resource path must fail");

        assert!(error.to_string().contains("invalid resource path"));
        assert!(!absolute_resource_path.exists());
    }

    #[tokio::test]
    async fn create_skill_rejects_skill_md_as_resource() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "bad-resource",
                    "description": "Bad resource",
                    "body": "# Bad",
                    "resources": [
                        {
                            "path": "references/SKILL.md",
                            "content": "not a nested skill"
                        }
                    ]
                }),
            )
            .await
            .expect_err("SKILL.md resources must fail");

        assert!(error.to_string().contains("SKILL.md"));
    }

    #[tokio::test]
    async fn create_skill_rejects_windows_hostile_resource_path_components() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());

        for path in [
            "references/bad:name.md",
            "references/bad\tname.md",
            "references/trailing-space.md ",
            "references/trailing-space /guide.md",
        ] {
            let error = tool
                .execute(
                    &make_ctx(),
                    json!({
                        "name": "bad-resource",
                        "description": "Bad resource",
                        "body": "# Bad",
                        "resources": [
                            {
                                "path": path,
                                "content": "bad"
                            }
                        ]
                    }),
                )
                .await
                .expect_err("Windows-hostile resource path must fail");

            assert!(
                error.to_string().contains("invalid resource path"),
                "{path}: {error}"
            );
        }
        assert!(!temp.path().join("skills").join("bad-resource").exists());
    }

    #[tokio::test]
    async fn create_skill_rejects_path_like_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());
        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "../escaped",
                    "description": "A test skill",
                    "body": "# Body"
                }),
            )
            .await
            .expect_err("path-like names should be invalid input");

        assert!(error.to_string().contains("invalid skill name"));
        assert!(
            !temp.path().join("escaped").exists(),
            "invalid skill name must not write outside the skills directory"
        );
    }

    #[tokio::test]
    async fn create_skill_rejects_windows_reserved_name() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());
        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "con",
                    "description": "A test skill",
                    "body": "# Body"
                }),
            )
            .await
            .expect_err("reserved names should be invalid input");

        assert!(error.to_string().contains("reserved Windows device name"));
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

        assert!(error.to_string().contains("symlinked skill directory"));
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

        assert!(error.to_string().contains("symlinked skill directory"));
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

        assert!(error.to_string().contains("symlinked skill directory"));
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

        assert!(error.to_string().contains("symlinked skill file"));
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

        assert!(error.to_string().contains("symlinked skill file"));
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

        assert!(error.to_string().contains("symlinked skill file"));
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

    #[tokio::test]
    async fn create_skill_rejects_invalid_host_metadata_without_side_effects() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = CreateSkillTool::new(temp.path());

        let interface_only = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "interface-only",
                    "description": "Interface metadata only",
                    "body": "# Interface Only",
                    "host_metadata": {
                        "interface": { "display_name": "Interface Only" }
                    }
                }),
            )
            .await
            .expect("interface-only metadata should be valid");
        assert!(!interface_only.is_error);

        for (name, host_metadata) in [
            ("empty-metadata", json!({})),
            (
                "multiline-dependency",
                json!({
                    "dependencies": [{ "type": "mcp", "value": "bad\nvalue" }]
                }),
            ),
        ] {
            let skill_dir = temp.path().join("skills").join(name);
            fs::create_dir_all(&skill_dir).await.expect("mkdir skill");
            let skill_file = skill_dir.join("SKILL.md");
            fs::write(&skill_file, "original")
                .await
                .expect("write original skill");

            let error = tool
                .execute(
                    &make_ctx(),
                    json!({
                        "name": name,
                        "description": "Rejected metadata",
                        "body": "# Replacement",
                        "host_metadata": host_metadata
                    }),
                )
                .await
                .expect_err("invalid metadata should be rejected");
            assert!(
                error.to_string().contains("host_metadata"),
                "error should identify host metadata: {error}"
            );
            assert_eq!(
                fs::read_to_string(&skill_file)
                    .await
                    .expect("read original skill"),
                "original"
            );
        }

        let legacy_dir = temp.path().join("skills").join("legacy-input");
        fs::create_dir_all(&legacy_dir)
            .await
            .expect("mkdir legacy skill");
        let legacy_file = legacy_dir.join("SKILL.md");
        fs::write(&legacy_file, "original")
            .await
            .expect("write legacy skill");
        let mut legacy_input = json!({
            "name": "legacy-input",
            "description": "Legacy input",
            "body": "# Replacement"
        });
        let retired_field = ["skill", "_type"].concat();
        legacy_input
            .as_object_mut()
            .expect("object input")
            .insert(retired_field.clone(), json!("prompt"));
        let error = tool
            .execute(&make_ctx(), legacy_input)
            .await
            .expect_err("retired CreateSkill field should be rejected");
        assert!(error.to_string().contains(&retired_field), "{error}");
        assert_eq!(
            fs::read_to_string(&legacy_file)
                .await
                .expect("read legacy skill"),
            "original"
        );
        assert!(
            !temp.path().join("backups").exists(),
            "rejected metadata must not create backups"
        );
    }

    #[tokio::test]
    async fn create_skill_rejects_non_file_sidecar_before_overwriting_skill() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skill_dir = temp.path().join("skills").join("blocked-sidecar");
        let skill_file = skill_dir.join("SKILL.md");
        fs::create_dir_all(skill_dir.join("agents").join("neo.yaml"))
            .await
            .expect("create directory at sidecar target");
        fs::write(&skill_file, "original")
            .await
            .expect("write original skill");

        let error = CreateSkillTool::new(temp.path())
            .execute(
                &make_ctx(),
                json!({
                    "name": "blocked-sidecar",
                    "description": "Blocked sidecar",
                    "body": "# Replacement",
                    "host_metadata": {
                        "interface": { "display_name": "Blocked Sidecar" }
                    }
                }),
            )
            .await
            .expect_err("directory sidecar target should be rejected");

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
        assert!(
            !temp.path().join("backups").exists(),
            "preflight failure must happen before backup"
        );
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
            error.to_string().contains("symlinked skill directory"),
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
