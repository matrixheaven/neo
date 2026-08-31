//! Tool-run group render-cache invalidation: the pane's steady-state group
//! block cache must never serve stale rows after a mutation that touches the
//! group's revisions (result/details change, expansion toggle, theme change,
//! suppression transition).

use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, strip_ansi};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::TranscriptPane;
use serde_json::json;

const WIDTH: usize = 100;
const HEIGHT: usize = 48;

fn plain_frame(pane: &mut TranscriptPane) -> Vec<String> {
    pane.render_visible_slice(WIDTH, HEIGHT)
        .iter()
        .map(|row| strip_ansi(row).clone())
        .collect()
}

/// Push a terminal tool card through the real store path: a running
/// placeholder followed by a result mutation (which touches the group's
/// revisions).
fn push_finished_tool(
    pane: &mut TranscriptPane,
    id: &str,
    name: &str,
    arguments: serde_json::Value,
    result: &str,
    details: Option<serde_json::Value>,
) {
    let store = pane.transcript_mut();
    store.push_tool_run(id, name, Some(arguments.to_string()));
    store.mutate_tool(id, |tool| {
        tool.set_result(Some(result.to_owned()), details, false, None)
    });
}

fn push_exit_plan_mode(pane: &mut TranscriptPane, id: &str, plan_step: &str) {
    push_finished_tool(
        pane,
        id,
        "ExitPlanMode",
        json!({}),
        "submitted",
        Some(json!({
            "plan_content": format!("# Plan\n\n1. {plan_step}\n2. Second step\n"),
            "plan_path": "/tmp/plan.md",
        })),
    );
}

fn push_created_write(pane: &mut TranscriptPane, id: &str) {
    let content = (0..30)
        .map(|index| format!("line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    push_finished_tool(
        pane,
        id,
        "Write",
        json!({ "path": "src/created.rs" }),
        "wrote 1 file",
        Some(json!({
            "kind": "write",
            "status": "committed",
            "files": 1,
            "created": 1,
            "changes": [{
                "path": "src/created.rs",
                "operation": "created",
                "status": "committed",
                "line_count": 30,
                "content": content,
            }],
        })),
    );
}

fn push_finished_read(pane: &mut TranscriptPane, id: &str) {
    push_finished_tool(
        pane,
        id,
        "Read",
        json!({ "path": "src/marker.rs" }),
        "file contents",
        None,
    );
}

#[test]
fn unchanged_plan_box_frame_is_identical_across_recompositions() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_exit_plan_mode(&mut pane, "plan-1", "First step");

    let first = plain_frame(&mut pane);
    assert!(
        first.iter().any(|row| row.contains("Current plan")),
        "plan card visible: {first:?}"
    );
    let second = plain_frame(&mut pane);
    assert_eq!(first, second);
}

#[test]
fn tool_group_block_rerenders_after_member_revision_change() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_exit_plan_mode(&mut pane, "plan-1", "First step");
    let before = plain_frame(&mut pane);
    assert!(before.iter().any(|row| row.contains("First step")));

    let changed = pane.transcript_mut().mutate_tool("plan-1", |tool| {
        let mut details = tool.state().details.clone().unwrap_or_default();
        details["plan_content"] = json!("# Plan\n\n1. Brand new final step\n");
        tool.set_result(tool.result().map(str::to_owned), Some(details), false, None)
    });
    assert!(changed, "fixture mutation must report a change");

    let after = plain_frame(&mut pane);
    assert!(
        after.iter().any(|row| row.contains("Brand new final step")),
        "re-rendered group block must reflect the new plan content: {after:?}"
    );
    assert!(
        !after.iter().any(|row| row.contains("First step")),
        "stale plan rows must not be served from the cache: {after:?}"
    );
}

#[test]
fn tool_group_block_rerenders_after_expansion_toggle() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_created_write(&mut pane, "write-1");

    let collapsed = plain_frame(&mut pane);
    assert!(
        collapsed.iter().any(|row| row.contains("lines hidden")),
        "collapsed preview note present: {collapsed:?}"
    );

    pane.set_tool_output_expanded(true);
    let expanded = plain_frame(&mut pane);
    assert!(
        !expanded.iter().any(|row| row.contains("lines hidden")),
        "expanded preview must drop the collapsed note: {expanded:?}"
    );
    assert!(
        expanded.iter().any(|row| row.contains("line 29")),
        "expanded preview must show tail rows: {expanded:?}"
    );
}

#[test]
fn tool_group_block_rerenders_after_theme_change() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_exit_plan_mode(&mut pane, "plan-1", "First step");

    let before = pane.render_visible_slice(WIDTH, HEIGHT);
    let theme = TuiTheme {
        status_ok: Color::Red,
        ..TuiTheme::default()
    };
    pane.set_theme(theme);
    let after = pane.render_visible_slice(WIDTH, HEIGHT);

    assert_ne!(
        before, after,
        "theme change must re-color the cached group block"
    );
}

#[test]
fn tool_group_block_rerenders_after_suppress_transition() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_exit_plan_mode(&mut pane, "plan-1", "First step");
    push_finished_read(&mut pane, "read-1");

    let before = plain_frame(&mut pane);
    assert!(
        before.iter().any(|row| row.contains("src/marker.rs")),
        "grouped read card visible before suppression: {before:?}"
    );

    pane.transcript_mut().suppress_tool_run("read-1");
    let after = plain_frame(&mut pane);
    assert!(
        !after.iter().any(|row| row.contains("src/marker.rs")),
        "suppressed member must leave the group block: {after:?}"
    );
    assert!(
        after.iter().any(|row| row.contains("Current plan")),
        "the plan card stays after group re-shaping: {after:?}"
    );
}

#[test]
fn collapsed_write_preview_keeps_head_tail_rows_and_numbering() {
    let mut pane = TranscriptPane::new(WIDTH, HEIGHT);
    push_created_write(&mut pane, "write-1");

    let frame = plain_frame(&mut pane);
    for expected in ["1  line 00", "5  line 04", "26  line 25", "30  line 29"] {
        assert!(
            frame.iter().any(|row| row.contains(expected)),
            "collapsed preview must keep numbered head/tail row {expected:?}: {frame:?}"
        );
    }
}
