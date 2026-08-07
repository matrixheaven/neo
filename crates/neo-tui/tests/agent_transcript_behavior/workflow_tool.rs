use neo_agent_core::session::ToolOutputStore;
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{AgentEvent, ShellCommandOrigin, ShellCommandOutcome, ToolResult};
use neo_tui::primitive::strip_ansi;
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::TranscriptEntry;

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
