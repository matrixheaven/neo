//! Focused interactive tests for the fullscreen transcript lifecycle:
//! exit projection ordering (built from canonical final state, printed after
//! terminal restoration), lazy resume (output references restored as
//! metadata without eager artifact reads, Workflow child activity grouped
//! without duplicate top-level cards), and visible-only animation scheduling
//! (off-screen entries never request frame deadlines).
//!
//! Kept outside `tests.rs` per the fullscreen transcript plan so the growing
//! controller test file does not absorb more surface.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use neo_agent_core::session::ToolOutputRef;
use neo_agent_core::workflow::{
    WorkflowExecutionOrigin, WorkflowId, WorkflowSnapshot, WorkflowState,
};
use neo_agent_core::{AgentEvent, AgentMessage, Content, StopReason, ToolResult};
use neo_tui::screen_output::FullscreenTerminal;
use neo_tui::transcript::TranscriptEntry;
use neo_tui::widgets::todo_panel::{TodoDisplayItem, TodoDisplayStatus};

use super::*;

fn test_workspace_root() -> PathBuf {
    let dir = std::env::temp_dir().join("neo-fullscreen-test-workspace");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn running_workflow_snapshot() -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId("workflow-run-1".to_owned()),
        title: "Demo".to_owned(),
        state: WorkflowState::Running,
        current_phase: Some("work".to_owned()),
        projection_sequence: Some(1),
        recovery_failure: false,
        started_at_ms: Some(1_000),
        updated_at_ms: Some(2_000),
        invocation_count: 1,
        failure_count: 0,
        actual_usage: None,
        latest_log_summary: None,
        latest_report_summary: None,
        terminal_reason: None,
        display_name: "Demo".to_owned(),
        purpose: "demo run".to_owned(),
    }
}

fn workflow_origin(invocation_id: &str) -> WorkflowExecutionOrigin {
    WorkflowExecutionOrigin {
        run_id: WorkflowId("workflow-run-1".to_owned()),
        human_handle: None,
        definition_name: "Demo".to_owned(),
        definition_revision: None,
        phase_id: None,
        invocation_id: Some(invocation_id.to_owned()),
        swarm_item_id: None,
    }
}

fn output_ref(task_id: &str) -> ToolOutputRef {
    ToolOutputRef {
        agent_id: "main".to_owned(),
        task_id: task_id.to_owned(),
        byte_len: 0,
        line_count: 0,
        complete: false,
    }
}

fn controller_with_session() -> InteractiveController {
    InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    )
}

#[test]
fn terminal_exit_projection_prints_after_restore() {
    let mut controller = controller_with_session();
    controller.set_active_session_id("test-session".to_owned());
    // Canonical final state: an interrupted tool, an interrupted assistant
    // answer, a running Workflow, and terminal todo state.
    controller.apply_turn_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt", "content": "draft"}),
        workflow_origin: None,
        output_ref: None,
    });
    controller.tui.transcript_mut().start_assistant_message();
    controller
        .tui
        .transcript_mut()
        .append_assistant_delta("unfinished assistant text");
    controller.apply_turn_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: running_workflow_snapshot(),
    });
    controller.tui.chrome_mut().set_todo_items(vec![
        TodoDisplayItem::new("Task 1", TodoDisplayStatus::Done),
        TodoDisplayItem::new("Task 2", TodoDisplayStatus::InProgress),
    ]);
    let mut terminal = FullscreenTerminal::for_test(80, 24);
    let initial = controller.tui.render_terminal_frame(80, 24);
    terminal
        .render_to(&mut Vec::new(), &initial)
        .expect("render initial live frame");

    let mut final_frame = None;
    controller
        .finalize_and_render_terminal_exit(|tui| {
            final_frame = Some(tui.render_terminal_frame(80, 24));
            Ok(())
        })
        .expect("finalize and render terminal exit");
    // The projection is derived from canonical final state BEFORE restore...
    let projection = controller.exit_projection();
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, final_frame.as_ref().expect("final frame"))
        .expect("commit final frame");
    // ...and the terminal is restored only after the final frame commits.
    terminal.leave(&mut output).expect("leave terminal");
    let terminal_output = String::from_utf8(output).expect("terminal output is UTF-8");

    // Inside the alternate screen the committed interrupted state is present,
    // but the static projection itself is never drawn there: it is returned
    // for `main` to print after restoration.
    assert!(
        terminal_output.contains("unfinished assistant text"),
        "{terminal_output}"
    );
    assert!(terminal_output.contains("Write"), "{terminal_output}");
    assert!(
        !terminal_output.contains("Resume: neo resume"),
        "{terminal_output}"
    );

    // The projection (printed after restore) carries exactly the final
    // assistant answer, terminal task/Workflow status, and the reopen command.
    // The interrupted workflow's terminal state is `failed` (interrupted when
    // the terminal exited), which is exactly the status the projection shows.
    assert!(
        projection.contains("unfinished assistant text"),
        "{projection}"
    );
    assert!(
        projection.contains("Tasks: 1 done, 1 in progress, 0 pending"),
        "{projection}"
    );
    assert!(projection.contains("Workflow Demo: failed"), "{projection}");
    assert!(
        projection.contains("Resume: neo resume test-session"),
        "{projection}"
    );
    assert!(
        !projection.contains('\x1b'),
        "projection must be plain text: {projection}"
    );
    assert!(projection.len() <= EXIT_PROJECTION_MAX_BYTES + 1);
}

#[test]
fn resume_restores_output_references_without_eager_full_reads() {
    let temp = tempfile::tempdir().expect("temp session dir");
    let session_dir = temp.path().to_path_buf();
    // The output artifacts for these references are NEVER created: a resume
    // that eagerly read complete output files would fail here.
    let child_ref = output_ref("child-task-1");
    let top_ref = output_ref("top-task-1");
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("run the workflow"),
        },
        AgentEvent::WorkflowStarted {
            turn: 1,
            workflow: running_workflow_snapshot(),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "wf-tool-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({"command": "echo child"}),
            workflow_origin: Some(workflow_origin("wf-tool-1")),
            output_ref: Some(child_ref.clone()),
        },
        AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "wf-tool-1".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok("child partial"),
            workflow_origin: Some(workflow_origin("wf-tool-1")),
            output_ref: Some(child_ref.clone()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "wf-tool-1".to_owned(),
            name: "Bash".to_owned(),
            result: ToolResult::ok("child done"),
            workflow_origin: Some(workflow_origin("wf-tool-1")),
            output_ref: Some(child_ref.clone()),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "top-tool-1".to_owned(),
            name: "Write".to_owned(),
            arguments: serde_json::json!({"path": "out.txt"}),
            workflow_origin: None,
            output_ref: Some(top_ref.clone()),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "top-tool-1".to_owned(),
            name: "Write".to_owned(),
            result: ToolResult::ok("top done"),
            workflow_origin: None,
            output_ref: Some(top_ref.clone()),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("workflow finished")],
                Vec::new(),
                StopReason::EndTurn,
            ),
        },
    ]);

    let mut controller = controller_with_session();
    controller.rebuild_transcript_from_session(&loaded);
    // Wire the session directory the way a startup resume does; the store
    // resolves refs lazily, so missing artifacts must not fail the rebuild.
    controller
        .tui
        .transcript_mut()
        .set_session_directory(Some(session_dir));

    let entries = controller.tui.transcript().transcript().entries();
    let workflow = entries
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::Workflow { component } => Some(component),
            _ => None,
        })
        .expect("workflow card rehydrated");
    assert_eq!(workflow.direct_tools().len(), 1, "child tool grouped once");
    let child = &workflow.direct_tools()[0];
    assert_eq!(child.id(), "wf-tool-1");
    assert_eq!(
        child.output_ref(),
        Some(&child_ref),
        "typed ref restored as metadata"
    );

    // The top-level tool stays a single top-level card: no duplicate card is
    // created for the Workflow child.
    let top_level_tools = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::ToolRun { .. }))
        .count();
    assert_eq!(
        top_level_tools, 1,
        "only the non-workflow tool gets a top-level card"
    );
    assert!(
        entries
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::ToolRun { component } if component.id() == "wf-tool-1"))
            .next()
            .is_none(),
        "workflow child must never appear as a top-level ToolRun card"
    );

    // Expanding the restored child tool reads lazily through the store: the
    // missing artifact renders as explicitly unavailable, never as complete
    // output, and never crashes the resume.
    controller
        .tui
        .transcript_mut()
        .toggle_workflow_direct_tool_expansion("wf-tool-1");
    let slice = controller
        .tui
        .transcript_mut()
        .render_terminal_slice(80, 24);
    let rendered = slice
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("complete output unavailable"),
        "{rendered}"
    );
    assert!(!rendered.contains("child partial"), "{rendered}");
}

#[test]
fn offscreen_entries_do_not_schedule_animation_frames() {
    let mut controller = controller_with_session();
    // The animating Workflow card sits at the top of a tall transcript; the
    // tail-following viewport shows only the filler statuses below it. The
    // background-workflow path keeps the chrome out of streaming mode, so the
    // deadline decision comes exclusively from the visible entry slice.
    controller.apply_background_workflow_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: running_workflow_snapshot(),
    });
    for index in 0..60 {
        controller.push_status(format!("filler status {index}"));
    }

    let now = Instant::now();
    let frame = controller.tui.render_terminal_frame_at(80, 24, now);
    assert!(
        frame.next_animation_deadline.is_none(),
        "off-screen animation must not request a frame deadline"
    );
    // Off-screen entries pause presentation ticks: advancing animation while
    // the card is outside the visible slice must not touch its entries.
    assert!(!controller.tui.is_transcript_dirty());
    controller
        .tui
        .advance_animation_at(now + Duration::from_millis(100));
    assert!(
        !controller.tui.is_transcript_dirty(),
        "off-screen presentation ticks must pause"
    );

    // Scroll the Workflow card into the visible slice: the same entry now
    // requests the existing 100 ms cadence deadline.
    controller.scroll_transcript_up(200);
    let frame = controller
        .tui
        .render_terminal_frame_at(80, 24, now + Duration::from_millis(16));
    let deadline = frame
        .next_animation_deadline
        .expect("visible animation requests the next deadline");
    assert_eq!(
        deadline,
        now + Duration::from_millis(16) + Duration::from_millis(100)
    );
    // Once visible, animation ticks resume and dirty the pane.
    controller
        .tui
        .advance_animation_at(now + Duration::from_millis(116));
    assert!(controller.tui.is_transcript_dirty(), "visible ticks resume");
    let rendered = frame
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Workflow"), "{rendered}");
}
