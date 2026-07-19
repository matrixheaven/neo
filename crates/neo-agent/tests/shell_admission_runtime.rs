//! Cross-boundary ownership tests for shell admission cancellation and isolation.
//!
//! These tests exercise the public `ToolRegistry` / `MultiAgentRuntime` surfaces with
//! a real capacity-one `ShellRuntime` (guardian binary from `CARGO_BIN_EXE_neo`).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use neo_agent_core::harness::{FakeHarness, fake_model};
use neo_agent_core::multi_agent::{
    AgentActivityKind, AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode,
    AgentToolActivityPhase, ChildRuntimeDeps, DelegateContext, MultiAgentRuntime, SwarmAggregate,
    SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::{
    AgentConfig, PermissionMode, ShellLimits, ShellRuntime, ToolAccess, ToolContext, ToolError,
    ToolRegistry, execute_model_bash_for_runtime,
};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;

fn capacity_one_runtime(workspace: &tempfile::TempDir) -> ShellRuntime {
    ShellRuntime::new(
        ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        },
        PathBuf::from(env!("CARGO_BIN_EXE_neo")),
        workspace.path().join("runtime"),
    )
}

fn tool_context(workspace: &tempfile::TempDir, runtime: ShellRuntime) -> ToolContext {
    ToolContext::new(workspace.path())
        .expect("tool context")
        .with_access(ToolAccess::all())
        .with_shell_runtime(runtime)
}

fn count_running_markers(runtime_root: &Path) -> usize {
    let mut count = 0;
    let Ok(entries) = std::fs::read_dir(runtime_root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            count += count_running_markers(&path);
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".running.json"))
        {
            count += 1;
        }
    }
    count
}

fn list_running_markers(runtime_root: &Path) -> Vec<PathBuf> {
    let mut markers = Vec::new();
    collect_running_markers(runtime_root, &mut markers);
    markers.sort();
    markers
}

fn collect_running_markers(runtime_root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(runtime_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_running_markers(&path, out);
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".running.json"))
        {
            out.push(path);
        }
    }
}

fn hold_command() -> &'static str {
    // Interactive shell keeps the Terminal session (and its permit) alive until Stop.
    "bash --noprofile --norc"
}

fn printf_command(payload: &str) -> String {
    format!("printf '{payload}'")
}

async fn start_terminal_hold(registry: &ToolRegistry, context: &ToolContext) -> String {
    let started = registry
        .run(
            "Terminal",
            context,
            json!({
                "mode": "start",
                "command": hold_command(),
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal hold start");
    started
        .details
        .as_ref()
        .and_then(|details| details["handle"].as_str())
        .expect("terminal handle")
        .to_owned()
}

async fn stop_terminal(registry: &ToolRegistry, context: &ToolContext, handle: &str) {
    registry
        .run(
            "Terminal",
            context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop");
}

async fn wait_for_running_markers(runtime_root: &Path, expected: usize) {
    for _ in 0..500 {
        if count_running_markers(runtime_root) >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "expected at least {expected} running marker(s), found {}",
        count_running_markers(runtime_root)
    );
}

fn bash_tool_harness(command: &str) -> FakeHarness {
    let args = json!({ "command": command }).to_string();
    FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            id: "msg".to_owned(),
        },
        AiStreamEvent::ToolCallStart {
            id: "bash_call".to_owned(),
            name: "Bash".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "bash_call".to_owned(),
            json_fragment: args.clone(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "bash_call".to_owned(),
            raw_arguments: args,
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::ToolUse,
            usage: None,
        },
    ])
}

fn agent_config(workspace: &tempfile::TempDir, runtime: ShellRuntime) -> AgentConfig {
    AgentConfig::for_model(fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Yolo)
        .with_shell_runtime(runtime)
}

fn activity_has_queued_bash(snapshot: &neo_agent_core::multi_agent::AgentSnapshot) -> bool {
    snapshot.activity.iter().any(|entry| {
        matches!(
            &entry.kind,
            AgentActivityKind::Tool {
                name,
                phase: AgentToolActivityPhase::Queued { .. },
                ..
            } if name == "Bash"
        )
    })
}

async fn wait_for_queued_bash(runtime: &MultiAgentRuntime, agent_id: &str) {
    for _ in 0..500 {
        if runtime
            .agent_snapshot(agent_id)
            .is_some_and(|snapshot| activity_has_queued_bash(&snapshot))
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("agent {agent_id} never entered queued Bash activity");
}

#[tokio::test]
async fn turn_cancellation_removes_queued_bash_without_spawning() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = capacity_one_runtime(&workspace);
    let runtime_root = runtime.runtime_root().to_path_buf();
    let context = tool_context(&workspace, runtime.clone());
    let registry = ToolRegistry::with_builtin_tools();

    let handle = start_terminal_hold(&registry, &context).await;
    wait_for_running_markers(&runtime_root, 1).await;
    let markers_before = list_running_markers(&runtime_root);
    assert_eq!(markers_before.len(), 1);

    let cancel = tokio_util::sync::CancellationToken::new();
    let queued_ctx = context.clone().with_cancel_token(cancel.clone());
    let queued = tokio::spawn({
        let queued_ctx = queued_ctx.clone();
        async move {
            execute_model_bash_for_runtime(
                &queued_ctx,
                json!({ "command": printf_command("should-not-run") }),
            )
            .await
        }
    });

    for _ in 0..50 {
        assert!(
            !queued.is_finished(),
            "queued bash finished before turn cancellation"
        );
        assert_eq!(
            list_running_markers(&runtime_root),
            markers_before,
            "queued bash must not spawn an extra guardian"
        );
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    cancel.cancel();
    let result = queued.await.expect("join queued bash");
    assert!(
        matches!(result, Err(ToolError::Cancelled)),
        "turn cancellation should cancel queued bash: {result:?}"
    );
    assert_eq!(
        list_running_markers(&runtime_root),
        markers_before,
        "cancellation must not create guardian markers"
    );

    stop_terminal(&registry, &context, &handle).await;

    let probe = execute_model_bash_for_runtime(
        &context,
        json!({ "command": printf_command("probe-turn") }),
    )
    .await
    .expect("probe bash after release");
    assert!(
        probe.content.contains("probe-turn"),
        "released capacity should admit a fresh bash: {}",
        probe.content
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_cancellation_removes_queued_child_bash_without_spawning() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = capacity_one_runtime(&workspace);
    let runtime_root = runtime.runtime_root().to_path_buf();
    let hold_ctx = tool_context(&workspace, runtime.clone());
    let registry = ToolRegistry::with_builtin_tools();

    let handle = start_terminal_hold(&registry, &hold_ctx).await;
    wait_for_running_markers(&runtime_root, 1).await;
    let markers_before = list_running_markers(&runtime_root);
    assert_eq!(markers_before.len(), 1);

    let multi = MultiAgentRuntime::new();
    let harness = bash_tool_harness(&printf_command("should-not-run-agent"));
    let deps = ChildRuntimeDeps::new(
        agent_config(&workspace, runtime.clone()),
        harness.client(),
        Arc::new(ToolRegistry::with_builtin_tools()),
    );
    let snapshot = multi.start_delegate(
        "queued bash child",
        None,
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::Root,
    );
    let agent_id = snapshot.id.clone();
    let run = tokio::spawn({
        let multi = multi.clone();
        async move {
            multi
                .run_started_child_turn(deps, snapshot, DelegateContext::None, |_| {})
                .await
        }
    });

    wait_for_queued_bash(&multi, agent_id.as_str()).await;
    for _ in 0..20 {
        assert_eq!(
            list_running_markers(&runtime_root),
            markers_before,
            "agent-queued bash must not spawn a guardian"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let cancelled = multi.cancel_agent(&agent_id).expect("cancel agent");
    assert_eq!(cancelled.state, AgentLifecycleState::Cancelled);

    let output = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .expect("child run should finish after agent cancel")
        .expect("join child run");
    assert_eq!(output.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(
        list_running_markers(&runtime_root),
        markers_before,
        "agent cancellation must not spawn queued bash"
    );

    stop_terminal(&registry, &hold_ctx, &handle).await;
    let probe = execute_model_bash_for_runtime(
        &hold_ctx,
        json!({ "command": printf_command("probe-agent") }),
    )
    .await
    .expect("probe bash after agent cancel");
    assert!(
        probe.content.contains("probe-agent"),
        "capacity must free for a fresh bash: {}",
        probe.content
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn swarm_cancellation_removes_all_queued_child_bash_without_spawning() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = capacity_one_runtime(&workspace);
    let runtime_root = runtime.runtime_root().to_path_buf();
    let hold_ctx = tool_context(&workspace, runtime.clone());
    let registry = ToolRegistry::with_builtin_tools();

    let handle = start_terminal_hold(&registry, &hold_ctx).await;
    wait_for_running_markers(&runtime_root, 1).await;
    let markers_before = list_running_markers(&runtime_root);
    assert_eq!(markers_before.len(), 1);

    let multi = MultiAgentRuntime::new();
    let swarm_id = multi.new_swarm_id();
    let child_a = multi.start_delegate(
        "swarm child a",
        Some("a"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    let child_b = multi.start_delegate(
        "swarm child b",
        Some("b"),
        AgentRole::Coder,
        AgentRunMode::Foreground,
        DelegateContext::None,
        AgentPathKind::SwarmChild(&swarm_id),
    );
    multi.register_swarm(SwarmSnapshot {
        swarm_id: swarm_id.clone(),
        description: "admission cancel swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: AgentLifecycleState::Running,
        max_concurrency: 2,
        aggregate: SwarmAggregate::from_states([
            AgentLifecycleState::Running,
            AgentLifecycleState::Running,
        ]),
        children: vec![
            SwarmChildSnapshot {
                item_index: 0,
                item: "a".to_owned(),
                agent: child_a.clone(),
            },
            SwarmChildSnapshot {
                item_index: 1,
                item: "b".to_owned(),
                agent: child_b.clone(),
            },
        ],
    });

    let harness_a = bash_tool_harness(&printf_command("should-not-run-a"));
    let harness_b = bash_tool_harness(&printf_command("should-not-run-b"));
    let deps_a = ChildRuntimeDeps::new(
        agent_config(&workspace, runtime.clone()),
        harness_a.client(),
        Arc::new(ToolRegistry::with_builtin_tools()),
    );
    let deps_b = ChildRuntimeDeps::new(
        agent_config(&workspace, runtime.clone()),
        harness_b.client(),
        Arc::new(ToolRegistry::with_builtin_tools()),
    );

    let run_a = tokio::spawn({
        let multi = multi.clone();
        let child = child_a.clone();
        let swarm_id = swarm_id.clone();
        async move {
            multi
                .run_started_swarm_child_turn(
                    deps_a,
                    child,
                    &swarm_id,
                    "a",
                    DelegateContext::None,
                    |_| {},
                )
                .await
        }
    });
    let run_b = tokio::spawn({
        let multi = multi.clone();
        let child = child_b.clone();
        let swarm_id = swarm_id.clone();
        async move {
            multi
                .run_started_swarm_child_turn(
                    deps_b,
                    child,
                    &swarm_id,
                    "b",
                    DelegateContext::None,
                    |_| {},
                )
                .await
        }
    });

    wait_for_queued_bash(&multi, child_a.id.as_str()).await;
    wait_for_queued_bash(&multi, child_b.id.as_str()).await;
    for _ in 0..20 {
        assert_eq!(
            list_running_markers(&runtime_root),
            markers_before,
            "swarm-queued bash children must not spawn guardians"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let cancelled = multi.cancel_swarm(&swarm_id).expect("cancel swarm");
    assert!(
        cancelled
            .children
            .iter()
            .all(|child| child.agent.state == AgentLifecycleState::Cancelled),
        "all swarm children should be cancelled: {cancelled:?}"
    );

    let out_a = tokio::time::timeout(Duration::from_secs(5), run_a)
        .await
        .expect("child a finishes")
        .expect("join a");
    let out_b = tokio::time::timeout(Duration::from_secs(5), run_b)
        .await
        .expect("child b finishes")
        .expect("join b");
    assert_eq!(out_a.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(out_b.snapshot.state, AgentLifecycleState::Cancelled);
    assert_eq!(
        list_running_markers(&runtime_root),
        markers_before,
        "swarm cancellation must not spawn queued bash"
    );

    stop_terminal(&registry, &hold_ctx, &handle).await;
    let probe = execute_model_bash_for_runtime(
        &hold_ctx,
        json!({ "command": printf_command("probe-swarm") }),
    )
    .await
    .expect("probe bash after swarm cancel");
    assert!(
        probe.content.contains("probe-swarm"),
        "capacity must free after swarm cancel: {}",
        probe.content
    );
}
