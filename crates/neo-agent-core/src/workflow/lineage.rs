//! Workflow child context, authority, and worktree isolation.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use neo_ai::ModelSpec;

use crate::AgentMessage;
use crate::PermissionMode;
use crate::instructions::InstructionInheritance;
use crate::multi_agent::{ChildPlan, ChildWorktreePolicy, DelegateContext};
use crate::tools::ToolRegistry;
use crate::workflow::error::{WorkflowError, WorkflowErrorCode};
use crate::worktree::{
    IsolatedWorktree, WorktreeError, WorktreeLifecycleState, WorktreeManager,
    path_is_portable_components,
};

// ---------------------------------------------------------------------------
// Per-child isolation and capability ceiling (design §32 / Task 17)
// ---------------------------------------------------------------------------

/// Maximum characters for a host-generated child context summary.
pub const CHILD_CONTEXT_SUMMARY_MAX_CHARS: usize = 2_048;

/// Parent authority snapshot used when resolving a child's ceilings.
pub struct ParentChildAuthority {
    pub permission_mode: PermissionMode,
    pub model: ModelSpec,
    /// Registered model aliases (`alias` → resolved [`ModelSpec`]).
    pub model_aliases: BTreeMap<String, ModelSpec>,
    /// Registered provider ids allowed as overrides.
    pub provider_ids: HashSet<String>,
    /// Parent-available tools (already role/session filtered as appropriate).
    pub tools: ToolRegistry,
    /// Canonical workspace the parent is running in.
    pub workspace_root: PathBuf,
    /// Parent messages used for inherit/summary context materialization.
    pub parent_messages: Vec<AgentMessage>,
}

/// Explicit child isolation request lowered from a [`ChildPlan`] or neo.delegate.
#[derive(Debug, Clone, PartialEq)]
pub struct ChildIsolationRequest {
    pub item_id: String,
    pub context: DelegateContext,
    pub worktree: ChildWorktreePolicy,
    pub tool_allow: Option<Vec<String>>,
    pub model: Option<String>,
    pub provider: Option<String>,
    /// Optional child-requested permission; must not exceed parent.
    pub permission_mode: Option<PermissionMode>,
}

impl ChildIsolationRequest {
    /// Build from a canonical [`ChildPlan`] (no child permission field on plan).
    #[must_use]
    pub fn from_child_plan(plan: &ChildPlan) -> Self {
        Self {
            item_id: plan.item_id.clone(),
            context: plan.context,
            worktree: plan.worktree,
            tool_allow: plan.tool_allow.clone(),
            model: plan.model.clone(),
            provider: plan.provider.clone(),
            permission_mode: None,
        }
    }
}

/// Resolved context policy — maps to existing instruction/context owners only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChildContext {
    pub mode: DelegateContext,
    pub instruction_inheritance: InstructionInheritance,
    /// Host-generated bounded summary for `summary` mode; never arbitrary hidden prompts.
    pub host_summary: Option<String>,
}

/// Shared or isolated worktree binding recorded in child provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedWorktreeBinding {
    Shared { workspace_root: PathBuf },
    Isolated { handle: IsolatedWorktree },
}

impl ResolvedWorktreeBinding {
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        match self {
            Self::Shared { workspace_root } => workspace_root.as_path(),
            Self::Isolated { handle } => handle.path.as_path(),
        }
    }

    #[must_use]
    pub fn policy(&self) -> ChildWorktreePolicy {
        match self {
            Self::Shared { .. } => ChildWorktreePolicy::Shared,
            Self::Isolated { .. } => ChildWorktreePolicy::Isolated,
        }
    }

    #[must_use]
    pub fn is_portable(&self) -> bool {
        path_is_portable_components(self.workspace_root())
    }
}

/// Fully resolved child start binding. Failures occur before any child agent starts.
#[derive(Debug, Clone)]
pub struct ResolvedChildIsolation {
    pub context: ResolvedChildContext,
    pub worktree: ResolvedWorktreeBinding,
    pub permission_mode: PermissionMode,
    pub model: ModelSpec,
    pub effective_tool_names: Vec<String>,
    pub tool_allow: Option<Vec<String>>,
}

/// Rank permission modes so a child cannot escalate Ask → Auto/Yolo.
#[must_use]
pub const fn permission_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Ask => 0,
        PermissionMode::Auto => 1,
        PermissionMode::Yolo => 2,
    }
}

/// Clamp or reject a child permission request against the parent ceiling.
///
/// Escalation is rejected with [`WorkflowErrorCode::PermissionDenied`].
/// When `requested` is `None`, the parent mode is inherited.
pub fn resolve_child_permission(
    parent: PermissionMode,
    requested: Option<PermissionMode>,
) -> Result<PermissionMode, WorkflowError> {
    let Some(child) = requested else {
        return Ok(parent);
    };
    if permission_rank(child) > permission_rank(parent) {
        return Err(WorkflowError::coded(
            WorkflowErrorCode::PermissionDenied,
            format!(
                "child permission mode {} escalates beyond parent {}",
                child.label(),
                parent.label()
            ),
        ));
    }
    Ok(child)
}

/// Resolve model/provider aliases through a provided catalog (canonical registries).
///
/// Missing aliases fail explicitly — there is no silent parent fallback once a
/// child supplies a model or provider override.
pub fn resolve_child_model(
    parent: &ParentChildAuthority,
    model_alias: Option<&str>,
    provider_override: Option<&str>,
) -> Result<ModelSpec, WorkflowError> {
    let mut resolved = if let Some(alias) = model_alias.map(str::trim).filter(|s| !s.is_empty()) {
        parent.model_aliases.get(alias).cloned().ok_or_else(|| {
            WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("unknown model alias `{alias}`"),
            )
        })?
    } else {
        parent.model.clone()
    };

    if let Some(provider) = provider_override.map(str::trim).filter(|s| !s.is_empty()) {
        if !parent.provider_ids.contains(provider) {
            return Err(WorkflowError::coded(
                WorkflowErrorCode::InvalidInput,
                format!("unknown provider override `{provider}`"),
            ));
        }
        // Compatible override: provider id must match a registered provider.
        // Model id stays as resolved; provider field is rewritten to the override.
        resolved.provider = neo_ai::ProviderId(provider.to_owned());
    }
    Ok(resolved)
}

/// Map context mode onto existing instruction inheritance + optional host summary.
///
/// Does not inject arbitrary hidden system prompts outside instruction ownership.
#[must_use]
pub fn resolve_child_context(
    mode: DelegateContext,
    parent_messages: &[AgentMessage],
) -> ResolvedChildContext {
    match mode {
        DelegateContext::Inherit => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::FullContext,
            host_summary: None,
        },
        DelegateContext::Summary => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::Summary,
            host_summary: Some(host_bounded_context_summary(parent_messages)),
        },
        DelegateContext::None => ResolvedChildContext {
            mode,
            instruction_inheritance: InstructionInheritance::Summary,
            host_summary: None,
        },
    }
}

/// Host-generated bounded summary from parent messages (existing context owner surface).
#[must_use]
pub fn host_bounded_context_summary(messages: &[AgentMessage]) -> String {
    let mut parts = Vec::new();
    for message in messages {
        let text = message.text();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_owned());
        }
    }
    let joined = parts.join("\n");
    let mut out: String = joined
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .take(CHILD_CONTEXT_SUMMARY_MAX_CHARS)
        .collect();
    if joined.chars().count() > CHILD_CONTEXT_SUMMARY_MAX_CHARS {
        out.push('…');
    }
    out.trim().to_owned()
}

/// Intersect parent tools with optional child `tool_allow` ceiling (exact names).
#[must_use]
pub fn resolve_child_tool_ceiling(
    parent_tools: &ToolRegistry,
    tool_allow: Option<&[String]>,
) -> ToolRegistry {
    parent_tools.for_workflow_child(tool_allow)
}

fn map_worktree_error(error: WorktreeError) -> WorkflowError {
    match error {
        WorktreeError::Unsupported { message } => {
            WorkflowError::coded(WorkflowErrorCode::InvalidInput, message)
        }
        WorktreeError::CreateFailed { message }
        | WorktreeError::CleanupFailed { message }
        | WorktreeError::Io { message } => WorkflowError::coded(WorkflowErrorCode::Host, message),
        WorktreeError::CleanupRefused { message } => {
            WorkflowError::coded(WorkflowErrorCode::InvalidOperation, message)
        }
    }
}

/// Resolve worktree policy. For `isolated`, fails before creation when unsupported.
///
/// Shared paths record the parent workspace. Isolated paths go through
/// [`WorktreeManager`] only — never ad-hoc shell strings. No auto-merge.
pub fn resolve_child_worktree(
    policy: ChildWorktreePolicy,
    parent_workspace: &Path,
    child_key: &str,
    manager: Option<&WorktreeManager>,
) -> Result<ResolvedWorktreeBinding, WorkflowError> {
    match policy {
        ChildWorktreePolicy::Shared => Ok(ResolvedWorktreeBinding::Shared {
            workspace_root: parent_workspace.to_path_buf(),
        }),
        ChildWorktreePolicy::Isolated => {
            let manager = manager.ok_or_else(|| {
                WorkflowError::coded(
                    WorkflowErrorCode::InvalidInput,
                    "isolated worktree unsupported: no worktree manager is configured",
                )
            })?;
            // Fail before child start when isolation is unsupported.
            manager
                .ensure_isolation_supported(parent_workspace)
                .map_err(map_worktree_error)?;
            let handle = manager
                .create_isolated(parent_workspace, child_key)
                .map_err(map_worktree_error)?;
            if !path_is_portable_components(&handle.path) {
                return Err(WorkflowError::coded(
                    WorkflowErrorCode::Host,
                    format!(
                        "isolated worktree path is not portable: {}",
                        handle.path.display()
                    ),
                ));
            }
            Ok(ResolvedWorktreeBinding::Isolated {
                handle: handle.mark_active(),
            })
        }
    }
}

/// Full pre-start resolution: context, model/provider, permission, tools, worktree.
///
/// On any failure no isolated worktree is left behind from this call when the
/// worktree step is what failed; earlier steps have no external effects.
pub fn resolve_child_isolation(
    parent: &ParentChildAuthority,
    request: &ChildIsolationRequest,
    worktree_manager: Option<&WorktreeManager>,
) -> Result<ResolvedChildIsolation, WorkflowError> {
    let context = resolve_child_context(request.context, &parent.parent_messages);
    let permission_mode =
        resolve_child_permission(parent.permission_mode, request.permission_mode)?;
    let model = resolve_child_model(
        parent,
        request.model.as_deref(),
        request.provider.as_deref(),
    )?;
    let tools = resolve_child_tool_ceiling(&parent.tools, request.tool_allow.as_deref());
    // Worktree last among policy checks that can create external state: still
    // runs before any child agent start, and fails closed on unsupported repos.
    let worktree = resolve_child_worktree(
        request.worktree,
        &parent.workspace_root,
        &request.item_id,
        worktree_manager,
    )?;
    Ok(ResolvedChildIsolation {
        context,
        worktree,
        permission_mode,
        model,
        effective_tool_names: tools.names(),
        tool_allow: request.tool_allow.clone(),
    })
}

/// Explicit cleanup helper for isolated bindings (never auto-invoked).
pub fn cleanup_isolated_worktree(
    manager: &WorktreeManager,
    binding: &mut ResolvedWorktreeBinding,
) -> Result<(), WorkflowError> {
    match binding {
        ResolvedWorktreeBinding::Shared { .. } => Ok(()),
        ResolvedWorktreeBinding::Isolated { handle } => {
            manager.cleanup_explicit(handle).map_err(map_worktree_error)
        }
    }
}

/// Provenance snapshot suitable for journal/details (no hidden authority).
#[must_use]
pub fn child_isolation_provenance(resolved: &ResolvedChildIsolation) -> serde_json::Value {
    let worktree = match &resolved.worktree {
        ResolvedWorktreeBinding::Shared { workspace_root } => serde_json::json!({
            "policy": "shared",
            "path": workspace_root.display().to_string(),
            "cleanup": "n/a",
        }),
        ResolvedWorktreeBinding::Isolated { handle } => serde_json::json!({
            "policy": "isolated",
            "path": handle.path.display().to_string(),
            "source": handle.source_workspace.display().to_string(),
            "state": match handle.state {
                WorktreeLifecycleState::Created => "created",
                WorktreeLifecycleState::Active => "active",
                WorktreeLifecycleState::Cleaned => "cleaned",
                WorktreeLifecycleState::DirtyRefusedCleanup => "dirty_refused_cleanup",
            },
            "dirty": handle.dirty,
            "cleanup": "explicit_only",
            "auto_merge": false,
        }),
    };
    serde_json::json!({
        "context_mode": resolved.context.mode.as_str(),
        "instruction_inheritance": match resolved.context.instruction_inheritance {
            InstructionInheritance::FullContext => "full_context",
            InstructionInheritance::Summary => "summary",
        },
        "host_summary_present": resolved.context.host_summary.is_some(),
        "permission_mode": resolved.permission_mode.label(),
        "model": {
            "provider": resolved.model.provider.0,
            "model": resolved.model.model,
        },
        "tool_allow": resolved.tool_allow,
        "effective_tools": resolved.effective_tool_names,
        "worktree": worktree,
    })
}
