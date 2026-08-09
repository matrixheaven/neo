//! Workflow child context, authority, and worktree isolation.

// ---------------------------------------------------------------------------
// Task 17: child isolation, capability ceiling, worktree manager
// ---------------------------------------------------------------------------

use neo_agent_core::PermissionMode;
use neo_agent_core::harness::fake_model;
use neo_agent_core::multi_agent::{ChildPlan, ChildWorktreePolicy, DelegateContext};
use neo_agent_core::tools::ToolRegistry;
use neo_agent_core::workflow::{
    ChildIsolationRequest, ParentChildAuthority, ResolvedWorktreeBinding, WorkflowErrorCode,
    child_isolation_provenance, cleanup_isolated_worktree, resolve_child_isolation,
    resolve_child_permission,
};
use neo_agent_core::worktree::{
    WorktreeLifecycleState, WorktreeManager, path_is_portable_components,
};
use neo_ai::{ApiKind, ModelCapabilities, ModelSpec, ProviderId, ReasoningCapability};
use std::collections::{BTreeMap, HashSet};
use std::process::Command;

fn sample_model(provider: &str, model: &str) -> ModelSpec {
    ModelSpec {
        provider: ProviderId(provider.to_owned()),
        model: model.to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities {
            streaming: true,
            tools: true,
            images: false,
            videos: false,
            reasoning: ReasoningCapability::None,
            embeddings: false,
            max_context_tokens: None,
            max_output_tokens: None,
        },
    }
}

fn parent_authority(
    workspace: &std::path::Path,
    permission: PermissionMode,
) -> ParentChildAuthority {
    let parent_model = fake_model();
    let mut aliases = BTreeMap::new();
    aliases.insert(
        "reviewer-fast".to_owned(),
        sample_model("openai", "gpt-test-reviewer"),
    );
    aliases.insert("parent-default".to_owned(), parent_model.clone());
    let mut providers = HashSet::new();
    providers.insert("openai".to_owned());
    providers.insert("fake".to_owned());
    providers.insert("anthropic".to_owned());
    ParentChildAuthority {
        permission_mode: permission,
        model: parent_model,
        model_aliases: aliases,
        provider_ids: providers,
        tools: ToolRegistry::with_builtin_tools(),
        workspace_root: workspace.to_path_buf(),
        parent_messages: vec![
            neo_agent_core::AgentMessage::user_text("parent user turn with details"),
            neo_agent_core::AgentMessage::assistant(
                vec![neo_agent_core::Content::text("parent assistant reply")],
                vec![],
                neo_agent_core::StopReason::EndTurn,
            ),
        ],
    }
}

fn init_git_repo(path: &std::path::Path) {
    assert!(
        Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(path)
            .status()
            .expect("git init")
            .success()
    );
    // Identity for commit without touching global config permanently.
    assert!(
        Command::new("git")
            .args(["-C"])
            .arg(path)
            .args(["config", "user.email", "neo-test@example.com"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["-C"])
            .arg(path)
            .args(["config", "user.name", "Neo Test"])
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(path.join("README.md"), "seed\n").unwrap();
    assert!(
        Command::new("git")
            .args(["-C"])
            .arg(path)
            .args(["add", "README.md"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["-C"])
            .arg(path)
            .args(["commit", "-m", "seed"])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn child_context_and_capability_ceiling_are_explicit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = parent_authority(dir.path(), PermissionMode::Ask);

    // inherit / summary / none are explicit and map to instruction owners only.
    for mode in [
        DelegateContext::Inherit,
        DelegateContext::Summary,
        DelegateContext::None,
    ] {
        let request = ChildIsolationRequest {
            item_id: format!("ctx-{}", mode.as_str()),
            context: mode,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: Some(vec![
                "Read".to_owned(),
                "Grep".to_owned(),
                "Delegate".to_owned(),
            ]),
            model: Some("reviewer-fast".to_owned()),
            provider: Some("openai".to_owned()),
            permission_mode: Some(PermissionMode::Ask),
        };
        let resolved = resolve_child_isolation(&parent, &request, None).expect("resolve");
        assert_eq!(resolved.context.mode, mode);
        match mode {
            DelegateContext::Inherit => {
                assert!(resolved.context.host_summary.is_none());
                assert_eq!(
                    resolved.context.instruction_inheritance,
                    neo_agent_core::instructions::InstructionInheritance::FullContext
                );
            }
            DelegateContext::Summary => {
                let summary = resolved
                    .context
                    .host_summary
                    .as_deref()
                    .expect("summary text");
                assert!(summary.contains("parent"));
                assert!(summary.chars().count() <= 2048 + 1);
                assert_eq!(
                    resolved.context.instruction_inheritance,
                    neo_agent_core::instructions::InstructionInheritance::Summary
                );
            }
            DelegateContext::None => {
                assert!(resolved.context.host_summary.is_none());
                assert_eq!(
                    resolved.context.instruction_inheritance,
                    neo_agent_core::instructions::InstructionInheritance::Summary
                );
            }
        }
        // tool_allow may only reduce; denied workflow tools never reappear.
        assert!(resolved.effective_tool_names.contains(&"Read".to_owned()));
        assert!(resolved.effective_tool_names.contains(&"Grep".to_owned()));
        assert!(
            !resolved
                .effective_tool_names
                .iter()
                .any(|n| n == "Delegate"),
            "Delegate must stay denied even if listed in tool_allow"
        );
        assert!(
            !resolved.effective_tool_names.iter().any(|n| n == "Write"),
            "Write not in tool_allow must not appear"
        );
        // Model/provider aliases resolve canonically.
        assert_eq!(resolved.model.provider.0, "openai");
        assert_eq!(resolved.model.model, "gpt-test-reviewer");
        assert_eq!(resolved.permission_mode, PermissionMode::Ask);
        assert!(matches!(
            resolved.worktree,
            ResolvedWorktreeBinding::Shared { .. }
        ));
        let prov = child_isolation_provenance(&resolved);
        assert_eq!(prov["context_mode"], mode.as_str());
        assert_eq!(prov["worktree"]["policy"], "shared");
    }

    // Child permission cannot escalate Ask → Auto/Yolo.
    let err = resolve_child_permission(PermissionMode::Ask, Some(PermissionMode::Yolo))
        .expect_err("escalation must fail");
    assert_eq!(err.code(), WorkflowErrorCode::PermissionDenied);

    let err = resolve_child_isolation(
        &parent,
        &ChildIsolationRequest {
            item_id: "escalate".to_owned(),
            context: DelegateContext::None,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: None,
            model: None,
            provider: None,
            permission_mode: Some(PermissionMode::Auto),
        },
        None,
    )
    .expect_err("permission escalate");
    assert_eq!(err.code(), WorkflowErrorCode::PermissionDenied);

    // Unknown model alias fails explicitly (no silent parent fallback).
    let err = resolve_child_isolation(
        &parent,
        &ChildIsolationRequest {
            item_id: "bad-model".to_owned(),
            context: DelegateContext::None,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: None,
            model: Some("does-not-exist".to_owned()),
            provider: None,
            permission_mode: None,
        },
        None,
    )
    .expect_err("unknown model");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidInput);

    // ChildPlan lowers into isolation request without inventing fields.
    let plan = ChildPlan {
        item_id: "from-plan".to_owned(),
        item_label: "from-plan".to_owned(),
        task: "review".to_owned(),
        title: None,
        resume: None,
        role: None,
        model: Some("reviewer-fast".to_owned()),
        provider: None,
        context: DelegateContext::Summary,
        worktree: ChildWorktreePolicy::Shared,
        tool_allow: Some(vec!["Read".to_owned()]),
        output_schema: None,
    };
    let from_plan = ChildIsolationRequest::from_child_plan(&plan);
    assert_eq!(from_plan.context, DelegateContext::Summary);
    assert_eq!(
        from_plan.tool_allow.as_deref(),
        Some(&["Read".to_owned()][..])
    );
}

#[test]
fn unsupported_isolated_worktree_fails_before_child_start() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Non-git directory: isolation unsupported.
    let parent = parent_authority(dir.path(), PermissionMode::Auto);
    let manager = WorktreeManager::new(dir.path().join("worktrees"));

    // ensure_isolation_supported fails closed.
    let unsupported = manager.ensure_isolation_supported(dir.path());
    assert!(
        unsupported.is_err(),
        "non-git workspace must be unsupported"
    );

    let mut child_started = false;
    let request = ChildIsolationRequest {
        item_id: "iso-fail".to_owned(),
        context: DelegateContext::None,
        worktree: ChildWorktreePolicy::Isolated,
        tool_allow: Some(vec!["Read".to_owned()]),
        model: None,
        provider: None,
        permission_mode: None,
    };
    let err = resolve_child_isolation(&parent, &request, Some(&manager)).expect_err("must fail");
    assert_eq!(
        err.code(),
        WorkflowErrorCode::InvalidInput,
        "unsupported isolation is typed invalid_input before child start"
    );
    // No worktree directory should have been created under the manager base.
    assert!(
        !dir.path().join("worktrees").join("iso-fail").exists(),
        "must not create worktree path on unsupported isolation"
    );
    // Simulate the gate: child start only happens after resolve succeeds.
    if err.code() != WorkflowErrorCode::InvalidInput {
        child_started = true;
    }
    assert!(!child_started);

    // Missing manager also fails before start (no silent shared fallback).
    let err = resolve_child_isolation(&parent, &request, None).expect_err("no manager");
    assert_eq!(err.code(), WorkflowErrorCode::InvalidInput);
    assert!(
        err.to_string().contains("no worktree manager") || err.to_string().contains("unsupported"),
        "message should explain unsupported isolation: {err}"
    );
}

#[test]
fn isolated_worktree_paths_are_portable_and_cleanup_is_explicit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git_repo(&repo);

    let base = dir.path().join("isolated-children");
    let manager = WorktreeManager::new(&base);
    manager
        .ensure_isolation_supported(&repo)
        .expect("git repo supports isolation");

    let parent = parent_authority(&repo, PermissionMode::Yolo);
    let request = ChildIsolationRequest {
        item_id: "child/path:key".to_owned(),
        context: DelegateContext::None,
        worktree: ChildWorktreePolicy::Isolated,
        tool_allow: None,
        model: None,
        provider: None,
        permission_mode: Some(PermissionMode::Auto), // reduce, do not escalate
    };
    let mut resolved =
        resolve_child_isolation(&parent, &request, Some(&manager)).expect("isolated resolve");
    assert_eq!(resolved.permission_mode, PermissionMode::Auto);

    let ResolvedWorktreeBinding::Isolated { handle } = &resolved.worktree else {
        panic!("expected isolated binding");
    };
    assert!(handle.path.exists(), "worktree path must exist");
    assert!(
        handle.path.starts_with(&base),
        "isolated path must live under manager base"
    );
    assert!(
        path_is_portable_components(&handle.path),
        "path components must be portable PathBuf segments"
    );
    // Sanitized key: no raw separators in the leaf component.
    let leaf = handle.path.file_name().unwrap().to_string_lossy();
    assert!(!leaf.contains('/'));
    assert!(!leaf.contains('\\'));
    assert_eq!(handle.state, WorktreeLifecycleState::Active);
    assert!(!handle.dirty);
    let prov = child_isolation_provenance(&resolved);
    assert_eq!(prov["worktree"]["policy"], "isolated");
    assert_eq!(prov["worktree"]["cleanup"], "explicit_only");
    assert_eq!(prov["worktree"]["auto_merge"], false);

    // Dirty tree: cleanup refuses (never delete dirty worktrees).
    std::fs::write(handle.path.join("dirty.txt"), "unreviewed\n").unwrap();
    let refuse = cleanup_isolated_worktree(&manager, &mut resolved.worktree);
    assert!(refuse.is_err(), "dirty cleanup must refuse");
    if let ResolvedWorktreeBinding::Isolated { handle } = &resolved.worktree {
        assert_eq!(handle.state, WorktreeLifecycleState::DirtyRefusedCleanup);
        assert!(handle.dirty);
        assert!(handle.path.exists(), "dirty path must remain");
    } else {
        panic!("still isolated");
    }

    // Explicit clean cleanup after reverting dirt.
    std::fs::remove_file(match &resolved.worktree {
        ResolvedWorktreeBinding::Isolated { handle } => handle.path.join("dirty.txt"),
        _ => unreachable!(),
    })
    .unwrap();
    // Refresh dirty flag via manager after clean.
    if let ResolvedWorktreeBinding::Isolated { handle } = &mut resolved.worktree {
        manager.refresh_dirty(handle).expect("refresh");
        assert!(!handle.dirty);
        // Reset state so cleanup_explicit proceeds (dirty refused left state sticky).
        handle.state = WorktreeLifecycleState::Active;
    }
    cleanup_isolated_worktree(&manager, &mut resolved.worktree).expect("explicit clean cleanup");
    if let ResolvedWorktreeBinding::Isolated { handle } = &resolved.worktree {
        assert_eq!(handle.state, WorktreeLifecycleState::Cleaned);
        assert!(
            !handle.path.exists(),
            "clean explicit cleanup removes worktree path"
        );
    }

    // Shared binding cleanup is a no-op (no auto side effects).
    let mut shared = ResolvedWorktreeBinding::Shared {
        workspace_root: repo.clone(),
    };
    cleanup_isolated_worktree(&manager, &mut shared).expect("shared cleanup no-op");
}
