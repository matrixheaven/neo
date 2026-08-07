//! Canonical theme repository for the single Neo home.
//!
//! Ownership boundary:
//! - `ThemeId` is the only persisted form of a theme location: a validated
//!   logical relative path under `$NEO_HOME/themes/`, always persisted with `/`
//!   separators and never as an absolute, traversing, or symlinked path.
//! - `ThemeRepository` is the only owner of theme file discovery and mutation
//!   (import/copy/delete/overwrite/save-as-new). The TUI manager and the future
//!   ThemeDraft adapter consume this repository; they never touch theme files
//!   directly.
//! - `resolve_themes` implements the startup-selection contract: explicit id
//!   first, then sorted-first discovery only when no id is configured. An
//!   explicit invalid/missing id resolves to the built-in default with a
//!   visible diagnostic and never falls back to another JSON file.
//!
//! Theme JSON is untrusted data: unknown semantic keys are rejected by the
//! strict `ThemeColors` schema and color values are parsed into the single
//! runtime `neo_tui::primitive::TuiTheme` model.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(test)]
use crate::config::expand_user_path_with_home;
use anyhow::{Context, bail};
use neo_tui::primitive::Color;
use neo_tui::shell::TuiTheme;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Logical id contract
// ---------------------------------------------------------------------------

/// A validated logical theme location under `$NEO_HOME/themes/`.
///
/// Persisted form always uses `/` separators, is never absolute, never
/// traverses out of the theme directory, contains no control characters, and
/// no platform-reserved component names.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ThemeId(String);

impl ThemeId {
    /// Validate and construct a `ThemeId` from raw input.
    ///
    /// Normalizes `/` and `\` to `/` before validating. Rejects absolute
    /// paths, `..` traversal, `.` components, empty components, control
    /// characters, and platform-reserved names (`CON`, `PRN`, `AUX`, `NUL`,
    /// `COM1..9`, `LPT1..9`) on any platform.
    pub fn new(raw: &str) -> anyhow::Result<Self> {
        let normalized = raw.replace('\\', "/");
        validate_theme_id(&normalized)?;
        Ok(Self(normalized))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert this logical id into a platform path under `root`.
    ///
    /// Only callable after `new` validation; the platform path is built from
    /// already-validated components. Callers must still run
    /// [`ensure_no_symlink_components`] before touching the filesystem.
    #[must_use]
    pub(crate) fn path_under(&self, root: &Path) -> PathBuf {
        let mut path = root.to_path_buf();
        for component in Path::new(&self.0).components() {
            if let Component::Normal(part) = component {
                path.push(part);
            }
        }
        path
    }
}

impl std::fmt::Display for ThemeId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::str::FromStr for ThemeId {
    type Err = anyhow::Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::new(raw)
    }
}

fn validate_theme_id(id: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!id.is_empty(), "theme id must not be empty");
    anyhow::ensure!(
        !id.ends_with('/'),
        "theme id must not end with a separator: {id:?}"
    );
    anyhow::ensure!(
        !id.split('/').any(|part| part.is_empty()),
        "theme id contains an empty component: {id:?}"
    );
    anyhow::ensure!(
        !id.split('/').any(|part| part == "." || part == ".."),
        "theme id must not contain '.' or '..' components: {id:?}"
    );
    let path = Path::new(id);
    anyhow::ensure!(
        !path.is_absolute(),
        "theme id must be a relative path, got absolute path {id:?}"
    );
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    anyhow::anyhow!("theme id contains a non-UTF-8 component: {id:?}")
                })?;
                anyhow::ensure!(
                    !part.is_empty(),
                    "theme id contains an empty component: {id:?}"
                );
                anyhow::ensure!(
                    !part.chars().any(|character| character.is_control()),
                    "theme id contains a control character: {id:?}"
                );
                anyhow::ensure!(
                    !part.ends_with('.') && !part.ends_with(' '),
                    "theme id component {part:?} must not end with '.' or ' '"
                );
                anyhow::ensure!(
                    !part.starts_with('.'),
                    "theme id component {part:?} must not start with '.'"
                );
                anyhow::ensure!(
                    !is_reserved_component(part),
                    "theme id component {part:?} is a platform-reserved name"
                );
            }
            Component::CurDir => {
                anyhow::bail!("theme id must not contain '.' components: {id:?}");
            }
            Component::ParentDir => {
                anyhow::bail!("theme id must not contain '..' components: {id:?}");
            }
            Component::RootDir | Component::Prefix(_) => {
                anyhow::bail!("theme id must be a relative path: {id:?}");
            }
        }
    }
    Ok(())
}

/// Windows device names are reserved even when a theme file is later consumed
/// on Windows; reject them on every platform so a catalog is portable.
fn is_reserved_component(part: &str) -> bool {
    let stem = part
        .split('.')
        .next()
        .expect("split always yields a first element");
    if stem.len() == 2 && stem.ends_with(':') {
        // A `C:` drive prefix is normalized to a `C:` component on platforms
        // where `\` is not a separator; reject it everywhere for portability.
        return true;
    }
    if stem.len() != 3 && stem.len() != 4 {
        return false;
    }
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

// ---------------------------------------------------------------------------
// Strict semantic-token schema (unchanged contract)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ThemeFile {
    name: Option<String>,
    #[serde(default)]
    colors: ThemeColors,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeColors {
    text_primary: Option<String>,
    prompt: Option<String>,
    brand: Option<String>,
    status_ok: Option<String>,
    status_error: Option<String>,
    status_warn: Option<String>,
    text_muted: Option<String>,
    user_message: Option<String>,
    diff_added: Option<String>,
    diff_removed: Option<String>,
    diff_hunk: Option<String>,
    diff_context: Option<String>,
    selection_bg: Option<String>,
    status_pending: Option<String>,
    status_cancelled: Option<String>,
    approval_border: Option<String>,
    selected_fg: Option<String>,
    selected_bg: Option<String>,
    overlay_border: Option<String>,
    footer_permission_allow: Option<String>,
    footer_permission_ask: Option<String>,
    footer_permission_deny: Option<String>,
    footer_working: Option<String>,
    footer_context_ok: Option<String>,
    footer_context_warn: Option<String>,
    footer_context_critical: Option<String>,
    shell_mode: Option<String>,
}

/// Semantic-token overrides for canonical materialization. Each entry is
/// serialized as the same strict token name used by `ThemeColors`.
///
/// Consumed by the future ThemeDraft adapter.
#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeOverrides {
    pub text_primary: Option<String>,
    pub prompt: Option<String>,
    pub brand: Option<String>,
    pub status_ok: Option<String>,
    pub status_error: Option<String>,
    pub status_warn: Option<String>,
    pub text_muted: Option<String>,
    pub user_message: Option<String>,
    pub diff_added: Option<String>,
    pub diff_removed: Option<String>,
    pub diff_hunk: Option<String>,
    pub diff_context: Option<String>,
    pub selection_bg: Option<String>,
    pub status_pending: Option<String>,
    pub status_cancelled: Option<String>,
    pub approval_border: Option<String>,
    pub selected_fg: Option<String>,
    pub selected_bg: Option<String>,
    pub overlay_border: Option<String>,
    pub footer_permission_allow: Option<String>,
    pub footer_permission_ask: Option<String>,
    pub footer_permission_deny: Option<String>,
    pub footer_working: Option<String>,
    pub footer_context_ok: Option<String>,
    pub footer_context_warn: Option<String>,
    pub footer_context_critical: Option<String>,
    pub shell_mode: Option<String>,
}

// ---------------------------------------------------------------------------
// Repository / catalog model
// ---------------------------------------------------------------------------

/// One theme entry in the catalog. Valid entries have a loaded theme; invalid
/// entries keep the load error so a malformed file is never silently hidden
/// while valid sibling files remain usable.
#[derive(Debug, Clone)]
pub struct ThemeEntry {
    pub id: ThemeId,
    pub name: String,
    pub path: PathBuf,
    pub theme: TuiTheme,
    pub error: Option<String>,
}

impl ThemeEntry {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.error.is_none()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            self.id.as_str()
        } else {
            &self.name
        }
    }
}

/// The full catalog of `$NEO_HOME/themes/**/*.json`, sorted by id. A malformed
/// entry does not hide valid siblings.
#[derive(Debug, Clone)]
pub struct ThemeCatalog {
    pub entries: Vec<ThemeEntry>,
}

impl ThemeCatalog {
    pub fn valid_entries(&self) -> impl Iterator<Item = &ThemeEntry> {
        self.entries.iter().filter(|entry| entry.is_valid())
    }

    /// Exact id match. Never fuzzy-matches.
    pub fn by_id(&self, id: &ThemeId) -> Option<&ThemeEntry> {
        self.entries.iter().find(|entry| entry.id == *id)
    }

    /// Unique exact display-name match. Returns the entry when exactly one
    /// valid theme has that name; otherwise an ambiguity or not-found error.
    #[allow(dead_code)]
    pub fn by_display_name(&self, name: &str) -> anyhow::Result<&ThemeEntry> {
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.is_valid() && entry.display_name() == name);
        let first = matches.next();
        let Some(first) = first else {
            bail!("no theme named {name:?}");
        };
        if matches.next().is_some() {
            bail!("theme name {name:?} is ambiguous; use its id instead");
        }
        Ok(first)
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Resolution provenance for startup selection. Carries enough information for
/// tests and logs to identify the bounded legacy fallback without telemetry.
#[derive(Debug, Clone, Default)]
pub enum ThemeResolution {
    /// An explicit configured id resolved successfully.
    Explicit(ThemeEntry),
    /// No id configured; sorted-first discovery selected an entry.
    Discovered(ThemeEntry),
    /// No id configured and nothing to discover; built-in default.
    #[default]
    Default,
    /// An explicit id was configured but did not resolve; the built-in default
    /// is used and the diagnostic explains why. The id is never replaced by
    /// sorted-first discovery.
    Fallback { id: ThemeId, reason: String },
}

impl ThemeResolution {
    /// Convert the resolution into the runtime `ResolvedTheme` used for
    /// propagation. `id` is `Some` only for a successful explicit selection.
    #[must_use]
    pub fn to_resolved(&self) -> ResolvedTheme {
        match self {
            Self::Explicit(entry) => ResolvedTheme {
                name: entry.name.clone(),
                theme: entry.theme,
                source: Some(entry.path.clone()),
                id: Some(entry.id.clone()),
            },
            Self::Discovered(entry) => ResolvedTheme {
                name: entry.name.clone(),
                theme: entry.theme,
                source: Some(entry.path.clone()),
                id: None,
            },
            Self::Default | Self::Fallback { .. } => ResolvedTheme::default(),
        }
    }

    /// Human-readable provenance line for startup diagnostics.
    #[must_use]
    pub fn diagnostic(&self) -> Option<String> {
        match self {
            Self::Fallback { id, reason } => Some(format!(
                "theme {id:?} is not usable; using the built-in default theme ({reason})"
            )),
            Self::Default | Self::Explicit(_) | Self::Discovered(_) => None,
        }
    }
}

/// The resolved runtime theme. `id` is `Some` only when the theme came from a
/// successful explicit id selection; `None` marks the built-in default or the
/// legacy sorted-first discovery fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub name: String,
    pub theme: TuiTheme,
    pub source: Option<PathBuf>,
    pub id: Option<ThemeId>,
}

impl Default for ResolvedTheme {
    fn default() -> Self {
        Self {
            name: "default".to_owned(),
            theme: TuiTheme::default(),
            source: None,
            id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Repository
// ---------------------------------------------------------------------------

/// The canonical theme repository for one Neo home. Owns all theme-file
/// mutation: import, copy, delete, overwrite, and save-as-new.
#[derive(Debug, Clone)]
pub struct ThemeRepository {
    root: PathBuf,
}

#[allow(dead_code)]
impl ThemeRepository {
    /// Repository for the single Neo home (`$NEO_HOME` or `~/.neo`).
    pub fn default() -> Self {
        Self::from_home(crate::config::neo_home())
    }

    /// Repository rooted at the themes directory next to `config_path` (the
    /// config lives directly inside the Neo home root).
    pub fn from_config_path(config_path: &Path) -> Self {
        let root = config_path
            .parent()
            .map_or_else(|| PathBuf::from("themes"), |home| home.join("themes"));
        Self { root }
    }

    /// Repository for an explicit Neo home root. `None` keeps the repository
    /// safely in-memory with no usable files.
    pub fn from_home(home: Option<PathBuf>) -> Self {
        Self {
            root: home.map_or_else(|| PathBuf::from("themes"), |home| home.join("themes")),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-scan the theme directory into a catalog. A malformed entry is listed
    /// with its error and does not hide valid siblings.
    pub fn catalog(&self) -> anyhow::Result<ThemeCatalog> {
        discover_catalog(&self.root)
    }

    /// Resolve an explicit logical id.
    pub fn resolve(&self, id: &ThemeId) -> anyhow::Result<ThemeEntry> {
        let catalog = self.catalog()?;
        catalog.by_id(id).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "theme {:?} not found in {}",
                id.as_str(),
                self.root.display()
            )
        })
    }

    /// Resolve by exact id first, then by unique exact display name.
    /// Never fuzzy-matches.
    pub fn resolve_ref(&self, reference: &str) -> anyhow::Result<ThemeEntry> {
        if let Ok(id) = ThemeId::new(reference)
            && let Some(entry) = self.catalog()?.by_id(&id)
        {
            return Ok(entry.clone());
        }
        self.catalog()?.by_display_name(reference).cloned()
    }

    /// Load the theme file for a validated id.
    pub fn load(&self, id: &ThemeId) -> anyhow::Result<ResolvedTheme> {
        let path = id.path_under(&self.root);
        ensure_no_symlink_components(&self.root, &path)?;
        load_theme_file(&path)
    }

    /// Atomically write `content` to `target`, creating a temporary file in the
    /// same directory and replacing the target on success.
    ///
    /// This is the repository's own atomic-write path (theme files only). It is
    /// intentionally not shared with `config::atomic_file::write_with`: that
    /// helper is owned by the config module, requires a writer closure for its
    /// config-mutation test hooks, and does not create parent directories,
    /// while theme ids may be nested (`nested/x.json`) and must create them.
    /// `json_store::write_json` is a private serializer-specific helper without
    /// fsync. The temp-file-in-root + atomic `persist` + sync sequence below
    /// follows the same convention as both existing helpers.
    pub fn write_atomic(target: &Path, content: &[u8]) -> anyhow::Result<()> {
        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create theme directory {}", parent.display()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!(
                "failed to create temporary theme file beside {}",
                target.display()
            )
        })?;
        use std::io::Write as _;
        temporary
            .write_all(content)
            .with_context(|| format!("failed to write temporary theme for {}", target.display()))?;
        temporary
            .as_file_mut()
            .flush()
            .with_context(|| format!("failed to flush temporary theme {}", target.display()))?;
        temporary
            .as_file()
            .sync_all()
            .with_context(|| format!("failed to sync temporary theme {}", target.display()))?;
        temporary
            .persist(target)
            .map_err(|error| error.error)
            .with_context(|| format!("failed to atomically replace theme {}", target.display()))?;
        // Align with `config::atomic_file::write_with`: after an atomic
        // replacement the parent directory entry must be durable too.
        #[cfg(unix)]
        File::open(parent)
            .with_context(|| format!("failed to open theme directory {}", parent.display()))?
            .sync_all()
            .with_context(|| format!("failed to sync theme directory {}", parent.display()))?;
        // On Windows, tempfile's MoveFileExW replacement is atomic, but Rust has
        // no safe API for flushing a directory handle after the replacement.
        Ok(())
    }

    /// Import a theme file from an outside source into the repository under
    /// `id`. The source path is read-only input; it is never stored.
    pub fn import(&self, id: &ThemeId, source: &Path) -> anyhow::Result<ThemeEntry> {
        let content = fs::read(source)
            .with_context(|| format!("failed to read theme source {}", source.display()))?;
        let text = std::str::from_utf8(&content)
            .with_context(|| format!("theme source {} is not UTF-8", source.display()))?;
        let parsed: ThemeFile = serde_json::from_str(text)
            .with_context(|| format!("failed to parse theme source {}", source.display()))?;
        self.mutate(id, |target| {
            let serialized = serialize_theme_file(&parsed, id)?;
            Self::write_atomic(target, serialized.as_bytes())
        })
    }

    /// Copy the theme at `from` into the repository under `id` (save-as-new
    /// from an existing repository entry).
    pub fn copy(&self, from: &ThemeEntry, id: &ThemeId) -> anyhow::Result<ThemeEntry> {
        self.import(id, &from.path)
    }

    /// Overwrite the theme at `id` with canonical materialization of `name`
    /// and `theme`. Fails when the id does not exist (overwrite never creates
    /// a new theme).
    pub fn overwrite(
        &self,
        id: &ThemeId,
        name: &str,
        theme: &TuiTheme,
    ) -> anyhow::Result<ThemeEntry> {
        self.mutate_existing(id, |target| {
            let serialized = materialize_complete_theme(name, theme)?;
            Self::write_atomic(target, serialized.as_bytes())
        })
    }

    /// Save a new theme under `id`. Fails when the id already exists.
    pub fn save_as_new(
        &self,
        id: &ThemeId,
        name: &str,
        theme: &TuiTheme,
    ) -> anyhow::Result<ThemeEntry> {
        self.mutate_new_required(id, |target| {
            let serialized = materialize_complete_theme(name, theme)?;
            Self::write_atomic(target, serialized.as_bytes())
        })
    }

    /// Delete the theme at `id`.
    pub fn delete(&self, id: &ThemeId) -> anyhow::Result<()> {
        self.with_lock(|repo| {
            let path = id.path_under(repo.root());
            ensure_no_symlink_components(repo.root(), &path)?;
            let entry = repo
                .catalog()?
                .by_id(id)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("theme {:?} not found", id.as_str()))?;
            if !entry.is_valid() {
                bail!(
                    "theme {:?} is malformed and cannot be deleted by id",
                    id.as_str()
                );
            }
            fs::remove_file(&path)
                .with_context(|| format!("failed to delete theme {}", path.display()))
        })
    }

    /// Run a mutation under the managed directory lock, re-scanning before and
    /// after so the returned entry reflects the on-disk state. The id may or
    /// may not exist yet (upsert semantics for import).
    fn mutate(
        &self,
        id: &ThemeId,
        write: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<ThemeEntry> {
        self.with_lock(|repo| {
            let path = id.path_under(repo.root());
            ensure_no_symlink_components(repo.root(), &path)?;
            write(&path)?;
            let refreshed = repo.catalog()?.by_id(id).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "theme {:?} did not appear in the catalog after write",
                    id.as_str()
                )
            })?;
            Ok(refreshed)
        })
    }

    /// Mutation that requires the theme to already exist (overwrite).
    fn mutate_existing(
        &self,
        id: &ThemeId,
        write: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<ThemeEntry> {
        self.with_lock(|repo| {
            let path = id.path_under(repo.root());
            ensure_no_symlink_components(repo.root(), &path)?;
            if repo.catalog()?.by_id(id).is_none() {
                bail!("theme {:?} does not exist, cannot overwrite", id.as_str());
            }
            write(&path)?;
            let refreshed = repo.catalog()?.by_id(id).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "theme {:?} did not appear in the catalog after write",
                    id.as_str()
                )
            })?;
            Ok(refreshed)
        })
    }

    /// Mutation that requires the id to be absent (save-as-new).
    fn mutate_new_required(
        &self,
        id: &ThemeId,
        write: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<ThemeEntry> {
        self.with_lock(|repo| {
            if repo.catalog()?.by_id(id).is_some() {
                bail!("theme {:?} already exists", id.as_str());
            }
            let path = id.path_under(repo.root());
            ensure_no_symlink_components(repo.root(), &path)?;
            write(&path)?;
            let refreshed = repo.catalog()?.by_id(id).cloned().ok_or_else(|| {
                anyhow::anyhow!(
                    "theme {:?} did not appear in the catalog after write",
                    id.as_str()
                )
            })?;
            Ok(refreshed)
        })
    }

    /// Hold the in-process lock plus the on-disk advisory lock for the whole
    /// themes directory while `operation` runs, so concurrent mutations
    /// re-scan against a consistent directory.
    fn with_lock<T>(
        &self,
        operation: impl FnOnce(&ThemeRepository) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let process_guard = process_lock(self.root.clone());
        let _process_guard = process_guard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create theme directory {}", self.root.display()))?;
        let lock_path = self.root.join("neo-themes.lock");
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open theme lock {}", lock_path.display()))?;
        lock_file
            .lock()
            .with_context(|| format!("failed to lock theme directory {}", self.root.display()))?;
        operation(self)
    }
}

#[allow(dead_code)]
fn process_lock(lock_key: PathBuf) -> Arc<Mutex<()>> {
    let mut locks = PROCESS_LOCKS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        locks
            .entry(lock_key)
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

static PROCESS_LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

// ---------------------------------------------------------------------------
// Discovery and startup resolution
// ---------------------------------------------------------------------------

/// Resolve the startup theme.
///
/// Selection order: explicit id (validated at the logical boundary) first;
/// built-in default plus a visible diagnostic for a missing or invalid
/// explicit id; sorted-first discovery only when no id is configured. An
/// explicit invalid id never selects another JSON file.
pub fn resolve_themes(
    config_path: &Path,
    configured_id: Option<&str>,
) -> anyhow::Result<ThemeResolution> {
    let repository = ThemeRepository::from_config_path(config_path);
    let catalog = repository.catalog()?;

    let Some(raw_id) = configured_id else {
        return Ok(discover_first(catalog));
    };

    let id = match ThemeId::new(raw_id) {
        Ok(id) => id,
        Err(error) => {
            return Ok(ThemeResolution::Fallback {
                id: ThemeId(raw_id.to_owned()),
                reason: format!("invalid theme id: {error}"),
            });
        }
    };
    let Some(entry) = catalog.by_id(&id).cloned() else {
        return Ok(ThemeResolution::Fallback {
            id: ThemeId(raw_id.to_owned()),
            reason: format!("no theme file exists at themes/{}", id.as_str()),
        });
    };
    if let Some(error) = &entry.error {
        return Ok(ThemeResolution::Fallback {
            id: ThemeId(raw_id.to_owned()),
            reason: format!("theme file is not usable: {error}"),
        });
    }
    Ok(ThemeResolution::Explicit(entry))
}

/// Legacy bounded fallback: no id configured, pick the sorted-first valid
/// theme, else the built-in default.
fn discover_first(catalog: ThemeCatalog) -> ThemeResolution {
    match catalog.valid_entries().next() {
        Some(entry) => ThemeResolution::Discovered(entry.clone()),
        None => ThemeResolution::Default,
    }
}

fn discover_catalog(root: &Path) -> anyhow::Result<ThemeCatalog> {
    if !root.exists() {
        return Ok(ThemeCatalog {
            entries: Vec::new(),
        });
    }
    let mut paths = Vec::new();
    collect_theme_paths(root, &mut paths)?;
    paths.sort();
    let mut entries = Vec::with_capacity(paths.len());
    let mut invalid_index = 0_usize;
    for path in paths {
        let relative = path.strip_prefix(root).unwrap_or(&path);
        let raw_id = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let id = match ThemeId::new(&raw_id) {
            Ok(id) => id,
            Err(error) => {
                // A malformed path (e.g. `a/b\u{1}.json` with an
                // unrepresentable component) cannot keep its raw id, and a
                // naive sanitized id could collide with a real file (e.g.
                // `a_b.json`). Derive a deterministic, unique-per-entry
                // placeholder id that is always itself a valid ThemeId; the
                // display name stays the original raw path.
                let placeholder = format!("invalid-{invalid_index}.json");
                invalid_index += 1;
                entries.push(ThemeEntry {
                    id: ThemeId::new(placeholder.as_str())
                        .unwrap_or_else(|_| ThemeId("invalid.json".to_owned())),
                    name: raw_id.clone(),
                    path,
                    theme: TuiTheme::default(),
                    error: Some(format!("invalid theme id: {error}")),
                });
                continue;
            }
        };
        match load_theme_file(&path) {
            Ok(resolved) => entries.push(ThemeEntry {
                id,
                name: resolved.name,
                path,
                theme: resolved.theme,
                error: None,
            }),
            Err(error) => entries.push(ThemeEntry {
                id,
                name: raw_id.clone(),
                path,
                theme: TuiTheme::default(),
                error: Some(format!("{error:#}")),
            }),
        }
    }
    Ok(ThemeCatalog { entries })
}

fn collect_theme_paths(dir: &Path, paths: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in
        fs::read_dir(dir).with_context(|| format!("failed to read theme dir {}", dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read theme dir entry {}", dir.display()))?;
        let path = entry.path();
        if is_symlink_or_reparse(&path) {
            // Symlinked or reparse-point entries are never followed: reading a
            // symlinked directory could escape the theme root, and mutating a
            // symlinked file would follow the link.
            continue;
        }
        if path.is_dir() {
            collect_theme_paths(&path, paths)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && path.is_file()
        {
            paths.push(path);
        }
    }
    Ok(())
}

/// Symlink/reparse targets are never part of the catalog: reading them could
/// escape the theme directory, and mutating them would follow the link.
fn is_symlink_or_reparse(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// Reject `path` (built from a validated logical id under `root`) when any
/// ancestor component up to and including `root` is a symlink or reparse
/// point. Every repository read/write/delete goes through this guard so a
/// nested theme id such as `nested/x.json` can never resolve through a
/// symlinked directory out of the managed root.
fn ensure_no_symlink_components(root: &Path, path: &Path) -> anyhow::Result<()> {
    let mut current = root.to_path_buf();
    loop {
        if is_symlink_or_reparse(&current) {
            bail!(
                "theme path {} traverses a symlink or reparse point at {}",
                path.display(),
                current.display()
            );
        }
        if current == path {
            return Ok(());
        }
        let Some(next) = path
            .strip_prefix(&current)
            .ok()
            .and_then(|relative| relative.components().next())
        else {
            bail!(
                "theme path {} escapes the theme root {}",
                path.display(),
                root.display()
            );
        };
        let Some(next) = next.as_os_str().to_str() else {
            bail!(
                "theme path {} contains a non-UTF-8 component",
                path.display()
            );
        };
        current.push(next);
    }
}

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

fn load_theme_file(path: &Path) -> anyhow::Result<ResolvedTheme> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read theme {}", path.display()))?;
    let file: ThemeFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse theme {}", path.display()))?;
    let mut theme = TuiTheme::default();
    apply_colors(&mut theme, &file.colors, path)?;
    let name = file
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(std::ffi::OsStr::to_str)
                .unwrap_or("theme")
                .to_owned()
        });
    Ok(ResolvedTheme {
        name,
        theme,
        source: Some(path.to_path_buf()),
        id: None,
    })
}

fn apply_colors(theme: &mut TuiTheme, colors: &ThemeColors, path: &Path) -> anyhow::Result<()> {
    apply_core_colors(theme, colors, path)?;
    apply_diff_and_selection_colors(theme, colors, path)?;
    apply_footer_colors(theme, colors, path)?;
    Ok(())
}

fn apply_core_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.text_primary,
        "text_primary",
        colors.text_primary.as_deref(),
        path,
    )?;
    apply_color(&mut theme.prompt, "prompt", colors.prompt.as_deref(), path)?;
    apply_color(&mut theme.brand, "brand", colors.brand.as_deref(), path)?;
    apply_color(
        &mut theme.status_ok,
        "status_ok",
        colors.status_ok.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_error,
        "status_error",
        colors.status_error.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_warn,
        "status_warn",
        colors.status_warn.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.text_muted,
        "text_muted",
        colors.text_muted.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.user_message,
        "user_message",
        colors.user_message.as_deref(),
        path,
    )
}

fn apply_diff_and_selection_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.diff_added,
        "diff_added",
        colors.diff_added.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_removed,
        "diff_removed",
        colors.diff_removed.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_hunk,
        "diff_hunk",
        colors.diff_hunk.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.diff_context,
        "diff_context",
        colors.diff_context.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selection_bg,
        "selection_bg",
        colors.selection_bg.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_pending,
        "status_pending",
        colors.status_pending.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.status_cancelled,
        "status_cancelled",
        colors.status_cancelled.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.approval_border,
        "approval_border",
        colors.approval_border.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selected_fg,
        "selected_fg",
        colors.selected_fg.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.selected_bg,
        "selected_bg",
        colors.selected_bg.as_deref(),
        path,
    )
}

fn apply_footer_colors(
    theme: &mut TuiTheme,
    colors: &ThemeColors,
    path: &Path,
) -> anyhow::Result<()> {
    apply_color(
        &mut theme.overlay_border,
        "overlay_border",
        colors.overlay_border.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_allow,
        "footer_permission_allow",
        colors.footer_permission_allow.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_ask,
        "footer_permission_ask",
        colors.footer_permission_ask.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_permission_deny,
        "footer_permission_deny",
        colors.footer_permission_deny.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_working,
        "footer_working",
        colors.footer_working.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_ok,
        "footer_context_ok",
        colors.footer_context_ok.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_warn,
        "footer_context_warn",
        colors.footer_context_warn.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.footer_context_critical,
        "footer_context_critical",
        colors.footer_context_critical.as_deref(),
        path,
    )?;
    apply_color(
        &mut theme.shell_mode,
        "shell_mode",
        colors.shell_mode.as_deref(),
        path,
    )?;
    Ok(())
}

fn apply_color(
    target: &mut Color,
    field: &str,
    value: Option<&str>,
    path: &Path,
) -> anyhow::Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    *target = parse_color(value)
        .with_context(|| format!("invalid color for {field} in {}", path.display()))?;
    Ok(())
}

fn parse_color(value: &str) -> anyhow::Result<Color> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex_color(hex);
    }
    named_color(value)
}

fn parse_hex_color(hex: &str) -> anyhow::Result<Color> {
    if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("expected #rrggbb");
    }
    let red = u8::from_str_radix(&hex[0..2], 16)?;
    let green = u8::from_str_radix(&hex[2..4], 16)?;
    let blue = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Color::Rgb(red, green, blue))
}

fn named_color(value: &str) -> anyhow::Result<Color> {
    let normalized = value.to_ascii_lowercase();
    named_color_table()
        .iter()
        .find_map(|(name, color)| (*name == normalized).then_some(*color))
        .ok_or_else(|| anyhow::anyhow!("unknown color {value:?}"))
}

fn named_color_table() -> &'static [(&'static str, Color)] {
    &[
        ("reset", Color::Reset),
        ("black", Color::Black),
        ("red", Color::Red),
        ("green", Color::Green),
        ("yellow", Color::Yellow),
        ("blue", Color::Blue),
        ("magenta", Color::Magenta),
        ("cyan", Color::Cyan),
        ("gray", Color::Gray),
        ("grey", Color::Gray),
        ("darkgray", Color::DarkGray),
        ("dark_gray", Color::DarkGray),
        ("dark-grey", Color::DarkGray),
        ("lightred", Color::LightRed),
        ("light_red", Color::LightRed),
        ("light-red", Color::LightRed),
        ("lightgreen", Color::LightGreen),
        ("light_green", Color::LightGreen),
        ("light-green", Color::LightGreen),
        ("lightyellow", Color::LightYellow),
        ("light_yellow", Color::LightYellow),
        ("light-yellow", Color::LightYellow),
        ("lightblue", Color::LightBlue),
        ("light_blue", Color::LightBlue),
        ("light-blue", Color::LightBlue),
        ("lightmagenta", Color::LightMagenta),
        ("light_magenta", Color::LightMagenta),
        ("light-magenta", Color::LightMagenta),
        ("lightcyan", Color::LightCyan),
        ("light_cyan", Color::LightCyan),
        ("light-cyan", Color::LightCyan),
        ("white", Color::White),
    ]
}

// ---------------------------------------------------------------------------
// Canonical serialization
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn serialize_theme_file(file: &ThemeFile, id: &ThemeId) -> anyhow::Result<String> {
    let mut colors = serde_json::Map::new();
    for (token, value) in theme_colors_as_map(&file.colors) {
        if let Some(value) = value {
            colors.insert(
                token.to_owned(),
                serde_json::Value::String(value.to_owned()),
            );
        }
    }
    let mut document = serde_json::Map::new();
    document.insert(
        "name".to_owned(),
        serde_json::Value::String(file.name.clone().unwrap_or_else(|| id.as_str().to_owned())),
    );
    document.insert("colors".to_owned(), serde_json::Value::Object(colors));
    serde_json::to_string_pretty(&serde_json::Value::Object(document))
        .context("failed to serialize theme")
}

#[allow(dead_code)]
fn theme_colors_as_map(colors: &ThemeColors) -> Vec<(&'static str, Option<&str>)> {
    vec![
        ("text_primary", colors.text_primary.as_deref()),
        ("prompt", colors.prompt.as_deref()),
        ("brand", colors.brand.as_deref()),
        ("status_ok", colors.status_ok.as_deref()),
        ("status_error", colors.status_error.as_deref()),
        ("status_warn", colors.status_warn.as_deref()),
        ("text_muted", colors.text_muted.as_deref()),
        ("user_message", colors.user_message.as_deref()),
        ("diff_added", colors.diff_added.as_deref()),
        ("diff_removed", colors.diff_removed.as_deref()),
        ("diff_hunk", colors.diff_hunk.as_deref()),
        ("diff_context", colors.diff_context.as_deref()),
        ("selection_bg", colors.selection_bg.as_deref()),
        ("status_pending", colors.status_pending.as_deref()),
        ("status_cancelled", colors.status_cancelled.as_deref()),
        ("approval_border", colors.approval_border.as_deref()),
        ("selected_fg", colors.selected_fg.as_deref()),
        ("selected_bg", colors.selected_bg.as_deref()),
        ("overlay_border", colors.overlay_border.as_deref()),
        (
            "footer_permission_allow",
            colors.footer_permission_allow.as_deref(),
        ),
        (
            "footer_permission_ask",
            colors.footer_permission_ask.as_deref(),
        ),
        (
            "footer_permission_deny",
            colors.footer_permission_deny.as_deref(),
        ),
        ("footer_working", colors.footer_working.as_deref()),
        ("footer_context_ok", colors.footer_context_ok.as_deref()),
        ("footer_context_warn", colors.footer_context_warn.as_deref()),
        (
            "footer_context_critical",
            colors.footer_context_critical.as_deref(),
        ),
        ("shell_mode", colors.shell_mode.as_deref()),
    ]
}

/// Canonical JSON for a complete, independent theme. Only the persisted schema
/// (`name` + strict semantic `colors`) is ever written; no `description`,
/// `extends`, or second schema.
#[allow(dead_code)]
pub fn materialize_complete_theme(name: &str, theme: &TuiTheme) -> anyhow::Result<String> {
    let mut document = serde_json::Map::new();
    document.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    document.insert(
        "colors".to_owned(),
        serde_json::Value::Object(
            color_overrides_from_theme(theme)?
                .into_iter()
                .map(|(key, value)| (key, serde_json::Value::String(value)))
                .collect(),
        ),
    );
    serde_json::to_string_pretty(&serde_json::Value::Object(document))
        .context("failed to serialize theme")
}

/// Canonical JSON for a base theme plus semantic-token overrides. The base
/// theme supplies any token the overrides do not, so the result is always a
/// complete, independent theme.
#[allow(dead_code)]
pub fn materialize_theme_with_overrides(
    id: &ThemeId,
    base: &TuiTheme,
    overrides: &ThemeOverrides,
) -> anyhow::Result<String> {
    let mut tokens = color_overrides_from_theme(base)?;
    for (token, value) in theme_overrides_as_map(overrides) {
        if let Some(value) = value {
            tokens.insert(token.to_owned(), value.to_owned());
        }
    }
    let mut document = serde_json::Map::new();
    document.insert(
        "name".to_owned(),
        serde_json::Value::String(id.as_str().to_owned()),
    );
    document.insert(
        "colors".to_owned(),
        serde_json::Value::Object(
            tokens
                .into_iter()
                .map(|(key, value)| (key, serde_json::Value::String(value)))
                .collect(),
        ),
    );
    serde_json::to_string_pretty(&serde_json::Value::Object(document))
        .context("failed to serialize theme")
}

#[allow(dead_code)]
fn theme_overrides_as_map(overrides: &ThemeOverrides) -> Vec<(&'static str, Option<&str>)> {
    vec![
        ("text_primary", overrides.text_primary.as_deref()),
        ("prompt", overrides.prompt.as_deref()),
        ("brand", overrides.brand.as_deref()),
        ("status_ok", overrides.status_ok.as_deref()),
        ("status_error", overrides.status_error.as_deref()),
        ("status_warn", overrides.status_warn.as_deref()),
        ("text_muted", overrides.text_muted.as_deref()),
        ("user_message", overrides.user_message.as_deref()),
        ("diff_added", overrides.diff_added.as_deref()),
        ("diff_removed", overrides.diff_removed.as_deref()),
        ("diff_hunk", overrides.diff_hunk.as_deref()),
        ("diff_context", overrides.diff_context.as_deref()),
        ("selection_bg", overrides.selection_bg.as_deref()),
        ("status_pending", overrides.status_pending.as_deref()),
        ("status_cancelled", overrides.status_cancelled.as_deref()),
        ("approval_border", overrides.approval_border.as_deref()),
        ("selected_fg", overrides.selected_fg.as_deref()),
        ("selected_bg", overrides.selected_bg.as_deref()),
        ("overlay_border", overrides.overlay_border.as_deref()),
        (
            "footer_permission_allow",
            overrides.footer_permission_allow.as_deref(),
        ),
        (
            "footer_permission_ask",
            overrides.footer_permission_ask.as_deref(),
        ),
        (
            "footer_permission_deny",
            overrides.footer_permission_deny.as_deref(),
        ),
        ("footer_working", overrides.footer_working.as_deref()),
        ("footer_context_ok", overrides.footer_context_ok.as_deref()),
        (
            "footer_context_warn",
            overrides.footer_context_warn.as_deref(),
        ),
        (
            "footer_context_critical",
            overrides.footer_context_critical.as_deref(),
        ),
        ("shell_mode", overrides.shell_mode.as_deref()),
    ]
}

/// Serialize every semantic token of `theme` as a canonical color string.
/// `Indexed` colors cannot be represented in the persisted schema.
#[allow(dead_code)]
fn color_overrides_from_theme(theme: &TuiTheme) -> anyhow::Result<BTreeMap<String, String>> {
    let mut tokens = BTreeMap::new();
    let mut insert = |token: &'static str, color: Color| -> anyhow::Result<()> {
        tokens.insert(token.to_owned(), color_to_string(color)?);
        Ok(())
    };
    insert("text_primary", theme.text_primary)?;
    insert("prompt", theme.prompt)?;
    insert("brand", theme.brand)?;
    insert("status_ok", theme.status_ok)?;
    insert("status_error", theme.status_error)?;
    insert("status_warn", theme.status_warn)?;
    insert("text_muted", theme.text_muted)?;
    insert("user_message", theme.user_message)?;
    insert("diff_added", theme.diff_added)?;
    insert("diff_removed", theme.diff_removed)?;
    insert("diff_hunk", theme.diff_hunk)?;
    insert("diff_context", theme.diff_context)?;
    insert("selection_bg", theme.selection_bg)?;
    insert("status_pending", theme.status_pending)?;
    insert("status_cancelled", theme.status_cancelled)?;
    insert("approval_border", theme.approval_border)?;
    insert("selected_fg", theme.selected_fg)?;
    insert("selected_bg", theme.selected_bg)?;
    insert("overlay_border", theme.overlay_border)?;
    insert("footer_permission_allow", theme.footer_permission_allow)?;
    insert("footer_permission_ask", theme.footer_permission_ask)?;
    insert("footer_permission_deny", theme.footer_permission_deny)?;
    insert("footer_working", theme.footer_working)?;
    insert("footer_context_ok", theme.footer_context_ok)?;
    insert("footer_context_warn", theme.footer_context_warn)?;
    insert("footer_context_critical", theme.footer_context_critical)?;
    insert("shell_mode", theme.shell_mode)?;
    Ok(tokens)
}

/// Canonical string form of a parsed color (`#rrggbb` for RGB, the named
/// form for ANSI palette colors). `Indexed` colors cannot be represented in a
/// persisted theme file. Public so the ThemeDraft adapter can normalize
/// override values into the same canonical form the repository writes.
pub fn color_to_string(color: Color) -> anyhow::Result<String> {
    match color {
        Color::Rgb(red, green, blue) => Ok(format!("#{red:02x}{green:02x}{blue:02x}")),
        Color::Reset => Ok("reset".to_owned()),
        Color::Black => Ok("black".to_owned()),
        Color::Red => Ok("red".to_owned()),
        Color::Green => Ok("green".to_owned()),
        Color::Yellow => Ok("yellow".to_owned()),
        Color::Blue => Ok("blue".to_owned()),
        Color::Magenta => Ok("magenta".to_owned()),
        Color::Cyan => Ok("cyan".to_owned()),
        Color::Gray => Ok("gray".to_owned()),
        Color::DarkGray => Ok("darkgray".to_owned()),
        Color::LightRed => Ok("lightred".to_owned()),
        Color::LightGreen => Ok("lightgreen".to_owned()),
        Color::LightYellow => Ok("lightyellow".to_owned()),
        Color::LightBlue => Ok("lightblue".to_owned()),
        Color::LightMagenta => Ok("lightmagenta".to_owned()),
        Color::LightCyan => Ok("lightcyan".to_owned()),
        Color::White => Ok("white".to_owned()),
        Color::Indexed(index) => {
            bail!("color index {index} cannot be persisted in a theme file")
        }
    }
}

/// Parse a canonical color string back into a `Color` value.
pub fn color_from_string(value: &str) -> anyhow::Result<Color> {
    parse_color(value)
}
#[cfg(test)]
mod test_cases {
    use super::*;

    use tempfile::TempDir;

    fn repository_in(temp: &TempDir) -> ThemeRepository {
        ThemeRepository::from_home(Some(temp.path().to_path_buf()))
    }

    fn write_theme(repo: &ThemeRepository, id: &str, name: &str, color: &str) -> ThemeId {
        let id = ThemeId::new(id).expect("valid id");
        let path = id.path_under(repo.root());
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        std::fs::write(
            &path,
            format!(r#"{{"name": "{name}", "colors": {{"brand": "{color}"}}}}"#),
        )
        .expect("write theme");
        id
    }

    #[path = "catalog.rs"]
    mod catalog;
    #[path = "ids.rs"]
    mod ids;
    #[path = "json.rs"]
    mod json;
    #[path = "materialize.rs"]
    mod materialize;
    #[path = "path.rs"]
    mod path;
    #[path = "repository.rs"]
    mod repository;
}
