use std::{
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
            validate_skill_name(&args.name)?;
            let skill_type = parse_skill_type(&args.skill_type)?;
            let frontmatter = CreateSkillFrontmatter {
                name: &args.name,
                description: &args.description,
                skill_type,
            };
            let frontmatter =
                serde_yaml::to_string(&frontmatter).map_err(|err| ToolError::InvalidInput {
                    tool: "CreateSkill".to_owned(),
                    message: format!("invalid skill frontmatter: {err}"),
                })?;
            let content = format!("---\n{frontmatter}---\n\n{}", args.body);

            let skills_root = user_home.join("skills");
            ensure_safe_directory(&skills_root)?;
            let skill_dir = ensure_safe_child_directory(&skills_root, Path::new(&args.name))
                .map_err(ToolError::Io)?;
            let path = skill_dir.join("SKILL.md");
            reject_reparse_or_symlink_if_present(&path).map_err(ToolError::Io)?;

            // Backup existing file before overwrite.
            if path.exists() {
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let backup_root = user_home.join("backups").join("skills");
                ensure_safe_directory(&backup_root)?;
                let timestamp_dir =
                    ensure_safe_child_directory(&backup_root, Path::new(&format!("{timestamp}")))
                        .map_err(ToolError::Io)?;
                let backup_dir = ensure_safe_child_directory(&timestamp_dir, Path::new(&args.name))
                    .map_err(ToolError::Io)?;
                let backup_path = backup_dir.join("SKILL.md");
                if let Err(error) = copy_file_safely(&path, &backup_path) {
                    let _ = fs::remove_dir_all(&backup_dir).await;
                    return Err(ToolError::Io(error));
                }
            }

            write_file_atomic(&path, content.as_bytes()).map_err(ToolError::Io)?;
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

fn ensure_safe_directory(path: &Path) -> Result<(), ToolError> {
    ensure_safe_directory_tree(path).map_err(ToolError::Io)
}

#[derive(Serialize)]
struct CreateSkillFrontmatter<'a> {
    name: &'a str,
    description: &'a str,
    #[serde(rename = "type")]
    skill_type: crate::skills::SkillType,
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

fn is_windows_reserved_basename(name: &str) -> bool {
    matches!(
        name,
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

fn parse_skill_type(value: &str) -> Result<crate::skills::SkillType, ToolError> {
    match value {
        "" | "prompt" => Ok(crate::skills::SkillType::Prompt),
        "inline" => Ok(crate::skills::SkillType::Inline),
        "flow" => Ok(crate::skills::SkillType::Flow),
        other => Err(invalid_create_skill_input(format!(
            "invalid skill_type {other:?}: expected 'prompt', 'inline', or 'flow'"
        ))),
    }
}

fn invalid_create_skill_input(message: String) -> ToolError {
    ToolError::InvalidInput {
        tool: "CreateSkill".to_owned(),
        message,
    }
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
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("skill");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let write_result = write_temp_file(&temp_path, content)
        .and_then(|()| replace_with_temp_file(&temp_path, path));
    if write_result.is_err() {
        let _ = stdfs::remove_file(&temp_path);
    }
    write_result
}

fn write_temp_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut file = stdfs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

fn replace_with_temp_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    // Rust's cross-platform `rename` contract replaces an existing file in one
    // filesystem operation, including on Windows, without a remove-first gap.
    stdfs::rename(temp_path, path)
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
    reject_reparse_or_symlink_if_present(destination)?;
    ensure_path_absent(destination)?;
    let mut input = stdfs::File::open(source)?;
    let mut output = stdfs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let bytes = io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(bytes)
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
            let backup_root = backup_home.join("backups").join("skills");
            ensure_safe_directory(&backup_root)?;
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

    #[test]
    fn builtin_skills_include_create_skill() {
        let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
        let names = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"create-skill"), "built-ins: {names:?}");
    }

    #[test]
    fn self_evo_builtin_requires_scope_and_verify_section() {
        let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
        let skill = skills
            .iter()
            .find(|skill| skill.name == "self-evo")
            .expect("self-evo built-in");
        assert!(
            skill.body.contains("No-argument invocation is not a scope"),
            "{}",
            skill.body
        );
        assert!(skill.body.contains("## Verify"), "{}", skill.body);
    }

    #[test]
    fn create_skill_builtin_requires_verify_and_create_skill_tool() {
        let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
        let skill = skills
            .iter()
            .find(|skill| skill.name == "create-skill")
            .expect("create-skill built-in");
        assert!(skill.body.contains("## Verify"), "{}", skill.body);
        assert!(skill.body.contains("CreateSkill"), "{}", skill.body);
        assert!(
            skill.manifest.disable_model_invocation,
            "create-skill must require explicit user invocation"
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
            "---\nname: self-evo\ndescription: stale\ntype: prompt\n---\n\nSTALE_MARKER\n",
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
        assert!(content.contains("type: prompt"));
        assert!(
            !content.contains(
                "---\nname: test-skill\ndescription: A test skill\ntype: prompt\n---\n\n---"
            ),
            "CreateSkill body is plain Markdown and must not be treated as a second frontmatter block"
        );
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

    #[cfg(unix)]
    #[tokio::test]
    async fn create_skill_rejects_symlinked_backup_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let skill_dir = temp.path().join("skills").join("existing-skill");
        fs::create_dir_all(&skill_dir)
            .await
            .expect("mkdir skill dir");
        fs::write(skill_dir.join("SKILL.md"), "old content")
            .await
            .expect("write old skill");
        let outside_file = outside.path().join("backup-target.md");
        fs::write(&outside_file, "outside")
            .await
            .expect("write outside");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("timestamp")
            .as_secs();
        for offset in 0..=30 {
            let backup_dir = temp
                .path()
                .join("backups")
                .join("skills")
                .join(format!("{}", timestamp + offset))
                .join("existing-skill");
            fs::create_dir_all(&backup_dir)
                .await
                .expect("mkdir backup dir");
            std::os::unix::fs::symlink(&outside_file, backup_dir.join("SKILL.md"))
                .expect("symlink backup file");
        }
        let tool = CreateSkillTool::new(temp.path());

        let error = tool
            .execute(
                &make_ctx(),
                json!({
                    "name": "existing-skill",
                    "description": "Updated skill",
                    "body": "# New"
                }),
            )
            .await
            .expect_err("symlinked backup file should fail");

        assert!(
            error.to_string().contains("symlinked skill file"),
            "error should name symlink risk: {error}"
        );
        assert_eq!(
            fs::read_to_string(&outside_file)
                .await
                .expect("read outside"),
            "outside"
        );
        assert_eq!(
            fs::read_to_string(skill_dir.join("SKILL.md"))
                .await
                .expect("read original skill"),
            "old content"
        );
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
