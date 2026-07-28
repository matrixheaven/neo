//! Verified linked-run checkpoints and seed import (Task 8 / design §34).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use neo_agent_core::AgentTokenUsage;
use neo_agent_core::workflow::journal::{JournalPayload, collect_journal_v2};
use neo_agent_core::workflow::{
    ArtifactKind, ArtifactValue, WorkflowActor, WorkflowErrorCode, WorkflowHandle,
    WorkflowInvocationKind, WorkflowInvocationOutcome, WorkflowLaunchRequest,
    WorkflowOutcomeStatus, WorkflowPhase, WorkflowRuntime, WorkflowState, run_dir,
};

fn launch_request(name: &str) -> WorkflowLaunchRequest {
    WorkflowLaunchRequest {
        name: name.to_owned(),
        description: "lineage test".to_owned(),
        phases: vec![WorkflowPhase {
            id: "work".to_owned(),
            description: "work".to_owned(),
        }],
        script: "return { ok = true }".to_owned(),
        args: serde_json::json!({}),
        launch_source: "/workflow".to_owned(),
        parent_run_id: None,
        output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
launch
    }
}

fn completed_with_usage(summary: &str, usage: AgentTokenUsage) -> WorkflowInvocationOutcome {
    WorkflowInvocationOutcome {
        ok: true,
        status: WorkflowOutcomeStatus::Completed,
        summary: summary.to_owned(),
        interruption: None,
        details: serde_json::json!({}),
        actual_usage: Some(usage),
        child_refs: Vec::new(),
    }
}

async fn wait_state(handle: &WorkflowHandle, want: WorkflowState) {
    for _ in 0..400 {
        if handle.snapshot().await.state == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "run did not reach {want:?}; last={:?}",
        handle.snapshot().await.state
    );
}

async fn wait_terminal(handle: &WorkflowHandle) {
    for _ in 0..400 {
        if handle.snapshot().await.state.is_terminal() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("run did not become terminal");
}

/// Build a terminal parent with one completed host call + one committed artifact.
///
/// Uses a dedicated runtime so callers can bind a different runner for the child.
async fn terminal_parent_with_artifact(session: &std::path::Path) -> WorkflowHandle {
    let runtime = WorkflowRuntime::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let gate_run = Arc::clone(&gate);
    runtime
        .bind_runner(move |handle, _meta, _session| {
            let gate = Arc::clone(&gate_run);
            async move {
                let _ = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Delegate,
                        serde_json::json!({"task": "parent-seed"}),
                        true,
                        |_| async {
                            completed_with_usage(
                                "parent done",
                                AgentTokenUsage {
                                    input_tokens: 11,
                                    output_tokens: 7,
                                    input_cache_read_tokens: 1,
                                    input_cache_write_tokens: 0,
                                },
                            )
                        },
                    )
                    .await?;
                handle
                    .commit_artifact(
                        "seed-note",
                        ArtifactKind::Text,
                        ArtifactValue::Text("verified seed artifact body".to_owned()),
                        None,
                    )
                    .await?;
                handle
                    .persist_canonical_final_result(serde_json::json!({"ok": true}), None)
                    .await?;
                gate.notified().await;
                Ok(())
            }
        })
        .expect("bind parent runner");

    let parent = runtime
        .create_run(session, launch_request("parent-lineage"))
        .await
        .expect("create parent");
    runtime
        .start_worker(&parent.run_id)
        .await
        .expect("start parent");
    wait_state(&parent, WorkflowState::Running).await;

    for _ in 0..200 {
        let out = parent.output().await.expect("parent output");
        if out.artifacts.iter().any(|a| a.logical_name == "seed-note") && out.final_result.is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    gate.notify_one();
    wait_terminal(&parent).await;
    assert_eq!(parent.snapshot().await.state, WorkflowState::Completed);
    // `WorkflowHandle` clones the runtime Arc; drop the local binding only.
    drop(runtime);
    parent
}

#[tokio::test]
async fn linked_upgrade_imports_verified_prefix_and_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    // Drop handle so only disk parent remains for the child runtime.
    drop(parent);

    let parent_journal_before =
        std::fs::read(run_dir(dir.path(), &parent_id).join("journal.jsonl")).unwrap();
    let parent_meta_before =
        std::fs::read(run_dir(dir.path(), &parent_id).join("run.json")).unwrap();

    let runtime = WorkflowRuntime::default();
    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id.clone(),
                checkpoint: None,
                link_reason: "v2_upgrade".to_owned(),
                launch: launch_request("child-lineage"),
            },
        )
        .await
        .expect("linked create");

    assert_ne!(child.run_id, parent_id);
    assert_eq!(child.snapshot().await.state, WorkflowState::Queued);

    let child_dir = run_dir(dir.path(), &child.run_id);
    let envelopes =
        collect_journal_v2(&child_dir.join("journal.jsonl"), Some(&child.run_id)).expect("journal");

    assert!(
        envelopes
            .iter()
            .any(|e| matches!(e.payload, JournalPayload::LineageSeedImported { .. })),
        "LineageSeedImported must be durable"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::InvocationStarted {
                kind: WorkflowInvocationKind::Delegate,
                ..
            }
        )),
        "seed must import completed InvocationStarted"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::InvocationFinished { outcome, .. } if outcome.ok
        )),
        "seed must import completed InvocationFinished"
    );
    assert!(
        envelopes.iter().any(|e| matches!(
            &e.payload,
            JournalPayload::ArtifactCommitted {
                logical_name: Some(name),
                ..
            } if name == "seed-note"
        )),
        "referenced artifacts must be imported by verified hash"
    );

    let output = child.output().await.expect("child output");
    assert!(
        output.actual_usage.is_none(),
        "inherited seed usage must not charge new-run actual_usage: {:?}",
        output.actual_usage
    );
    let inherited = output.inherited_usage.expect("inherited_usage set");
    assert_eq!(inherited.input_tokens, 11);
    assert_eq!(inherited.output_tokens, 7);
    assert!(
        output
            .artifacts
            .iter()
            .any(|a| a.logical_name == "seed-note"),
        "child must list imported artifact metadata"
    );
    let art = output
        .artifacts
        .iter()
        .find(|a| a.logical_name == "seed-note")
        .unwrap();
    let content = child
        .get_artifact(&art.artifact_id)
        .await
        .expect("read imported artifact");
    assert_eq!(content.bytes, b"verified seed artifact body");
    assert_eq!(
        content.metadata.sha256, art.sha256,
        "artifact content hash must match journal"
    );

    assert_eq!(
        parent_journal_before,
        std::fs::read(run_dir(dir.path(), &parent_id).join("journal.jsonl")).unwrap()
    );
    assert_eq!(
        parent_meta_before,
        std::fs::read(run_dir(dir.path(), &parent_id).join("run.json")).unwrap()
    );

    // Independent linked runs from the same parent never contend on a global
    // one-shot lock: a second fork succeeds immediately, concurrently.
    let runtime_for_second = runtime.clone();
    let session = dir.path().to_path_buf();
    let parent_for_second = parent_id.clone();
    let second = tokio::spawn(async move {
        runtime_for_second
            .create_linked_run(
                &session,
                neo_agent_core::workflow::runtime::LinkedRunRequest {
                    parent_run_id: parent_for_second,
                    checkpoint: None,
                    link_reason: "independent_fork".to_owned(),
                    launch: launch_request("second-child"),
                },
            )
            .await
    });
    let third = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id,
                checkpoint: None,
                link_reason: "independent_fork".to_owned(),
                launch: launch_request("third-child"),
            },
        )
        .await
        .expect("concurrent independent linked create");
    let second = second
        .await
        .expect("join second fork")
        .expect("concurrent independent linked create");
    assert_ne!(second.run_id, third.run_id);
    assert_ne!(second.run_id, child.run_id);
}

#[tokio::test]
async fn mismatch_stops_before_new_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    drop(parent);

    let runtime = WorkflowRuntime::default();
    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id,
                checkpoint: None,
                link_reason: "fork".to_owned(),
                launch: launch_request("mismatch-child"),
            },
        )
        .await
        .expect("linked create");

    let effect_ran = Arc::new(AtomicBool::new(false));
    let effect_flag = Arc::clone(&effect_ran);
    let mismatch = Arc::new(tokio::sync::Mutex::new(None));
    let mismatch_slot = Arc::clone(&mismatch);

    runtime
        .bind_runner(move |handle, _meta, _session| {
            let effect_flag = Arc::clone(&effect_flag);
            let mismatch_slot = Arc::clone(&mismatch_slot);
            async move {
                let err = handle
                    .invoke(
                        0,
                        WorkflowInvocationKind::Delegate,
                        serde_json::json!({"task": "DIFFERENT-from-seed"}),
                        true,
                        |_| {
                            let effect_flag = Arc::clone(&effect_flag);
                            async move {
                                effect_flag.store(true, Ordering::SeqCst);
                                completed_with_usage(
                                    "should never run",
                                    AgentTokenUsage {
                                        input_tokens: 1,
                                        output_tokens: 1,
                                        input_cache_read_tokens: 0,
                                        input_cache_write_tokens: 0,
                                    },
                                )
                            }
                        },
                    )
                    .await
                    .expect_err("seed mismatch must fail");
                *mismatch_slot.lock().await = Some(err);
                Err(neo_agent_core::workflow::WorkflowError::coded(
                    WorkflowErrorCode::LineageMismatch,
                    "seed mismatch stopped runner",
                ))
            }
        })
        .expect("bind mismatch runner");

    runtime
        .start_worker(&child.run_id)
        .await
        .expect("start child");
    wait_terminal(&child).await;

    assert!(
        !effect_ran.load(Ordering::SeqCst),
        "external effect must not run on lineage seed mismatch"
    );
    let err = mismatch
        .lock()
        .await
        .take()
        .expect("mismatch error captured");
    assert_eq!(err.code(), WorkflowErrorCode::LineageMismatch);

    let envelopes = collect_journal_v2(
        &run_dir(dir.path(), &child.run_id).join("journal.jsonl"),
        Some(&child.run_id),
    )
    .expect("child journal");
    let seed_pairs = neo_agent_core::workflow::runtime::seed_pair_count_from_journal(&envelopes);
    let started = envelopes
        .iter()
        .filter(|e| matches!(e.payload, JournalPayload::InvocationStarted { .. }))
        .count();
    assert_eq!(
        started, seed_pairs,
        "no new InvocationStarted may be appended before mismatch failure"
    );
}

#[tokio::test]
async fn terminal_parent_never_changes_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let parent = terminal_parent_with_artifact(dir.path()).await;
    let parent_id = parent.run_id.clone();
    let parent_dir = run_dir(dir.path(), &parent_id);
    let journal_before = std::fs::read(parent_dir.join("journal.jsonl")).unwrap();
    let meta_before = std::fs::read(parent_dir.join("run.json")).unwrap();
    drop(parent);

    // Load parent projection into a fresh runtime to assert in-memory state stability.
    let runtime = WorkflowRuntime::default();
    let handles = runtime.rehydrate(dir.path()).await.expect("rehydrate");
    let parent = handles
        .into_iter()
        .find(|h| h.run_id == parent_id)
        .expect("parent rehydrated");
    let parent_state = parent.snapshot().await.state;
    assert_eq!(parent_state, WorkflowState::Completed);
    let snap_before = parent.snapshot().await;

    let child = runtime
        .create_linked_run(
            dir.path(),
            neo_agent_core::workflow::runtime::LinkedRunRequest {
                parent_run_id: parent_id.clone(),
                checkpoint: None,
                link_reason: "retry_terminal".to_owned(),
                launch: launch_request("retry-child"),
            },
        )
        .await
        .expect("linked create from terminal parent");

    assert_ne!(child.run_id.as_str(), parent_id.as_str());
    let snap_after = parent.snapshot().await;
    assert_eq!(snap_after.state, parent_state);
    assert_eq!(snap_after.state, WorkflowState::Completed);
    assert_eq!(snap_after.terminal_reason, snap_before.terminal_reason);
    assert_eq!(
        journal_before,
        std::fs::read(parent_dir.join("journal.jsonl")).unwrap(),
        "terminal parent journal must remain byte-immutable"
    );
    assert_eq!(
        meta_before,
        std::fs::read(parent_dir.join("run.json")).unwrap(),
        "terminal parent run.json must remain byte-immutable"
    );

    let _err = parent
        .resume(WorkflowActor::Human)
        .await
        .expect_err("terminal resume");
    assert_eq!(
        journal_before,
        std::fs::read(parent_dir.join("journal.jsonl")).unwrap()
    );
}

// ---------------------------------------------------------------------------
// Task 17: child isolation, capability ceiling, worktree manager
// ---------------------------------------------------------------------------

use neo_agent_core::PermissionMode;
use neo_agent_core::harness::fake_model;
use neo_agent_core::multi_agent::{ChildPlan, ChildWorktreePolicy, DelegateContext};
use neo_agent_core::tools::ToolRegistry;
use neo_agent_core::workflow::{
    ChildIsolationRequest, ParentChildAuthority, ResolvedWorktreeBinding,
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
        output_schema: Nonemeta
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
,
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
