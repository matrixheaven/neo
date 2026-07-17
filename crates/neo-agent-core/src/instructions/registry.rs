//! Session-level instruction registry: the only source of truth for
//! canonical scopes, bundle revisions, admission selection, and failure
//! fingerprints.
//!
//! `reconcile` freezes one registry generation, resolves the union of all
//! target chains, selects complete bundles against the dynamic budget, and
//! compares the selection with the supplied agent state, returning exactly
//! one `Proceed`, `Defer`, or `Block`. Reconciliation is serialized through
//! a Tokio mutex, which also provides keyed single-flight behavior for
//! concurrent reads of one source; positive bundle caches live behind a
//! standard mutex and missing results are never cached across calls.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use super::resolver::{
    InstructionBundle, InstructionResolver, SourceIo, escape_attribute, find_agents_file,
    sha256_hex,
};
use super::types::{
    AgentInstructionState, IgnoredInstructionBundle, InstructionBudget, InstructionBundleMetadata,
    InstructionEpochData, InstructionEpochOutcome, InstructionError, InstructionFailure,
    InstructionFingerprint, InstructionInheritance, InstructionOmissionReason,
    InstructionPreflightDecision, InstructionReconcileKind, InstructionReconcileRequest,
    InstructionRegistryConfig, InstructionReplacement, InstructionScopeData, InstructionScopeKind,
};

/// One complete bundle offered for budget admission.
#[derive(Debug, Clone)]
pub struct AdmissionCandidate {
    pub kind: InstructionScopeKind,
    /// Canonical scope directory; drives deterministic ordering.
    pub scope_dir: PathBuf,
    pub metadata: InstructionBundleMetadata,
    /// Complete expanded bundle content. Dropped for ignored bundles.
    pub content: String,
}

/// The deterministic whole-bundle admission outcome.
#[derive(Debug, Clone, Default)]
pub struct AdmissionSelection {
    /// Admitted bundles in admission priority order.
    pub admitted: Vec<AdmissionCandidate>,
    /// Ignored bundles in admission priority order; bodies already dropped.
    pub ignored: Vec<IgnoredInstructionBundle>,
}

/// Deterministic atomic budget admission.
pub struct InstructionAdmission;

impl InstructionAdmission {
    /// Selects complete bundles in admission priority order — global,
    /// workspace root, nested deepest to shallowest, ancestors nearest
    /// first — until `budget.actual` is exhausted. Bundles that do not fit
    /// are ignored as whole units; their bodies are discarded after
    /// measurement and only display path, hash, token estimate, and the
    /// omission reason are retained.
    #[must_use]
    pub fn select(
        mut candidates: Vec<AdmissionCandidate>,
        budget: InstructionBudget,
    ) -> AdmissionSelection {
        candidates.sort_by(admission_cmp);
        let mut remaining = budget.actual;
        let mut selection = AdmissionSelection::default();
        for candidate in candidates {
            let estimate = candidate.metadata.token_estimate;
            if estimate <= remaining {
                remaining -= estimate;
                selection.admitted.push(candidate);
            } else {
                selection.ignored.push(IgnoredInstructionBundle {
                    display_path: candidate.metadata.display_path,
                    revision: candidate.metadata.revision,
                    token_estimate: estimate,
                    reason: InstructionOmissionReason::OverBudget,
                });
            }
        }
        selection
    }

    /// Model rendering order — global, ancestors outermost to nearest,
    /// workspace root, then nested shallowest to deepest — so deeper
    /// instructions appear later and can override broader guidance.
    #[must_use]
    pub fn rendering_order(mut candidates: Vec<AdmissionCandidate>) -> Vec<AdmissionCandidate> {
        candidates.sort_by(rendering_cmp);
        candidates
    }
}

fn scope_depth(candidate: &AdmissionCandidate) -> usize {
    candidate.scope_dir.components().count()
}

fn admission_rank(kind: InstructionScopeKind) -> u8 {
    match kind {
        InstructionScopeKind::Global => 0,
        InstructionScopeKind::WorkspaceRoot => 1,
        InstructionScopeKind::Nested => 2,
        InstructionScopeKind::Ancestor => 3,
    }
}

fn rendering_rank(kind: InstructionScopeKind) -> u8 {
    match kind {
        InstructionScopeKind::Global => 0,
        InstructionScopeKind::Ancestor => 1,
        InstructionScopeKind::WorkspaceRoot => 2,
        InstructionScopeKind::Nested => 3,
    }
}

fn admission_cmp(a: &AdmissionCandidate, b: &AdmissionCandidate) -> std::cmp::Ordering {
    admission_rank(a.kind)
        .cmp(&admission_rank(b.kind))
        // Deepest nested / nearest ancestor first.
        .then_with(|| scope_depth(b).cmp(&scope_depth(a)))
        .then_with(|| a.scope_dir.cmp(&b.scope_dir))
}

fn rendering_cmp(a: &AdmissionCandidate, b: &AdmissionCandidate) -> std::cmp::Ordering {
    rendering_rank(a.kind)
        .cmp(&rendering_rank(b.kind))
        // Outermost ancestor / shallowest nested first.
        .then_with(|| scope_depth(a).cmp(&scope_depth(b)))
        .then_with(|| a.scope_dir.cmp(&b.scope_dir))
}

#[derive(Debug, Clone)]
struct CachedBundle {
    /// `(path, modified, len)` for every source; only a fast path, the
    /// content hash inside `bundle` remains the final identity.
    stamps: Vec<(PathBuf, Option<SystemTime>, u64)>,
    bundle: InstructionBundle,
}

/// Session-scoped instruction registry.
#[derive(Debug)]
pub struct InstructionRegistry {
    config: InstructionRegistryConfig,
    resolver: InstructionResolver,
    generation: AtomicU64,
    bundles: Mutex<HashMap<PathBuf, CachedBundle>>,
    sync: tokio::sync::Mutex<()>,
}

impl InstructionRegistry {
    /// Builds a registry, canonicalizing the workspace and `$NEO_HOME`.
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when the primary workspace cannot be
    /// canonicalized.
    // By-value config is part of the frozen interface contract.
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(config: InstructionRegistryConfig) -> Result<Self, InstructionError> {
        let resolver = InstructionResolver::new(&config)?;
        Ok(Self::from_resolver(resolver, config.project_trusted))
    }

    /// Builds a registry with an explicit home directory and source I/O
    /// implementation (tests, sandboxes). Production uses [`Self::new`]; the
    /// injected reader observes the same reconciliation, single-flight, and
    /// cache behavior — the seam changes only how source bytes are read.
    ///
    /// # Errors
    /// Returns [`InstructionError::Io`] when the primary workspace cannot be
    /// canonicalized.
    // By-value config mirrors `new`'s frozen interface contract.
    #[allow(clippy::needless_pass_by_value)]
    pub fn with_source_io(
        config: InstructionRegistryConfig,
        home_dir: Option<PathBuf>,
        source_io: Arc<dyn SourceIo>,
    ) -> Result<Self, InstructionError> {
        let resolver = InstructionResolver::with_source_io(&config, home_dir, source_io)?;
        Ok(Self::from_resolver(resolver, config.project_trusted))
    }

    fn from_resolver(resolver: InstructionResolver, project_trusted: bool) -> Self {
        let config = InstructionRegistryConfig {
            primary_workspace: resolver.canonical_workspace().to_path_buf(),
            neo_home: resolver.canonical_neo_home().map(Path::to_path_buf),
            project_trusted,
        };
        Self {
            config,
            resolver,
            generation: AtomicU64::new(0),
            bundles: Mutex::new(HashMap::new()),
            sync: tokio::sync::Mutex::new(()),
        }
    }

    /// Reconciles one frozen generation against the agent's visible state.
    pub async fn reconcile(
        &self,
        request: InstructionReconcileRequest,
        state: &AgentInstructionState,
    ) -> InstructionPreflightDecision {
        let _guard = self.sync.lock().await;
        let (fingerprint, epoch) = self.evaluate(&request, state);
        match epoch {
            None => InstructionPreflightDecision::Proceed { fingerprint },
            Some(epoch) if epoch.failure.is_some() => {
                InstructionPreflightDecision::Block { epoch, fingerprint }
            }
            Some(epoch) => InstructionPreflightDecision::Defer { epoch, fingerprint },
        }
    }

    /// Re-verifies a frozen fingerprint before batch execution. A matching
    /// fingerprint proceeds; any change produces a fresh epoch built with
    /// the original request parameters captured in the fingerprint.
    pub async fn recheck(
        &self,
        fingerprint: &InstructionFingerprint,
        state: &AgentInstructionState,
    ) -> InstructionPreflightDecision {
        let request = InstructionReconcileRequest {
            agent_id: fingerprint.agent_id.clone(),
            kind: InstructionReconcileKind::ToolPreflight,
            target_directories: fingerprint.target_directories.clone(),
            budget: fingerprint.budget,
            deferred_tool_ids: fingerprint.deferred_tool_ids.clone(),
        };
        let _guard = self.sync.lock().await;
        let (fresh, epoch) = self.evaluate(&request, state);
        if fresh.hash == fingerprint.hash {
            return InstructionPreflightDecision::Proceed {
                fingerprint: fingerprint.clone(),
            };
        }
        match epoch {
            None => InstructionPreflightDecision::Proceed { fingerprint: fresh },
            Some(epoch) if epoch.failure.is_some() => InstructionPreflightDecision::Block {
                epoch,
                fingerprint: fresh,
            },
            Some(epoch) => InstructionPreflightDecision::Defer {
                epoch,
                fingerprint: fresh,
            },
        }
    }

    /// Absorbs one persisted epoch during replay so freshly emitted
    /// generations never collide with restored history.
    pub fn restore_epoch(&self, epoch: &InstructionEpochData) {
        self.generation
            .fetch_max(epoch.generation, AtomicOrdering::SeqCst);
    }

    /// Builds the child agent's baseline request. Full-context inheritance
    /// seeds the parent's currently applicable workspace scopes; summary
    /// inheritance starts from the plain global/workspace baseline.
    #[must_use]
    pub fn child_baseline_request(
        &self,
        parent: &AgentInstructionState,
        child_agent_id: String,
        inheritance: InstructionInheritance,
        budget: InstructionBudget,
    ) -> InstructionReconcileRequest {
        let target_directories = match inheritance {
            InstructionInheritance::FullContext => parent
                .active_scopes
                .iter()
                .filter(|dir| dir.starts_with(&self.config.primary_workspace))
                .cloned()
                .collect(),
            InstructionInheritance::Summary => Vec::new(),
        };
        InstructionReconcileRequest {
            agent_id: child_agent_id,
            kind: InstructionReconcileKind::Baseline,
            target_directories,
            budget,
            deferred_tool_ids: Vec::new(),
        }
    }

    /// Shared evaluation core: resolve scopes, load bundles, admit against
    /// the budget, and compare the frozen selection with the agent state.
    /// Returns the fresh fingerprint and, when anything changed, the epoch.
    fn evaluate(
        &self,
        request: &InstructionReconcileRequest,
        state: &AgentInstructionState,
    ) -> (InstructionFingerprint, Option<InstructionEpochData>) {
        let mut failure: Option<InstructionError> = None;
        let mut scopes: Vec<(InstructionScopeKind, PathBuf)> = Vec::new();
        let mut candidates: Vec<AdmissionCandidate> = Vec::new();

        match self.resolver.discover_scopes(&request.target_directories) {
            Err(error) => failure = Some(error),
            Ok(resolved) => {
                scopes = resolved.rendering_order();
                for (kind, dir) in &scopes {
                    match self.load_cached(dir, *kind) {
                        Ok(Some(candidate)) => candidates.push(candidate),
                        Ok(None) => {}
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                }
            }
        }

        let selection = if failure.is_none() {
            InstructionAdmission::select(candidates, request.budget)
        } else {
            // A blocked scope injects nothing, not even other scopes'
            // readable subsets.
            AdmissionSelection::default()
        };

        let hash = fingerprint_hash(failure.as_ref(), &selection);
        let fingerprint = InstructionFingerprint {
            hash: hash.clone(),
            agent_id: request.agent_id.clone(),
            target_directories: request.target_directories.clone(),
            budget: request.budget,
            deferred_tool_ids: request.deferred_tool_ids.clone(),
        };

        let unchanged = state.last_epoch_fingerprint.as_deref() == Some(hash.as_str());
        let nothing_to_say = failure.is_none()
            && selection.admitted.is_empty()
            && selection.ignored.is_empty()
            && state.visible_revisions.is_empty()
            && state.active_scopes.is_empty();
        if unchanged || nothing_to_say {
            return (fingerprint, None);
        }

        let scope_data: Vec<InstructionScopeData> = scopes
            .iter()
            .map(|(kind, dir)| {
                let admitted = selection
                    .admitted
                    .iter()
                    .find(|candidate| &candidate.scope_dir == dir);
                let ignored = selection
                    .ignored
                    .iter()
                    .find(|bundle| &bundle.display_path == dir);
                InstructionScopeData {
                    display_path: dir.clone(),
                    kind: *kind,
                    revision: admitted
                        .map(|candidate| candidate.metadata.revision.clone())
                        .or_else(|| ignored.map(|bundle| bundle.revision.clone())),
                    token_estimate: admitted
                        .map_or(0, |candidate| candidate.metadata.token_estimate)
                        .max(ignored.map_or(0, |bundle| bundle.token_estimate)),
                }
            })
            .collect();

        let mut replacements = Vec::new();
        for candidate in &selection.admitted {
            if let Some(previous) = state.visible_revisions.get(&candidate.scope_dir)
                && previous != &candidate.metadata.revision
            {
                replacements.push(InstructionReplacement {
                    display_path: candidate.scope_dir.clone(),
                    previous_revision: previous.clone(),
                    new_revision: candidate.metadata.revision.clone(),
                });
            }
        }

        let outcome = if failure.is_some() {
            InstructionEpochOutcome::Blocked
        } else if !selection.ignored.is_empty() {
            InstructionEpochOutcome::PartiallyLoaded
        } else if !replacements.is_empty() {
            InstructionEpochOutcome::Updated
        } else if state.active_scopes.iter().any(|dir| {
            !selection
                .admitted
                .iter()
                .any(|candidate| &candidate.scope_dir == dir)
        }) {
            InstructionEpochOutcome::Removed
        } else if selection.admitted.iter().any(|candidate| {
            state.visible_revisions.contains_key(&candidate.scope_dir)
                && !state.active_scopes.contains(&candidate.scope_dir)
        }) {
            InstructionEpochOutcome::Reactivated
        } else if request.kind == InstructionReconcileKind::Baseline
            && state.visible_generation == 0
            && state.visible_revisions.is_empty()
        {
            InstructionEpochOutcome::Ready
        } else {
            InstructionEpochOutcome::Activated
        };

        let failure_data = failure.as_ref().map(|error| InstructionFailure {
            fingerprint: sha256_hex(error.failure_identity().as_bytes()),
            display_path: error.failure_path(),
            kind: error.failure_kind(),
            detail: error.to_string(),
        });

        let model_content = if let Some(failure) = &failure_data {
            Some(format!(
                "Instruction scope blocked: {}. No instructions from this scope were \
                 injected; resolve the issue to load them.",
                failure.detail
            ))
        } else {
            let mut rendered = String::new();
            for candidate in InstructionAdmission::rendering_order(selection.admitted.clone()) {
                let display = escape_attribute(
                    &self
                        .resolver
                        .display_for_model(&candidate.scope_dir)
                        .to_string_lossy(),
                );
                writeln!(rendered, "<instructions path=\"{display}\">").expect("write to string");
                rendered.push_str(&candidate.content);
                if !candidate.content.ends_with('\n') {
                    rendered.push('\n');
                }
                rendered.push_str("</instructions>\n");
            }
            if !selection.ignored.is_empty() {
                let ignored: Vec<String> = selection
                    .ignored
                    .iter()
                    .map(|bundle| {
                        format!(
                            "`{}` ({} tokens, over budget)",
                            bundle.display_path.display(),
                            bundle.token_estimate
                        )
                    })
                    .collect();
                write!(
                    rendered,
                    "\u{26a0} Ignored instruction bundles: {}. The model must not claim \
                     compliance with omitted rules.",
                    ignored.join(", ")
                )
                .expect("write to string");
            }
            if rendered.is_empty() {
                None
            } else {
                Some(rendered)
            }
        };

        let generation = self.generation.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        let epoch = InstructionEpochData {
            agent_id: request.agent_id.clone(),
            generation,
            outcome,
            scopes: scope_data,
            selected_bundles: selection
                .admitted
                .iter()
                .map(|candidate| candidate.metadata.clone())
                .collect(),
            ignored_bundles: selection.ignored,
            replacements,
            failure: failure_data,
            deferred_tool_ids: request.deferred_tool_ids.clone(),
            model_content,
        };
        (fingerprint, Some(epoch))
    }

    /// Loads one scope's bundle through the positive-only cache. Metadata
    /// change triggers a reread; the content hash, not mtime, decides
    /// whether a new revision exists. Missing AGENTS.md results are never
    /// cached across calls.
    fn load_cached(
        &self,
        scope_dir: &Path,
        kind: InstructionScopeKind,
    ) -> Result<Option<AdmissionCandidate>, InstructionError> {
        let canonical_dir =
            scope_dir
                .canonicalize()
                .map_err(|error| InstructionError::UnreadableSource {
                    path: scope_dir.to_path_buf(),
                    reason: error.to_string(),
                })?;
        if find_agents_file(&canonical_dir)?.is_none() {
            self.cache().remove(&canonical_dir);
            return Ok(None);
        }
        {
            let cache = self.cache();
            if let Some(cached) = cache.get(&canonical_dir)
                && stamps_current(&cached.stamps)
            {
                return Ok(Some(candidate_from_bundle(kind, &cached.bundle)));
            }
        }
        let Some(bundle) = self.resolver.load_bundle(&canonical_dir)? else {
            self.cache().remove(&canonical_dir);
            return Ok(None);
        };
        let stamps = bundle
            .sources
            .iter()
            .map(|path| {
                let metadata = std::fs::metadata(path).ok();
                (
                    path.clone(),
                    metadata.as_ref().and_then(|m| m.modified().ok()),
                    metadata.map_or(0, |m| m.len()),
                )
            })
            .collect();
        let candidate = candidate_from_bundle(kind, &bundle);
        self.cache()
            .insert(canonical_dir, CachedBundle { stamps, bundle });
        Ok(Some(candidate))
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, CachedBundle>> {
        self.bundles.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn candidate_from_bundle(
    kind: InstructionScopeKind,
    bundle: &InstructionBundle,
) -> AdmissionCandidate {
    AdmissionCandidate {
        kind,
        scope_dir: bundle.scope_dir.clone(),
        metadata: InstructionBundleMetadata {
            display_path: bundle.scope_dir.clone(),
            revision: bundle.revision.clone(),
            token_estimate: bundle.token_estimate,
            byte_size: bundle.byte_size,
            source_count: bundle.source_count(),
            import_count: bundle.import_count(),
        },
        content: bundle.expanded.clone(),
    }
}

fn stamps_current(stamps: &[(PathBuf, Option<SystemTime>, u64)]) -> bool {
    stamps.iter().all(|(path, modified, len)| {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        // Without a modification time there is no reliable fast path:
        // force a reread; the content hash still deduplicates revisions.
        metadata.len() == *len && modified.is_some() && metadata.modified().ok() == *modified
    })
}

/// Stable fingerprint over the frozen selection: admitted scope/revision
/// pairs, ignored bundles, and the failure identity, all in canonical
/// order. Budget inputs are deliberately excluded — they are carried by
/// [`InstructionFingerprint`] for recheck instead.
fn fingerprint_hash(failure: Option<&InstructionError>, selection: &AdmissionSelection) -> String {
    let mut canonical = String::new();
    let mut admitted: Vec<&AdmissionCandidate> = selection.admitted.iter().collect();
    admitted.sort_by(|a, b| a.scope_dir.cmp(&b.scope_dir));
    for candidate in admitted {
        writeln!(
            canonical,
            "A|{}|{}",
            candidate.scope_dir.display(),
            candidate.metadata.revision
        )
        .expect("write to string");
    }
    let mut ignored: Vec<&IgnoredInstructionBundle> = selection.ignored.iter().collect();
    ignored.sort_by(|a, b| a.display_path.cmp(&b.display_path));
    for bundle in ignored {
        writeln!(
            canonical,
            "I|{}|{}|over_budget",
            bundle.display_path.display(),
            bundle.revision
        )
        .expect("write to string");
    }
    if let Some(error) = failure {
        writeln!(canonical, "F|{}", error.failure_identity()).expect("write to string");
    }
    sha256_hex(canonical.as_bytes())
}
