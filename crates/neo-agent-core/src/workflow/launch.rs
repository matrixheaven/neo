//! Stateless workflow launch normalization and sequencing.
//!
//! All adapters (model `Workflow` tool, named slash, headless CLI) build one
//! immutable [`WorkflowLaunchIntent`] and call [`WorkflowLaunchCoordinator`].
//! The coordinator never writes `run.json`/journal, never owns admission
//! state, and never registers tasks itself beyond calling the existing owners
//! in the required order. Interactive human authorization lives in the normal
//! permission/approval layer, never in launch state.

use std::path::Path;

use super::error::{WorkflowError, WorkflowErrorCode};
use super::journal::canonical_input_hash;
use super::runtime::{WorkflowHandle, WorkflowLaunchRequest, WorkflowRuntime};
use super::schema::CompiledSchema;
use super::source_sha256_hex;
use super::state::{WorkflowActor, WorkflowPhase, WorkflowRevision};
use crate::PermissionMode;
use crate::tools::BackgroundTaskManager;

/// Immutable launch intent produced by every adapter before durable creation.
///
/// Payload fields drive [`WorkflowLaunchRequest`] construction only.
#[derive(Clone)]
pub struct WorkflowLaunchIntent {
    pub session_identity: String,
    pub workspace_identity: String,
    pub launch_source: String,
    pub definition_revision: WorkflowRevision,
    pub source_sha256: String,
    pub args: serde_json::Value,
    pub args_sha256: String,
    /// SHA-256 of canonical schema material (empty when no schema binding).
    pub schema_sha256: String,
    pub actor: WorkflowActor,
    pub permission_mode: PermissionMode,
    pub name: String,
    pub description: String,
    pub phases: Vec<WorkflowPhase>,
    pub script: String,
    /// Optional precompiled input schema for argument validation.
    pub compiled_input_schema: Option<CompiledSchema>,
    /// Final output schema JSON pinned onto the run for production validation.
    pub output_schema: Option<serde_json::Value>,
    /// Optional user-facing display name for Operator and completion.
    pub display_name: Option<String>,
    /// Input schema JSON pinned at launch.
    pub input_schema: Option<serde_json::Value>,
    /// Pinned definition origin.
    pub definition_origin: Option<super::state::WorkflowSourceOrigin>,
    /// Whether this is an inline (unsaved) run.
    pub inline_unsaved: bool,
}

/// Session/workspace binding fields for [`WorkflowLaunchIntent::from_parts`].
#[derive(Clone)]
pub struct WorkflowLaunchBinding {
    pub session_identity: String,
    pub workspace_identity: String,
    pub actor: WorkflowActor,
    pub permission_mode: PermissionMode,
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
        let definition_revision = WorkflowRevision::from_bytes(request.script.as_bytes());
        Self {
            session_identity: binding.session_identity,
            workspace_identity: binding.workspace_identity,
            launch_source: request.launch_source.clone(),
            definition_revision,
            source_sha256,
            args: request.args.clone(),
            args_sha256,
            schema_sha256: binding.schema_sha256,
            actor: binding.actor,
            permission_mode: binding.permission_mode,
            name: request.name,
            description: request.description,
            phases: request.phases,
            script: request.script,
            compiled_input_schema: binding.compiled_input_schema,
            output_schema: request.output_schema,
            display_name: request.display_name.clone(),
            input_schema: request.input_schema.clone(),
            definition_origin: request.definition_origin,
            inline_unsaved: request.inline_unsaved,
        }
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
            output_schema: self.output_schema.clone(),
            display_name: self.display_name.clone(),
            input_schema: self.input_schema.clone(),
            definition_origin: self.definition_origin,
            inline_unsaved: self.inline_unsaved,
        }
    }
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
    pub background_tasks: &'a BackgroundTaskManager,
    pub session_dir: &'a Path,
}

/// Stateless launch normalization / orchestration.
#[derive(Debug, Default, Clone, Copy)]
pub struct WorkflowLaunchCoordinator;

impl WorkflowLaunchCoordinator {
    /// Preflight, durable-create, register, emit started, admit/start.
    ///
    /// Order (design §12–§14):
    /// 1. pure preflight (limits, Lua compile, input schema)
    /// 2. `WorkflowRuntime::create_run`
    /// 3. task registration (failure rolls the durable run back)
    /// 4. emit started + `start_worker` (admission may leave the run queued)
    pub async fn launch(
        self,
        intent: &WorkflowLaunchIntent,
        hosts: WorkflowLaunchHosts<'_>,
    ) -> Result<WorkflowLaunchOutcome, WorkflowError> {
        self.preflight(intent, hosts.runtime)?;

        let request = intent.to_launch_request();
        let handle = hosts.runtime.create_run(hosts.session_dir, request).await?;

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

    /// Pure validation before any durable create.
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
                WorkflowErrorCode::InvalidInput,
                "source_sha256 does not match script bytes",
            ));
        }
        let expected_args = canonical_input_hash(&intent.args);
        if intent.args_sha256 != expected_args {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
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
            output_schema: None,
            display_name: None,
            input_schema: None,
            definition_origin: None,
            inline_unsaved: false,
        }
    }

    fn sample_binding() -> WorkflowLaunchBinding {
        WorkflowLaunchBinding {
            session_identity: "session-a".to_owned(),
            workspace_identity: "workspace-a".to_owned(),
            actor: WorkflowActor::Model,
            permission_mode: PermissionMode::Auto,
            compiled_input_schema: None,
            schema_sha256: String::new(),
        }
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
