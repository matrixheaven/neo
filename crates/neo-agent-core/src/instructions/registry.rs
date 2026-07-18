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

use std::collections::{BTreeMap, HashMap};
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
    instruction_selection_fingerprint,
};
use crate::runtime::estimate_text_tokens;

const AUTHORITY_PREFIX: &str = "<instruction_authority mode=\"replace_all\">\n\
This epoch is the complete current path-scoped instruction snapshot. Earlier \
path-scoped instruction epochs are historical and no longer authoritative.\n";
const AUTHORITY_EMPTY: &str = "No path-scoped instruction bundles are currently active.\n";
const AUTHORITY_SUFFIX: &str = "</instruction_authority>\n";

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

/// Exact post-compaction rehydration data for the refreshed current scope
/// chain (global, trusted ancestors, workspace root, and the chain to the
/// most recently used nested scope).
#[derive(Debug, Clone)]
pub struct RehydrationSnapshot {
    /// Current chain bundles in model rendering order, refreshed from disk.
    pub chain: Vec<AdmissionCandidate>,
    /// Rendered model content for `chain`, byte-identical to epoch rendering.
    pub model_content: Option<String>,
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
    stamps: Vec<SourceStamp>,
    bundle: InstructionBundle,
}

#[derive(Debug, Clone)]
struct SourceStamp {
    source_path: PathBuf,
    canonical_path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
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
        self.restore_generation(epoch.generation);
    }

    /// Advances the session generation counter from replayed agent state.
    pub fn restore_generation(&self, generation: u64) {
        self.generation
            .fetch_max(generation, AtomicOrdering::SeqCst);
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
            InstructionInheritance::FullContext => {
                let mut targets = parent
                    .active_scopes
                    .iter()
                    .filter(|dir| dir.starts_with(&self.config.primary_workspace))
                    .cloned()
                    .collect::<Vec<_>>();
                if let Some(scope) = parent
                    .most_recent_scope
                    .as_ref()
                    .filter(|scope| scope.starts_with(&self.config.primary_workspace))
                    && !targets.contains(scope)
                {
                    targets.push(scope.clone());
                }
                targets
            }
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

        if failure.is_none() {
            let selected_bundles = candidates
                .iter()
                .map(|candidate| candidate.metadata.clone())
                .collect::<Vec<_>>();
            let hash = instruction_selection_fingerprint(&selected_bundles, &[], None);
            if state.last_epoch_fingerprint.as_deref() == Some(hash.as_str()) {
                return (
                    InstructionFingerprint {
                        hash,
                        agent_id: request.agent_id.clone(),
                        target_directories: request.target_directories.clone(),
                        budget: request.budget,
                        deferred_tool_ids: request.deferred_tool_ids.clone(),
                    },
                    None,
                );
            }
        }

        let selection = if failure.is_none() {
            self.select_for_rendered_budget(candidates, request.budget)
        } else {
            // A blocked scope injects nothing, not even other scopes'
            // readable subsets.
            AdmissionSelection::default()
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
            self.render_selection_within_budget(&selection, request.budget.actual)
        };
        self.drop_ignored_bodies(&selection.ignored);
        let selected_bundles = selection
            .admitted
            .iter()
            .map(|candidate| candidate.metadata.clone())
            .collect::<Vec<_>>();
        let hash = instruction_selection_fingerprint(
            &selected_bundles,
            &selection.ignored,
            failure_data.as_ref(),
        );
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

        let current_scope_removed = state.visible_revisions.keys().any(|scope| {
            request
                .target_directories
                .iter()
                .any(|target| target.starts_with(scope))
                && !selection
                    .admitted
                    .iter()
                    .any(|candidate| &candidate.scope_dir == scope)
        });

        let outcome = if failure.is_some() {
            InstructionEpochOutcome::Blocked
        } else if !selection.ignored.is_empty() {
            InstructionEpochOutcome::PartiallyLoaded
        } else if !replacements.is_empty() {
            InstructionEpochOutcome::Updated
        } else if current_scope_removed {
            InstructionEpochOutcome::Removed
        } else if selection.admitted.iter().any(|candidate| {
            state.visited_revisions.contains_key(&candidate.scope_dir)
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

        let generation = self.generation.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        let epoch = InstructionEpochData {
            agent_id: request.agent_id.clone(),
            generation,
            outcome,
            scopes: scope_data,
            selected_bundles,
            ignored_bundles: selection.ignored,
            replacements,
            failure: failure_data,
            deferred_tool_ids: request.deferred_tool_ids.clone(),
            budget: request.budget,
            model_content,
        };
        (fingerprint, Some(epoch))
    }

    /// Renders admitted bundles in model rendering order with provenance
    /// wrappers. Epoch evaluation and post-compaction rehydration share this
    /// path so both produce byte-identical content for the same bundle set.
    fn render_admitted_bundles(&self, admitted: &[AdmissionCandidate]) -> String {
        let mut rendered = String::new();
        for candidate in InstructionAdmission::rendering_order(admitted.to_vec()) {
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
        rendered
    }

    fn render_authoritative_content(
        &self,
        admitted: &[AdmissionCandidate],
        ignored: &[IgnoredInstructionBundle],
        include_omission_notice: bool,
    ) -> String {
        let mut rendered = String::from(AUTHORITY_PREFIX);
        if admitted.is_empty() {
            rendered.push_str(AUTHORITY_EMPTY);
        } else {
            rendered.push_str(&self.render_admitted_bundles(admitted));
        }
        if include_omission_notice && !ignored.is_empty() {
            let ignored = ignored
                .iter()
                .map(|bundle| {
                    format!(
                        "`{}` ({} tokens, over budget)",
                        bundle.display_path.display(),
                        bundle.token_estimate
                    )
                })
                .collect::<Vec<_>>();
            writeln!(
                rendered,
                "Ignored instruction bundles: {}. The model must not claim compliance with \
                 omitted rules.",
                ignored.join(", ")
            )
            .expect("write to string");
        }
        rendered.push_str(AUTHORITY_SUFFIX);
        rendered
    }

    fn select_for_rendered_budget(
        &self,
        mut candidates: Vec<AdmissionCandidate>,
        budget: InstructionBudget,
    ) -> AdmissionSelection {
        let base_tokens = estimate_text_tokens(AUTHORITY_PREFIX)
            .saturating_add(estimate_text_tokens(AUTHORITY_SUFFIX));
        let available = budget
            .actual
            .saturating_sub(u64::try_from(base_tokens).unwrap_or(u64::MAX));
        let original_estimates = candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.scope_dir.clone(),
                    candidate.metadata.token_estimate,
                )
            })
            .collect::<HashMap<_, _>>();
        candidates.sort_by(admission_cmp);
        let admission_priority = candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.scope_dir.clone(), index))
            .collect::<HashMap<_, _>>();
        for candidate in &mut candidates {
            let rendered = self.render_admitted_bundles(std::slice::from_ref(candidate));
            candidate.metadata.token_estimate =
                u64::try_from(estimate_text_tokens(&rendered)).unwrap_or(u64::MAX);
        }
        let mut selection = InstructionAdmission::select(
            candidates,
            InstructionBudget {
                actual: available,
                ..budget
            },
        );
        for candidate in &mut selection.admitted {
            candidate.metadata.token_estimate = original_estimates[&candidate.scope_dir];
        }
        for bundle in &mut selection.ignored {
            bundle.token_estimate = original_estimates[&bundle.display_path];
        }
        while !selection.ignored.is_empty()
            && self
                .render_selection_within_budget(&selection, budget.actual)
                .is_none()
        {
            let Some(candidate) = selection.admitted.pop() else {
                break;
            };
            selection.ignored.push(IgnoredInstructionBundle {
                display_path: candidate.metadata.display_path,
                revision: candidate.metadata.revision,
                token_estimate: candidate.metadata.token_estimate,
                reason: InstructionOmissionReason::OverBudget,
            });
            selection.ignored.sort_by_key(|bundle| {
                admission_priority
                    .get(&bundle.display_path)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        }
        selection
    }

    fn render_selection_within_budget(
        &self,
        selection: &AdmissionSelection,
        actual: u64,
    ) -> Option<String> {
        let without_notice =
            self.render_authoritative_content(&selection.admitted, &selection.ignored, false);
        let without_notice_tokens =
            u64::try_from(estimate_text_tokens(&without_notice)).unwrap_or(u64::MAX);
        if without_notice_tokens > actual {
            return None;
        }
        let with_notice =
            self.render_authoritative_content(&selection.admitted, &selection.ignored, true);
        let with_notice_tokens =
            u64::try_from(estimate_text_tokens(&with_notice)).unwrap_or(u64::MAX);
        if selection.ignored.is_empty() {
            Some(without_notice)
        } else if with_notice_tokens <= actual {
            Some(with_notice)
        } else {
            None
        }
    }

    fn drop_ignored_bodies(&self, ignored: &[IgnoredInstructionBundle]) {
        let mut cache = self.cache();
        for bundle in ignored {
            cache.remove(&bundle.display_path);
        }
    }

    /// Loads the exact post-compaction rehydration state for the last admitted
    /// revisions. Cached bytes win so disk changes cannot silently alter
    /// authority; disk is read only when the admitted revision is absent from
    /// cache. Agent-local visited history is durable state and remains
    /// unchanged by rehydration.
    ///
    /// # Errors
    /// Returns the typed [`InstructionError`] when scope discovery or a
    /// bundle read fails; callers must surface it rather than rehydrate
    /// partial rules.
    pub async fn rehydration_snapshot(
        &self,
        most_recent_scope: Option<&Path>,
        admitted_revisions: &BTreeMap<PathBuf, String>,
    ) -> Result<RehydrationSnapshot, InstructionError> {
        let _guard = self.sync.lock().await;
        let targets: Vec<PathBuf> =
            most_recent_scope.map_or_else(Vec::new, |dir| vec![dir.to_path_buf()]);
        let resolved = self.resolver.discover_scopes(&targets)?;
        let mut chain = Vec::new();
        for (kind, dir) in resolved.rendering_order() {
            let Some(expected_revision) = admitted_revisions.get(&dir) else {
                continue;
            };
            let canonical_dir =
                dir.canonicalize()
                    .map_err(|error| InstructionError::UnreadableSource {
                        path: dir.clone(),
                        reason: error.to_string(),
                    })?;
            if let Some(candidate) = self
                .cache()
                .get(&canonical_dir)
                .filter(|cached| cached.bundle.revision == *expected_revision)
                .map(|cached| candidate_from_bundle(kind, &cached.bundle))
            {
                chain.push(candidate);
                continue;
            }
            let Some(bundle) = self.resolver.load_bundle(&canonical_dir)? else {
                return Err(InstructionError::UnreadableSource {
                    path: canonical_dir,
                    reason: "previously admitted instruction revision is no longer available"
                        .to_owned(),
                });
            };
            if bundle.revision != *expected_revision {
                return Err(InstructionError::UnreadableSource {
                    path: canonical_dir,
                    reason: "instruction source changed before its replacement epoch was admitted"
                        .to_owned(),
                });
            }
            let candidate = candidate_from_bundle(kind, &bundle);
            self.cache().insert(
                canonical_dir,
                CachedBundle {
                    stamps: source_stamps(&bundle),
                    bundle,
                },
            );
            chain.push(candidate);
        }
        let rendered = self.render_authoritative_content(&chain, &[], false);
        let model_content = if chain.is_empty() {
            None
        } else {
            Some(rendered)
        };
        Ok(RehydrationSnapshot {
            chain,
            model_content,
        })
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
        let stamps = source_stamps(&bundle);
        let candidate = candidate_from_bundle(kind, &bundle);
        self.cache()
            .insert(canonical_dir, CachedBundle { stamps, bundle });
        Ok(Some(candidate))
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, CachedBundle>> {
        self.bundles.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn source_stamps(bundle: &InstructionBundle) -> Vec<SourceStamp> {
    bundle
        .source_identities
        .iter()
        .map(|(source_path, canonical_path)| {
            let metadata = std::fs::metadata(canonical_path).ok();
            SourceStamp {
                source_path: source_path.clone(),
                canonical_path: canonical_path.clone(),
                modified: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok()),
                len: metadata.map_or(0, |metadata| metadata.len()),
            }
        })
        .collect()
}

fn candidate_from_bundle(
    kind: InstructionScopeKind,
    bundle: &InstructionBundle,
) -> AdmissionCandidate {
    AdmissionCandidate {
        kind,
        scope_dir: bundle.scope_dir.clone(),
        metadata: metadata_from_bundle(bundle),
        content: bundle.expanded.clone(),
    }
}

fn metadata_from_bundle(bundle: &InstructionBundle) -> InstructionBundleMetadata {
    InstructionBundleMetadata {
        display_path: bundle.scope_dir.clone(),
        revision: bundle.revision.clone(),
        token_estimate: bundle.token_estimate,
        byte_size: bundle.byte_size,
        source_count: bundle.source_count(),
        import_count: bundle.import_count(),
        import_paths: bundle.sources.iter().skip(1).cloned().collect(),
    }
}

fn stamps_current(stamps: &[SourceStamp]) -> bool {
    stamps.iter().all(|stamp| {
        if stamp.source_path.canonicalize().ok().as_ref() != Some(&stamp.canonical_path) {
            return false;
        }
        let Ok(metadata) = std::fs::metadata(&stamp.canonical_path) else {
            return false;
        };
        // Without a modification time there is no reliable fast path:
        // force a reread; the content hash still deduplicates revisions.
        metadata.len() == stamp.len
            && stamp.modified.is_some()
            && metadata.modified().ok() == stamp.modified
    })
}
