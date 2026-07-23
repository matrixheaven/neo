use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use neo_ai::{ModelClient, ModelSpec};

use super::config::{AgentConfig, AsyncApprovalHandler};
use super::context::AgentContext;
use super::error::AgentRuntimeError;
use super::events::EventEmitter;
use super::permission::PermissionTerminalDecision;
use super::tool_dispatch::{InstructionInterruption, ToolBatchOutcome, execute_workflow_tool_call};
use crate::multi_agent::{AgentLifecycleState, AgentRunMode};
use crate::skills::SkillStoreHandle;
use crate::tools::{ProcessSupervisor, ToolEventCallback, ToolRegistry};
use crate::workflow::{
    WorkflowChildRef, WorkflowInterruptionReason, WorkflowInvocationContext,
    WorkflowInvocationOutcome, WorkflowOutcomeStatus,
};
use crate::{AgentEvent, AgentTokenUsage, AgentToolCall, ToolResult};

/// Owned dependencies for one workflow-hosted tool invocation.
#[derive(Clone)]
pub struct WorkflowDispatchSnapshot {
    pub config: AgentConfig,
    pub model_client: Arc<dyn ModelClient>,
    pub registry: Arc<ToolRegistry>,
    pub skills: Option<SkillStoreHandle>,
    pub process_supervisor: ProcessSupervisor,
    pub context: AgentContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WorkflowDispatchSessionKey(PathBuf);

impl WorkflowDispatchSessionKey {
    fn from_config(config: &AgentConfig) -> Self {
        Self(config.session_directory.clone().unwrap_or_default())
    }

    fn from_directory(session_directory: Option<&Path>) -> Self {
        Self(session_directory.map_or_else(PathBuf::new, Path::to_path_buf))
    }
}

#[derive(Default)]
struct WorkflowDispatchResolverState {
    snapshots: HashMap<WorkflowDispatchSessionKey, WorkflowDispatchSnapshot>,
    latest_session: Option<WorkflowDispatchSessionKey>,
    next_event_lease_id: u64,
    event_routes: HashMap<WorkflowDispatchSessionKey, WorkflowDispatchEventRoute>,
    idle_event_routes: HashMap<WorkflowDispatchSessionKey, WorkflowDispatchIdleEventRoute>,
    approval_routes: HashMap<WorkflowDispatchSessionKey, WorkflowDispatchApprovalRoute>,
}

struct WorkflowDispatchIdleEventRoute {
    lease_id: u64,
    handler: ToolEventCallback,
}

struct WorkflowDispatchEventRoute {
    lease_id: u64,
    turn: u32,
    handler: Option<ToolEventCallback>,
    pending_idle_events: Vec<AgentEvent>,
}

struct WorkflowDispatchApprovalRoute {
    lease_id: u64,
    handler: AsyncApprovalHandler,
}

/// Session-shared live resolver. Locks protect snapshot replacement only and
/// are never held while permission, provider, shell, or tool futures await.
#[derive(Clone, Default)]
pub struct WorkflowDispatchResolver {
    state: Arc<RwLock<WorkflowDispatchResolverState>>,
}

/// Active natural-turn event route. Dropping the lease removes the route only
/// when it is still the current generation, so an older turn cannot clear a
/// newer turn's route.
#[must_use = "dropping the lease releases the workflow event route"]
pub struct WorkflowDispatchEventLease {
    resolver: WorkflowDispatchResolver,
    session: WorkflowDispatchSessionKey,
    lease_id: u64,
}

/// Receiver-side guard that keeps idle persistence paused until all active
/// turn events have been consumed.
#[must_use = "dropping the drain lease releases queued workflow events to the idle route"]
pub struct WorkflowDispatchEventDrainLease {
    resolver: WorkflowDispatchResolver,
    session: WorkflowDispatchSessionKey,
    lease_id: u64,
}

#[must_use = "dropping the lease removes this session's idle workflow event route"]
pub struct WorkflowDispatchIdleEventLease {
    resolver: WorkflowDispatchResolver,
    session: WorkflowDispatchSessionKey,
    lease_id: u64,
}

#[must_use = "dropping the lease removes this session's workflow approval route"]
pub struct WorkflowDispatchApprovalLease {
    resolver: WorkflowDispatchResolver,
    session: WorkflowDispatchSessionKey,
    lease_id: u64,
}

impl Drop for WorkflowDispatchEventLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.resolver.state.write() {
            if let Some(route) = state
                .event_routes
                .get_mut(&self.session)
                .filter(|route| route.lease_id == self.lease_id)
            {
                route.handler = None;
            }
        }
    }
}

impl Drop for WorkflowDispatchEventDrainLease {
    fn drop(&mut self) {
        let release = self.resolver.state.write().ok().and_then(|mut state| {
            let is_current = state
                .event_routes
                .get(&self.session)
                .is_some_and(|route| route.lease_id == self.lease_id);
            if !is_current {
                return None;
            }
            let route = state.event_routes.remove(&self.session)?;
            let idle = state
                .idle_event_routes
                .get(&self.session)
                .map(|route| Arc::clone(&route.handler));
            Some((idle, route.pending_idle_events))
        });
        if let Some((Some(handler), events)) = release {
            for event in events {
                handler(event);
            }
        }
    }
}

impl Drop for WorkflowDispatchIdleEventLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.resolver.state.write() {
            let is_current = state
                .idle_event_routes
                .get(&self.session)
                .is_some_and(|route| route.lease_id == self.lease_id);
            if is_current {
                state.idle_event_routes.remove(&self.session);
            }
        }
    }
}

impl Drop for WorkflowDispatchApprovalLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.resolver.state.write() {
            let is_current = state
                .approval_routes
                .get(&self.session)
                .is_some_and(|route| route.lease_id == self.lease_id);
            if is_current {
                state.approval_routes.remove(&self.session);
            }
        }
    }
}

impl std::fmt::Debug for WorkflowDispatchResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowDispatchResolver")
            .field(
                "bound",
                &self
                    .state
                    .read()
                    .map(|state| !state.snapshots.is_empty())
                    .unwrap_or(false),
            )
            .finish()
    }
}

impl WorkflowDispatchResolver {
    /// Whether two handles resolve through the same session owner.
    #[must_use]
    pub fn shares_state_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    /// Replace the dependencies used by subsequent workflow invocations.
    ///
    /// # Errors
    /// Returns an error when the shared resolver lock is poisoned.
    pub fn replace(&self, mut snapshot: WorkflowDispatchSnapshot) -> Result<(), String> {
        snapshot.config.workflow_dispatch_resolver = Self::default();
        let session = WorkflowDispatchSessionKey::from_config(&snapshot.config);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        state.latest_session = Some(session.clone());
        state.snapshots.insert(session, snapshot);
        Ok(())
    }

    /// Refresh live dependencies without changing the active natural-turn
    /// event lease.
    ///
    /// # Errors
    /// Returns an error when the shared resolver lock is poisoned.
    pub fn refresh(&self, mut snapshot: WorkflowDispatchSnapshot) -> Result<(), String> {
        snapshot.config.workflow_dispatch_resolver = Self::default();
        let session = WorkflowDispatchSessionKey::from_config(&snapshot.config);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        state.latest_session = Some(session.clone());
        state.snapshots.insert(session, snapshot);
        Ok(())
    }

    /// Resolve an owned snapshot for one invocation.
    ///
    /// # Errors
    /// Returns an error when the resolver is unbound or its lock is poisoned.
    pub fn resolve(&self) -> Result<WorkflowDispatchSnapshot, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let session = state
            .latest_session
            .as_ref()
            .ok_or_else(|| "workflow dispatch resolver is not bound".to_owned())?;
        state
            .snapshots
            .get(session)
            .cloned()
            .ok_or_else(|| "workflow dispatch resolver is not bound".to_owned())
    }

    /// Build a canonical dispatch handle for one workflow session. The handle
    /// retains this resolver, so every host call still resolves fresh live
    /// model/provider/approval dependencies.
    pub fn handle_for_session(
        &self,
        session_directory: &Path,
    ) -> Result<WorkflowDispatchHandle, String> {
        let session = WorkflowDispatchSessionKey::from_directory(Some(session_directory));
        let mut snapshot = self.resolve_for(&session)?;
        snapshot.config.workflow_dispatch_resolver = self.clone();
        Ok(WorkflowDispatchHandle {
            config: snapshot.config,
            model_client: snapshot.model_client,
            registry: snapshot.registry,
            process_supervisor: snapshot.process_supervisor,
            context: snapshot.context,
        })
    }

    fn resolve_for(
        &self,
        session: &WorkflowDispatchSessionKey,
    ) -> Result<WorkflowDispatchSnapshot, String> {
        let state = self
            .state
            .read()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let mut snapshot =
            state.snapshots.get(session).cloned().ok_or_else(|| {
                "workflow dispatch resolver is not bound for this session".to_owned()
            })?;
        if let Some(route) = state.approval_routes.get(session) {
            snapshot.config.approval_handler = None;
            snapshot.config.async_approval_handler = Some(Arc::clone(&route.handler));
        }
        Ok(snapshot)
    }

    /// Lease the current natural turn's event route. The route is released
    /// when the returned guard is dropped.
    ///
    /// # Errors
    /// Returns an error when the resolver lock is poisoned or the lease
    /// generation is exhausted.
    pub fn lease_event_route(
        &self,
        session_directory: Option<&Path>,
        turn: u32,
        handler: ToolEventCallback,
    ) -> Result<(WorkflowDispatchEventLease, WorkflowDispatchEventDrainLease), String> {
        let session = WorkflowDispatchSessionKey::from_directory(session_directory);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let lease_id = state
            .next_event_lease_id
            .checked_add(1)
            .ok_or_else(|| "workflow dispatch event lease generation exhausted".to_owned())?;
        state.next_event_lease_id = lease_id;
        state.event_routes.insert(
            session.clone(),
            WorkflowDispatchEventRoute {
                lease_id,
                turn,
                handler: Some(handler),
                pending_idle_events: Vec::new(),
            },
        );
        Ok((
            WorkflowDispatchEventLease {
                resolver: self.clone(),
                session: session.clone(),
                lease_id,
            },
            WorkflowDispatchEventDrainLease {
                resolver: self.clone(),
                session,
                lease_id,
            },
        ))
    }

    /// Register the durable idle event route for one session.
    ///
    /// # Errors
    /// Returns an error when the resolver lock is poisoned.
    pub fn lease_idle_event_route(
        &self,
        session_directory: Option<&Path>,
        handler: ToolEventCallback,
    ) -> Result<WorkflowDispatchIdleEventLease, String> {
        let session = WorkflowDispatchSessionKey::from_directory(session_directory);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let lease_id = state
            .next_event_lease_id
            .checked_add(1)
            .ok_or_else(|| "workflow dispatch event lease generation exhausted".to_owned())?;
        state.next_event_lease_id = lease_id;
        state.idle_event_routes.insert(
            session.clone(),
            WorkflowDispatchIdleEventRoute { lease_id, handler },
        );
        Ok(WorkflowDispatchIdleEventLease {
            resolver: self.clone(),
            session,
            lease_id,
        })
    }

    /// Register the live approval transport for one workflow session.
    ///
    /// # Errors
    /// Returns an error when the resolver lock is poisoned or the lease
    /// generation is exhausted.
    pub fn lease_approval_route(
        &self,
        session_directory: Option<&Path>,
        handler: AsyncApprovalHandler,
    ) -> Result<WorkflowDispatchApprovalLease, String> {
        let session = WorkflowDispatchSessionKey::from_directory(session_directory);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let lease_id = state
            .next_event_lease_id
            .checked_add(1)
            .ok_or_else(|| "workflow dispatch route lease generation exhausted".to_owned())?;
        state.next_event_lease_id = lease_id;
        state.approval_routes.insert(
            session.clone(),
            WorkflowDispatchApprovalRoute { lease_id, handler },
        );
        Ok(WorkflowDispatchApprovalLease {
            resolver: self.clone(),
            session,
            lease_id,
        })
    }

    pub(crate) fn update_event_route_turn(
        &self,
        session_directory: Option<&Path>,
        turn: u32,
    ) -> Result<(), String> {
        let session = WorkflowDispatchSessionKey::from_directory(session_directory);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        if let Some(route) = state.event_routes.get_mut(&session) {
            route.turn = turn;
        }
        Ok(())
    }

    fn event_route(
        &self,
        session: &WorkflowDispatchSessionKey,
        idle_turn: u32,
    ) -> Result<Option<(u32, ToolEventCallback)>, String> {
        self.state
            .read()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())
            .map(|state| {
                let turn = state
                    .event_routes
                    .get(session)
                    .map(|route| route.turn)
                    .or_else(|| {
                        state
                            .idle_event_routes
                            .contains_key(session)
                            .then_some(idle_turn)
                    })?;
                let resolver = self.clone();
                let session = session.clone();
                Some((
                    turn,
                    Arc::new(move |event| resolver.dispatch_event(&session, event))
                        as ToolEventCallback,
                ))
            })
    }

    fn dispatch_event(&self, session: &WorkflowDispatchSessionKey, event: AgentEvent) {
        let handler = {
            let Ok(mut state) = self.state.write() else {
                return;
            };
            if let Some(route) = state.event_routes.get_mut(session) {
                if let Some(handler) = &route.handler {
                    Some(Arc::clone(handler))
                } else {
                    route.pending_idle_events.push(event);
                    return;
                }
            } else {
                state
                    .idle_event_routes
                    .get(session)
                    .map(|route| Arc::clone(&route.handler))
            }
        };
        if let Some(handler) = handler {
            handler(event);
        }
    }

    /// Replace only the model selection and client used by later workflow
    /// invocations, retaining the session's registry and context.
    /// An unbound resolver needs no update because the next turn will bind the
    /// already-selected model.
    ///
    /// # Errors
    /// Returns an error when the shared resolver lock is poisoned.
    pub fn update_model_for_session(
        &self,
        session_directory: Option<&Path>,
        model: ModelSpec,
        model_client: Arc<dyn ModelClient>,
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let session = WorkflowDispatchSessionKey::from_directory(session_directory);
        if let Some(snapshot) = state.snapshots.get_mut(&session) {
            snapshot.config.model = model;
            snapshot.model_client = model_client;
        }
        Ok(())
    }

    fn bind_once(&self, snapshot: WorkflowDispatchSnapshot) -> Result<(), String> {
        let session = WorkflowDispatchSessionKey::from_config(&snapshot.config);
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        if !state.snapshots.contains_key(&session) {
            let mut snapshot = snapshot;
            snapshot.config.workflow_dispatch_resolver = Self::default();
            state.snapshots.insert(session.clone(), snapshot);
        }
        state.latest_session = Some(session);
        Ok(())
    }

    fn commit_context(
        &self,
        session: &WorkflowDispatchSessionKey,
        context: &AgentContext,
        events: &[AgentEvent],
    ) -> Result<(), String> {
        let mut state = self
            .state
            .write()
            .map_err(|_| "workflow dispatch resolver lock poisoned".to_owned())?;
        let snapshot = state
            .snapshots
            .get_mut(session)
            .ok_or_else(|| "workflow dispatch resolver is not bound for this session".to_owned())?;
        for event in events {
            if let AgentEvent::InstructionEpoch { epoch } = event
                && epoch.generation < snapshot.context.instruction_state().visible_generation
            {
                continue;
            }
            EventEmitter::apply_to_context(&mut snapshot.context, event);
        }
        if context.instruction_state().visible_generation
            >= snapshot.context.instruction_state().visible_generation
        {
            snapshot
                .context
                .instruction_state_mut()
                .last_epoch_fingerprint =
                context.instruction_state().last_epoch_fingerprint.clone();
        }
        Ok(())
    }
}

/// Cloneable bridge from workflow effects to the canonical runtime dispatcher.
#[derive(Clone)]
pub struct WorkflowDispatchHandle {
    pub config: AgentConfig,
    pub model_client: Arc<dyn ModelClient>,
    pub registry: Arc<ToolRegistry>,
    pub process_supervisor: ProcessSupervisor,
    pub context: AgentContext,
}

impl WorkflowDispatchHandle {
    /// Return the live resolver shared by all clones of this handle.
    ///
    /// # Errors
    /// Returns an error when the resolver lock is poisoned.
    pub fn resolver(&self) -> Result<WorkflowDispatchResolver, String> {
        let resolver = self.config.workflow_dispatch_resolver.clone();
        resolver.bind_once(WorkflowDispatchSnapshot {
            config: self.config.clone(),
            model_client: Arc::clone(&self.model_client),
            registry: Arc::clone(&self.registry),
            skills: None,
            process_supervisor: self.process_supervisor.clone(),
            context: self.context.clone(),
        })?;
        Ok(resolver)
    }

    /// Dispatch one workflow effect through the same canonical pipeline used by
    /// ordinary model tool batches.
    pub async fn run_one(
        &self,
        invocation: WorkflowInvocationContext,
        tool_name: &str,
        tool_input: serde_json::Value,
    ) -> WorkflowInvocationOutcome {
        let resolver = match self.resolver() {
            Ok(resolver) => resolver,
            Err(error) => return failed_outcome(error),
        };
        let session = WorkflowDispatchSessionKey::from_config(&self.config);
        let snapshot = match resolver.resolve_for(&session) {
            Ok(snapshot) => snapshot,
            Err(error) => return failed_outcome(error),
        };
        let (turn, event_handler) = match resolver.event_route(&session, snapshot.context.turns) {
            Ok(Some((turn, handler))) => (turn, Some(handler)),
            Ok(None) => (snapshot.context.turns, None),
            Err(error) => return failed_outcome(error),
        };
        let raw_arguments = match serde_json::to_string(&tool_input) {
            Ok(raw_arguments) => raw_arguments,
            Err(error) => return failed_outcome(error.to_string()),
        };
        let call = AgentToolCall {
            id: Arc::from(invocation.invocation_id),
            name: Arc::from(tool_name.to_owned()),
            raw_arguments: Arc::from(raw_arguments),
        };
        let (batch, context, events) = execute_workflow_tool_call(
            &snapshot.config,
            snapshot.model_client,
            snapshot.registry,
            snapshot.skills.as_ref(),
            &call,
            snapshot.context,
            &invocation.cancel_token,
            &snapshot.process_supervisor,
            turn,
            event_handler,
        )
        .await;
        if let Err(error) = resolver.commit_context(&session, &context, &events) {
            return failed_outcome(error);
        }
        match batch {
            Ok(batch) => batch_to_outcome(batch),
            Err(AgentRuntimeError::Cancelled) => cancelled_outcome(),
            Err(error) => failed_outcome(error.to_string()),
        }
    }
}

fn batch_to_outcome(mut batch: ToolBatchOutcome) -> WorkflowInvocationOutcome {
    let Some((call, result)) = batch.results.pop() else {
        return failed_outcome("canonical workflow dispatch returned no result".to_owned());
    };
    let Some(permission_decision) = batch.permission_decisions.pop() else {
        return failed_outcome(
            "canonical workflow dispatch returned no typed permission outcome".to_owned(),
        );
    };
    if let Some(interruption) = batch.instruction_interruption {
        return interrupted_outcome(result, interruption);
    }
    tool_result_to_outcome(
        call.name.as_ref(),
        result,
        batch.executed_any,
        permission_decision,
    )
}

fn interrupted_outcome(
    result: ToolResult,
    interruption: InstructionInterruption,
) -> WorkflowInvocationOutcome {
    let (decision, generation) = match interruption {
        InstructionInterruption::Deferred { generation } => ("deferred", generation),
        InstructionInterruption::Blocked { generation } => ("blocked", generation),
    };
    let mut details = result
        .details
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    if let Some(details) = details.as_object_mut() {
        details.insert(
            "reason".to_owned(),
            serde_json::Value::String("instruction_replan_required".to_owned()),
        );
        details.insert(
            "instruction_decision".to_owned(),
            serde_json::Value::String(decision.to_owned()),
        );
        details.insert("instruction_generation".to_owned(), generation.into());
        details.insert(
            "side_effect_occurred".to_owned(),
            serde_json::Value::Bool(false),
        );
    }
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Interrupted,
        summary: result.content,
        interruption: Some(WorkflowInterruptionReason::InstructionReplanRequired),
        details,
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

fn tool_result_to_outcome(
    tool_name: &str,
    result: ToolResult,
    side_effect_occurred: bool,
    permission_decision: Option<PermissionTerminalDecision>,
) -> WorkflowInvocationOutcome {
    let mut details = result
        .details
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let is_cancelled = details.get("kind").and_then(serde_json::Value::as_str) == Some("cancelled");
    let is_resource_limited =
        details.get("outcome").and_then(serde_json::Value::as_str) == Some("resource_limited");
    let fallback_status = match permission_decision {
        Some(PermissionTerminalDecision::Cancelled) => WorkflowOutcomeStatus::Cancelled,
        Some(PermissionTerminalDecision::Denied | PermissionTerminalDecision::Required) => {
            WorkflowOutcomeStatus::Denied
        }
        None if is_cancelled => WorkflowOutcomeStatus::Cancelled,
        None if is_resource_limited => WorkflowOutcomeStatus::ResourceLimited,
        None if result.is_error => WorkflowOutcomeStatus::Failed,
        None => WorkflowOutcomeStatus::Completed,
    };
    if let Some(object) = details.as_object_mut() {
        object
            .entry("side_effect_occurred")
            .or_insert(serde_json::Value::Bool(side_effect_occurred));
    }
    let canonical_child = match canonical_child_outcome(tool_name, result.is_error, &details) {
        Ok(correlation) => correlation,
        Err(error) => {
            if let Some(object) = details.as_object_mut() {
                object.insert(
                    "workflow_outcome_error".to_owned(),
                    serde_json::Value::String(error.clone()),
                );
            }
            return WorkflowInvocationOutcome {
                ok: false,
                status: WorkflowOutcomeStatus::Failed,
                summary: error,
                interruption: None,
                details,
                actual_usage: None,
                child_refs: Vec::new(),
            };
        }
    };
    let (status, actual_usage, child_refs) = canonical_child.map_or_else(
        || (fallback_status, None, Vec::new()),
        |child| {
            if child.status == WorkflowOutcomeStatus::Interrupted
                && let Some(object) = details.as_object_mut()
            {
                object
                    .entry("reason")
                    .or_insert_with(|| serde_json::Value::String("child_interrupted".to_owned()));
            }
            (child.status, child.actual_usage, child.child_refs)
        },
    );
    WorkflowInvocationOutcome {
        ok: status == WorkflowOutcomeStatus::Completed,
        status,
        summary: result.content,
        interruption: None,
        details,
        actual_usage,
        child_refs,
    }
}

#[derive(serde::Deserialize)]
struct DelegateOutcomeDetails {
    agent_id: String,
    status: AgentLifecycleState,
    mode: AgentRunMode,
    task_id: Option<String>,
    actual_usage: Option<AgentTokenUsage>,
}

#[derive(serde::Deserialize)]
struct SwarmOutcomeItem {
    agent_id: String,
    status: AgentLifecycleState,
}

#[derive(serde::Deserialize)]
struct SwarmOutcomeDetails {
    swarm_id: String,
    status: AgentLifecycleState,
    mode: AgentRunMode,
    items: Vec<SwarmOutcomeItem>,
    task_id: Option<String>,
    actual_usage: Option<AgentTokenUsage>,
}

struct CanonicalChildOutcome {
    status: WorkflowOutcomeStatus,
    actual_usage: Option<AgentTokenUsage>,
    child_refs: Vec<WorkflowChildRef>,
}

fn canonical_child_outcome(
    tool_name: &str,
    result_is_error: bool,
    details: &serde_json::Value,
) -> Result<Option<CanonicalChildOutcome>, String> {
    let kind = details.get("kind").and_then(serde_json::Value::as_str);
    match tool_name {
        "Delegate" => {
            if kind != Some("delegate") {
                return Err(format!(
                    "invalid canonical Delegate outcome details: expected kind delegate, got {}",
                    kind.unwrap_or("missing")
                ));
            }
            let decoded: DelegateOutcomeDetails = serde_json::from_value(details.clone())
                .map_err(|error| format!("invalid canonical Delegate outcome details: {error}"))?;
            if decoded.agent_id.is_empty() {
                return Err("invalid canonical Delegate outcome details: empty agent_id".to_owned());
            }
            let mut child_refs = vec![WorkflowChildRef {
                kind: "delegate".to_owned(),
                id: decoded.agent_id,
            }];
            if let Some(task_id) = decoded.task_id {
                if task_id.is_empty() {
                    return Err(
                        "invalid canonical Delegate outcome details: empty task_id".to_owned()
                    );
                }
                child_refs.push(WorkflowChildRef {
                    kind: "task".to_owned(),
                    id: task_id,
                });
            }
            let status = canonical_child_status("Delegate", decoded.status, decoded.mode)?;
            if result_is_error && status == WorkflowOutcomeStatus::Completed {
                return Err(
                    "invalid canonical Delegate outcome details: error result cannot be completed"
                        .to_owned(),
                );
            }
            Ok(Some(CanonicalChildOutcome {
                status,
                actual_usage: decoded.actual_usage,
                child_refs,
            }))
        }
        "DelegateSwarm" => {
            if kind != Some("delegate_swarm") {
                return Err(format!(
                    "invalid canonical DelegateSwarm outcome details: expected kind delegate_swarm, got {}",
                    kind.unwrap_or("missing")
                ));
            }
            let decoded: SwarmOutcomeDetails =
                serde_json::from_value(details.clone()).map_err(|error| {
                    format!("invalid canonical DelegateSwarm outcome details: {error}")
                })?;
            if decoded.swarm_id.is_empty() {
                return Err(
                    "invalid canonical DelegateSwarm outcome details: empty swarm_id".to_owned(),
                );
            }
            if decoded.items.iter().any(|item| !item.status.is_terminal()) {
                return Err(
                    "invalid canonical DelegateSwarm outcome details: nonterminal child".to_owned(),
                );
            }
            if decoded.items.iter().any(|item| item.agent_id.is_empty()) {
                return Err(
                    "invalid canonical DelegateSwarm outcome details: empty child agent_id"
                        .to_owned(),
                );
            }
            let mut child_refs = Vec::with_capacity(decoded.items.len() + 2);
            child_refs.push(WorkflowChildRef {
                kind: "delegate_swarm".to_owned(),
                id: decoded.swarm_id,
            });
            child_refs.extend(decoded.items.into_iter().map(|item| WorkflowChildRef {
                kind: "delegate".to_owned(),
                id: item.agent_id,
            }));
            if let Some(task_id) = decoded.task_id {
                if task_id.is_empty() {
                    return Err(
                        "invalid canonical DelegateSwarm outcome details: empty task_id".to_owned(),
                    );
                }
                child_refs.push(WorkflowChildRef {
                    kind: "task".to_owned(),
                    id: task_id,
                });
            }
            let status = canonical_child_status("DelegateSwarm", decoded.status, decoded.mode)?;
            if result_is_error && status == WorkflowOutcomeStatus::Completed {
                return Err(
                    "invalid canonical DelegateSwarm outcome details: error result cannot be completed"
                        .to_owned(),
                );
            }
            Ok(Some(CanonicalChildOutcome {
                status,
                actual_usage: decoded.actual_usage,
                child_refs,
            }))
        }
        _ if matches!(kind, Some("delegate" | "delegate_swarm")) => Err(format!(
            "invalid canonical child outcome details: tool {tool_name} cannot report kind {}",
            kind.expect("matched child kind")
        )),
        _ => Ok(None),
    }
}

fn canonical_child_status(
    tool: &str,
    status: AgentLifecycleState,
    mode: AgentRunMode,
) -> Result<WorkflowOutcomeStatus, String> {
    if mode == AgentRunMode::Background {
        return Err(format!(
            "invalid canonical {tool} outcome details: background result is nonterminal"
        ));
    }
    match status {
        AgentLifecycleState::Completed => Ok(WorkflowOutcomeStatus::Completed),
        AgentLifecycleState::Failed | AgentLifecycleState::TimedOut => {
            Ok(WorkflowOutcomeStatus::Failed)
        }
        AgentLifecycleState::Cancelled => Ok(WorkflowOutcomeStatus::Cancelled),
        AgentLifecycleState::Interrupted => Ok(WorkflowOutcomeStatus::Interrupted),
        AgentLifecycleState::Queued | AgentLifecycleState::Running => Err(format!(
            "invalid canonical {tool} outcome details: nonterminal status {}",
            status.as_str()
        )),
    }
}

fn failed_outcome(summary: String) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Failed,
        summary,
        interruption: None,
        details: serde_json::json!({"side_effect_occurred": false}),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}

fn cancelled_outcome() -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: false,
        status: WorkflowOutcomeStatus::Cancelled,
        summary: "tool execution cancelled".to_owned(),
        interruption: None,
        details: serde_json::json!({
            "kind": "cancelled",
            "side_effect_occurred": false,
        }),
        actual_usage: None,
        child_refs: Vec::new(),
    }
}
