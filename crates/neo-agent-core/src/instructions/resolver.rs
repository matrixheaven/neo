//! Canonical scope discovery, Markdown instruction imports, and stable source
//! reads for the path-scoped instruction engine.
//!
//! The resolver is deterministic and side-effect-free after filesystem
//! reads. It never caches results across calls, so newly created
//! `AGENTS.md` files are discovered promptly.

use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use pulldown_cmark::{Event, Parser, Tag};
use sha2::{Digest, Sha256};

use super::types::{
    InstructionError, InstructionRegistryConfig, InstructionScopeKind, MAX_GRAPH_BYTES,
    MAX_GRAPH_SOURCES, MAX_IMPORT_DEPTH, MAX_SOURCE_BYTES,
};
use crate::runtime::estimate_text_tokens;

/// Portable file identity used by the bounded stability check. Content hash
/// remains the final source identity; metadata is only a fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceMetadata {
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub is_file: bool,
}

/// Filesystem access seam for source reads. Production uses
/// [`FilesystemSourceIo`]; tests can script failures deterministically.
pub trait SourceIo: Send + Sync + std::fmt::Debug {
    /// Reads portable metadata for `path`.
    ///
    /// # Errors
    /// Returns the underlying I/O error.
    fn read_metadata(&self, path: &Path) -> io::Result<SourceMetadata>;
    /// Reads the complete bytes of `path`.
    ///
    /// # Errors
    /// Returns the underlying I/O error.
    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// Reads at most `max_bytes` from `path`.
    ///
    /// Custom test/source implementations may rely on this default adapter;
    /// production filesystem I/O overrides it to avoid an unbounded allocation.
    ///
    /// # Errors
    /// Returns the underlying I/O error.
    fn read_bytes_bounded(&self, path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
        let mut bytes = self.read_bytes(path)?;
        bytes.truncate(max_bytes);
        Ok(bytes)
    }
}

/// Default [`SourceIo`] backed by `std::fs`.
#[derive(Debug, Default, Clone, Copy)]
pub struct FilesystemSourceIo;

impl SourceIo for FilesystemSourceIo {
    fn read_metadata(&self, path: &Path) -> io::Result<SourceMetadata> {
        let metadata = std::fs::metadata(path)?;
        Ok(SourceMetadata {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            is_file: metadata.is_file(),
        })
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn read_bytes_bounded(&self, path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
        let file = std::fs::File::open(path)?;
        let mut reader = file.take(u64::try_from(max_bytes).unwrap_or(u64::MAX));
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

/// Reads a source with the bounded stability check:
/// metadata A -> bytes -> metadata B, retry once on change, then fail with
/// [`InstructionError::UnstableSource`].
///
/// # Errors
/// Returns [`InstructionError::UnreadableSource`] on I/O failure and
/// [`InstructionError::UnstableSource`] when the file changes twice.
pub fn read_source_stable(io: &dyn SourceIo, path: &Path) -> Result<Vec<u8>, InstructionError> {
    fn unreadable(path: &Path) -> impl Fn(io::Error) -> InstructionError + '_ {
        move |error| InstructionError::UnreadableSource {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    }
    let limit_error = || {
        InstructionError::LimitExceeded(format!(
            "source `{}` exceeds {MAX_SOURCE_BYTES} bytes",
            path.display()
        ))
    };
    let read_limit = usize::try_from(MAX_SOURCE_BYTES.saturating_add(1)).unwrap_or(usize::MAX);
    let attempt =
        |io: &dyn SourceIo| -> Result<(SourceMetadata, Vec<u8>, SourceMetadata), InstructionError> {
            let before = io.read_metadata(path).map_err(unreadable(path))?;
            if before.len > MAX_SOURCE_BYTES {
                return Err(limit_error());
            }
            let bytes = io
                .read_bytes_bounded(path, read_limit)
                .map_err(unreadable(path))?;
            let after = io.read_metadata(path).map_err(unreadable(path))?;
            Ok((before, bytes, after))
        };
    let (before, bytes, after) = attempt(io)?;
    if before == after {
        return if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SOURCE_BYTES {
            Err(limit_error())
        } else {
            Ok(bytes)
        };
    }
    let (retry_before, retry_bytes, retry_after) = attempt(io)?;
    if retry_before == retry_after {
        if u64::try_from(retry_bytes.len()).unwrap_or(u64::MAX) > MAX_SOURCE_BYTES {
            Err(limit_error())
        } else {
            Ok(retry_bytes)
        }
    } else {
        Err(InstructionError::UnstableSource {
            path: path.to_path_buf(),
        })
    }
}

/// Selects the exact `AGENTS.md` directory entry, if present.
#[must_use]
pub fn select_agents_file_name(entry_names: &[OsString]) -> Option<OsString> {
    entry_names
        .iter()
        .find(|name| name.as_os_str() == OsStr::new("AGENTS.md"))
        .cloned()
}

/// Finds the exact `AGENTS.md` entry in `directory`, if any.
///
/// A missing directory yields `Ok(None)` — no scope exists there, which is
/// not an error. File contents are never read.
///
/// # Errors
/// Returns [`InstructionError::Io`] on directory-read failures other than
/// `NotFound`.
pub fn find_agents_file(directory: &Path) -> Result<Option<PathBuf>, InstructionError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstructionError::Io(error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        names.push(entry?.file_name());
    }
    Ok(select_agents_file_name(&names).map(|name| directory.join(name)))
}

/// The applicable scope set for one batch, canonicalized and deduplicated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedScopes {
    /// `$NEO_HOME` scope directory, when it contains an AGENTS.md.
    pub global: Option<PathBuf>,
    /// Trusted ancestors above the primary workspace, outermost to nearest.
    pub ancestors: Vec<PathBuf>,
    /// Primary workspace root, when it contains an AGENTS.md.
    pub workspace_root: Option<PathBuf>,
    /// Nested scopes inside the workspace, shallowest to deepest.
    pub nested: Vec<PathBuf>,
}

impl ResolvedScopes {
    /// True when no scope directory contains an AGENTS.md.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.global.is_none()
            && self.ancestors.is_empty()
            && self.workspace_root.is_none()
            && self.nested.is_empty()
    }

    /// Admission priority order: global -> workspace root -> nested deepest
    /// to shallowest -> ancestors nearest first.
    #[must_use]
    pub fn admission_order(&self) -> Vec<(InstructionScopeKind, PathBuf)> {
        let mut ordered = Vec::new();
        if let Some(global) = &self.global {
            ordered.push((InstructionScopeKind::Global, global.clone()));
        }
        if let Some(root) = &self.workspace_root {
            ordered.push((InstructionScopeKind::WorkspaceRoot, root.clone()));
        }
        ordered.extend(
            self.nested
                .iter()
                .rev()
                .map(|dir| (InstructionScopeKind::Nested, dir.clone())),
        );
        ordered.extend(
            self.ancestors
                .iter()
                .rev()
                .map(|dir| (InstructionScopeKind::Ancestor, dir.clone())),
        );
        ordered
    }

    /// Model rendering order: global -> ancestors outermost to nearest ->
    /// workspace root -> nested shallowest to deepest.
    #[must_use]
    pub fn rendering_order(&self) -> Vec<(InstructionScopeKind, PathBuf)> {
        let mut ordered = Vec::new();
        if let Some(global) = &self.global {
            ordered.push((InstructionScopeKind::Global, global.clone()));
        }
        ordered.extend(
            self.ancestors
                .iter()
                .map(|dir| (InstructionScopeKind::Ancestor, dir.clone())),
        );
        if let Some(root) = &self.workspace_root {
            ordered.push((InstructionScopeKind::WorkspaceRoot, root.clone()));
        }
        ordered.extend(
            self.nested
                .iter()
                .map(|dir| (InstructionScopeKind::Nested, dir.clone())),
        );
        ordered
    }

    /// Every scope directory in rendering order.
    #[must_use]
    pub fn all_scope_dirs(&self) -> Vec<PathBuf> {
        self.rendering_order()
            .into_iter()
            .map(|(_, dir)| dir)
            .collect()
    }
}

/// One fully expanded, content-addressed instruction bundle.
#[derive(Debug, Clone)]
pub struct InstructionBundle {
    /// Canonical directory containing the root AGENTS.md.
    pub scope_dir: PathBuf,
    /// Canonical path of the root AGENTS.md.
    pub source_path: PathBuf,
    /// SHA-256 hex of the expanded content; the bundle revision.
    pub revision: String,
    /// Complete expanded UTF-8 text with provenance wrappers.
    pub expanded: String,
    /// Canonical paths of every source in first-expansion order.
    pub sources: Vec<PathBuf>,
    /// Logical source paths paired with their canonical targets. Cache
    /// validation uses these identities to detect symlink retargeting.
    pub(crate) source_identities: Vec<(PathBuf, PathBuf)>,
    /// Combined raw byte size of all sources in the graph.
    pub byte_size: u64,
    /// Token estimate of the expanded content.
    pub token_estimate: u64,
}

impl InstructionBundle {
    /// Number of sources in the import graph, root included.
    #[must_use]
    pub fn source_count(&self) -> u32 {
        u32::try_from(self.sources.len()).unwrap_or(u32::MAX)
    }

    /// Number of imports in the graph.
    #[must_use]
    pub fn import_count(&self) -> u32 {
        self.source_count().saturating_sub(1)
    }
}

/// Deterministic, side-effect-free resolver for scope chains and bundles.
#[derive(Debug)]
pub struct InstructionResolver {
    canonical_workspace: PathBuf,
    canonical_neo_home: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    project_trusted: bool,
    source_io: Arc<dyn SourceIo>,
}

impl InstructionResolver {
    /// Builds a resolver using the platform home directory for `~` imports.
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when the primary workspace cannot be
    /// canonicalized.
    pub fn new(config: &InstructionRegistryConfig) -> Result<Self, InstructionError> {
        Self::build(config, std::env::home_dir(), Arc::new(FilesystemSourceIo))
    }

    /// Builds a resolver with an explicit home directory (tests, sandboxes).
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when the primary workspace cannot be
    /// canonicalized.
    pub fn with_home(
        config: &InstructionRegistryConfig,
        home_dir: PathBuf,
    ) -> Result<Self, InstructionError> {
        Self::build(config, Some(home_dir), Arc::new(FilesystemSourceIo))
    }

    /// Builds a resolver with explicit home and source I/O implementations.
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when the primary workspace cannot be
    /// canonicalized.
    pub fn with_source_io(
        config: &InstructionRegistryConfig,
        home_dir: Option<PathBuf>,
        source_io: Arc<dyn SourceIo>,
    ) -> Result<Self, InstructionError> {
        Self::build(config, home_dir, source_io)
    }

    fn build(
        config: &InstructionRegistryConfig,
        home_dir: Option<PathBuf>,
        source_io: Arc<dyn SourceIo>,
    ) -> Result<Self, InstructionError> {
        let canonical_workspace = config.primary_workspace.canonicalize()?;
        let canonical_neo_home = config
            .neo_home
            .as_ref()
            .map(|home| home.canonicalize().unwrap_or_else(|_| home.clone()));
        Ok(Self {
            canonical_workspace,
            canonical_neo_home,
            home_dir: home_dir.map(|home| home.canonicalize().unwrap_or(home)),
            project_trusted: config.project_trusted,
            source_io,
        })
    }

    /// Canonical primary workspace root.
    #[must_use]
    pub fn canonical_workspace(&self) -> &Path {
        &self.canonical_workspace
    }

    /// Canonical `$NEO_HOME` root, when configured.
    #[must_use]
    pub fn canonical_neo_home(&self) -> Option<&Path> {
        self.canonical_neo_home.as_deref()
    }

    /// True when `path` (already canonical) stays inside an allowed root.
    #[must_use]
    pub fn is_contained(&self, path: &Path) -> bool {
        path.starts_with(&self.canonical_workspace)
            || self
                .canonical_neo_home
                .as_ref()
                .is_some_and(|home| path.starts_with(home))
    }

    fn is_allowed_global_path(&self, path: &Path) -> bool {
        if path.starts_with(&self.canonical_workspace) {
            return self.project_trusted;
        }
        self.canonical_neo_home
            .as_ref()
            .is_some_and(|home| path.starts_with(home))
            || self
                .home_dir
                .as_ref()
                .is_some_and(|home| path.starts_with(home))
    }

    /// Model-facing display path: redacts the platform home as `~`.
    #[must_use]
    pub fn display_for_model(&self, path: &Path) -> PathBuf {
        if let Some(home) = &self.home_dir
            && let Ok(relative) = path.strip_prefix(home)
        {
            return Path::new("~").join(relative);
        }
        path.to_path_buf()
    }

    /// Discovers the applicable scope chain for `target_directories`.
    ///
    /// Only the primary-workspace-to-target chains are scanned — never
    /// siblings or descendants. Targets outside the workspace trigger no
    /// discovery. Missing directories are simply absent scopes; directory
    /// results are memoized only within this call and missing AGENTS.md
    /// results are never cached across calls.
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when a scanned directory cannot be
    /// read.
    pub fn discover_scopes(
        &self,
        target_directories: &[PathBuf],
    ) -> Result<ResolvedScopes, InstructionError> {
        let mut probes: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
        let probe = |dir: &Path,
                     probes: &mut HashMap<PathBuf, Option<PathBuf>>|
         -> Result<Option<PathBuf>, InstructionError> {
            if let Some(cached) = probes.get(dir) {
                return Ok(cached.clone());
            }
            let found = find_agents_file(dir)?;
            probes.insert(dir.to_path_buf(), found.clone());
            Ok(found)
        };

        let mut resolved = ResolvedScopes::default();
        if let Some(home) = &self.canonical_neo_home
            && probe(home, &mut probes)?.is_some()
        {
            resolved.global = Some(home.clone());
        }
        if self.project_trusted {
            if probe(&self.canonical_workspace.clone(), &mut probes)?.is_some() {
                resolved.workspace_root = Some(self.canonical_workspace.clone());
            }

            // Trusted ancestors above the workspace, nearest first from
            // `ancestors()`, reversed to outermost-to-nearest.
            let mut ancestors = Vec::new();
            for dir in self.canonical_workspace.ancestors().skip(1) {
                if probe(dir, &mut probes)?.is_some() {
                    ancestors.push(dir.to_path_buf());
                }
            }
            ancestors.reverse();
            resolved.ancestors = ancestors;

            let mut nested = Vec::new();
            let mut seen = HashSet::new();
            for target in target_directories {
                let Ok(canonical_target) = target.canonicalize() else {
                    continue;
                };
                if !canonical_target.starts_with(&self.canonical_workspace) {
                    continue;
                }
                let chain: Vec<PathBuf> = canonical_target
                    .ancestors()
                    .take_while(|dir| dir.starts_with(&self.canonical_workspace))
                    .map(Path::to_path_buf)
                    .collect();
                for dir in chain.into_iter().rev() {
                    if dir == self.canonical_workspace || !seen.insert(dir.clone()) {
                        continue;
                    }
                    if probe(&dir, &mut probes)?.is_some() {
                        nested.push(dir);
                    }
                }
            }
            nested.sort_by(|a, b| {
                (a.components().count(), a.as_os_str())
                    .cmp(&(b.components().count(), b.as_os_str()))
            });
            resolved.nested = nested;
        }
        Ok(resolved)
    }

    /// Loads and fully expands the bundle rooted at `scope_dir`, or `None`
    /// when the directory holds no AGENTS.md. Structural or integrity
    /// failures block the whole bundle; nothing partially read is returned.
    ///
    /// # Errors
    /// Returns the typed [`InstructionError`] describing the first failure.
    pub fn load_bundle(
        &self,
        scope_dir: &Path,
    ) -> Result<Option<InstructionBundle>, InstructionError> {
        let canonical_dir =
            scope_dir
                .canonicalize()
                .map_err(|error| InstructionError::UnreadableSource {
                    path: scope_dir.to_path_buf(),
                    reason: error.to_string(),
                })?;
        let Some(agents_file) = find_agents_file(&canonical_dir)? else {
            return Ok(None);
        };
        let canonical_file =
            agents_file
                .canonicalize()
                .map_err(|error| InstructionError::UnreadableSource {
                    path: agents_file.clone(),
                    reason: error.to_string(),
                })?;
        let global_bundle = self
            .canonical_neo_home
            .as_ref()
            .is_some_and(|home| canonical_dir.starts_with(home));
        let source_is_allowed = if global_bundle {
            self.is_allowed_global_path(&canonical_file)
        } else if canonical_dir.starts_with(&self.canonical_workspace) {
            self.is_contained(&canonical_file)
        } else {
            // Trusted ancestor roots are outside the ordinary import roots,
            // but their AGENTS.md source must still stay inside that ancestor.
            canonical_file.starts_with(&canonical_dir)
        };
        if !source_is_allowed {
            return Err(InstructionError::UntrustedImport {
                path: canonical_file,
            });
        }
        let root_metadata = self
            .source_io
            .read_metadata(&canonical_file)
            .map_err(|error| InstructionError::UnreadableSource {
                path: canonical_file.clone(),
                reason: error.to_string(),
            })?;
        if !root_metadata.is_file {
            return Err(InstructionError::UnreadableSource {
                path: canonical_file,
                reason: "not a regular file".to_owned(),
            });
        }
        let mut expander = ImportExpander {
            resolver: self,
            global_bundle,
            visited: HashSet::new(),
            sources: Vec::new(),
            source_identities: Vec::new(),
            total_bytes: 0,
        };
        let expanded_text = expander.expand_canonical(&agents_file, &canonical_file, 0)?;
        let revision = sha256_hex(expanded_text.as_bytes());
        let token_estimate =
            u64::try_from(estimate_text_tokens(&expanded_text)).unwrap_or(u64::MAX);
        Ok(Some(InstructionBundle {
            scope_dir: canonical_dir,
            source_path: canonical_file,
            revision,
            expanded: expanded_text,
            sources: expander.sources,
            source_identities: expander.source_identities,
            byte_size: expander.total_bytes,
            token_estimate,
        }))
    }
}

struct ImportExpander<'a> {
    resolver: &'a InstructionResolver,
    global_bundle: bool,
    visited: HashSet<PathBuf>,
    sources: Vec<PathBuf>,
    source_identities: Vec<(PathBuf, PathBuf)>,
    total_bytes: u64,
}

impl ImportExpander<'_> {
    fn expand_canonical(
        &mut self,
        source_path: &Path,
        canonical: &Path,
        depth: u32,
    ) -> Result<String, InstructionError> {
        self.source_identities
            .push((source_path.to_path_buf(), canonical.to_path_buf()));
        if !self.visited.insert(canonical.to_path_buf()) {
            // A source imported more than once expands only at its first
            // occurrence; the repeated directive collapses to nothing.
            return Ok(String::new());
        }
        if depth > MAX_IMPORT_DEPTH {
            return Err(InstructionError::LimitExceeded(format!(
                "import depth exceeds {MAX_IMPORT_DEPTH} at `{}`",
                canonical.display()
            )));
        }
        let source_index = u32::try_from(self.sources.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        if source_index > MAX_GRAPH_SOURCES {
            return Err(InstructionError::LimitExceeded(format!(
                "import graph exceeds {MAX_GRAPH_SOURCES} sources"
            )));
        }
        let bytes = read_source_stable(self.resolver.source_io.as_ref(), canonical)?;
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if len > MAX_SOURCE_BYTES {
            return Err(InstructionError::LimitExceeded(format!(
                "source `{}` exceeds {MAX_SOURCE_BYTES} bytes",
                canonical.display()
            )));
        }
        self.total_bytes = self.total_bytes.saturating_add(len);
        if self.total_bytes > MAX_GRAPH_BYTES {
            return Err(InstructionError::LimitExceeded(format!(
                "import graph exceeds {MAX_GRAPH_BYTES} bytes"
            )));
        }
        let text = String::from_utf8(bytes).map_err(|error| InstructionError::InvalidEncoding {
            path: canonical.to_path_buf(),
            content_fingerprint: sha256_hex(error.as_bytes()),
        })?;
        self.sources.push(canonical.to_path_buf());
        let importer_dir = canonical
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.expand_lines(&text, &importer_dir, depth)
    }

    fn expand_lines(
        &mut self,
        text: &str,
        importer_dir: &Path,
        depth: u32,
    ) -> Result<String, InstructionError> {
        let text = self.expand_markdown_links(text, importer_dir, depth)?;
        let mut output = String::with_capacity(text.len());
        let mut fence: Option<(u8, usize)> = None;
        for line in text.split_inclusive('\n') {
            let (body, ending) = match line.strip_suffix('\n') {
                Some(body) => (body.strip_suffix('\r').unwrap_or(body), "\n"),
                None => (line, ""),
            };
            let trimmed = body.trim();
            if let Some((fence_char, fence_len)) = fence_marker(trimmed) {
                match fence {
                    Some((open_char, open_len))
                        if open_char == fence_char
                            && fence_len >= open_len
                            && trimmed[fence_len..].trim().is_empty() =>
                    {
                        fence = None;
                    }
                    None => fence = Some((fence_char, fence_len)),
                    _ => {}
                }
                output.push_str(line);
                continue;
            }
            if fence.is_none()
                && let Some(raw_target) = import_directive(trimmed)
            {
                let (source_path, canonical) = self.resolve_import(importer_dir, raw_target)?;
                let included = self.expand_canonical(&source_path, &canonical, depth + 1)?;
                if included.is_empty() {
                    // Repeated import: the directive collapses entirely.
                    output.push_str(ending);
                    continue;
                }
                let display = escape_attribute(
                    &self
                        .resolver
                        .display_for_model(&canonical)
                        .to_string_lossy(),
                );
                write!(
                    output,
                    "<included_instructions path=\"{display}\">\n{included}"
                )
                .expect("write to string");
                if !included.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("</included_instructions>");
                output.push_str(ending);
                continue;
            }
            output.push_str(line);
        }
        Ok(output)
    }

    fn expand_markdown_links(
        &mut self,
        text: &str,
        importer_dir: &Path,
        depth: u32,
    ) -> Result<String, InstructionError> {
        let links = Parser::new(text)
            .into_offset_iter()
            .filter_map(|(event, range)| match event {
                Event::Start(Tag::Link { dest_url, .. }) => {
                    markdown_import_target(&dest_url).map(|target| (range, target.to_owned()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if links.is_empty() {
            return Ok(text.to_owned());
        }

        let mut output = String::with_capacity(text.len());
        let mut cursor = 0;
        for (range, target) in links {
            output.push_str(&text[cursor..range.end]);
            let (source_path, canonical) = self.resolve_import(importer_dir, &target)?;
            let included = self.expand_canonical(&source_path, &canonical, depth + 1)?;
            if !included.is_empty() {
                let display = escape_attribute(
                    &self
                        .resolver
                        .display_for_model(&canonical)
                        .to_string_lossy(),
                );
                write!(
                    output,
                    "<included_instructions path=\"{display}\">\n{included}"
                )
                .expect("write to string");
                if !included.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("</included_instructions>");
            }
            cursor = range.end;
        }
        output.push_str(&text[cursor..]);
        Ok(output)
    }

    fn resolve_import(
        &self,
        importer_dir: &Path,
        raw_target: &str,
    ) -> Result<(PathBuf, PathBuf), InstructionError> {
        let candidate = Path::new(raw_target);
        let joined =
            if let Ok(relative) = candidate.strip_prefix(Path::new("~")) {
                let home = self.resolver.home_dir.clone().ok_or_else(|| {
                    InstructionError::MissingImport {
                        path: PathBuf::from(raw_target),
                    }
                })?;
                home.join(relative)
            } else if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                importer_dir.join(candidate)
            };
        let canonical = joined.canonicalize().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                InstructionError::MissingImport {
                    path: joined.clone(),
                }
            } else {
                InstructionError::UnreadableSource {
                    path: joined.clone(),
                    reason: error.to_string(),
                }
            }
        })?;
        let allowed = if self.global_bundle {
            self.resolver.is_allowed_global_path(&canonical)
        } else {
            self.resolver.is_contained(&canonical)
        };
        if !allowed {
            return Err(InstructionError::UntrustedImport { path: canonical });
        }
        let metadata = self
            .resolver
            .source_io
            .read_metadata(&canonical)
            .map_err(|error| InstructionError::UnreadableSource {
                path: canonical.clone(),
                reason: error.to_string(),
            })?;
        if !metadata.is_file {
            return Err(InstructionError::UnreadableSource {
                path: canonical,
                reason: "not a regular file".to_owned(),
            });
        }
        let is_markdown = canonical
            .extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("md"));
        if !is_markdown {
            return Err(InstructionError::UnreadableSource {
                path: canonical,
                reason: "not a Markdown (.md) source".to_owned(),
            });
        }
        Ok((joined, canonical))
    }
}

fn markdown_import_target(destination: &str) -> Option<&str> {
    let target = destination
        .split_once('#')
        .map_or(destination, |(path, _)| path);
    if target.is_empty() || target.contains('?') || has_uri_scheme(target) {
        return None;
    }
    Path::new(target)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
        .then_some(target)
}

fn has_uri_scheme(target: &str) -> bool {
    if Path::new(target).is_absolute() {
        return false;
    }
    let Some((scheme, _)) = target.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.starts_with(char::is_alphabetic)
        && scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Returns the fence marker `(char, length)` for a trimmed line that opens
/// or closes a Markdown fenced code block (3+ backticks or tildes).
fn fence_marker(trimmed: &str) -> Option<(u8, usize)> {
    let bytes = trimmed.as_bytes();
    let first = *bytes.first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let len = bytes.iter().take_while(|&&b| b == first).count();
    if len >= 3 { Some((first, len)) } else { None }
}

/// Extracts the target of a standalone import directive: exactly one
/// leading `@` and nothing else on the line. `@@` escapes, inline mentions,
/// URLs, and environment-variable expressions stay literal (`None`).
fn import_directive(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('@')?;
    if rest.starts_with('@') {
        return None;
    }
    let target = rest.trim();
    if target.is_empty()
        || target.contains("://")
        || target.starts_with('$')
        || target.contains("${")
    {
        return None;
    }
    Some(target)
}

/// Escapes a provenance wrapper path attribute.
pub(crate) fn escape_attribute(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// SHA-256 hex digest, the content-addressed identity of sources and
/// expanded bundles.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").expect("write to string");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instructions::InstructionFailureKind;

    fn global_import_fixture(project_trusted: bool) -> (tempfile::TempDir, InstructionResolver) {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let neo_home = temp.path().join("neo-home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&neo_home).expect("neo home");
        let workspace_rule = workspace.join("workspace-rule.md");
        std::fs::write(&workspace_rule, "WORKSPACE SECRET\n").expect("workspace rule");
        std::fs::write(
            neo_home.join("AGENTS.md"),
            format!("@{}\n", workspace_rule.display()),
        )
        .expect("global agents");
        let resolver = InstructionResolver::with_home(
            &InstructionRegistryConfig {
                primary_workspace: workspace,
                neo_home: Some(neo_home),
                project_trusted,
            },
            temp.path().to_path_buf(),
        )
        .expect("resolver");
        (temp, resolver)
    }

    #[test]
    fn untrusted_global_agents_cannot_import_workspace_markdown() {
        let (_temp, resolver) = global_import_fixture(false);
        let neo_home = resolver
            .canonical_neo_home()
            .expect("canonical neo home")
            .to_path_buf();

        let error = resolver
            .load_bundle(&neo_home)
            .expect_err("untrusted global instructions must not read workspace files");

        assert_eq!(
            error.failure_kind(),
            InstructionFailureKind::UntrustedImport
        );
    }

    #[test]
    fn trusted_global_agents_can_import_workspace_markdown() {
        let (_temp, resolver) = global_import_fixture(true);
        let neo_home = resolver
            .canonical_neo_home()
            .expect("canonical neo home")
            .to_path_buf();

        let bundle = resolver
            .load_bundle(&neo_home)
            .expect("trusted global import")
            .expect("global bundle");

        assert!(bundle.expanded.contains("WORKSPACE SECRET"));
    }

    #[test]
    fn global_agents_can_import_platform_home_markdown_outside_neo_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let workspace = home.join("workspace");
        let neo_home = home.join(".neo");
        let shared_rules = home.join(".codex/RTK.md");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&neo_home).expect("neo home");
        std::fs::create_dir_all(shared_rules.parent().expect("rules parent"))
            .expect("rules parent");
        std::fs::write(&shared_rules, "SHARED USER RULES\n").expect("shared rules");
        std::fs::write(
            neo_home.join("AGENTS.md"),
            format!("@{}\n", shared_rules.display()),
        )
        .expect("global agents");
        let resolver = InstructionResolver::with_home(
            &InstructionRegistryConfig {
                primary_workspace: workspace,
                neo_home: Some(neo_home.clone()),
                project_trusted: false,
            },
            home,
        )
        .expect("resolver");

        let bundle = resolver
            .load_bundle(&neo_home)
            .expect("global home import")
            .expect("global bundle");

        assert!(bundle.expanded.contains("SHARED USER RULES"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_home_import_separator_resolves_under_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::write(home.join("rules.md"), "WINDOWS HOME RULES\n").expect("rules");
        std::fs::write(workspace.join("AGENTS.md"), "@~\\rules.md\n").expect("agents");
        let resolver = InstructionResolver::with_home(
            &InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: Some(home.clone()),
                project_trusted: true,
            },
            home,
        )
        .expect("resolver");

        let bundle = resolver
            .load_bundle(&workspace)
            .expect("load")
            .expect("bundle");

        assert!(bundle.expanded.contains("WINDOWS HOME RULES"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_backslash_home_like_import_remains_a_relative_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::write(workspace.join("~\\rules.md"), "UNIX LOCAL RULES\n").expect("rules");
        std::fs::write(workspace.join("AGENTS.md"), "@~\\rules.md\n").expect("agents");
        let resolver = InstructionResolver::with_home(
            &InstructionRegistryConfig {
                primary_workspace: workspace.clone(),
                neo_home: Some(home.clone()),
                project_trusted: true,
            },
            home,
        )
        .expect("resolver");

        let bundle = resolver
            .load_bundle(&workspace)
            .expect("load")
            .expect("bundle");

        assert!(bundle.expanded.contains("UNIX LOCAL RULES"));
    }

    #[cfg(unix)]
    #[test]
    fn untrusted_global_agents_root_symlink_cannot_target_workspace_markdown() {
        use std::os::unix::fs::symlink;

        let (_temp, resolver) = global_import_fixture(false);
        let neo_home = resolver
            .canonical_neo_home()
            .expect("canonical neo home")
            .to_path_buf();
        let agents_file = neo_home.join("AGENTS.md");
        std::fs::remove_file(&agents_file).expect("remove regular global agents");
        symlink(
            resolver.canonical_workspace().join("workspace-rule.md"),
            &agents_file,
        )
        .expect("symlink global agents to workspace");

        let error = resolver
            .load_bundle(&neo_home)
            .expect_err("untrusted global root must stay inside neo home");

        assert_eq!(
            error.failure_kind(),
            InstructionFailureKind::UntrustedImport
        );
    }

    #[cfg(unix)]
    #[test]
    fn trusted_global_agents_root_symlink_can_target_workspace_markdown() {
        use std::os::unix::fs::symlink;

        let (_temp, resolver) = global_import_fixture(true);
        let neo_home = resolver
            .canonical_neo_home()
            .expect("canonical neo home")
            .to_path_buf();
        let agents_file = neo_home.join("AGENTS.md");
        std::fs::remove_file(&agents_file).expect("remove regular global agents");
        symlink(
            resolver.canonical_workspace().join("workspace-rule.md"),
            &agents_file,
        )
        .expect("symlink global agents to workspace");

        let bundle = resolver
            .load_bundle(&neo_home)
            .expect("trusted global root symlink")
            .expect("global bundle");

        assert!(bundle.expanded.contains("WORKSPACE SECRET"));
    }
}
