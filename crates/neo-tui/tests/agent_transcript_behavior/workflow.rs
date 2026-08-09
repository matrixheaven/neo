use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome, AgentTerminalReason,
    AgentToolActivityPhase, AgentToolOutputPreview, DelegateContext, SwarmAggregate,
    SwarmChildSnapshot, SwarmSnapshot,
};
use neo_agent_core::session::ToolOutputStore;
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{
    AgentEvent, ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest,
    ApprovalResolution, PermissionOperation, ShellCommandOrigin, ShellCommandOutcome, ToolResult,
};
use neo_tui::dialogs::{QuestionDisplayData, QuestionDisplayOption};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Component, Expandable, Finalization, Line, strip_ansi, visible_width};
use neo_tui::shell::StreamUpdate;
use neo_tui::transcript::{
    BlockingEntryKind, DelegateCardComponent, DelegateGroupComponent, SwarmCardComponent,
    TranscriptEntry, WorkflowCardComponent,
};
use std::time::Duration;

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
        input_token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
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
        input_token_count: 0,
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
fn frozen_rows(lines: Vec<Line>) -> Vec<String> {
    lines
        .into_iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect()
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
