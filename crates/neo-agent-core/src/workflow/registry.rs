//! Trusted workflow definition registry (design §11).
//!
//! Scopes and precedence are exactly `builtin < user < trusted project`.
//! The registry owns discovery, validation, revision, and no-clobber save
//! only — never run state. Project discovery and save reuse the host's
//! existing workspace trust decision; there is no second trust store.
//!
//! Directory scanning never executes Lua. Cache entries are rebuildable
//! projections keyed by path, size, and mtime and cannot authorize launch.
//! Each resolved definition pins its exact source snapshot for the run.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use super::ResolvedWorkflowDefinition;
use super::definition::{DEFINITION_FORMAT_VERSION, resolve_paired_definition, source_sha256_hex};
use super::error::{WorkflowError, WorkflowErrorCode};
use super::limits::WorkflowLimits;
use super::state::{
    WorkflowName, WorkflowPhase, WorkflowPinnedSource, WorkflowRevision, WorkflowSourceOrigin,
};
use crate::session::atomic_file;

/// Directory under `$NEO_HOME` for user-scope definitions.
pub const USER_WORKFLOWS_DIR: &str = "workflows";
/// Workspace-relative directory for project-scope definitions.
pub const PROJECT_WORKFLOWS_DIR: &str = ".neo/workflows";
/// Exact manifest suffix (paired with [`SOURCE_SUFFIX`]).
pub const MANIFEST_SUFFIX: &str = ".workflow.toml";
/// Exact Lua source suffix (paired with [`MANIFEST_SUFFIX`]).
pub const SOURCE_SUFFIX: &str = ".lua";

/// Save target scopes (builtin is not writable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSaveScope {
    User,
    Project,
}

impl WorkflowSaveScope {
    #[must_use]
    pub fn as_origin(self) -> WorkflowSourceOrigin {
        match self {
            Self::User => WorkflowSourceOrigin::User,
            Self::Project => WorkflowSourceOrigin::Project,
        }
    }
}

/// Optional list / filter scope (includes effective merge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowListScope {
    Builtin,
    User,
    Project,
    Effective,
}

/// In-memory builtin definition pair compiled into the host.
#[derive(Debug, Clone)]
pub struct BuiltinWorkflowDefinition {
    pub name: String,
    pub manifest_bytes: Vec<u8>,
    pub source_bytes: Vec<u8>,
}

/// Host-supplied roots and trust decision for one registry instance.
#[derive(Debug, Clone)]
pub struct WorkflowDefinitionRegistryConfig {
    /// `$NEO_HOME` root (`$NEO_HOME/workflows` is the user scope).
    pub neo_home: PathBuf,
    /// Current workspace root (`<workspace>/.neo/workflows` is project scope).
    pub workspace: PathBuf,
    /// Existing workspace trust decision — never invented here.
    pub project_trusted: bool,
    pub limits: WorkflowLimits,
    pub builtins: Vec<BuiltinWorkflowDefinition>,
}

/// Input for atomic no-clobber save of a paired definition.
#[derive(Debug, Clone)]
pub struct WorkflowSaveRequest {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub lua_source: String,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: serde_json::Value,
}

/// One successfully resolved, listable definition summary.
#[derive(Debug, Clone)]
pub struct RegistryDefinitionSummary {
    pub name: WorkflowName,
    pub display_name: String,
    pub description: String,
    pub revision: WorkflowRevision,
    pub source_origin: WorkflowSourceOrigin,
    pub source_locator: Option<String>,
}

/// Session-shared trusted definition registry.
#[derive(Clone)]
pub struct WorkflowDefinitionRegistry {
    inner: Arc<Mutex<RegistryInner>>,
}

struct RegistryInner {
    config: WorkflowDefinitionRegistryConfig,
    /// Rebuildable projection; never a durability or authorization source.
    cache: Option<RegistryProjection>,
}

#[derive(Debug, Clone, Default)]
struct ScopeMaps {
    builtin: BTreeMap<String, ScopeEntry>,
    user: BTreeMap<String, ScopeEntry>,
    project: BTreeMap<String, ScopeEntry>,
}

impl ScopeMaps {
    fn get(&self, origin: WorkflowSourceOrigin) -> &BTreeMap<String, ScopeEntry> {
        match origin {
            WorkflowSourceOrigin::Builtin => &self.builtin,
            WorkflowSourceOrigin::User => &self.user,
            WorkflowSourceOrigin::Project => &self.project,
            WorkflowSourceOrigin::Dynamic => &self.builtin, // unused
        }
    }
}

#[derive(Debug, Clone)]
struct RegistryProjection {
    /// Per-scope name → entry (same-scope conflicts / invalids recorded).
    scopes: ScopeMaps,
    /// Cache stamps used only to decide rebuild; never authorize launch.
    stamps: Vec<ScopeStamp>,
}

#[derive(Debug, Clone)]
enum ScopeEntry {
    Ready(Box<ResolvedWorkflowDefinition>),
    /// Higher-scope invalid content must not fall back.
    Invalid {
        error: WorkflowError,
    },
    /// Two candidates claimed the same name in one scope.
    Conflict {
        detail: String,
    },
}

#[derive(Debug, Clone)]
struct ScopeStamp {
    origin: WorkflowSourceOrigin,
    root: PathBuf,
    /// `None` when the directory is absent (valid empty scope).
    dir_present: bool,
    dir_modified: Option<SystemTime>,
    /// Pair file stamps under the root (path relative + size + mtime).
    files: BTreeMap<PathBuf, FileStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

impl std::fmt::Debug for WorkflowDefinitionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowDefinitionRegistry")
            .finish_non_exhaustive()
    }
}

impl Default for WorkflowDefinitionRegistry {
    fn default() -> Self {
        Self::new(WorkflowDefinitionRegistryConfig {
            neo_home: PathBuf::new(),
            workspace: PathBuf::new(),
            project_trusted: false,
            limits: WorkflowLimits::default(),
            builtins: Vec::new(),
        })
    }
}

impl WorkflowDefinitionRegistry {
    /// Construct a registry. Does not scan until first resolve/list/refresh.
    #[must_use]
    pub fn new(config: WorkflowDefinitionRegistryConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RegistryInner {
                config,
                cache: None,
            })),
        }
    }

    /// Empty registry used by test fixtures that do not need discovery.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Registry projection seeds using the ordinary host-compiled built-ins.
    ///
    /// Callers still supply `neo_home` / `workspace` / trust / limits. Built-ins
    /// resolve through the same paired path as disk definitions (design §40).
    #[must_use]
    pub fn with_builtin_definitions(
        neo_home: PathBuf,
        workspace: PathBuf,
        project_trusted: bool,
        limits: WorkflowLimits,
    ) -> Self {
        Self::new(WorkflowDefinitionRegistryConfig {
            neo_home,
            workspace,
            project_trusted,
            limits,
            builtins: crate::workflow::builtins::builtin_workflow_definitions(),
        })
    }

    #[must_use]
    pub fn project_trusted(&self) -> bool {
        self.lock().config.project_trusted
    }

    #[must_use]
    pub fn limits(&self) -> WorkflowLimits {
        self.lock().config.limits.clone()
    }

    /// Replace the host trust / root configuration and drop the projection.
    pub fn reconfigure(&self, config: WorkflowDefinitionRegistryConfig) {
        let mut inner = self.lock();
        inner.config = config;
        inner.cache = None;
    }

    /// Drop the rebuildable projection so the next access rescans.
    pub fn invalidate(&self) {
        self.lock().cache = None;
    }

    /// Explicit refresh: recompute the projection from disk / builtins.
    pub fn refresh(&self) -> Result<(), WorkflowError> {
        let mut inner = self.lock();
        let projection = scan_all(&inner.config)?;
        inner.cache = Some(projection);
        Ok(())
    }

    /// Resolve a name under effective precedence.
    ///
    /// Higher-scope invalid or conflict entries fail closed without falling
    /// back to a lower scope.
    pub fn resolve(&self, name: &str) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
        let parsed = WorkflowName::parse(name.trim())?;
        let mut inner = self.lock();
        ensure_projection(&mut inner)?;
        let projection = inner.cache.as_ref().expect("projection after ensure");
        resolve_from_projection(projection, parsed.as_str())
    }

    /// List definitions for a scope filter (effective = precedence merge).
    pub fn list(
        &self,
        scope: WorkflowListScope,
    ) -> Result<Vec<RegistryDefinitionSummary>, WorkflowError> {
        let mut inner = self.lock();
        ensure_projection(&mut inner)?;
        let projection = inner.cache.as_ref().expect("projection after ensure");
        Ok(list_from_projection(projection, scope))
    }

    /// Pin the exact source snapshot a run must keep for its lifetime.
    #[must_use]
    pub fn pin_source(definition: &ResolvedWorkflowDefinition) -> WorkflowPinnedSource {
        pin_resolved_source(definition)
    }

    /// User-scope directory (`$NEO_HOME/workflows`).
    #[must_use]
    pub fn user_workflows_dir(neo_home: &Path) -> PathBuf {
        neo_home.join(USER_WORKFLOWS_DIR)
    }

    /// Project-scope directory (`<workspace>/.neo/workflows`).
    #[must_use]
    pub fn project_workflows_dir(workspace: &Path) -> PathBuf {
        workspace.join(PROJECT_WORKFLOWS_DIR)
    }

    /// Atomic no-clobber save of a paired definition into user or project scope.
    ///
    /// Writes and syncs the Lua source first, then the manifest last so a crash
    /// can never expose a mismatched pair as launchable. Project scope requires
    /// the current workspace trust decision.
    pub fn save(
        &self,
        scope: WorkflowSaveScope,
        request: &WorkflowSaveRequest,
        force: bool,
    ) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
        let name = WorkflowName::parse(request.name.trim())?;
        let (root, origin, trusted_ok) = {
            let inner = self.lock();
            match scope {
                WorkflowSaveScope::User => (
                    Self::user_workflows_dir(&inner.config.neo_home),
                    WorkflowSourceOrigin::User,
                    true,
                ),
                WorkflowSaveScope::Project => (
                    Self::project_workflows_dir(&inner.config.workspace),
                    WorkflowSourceOrigin::Project,
                    inner.config.project_trusted,
                ),
            }
        };
        if !trusted_ok {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::UntrustedProjectDefinition,
                "project workflow save requires a trusted workspace",
            ));
        }

        let source_bytes = request.lua_source.as_bytes();
        if source_bytes.is_empty() || request.lua_source.trim().is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                "Lua source must not be empty",
            ));
        }
        let source_sha = source_sha256_hex(source_bytes);
        let manifest_bytes = serialize_file_manifest_toml(
            &name,
            &request.display_name,
            &request.description,
            &request.phases,
            &source_sha,
            request.input_schema.as_ref(),
            &request.output_schema,
        )?;

        let limits = self.limits();
        let locator = Some(pair_locator(origin, &root, name.as_str()));
        // Validate before any write.
        let resolved = resolve_paired_definition(
            name.as_str(),
            &manifest_bytes,
            source_bytes,
            origin,
            locator.clone(),
            &limits,
        )?;

        let source_path = root.join(format!("{}{SOURCE_SUFFIX}", name.as_str()));
        let manifest_path = root.join(format!("{}{MANIFEST_SUFFIX}", name.as_str()));
        validate_save_target(&root, &source_path)?;
        validate_save_target(&root, &manifest_path)?;

        match evaluate_no_clobber(
            NoClobberPair {
                source_path: &source_path,
                manifest_path: &manifest_path,
                source_bytes,
                manifest_bytes: &manifest_bytes,
            },
            force,
            &limits,
            origin,
            locator.clone(),
            name.as_str(),
        )? {
            NoClobberDecision::Idempotent(existing) => {
                self.invalidate();
                return Ok(*existing);
            }
            NoClobberDecision::Write => {}
        }

        atomic_file::ensure_safe_directory_tree(&root).map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::Host,
                format!("cannot create workflow directory {}: {err}", root.display()),
            )
        })?;
        atomic_file::validate_safe_directory(&root).map_err(|err| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!("workflow directory unsafe {}: {err}", root.display()),
            )
        })?;

        // Source first, manifest last (design §11.4).
        write_pair_atomic(
            &source_path,
            source_bytes,
            &manifest_path,
            &manifest_bytes,
            force,
        )?;

        self.invalidate();
        // Re-resolve from the durable pair so the return value matches discovery.
        resolve_paired_definition(
            name.as_str(),
            &manifest_bytes,
            source_bytes,
            origin,
            locator,
            &limits,
        )
        .map(|_| resolved)
    }

    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Pin the exact source snapshot carried by a resolved definition.
#[must_use]
pub fn pin_resolved_source(definition: &ResolvedWorkflowDefinition) -> WorkflowPinnedSource {
    WorkflowPinnedSource {
        origin: definition.source_origin,
        name: definition.name.clone(),
        revision: definition.revision.clone(),
        source_locator: definition.source_locator.clone(),
        lua_source: definition.lua_source.clone(),
        source_sha256: definition.source_sha256.clone(),
    }
}

fn ensure_projection(inner: &mut RegistryInner) -> Result<(), WorkflowError> {
    if let Some(cache) = &inner.cache
        && projection_still_valid(cache, &inner.config)
    {
        return Ok(());
    }
    let projection = scan_all(&inner.config)?;
    inner.cache = Some(projection);
    Ok(())
}

fn projection_still_valid(
    cache: &RegistryProjection,
    config: &WorkflowDefinitionRegistryConfig,
) -> bool {
    let expected = expected_stamps(config);
    if expected.len() != cache.stamps.len() {
        return false;
    }
    for (left, right) in expected.iter().zip(cache.stamps.iter()) {
        if left.origin != right.origin
            || left.root != right.root
            || left.dir_present != right.dir_present
            || left.dir_modified != right.dir_modified
            || left.files != right.files
        {
            return false;
        }
    }
    true
}

fn expected_stamps(config: &WorkflowDefinitionRegistryConfig) -> Vec<ScopeStamp> {
    let mut stamps = Vec::with_capacity(3);
    // Builtins have no filesystem stamp; empty stamp with origin only.
    stamps.push(ScopeStamp {
        origin: WorkflowSourceOrigin::Builtin,
        root: PathBuf::from("builtin://"),
        dir_present: true,
        dir_modified: None,
        files: BTreeMap::new(),
    });
    stamps.push(stamp_scope_dir(
        WorkflowSourceOrigin::User,
        &WorkflowDefinitionRegistry::user_workflows_dir(&config.neo_home),
    ));
    if config.project_trusted {
        stamps.push(stamp_scope_dir(
            WorkflowSourceOrigin::Project,
            &WorkflowDefinitionRegistry::project_workflows_dir(&config.workspace),
        ));
    }
    stamps
}

fn stamp_scope_dir(origin: WorkflowSourceOrigin, root: &Path) -> ScopeStamp {
    match fs::symlink_metadata(root) {
        Ok(meta) if meta.is_dir() && !atomic_file::is_reparse_or_symlink(&meta) => {
            let mut files = BTreeMap::new();
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        continue;
                    };
                    if atomic_file::is_reparse_or_symlink(&meta) || !meta.is_file() {
                        continue;
                    }
                    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                        continue;
                    };
                    if !(name.ends_with(MANIFEST_SUFFIX) || name.ends_with(SOURCE_SUFFIX)) {
                        continue;
                    }
                    files.insert(
                        PathBuf::from(name),
                        FileStamp {
                            len: meta.len(),
                            modified: meta.modified().ok(),
                        },
                    );
                }
            }
            ScopeStamp {
                origin,
                root: root.to_path_buf(),
                dir_present: true,
                dir_modified: meta.modified().ok(),
                files,
            }
        }
        Ok(_) => ScopeStamp {
            origin,
            root: root.to_path_buf(),
            dir_present: false,
            dir_modified: None,
            files: BTreeMap::new(),
        },
        Err(_) => ScopeStamp {
            origin,
            root: root.to_path_buf(),
            dir_present: false,
            dir_modified: None,
            files: BTreeMap::new(),
        },
    }
}

fn scan_all(
    config: &WorkflowDefinitionRegistryConfig,
) -> Result<RegistryProjection, WorkflowError> {
    let project = if config.project_trusted {
        scan_directory_scope(
            &WorkflowDefinitionRegistry::project_workflows_dir(&config.workspace),
            WorkflowSourceOrigin::Project,
            &config.limits,
        )
    } else {
        // Untrusted / disabled project discovery produces no project candidates.
        BTreeMap::new()
    };
    let scopes = ScopeMaps {
        builtin: scan_builtins(&config.builtins, &config.limits),
        user: scan_directory_scope(
            &WorkflowDefinitionRegistry::user_workflows_dir(&config.neo_home),
            WorkflowSourceOrigin::User,
            &config.limits,
        ),
        project,
    };
    Ok(RegistryProjection {
        scopes,
        stamps: expected_stamps(config),
    })
}

fn scan_builtins(
    builtins: &[BuiltinWorkflowDefinition],
    limits: &WorkflowLimits,
) -> BTreeMap<String, ScopeEntry> {
    let mut out: BTreeMap<String, ScopeEntry> = BTreeMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for builtin in builtins {
        let key = builtin.name.trim().to_owned();
        *seen.entry(key.clone()).or_insert(0) += 1;
        if seen.get(&key).copied().unwrap_or(0) > 1 {
            out.insert(
                key.clone(),
                ScopeEntry::Conflict {
                    detail: format!("duplicate builtin definition name `{key}`"),
                },
            );
            continue;
        }
        let entry = match WorkflowName::parse(&key) {
            Ok(name) => match resolve_paired_definition(
                name.as_str(),
                &builtin.manifest_bytes,
                &builtin.source_bytes,
                WorkflowSourceOrigin::Builtin,
                Some(format!("builtin://{}", name.as_str())),
                limits,
            ) {
                Ok(resolved) => ScopeEntry::Ready(Box::new(resolved)),
                Err(error) => ScopeEntry::Invalid { error },
            },
            Err(error) => ScopeEntry::Invalid { error },
        };
        out.insert(key, entry);
    }
    // Mark all names that appeared more than once as conflicts (overwrite Ready).
    for (name, count) in seen {
        if count > 1 {
            out.insert(
                name.clone(),
                ScopeEntry::Conflict {
                    detail: format!("duplicate builtin definition name `{name}`"),
                },
            );
        }
    }
    out
}

fn scan_directory_scope(
    root: &Path,
    origin: WorkflowSourceOrigin,
    limits: &WorkflowLimits,
) -> BTreeMap<String, ScopeEntry> {
    let mut out: BTreeMap<String, ScopeEntry> = BTreeMap::new();

    let meta = match fs::symlink_metadata(root) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return out,
        Err(err) => {
            // Directory unreadable: surface as empty with no candidates (list
            // remains usable). Individual resolve of names stays not-found.
            let _ = err;
            return out;
        }
    };
    if atomic_file::is_reparse_or_symlink(&meta) {
        // Do not follow directory links.
        return out;
    }
    if !meta.is_dir() {
        return out;
    }

    let Ok(entries) = fs::read_dir(root) else {
        return out;
    };

    // stem → (manifest_path?, source_path?)
    let mut pairs: BTreeMap<String, (Option<PathBuf>, Option<PathBuf>)> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        // Reject symlink/reparse definition files; do not follow.
        if atomic_file::is_reparse_or_symlink(&meta) {
            if let Some(stem) = pair_stem_from_path(&path) {
                out.insert(
                    stem,
                    ScopeEntry::Invalid {
                        error: WorkflowError::coded(
                            WorkflowErrorCode::InvalidDefinition,
                            format!("refusing symlinked definition file {}", path.display()),
                        ),
                    },
                );
            }
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(stem) = file_name.strip_suffix(MANIFEST_SUFFIX) {
            if !is_exact_suffix_file(file_name, MANIFEST_SUFFIX) {
                continue;
            }
            let slot = pairs.entry(stem.to_owned()).or_insert((None, None));
            if slot.0.is_some() {
                out.insert(
                    stem.to_owned(),
                    ScopeEntry::Conflict {
                        detail: format!("duplicate manifest for `{stem}` in {}", root.display()),
                    },
                );
            } else {
                slot.0 = Some(path);
            }
        } else if let Some(stem) = file_name.strip_suffix(SOURCE_SUFFIX) {
            if !is_exact_suffix_file(file_name, SOURCE_SUFFIX) {
                continue;
            }
            // Avoid treating `foo.workflow.toml` as a `.lua` (suffix check order
            // already preferred longer manifest suffix above).
            if file_name.ends_with(MANIFEST_SUFFIX) {
                continue;
            }
            let slot = pairs.entry(stem.to_owned()).or_insert((None, None));
            if slot.1.is_some() {
                out.insert(
                    stem.to_owned(),
                    ScopeEntry::Conflict {
                        detail: format!("duplicate source for `{stem}` in {}", root.display()),
                    },
                );
            } else {
                slot.1 = Some(path);
            }
        }
    }

    for (stem, (manifest_path, source_path)) in pairs {
        if out.contains_key(&stem) {
            // Already recorded as conflict or symlink invalid.
            continue;
        }
        if WorkflowName::parse(&stem).is_err() {
            // Non-portable stems are not registry names.
            continue;
        }
        let entry = match (manifest_path, source_path) {
            (Some(manifest_path), Some(source_path)) => {
                load_pair_entry(&stem, &manifest_path, &source_path, origin, root, limits)
            }
            (Some(manifest_path), None) => ScopeEntry::Invalid {
                error: WorkflowError::coded(
                    WorkflowErrorCode::InvalidDefinition,
                    format!(
                        "incomplete definition pair for `{stem}`: missing {stem}{SOURCE_SUFFIX} next to {}",
                        manifest_path.display()
                    ),
                ),
            },
            (None, Some(source_path)) => ScopeEntry::Invalid {
                error: WorkflowError::coded(
                    WorkflowErrorCode::InvalidDefinition,
                    format!(
                        "incomplete definition pair for `{stem}`: missing {stem}{MANIFEST_SUFFIX} next to {}",
                        source_path.display()
                    ),
                ),
            },
            (None, None) => continue,
        };
        out.insert(stem, entry);
    }

    out
}

fn is_exact_suffix_file(file_name: &str, suffix: &str) -> bool {
    file_name.len() > suffix.len() && file_name.ends_with(suffix)
}

fn pair_stem_from_path(path: &Path) -> Option<String> {
    let file_name = path.file_name()?.to_str()?;
    if let Some(stem) = file_name.strip_suffix(MANIFEST_SUFFIX) {
        return Some(stem.to_owned());
    }
    if let Some(stem) = file_name.strip_suffix(SOURCE_SUFFIX)
        && !file_name.ends_with(MANIFEST_SUFFIX)
    {
        return Some(stem.to_owned());
    }
    None
}

fn load_pair_entry(
    stem: &str,
    manifest_path: &Path,
    source_path: &Path,
    origin: WorkflowSourceOrigin,
    root: &Path,
    limits: &WorkflowLimits,
) -> ScopeEntry {
    // Reject parent escapes: both paths must live directly under root.
    if let Err(error) = validate_pair_under_root(root, manifest_path, source_path) {
        return ScopeEntry::Invalid { error };
    }

    let manifest_bytes = match read_regular_file_capped(manifest_path, limits.manifest_bytes) {
        Ok(bytes) => bytes,
        Err(error) => return ScopeEntry::Invalid { error },
    };
    let source_bytes = match read_regular_file_capped(source_path, limits.lua_source_bytes) {
        Ok(bytes) => bytes,
        Err(error) => return ScopeEntry::Invalid { error },
    };

    let locator = Some(pair_locator(origin, root, stem));
    match resolve_paired_definition(
        stem,
        &manifest_bytes,
        &source_bytes,
        origin,
        locator,
        limits,
    ) {
        Ok(resolved) => ScopeEntry::Ready(Box::new(resolved)),
        Err(error) => ScopeEntry::Invalid { error },
    }
}

fn validate_pair_under_root(
    root: &Path,
    manifest_path: &Path,
    source_path: &Path,
) -> Result<(), WorkflowError> {
    for path in [manifest_path, source_path] {
        let parent = path.parent().ok_or_else(|| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!("definition path has no parent: {}", path.display()),
            )
        })?;
        if !paths_same_dir(parent, root) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!(
                    "definition path escapes workflow directory: {}",
                    path.display()
                ),
            ));
        }
        // Reject `..` components in the relative name.
        if path_has_parent_escape(path) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!("definition path contains parent escape: {}", path.display()),
            ));
        }
    }
    Ok(())
}

fn paths_same_dir(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => a == b,
    }
}

fn path_has_parent_escape(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

fn read_regular_file_capped(path: &Path, limit: u64) -> Result<Vec<u8>, WorkflowError> {
    let meta = fs::symlink_metadata(path).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("cannot stat {}: {err}", path.display()),
        )
    })?;
    if atomic_file::is_reparse_or_symlink(&meta) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("refusing symlinked definition file {}", path.display()),
        ));
    }
    if !meta.is_file() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("definition path is not a regular file: {}", path.display()),
        ));
    }
    if meta.len() > limit {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!(
                "definition file {} size {} exceeds limit {limit}",
                path.display(),
                meta.len()
            ),
        ));
    }
    fs::read(path).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("cannot read {}: {err}", path.display()),
        )
    })
}

fn pair_locator(origin: WorkflowSourceOrigin, root: &Path, name: &str) -> String {
    match origin {
        WorkflowSourceOrigin::Builtin => format!("builtin://{name}"),
        WorkflowSourceOrigin::User | WorkflowSourceOrigin::Project => root
            .join(format!("{name}{MANIFEST_SUFFIX}"))
            .display()
            .to_string(),
        WorkflowSourceOrigin::Dynamic => format!("dynamic://{name}"),
    }
}

fn resolve_from_projection(
    projection: &RegistryProjection,
    name: &str,
) -> Result<ResolvedWorkflowDefinition, WorkflowError> {
    // Precedence: project > user > builtin (higher shadows lower).
    for origin in [
        WorkflowSourceOrigin::Project,
        WorkflowSourceOrigin::User,
        WorkflowSourceOrigin::Builtin,
    ] {
        let scope_map = projection.scopes.get(origin);
        if let Some(entry) = scope_map.get(name) {
            return match entry {
                ScopeEntry::Ready(resolved) => Ok(resolved.as_ref().clone()),
                ScopeEntry::Invalid { error } => Err(error.clone()),
                ScopeEntry::Conflict { detail } => Err(WorkflowError::coded(
                    WorkflowErrorCode::DefinitionConflict,
                    detail.clone(),
                )),
            };
        }
    }
    Err(WorkflowError::coded(
        WorkflowErrorCode::DefinitionNotFound,
        format!("workflow definition `{name}` not found"),
    ))
}

fn list_from_projection(
    projection: &RegistryProjection,
    scope: WorkflowListScope,
) -> Vec<RegistryDefinitionSummary> {
    match scope {
        WorkflowListScope::Builtin => list_scope(projection, WorkflowSourceOrigin::Builtin),
        WorkflowListScope::User => list_scope(projection, WorkflowSourceOrigin::User),
        WorkflowListScope::Project => list_scope(projection, WorkflowSourceOrigin::Project),
        WorkflowListScope::Effective => {
            let mut effective: BTreeMap<String, RegistryDefinitionSummary> = BTreeMap::new();
            // Collect every name that appears in any scope, then apply resolve
            // precedence so invalid/conflict higher scopes never surface a
            // lower-scope ready definition as effective.
            let mut names = BTreeMap::<String, ()>::new();
            for origin in [
                WorkflowSourceOrigin::Builtin,
                WorkflowSourceOrigin::User,
                WorkflowSourceOrigin::Project,
            ] {
                for name in projection.scopes.get(origin).keys() {
                    names.insert(name.clone(), ());
                }
            }
            for name in names.keys() {
                if let Ok(resolved) = resolve_from_projection(projection, name) {
                    effective.insert(
                        name.clone(),
                        RegistryDefinitionSummary {
                            name: resolved.name.clone(),
                            display_name: resolved.display_name.clone(),
                            description: resolved.description.clone(),
                            revision: resolved.revision.clone(),
                            source_origin: resolved.source_origin,
                            source_locator: resolved.source_locator.clone(),
                        },
                    );
                } else {
                    // Not effectively launchable (missing, invalid higher, conflict).
                }
            }
            effective.into_values().collect()
        }
    }
}

fn list_scope(
    projection: &RegistryProjection,
    origin: WorkflowSourceOrigin,
) -> Vec<RegistryDefinitionSummary> {
    let scope_map = projection.scopes.get(origin);
    let mut out = Vec::new();
    for entry in scope_map.values() {
        if let ScopeEntry::Ready(resolved) = entry {
            out.push(RegistryDefinitionSummary {
                name: resolved.name.clone(),
                display_name: resolved.display_name.clone(),
                description: resolved.description.clone(),
                revision: resolved.revision.clone(),
                source_origin: resolved.source_origin,
                source_locator: resolved.source_locator.clone(),
            });
        }
    }
    out.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    out
}

enum NoClobberDecision {
    Write,
    Idempotent(Box<ResolvedWorkflowDefinition>),
}

struct NoClobberPair<'a> {
    source_path: &'a Path,
    manifest_path: &'a Path,
    source_bytes: &'a [u8],
    manifest_bytes: &'a [u8],
}

fn evaluate_no_clobber(
    pair: NoClobberPair<'_>,
    force: bool,
    limits: &WorkflowLimits,
    origin: WorkflowSourceOrigin,
    locator: Option<String>,
    name: &str,
) -> Result<NoClobberDecision, WorkflowError> {
    let source_exists = path_exists_as_any(pair.source_path)?;
    let manifest_exists = path_exists_as_any(pair.manifest_path)?;
    if !source_exists && !manifest_exists {
        return Ok(NoClobberDecision::Write);
    }

    // Existing content: compare exact pair bytes when both present and regular.
    if source_exists && manifest_exists {
        let existing_source = read_if_regular(pair.source_path)?;
        let existing_manifest = read_if_regular(pair.manifest_path)?;
        if let (Some(existing_source), Some(existing_manifest)) =
            (existing_source, existing_manifest)
            && existing_source == pair.source_bytes
            && existing_manifest == pair.manifest_bytes
        {
            let existing = resolve_paired_definition(
                name,
                pair.manifest_bytes,
                pair.source_bytes,
                origin,
                locator,
                limits,
            )?;
            return Ok(NoClobberDecision::Idempotent(Box::new(existing)));
        }
    }

    if force {
        return Ok(NoClobberDecision::Write);
    }

    Err(WorkflowError::coded(
        WorkflowErrorCode::DefinitionConflict,
        format!("workflow definition `{name}` already exists; pass force to overwrite"),
    ))
}

fn path_exists_as_any(path: &Path) -> Result<bool, WorkflowError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(WorkflowError::coded(
            WorkflowErrorCode::Host,
            format!("cannot stat {}: {err}", path.display()),
        )),
    }
}

fn read_if_regular(path: &Path) -> Result<Option<Vec<u8>>, WorkflowError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::Host,
                format!("cannot stat {}: {err}", path.display()),
            ));
        }
    };
    if atomic_file::is_reparse_or_symlink(&meta) || !meta.is_file() {
        return Ok(None);
    }
    fs::read(path).map(Some).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::Host,
            format!("cannot read {}: {err}", path.display()),
        )
    })
}

fn validate_save_target(root: &Path, target: &Path) -> Result<(), WorkflowError> {
    if path_has_parent_escape(target) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("save path contains parent escape: {}", target.display()),
        ));
    }
    let parent = target.parent().ok_or_else(|| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("save path has no parent: {}", target.display()),
        )
    })?;
    // Target must be exactly root/<filename> with a single normal component.
    let file_name = target.file_name().ok_or_else(|| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("save path has no file name: {}", target.display()),
        )
    })?;
    if file_name.to_string_lossy().contains(['/', '\\']) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("save path file name is not portable: {}", target.display()),
        ));
    }
    if parent != root {
        // Allow when root does not yet exist (will be created) — compare components.
        let expected = root.join(file_name);
        if expected != target {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidDefinition,
                format!("save path escapes workflow directory: {}", target.display()),
            ));
        }
    }
    // Reject existing symlink/reparse at the target.
    atomic_file::reject_reparse_or_symlink_if_present(target).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidDefinition,
            format!("refusing symlinked save target {}: {err}", target.display()),
        )
    })?;
    Ok(())
}

fn write_pair_atomic(
    source_path: &Path,
    source_bytes: &[u8],
    manifest_path: &Path,
    manifest_bytes: &[u8],
    force: bool,
) -> Result<(), WorkflowError> {
    // Source first.
    write_one_atomic(source_path, source_bytes, force)?;
    // Manifest last (content hash gate for discovery).
    // Best-effort: leave source in place; discovery will reject incomplete
    // or hash-mismatched pairs. Do not roll back via destructive delete.
    write_one_atomic(manifest_path, manifest_bytes, force)
}

fn write_one_atomic(path: &Path, bytes: &[u8], force: bool) -> Result<(), WorkflowError> {
    let result = if force {
        // Overwrite path (create or replace) via atomic rename.
        atomic_file::write_file_atomic(path, bytes)
    } else if path_exists_as_any(path)? {
        // No-clobber path should have been rejected earlier; still refuse.
        return Err(WorkflowError::coded(
            WorkflowErrorCode::DefinitionConflict,
            format!("path already exists: {}", path.display()),
        ));
    } else {
        match atomic_file::write_file_atomic_create_new(path, bytes) {
            Ok(
                atomic_file::AtomicWriteStatus::Durable
                | atomic_file::AtomicWriteStatus::CommittedUnsynced(_),
            ) => Ok(()),
            Err(err) => Err(err),
        }
    };
    result.map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::Host,
            format!("atomic write failed for {}: {err}", path.display()),
        )
    })
}

/// Serialize a file-backed TOML manifest for save.
fn serialize_file_manifest_toml(
    name: &WorkflowName,
    display_name: &str,
    description: &str,
    phases: &[WorkflowPhase],
    source_sha256: &str,
    input_schema: Option<&serde_json::Value>,
    output_schema: &serde_json::Value,
) -> Result<Vec<u8>, WorkflowError> {
    let display_name = display_name.trim();
    let description = description.trim();
    if display_name.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "display_name must not be empty",
        ));
    }
    if description.is_empty() {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "description must not be empty",
        ));
    }

    let mut table = toml::map::Map::new();
    table.insert(
        "definition_format_version".to_owned(),
        toml::Value::Integer(i64::from(DEFINITION_FORMAT_VERSION)),
    );
    table.insert(
        "name".to_owned(),
        toml::Value::String(name.as_str().to_owned()),
    );
    table.insert(
        "display_name".to_owned(),
        toml::Value::String(display_name.to_owned()),
    );
    table.insert(
        "description".to_owned(),
        toml::Value::String(description.to_owned()),
    );
    table.insert(
        "source_sha256".to_owned(),
        toml::Value::String(source_sha256.to_owned()),
    );

    let mut phase_values = Vec::with_capacity(phases.len());
    for phase in phases {
        let mut phase_table = toml::map::Map::new();
        phase_table.insert("id".to_owned(), toml::Value::String(phase.id.clone()));
        phase_table.insert(
            "description".to_owned(),
            toml::Value::String(phase.description.clone()),
        );
        phase_values.push(toml::Value::Table(phase_table));
    }
    table.insert("phases".to_owned(), toml::Value::Array(phase_values));
    table.insert(
        "output_schema".to_owned(),
        json_to_toml_value(output_schema)?,
    );
    if let Some(schema) = input_schema {
        table.insert("input_schema".to_owned(), json_to_toml_value(schema)?);
    }

    let doc = toml::Value::Table(table);
    let text = toml::to_string_pretty(&doc).map_err(|err| {
        WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            format!("manifest TOML encode failed: {err}"),
        )
    })?;
    Ok(text.into_bytes())
}

fn json_to_toml_value(value: &serde_json::Value) -> Result<toml::Value, WorkflowError> {
    match value {
        serde_json::Value::Null => Err(WorkflowError::coded(
            WorkflowErrorCode::InvalidManifest,
            "JSON null cannot be represented in workflow TOML manifests",
        )),
        serde_json::Value::Bool(b) => Ok(toml::Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(toml::Value::Integer(i))
            } else if let Some(u) = n.as_u64() {
                i64::try_from(u).map(toml::Value::Integer).map_err(|_| {
                    WorkflowError::coded(
                        WorkflowErrorCode::InvalidManifest,
                        "JSON number exceeds TOML integer range",
                    )
                })
            } else if let Some(f) = n.as_f64() {
                Ok(toml::Value::Float(f))
            } else {
                Err(WorkflowError::coded(
                    WorkflowErrorCode::InvalidManifest,
                    "unsupported JSON number in schema",
                ))
            }
        }
        serde_json::Value::String(s) => Ok(toml::Value::String(s.clone())),
        serde_json::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(json_to_toml_value(item)?);
            }
            Ok(toml::Value::Array(out))
        }
        serde_json::Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (key, item) in map {
                table.insert(key.clone(), json_to_toml_value(item)?);
            }
            Ok(toml::Value::Table(table))
        }
    }
}
