//! Stateless workflow launch normalization and sequencing.
//!
//! All adapters (dynamic `RunWorkflow`, named slash, headless CLI) build one
//! immutable [`WorkflowLaunchIntent`] and call [`WorkflowLaunchCoordinator`].
//! The coordinator never writes `run.json`/journal, never owns capability or
//! admission state, and never registers tasks itself beyond calling the
//! existing owners in the required order.

use std::path::Path;

use sha2::{Digest, Sha256};

use super::capability::WorkflowCapability;
use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::canonical_input_hash;
use super::runtime::{WorkflowHandle, WorkflowLaunchRequest, WorkflowRuntime};
use super::schema::CompiledSchema;
use super::source_sha256_hex;
use super::state::{
    WorkflowActor, WorkflowId, WorkflowLineageMetadata, WorkflowPhase, WorkflowRevision,
};
use crate::PermissionMode;
use crate::tools::BackgroundTaskManager;

/// ASCII framing magic for the exact launch-intent digest (includes trailing NUL).
pub const LAUNCH_INTENT_DIGEST_PREFIX: &[u8] = b"neo-workflow-launch-intent-v1\0";

/// Immutable launch intent produced by every adapter before durable creation.
///
/// Binding fields (session, workspace, nonce, source, revision, args/schema/
/// lineage hashes, actor, parent lineage) participate in [`Self::digest`].
/// Payload fields drive [`WorkflowLaunchRequest`] construction only.
#[derive(Clone)]
pub struct WorkflowLaunchIntent {
    pub session_identity: String,
    pub workspace_identity: String,
    pub launch_nonce: String,
    pub launch_source: String,
    pub definition_revision: WorkflowRevision,
    pub source_sha256: String,
    pub args: serde_json::Value,
    pub args_sha256: String,
    /// SHA-256 of canonical schema material (empty when no schema binding).
    pub schema_sha256: String,
    /// SHA-256 of canonical parent lineage material (empty when none).
    pub lineage_digest: String,
    pub actor: WorkflowActor,
    pub permission_mode: PermissionMode,
    pub parent_lineage: Option<WorkflowLineageMetadata>,
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    pub parent_run_id: Option<WorkflowId>,
    /// Optional precompiled input schema for argument validation.
    pub compiled_input_schema: Option<CompiledSchema>,
    /// Final output schema JSON pinned onto the run for production validation.
    pub output_schema: Option<serde_json::Value>,
}

/// Session/workspace binding fields for [`WorkflowLaunchIntent::from_parts`].
#[derive(Clone)]
pub struct WorkflowLaunchBinding {
    pub session_identity: String,
    pub workspace_identity: String,
    pub launch_nonce: String,
    pub actor: WorkflowActor,
    pub permission_mode: PermissionMode,
    pub parent_lineage: Option<WorkflowLineageMetadata>,
    pub compiled_input_schema: Option<CompiledSchema>,
    /// SHA-256 of canonical schema material (empty when no schema binding).
    pub schema_sha256: String,
}

impl WorkflowLaunchIntent {
    /// Build an intent from a resolved launch request plus identity binding fields.
    #[must_use]
    pub fn from_parts(request: WorkflowLaunchRequest, binding: WorkflowLaunchBinding) -> Self {
        let source_sha256 = source_sha256_hex(request.script.as_bytes());
        let args_sha256 = canonical_input_hash(&request.args);
        let lineage_digest = binding
            .parent_lineage
            .as_ref()
            .map_or_else(String::new, lineage_digest_hex);
        let definition_revision = WorkflowRevision::from_bytes(request.script.as_bytes());
        Self {
            session_identity: binding.session_identity,
            workspace_identity: binding.workspace_identity,
            launch_nonce: binding.launch_nonce,
            launch_source: request.launch_source.clone(),
            definition_revision,
            source_sha256,
            args: request.args.clone(),
            args_sha256,
            schema_sha256: binding.schema_sha256,
            lineage_digest,
            actor: binding.actor,
            permission_mode: binding.permission_mode,
            parent_lineage: binding.parent_lineage,
            name: request.name,
            description: request.description,
            phases: request.phases,
            script: request.script,
            parent_run_id: request.parent_run_id,
            compiled_input_schema: binding.compiled_input_schema,
            output_schema: request.output_schema,
        }
    }

    /// Exact SHA-256 digest over length-prefixed binding fields.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut frame = Vec::new();
        frame.extend_from_slice(LAUNCH_INTENT_DIGEST_PREFIX);
        append_len_prefixed(&mut frame, self.session_identity.as_bytes());
        append_len_prefixed(&mut frame, self.workspace_identity.as_bytes());
        append_len_prefixed(&mut frame, self.launch_nonce.as_bytes());
        append_len_prefixed(&mut frame, self.source_sha256.as_bytes());
        append_len_prefixed(&mut frame, self.definition_revision.as_str().as_bytes());
        append_len_prefixed(&mut frame, self.args_sha256.as_bytes());
        append_len_prefixed(&mut frame, self.schema_sha256.as_bytes());
        append_len_prefixed(&mut frame, self.lineage_digest.as_bytes());
        append_len_prefixed(&mut frame, self.actor.as_str().as_bytes());
        let parent = self.parent_run_id.as_ref().map_or("", WorkflowId::as_str);
        append_len_prefixed(&mut frame, parent.as_bytes());
        if let Some(lineage) = &self.parent_lineage {
            let encoded = serde_json::to_vec(lineage).unwrap_or_default();
            append_len_prefixed(&mut frame, &encoded);
        } else {
            append_len_prefixed(&mut frame, b"");
        }
        format!("{:x}", Sha256::digest(&frame))
    }

    /// Convert to the durable runtime create request.
    #[must_use]
    pub fn to_launch_request(&self) -> WorkflowLaunchRequest {
        WorkflowLaunchRequest {
            name: self.name.clone(),
            description: self.description.clone(),
            phases: self.phases.clone(),
            script: self.script.clone(),
            args: self.args.clone(),
            launch_source: self.launch_source.clone(),
            parent_run_id: self.parent_run_id.clone(),
            output_schema: self.output_schema.clone(),
        }
    }
}

fn append_len_prefixed(frame: &mut Vec<u8>, bytes: &[u8]) {
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(bytes);
}

fn lineage_digest_hex(lineage: &WorkflowLineageMetadata) -> String {
    let encoded = serde_json::to_vec(lineage).unwrap_or_default();
    format!("{:x}", Sha256::digest(encoded))
}

impl WorkflowActor {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Model => "model",
            Self::Runtime => "runtime",
        }
    }
}

/// How launch authorization is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchAuthorizationMode {
    /// Session-scoped capability: bind exact intent digest then consume after
    /// durable create + task registration.
    SessionCapability,
    /// Explicit human/headless launch: no session capability required.
    Headless,
}

/// Successful launch outcome after registration and worker admission attempt.
#[derive(Clone)]
pub struct WorkflowLaunchOutcome {
    pub handle: WorkflowHandle,
    pub task_id: String,
}

impl std::fmt::Debug for WorkflowLaunchOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkflowLaunchOutcome")
            .field("task_id", &self.task_id)
            .field("run_id", &self.handle.run_id)
            .finish_non_exhaustive()
    }
}

/// Owners the coordinator sequences. The coordinator holds no durable state.
pub struct WorkflowLaunchHosts<'a> {
    pub runtime: &'a WorkflowRuntime,
    pub capability: &'a WorkflowCapability,
    pub background_tasks: &'a BackgroundTaskManager,
    pub session_dir: &'a Path,
}

/// Stateless launch normalization / orchestration.
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkflowLaunchCoordinator;

impl WorkflowLaunchCoordinator {
    /// Preflight, authorize, durable-create, consume auth, register, admit/start.
    ///
    /// Order (design §12–§14):
    /// 1. pure preflight (limits, Lua compile, input schema) — never consumes auth
    /// 2. exact capability bind (session mode only)
    /// 3. `WorkflowRuntime::create_run`
    /// 4. task registration
    /// 5. capability consume (session mode only)
    /// 6. emit started + `start_worker` (admission may leave the run queued)
    pub async fn launch(
        self,
        intent: &WorkflowLaunchIntent,
        hosts: WorkflowLaunchHosts<'_>,
        auth_mode: LaunchAuthorizationMode,
    ) -> Result<WorkflowLaunchOutcome, WorkflowError> {
        self.preflight(intent, hosts.runtime)?;

        let intent_digest = intent.digest();
        if auth_mode == LaunchAuthorizationMode::SessionCapability {
            self.bind_authorization(intent, &intent_digest, hosts.capability)?;
        }

        let request = intent.to_launch_request();
        let handle = match hosts.runtime.create_run(hosts.session_dir, request).await {
            Ok(handle) => handle,
            Err(error) => {
                // Bound authorization remains reusable for the same intent.
                return Err(error);
            }
        };

        let task_id = handle.run_id.0.clone();
        if let Err(register_error) = hosts
            .background_tasks
            .start_workflow(task_id.clone(), intent.description.clone(), handle.clone())
            .await
        {
            let _ = hosts.runtime.rollback_created_run(&handle.run_id).await;
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchFailedAfterCreate,
                format!("workflow registration failed: {register_error}"),
            ));
        }

        if auth_mode == LaunchAuthorizationMode::SessionCapability
            && !hosts.capability.consume_bound(&intent_digest)
        {
            hosts.background_tasks.remove_workflow(&task_id).await;
            match hosts.runtime.rollback_created_run(&handle.run_id).await {
                Ok(()) => {
                    return Err(WorkflowError::coded(
                        WorkflowErrorCode::LaunchFailedAfterCreate,
                        "launch authorization changed during launch",
                    ));
                }
                Err(rollback_error) => {
                    let _ = hosts
                        .runtime
                        .fail_worker_start(&handle.run_id, &rollback_error)
                        .await;
                    return Err(WorkflowError::coded(
                        WorkflowErrorCode::LaunchFailedAfterCreate,
                        format!(
                            "launch authorization changed during launch; rollback failed: {rollback_error}"
                        ),
                    ));
                }
            }
        }

        hosts
            .runtime
            .emit_started(&handle.run_id)
            .await
            .map_err(|error| {
                WorkflowError::coded(
                    WorkflowErrorCode::LaunchFailedAfterCreate,
                    format!("workflow start event failed: {error}"),
                )
            })?;

        if let Err(error) = hosts.runtime.start_worker(&handle.run_id).await {
            let _ = hosts
                .runtime
                .fail_worker_start(&handle.run_id, &error)
                .await;
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchFailedAfterCreate,
                format!("worker startup failed: {error}"),
            ));
        }

        Ok(WorkflowLaunchOutcome { handle, task_id })
    }

    /// Pure validation before any capability mutation or durable create.
    pub fn preflight(
        &self,
        intent: &WorkflowLaunchIntent,
        runtime: &WorkflowRuntime,
    ) -> Result<(), WorkflowError> {
        if intent.session_identity.trim().is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "session identity must not be empty",
            ));
        }
        if intent.workspace_identity.trim().is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "workspace identity must not be empty",
            ));
        }
        if intent.name.trim().is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "name must not be empty",
            ));
        }
        if intent.script.trim().is_empty() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "script must not be empty",
            ));
        }
        if !intent.args.is_object() {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                "args must be an object",
            ));
        }

        let expected_source = source_sha256_hex(intent.script.as_bytes());
        if intent.source_sha256 != expected_source {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMismatch,
                "source_sha256 does not match script bytes",
            ));
        }
        let expected_args = canonical_input_hash(&intent.args);
        if intent.args_sha256 != expected_args {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMismatch,
                "args_sha256 does not match args",
            ));
        }

        let request = intent.to_launch_request();
        runtime.validate_launch_request(&request)?;

        if let Some(schema) = &intent.compiled_input_schema {
            schema.validate_instance(&intent.args).map_err(|error| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    format!("args failed input_schema validation: {error}"),
                )
            })?;
        }

        compile_lua_source(&intent.script)?;
        Ok(())
    }

    fn bind_authorization(
        &self,
        intent: &WorkflowLaunchIntent,
        intent_digest: &str,
        capability: &WorkflowCapability,
    ) -> Result<(), WorkflowError> {
        let Some(nonce) = capability.launch_nonce() else {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMissing,
                "RunWorkflow requires a launch capability. Use the exact /workflow slash command first.",
            ));
        };
        if nonce != intent.launch_nonce {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::LaunchAuthorizationMismatch,
                "launch nonce does not match session capability",
            ));
        }
        capability.bind(intent_digest)
    }
}

/// Compile Lua without executing (preflight only).
pub fn compile_lua_source(source: &str) -> Result<(), WorkflowError> {
    use mlua::{Lua, LuaOptions, StdLib};

    let libs = StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
    let lua = Lua::new_with(libs, LuaOptions::default()).map_err(|error| {
        WorkflowError::coded(
            WorkflowErrorCode::LuaCompileFailed,
            format!("failed to create Lua VM for compile: {error}"),
        )
    })?;
    lua.load(source)
        .set_name("workflow script")
        .into_function()
        .map_err(|error| {
            WorkflowError::coded(
                WorkflowErrorCode::LuaCompileFailed,
                format!("Lua compile failed: {error}"),
            )
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_request() -> WorkflowLaunchRequest {
        WorkflowLaunchRequest {
            name: "demo".to_owned(),
            description: "demo workflow".to_owned(),
            phases: vec![WorkflowPhase {
                id: "work".to_owned(),
                description: "do work".to_owned(),
            }],
            script: "neo.phase('work')".to_owned(),
            args: json!({"target": "core"}),
            launch_source: "test".to_owned(),
            parent_run_id: None,
            output_schema: None,
        }
    }

    fn sample_binding() -> WorkflowLaunchBinding {
        WorkflowLaunchBinding {
            session_identity: "session-a".to_owned(),
            workspace_identity: "workspace-a".to_owned(),
            launch_nonce: "nonce-1".to_owned(),
            actor: WorkflowActor::Model,
            permission_mode: PermissionMode::Auto,
            parent_lineage: None,
            compiled_input_schema: None,
            schema_sha256: String::new(),
        }
    }

    #[test]
    fn intent_digest_is_stable_and_args_sensitive() {
        let a = WorkflowLaunchIntent::from_parts(sample_request(), sample_binding());
        let mut request_b = sample_request();
        request_b.args = json!({"target": "other"});
        let b = WorkflowLaunchIntent::from_parts(request_b, sample_binding());
        assert_eq!(a.digest(), a.digest());
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn preflight_rejects_invalid_lua_without_side_effects() {
        let runtime = WorkflowRuntime::default();
        let mut intent = WorkflowLaunchIntent::from_parts(sample_request(), sample_binding());
        intent.script = "function (".to_owned();
        intent.source_sha256 = source_sha256_hex(intent.script.as_bytes());
        let err = WorkflowLaunchCoordinator
            .preflight(&intent, &runtime)
            .expect_err("invalid lua");
        assert_eq!(err.code(), WorkflowErrorCode::LuaCompileFailed);
    }
}
