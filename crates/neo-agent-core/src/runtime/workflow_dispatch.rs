use std::path::PathBuf;
use std::sync::Arc;

use neo_ai::ModelClient;

use super::config::AgentConfig;
use super::context::AgentContext;
use super::instruction_context::InstructionContextBridge;
use super::tool_arguments::InstructionScopeProbe;
use crate::instructions::{
    InstructionPreflightDecision, InstructionReconcileKind, InstructionReconcileRequest,
};
use crate::workflow::{WorkflowInvocationOutcome, WorkflowOutcomeStatus};
use crate::{ToolAccess, tools::ProcessSupervisor, tools::ToolRegistry};

/// Cloneable bridge that carries the live runtime state needed to dispatch a
/// single workflow-hosted tool invocation through canonical instruction
/// preflight, permission, shell, and child-owner paths.
///
/// The bridge is cheap to clone so every live invocation can read the current
/// model/provider resolver, instruction state, and permission mode without
/// holding a mutex across awaits.
#[derive(Clone)]
pub struct WorkflowDispatchHandle {
    pub config: AgentConfig,
    pub model_client: Arc<dyn ModelClient>,
    pub registry: Arc<ToolRegistry>,
    pub process_supervisor: ProcessSupervisor,
    pub context: AgentContext,
    pub tool_access: Option<ToolAccess>,
}

impl WorkflowDispatchHandle {
    #[must_use]
    pub fn context(&self) -> &AgentContext {
        &self.context
    }

    /// Run instruction preflight for one workflow invocation. Returns `None` when
    /// preflight is not wired (no registry, no workspace, or no typed probes) or
    /// when the call is allowed to proceed. Returns a typed outcome when the call
    /// is deferred or blocked — no external effect runs and the workflow should
    /// pause with `instruction_replan_required`.
    pub async fn preflight_one(
        &self,
        invocation_id: &str,
        tool_name: &str,
        tool_input: &serde_json::Value,
    ) -> Option<WorkflowInvocationOutcome> {
        let registry = self.context.instruction_registry()?;
        let workspace = self.config.workspace_root.clone()?;

        let probes = InstructionScopeProbe::from_prepared_tool(tool_name, tool_input, &workspace);
        let targets: Vec<PathBuf> = probes
            .into_iter()
            .map(|p| {
                p.target_directory
                    .canonicalize()
                    .unwrap_or(p.target_directory)
            })
            .collect();

        if targets.is_empty() {
            return None;
        }

        let agent_id = self
            .config
            .agent_id
            .clone()
            .unwrap_or_else(|| crate::session::MAIN_AGENT_ID.to_owned());

        let request = InstructionReconcileRequest {
            agent_id: agent_id.clone(),
            kind: InstructionReconcileKind::ToolPreflight,
            target_directories: targets.clone(),
            budget: InstructionContextBridge::budget(&self.config, &self.context),
            deferred_tool_ids: vec![invocation_id.to_owned()],
        };

        match registry
            .reconcile(request, self.context.instruction_state())
            .await
        {
            InstructionPreflightDecision::Proceed { .. } => None,
            InstructionPreflightDecision::Defer { epoch, .. }
            | InstructionPreflightDecision::Block { epoch, .. } => {
                let reason = if epoch.failure.is_some() {
                    "instruction_block".to_owned()
                } else {
                    "instruction_replan_required".to_owned()
                };
                Some(WorkflowInvocationOutcome {
                    ok: false,
                    status: WorkflowOutcomeStatus::Interrupted,
                    summary: reason.clone(),
                    details: serde_json::json!({
                        "reason": reason,
                        "instruction_generation": epoch.generation,
                    }),
                    actual_usage: None,
                    child_refs: vec![],
                })
            }
        }
    }

    /// Execute one tool through the canonical `ToolRegistry::run` path. The
    /// registry uses the context's permission mode for approval. This is the
    /// same path used by `tool_dispatch::execute_tool_calls` for individual
    /// tool invocation.
    pub async fn run_one(
        &self,
        tool_name: &str,
        tool_input: serde_json::Value,
    ) -> WorkflowInvocationOutcome {
        let mut tool_context = crate::ToolContext::new(
            self.config
                .workspace_root
                .clone()
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        )
        .unwrap_or_else(|_| {
            crate::ToolContext::new(std::path::Path::new(".")).expect("fallback cwd")
        })
        .with_child_runtime(
            self.config.clone(),
            Arc::clone(&self.model_client),
            Arc::clone(&self.registry),
            0u32,
        );

        if let Some(access) = &self.tool_access {
            tool_context = tool_context.with_access(access.clone());
        }

        let result = self
            .registry
            .run(tool_name, &tool_context, tool_input)
            .await;

        tool_result_to_outcome(result)
    }
}

fn tool_result_to_outcome(
    result: Result<crate::ToolResult, crate::ToolError>,
) -> WorkflowInvocationOutcome {
    match result {
        Ok(r) => WorkflowInvocationOutcome {
            ok: !r.is_error,
            status: if r.is_error {
                WorkflowOutcomeStatus::Failed
            } else {
                WorkflowOutcomeStatus::Completed
            },
            summary: r.content.clone(),
            details: r.details.unwrap_or_else(|| serde_json::json!({})),
            actual_usage: None,
            child_refs: vec![],
        },
        Err(e) => {
            let msg = e.to_string();
            let is_denied = msg.to_ascii_lowercase().contains("permission")
                || msg.to_ascii_lowercase().contains("denied");
            WorkflowInvocationOutcome {
                ok: false,
                status: if is_denied {
                    WorkflowOutcomeStatus::Denied
                } else {
                    WorkflowOutcomeStatus::Failed
                },
                summary: msg,
                details: serde_json::json!({}),
                actual_usage: None,
                child_refs: vec![],
            }
        }
    }
}
