use std::{
    collections::BTreeSet,
    fmt::Write,
    fs as stdfs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;
use uuid::Uuid;

use crate::session::atomic_file;
use crate::skills::{SkillSource, SkillStore, SkillStoreHandle};
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
    skill_store: SkillStoreHandle,
}

impl ListSkillsTool {
    #[must_use]
    pub fn new(skill_store: SkillStoreHandle) -> Self {
        Self { skill_store }
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
         2. extra: Skills in configured extra_skill_dirs and skill_path directories. Useful for \
         team-shared skill directories.\n\
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
        let skill_store = self.skill_store.clone();
        Box::pin(async move {
            let args = args?;
            let store = skill_store.snapshot();
            let mut lines = Vec::new();
            for (source, tier) in [
                (SkillSource::User, "user"),
                (SkillSource::Extra, "extra"),
                (SkillSource::Builtin, "builtin"),
            ] {
                if source == SkillSource::Builtin && !args.include_builtin {
                    continue;
                }
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
                    let label = if display == skill.name.as_str() {
                        String::new()
                    } else {
                        format!(" ({display})")
                    };
                    let mut entry = format!(
                        "  {}{}: {}{}",
                        skill.name,
                        label,
                        skill.root.display(),
                        resources
                    );
                    if let Some(short) = skill.short_description() {
                        let _ = write!(entry, " — {short}");
                    }
                    if !skill.host_metadata.dependencies.is_empty() {
                        let deps: Vec<_> = skill
                            .host_metadata
                            .dependencies
                            .iter()
                            .map(|d| d.value.as_str())
                            .collect();
                        let _ = write!(entry, "  [needs: {}]", deps.join(", "));
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

            let skills_root = user_home.join("skills");
            atomic_file::ensure_safe_directory_tree(&skills_root).map_err(ToolError::Io)?;
            let skill_name = Path::new(&args.name);
            let skill_dir_path = skills_root.join(skill_name);
            let skill_dir_existed = match stdfs::symlink_metadata(&skill_dir_path) {
                Ok(_) => true,
                Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                Err(error) => return Err(ToolError::Io(error)),
            };
            let skill_dir = skills_root.join(skill_name);
            atomic_file::ensure_safe_directory_tree(&skill_dir).map_err(ToolError::Io)?;
            let path = skill_dir.join("SKILL.md");
            atomic_file::reject_reparse_or_symlink_if_present(&path).map_err(ToolError::Io)?;
            let agents_dir = skill_dir.join("agents");
            let sidecar_path = agents_dir.join("neo.yaml");
            if sidecar_yaml.is_some() {
                preflight_sidecar_target(&agents_dir, &sidecar_path).map_err(ToolError::Io)?;
            }

            for resource in &resources {
                preflight_resource_file(&skill_dir, resource).map_err(ToolError::Io)?;
            }

            let backup_path =
                backup_skill_if_exists(skill_dir_existed, &user_home, &args.name, &skill_dir)
                    .await?;

            atomic_file::write_file_atomic(&path, content.as_bytes()).map_err(ToolError::Io)?;

            if let Some(sidecar_yaml) = sidecar_yaml {
                atomic_file::ensure_safe_directory_tree(&agents_dir).map_err(ToolError::Io)?;
                atomic_file::write_file_atomic(&sidecar_path, sidecar_yaml.as_bytes())
                    .map_err(ToolError::Io)?;
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

async fn backup_skill_if_exists(
    skill_dir_existed: bool,
    user_home: &Path,
    skill_name: &str,
    skill_dir: &Path,
) -> Result<Option<PathBuf>, ToolError> {
    if !skill_dir_existed {
        return Ok(None);
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_child = PathBuf::from("backups").join("skills");
    let backup_root = user_home.join(&backup_child);
    atomic_file::ensure_safe_directory_tree(&backup_root).map_err(ToolError::Io)?;
    let backup_id = format!("{timestamp}-{}", Uuid::new_v4());
    let timestamp_dir = backup_root.join(&backup_id);
    atomic_file::ensure_safe_directory_tree(&timestamp_dir).map_err(ToolError::Io)?;
    let backup_dir = timestamp_dir.join(skill_name);
    atomic_file::reject_reparse_or_symlink_if_present(&backup_dir).map_err(ToolError::Io)?;
    if let Err(error) = copy_dir(skill_dir, &backup_dir).await {
        let _ = fs::remove_dir_all(&backup_dir).await;
        return Err(ToolError::Io(error));
    }
    Ok(Some(backup_dir))
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
        Ok(_) => atomic_file::validate_safe_directory(agents_dir)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    match stdfs::symlink_metadata(sidecar_path) {
        Ok(metadata) if atomic_file::is_reparse_or_symlink(&metadata) || !metadata.is_file() => {
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
    atomic_file::ensure_safe_directory_tree(parent)?;
    atomic_file::write_file_atomic(&path, resource.content.as_bytes())?;
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
    atomic_file::reject_reparse_or_symlink_if_present(&path)?;
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
    atomic_file::validate_safe_directory(skill_dir)?;
    let mut current = skill_dir.to_path_buf();
    for component in relative_parent.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                match stdfs::symlink_metadata(&current) {
                    Ok(_) => atomic_file::validate_safe_directory(&current)?,
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

fn validate_regular_file(path: &Path) -> io::Result<()> {
    let metadata = stdfs::symlink_metadata(path)?;
    if atomic_file::is_reparse_or_symlink(&metadata) {
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
    atomic_file::reject_reparse_or_symlink_if_present(destination)?;
    atomic_file::ensure_path_absent(destination)?;
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
                Ok(_) => atomic_file::validate_safe_directory(&source).map_err(ToolError::Io)?,
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
            atomic_file::ensure_safe_directory_tree(&parent).map_err(ToolError::Io)?;
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
            let backup_root = backup_home.join(&backup_child);
            atomic_file::ensure_safe_directory_tree(&backup_root).map_err(ToolError::Io)?;
            let backup_dir = backup_root.join(format!("{timestamp}"));
            atomic_file::ensure_safe_directory_tree(&backup_dir).map_err(ToolError::Io)?;
            let backup_target = backup_dir.join(source.file_name().unwrap());
            atomic_file::ensure_path_absent(&backup_target).map_err(ToolError::Io)?;
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
    atomic_file::validate_safe_directory(source)?;
    let parent = destination.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", destination.display()),
        )
    })?;
    atomic_file::validate_safe_directory(parent)?;
    atomic_file::ensure_path_absent(destination)?;
    stdfs::create_dir(destination)?;
    atomic_file::validate_safe_directory(destination)?;
    let mut entries = fs::read_dir(source).await?;
    while let Some(entry) = entries.next_entry().await? {
        let source_path = entry.path();
        let dest_path = destination.join(entry.file_name());
        let metadata = stdfs::symlink_metadata(&source_path)?;
        if atomic_file::is_reparse_or_symlink(&metadata) {
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
#[path = "test_cases/list_skills.rs"]
mod list_skills;

#[cfg(test)]
#[path = "test_cases/extract_builtin.rs"]
mod extract_builtin;

#[cfg(test)]
#[path = "test_cases/create_skill_write.rs"]
mod create_skill_write;

#[cfg(test)]
#[path = "test_cases/create_skill_reject_paths.rs"]
mod create_skill_reject_paths;

#[cfg(test)]
#[path = "test_cases/create_skill_reject_symlinks.rs"]
mod create_skill_reject_symlinks;

#[cfg(test)]
#[path = "test_cases/create_skill_backup.rs"]
mod create_skill_backup;

#[cfg(test)]
#[path = "test_cases/move_skill.rs"]
mod move_skill;

#[cfg(test)]
#[path = "test_cases/skill_descriptions.rs"]
mod skill_descriptions;
