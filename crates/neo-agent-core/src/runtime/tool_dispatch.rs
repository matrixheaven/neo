use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::{StreamExt, stream::FuturesUnordered};
use neo_ai::ModelClient;
use tokio_util::sync::CancellationToken;

use super::config::{AgentConfig, BlockedInstructionScope, ToolExecutionMode};
use super::context::AgentContext;
use super::error::AgentRuntimeError;
use super::events::{
    EventEmitter, emit_shell_finished, emit_terminal_events, make_shell_admission_callback,
    make_tool_event_callback, make_tool_update_callback,
};
use super::instruction_context::InstructionContextBridge;
use super::permission::{
    current_permission_mode, permission_preparation_for_mode, resolve_permission_preparation,
};
use super::plan_orchestration::{attach_exit_plan_details, exit_plan_mode_has_reviewable_plan};
use super::skill_dispatch::{execute_invoke_skill, format_skill_tool_arguments};
use super::tool_arguments::{InstructionScopeProbe, PreparedExecution, prepare_tool_arguments};
use crate::instructions::{
    InstructionEpochData, InstructionFailure, InstructionFingerprint, InstructionPreflightDecision,
    InstructionReconcileKind, InstructionReconcileRequest,
};
use crate::skills::SkillStoreHandle;
use crate::tools::PreparedEdit;
use crate::tools::execute_model_bash_for_runtime;
use crate::{
    AgentEvent, AgentToolCall, PermissionMode, ProcessSupervisor, ResourceLimitDetail,
    SkillInvocationOutcome, SkillInvocationSource, ToolAccess, ToolContext, ToolError,
    ToolRegistry, ToolResult,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToolSchedulingClass {
    ParallelSafe,
    Exclusive,
    BlockingDialog,
}

pub(super) fn terminates_tool_batch(tool_results: &[(AgentToolCall, ToolResult)]) -> bool {
    !tool_results.is_empty() && tool_results.iter().all(|(_, result)| result.terminate)
}

pub(super) fn continues_after_terminating_batch(
    tool_results: &[(AgentToolCall, ToolResult)],
) -> bool {
    tool_results.iter().any(|(call, result)| {
        // Mode transitions terminate their batch (so the runtime can fire the
        // mode-switch side effects keyed off `result.terminate`), but the loop
        // generally keeps going so the model can act on the result: continue
        // planning after EnterPlanMode, execute the approved plan after
        // ExitPlanMode. Only the successful branch continues; a rejected/revised
        // ExitPlanMode returns a non-terminating synthesized result and never
        // reaches this predicate.
        //
        // ExitGoalMode is intentionally excluded: it starts the durable goal,
        // and goal continuation (`goal_continuation_messages`) drives subsequent
        // turns on the next `run_agent_turn` entry by design. Continuing inline
        // here would re-feed the continuation message every turn and spin.
        !result.is_error && matches!(call.name.as_ref(), "EnterPlanMode" | "ExitPlanMode")
    })
}

/// Parse raw arguments for every tool call up front, returning a vec of
/// `(tool_call, parsed_arguments_or_error_result)`. Invalid arguments produce
/// a `ToolResult` error that short-circuits execution for that call.
fn prepare_tool_calls_for_execution<'a>(
    tool_calls: &'a [AgentToolCall],
    tool_specs: &[neo_ai::ToolSpec],
) -> Vec<(
    &'a AgentToolCall,
    Result<super::tool_arguments::PreparedToolCall, ToolResult>,
)> {
    tool_calls
        .iter()
        .map(|tool_call| {
            let prepared = prepare_tool_arguments(tool_call, tool_specs);
            if let Ok(ref parsed) = prepared
                && let Some(warning) = &parsed.warning
            {
                emit_repaired_tool_arguments_warning(&parsed.name, warning);
            }
            (tool_call, prepared)
        })
        .collect()
}

pub fn emit_repaired_tool_arguments_warning(tool_name: &str, warning: &str) {
    tracing::warn!(tool_name, warning, "tool arguments repaired");
}

// ---------------------------------------------------------------------------
// Batch pipeline outcome
// ---------------------------------------------------------------------------

/// The outcome of one assistant tool-call batch: the provider-valid results
/// in original call order plus, for deferred/blocked batches, the instruction
/// epoch the turn loop must emit after appending the results.
pub(super) struct ToolBatchOutcome {
    pub results: Vec<(AgentToolCall, ToolResult)>,
    /// Fresh epoch to emit after the tool results (defer/block). `None` for
    /// executed batches and for policy-blocked batches (whose blocked epoch
    /// is already visible).
    pub pending_epoch: Option<PendingInstructionEpoch>,
    /// Whether any tool body actually ran (drives post-tool reconciliation).
    pub executed_any: bool,
    /// Frozen preflight probe targets. `Some` whenever a registry is
    /// attached — also when empty — so post-tool reconciliation can run;
    /// `None` when instruction preflight is not wired at all.
    pub preflight_targets: Option<Vec<PathBuf>>,
}

/// A fresh defer/block epoch plus the fingerprint to record after emission.
pub(super) struct PendingInstructionEpoch {
    pub epoch: InstructionEpochData,
    pub fingerprint: InstructionFingerprint,
}

// ---------------------------------------------------------------------------
// Instruction preflight
// ---------------------------------------------------------------------------

/// Typed probe targets for one batch: deduplicated reconcile targets plus a
/// per-call probe collection used for blocked-scope coverage checks.
struct BatchProbes {
    targets: Vec<PathBuf>,
    /// Per call, every distinct probe directory in declaration order.
    per_call: Vec<Vec<PathBuf>>,
}

/// The preflight gate for one fully parsed batch.
enum InstructionGate {
    /// No registry, no workspace, or no typed probes: run the legacy path
    /// without a fingerprint recheck.
    Bypass,
    /// Preflight passed; recheck this fingerprint after authorization.
    Proceed(InstructionFingerprint),
    /// A fresh reconciliation deferred or blocked the whole batch.
    Synthesize(Box<PendingInstructionEpoch>),
    /// A previously visible blocked scope still governs a mutation/execution
    /// call in this batch; block the whole batch without a new epoch.
    PolicyBlocked(BlockedInstructionScope),
}

fn current_agent_id(config: &AgentConfig) -> String {
    config
        .agent_id
        .clone()
        .unwrap_or_else(|| crate::session::MAIN_AGENT_ID.to_owned())
}

fn canonical_lenient(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Mutation/execution tool classes that stay blocked while their scope is
/// blocked (read-only `Read`/`List`/`Grep`/`Find`/`Glob` may diagnose).
fn is_mutation_or_execution_tool(name: &str) -> bool {
    matches!(name, "Write" | "Edit" | "Bash" | "Terminal")
}

/// Directories governed by one fresh decision: the batch's canonical probe
/// targets plus the epoch's scope directories.
fn governed_directories(targets: &[PathBuf], epoch: &InstructionEpochData) -> Vec<PathBuf> {
    let mut directories = targets.to_vec();
    for scope in &epoch.scopes {
        if !directories.contains(&scope.display_path) {
            directories.push(scope.display_path.clone());
        }
    }
    directories
}

/// Collect typed probes for every valid call in the batch. Returns `None`
/// when no registry or workspace root is available (preflight unwired).
fn collect_batch_probes(
    workspace: &Path,
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
) -> BatchProbes {
    let mut targets: Vec<PathBuf> = Vec::new();
    let mut per_call = Vec::with_capacity(prepared.len());
    for (tool_call, parsed) in prepared {
        let call_probes = parsed.as_ref().ok().map_or_else(Vec::new, |prepared_call| {
            InstructionScopeProbe::from_prepared_tool(
                &tool_call.name,
                &prepared_call.arguments,
                workspace,
            )
            .into_iter()
            .map(|probe| canonical_lenient(&probe.target_directory))
            .collect::<Vec<_>>()
        });
        for dir in &call_probes {
            if !targets.contains(dir) {
                targets.push(dir.clone());
            }
        }
        per_call.push(call_probes);
    }
    BatchProbes { targets, per_call }
}

/// Run instruction preflight for the whole parsed batch. Preflight runs
/// before any permission prompt, scheduling, `before_tool_call`,
/// `ToolExecutionStarted`, or tool body: a deferred or blocked batch never
/// partially executes.
async fn instruction_batch_preflight(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
) -> (InstructionGate, Option<BatchProbes>) {
    let Some(registry) = emitter.context.instruction_registry() else {
        return (InstructionGate::Bypass, None);
    };
    let Some(workspace) = config.workspace_root.clone() else {
        return (InstructionGate::Bypass, None);
    };
    let probes = collect_batch_probes(&workspace, prepared);
    if probes.targets.is_empty() {
        // No typed probes: nothing new to discover before this batch.
        return (InstructionGate::Bypass, Some(probes));
    }
    let agent_id = current_agent_id(config);
    let deferred_tool_ids: Vec<String> = prepared
        .iter()
        .filter(|(_, parsed)| parsed.is_ok())
        .map(|(tool_call, _)| tool_call.id.to_string())
        .collect();
    let request = InstructionReconcileRequest {
        agent_id: agent_id.clone(),
        kind: InstructionReconcileKind::ToolPreflight,
        target_directories: probes.targets.clone(),
        budget: InstructionContextBridge::budget(config, &emitter.context),
        deferred_tool_ids,
    };
    match registry
        .reconcile(request, emitter.context.instruction_state())
        .await
    {
        InstructionPreflightDecision::Proceed { fingerprint } => {
            clear_stale_blocked_scopes(config, &agent_id, &probes.targets, &fingerprint.hash);
            if let Some(blocked) =
                current_blocked_scope(config, &agent_id, prepared, &probes, &fingerprint.hash)
            {
                return (InstructionGate::PolicyBlocked(blocked), Some(probes));
            }
            record_decision_fingerprint(&mut emitter.context, &fingerprint);
            (InstructionGate::Proceed(fingerprint), Some(probes))
        }
        InstructionPreflightDecision::Defer { epoch, fingerprint } => {
            clear_covered_blocked_scopes(config, &agent_id, &probes.targets);
            (
                InstructionGate::Synthesize(Box::new(PendingInstructionEpoch {
                    epoch,
                    fingerprint,
                })),
                Some(probes),
            )
        }
        InstructionPreflightDecision::Block { epoch, fingerprint } => (
            InstructionGate::Synthesize(Box::new(PendingInstructionEpoch { epoch, fingerprint })),
            Some(probes),
        ),
    }
}

// ---------------------------------------------------------------------------
// Blocked-scope registry (session-shared, agent-local entries)
// ---------------------------------------------------------------------------

/// Register one freshly blocked scope for `agent_id`, replacing any entry
/// with the same fingerprint.
pub(super) fn register_blocked_scope(
    config: &AgentConfig,
    agent_id: &str,
    fingerprint: &str,
    directories: Vec<PathBuf>,
    failure: InstructionFailure,
    generation: u64,
) {
    let mut store = config
        .blocked_instruction_scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store.retain(|entry| !(entry.agent_id == agent_id && entry.fingerprint == fingerprint));
    store.push(BlockedInstructionScope {
        agent_id: agent_id.to_owned(),
        fingerprint: fingerprint.to_owned(),
        directories,
        failure,
        generation,
    });
}

/// Drop entries for `agent_id` covered by `targets` whose fingerprint no
/// longer matches the fresh reconciliation: the failure was fixed or
/// re-resolved, so the blocked state is stale.
fn clear_stale_blocked_scopes(
    config: &AgentConfig,
    agent_id: &str,
    targets: &[PathBuf],
    fresh_hash: &str,
) {
    let mut store = config
        .blocked_instruction_scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store.retain(|entry| {
        if entry.agent_id != agent_id {
            return true;
        }
        let covered = entry
            .directories
            .iter()
            .any(|dir| targets.iter().any(|target| target.starts_with(dir)));
        !covered || entry.fingerprint == fresh_hash
    });
}

/// Drop every entry for `agent_id` covered by `targets`: a successful
/// resolution replaced the failure state.
fn clear_covered_blocked_scopes(config: &AgentConfig, agent_id: &str, targets: &[PathBuf]) {
    let mut store = config
        .blocked_instruction_scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store.retain(|entry| {
        if entry.agent_id != agent_id {
            return true;
        }
        !entry
            .directories
            .iter()
            .any(|dir| targets.iter().any(|target| target.starts_with(dir)))
    });
}

/// The first still-current blocked entry governing a mutation/execution
/// call in this batch. An entry is current when the fresh reconciliation of
/// the same targets reproduces its failure fingerprint.
fn current_blocked_scope(
    config: &AgentConfig,
    agent_id: &str,
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
    probes: &BatchProbes,
    fresh_hash: &str,
) -> Option<BlockedInstructionScope> {
    let store = config
        .blocked_instruction_scopes
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    store
        .iter()
        .find(|entry| {
            if entry.agent_id != agent_id || entry.fingerprint != fresh_hash {
                return false;
            }
            prepared
                .iter()
                .zip(probes.per_call.iter())
                .any(|((tool_call, parsed), call_probes)| {
                    parsed.is_ok()
                        && is_mutation_or_execution_tool(tool_call.name.as_ref())
                        && call_probes
                            .iter()
                            .any(|probe| entry.directories.iter().any(|dir| probe.starts_with(dir)))
                })
        })
        .cloned()
}

/// Record the decision fingerprint so an unchanged re-probe proceeds
/// silently. Callers that emit the epoch through an `EventEmitter` must call
/// this after the emission (the emitter applies visibility only).
pub(super) fn record_decision_fingerprint(
    context: &mut AgentContext,
    fingerprint: &InstructionFingerprint,
) {
    context.instruction_state_mut().last_epoch_fingerprint = Some(fingerprint.hash.clone());
}

/// Apply one reconciliation decision made outside the tool-batch path
/// (baseline establishment, post-tool reconciliation): emit the epoch when
/// the decision produced one, record the decision fingerprint, and maintain
/// the session blocked-scope registry. Returns the emitted epoch, if any.
pub(super) fn apply_reconciliation_decision(
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    decision: InstructionPreflightDecision,
    targets: &[PathBuf],
) -> Option<InstructionEpochData> {
    let agent_id = current_agent_id(config);
    match decision {
        InstructionPreflightDecision::Proceed { fingerprint } => {
            clear_stale_blocked_scopes(config, &agent_id, targets, &fingerprint.hash);
            record_decision_fingerprint(&mut emitter.context, &fingerprint);
            None
        }
        InstructionPreflightDecision::Defer { epoch, fingerprint } => {
            clear_covered_blocked_scopes(config, &agent_id, targets);
            record_decision_fingerprint_after_epoch(emitter, &epoch, &fingerprint);
            Some(epoch)
        }
        InstructionPreflightDecision::Block { epoch, fingerprint } => {
            let governed = governed_directories(targets, &epoch);
            if let Some(failure) = epoch.failure.clone() {
                register_blocked_scope(
                    config,
                    &agent_id,
                    &fingerprint.hash,
                    governed,
                    failure,
                    epoch.generation,
                );
            }
            record_decision_fingerprint_after_epoch(emitter, &epoch, &fingerprint);
            Some(epoch)
        }
    }
}

/// Emit the epoch event (which applies model visibility) and then record the
/// decision fingerprint.
fn record_decision_fingerprint_after_epoch(
    emitter: &mut EventEmitter,
    epoch: &InstructionEpochData,
    fingerprint: &InstructionFingerprint,
) {
    emitter.emit(AgentEvent::InstructionEpoch {
        epoch: epoch.clone(),
    });
    record_decision_fingerprint(&mut emitter.context, fingerprint);
}

// ---------------------------------------------------------------------------
// Synthesized deferred/blocked results
// ---------------------------------------------------------------------------

/// One provider-valid non-error deferred result: no side effect occurred
/// because project instructions changed; the model re-issues the call after
/// reading the new epoch.
fn deferred_tool_result(epoch: &InstructionEpochData) -> ToolResult {
    ToolResult::ok(format!(
        "Tool call deferred: project instructions changed (instruction epoch {}). No side \
         effect occurred. Read the new instructions, then re-issue the tool call.",
        epoch.generation
    ))
    .with_details(serde_json::json!({
        "status": "deferred",
        "reason": "instruction_epoch",
        "side_effect_occurred": false,
        "generation": epoch.generation,
    }))
}

/// One structured blocked result for a scope whose failure epoch is (or is
/// about to be) visible.
fn blocked_tool_result(failure: &InstructionFailure, generation: u64) -> ToolResult {
    ToolResult::error(format!(
        "Tool call blocked: instruction scope blocked ({}): {}. No side effect occurred. \
         Resolve the instruction problem to load the scope; read-only Read, List, Grep, Find, \
         and Glob diagnosis is allowed.",
        failure.kind.describe(),
        failure.detail
    ))
    .with_details(serde_json::json!({
        "status": "blocked",
        "reason": "instruction_scope_blocked",
        "side_effect_occurred": false,
        "generation": generation,
        "failure": {
            "kind": failure.kind.describe(),
            "path": failure.display_path.display().to_string(),
            "detail": failure.detail,
        },
    }))
}

/// Build results for a batch stopped by a fresh defer/block decision: every
/// valid call receives the deferred/blocked payload; invalid-argument calls
/// keep their parse-error results.
fn synthesized_batch_results(
    prepared: &[(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
    epoch: &InstructionEpochData,
) -> Vec<(AgentToolCall, ToolResult)> {
    prepared
        .iter()
        .map(|(tool_call, parsed)| {
            let result = match parsed {
                Ok(_) => match &epoch.failure {
                    Some(failure) => blocked_tool_result(failure, epoch.generation),
                    None => deferred_tool_result(epoch),
                },
                Err(error) => error.clone(),
            };
            ((*tool_call).clone(), result)
        })
        .collect()
}

/// Emit plain finished events for synthesized results. No
/// `ToolExecutionStarted`, shell, terminal, or skill side events fire for
/// calls that never began execution.
fn emit_synthesized_finished(
    turn: u32,
    results: &[(AgentToolCall, ToolResult)],
    emitter: &mut EventEmitter,
) {
    for (tool_call, result) in results {
        emitter.emit(AgentEvent::ToolExecutionFinished {
            turn,
            id: tool_call.id.to_string(),
            name: tool_call.name.to_string(),
            result: result.clone(),
        });
    }
}

// ---------------------------------------------------------------------------
// Prepared Edit phase
// ---------------------------------------------------------------------------

/// Prepare every successfully parsed Edit call without side effects. Failures
/// replace the prepared call with a terminal `prepare_failed` result so Ask
/// mode never opens an approval dialog.
async fn prepare_edit_calls(
    tool_context: &ToolContext,
    prepared: &mut [(
        &AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )],
) {
    for (tool_call, parsed) in prepared.iter_mut() {
        let Ok(prepared_call) = parsed else {
            continue;
        };
        if prepared_call.name != "Edit" {
            continue;
        }
        // Prepare with write access only for path resolution; no writes occur.
        let prepare_ctx = tool_context.clone().with_access(ToolAccess {
            file_write: true,
            ..ToolAccess::none()
        });
        match PreparedEdit::prepare(&prepare_ctx, &prepared_call.arguments).await {
            Ok(edit) => {
                prepared_call.execution = PreparedExecution::Edit(edit);
            }
            Err(result) => {
                *parsed = Err(result);
            }
        }
        let _ = tool_call;
    }
}

/// Recheck every authorized prepared Edit. Stale targets become terminal
/// results with zero writes.
async fn recheck_prepared_edits(authorized: &mut [AuthorizedToolCall<'_>]) {
    for entry in authorized.iter_mut() {
        if !matches!(entry.outcome, AuthorizedToolCallOutcome::Run { .. }) {
            continue;
        }
        let Some(prepared) = entry.prepared.as_ref() else {
            continue;
        };
        let PreparedExecution::Edit(edit) = &prepared.execution else {
            continue;
        };
        if let Err(result) = edit.recheck_all().await {
            entry.outcome = AuthorizedToolCallOutcome::Terminal(result);
        }
    }
}

// ---------------------------------------------------------------------------
// Authorization phase
// ---------------------------------------------------------------------------

/// One authorized call: its scheduling class and the authorization outcome.
///
/// Plan/Goal execution metadata lives only on [`PreparedToolCall::approval`]
/// after a successful allow — not on a separate outcome field or id map.
struct AuthorizedToolCall<'a> {
    tool_call: &'a AgentToolCall,
    prepared: Option<super::tool_arguments::PreparedToolCall>,
    class: ToolSchedulingClass,
    outcome: AuthorizedToolCallOutcome,
}

enum AuthorizedToolCallOutcome {
    /// Approved to run with this access grant.
    Run { access: ToolAccess },
    /// Terminal result without running the body (invalid arguments,
    /// `before_tool_call` block, permission denial, or revision feedback).
    Terminal(ToolResult),
}

/// Build the permission decision for every call in the batch. Dialogs await
/// sequentially; `before_tool_call` runs here, after instruction preflight
/// and before the fingerprint recheck.
///
/// Consumes the prepared batch so each call can be re-owned with
/// `PreparedToolCall.approval` written from the validated resolution.
async fn authorize_tool_batch<'a>(
    config: &AgentConfig,
    prepared: Vec<(
        &'a AgentToolCall,
        Result<super::tool_arguments::PreparedToolCall, ToolResult>,
    )>,
    turn: u32,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Vec<AuthorizedToolCall<'a>> {
    let mut authorized = Vec::with_capacity(prepared.len());
    for (tool_call, parsed) in prepared {
        let entry = match parsed {
            Err(error) => AuthorizedToolCall {
                tool_call,
                prepared: None,
                class: ToolSchedulingClass::ParallelSafe,
                outcome: AuthorizedToolCallOutcome::Terminal(error),
            },
            Ok(mut prepared_call) => {
                let preparation =
                    permission_preparation_for_mode(config, tool_call, &prepared_call);
                let class = scheduling_class_for_preparation(
                    config,
                    tool_call,
                    &preparation,
                    &prepared_call.arguments,
                );
                let outcome = if let Some(blocked) =
                    before_tool_result(config, tool_call, cancel_token).await
                {
                    AuthorizedToolCallOutcome::Terminal(blocked)
                } else {
                    match resolve_permission_preparation(
                        config,
                        preparation,
                        tool_call,
                        &prepared_call,
                        turn,
                        emitter,
                        cancel_token,
                    )
                    .await
                    {
                        super::permission::PermissionResolution::Run { access, approval } => {
                            // Single home for Plan/Goal execution context.
                            prepared_call.approval = approval;
                            AuthorizedToolCallOutcome::Run { access }
                        }
                        super::permission::PermissionResolution::Terminal(result) => {
                            AuthorizedToolCallOutcome::Terminal(result)
                        }
                    }
                };
                AuthorizedToolCall {
                    tool_call,
                    prepared: Some(prepared_call),
                    class,
                    outcome,
                }
            }
        };
        authorized.push(entry);
    }
    authorized
}

// ---------------------------------------------------------------------------
// Batch execution
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) async fn execute_tool_calls(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStoreHandle>,
    turn: u32,
    tool_calls: &[AgentToolCall],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<ToolBatchOutcome, AgentRuntimeError> {
    // Phase 1 — parse every call up front. Invalid arguments produce valid
    // error results later without letting valid calls bypass preflight.
    let tool_specs = registry.specs();
    let prepared = prepare_tool_calls_for_execution(tool_calls, &tool_specs);

    // Phase 2 — instruction preflight over all typed probes. Nothing below
    // runs before preflight returns Proceed: no permission prompt,
    // scheduling, `before_tool_call`, `ToolExecutionStarted`, or tool body.
    let (gate, probes) = instruction_batch_preflight(config, emitter, &prepared).await;
    let preflight_targets = probes.as_ref().map(|probes| probes.targets.clone());
    let fingerprint = match gate {
        InstructionGate::Bypass => None,
        InstructionGate::Proceed(fingerprint) => Some(fingerprint),
        InstructionGate::Synthesize(pending) => {
            let PendingInstructionEpoch { epoch, fingerprint } = *pending;
            if let Some(failure) = epoch.failure.clone() {
                let targets = preflight_targets.clone().unwrap_or_default();
                register_blocked_scope(
                    config,
                    &epoch.agent_id,
                    &fingerprint.hash,
                    governed_directories(&targets, &epoch),
                    failure,
                    epoch.generation,
                );
            }
            let results = synthesized_batch_results(&prepared, &epoch);
            emit_synthesized_finished(turn, &results, emitter);
            return Ok(ToolBatchOutcome {
                results,
                pending_epoch: Some(PendingInstructionEpoch { epoch, fingerprint }),
                executed_any: false,
                preflight_targets,
            });
        }
        InstructionGate::PolicyBlocked(blocked) => {
            let results = prepared
                .iter()
                .map(|(tool_call, parsed)| {
                    let result = match parsed {
                        Ok(_) => blocked_tool_result(&blocked.failure, blocked.generation),
                        Err(error) => error.clone(),
                    };
                    ((*tool_call).clone(), result)
                })
                .collect::<Vec<_>>();
            emit_synthesized_finished(turn, &results, emitter);
            return Ok(ToolBatchOutcome {
                results,
                pending_epoch: None,
                executed_any: false,
                preflight_targets,
            });
        }
    };

    // Phase 3 — construct the base ToolContext (no tool body yet) and prepare
    // every Edit call side-effect free. Prepare failures become terminal
    // results before permission dialogs.
    let tool_context = default_tool_context(
        config,
        Arc::clone(&model),
        Arc::clone(&registry),
        turn,
        cancel_token,
        process_supervisor.clone(),
        emitter
            .context
            .instruction_registry()
            .is_some()
            .then(|| emitter.context.instruction_state().clone()),
    )?;
    let mut prepared = prepared;
    prepare_edit_calls(&tool_context, &mut prepared).await;

    // Phase 4 — authorize the full batch (dialogs await sequentially).
    // Consumes `prepared` so Allow can write Plan/Goal context onto
    // `PreparedToolCall.approval` (single transport home).
    let mut authorized = authorize_tool_batch(config, prepared, turn, emitter, cancel_token).await;

    // Phase 5 — one frozen fingerprint recheck after all authorization. A
    // source changed while a dialog waited returns to the defer path instead
    // of executing against stale instructions.
    if let Some(fingerprint) = fingerprint {
        let registry_handle = emitter
            .context
            .instruction_registry()
            .expect("a proceeded gate implies an attached registry");
        match registry_handle
            .recheck(&fingerprint, emitter.context.instruction_state())
            .await
        {
            InstructionPreflightDecision::Proceed { fingerprint } => {
                record_decision_fingerprint(&mut emitter.context, &fingerprint);
            }
            InstructionPreflightDecision::Defer { epoch, fingerprint }
            | InstructionPreflightDecision::Block { epoch, fingerprint } => {
                let targets = preflight_targets.clone().unwrap_or_default();
                let governed = governed_directories(&targets, &epoch);
                if let Some(failure) = epoch.failure.clone() {
                    register_blocked_scope(
                        config,
                        &epoch.agent_id,
                        &fingerprint.hash,
                        governed.clone(),
                        failure,
                        epoch.generation,
                    );
                } else {
                    clear_covered_blocked_scopes(config, &epoch.agent_id, &targets);
                }
                // Calls already denied or invalid keep their results; only
                // approved calls are deferred.
                let results = authorized
                    .iter()
                    .map(|entry| {
                        let result = match &entry.outcome {
                            AuthorizedToolCallOutcome::Run { .. } => match &epoch.failure {
                                Some(failure) => blocked_tool_result(failure, epoch.generation),
                                None => deferred_tool_result(&epoch),
                            },
                            AuthorizedToolCallOutcome::Terminal(result) => result.clone(),
                        };
                        (entry.tool_call.clone(), result)
                    })
                    .collect::<Vec<_>>();
                emit_synthesized_finished(turn, &results, emitter);
                return Ok(ToolBatchOutcome {
                    results,
                    pending_epoch: Some(PendingInstructionEpoch { epoch, fingerprint }),
                    executed_any: false,
                    preflight_targets,
                });
            }
        }
    }

    // Phase 6 — recheck every prepared Edit target after approval and instruction
    // recheck. Stale targets become terminal results with zero writes.
    recheck_prepared_edits(&mut authorized).await;

    // Phase 7 — schedule and execute the authorized batch.
    let needs_sequential = matches!(config.tool_execution_mode, ToolExecutionMode::Sequential)
        || authorized
            .iter()
            .any(|entry| entry.class != ToolSchedulingClass::ParallelSafe);

    let (mut results, executed_any) = if needs_sequential {
        execute_authorized_sequential(
            config,
            Arc::clone(&registry),
            skills,
            turn,
            &authorized,
            &tool_context,
            emitter,
            cancel_token,
        )
        .await?
    } else {
        execute_authorized_parallel(
            config,
            Arc::clone(&registry),
            skills,
            turn,
            &authorized,
            &tool_context,
            emitter,
            cancel_token,
        )
        .await?
    };

    // Attach plan details while plan mode is still active, before the side-effect
    // events below flip it off. Selection decoration reads
    // `PreparedToolCall.approval` in batch order — no tool-id map.
    attach_exit_plan_details(
        config,
        &mut results,
        authorized
            .iter()
            .map(|entry| entry.prepared.as_ref().and_then(|p| p.approval.as_ref())),
    );
    // Re-emit the finished event for ExitPlanMode so the TUI can render the
    // plan box from the freshly attached details.
    for (tool_call, result) in &results {
        if tool_call.name.as_ref() == "ExitPlanMode" {
            emitter.emit(AgentEvent::ToolExecutionFinished {
                turn,
                id: tool_call.id.to_string(),
                name: tool_call.name.to_string(),
                result: result.clone(),
            });
        }
    }
    Ok(ToolBatchOutcome {
        results,
        pending_epoch: None,
        executed_any,
        preflight_targets,
    })
}

fn emit_tool_execution_finished(
    turn: u32,
    tool_call: &AgentToolCall,
    arguments: Option<&serde_json::Value>,
    result: &ToolResult,
    emitter: &mut EventEmitter,
) {
    if tool_call.name.as_ref() == "Skill" {
        let raw_arguments;
        let arguments = if let Some(arguments) = arguments {
            arguments
        } else {
            raw_arguments =
                serde_json::from_str(&tool_call.raw_arguments).unwrap_or(serde_json::Value::Null);
            &raw_arguments
        };
        let name = arguments
            .get("skill")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_owned();
        emitter.emit(AgentEvent::SkillInvocation {
            names: vec![name],
            source: SkillInvocationSource::Auto,
            outcome: if result.is_error {
                SkillInvocationOutcome::Failed
            } else {
                SkillInvocationOutcome::Activated
            },
            body: if result.is_error {
                result.content.clone()
            } else {
                format_skill_tool_arguments(arguments)
            },
        });
    }
    emitter.emit(AgentEvent::ToolExecutionFinished {
        turn,
        id: tool_call.id.to_string(),
        name: tool_call.name.to_string(),
        result: result.clone(),
    });
}

fn scheduling_class_for_preparation(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    preparation: &super::permission::PermissionPreparation,
    arguments: &serde_json::Value,
) -> ToolSchedulingClass {
    if matches!(
        preparation,
        super::permission::PermissionPreparation::Ask { .. }
    ) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name.as_ref() == "AskUserQuestion" && !ask_user_runs_in_background(arguments) {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name.as_ref() == "ExitPlanMode"
        && current_permission_mode(config) != PermissionMode::Auto
        && exit_plan_mode_has_reviewable_plan(config)
    {
        return ToolSchedulingClass::BlockingDialog;
    }
    if tool_call.name.as_ref() == "ExitGoalMode"
        && current_permission_mode(config) != PermissionMode::Auto
    {
        return ToolSchedulingClass::BlockingDialog;
    }
    let name = tool_call.name.as_ref();
    // ShellScheduler owns concurrency for commands that acquire admission.
    if matches!(name, "Write" | "Edit")
        || (name == "Terminal" && !uses_shell_admission(name, arguments))
    {
        return ToolSchedulingClass::Exclusive;
    }
    ToolSchedulingClass::ParallelSafe
}

pub(super) fn ask_user_runs_in_background(arguments: &serde_json::Value) -> bool {
    arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn uses_shell_admission(name: &str, arguments: &serde_json::Value) -> bool {
    name == "Bash"
        || (name == "Terminal"
            && arguments.get("mode").and_then(serde_json::Value::as_str) == Some("start"))
}

#[allow(clippy::too_many_arguments)]
async fn execute_authorized_sequential(
    config: &AgentConfig,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStoreHandle>,
    turn: u32,
    authorized: &[AuthorizedToolCall<'_>],
    tool_context: &ToolContext,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<(Vec<(AgentToolCall, ToolResult)>, bool), AgentRuntimeError> {
    let mut results = Vec::new();
    let mut executed_any = false;
    for entry in authorized {
        let tool_call = entry.tool_call;
        let Some(prepared_call) = entry.prepared.as_ref() else {
            // Invalid arguments: emit a finished error without starting execution.
            let AuthorizedToolCallOutcome::Terminal(result) = &entry.outcome else {
                unreachable!("unparsed calls always carry a terminal result");
            };
            emit_tool_execution_finished(turn, tool_call, None, result, emitter);
            results.push((tool_call.clone(), result.clone()));
            continue;
        };
        let AuthorizedToolCallOutcome::Run { access } = &entry.outcome else {
            // Terminal result (before-hook block or permission denial).
            let AuthorizedToolCallOutcome::Terminal(result) = &entry.outcome else {
                unreachable!("matched above");
            };
            let mut result = result.clone();
            if !cancel_token.is_cancelled() {
                result = after_tool_result(config, tool_call, result, cancel_token).await;
            }
            emit_authorized_call_result(
                turn,
                tool_call,
                Some(&prepared_call.arguments),
                &result,
                tool_context,
                emitter,
            );
            results.push((tool_call.clone(), result));
            if cancel_token.is_cancelled() {
                break;
            }
            continue;
        };
        let sink = emitter.sink();
        let arguments = Arc::new(prepared_call.arguments.clone());
        let mut context = tool_context
            .clone()
            .with_access(*access)
            .with_tool_update(make_tool_update_callback(
                sink.clone(),
                turn,
                tool_call.id.to_string(),
                tool_call.name.to_string(),
            ))
            .with_tool_event(make_tool_event_callback(sink.clone()));
        if uses_shell_admission(tool_call.name.as_ref(), arguments.as_ref()) {
            let workspace_root = context.workspace_root().to_path_buf();
            context = context.with_shell_admission_callback(make_shell_admission_callback(
                sink.clone(),
                turn,
                tool_call.id.to_string(),
                tool_call.name.to_string(),
                Arc::clone(&arguments),
                workspace_root,
            ));
        } else {
            emitter.emit(AgentEvent::ToolExecutionStarted {
                turn,
                id: tool_call.id.to_string(),
                name: tool_call.name.to_string(),
                arguments: arguments.as_ref().clone(),
            });
        }
        let mut result = if let PreparedExecution::Edit(edit) = &prepared_call.execution {
            // Emit verified planned projection before the first commit.
            emitter.emit(AgentEvent::ToolExecutionUpdate {
                turn,
                id: tool_call.id.to_string(),
                name: tool_call.name.to_string(),
                partial_result: edit.prepared_update(),
            });
            let progress_sink = sink;
            let progress_id = tool_call.id.to_string();
            let progress_name = tool_call.name.to_string();
            let mut on_progress = move |update: ToolResult| {
                progress_sink.emit_event(AgentEvent::ToolExecutionUpdate {
                    turn,
                    id: progress_id.clone(),
                    name: progress_name.clone(),
                    partial_result: update,
                });
            };
            edit.commit(cancel_token, &mut on_progress).await
        } else {
            run_tool_with_cancel(
                skills,
                registry.as_ref(),
                tool_call,
                arguments.as_ref(),
                &context,
                cancel_token,
            )
            .await
        };
        executed_any = true;
        if !cancel_token.is_cancelled() {
            result = after_tool_result(config, tool_call, result, cancel_token).await;
        }
        emit_authorized_call_result(
            turn,
            tool_call,
            Some(arguments.as_ref()),
            &result,
            tool_context,
            emitter,
        );
        results.push((tool_call.clone(), result));
        if cancel_token.is_cancelled() {
            break;
        }
    }
    Ok((results, executed_any))
}

fn emit_authorized_call_result(
    turn: u32,
    tool_call: &AgentToolCall,
    arguments: Option<&serde_json::Value>,
    result: &ToolResult,
    tool_context: &ToolContext,
    emitter: &mut EventEmitter,
) {
    emit_shell_finished(turn, tool_call, result, emitter);
    emit_terminal_events(turn, arguments, tool_call, result, tool_context, emitter);
    emit_tool_execution_finished(turn, tool_call, arguments, result, emitter);
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn execute_authorized_parallel(
    config: &AgentConfig,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStoreHandle>,
    turn: u32,
    authorized: &[AuthorizedToolCall<'_>],
    tool_context: &ToolContext,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<(Vec<(AgentToolCall, ToolResult)>, bool), AgentRuntimeError> {
    let mut completed = Vec::with_capacity(authorized.len());
    let mut running = FuturesUnordered::new();
    let mut executed_any = false;

    for (index, entry) in authorized.iter().enumerate() {
        if cancel_token.is_cancelled() {
            break;
        }
        let tool_call = entry.tool_call;
        let Some(prepared_call) = entry.prepared.as_ref() else {
            // Invalid arguments: emit a finished error without starting execution.
            let AuthorizedToolCallOutcome::Terminal(result) = &entry.outcome else {
                unreachable!("unparsed calls always carry a terminal result");
            };
            emit_tool_execution_finished(turn, tool_call, None, result, emitter);
            completed.push((index, tool_call.clone(), result.clone()));
            continue;
        };
        let AuthorizedToolCallOutcome::Run { access } = &entry.outcome else {
            // Terminal result (before-hook block or permission denial).
            let AuthorizedToolCallOutcome::Terminal(result) = &entry.outcome else {
                unreachable!("matched above");
            };
            let mut result = result.clone();
            if !cancel_token.is_cancelled() {
                result = after_tool_result(config, tool_call, result, cancel_token).await;
            }
            emit_shell_finished(turn, tool_call, &result, emitter);
            emit_terminal_events(
                turn,
                Some(&prepared_call.arguments),
                tool_call,
                &result,
                tool_context,
                emitter,
            );
            emit_tool_execution_finished(
                turn,
                tool_call,
                Some(&prepared_call.arguments),
                &result,
                emitter,
            );
            completed.push((index, tool_call.clone(), result));
            continue;
        };

        let config = config.clone();
        let registry = Arc::clone(&registry);
        let tool_context = tool_context.clone().with_access(*access);
        let cancel_token = cancel_token.clone();
        let sink = emitter.sink();
        let arguments = Arc::new(prepared_call.arguments.clone());
        let uses_admission = uses_shell_admission(tool_call.name.as_ref(), arguments.as_ref());
        if !uses_admission {
            emitter.emit(AgentEvent::ToolExecutionStarted {
                turn,
                id: tool_call.id.to_string(),
                name: tool_call.name.to_string(),
                arguments: arguments.as_ref().clone(),
            });
        }
        running.push(async move {
            let mut tool_context = tool_context
                .with_tool_update(make_tool_update_callback(
                    sink.clone(),
                    turn,
                    tool_call.id.to_string(),
                    tool_call.name.to_string(),
                ))
                .with_tool_event(make_tool_event_callback(sink.clone()));
            if uses_admission {
                let workspace_root = tool_context.workspace_root().to_path_buf();
                tool_context =
                    tool_context.with_shell_admission_callback(make_shell_admission_callback(
                        sink,
                        turn,
                        tool_call.id.to_string(),
                        tool_call.name.to_string(),
                        Arc::clone(&arguments),
                        workspace_root,
                    ));
            }
            let mut result = run_tool_with_cancel(
                skills,
                registry.as_ref(),
                tool_call,
                arguments.as_ref(),
                &tool_context,
                &cancel_token,
            )
            .await;
            if !cancel_token.is_cancelled() {
                result = after_tool_result(&config, tool_call, result, &cancel_token).await;
            }
            Ok::<_, AgentRuntimeError>((index, (*tool_call).clone(), result))
        });
    }

    while let Some(outcome) = running.next().await {
        let (index, tool_call, result) = outcome?;
        executed_any = true;
        let arguments = match authorized[index].prepared.as_ref() {
            Some(p) => p.arguments.clone(),
            None => serde_json::Value::Null,
        };
        emit_shell_finished(turn, &tool_call, &result, emitter);
        emit_terminal_events(
            turn,
            Some(&arguments),
            &tool_call,
            &result,
            tool_context,
            emitter,
        );
        emit_tool_execution_finished(turn, &tool_call, Some(&arguments), &result, emitter);
        completed.push((index, tool_call, result));
    }

    completed.sort_by_key(|(index, _, _)| *index);
    Ok((
        completed
            .into_iter()
            .map(|(_, tool_call, result)| (tool_call, result))
            .collect(),
        executed_any,
    ))
}

async fn before_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    cancel_token: &CancellationToken,
) -> Option<ToolResult> {
    if let Some(before_tool_call) = &config.before_tool_call
        && let Some(result) = before_tool_call(tool_call)
    {
        return Some(result);
    }
    let async_before_tool_call = config.async_before_tool_call.as_ref()?;
    tokio::select! {
        biased;
        result = async_before_tool_call(tool_call.clone(), cancel_token.clone()) => result,
        () = cancel_token.cancelled() => Some(cancelled_tool_result()),
    }
}

async fn after_tool_result(
    config: &AgentConfig,
    tool_call: &AgentToolCall,
    mut result: ToolResult,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if let Some(after_tool_call) = &config.after_tool_call {
        result = after_tool_call(tool_call, result);
    }
    let Some(async_after_tool_call) = &config.async_after_tool_call else {
        return result;
    };
    tokio::select! {
        biased;
        result = async_after_tool_call(tool_call.clone(), result, cancel_token.clone()) => result,
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

async fn run_tool_with_cancel(
    skills: Option<&SkillStoreHandle>,
    registry: &ToolRegistry,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    if tool_call.name.as_ref() == "Skill" {
        return execute_invoke_skill(skills, arguments);
    }
    if tool_call.name.as_ref() == "Bash" {
        return run_model_bash_with_cancel(arguments, tool_context, cancel_token).await;
    }
    if matches!(tool_call.name.as_ref(), "Delegate" | "DelegateSwarm") {
        return registry
            .run(&tool_call.name, tool_context, arguments.clone())
            .await
            .unwrap_or_else(|err| ToolResult::error(err.to_string()));
    }
    // Start may need async cleanup after registering a handle the model has not received yet.
    if tool_call.name.as_ref() == "Terminal"
        && arguments.get("mode").and_then(serde_json::Value::as_str) == Some("start")
    {
        return registry
            .run(&tool_call.name, tool_context, arguments.clone())
            .await
            .unwrap_or_else(|err| ToolResult::error(err.to_string()));
    }
    tokio::select! {
        biased;
        result = registry.run(&tool_call.name, tool_context, arguments.clone()) => {
            result.unwrap_or_else(|err| ToolResult::error(err.to_string()))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

pub(super) fn cancelled_tool_result() -> ToolResult {
    ToolResult::error(ToolError::Cancelled.to_string())
}

async fn run_model_bash_with_cancel(
    arguments: &serde_json::Value,
    tool_context: &ToolContext,
    cancel_token: &CancellationToken,
) -> ToolResult {
    tokio::select! {
        biased;
        result = execute_model_bash_for_runtime(tool_context, arguments.clone()) => {
            result.unwrap_or_else(|error| model_bash_error_result(tool_context, &error))
        }
        () = cancel_token.cancelled() => cancelled_tool_result(),
    }
}

fn model_bash_error_result(_tool_context: &ToolContext, error: &ToolError) -> ToolResult {
    match error {
        ToolError::ResourceLimited { cause } => {
            let detail = ResourceLimitDetail {
                cause: *cause,
                configured: None,
                observed: None,
            };
            ToolResult::error(crate::tools::format_resource_limit(Some(&detail))).with_details(
                serde_json::json!({
                    "exit_code": null,
                    "signal": null,
                    "stdout": "",
                    "stderr": "",
                    "stdout_truncated": false,
                    "stderr_truncated": false,
                    "truncated": false,
                    "outcome": "resource_limited",
                    "resource_limit": detail,
                }),
            )
        }
        _ => ToolResult::error(error.to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn default_tool_context(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    turn: u32,
    cancel_token: &CancellationToken,
    process_supervisor: ProcessSupervisor,
    parent_instruction_state: Option<crate::instructions::AgentInstructionState>,
) -> Result<ToolContext, AgentRuntimeError> {
    let workspace_root = if let Some(workspace_root) = &config.workspace_root {
        workspace_root.clone()
    } else {
        std::env::current_dir()?
    };
    let multi_agent = if let Some(session_directory) = &config.session_directory {
        config
            .multi_agent
            .clone()
            .with_session_directory(session_directory.clone())
    } else {
        config.multi_agent.clone()
    };
    let configured_policy = config
        .workspace_policy
        .read()
        .ok()
        .and_then(|policy| policy.clone());
    ToolContext::new(workspace_root)
        .map(|context| {
            let context = if let Some(policy) = configured_policy.clone() {
                context.with_workspace_policy(policy)
            } else {
                context
            };
            let context = context
                .with_access(ToolAccess::none())
                .with_cancel_token(cancel_token.clone())
                .with_process_supervisor(process_supervisor)
                .with_background_tasks(config.background_tasks.clone())
                .with_shell_runtime(config.shell_runtime.clone())
                .with_multi_agent(multi_agent)
                .with_child_runtime(config.clone(), model, registry, turn)
                .with_parent_instruction_state(parent_instruction_state);
            if let Some(session_directory) = &config.session_directory {
                context.with_agent_session_context(
                    session_directory.clone(),
                    config
                        .agent_id
                        .as_deref()
                        .unwrap_or(crate::session::MAIN_AGENT_ID),
                )
            } else {
                context
            }
        })
        .map(|context| {
            // The active plan file lives under the NEO_HOME sessions bucket
            // (outside the workspace). Whitelist it so Write/Edit can resolve
            // the path while plan mode is active; the plan-mode guard and the
            // permission layer still restrict writes to *only* that path.
            let plan_path = config
                .plan_mode
                .read()
                .ok()
                .and_then(|plan_mode| plan_mode.plan_file_path().map(PathBuf::from));
            match plan_path {
                Some(path) => context.with_allowed_external_write_paths([path]),
                None => context,
            }
        })
        .map_err(AgentRuntimeError::Tool)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use neo_ai::ModelClient;
    use serde_json::json;
    use tokio::sync::Notify;
    use tokio_util::sync::CancellationToken;

    use super::{EventEmitter, execute_tool_calls, run_tool_with_cancel};
    use crate::harness::fake_model;
    use crate::runtime::config::{AgentConfig, ToolExecutionMode};
    use crate::tools::{
        ShellAdmissionClass, ShellAdmissionRequest, ShellLimits, ShellRuntime, Tool, ToolContext,
        ToolError, ToolFuture, ToolRegistry,
    };
    use crate::{
        AgentContext, AgentEvent, AgentToolCall, ApprovalAction, ApprovalResponse, PermissionMode,
        ProcessSupervisor,
    };

    struct CancellationSettlingTerminal {
        entered: Arc<Notify>,
        settled: Arc<AtomicBool>,
    }

    impl Tool for CancellationSettlingTerminal {
        fn name(&self) -> &'static str {
            "Terminal"
        }

        fn description(&self) -> &'static str {
            "test terminal"
        }

        fn input_schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }

        fn execute<'a>(
            &'a self,
            ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> ToolFuture<'a> {
            Box::pin(async move {
                self.entered.notify_one();
                ctx.cancel_token.cancelled().await;
                tokio::task::yield_now().await;
                self.settled.store(true, Ordering::SeqCst);
                Err(ToolError::Cancelled)
            })
        }
    }

    #[tokio::test]
    async fn terminal_start_cancellation_allows_internal_cleanup_to_settle() {
        let workspace = tempfile::tempdir().expect("workspace");
        let cancel = CancellationToken::new();
        let context = ToolContext::new(workspace.path())
            .expect("tool context")
            .with_cancel_token(cancel.clone());
        let entered = Arc::new(Notify::new());
        let settled = Arc::new(AtomicBool::new(false));
        let mut registry = ToolRegistry::new();
        registry.register(CancellationSettlingTerminal {
            entered: Arc::clone(&entered),
            settled: Arc::clone(&settled),
        });
        let call = AgentToolCall {
            id: "terminal-start".into(),
            name: "Terminal".into(),
            raw_arguments: r#"{"mode":"start"}"#.into(),
        };
        let arguments = json!({ "mode": "start" });

        let run = run_tool_with_cancel(None, &registry, &call, &arguments, &context, &cancel);
        tokio::pin!(run);
        tokio::select! {
            () = entered.notified() => {}
            result = &mut run => panic!("Terminal returned before cancellation: {result:?}"),
        }
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), run)
            .await
            .expect("Terminal cleanup should settle after cancellation");

        assert!(result.is_error);
        assert!(
            settled.load(Ordering::SeqCst),
            "runtime returned before Terminal cleanup settled"
        );
    }

    #[tokio::test]
    async fn approved_bash_emits_queued_then_started_only_after_grant() {
        let workspace = tempfile::tempdir().expect("workspace");
        let runtime = ShellRuntime::new(
            ShellLimits {
                max_active_commands: 1,
                ..ShellLimits::default()
            },
            PathBuf::from("missing-guardian"),
            workspace.path().join("runtime"),
        );
        let held = runtime
            .acquire(
                ShellAdmissionRequest {
                    owner: "hold".to_owned(),
                    class: ShellAdmissionClass::AgentForeground,
                },
                None,
            )
            .await;
        let config = AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Ask)
            .with_approval_handler(|request| ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::PermitOnce,
                feedback: None,
            })
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_shell_runtime(runtime);
        let model: Arc<dyn ModelClient> =
            Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
        let registry = Arc::new(ToolRegistry::with_builtin_tools());
        let calls = [AgentToolCall {
            id: "call-1".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"printf ready"}"#.into(),
        }];
        let cancel = CancellationToken::new();
        let supervisor = ProcessSupervisor::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut emitter = EventEmitter::new(tx, AgentContext::new());
        let run = execute_tool_calls(
            &config,
            model,
            registry,
            None,
            1,
            &calls,
            &mut emitter,
            &cancel,
            &supervisor,
        );
        tokio::pin!(run);
        let mut approval_seen = false;
        loop {
            tokio::select! {
                event = rx.recv() => {
                    let event = event.expect("event channel").expect("runtime event");
                    if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                        approval_seen = true;
                    }
                    if matches!(event, AgentEvent::ToolExecutionQueued { .. }) {
                        assert!(approval_seen, "Bash queued before approval completed");
                        break;
                    }
                    assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
                }
                result = &mut run => panic!(
                    "Bash returned before admission: ok={}",
                    result.is_ok()
                ),
            }
        }
        while let Ok(Ok(event)) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
        }
        drop(held);
        let results = run.await.expect("tool dispatch");
        let events = std::iter::from_fn(|| rx.try_recv().ok())
            .collect::<Result<Vec<_>, _>>()
            .expect("runtime events");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::ToolExecutionStarted { .. }))
        );
        let result = &results.results[0].1;
        let model_visible = format!("{} {:?}", result.content, result.details);
        assert!(!model_visible.contains("position"));
        assert!(!model_visible.contains("waiting_ms"));
    }

    #[tokio::test]
    async fn parallel_shell_batch_reaches_shared_admission_for_every_call() {
        let workspace = tempfile::tempdir().expect("workspace");
        let runtime = ShellRuntime::new(
            ShellLimits {
                max_active_commands: 1,
                ..ShellLimits::default()
            },
            PathBuf::from("missing-guardian"),
            workspace.path().join("runtime"),
        );
        let held = runtime
            .acquire(
                ShellAdmissionRequest {
                    owner: "hold".to_owned(),
                    class: ShellAdmissionClass::AgentForeground,
                },
                None,
            )
            .await;
        let config = AgentConfig::for_model(fake_model())
            .with_workspace_root(workspace.path())
            .expect("workspace root")
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Parallel)
            .with_shell_runtime(runtime);
        let model: Arc<dyn ModelClient> =
            Arc::new(neo_ai::providers::fake::FakeModelClient::new(Vec::new()));
        let registry = Arc::new(ToolRegistry::with_builtin_tools());
        let calls = [
            AgentToolCall {
                id: "call-1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"printf one"}"#.into(),
            },
            AgentToolCall {
                id: "call-2".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"printf two"}"#.into(),
            },
            AgentToolCall {
                id: "call-3".into(),
                name: "Terminal".into(),
                raw_arguments: r#"{"mode":"start","command":"printf three"}"#.into(),
            },
        ];
        let cancel = CancellationToken::new();
        let supervisor = ProcessSupervisor::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut emitter = EventEmitter::new(tx, AgentContext::new());
        let run = execute_tool_calls(
            &config,
            model,
            registry,
            None,
            1,
            &calls,
            &mut emitter,
            &cancel,
            &supervisor,
        );
        tokio::pin!(run);
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
        tokio::pin!(deadline);
        let mut queued_ids = Vec::new();
        while queued_ids.len() < calls.len() {
            tokio::select! {
                event = rx.recv() => {
                    let event = event.expect("event channel").expect("runtime event");
                    match event {
                        AgentEvent::ToolExecutionQueued { id, .. } => queued_ids.push(id),
                        AgentEvent::ToolExecutionStarted { id, .. } => {
                            panic!("shell call {id} started while capacity was held")
                        }
                        _ => {}
                    }
                }
                result = &mut run => panic!(
                    "shell batch returned before every call queued: ok={}",
                    result.is_ok()
                ),
                () = &mut deadline => panic!(
                    "only {} of {} shell calls reached admission",
                    queued_ids.len(),
                    calls.len()
                ),
            }
        }
        queued_ids.sort();
        assert_eq!(queued_ids, ["call-1", "call-2", "call-3"]);

        cancel.cancel();
        let results = run.await.expect("tool dispatch cancellation");
        assert_eq!(results.results.len(), calls.len());
        drop(held);
    }
}
