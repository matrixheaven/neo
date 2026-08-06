use std::time::Duration;

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentProgressSnapshot, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentTerminalReason, AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext,
    SwarmAggregate, SwarmChildProgress, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::session::{JsonlSessionReader, JsonlSessionWriter, ToolOutputStore};
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{
    AgentEvent, ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest,
    ApprovalResolution, PermissionOperation, QuestionEventData, QuestionOptionData,
    ShellCommandOrigin, ShellCommandOutcome, ToolResult,
};
use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Finalization, Line, strip_ansi, visible_width};
use neo_tui::shell::{StreamUpdate, ToolStatusKind};
use neo_tui::transcript::{
    BlockingEntryKind, DelegateCardComponent, DelegateGroupComponent, SwarmCardComponent,
    TranscriptEntry, WorkflowCardComponent,
};

fn snapshot(state: WorkflowState) -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId("wf-test".to_owned()),
        title: "Runtime audit and fix".to_owned(),
        state,
        current_phase: Some("verify".to_owned()),
        projection_sequence: Some(7),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(6_000),
        invocation_count: 3,
        failure_count: 1,
        actual_usage: Some(neo_agent_core::AgentTokenUsage {
            input_tokens: 20,
            output_tokens: 5,
            input_cache_read_tokens: 10,
            input_cache_write_tokens: 0,
        }),
        latest_log_summary: Some("focused verification running".to_owned()),
        latest_report_summary: Some("all scoped checks passed".to_owned()),
        terminal_reason: state
            .is_terminal()
            .then(|| "workflow reached its durable boundary".to_owned()),
        display_name: "Runtime audit and fix".to_owned(),
        purpose: "Verify runtime correctness".to_owned(),
    }
}

fn text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn origin(run_id: &str, invocation_id: &str) -> WorkflowExecutionOrigin {
    WorkflowExecutionOrigin {
        run_id: WorkflowId(run_id.to_owned()),
        human_handle: None,
        definition_name: "test-workflow".to_owned(),
        definition_revision: None,
        phase_id: Some("verify".to_owned()),
        invocation_id: Some(invocation_id.to_owned()),
        swarm_item_id: None,
    }
}

fn agent_snapshot(id: &str) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(id);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "verify workflow".to_owned(),
        task_title: "verify workflow".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn agent_progress(agent: &AgentSnapshot, state: AgentLifecycleState) -> AgentProgressSnapshot {
    AgentProgressSnapshot {
        agent_id: agent.id.clone(),
        state,
        mode: agent.mode,
        detached_from_foreground: false,
        started_at_ms: agent.started_at_ms,
        updated_at_ms: 3,
        terminal_at_ms: state.is_terminal().then_some(3),
        terminal_reason: None,
        run_count: agent.run_count,
        live_messages_received: 1,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed_ms: 2,
        latest_text: Some("progress merged".to_owned()),
        last_tool: None,
        outcome: None,
    }
}

fn swarm_snapshot(id: &str, agent: AgentSnapshot) -> SwarmSnapshot {
    let children = vec![SwarmChildSnapshot {
        item_index: 0,
        item: "verify item".to_owned(),
        agent,
    }];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: id.to_owned(),
        description: "workflow swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 1,
        aggregate,
        children,
    }
}

fn tool_result(content: &str, is_error: bool) -> ToolResult {
    ToolResult {
        content: content.to_owned(),
        is_error,
        details: None,
        terminate: false,
    }
}

fn replayed_reference() -> neo_agent_core::session::ToolOutputRef {
    neo_agent_core::session::ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "bash-replay-artifact".to_owned(),
        byte_len: 8192,
        line_count: 24,
        complete: true,
    }
}

fn terminal_text(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn workflow_tool_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> WorkflowExecutionOrigin {
    let tool_origin = origin("wf-test", id);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: id.to_owned(),
        name: name.to_owned(),
        arguments,
        workflow_origin: Some(tool_origin.clone()),
        output_ref: None,
    });
    tool_origin
}

fn workflow_delegate_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    invocation_id: &str,
    agent: AgentSnapshot,
) {
    let tool_origin = workflow_tool_started(
        pane,
        invocation_id,
        "Delegate",
        serde_json::json!({"task": agent.task.clone()}),
    );
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent,
        workflow_origin: Some(tool_origin),
    });
}

fn workflow_swarm_started(
    pane: &mut neo_tui::transcript::TranscriptPane,
    invocation_id: &str,
    swarm: SwarmSnapshot,
) {
    let tool_origin = workflow_tool_started(
        pane,
        invocation_id,
        "DelegateSwarm",
        serde_json::json!({"description": swarm.description.clone()}),
    );
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm,
        workflow_origin: Some(tool_origin),
    });
}

#[test]
fn workflow_card_projects_runtime_summary() {
    let rendered = text(&WorkflowCardComponent::new(snapshot(WorkflowState::Running)).render(120));

    assert!(rendered.contains("Runtime audit and fix"), "{rendered}");
    assert!(rendered.contains("phase verify"), "{rendered}");
    assert!(rendered.contains("3 invocations"), "{rendered}");
    assert!(rendered.contains("25 tokens"), "{rendered}");
    assert!(
        rendered.contains("focused verification running"),
        "{rendered}"
    );
    assert!(rendered.contains("TaskPause · TaskStop"), "{rendered}");
    assert!(
        rendered.find("phase verify") < rendered.find("TaskPause · TaskStop"),
        "workflow context must precede controls:\n{rendered}"
    );
}

#[test]
fn workflow_card_renders_paused_resource_limited_and_terminal_states() {
    for (state, label, controls) in [
        (
            WorkflowState::Paused,
            "paused",
            Some("TaskResume · TaskStop"),
        ),
        (WorkflowState::Completed, "completed", None),
        (WorkflowState::Failed, "failed", None),
        (WorkflowState::Cancelled, "cancelled", None),
        (WorkflowState::ResourceLimited, "resource limited", None),
    ] {
        let mut card = WorkflowCardComponent::new(snapshot(state));
        let rendered = text(&card.render_with_theme(120, &Default::default()));
        assert!(rendered.contains(label), "{state:?}: {rendered}");
        let expected_finalization = if state == WorkflowState::Paused {
            Finalization::Live
        } else {
            Finalization::Finalized
        };
        assert_eq!(card.finalization(), expected_finalization);
        assert!(!card.on_render_tick(10_000), "{state:?} elapsed is frozen");
        match controls {
            Some(controls) => assert!(rendered.contains(controls), "{rendered}"),
            None => assert!(!rendered.contains("Controls"), "{rendered}"),
        }
    }
    let mut running = WorkflowCardComponent::new(snapshot(WorkflowState::Running));
    assert!(running.on_render_tick(10_000));
    assert!(text(&running.render(120)).contains("9s"));
}

#[test]
fn workflow_main_card_truncates_long_content_without_leaking_raw_output() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(48, 30);
    let mut workflow = snapshot(WorkflowState::Running);
    workflow.title = "宽字符工作流标题 with a deliberately long suffix".to_owned();
    workflow.latest_report_summary =
        Some("报告包含很长的路径 /tmp/a/very/long/path/that/must/not/overflow".to_owned());
    pane.apply_agent_event(AgentEvent::WorkflowStarted { turn: 1, workflow });

    workflow_tool_started(
        &mut pane,
        "running-bash",
        "Bash",
        serde_json::json!({
            "command": "cargo test --package neo-tui --test workflow_transcript a_very_long_test_name -- --exact --nocapture"
        }),
    );
    let failed_origin = workflow_tool_started(
        &mut pane,
        "failed-edit",
        "Edit",
        serde_json::json!({"path": "/tmp/a/very/long/path/failed.rs"}),
    );
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "failed-edit".to_owned(),
        name: "Edit".to_owned(),
        result: tool_result("FULL_TOOL_OUTPUT must stay out of the normal card", true),
        workflow_origin: Some(failed_origin),
        output_ref: None,
    });
    for index in 0..6 {
        let id = format!("completed-read-{index}");
        let read_origin = workflow_tool_started(
            &mut pane,
            &id,
            "Read",
            serde_json::json!({
                "path": format!("/tmp/a/very/long/path/{index}/宽字符-report.md")
            }),
        );
        pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id,
            name: "Read".to_owned(),
            result: tool_result("FULL_TOOL_OUTPUT must stay out of the normal card", false),
            workflow_origin: Some(read_origin),
            output_ref: None,
        });
    }

    let slice = pane.render_visible_slice(48, 30);
    let live = terminal_text(&slice);
    assert!(slice.len() <= 30, "slice rows:\n{live}");
    assert!(
        slice.iter().all(|line| visible_width(line) <= 48),
        "slice width:\n{live}"
    );
    assert!(live.contains("Bash"), "active tool missing:\n{live}");
    assert!(live.contains("Report"), "latest report missing:\n{live}");
    assert!(live.contains('…'), "long content must be explicit:\n{live}");
    assert!(
        !live.contains("FULL_TOOL_OUTPUT"),
        "raw output leaked:\n{live}"
    );
}

#[test]
fn workflow_child_summaries_use_two_sibling_cards_and_one_row_per_agent() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });

    let mut euclid = agent_snapshot("delegate-euclid");
    euclid.display_name = AgentDisplayName::new("Euclid");
    euclid.role = AgentRole::Explorer;
    euclid.elapsed = Duration::from_secs(4);
    euclid.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-report".to_owned(),
            name: "Read".to_owned(),
            summary: Some("report.md".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    });
    workflow_delegate_started(&mut pane, "delegate-euclid-call", euclid);

    let mut hypatia = agent_snapshot("delegate-hypatia");
    hypatia.display_name = AgentDisplayName::new("Hypatia");
    hypatia.role = AgentRole::Reviewer;
    hypatia.state = AgentLifecycleState::Completed;
    hypatia.terminal_at_ms = Some(4_000);
    hypatia.elapsed = Duration::from_secs(3);
    hypatia.outcome = Some(AgentTerminalOutcome {
        summary: "review completed".to_owned(),
        is_error: false,
    });
    workflow_delegate_started(&mut pane, "delegate-hypatia-call", hypatia);

    let mut alpha = agent_snapshot("swarm-alpha");
    alpha.display_name = AgentDisplayName::new("Alpha");
    alpha.role = AgentRole::Coder;
    alpha.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "cargo-test".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo test --package neo-tui".to_owned()),
            phase: AgentToolActivityPhase::Ongoing,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    });
    let mut beta = agent_snapshot("swarm-beta");
    beta.display_name = AgentDisplayName::new("Beta");
    beta.role = AgentRole::Reviewer;
    beta.state = AgentLifecycleState::Queued;
    beta.started_at_ms = None;
    let children = vec![
        SwarmChildSnapshot {
            item_index: 1,
            item: "beta item".to_owned(),
            agent: beta,
        },
        SwarmChildSnapshot {
            item_index: 0,
            item: "alpha item".to_owned(),
            agent: alpha,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    workflow_swarm_started(
        &mut pane,
        "swarm-call",
        SwarmSnapshot {
            swarm_id: "swarm-security-review".to_owned(),
            description: "security review".to_owned(),
            role: AgentRole::Reviewer,
            mode: AgentRunMode::Foreground,
            state: aggregate.status(),
            max_concurrency: 2,
            aggregate,
            children,
        },
    );

    let slice = pane.render_visible_slice(120, 30);
    let live = terminal_text(&slice);
    let main = live
        .find("Workflow  Runtime audit and fix")
        .expect("main card");
    let delegates = live.find("Workflow Delegates").expect("delegate summary");
    let swarms = live.find("Workflow Swarms").expect("swarm summary");
    assert!(
        main < delegates && delegates < swarms,
        "sibling order:\n{live}"
    );
    for name in ["Euclid", "Hypatia", "Alpha", "Beta"] {
        assert_eq!(live.matches(name).count(), 1, "one row for {name}:\n{live}");
    }
    assert!(
        live.find("Alpha") < live.find("Beta"),
        "swarm rows must stay in item order:\n{live}"
    );
    assert!(live.contains("[Explorer]"), "delegate role:\n{live}");
    assert!(live.contains("[Reviewer]"), "reviewer role:\n{live}");
    assert!(live.contains("security review"), "swarm label:\n{live}");
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. }
            | TranscriptEntry::DelegateGroup { .. }
            | TranscriptEntry::DelegateSwarm { .. }
    )));
}

/// Frozen-fixture snapshot builders for the ordinary Delegate-family cards.
///
/// These mirror the option-b fixtures in `multi_agent_transcript.rs`: the
/// fixtures must render the exact same rows, so the ordinary cards stay
/// byte-identical across the Workflow outer-rendering change.
fn frozen_delegate_terminal_reason(state: AgentLifecycleState) -> Option<AgentTerminalReason> {
    match state {
        AgentLifecycleState::Queued | AgentLifecycleState::Running => None,
        AgentLifecycleState::Completed => Some(AgentTerminalReason::Completed),
        AgentLifecycleState::Failed => Some(AgentTerminalReason::Error),
        AgentLifecycleState::Cancelled => Some(AgentTerminalReason::CancelledByUser),
        AgentLifecycleState::TimedOut => Some(AgentTerminalReason::TimedOut),
        AgentLifecycleState::Interrupted => Some(AgentTerminalReason::ProcessExited),
    }
}

fn frozen_delegate_option_b(
    id_suffix: &str,
    name: &str,
    role: AgentRole,
    state: AgentLifecycleState,
    title: &str,
) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(name);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id_suffix),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state,
        task: format!("{title}\n\nFull prompt that must not replace the display name."),
        task_title: title.to_owned(),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        started_at_ms: matches!(state, AgentLifecycleState::Running).then_some(1_000),
        terminal_at_ms: state.is_terminal().then_some(31_000),
        detached_from_foreground: false,
        terminal_reason: frozen_delegate_terminal_reason(state),
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}

fn frozen_running_delegate() -> AgentSnapshot {
    let mut snapshot = frozen_delegate_option_b(
        "nova",
        "Nova",
        AgentRole::Coder,
        AgentLifecycleState::Running,
        "角色对比测试 coder",
    );
    snapshot.tool_count = 3;
    snapshot.token_count = 22_700;
    snapshot.elapsed = Duration::from_secs(21);
    snapshot.activity = vec![
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-delegate".to_owned(),
                name: "Read".to_owned(),
                summary: Some("crates/neo-agent-core/src/tools/delegate.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "bash-nextest".to_owned(),
                name: "Bash".to_owned(),
                summary: Some("cargo nextest run -p neo-agent-core ...".to_owned()),
                phase: AgentToolActivityPhase::Ongoing,
                output: Some(AgentToolOutputPreview {
                    text: "running: cargo nextest run -p neo-agent-core ...\nCompiling neo-agent-core v0.1.0"
                        .to_owned(),
                    is_error: false,
                    truncated: true,
                    tail: true,
                }),
                files: Vec::new(),
                output_ref: None,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "Let me verify the state mutation path before editing.".to_owned(),
                thinking: true,
            },
        },
        AgentActivityEntry {
            kind: AgentActivityKind::Text {
                text: "I found the foreground aggregation issue. Next I will make the renderer change."
                    .to_owned(),
                thinking: false,
            },
        },
    ];
    snapshot.latest_text = Some(
        "I found the foreground aggregation issue. Next I will make the renderer change."
            .to_owned(),
    );
    snapshot
}

#[test]
fn non_workflow_delegate_family_cards_remain_unchanged() {
    // Frozen output fixtures: ordinary Delegate-family cards render exactly
    // through their existing components. The rows below are the frozen
    // component output; any change to these cards' rendered rows is a
    // compatibility break.
    let theme = TuiTheme::default();

    let delegate = DelegateCardComponent::new(frozen_running_delegate());
    let delegate_rows = frozen_rows(delegate.render_with_theme(160, &theme));
    assert_eq!(
        delegate_rows,
        vec![
            "● Nova  [Coder] · Delegate · 角色对比测试 coder · running · 3 tools · 21s · 22.7k tok",
            "│ agent_nova",
            "  Press Ctrl+B to run in background",
            "  • Used Read (crates/neo-agent-core/src/tools/delegate.rs)",
            "  • Using Bash (cargo nextest run -p neo-agent-core ...)",
            "      running: cargo nextest run -p neo-agent-core ...",
            "      Compiling neo-agent-core v0.1.0",
            "  ◌ thinking",
            "    Let me verify the state mutation path before editing.",
            "  │ I found the foreground aggregation issue. Next I will make the renderer change.",
        ],
        "ordinary Delegate card fixture"
    );

    let mut nova = frozen_running_delegate();
    nova.state = AgentLifecycleState::Completed;
    nova.terminal_at_ms = Some(31_000);
    nova.terminal_reason = Some(AgentTerminalReason::Completed);
    nova.outcome = Some(AgentTerminalOutcome {
        summary: "All edits applied.".to_owned(),
        is_error: false,
    });
    let vega = frozen_delegate_option_b(
        "vega",
        "Vega",
        AgentRole::Explorer,
        AgentLifecycleState::Queued,
        "queued task",
    );
    let group = DelegateGroupComponent::new(1, vec![nova, vega]);
    let group_rows = frozen_rows(group.render_with_theme(160, &theme));
    assert_eq!(
        group_rows,
        vec![
            "● Delegate group · Running 2 agents (1 waiting) · 21s",
            "  ├─ Nova  [Coder]  角色对比测试 coder · 3 tools · 21s · 22.7k tok",
            "  │      • Used Read (crates/neo-agent-core/src/tools/delegate.rs)",
            "  │      • Using Bash (cargo nextest run -p neo-agent-core ...)",
            "  │          running: cargo nextest run -p neo-agent-core ...",
            "  │          Compiling neo-agent-core v0.1.0",
            "  │      ◌ thinking",
            "  │        Let me verify the state mutation path before editing.",
            "  │      │ I found the foreground aggregation issue. Next I will make the renderer change.",
            "  │      └ All edits applied.",
            "  └─ Vega  [Explorer]  queued task",
            "         ◌ Waiting for scheduler slot",
        ],
        "DelegateGroup card fixture"
    );

    let mut iris = frozen_delegate_option_b(
        "iris",
        "Iris",
        AgentRole::Planner,
        AgentLifecycleState::Completed,
        "planner item",
    );
    iris.tool_count = 3;
    iris.token_count = 8_200;
    iris.elapsed = Duration::from_secs(12);
    iris.terminal_at_ms = Some(12_000);
    iris.terminal_reason = Some(AgentTerminalReason::Completed);
    iris.outcome = Some(AgentTerminalOutcome {
        summary: "Plan is ready".to_owned(),
        is_error: false,
    });
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "coder item".to_owned(),
            agent: frozen_running_delegate(),
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "planner item".to_owned(),
            agent: iris,
        },
        SwarmChildSnapshot {
            item_index: 2,
            item: "explorer item".to_owned(),
            agent: frozen_delegate_option_b(
                "vega",
                "Vega",
                AgentRole::Explorer,
                AgentLifecycleState::Running,
                "搜索历史卡片回归点",
            ),
        },
        SwarmChildSnapshot {
            item_index: 3,
            item: "queued item".to_owned(),
            agent: frozen_delegate_option_b(
                "rune",
                "Rune",
                AgentRole::Coder,
                AgentLifecycleState::Queued,
                "queued renderer task",
            ),
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let swarm_snapshot = SwarmSnapshot {
        swarm_id: "option-b-swarm".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let collapsed_rows =
        frozen_rows(SwarmCardComponent::new(swarm_snapshot).render_with_theme(160, &theme));
    assert_eq!(
        collapsed_rows,
        vec![
            "● DelegateSwarm · running · 角色对比测试 · 4 agents · 2 run · 1 done · 1 wait · progress [■■■■■·············] 25% · max 2",
            "│ option-b-swarm",
            "├─ Nova  [Coder] ● [■·······]  running · 3 tools · 21s · 22.7k tok · Using Bash (cargo nextest run -p neo-agent-core ...)",
            "├─ Iris  [Planner] ✓ [■■■■■■■■]  done · 3 tools · 12s · 8.2k tok · Plan is ready",
            "├─ Vega  [Explorer] ● [········]  running · 0 tools · 0s · 0 tok · 搜索历史卡片回归点",
            "└─ Rune  [Coder] ◌ [········]  queued · 0 tools · 0s · 0 tok · queued renderer task",
            "Scheduling: 2/4 running · max concurrency 2 · 1 queued",
            "",
            "● Working... 25% ━━━━━━━━┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄",
        ],
        "collapsed DelegateSwarm fixture"
    );

    let mut nova = frozen_running_delegate();
    nova.activity.push(AgentActivityEntry {
        kind: AgentActivityKind::Text {
            text: "All edits applied. Now let me verify the paths.".to_owned(),
            thinking: false,
        },
    });
    let mut iris = frozen_delegate_option_b(
        "iris-expanded",
        "Iris",
        AgentRole::Planner,
        AgentLifecycleState::Completed,
        "Plan renderer work",
    );
    iris.tool_count = 2;
    iris.token_count = 8_200;
    iris.elapsed = Duration::from_secs(12);
    iris.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "read-plan".to_owned(),
            name: "Read".to_owned(),
            summary: Some("docs/aegis/plans/...".to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }];
    iris.outcome = Some(AgentTerminalOutcome {
        summary: "The implementation should stay inside transcript cards.".to_owned(),
        is_error: false,
    });
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "nova".to_owned(),
            agent: nova,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "iris".to_owned(),
            agent: iris,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    let expanded_snapshot = SwarmSnapshot {
        swarm_id: "option-b-expanded".to_owned(),
        description: "角色对比测试".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    };
    let mut expanded_card = SwarmCardComponent::new(expanded_snapshot);
    expanded_card.set_expanded(true);
    let expanded_rows = frozen_rows(expanded_card.render_with_theme(160, &theme));
    assert_eq!(
        expanded_rows,
        vec![
            "● DelegateSwarm · running · 角色对比测试 · 2 agents · 1 run · 1 done · 0 wait · progress [■■■■■■■■■·········] 51% · max 2",
            "│ option-b-expanded",
            "├─ Nova  [Coder] ● [■·······]  running · 3 tools · 21s · 22.7k tok · Using Bash (cargo nextest run -p neo-agent-core ...)",
            "└─ Iris  [Planner] ✓ [■■■■■■■■]  done · 2 tools · 12s · 8.2k tok · The implementation should stay inside transcript cards.",
            "Scheduling: 1/2 running · max concurrency 2 · 0 queued",
            "",
            "● Working... 51% ━━━━━━━━━━━━━━━┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄",
            "  ├─ Nova  [Coder]  running · 21s · 3 tools · 22.7k tok",
            "  │   • Used Read (crates/neo-agent-core/src/tools/delegate.rs)",
            "  │   • Using Bash (cargo nextest run -p neo-agent-core ...)",
            "  │       running: cargo nextest run -p neo-agent-core ...",
            "  │       Compiling neo-agent-core v0.1.0",
            "  │   ◌ thinking",
            "  │     Let me verify the state mutation path before editing.",
            "  │   │ All edits applied. Now let me verify the paths.",
            "  └─ Iris  [Planner]  done · 12s · 2 tools · 8.2k tok",
            "      • Used Read (docs/aegis/plans/...)",
            "      └ The implementation should stay inside transcript cards.",
        ],
        "expanded DelegateSwarm fixture"
    );
}

fn frozen_rows(lines: Vec<Line>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect()
}

#[test]
fn workflow_updates_stay_in_one_entry_without_duplicate_cards() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);

    let mut running_a = snapshot(WorkflowState::Running);
    running_a.projection_sequence = Some(1);
    running_a.current_phase = Some("verify".to_owned());
    pane.transcript_mut().upsert_workflow(running_a);
    let slice = pane.render_visible_slice(120, 24);
    assert!(
        terminal_text(&slice).contains("Runtime audit and fix"),
        "the workflow card renders in the document"
    );
    let mut running_b = snapshot(WorkflowState::Running);
    running_b.projection_sequence = Some(2);
    running_b.current_phase = Some("build".to_owned());
    pane.transcript_mut().upsert_workflow(running_b);
    let slice = pane.render_visible_slice(120, 24);
    let live = terminal_text(&slice);
    assert!(live.contains("phase build"), "slice:\n{live}");
    assert!(!live.contains("phase verify"), "slice:\n{live}");

    let mut completed = snapshot(WorkflowState::Completed);
    completed.projection_sequence = Some(9);
    completed.updated_at_ms = Some(9_000);
    pane.transcript_mut().upsert_workflow(completed);
    let slice = pane.render_visible_slice(120, 24);
    let live = terminal_text(&slice);
    assert!(live.contains("completed"), "slice:\n{live}");
    assert_eq!(
        live.matches("Workflow").count(),
        1,
        "one terminal status, no duplicate card:\n{live}"
    );
}

#[test]
fn workflow_origin_routes_tools_and_children_into_one_entry() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });

    let read_origin = origin("wf-test", "read-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "README.md"}),
        workflow_origin: Some(read_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "read-call".to_owned(),
        position: 2,
        waiting_ms: 10,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "README.md"}),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        partial_result: tool_result("partial", false),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        result: tool_result("done", false),
        workflow_origin: Some(read_origin.clone()),
        output_ref: None,
    });

    let bash_origin = origin("wf-test", "bash-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-call".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf ok"}),
        workflow_origin: Some(bash_origin),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "bash-call".to_owned(),
        command: "printf ok".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "bash-call".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    let delegate_origin = origin("wf-test", "delegate-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "delegate-call".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "verify"}),
        workflow_origin: Some(delegate_origin.clone()),
        output_ref: None,
    });
    let delegate = agent_snapshot("delegate");
    pane.apply_agent_event(AgentEvent::DelegateStarted {
        turn: 1,
        agent: delegate.clone(),
        workflow_origin: Some(delegate_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 1,
        progress: agent_progress(&delegate, AgentLifecycleState::Running),
        workflow_origin: Some(delegate_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "delegate-call".to_owned(),
        name: "Delegate".to_owned(),
        result: tool_result("delegated", false),
        workflow_origin: Some(delegate_origin),
        output_ref: None,
    });

    let swarm_origin = origin("wf-test", "swarm-call");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "swarm-call".to_owned(),
        name: "DelegateSwarm".to_owned(),
        arguments: serde_json::json!({"tasks": ["verify"]}),
        workflow_origin: Some(swarm_origin.clone()),
        output_ref: None,
    });
    let swarm_agent = agent_snapshot("swarm-child");
    let swarm = swarm_snapshot("swarm", swarm_agent.clone());
    pane.apply_agent_event(AgentEvent::DelegateSwarmStarted {
        turn: 1,
        swarm: swarm.clone(),
        workflow_origin: Some(swarm_origin.clone()),
    });
    let child_progress = SwarmChildProgress {
        item_index: 0,
        progress: agent_progress(&swarm_agent, AgentLifecycleState::Completed),
    };
    let aggregate = SwarmAggregate::from_states([AgentLifecycleState::Completed]);
    pane.apply_agent_event(AgentEvent::DelegateSwarmProgressUpdated {
        turn: 1,
        swarm_id: swarm.swarm_id.clone(),
        state: AgentLifecycleState::Completed,
        aggregate,
        child_progress,
        workflow_origin: Some(swarm_origin.clone()),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "swarm-call".to_owned(),
        name: "DelegateSwarm".to_owned(),
        result: tool_result("swarmed", false),
        workflow_origin: Some(swarm_origin),
        output_ref: None,
    });

    let failed_origin = origin("wf-test", "failed-delegate");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        arguments: serde_json::json!({"task": "never started"}),
        workflow_origin: Some(failed_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "failed-delegate".to_owned(),
        name: "Delegate".to_owned(),
        result: tool_result("failed before child start", true),
        workflow_origin: Some(failed_origin),
        output_ref: None,
    });

    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(workflow.direct_tools().len(), 3);
    assert_eq!(workflow.direct_tools()[0].id(), "read-call");
    assert_eq!(
        workflow.direct_tools()[0].status(),
        ToolStatusKind::Succeeded
    );
    assert_eq!(
        workflow.direct_tools()[0].workflow_origin(),
        Some(&read_origin)
    );
    assert_eq!(workflow.direct_tools()[1].id(), "bash-call");
    assert_eq!(
        workflow.direct_tools()[1].status(),
        ToolStatusKind::Succeeded
    );
    assert_eq!(workflow.direct_tools()[2].id(), "failed-delegate");
    assert_eq!(workflow.direct_tools()[2].status(), ToolStatusKind::Failed);
    assert_eq!(workflow.delegates().len(), 1);
    assert_eq!(
        workflow.delegates()[0].latest_text.as_deref(),
        Some("progress merged")
    );
    assert_eq!(workflow.swarms().len(), 1);
    assert_eq!(
        workflow.swarms()[0].children[0].agent.state,
        AgentLifecycleState::Completed
    );
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. } | TranscriptEntry::DelegateSwarm { .. }
    )));

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "other"}),
        workflow_origin: Some(origin("other-run", "other-invocation")),
        output_ref: None,
    });
    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(
        workflow.direct_tools()[0].workflow_origin(),
        Some(&read_origin)
    );

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "ordinary-call".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "ordinary"}),
        workflow_origin: None,
        output_ref: None,
    });
    assert!(pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "ordinary-call")
    ));

    let mut missing = neo_tui::transcript::TranscriptPane::new(120, 24);
    let missing_origin = origin("internal-run-id", "internal-invocation-id");
    for id in ["orphan-one", "orphan-two"] {
        missing.apply_agent_event(AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: id.to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": "missing"}),
            workflow_origin: Some(missing_origin.clone()),
        });
    }
    assert_eq!(missing.transcript().entries().len(), 1);
    let TranscriptEntry::Status { text, .. } = &missing.transcript().entries()[0] else {
        panic!("bounded presentation error")
    };
    assert!(!text.contains("internal-run-id"), "{text}");
    assert!(!text.contains("internal-invocation-id"), "{text}");
}

#[test]
fn successful_workflow_launch_replaces_the_generic_tool_card() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "workflow-launch".to_owned(),
        name: "Workflow".to_owned(),
        arguments: serde_json::json!({"action": "run_saved", "name": "review"}),
        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "workflow-launch".to_owned(),
        name: "Workflow".to_owned(),
        result: ToolResult {
            content: "started".to_owned(),
            is_error: false,
            details: Some(serde_json::json!({
                "action": "run_saved",
                "status": "started",
                "task": {
                    "task_id": "wf-test",
                    "kind": "workflow",
                    "status": "started",
                    "display_name": "Runtime audit and fix"
                }
            })),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    });

    assert!(pane.transcript().is_tool_run_suppressed("workflow-launch"));
    let slice = pane.render_visible_slice(120, 24);
    let rendered = terminal_text(&slice);
    assert!(!rendered.contains("Used Workflow"), "{rendered}");
    assert_eq!(rendered.matches("Workflow").count(), 1, "{rendered}");

    let mut failed = neo_tui::transcript::TranscriptPane::new(120, 24);
    failed.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "workflow-preflight-failure".to_owned(),
        name: "Workflow".to_owned(),
        arguments: serde_json::json!({"action": "run_saved", "name": "missing"}),
        workflow_origin: None,
        output_ref: None,
    });
    failed.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "workflow-preflight-failure".to_owned(),
        name: "Workflow".to_owned(),
        result: ToolResult {
            content: "workflow not found".to_owned(),
            is_error: true,
            details: Some(serde_json::json!({
                "action": "run_saved",
                "status": "failed",
                "error": {"message": "workflow not found"}
            })),
            terminate: false,
        },
        workflow_origin: None,
        output_ref: None,
    });
    assert!(
        !failed
            .transcript()
            .is_tool_run_suppressed("workflow-preflight-failure")
    );
}

#[tokio::test]
async fn jsonl_replay_preserves_workflow_question_tool_and_child_grouping() {
    let delegate_origin = origin("wf-test", "delegate-replay-call");
    let swarm_origin = origin("wf-test", "swarm-replay-call");
    let bash_origin = origin("wf-test", "bash-replay-call");
    let question_origin = origin("wf-test", "question-replay-call");
    let delegate = agent_snapshot("delegate-replay");
    let swarm = swarm_snapshot("swarm-replay", agent_snapshot("swarm-child-replay"));
    let events = vec![
        AgentEvent::WorkflowStarted {
            turn: 1,
            workflow: snapshot(WorkflowState::Running),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "bash-replay-call".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({"command": "printf replay"}),
            workflow_origin: Some(bash_origin.clone()),
            output_ref: Some(replayed_reference()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "bash-replay-call".to_owned(),
            name: "Bash".to_owned(),
            result: tool_result("replayed", false),
            workflow_origin: Some(bash_origin),
            output_ref: Some(replayed_reference()),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "delegate-replay-call".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({"task": "delegate replay"}),
            workflow_origin: Some(delegate_origin.clone()),
            output_ref: None,
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: delegate,
            workflow_origin: Some(delegate_origin),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "swarm-replay-call".to_owned(),
            name: "DelegateSwarm".to_owned(),
            arguments: serde_json::json!({"tasks": ["swarm replay"]}),
            workflow_origin: Some(swarm_origin.clone()),
            output_ref: None,
        },
        AgentEvent::DelegateSwarmStarted {
            turn: 1,
            swarm,
            workflow_origin: Some(swarm_origin),
        },
        AgentEvent::QuestionRequested {
            turn: 1,
            id: "question-replay".to_owned(),
            questions: vec![QuestionEventData {
                question: "Continue replay?".to_owned(),
                header: None,
                body: None,
                options: vec![QuestionOptionData {
                    label: "Continue".to_owned(),
                    description: None,
                }],
                multi_select: false,
            }],
            workflow_origin: Some(question_origin.clone()),
        },
    ];

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("main.jsonl");
    let mut writer = JsonlSessionWriter::create(&path).await.expect("writer");
    for event in &events {
        writer.append(event).await.expect("append event");
    }
    writer.flush().await.expect("flush");
    let replayed = JsonlSessionReader::read_all(&path)
        .await
        .expect("read events");

    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    for event in replayed {
        match event {
            AgentEvent::QuestionRequested {
                id,
                questions,
                workflow_origin,
                ..
            } => {
                let questions = questions
                    .into_iter()
                    .map(|question| QuestionDisplayData {
                        question: question.question,
                        header: question.header,
                        body: question.body,
                        options: question
                            .options
                            .into_iter()
                            .map(|option| QuestionDisplayOption {
                                label: option.label,
                                description: option.description,
                            })
                            .collect(),
                        multi_select: question.multi_select,
                    })
                    .collect();
                pane.apply_question_stream_update(StreamUpdate::QuestionRequested {
                    id,
                    questions,
                    workflow_origin,
                });
            }
            event => pane.apply_agent_event(event),
        }
    }

    let workflow = pane
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow entry");
    assert_eq!(workflow.direct_tools().len(), 1);
    assert_eq!(workflow.direct_tools()[0].id(), "bash-replay-call");
    assert_eq!(
        workflow.direct_tools()[0].output_ref(),
        Some(&replayed_reference()),
        "the typed reference must rehydrate onto the Workflow direct tool from JSONL"
    );
    assert_eq!(
        workflow.delegates()[0].display_name.as_str(),
        "delegate-replay"
    );
    assert_eq!(workflow.swarms()[0].swarm_id, "swarm-replay");
    assert!(!pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Delegate { .. } | TranscriptEntry::DelegateSwarm { .. }
    )));
    assert!(pane.transcript().entries().iter().any(|entry| matches!(
        entry,
        TranscriptEntry::QuestionPrompt(data)
            if data.workflow_origin.as_ref() == Some(&question_origin)
    )));
}

#[test]
fn workflow_origin_conflict_is_one_terminal_error_and_blocks_id_only_updates() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    let original_origin = origin("wf-test", "original-invocation");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf original"}),
        workflow_origin: Some(original_origin.clone()),
        output_ref: None,
    });
    let conflicting_origin = origin("wf-test", "conflicting-invocation");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf conflicting"}),
        workflow_origin: Some(conflicting_origin.clone()),
        output_ref: None,
    });

    let workflow_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::Workflow { .. }))
        .expect("workflow entry");
    let revision_after_error = pane.transcript().entry_revisions()[workflow_index];
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    let tool = &component.direct_tools()[0];
    let route_error = tool.result().expect("route error").to_owned();
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.workflow_origin(), Some(&original_origin));
    assert!(route_error.len() < 100, "{route_error}");
    assert!(!route_error.contains("wf-test"), "{route_error}");
    assert!(
        !route_error.contains("original-invocation"),
        "{route_error}"
    );
    assert!(
        !route_error.contains("conflicting-invocation"),
        "{route_error}"
    );

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf conflicting"}),
        workflow_origin: Some(conflicting_origin),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "shared-tool".to_owned(),
        position: 1,
        waiting_ms: 50,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "shared-tool".to_owned(),
        command: "printf later".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shared-tool".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "later".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    assert_eq!(
        pane.transcript().entry_revisions()[workflow_index],
        revision_after_error
    );
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    assert_eq!(component.direct_tools().len(), 1);
    let tool = &component.direct_tools()[0];
    assert_eq!(tool.status(), ToolStatusKind::Failed);
    assert_eq!(tool.result(), Some(route_error.as_str()));
    assert_eq!(tool.workflow_origin(), Some(&original_origin));
    assert!(!pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "shared-tool")
    ));
}

#[test]
fn orphan_model_shell_events_do_not_create_top_level_tools() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "orphan-shell".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf orphan"}),
        workflow_origin: Some(origin("missing-workflow", "orphan-shell")),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "orphan-shell".to_owned(),
        command: "printf orphan".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "orphan-shell".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "orphan".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: None,
    });

    assert_eq!(pane.transcript().entries().len(), 1);
    assert!(matches!(
        &pane.transcript().entries()[0],
        TranscriptEntry::Status { text, .. }
            if text == "Workflow activity could not be displayed because the workflow has not started."
    ));
    assert!(!pane.transcript().entries().iter().any(
        |entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "orphan-shell")
    ));
}

#[test]
fn finalized_workflow_tool_rejects_late_updates() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    let tool_origin = origin("wf-test", "terminal-tool");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "result"}),
        workflow_origin: Some(tool_origin.clone()),
        output_ref: None,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        result: tool_result("final", false),
        workflow_origin: Some(tool_origin.clone()),
        output_ref: None,
    });
    let workflow_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::Workflow { .. }))
        .expect("workflow entry");
    let revision = pane.transcript().entry_revisions()[workflow_index];

    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "late-start"}),
        workflow_origin: Some(tool_origin.clone()),
        output_ref: None,
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        partial_result: tool_result("late", false),
        workflow_origin: Some(tool_origin.clone()),
        output_ref: None,
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "terminal-tool".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "late-queue"}),
        workflow_origin: Some(tool_origin),
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);

    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "terminal-tool".to_owned(),
        position: 3,
        waiting_ms: 50,
    });
    assert_finalized_workflow_tool(&pane, workflow_index, revision);
}

#[test]
fn workflow_group_commits_once_at_terminal_event_position() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    let mut running = snapshot(WorkflowState::Running);
    running.projection_sequence = Some(1);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: running,
    });

    let mut delegate = agent_snapshot("terminal-delegate");
    delegate.display_name = AgentDisplayName::new("TerminalDelegate");
    delegate.state = AgentLifecycleState::Completed;
    delegate.terminal_at_ms = Some(3_000);
    delegate.outcome = Some(AgentTerminalOutcome {
        summary: "delegate done".to_owned(),
        is_error: false,
    });
    workflow_delegate_started(&mut pane, "terminal-delegate-call", delegate);
    let mut swarm_child = agent_snapshot("terminal-swarm-child");
    swarm_child.display_name = AgentDisplayName::new("TerminalSwarmChild");
    swarm_child.state = AgentLifecycleState::Completed;
    swarm_child.terminal_at_ms = Some(3_000);
    swarm_child.outcome = Some(AgentTerminalOutcome {
        summary: "swarm child done".to_owned(),
        is_error: false,
    });
    workflow_swarm_started(
        &mut pane,
        "terminal-swarm-call",
        swarm_snapshot("terminal-swarm", swarm_child),
    );

    let first = pane.render_visible_slice(120, 30);
    assert!(
        terminal_text(&first).contains("Workflow"),
        "workflow card renders"
    );
    pane.push_status("unrelated finalized row");
    let unrelated = pane.render_visible_slice(120, 30);
    let unrelated_text = terminal_text(&unrelated);
    assert!(
        unrelated_text.contains("unrelated finalized row"),
        "{unrelated_text}"
    );
    assert!(unrelated_text.contains("Workflow"), "{unrelated_text}");

    for (sequence, phase) in [(2, "build"), (3, "verify"), (4, "report")] {
        let mut update = snapshot(WorkflowState::Running);
        update.projection_sequence = Some(sequence);
        update.current_phase = Some(phase.to_owned());
        pane.transcript_mut().upsert_workflow(update);
        let slice = pane.render_visible_slice(120, 30);
        assert!(terminal_text(&slice).contains(phase), "sequence {sequence}");
    }

    let mut completed = snapshot(WorkflowState::Completed);
    completed.projection_sequence = Some(5);
    completed.updated_at_ms = Some(12_000);
    pane.transcript_mut().upsert_workflow(completed);
    let slice = pane.render_visible_slice(120, 30);
    let group = terminal_text(&slice);
    assert_eq!(
        group.matches("Workflow  Runtime audit and fix").count(),
        1,
        "{group}"
    );
    assert_eq!(group.matches("Workflow Delegates").count(), 1, "{group}");
    assert_eq!(group.matches("Workflow Swarms").count(), 1, "{group}");
    assert_eq!(
        group.matches("Workflow").count(),
        3,
        "one main card plus two sibling summaries: {group}"
    );
}

#[test]
fn workflow_group_keeps_earliest_blocking_input_owner() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 30);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    pane.apply_agent_event(AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "workflow-approval".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Allow workflow command?".to_owned(),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                label: "Allow once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            }],
            workflow_origin: Some(origin("wf-test", "approval-call")),
        },
    });
    pane.apply_question_stream_update(StreamUpdate::QuestionRequested {
        id: "workflow-question".to_owned(),
        questions: vec![QuestionDisplayData {
            question: "Choose target?".to_owned(),
            header: None,
            body: None,
            options: vec![QuestionDisplayOption {
                label: "Local".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
        workflow_origin: Some(origin("wf-test", "question-call")),
    });

    let mut later = snapshot(WorkflowState::AwaitingUser);
    later.projection_sequence = Some(8);
    later.current_phase = Some("choose_target".to_owned());
    pane.transcript_mut().upsert_workflow(later);
    let mut delegate = agent_snapshot("later-agent");
    delegate.display_name = AgentDisplayName::new("LaterAgent");
    workflow_delegate_started(&mut pane, "later-agent-call", delegate.clone());
    delegate.latest_text = Some("later progress".to_owned());
    pane.apply_agent_event(AgentEvent::DelegateProgressUpdated {
        turn: 1,
        progress: agent_progress(&delegate, AgentLifecycleState::Running),
        workflow_origin: Some(origin("wf-test", "later-agent-call")),
    });

    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval("workflow-approval".to_owned()))
    );
    let blocked = terminal_text(&pane.render_visible_slice(120, 30));
    assert!(blocked.contains("Allow workflow command?"), "{blocked}");
    assert!(blocked.contains("Workflow"), "{blocked}");

    pane.resolve_approval(
        "workflow-approval",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Question("workflow-question".to_owned()))
    );
    let promoted = terminal_text(&pane.render_visible_slice(120, 30));
    assert!(promoted.contains("Choose target?"), "{promoted}");
}

/// In a workflow session, the earliest unresolved approval owns the visible
/// focus: later activity stays in the store but out of the current frame
/// until the approval is handled, then the next blocking entry is revealed.
#[test]
fn workflow_approval_focus_defers_later_activity() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 12);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    pane.apply_agent_event(AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "workflow-approval".to_owned(),
            operation: PermissionOperation::Shell,
            presentation: ApprovalPresentation::Tool {
                title: "Approve workflow command?".to_owned(),
                details: vec!["cargo test".to_owned()],
            },
            options: vec![ApprovalOption {
                label: "Allow once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            }],
            workflow_origin: Some(origin("wf-test", "approval-call")),
        },
    });
    // A later workflow question lands after the approval in the store.
    pane.apply_question_stream_update(StreamUpdate::QuestionRequested {
        id: "workflow-question".to_owned(),
        questions: vec![QuestionDisplayData {
            question: "Choose target?".to_owned(),
            header: None,
            body: None,
            options: vec![QuestionDisplayOption {
                label: "Local".to_owned(),
                description: None,
            }],
            multi_select: false,
        }],
        workflow_origin: Some(origin("wf-test", "question-call")),
    });

    // The approval owns the visible focus: its action area is visible and
    // the later question stays out of the current frame.
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Approval("workflow-approval".to_owned()))
    );
    let blocked = terminal_text(&pane.render_visible_slice(120, 12));
    assert!(
        blocked.contains("Approve workflow command?"),
        "approval action area visible:\n{blocked}"
    );
    assert!(
        blocked.contains("↑/↓ select"),
        "approval action hint visible:\n{blocked}"
    );
    assert!(
        !blocked.contains("Choose target?"),
        "later workflow activity deferred while the approval is pending:\n{blocked}"
    );
    let entries = pane.transcript().entries();
    let approval_position = entries
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::ApprovalPrompt(data) if data.id() == "workflow-approval")
        })
        .expect("approval entry");
    let question_position = entries
        .iter()
        .position(|entry| {
            matches!(entry, TranscriptEntry::QuestionPrompt(data) if data.id == "workflow-question")
        })
        .expect("question entry");
    assert!(
        approval_position < question_position,
        "the store keeps the approval before the later question"
    );

    // Handling the approval reveals the next earliest blocking entry: the
    // question's action area becomes visible.
    pane.resolve_approval(
        "workflow-approval",
        &ApprovalResolution::Selected {
            action: ApprovalAction::PermitOnce,
            label: "Allow once".to_owned(),
            feedback: None,
        },
    );
    assert_eq!(
        pane.earliest_blocking_entry(),
        Some(BlockingEntryKind::Question("workflow-question".to_owned()))
    );
    let promoted = terminal_text(&pane.render_visible_slice(120, 12));
    assert!(
        promoted.contains("Choose target?"),
        "the next blocking entry is revealed:\n{promoted}"
    );

    // Answering the question releases the focus entirely: the answered
    // facts render in canonical order.
    pane.resolve_question_prompt("workflow-question", vec!["Local".to_owned()]);
    assert_eq!(pane.earliest_blocking_entry(), None);
    let released = terminal_text(&pane.render_visible_slice(120, 12));
    assert!(
        released.contains("question: answered · Local"),
        "release restores the ordinary view:\n{released}"
    );
    assert!(
        released.contains("approval: Allow once"),
        "resolved approval still rendered:\n{released}"
    );
}

fn assert_finalized_workflow_tool(
    pane: &neo_tui::transcript::TranscriptPane,
    workflow_index: usize,
    revision: u64,
) {
    assert_eq!(
        pane.transcript().entry_revisions()[workflow_index],
        revision
    );
    let TranscriptEntry::Workflow { component } = &pane.transcript().entries()[workflow_index]
    else {
        panic!("workflow entry")
    };
    let tool = &component.direct_tools()[0];
    assert_eq!(tool.result(), Some("final"));
    assert_eq!(tool.status(), ToolStatusKind::Succeeded);
}

fn workflow_toggle_and_render(
    pane: &mut neo_tui::transcript::TranscriptPane,
    tool_id: &str,
    rows: usize,
) -> String {
    assert!(
        pane.toggle_workflow_direct_tool_expansion(tool_id),
        "toggle {tool_id}"
    );
    terminal_text(&pane.render_visible_slice(120, rows))
}

#[test]
fn workflow_document_renders_every_row_without_viewport_omissions() {
    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 8);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    for index in 0..12 {
        let id = format!("completed-read-{index}");
        let read_origin = workflow_tool_started(
            &mut pane,
            &id,
            "Read",
            serde_json::json!({"path": format!("/tmp/{index}.md")}),
        );
        pane.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id,
            name: "Read".to_owned(),
            result: tool_result("read content", false),
            workflow_origin: Some(read_origin),
            output_ref: None,
        });
    }
    workflow_tool_started(
        &mut pane,
        "running-bash",
        "Bash",
        serde_json::json!({"command": "cargo check"}),
    );
    let mut euclid = agent_snapshot("delegate-euclid");
    euclid.display_name = AgentDisplayName::new("Euclid");
    euclid.role = AgentRole::Explorer;
    euclid.state = AgentLifecycleState::Completed;
    euclid.terminal_at_ms = Some(4_000);
    euclid.elapsed = Duration::from_secs(3);
    euclid.outcome = Some(AgentTerminalOutcome {
        summary: "scan completed".to_owned(),
        is_error: false,
    });
    workflow_delegate_started(&mut pane, "delegate-euclid-call", euclid);
    let mut alpha = agent_snapshot("swarm-alpha");
    alpha.display_name = AgentDisplayName::new("Alpha");
    alpha.state = AgentLifecycleState::Completed;
    alpha.terminal_at_ms = Some(4_000);
    alpha.elapsed = Duration::from_secs(2);
    alpha.outcome = Some(AgentTerminalOutcome {
        summary: "verify completed".to_owned(),
        is_error: false,
    });
    let mut beta = agent_snapshot("swarm-beta");
    beta.display_name = AgentDisplayName::new("Beta");
    beta.state = AgentLifecycleState::Running;
    let children = vec![
        SwarmChildSnapshot {
            item_index: 0,
            item: "alpha item".to_owned(),
            agent: alpha,
        },
        SwarmChildSnapshot {
            item_index: 1,
            item: "beta item".to_owned(),
            agent: beta,
        },
    ];
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    workflow_swarm_started(
        &mut pane,
        "swarm-call",
        SwarmSnapshot {
            swarm_id: "swarm-audit".to_owned(),
            description: "audit swarm".to_owned(),
            role: AgentRole::Reviewer,
            mode: AgentRunMode::Foreground,
            state: aggregate.status(),
            max_concurrency: 2,
            aggregate,
            children,
        },
    );

    let full = terminal_text(&pane.render_frame(120, 200).expect("dirty frame"));
    assert!(
        full.contains("Workflow  Runtime audit and fix"),
        "main card:\n{full}"
    );
    for index in 0..12 {
        assert!(
            full.contains(&format!("/tmp/{index}.md")),
            "completed direct tool {index} must render:\n{full}"
        );
    }
    assert!(full.contains("Using Bash"), "running direct tool:\n{full}");
    for name in ["Euclid", "Alpha", "Beta"] {
        assert!(full.contains(name), "child row {name}:\n{full}");
    }
    assert!(
        full.contains("Workflow Delegates") && full.contains("Workflow Swarms"),
        "both summaries render:\n{full}"
    );
    assert!(full.contains("Report") && full.contains("Log"), "{full}");
    for banned in [
        "direct tools omitted",
        "agents omitted",
        "child rows omitted",
        "more rows",
    ] {
        assert!(
            !full.contains(banned),
            "omission marker {banned:?}:\n{full}"
        );
    }

    // A small viewport slices the same complete document; scrolling reaches
    // every structural row.
    let tail_slice = pane.render_visible_slice(120, 8);
    assert!(tail_slice.len() <= 8, "window rows:\n{tail_slice:?}");
    let tail = terminal_text(&tail_slice);
    assert!(tail.contains("Log"), "tail window:\n{tail}");
    pane.scroll_transcript_up(10_000);
    let top = terminal_text(&pane.render_visible_slice(120, 8));
    assert!(
        top.contains("Workflow  Runtime audit and fix"),
        "top window:\n{top}"
    );
}

#[test]
fn workflow_direct_tool_expands_inline_and_collapses_to_one_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ToolOutputStore::new(dir.path().to_owned());
    let output = (1..=12)
        .map(|index| format!("line {index:02}\n"))
        .collect::<String>();
    store
        .append("main", "expanded-bash", &output)
        .expect("append");
    let output_ref = store.finish("main", "expanded-bash").expect("finish");
    assert!(output_ref.complete);

    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 24);
    pane.set_session_directory(Some(dir.path().to_owned()));
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });
    let bash_origin = origin("wf-test", "expanded-bash");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "expanded-bash".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf lines"}),
        workflow_origin: Some(bash_origin.clone()),
        output_ref: Some(output_ref.clone()),
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "expanded-bash".to_owned(),
        command: "printf lines".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "expanded-bash".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: "ok".to_owned(),
        stderr: String::new(),
        truncated: false,
        origin: ShellCommandOrigin::ModelBashTool,
        outcome: ShellCommandOutcome::Completed,
        output_ref: Some(output_ref.clone()),
    });

    let collapsed = terminal_text(&pane.render_visible_slice(120, 24));
    assert_eq!(
        collapsed.matches("Used Bash").count(),
        1,
        "one line per tool by default:\n{collapsed}"
    );
    assert!(!collapsed.contains("line 07"), "collapsed:\n{collapsed}");

    let expanded = workflow_toggle_and_render(&mut pane, "expanded-bash", 24);
    assert!(
        expanded.contains("line 07"),
        "expansion reads beyond the six-line live preview:\n{expanded}"
    );
    assert!(expanded.contains("line 12"), "visible range:\n{expanded}");
    assert!(
        expanded.contains("printf lines"),
        "command row:\n{expanded}"
    );

    let restored = workflow_toggle_and_render(&mut pane, "expanded-bash", 24);
    assert!(
        !restored.contains("line 07"),
        "collapses to one row:\n{restored}"
    );
    assert!(
        !pane.toggle_workflow_direct_tool_expansion("missing-tool"),
        "unknown typed tool ID is rejected"
    );
}

#[test]
fn workflow_expansion_reports_output_states_honestly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ToolOutputStore::new(dir.path().to_owned());

    // Corrupt index: the log exists but the derived index is garbage. The
    // store rebuilds it; the completion marker is lost, so the card must not
    // relabel the rebuilt artifact complete.
    store
        .append(
            "main",
            "corrupt-index",
            "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n",
        )
        .expect("append");
    store.finish("main", "corrupt-index").expect("finish");
    std::fs::write(
        dir.path()
            .join("agents")
            .join("main")
            .join("tasks")
            .join("corrupt-index.log.idx"),
        b"garbage index",
    )
    .expect("corrupt index");

    // Child-origin reference: the artifact resolves through the ref's own
    // agent id, never an assumed main-agent path.
    store
        .append("agent-42", "child-task", "child one\nchild two\n")
        .expect("append");
    let child_ref = store.finish("agent-42", "child-task").expect("finish");

    // Stale reference: claims two lines and incomplete, while the artifact
    // has grown and finished. Expansion must re-read the store metadata.
    let stale_ref = neo_agent_core::session::ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "stale-task".to_owned(),
        byte_len: 12,
        line_count: 2,
        complete: false,
    };
    store
        .append(
            "main",
            "stale-task",
            "stale one\nstale two\nstale three\nstale four\n",
        )
        .expect("append");
    store.finish("main", "stale-task").expect("finish");

    // Absent artifact: the ref exists but the log was never opened.
    let absent_ref = neo_agent_core::session::ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: "missing-task".to_owned(),
        byte_len: 10,
        line_count: 3,
        complete: true,
    };

    // Live artifact: appended but never finished (complete: false).
    store
        .append("main", "live-task", "live one\nlive two\n")
        .expect("append");
    let live_ref = store.metadata("main", "live-task").expect("live metadata");

    let mut pane = neo_tui::transcript::TranscriptPane::new(120, 60);
    pane.set_session_directory(Some(dir.path().to_owned()));
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: snapshot(WorkflowState::Running),
    });

    let start_finished_bash =
        |pane: &mut neo_tui::transcript::TranscriptPane,
         id: &str,
         output_ref: Option<neo_agent_core::session::ToolOutputRef>| {
            let tool_origin = origin("wf-test", id);
            pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: "Bash".to_owned(),
                arguments: serde_json::json!({"command": format!("printf {id}")}),
                workflow_origin: Some(tool_origin.clone()),
                output_ref: output_ref.clone(),
            });
            pane.apply_agent_event(AgentEvent::ShellCommandStarted {
                turn: 1,
                id: id.to_owned(),
                command: format!("printf {id}"),
                cwd: "/tmp".into(),
                origin: ShellCommandOrigin::ModelBashTool,
            });
            pane.apply_agent_event(AgentEvent::ShellCommandFinished {
                turn: 1,
                id: id.to_owned(),
                exit_code: Some(0),
                signal: None,
                stdout: "ok".to_owned(),
                stderr: String::new(),
                truncated: false,
                origin: ShellCommandOrigin::ModelBashTool,
                outcome: ShellCommandOutcome::Completed,
                output_ref,
            });
        };

    start_finished_bash(
        &mut pane,
        "corrupt-index",
        Some(store.metadata("main", "corrupt-index").expect("ref")),
    );
    start_finished_bash(&mut pane, "child-origin", Some(child_ref));
    start_finished_bash(&mut pane, "stale-task", Some(stale_ref));
    start_finished_bash(&mut pane, "absent-artifact", Some(absent_ref));
    start_finished_bash(&mut pane, "legacy-tool", None);
    let live_origin = origin("wf-test", "live-task");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "live-task".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({"command": "printf live"}),
        workflow_origin: Some(live_origin),
        output_ref: Some(live_ref),
    });
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 1,
        id: "live-task".to_owned(),
        command: "printf live".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::ModelBashTool,
    });

    let corrupt = workflow_toggle_and_render(&mut pane, "corrupt-index", 60);
    for line in ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta"] {
        assert!(
            corrupt.contains(line),
            "rebuilt index serves rows:\n{corrupt}"
        );
    }
    assert!(
        !corrupt.contains("complete output unavailable"),
        "{corrupt}"
    );

    let child = workflow_toggle_and_render(&mut pane, "child-origin", 60);
    assert!(
        child.contains("child one") && child.contains("child two"),
        "child-origin artifact resolves through the ref agent id:\n{child}"
    );

    let stale = workflow_toggle_and_render(&mut pane, "stale-task", 60);
    assert!(
        stale.contains("stale one") && stale.contains("stale four"),
        "metadata re-read, never the stale reference:\n{stale}"
    );

    let absent = workflow_toggle_and_render(&mut pane, "absent-artifact", 60);
    assert!(
        absent.contains("complete output unavailable"),
        "absent artifact is explicitly unavailable:\n{absent}"
    );
    assert!(!absent.contains("complete output not captured"), "{absent}");

    let legacy = workflow_toggle_and_render(&mut pane, "legacy-tool", 60);
    assert!(
        legacy.contains("complete output not captured"),
        "legacy tool without a ref is honest:\n{legacy}"
    );

    let live = workflow_toggle_and_render(&mut pane, "live-task", 60);
    assert!(
        live.contains("live one") && live.contains("live two"),
        "live artifact serves the written-so-far range:\n{live}"
    );
    assert!(
        live.contains("output incomplete"),
        "unfinished artifact is never relabeled complete:\n{live}"
    );
}
