//! Phase 4/5: consecutive same-tool calls collapse into tree cards.
//!
//! These exercise the live runtime path (not the components.rs widget path),
//! so they cover what the user actually sees.

use neo_agent_core::AgentEvent;
use neo_tui::NeoTuiRuntime;
use neo_tui::ansi::strip_ansi;

fn plain_frame(runtime: &mut NeoTuiRuntime, width: usize, height: usize) -> Vec<String> {
    runtime
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| strip_ansi(line).trim_end().to_owned())
        .collect()
}

#[test]
fn consecutive_reads_collapse_into_one_tree_card() {
    let mut runtime = NeoTuiRuntime::new(80, 20);
    for (id, path) in ["a.rs", "b.rs", "c.rs"].into_iter().enumerate() {
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: format!("read-{id}"),
            name: "Read".to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: format!("read-{id}"),
            json_fragment: format!(r#"{{"path":"{path}"}}"#),
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: format!("read-{id}"),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("one\ntwo\nthree"),
        });
    }

    let frame = plain_frame(&mut runtime, 80, 20);
    let joined = frame.join("\n");

    assert!(
        joined.contains("Read 3 files · 9 lines"),
        "group header present: {joined}"
    );
    assert!(
        joined.contains("├─ a.rs · 3 lines"),
        "first branch: {joined}"
    );
    assert!(
        joined.contains("├─ b.rs · 3 lines"),
        "middle branch: {joined}"
    );
    assert!(
        joined.contains("└─ c.rs · 3 lines"),
        "last branch: {joined}"
    );
    assert!(
        !joined.contains("Used Read (a.rs)"),
        "no solo cards when grouped: {joined}"
    );
}

#[test]
fn single_read_still_renders_as_solo_card() {
    let mut runtime = NeoTuiRuntime::new(80, 12);
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "read-0".to_owned(),
        name: "Read".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "read-0".to_owned(),
        json_fragment: r#"{"path":"only.rs"}"#.to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-0".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("one\ntwo"),
    });

    let frame = plain_frame(&mut runtime, 80, 12);
    let joined = frame.join("\n");
    assert!(
        joined.contains("Used Read (only.rs)") || joined.contains("Read (only.rs)"),
        "single read renders solo: {joined}"
    );
    assert!(
        !joined.contains("Read 1 files"),
        "no group header for single read: {joined}"
    );
}

#[test]
fn non_groupable_tool_between_reads_breaks_the_group() {
    let mut runtime = NeoTuiRuntime::new(80, 20);
    for (id, name, is_path, fragment) in [
        ("read-0", "Read", true, "a.rs"),
        ("bash-0", "Bash", false, "ls"),
        ("read-1", "Read", true, "b.rs"),
    ] {
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: id.to_owned(),
            name: name.to_owned(),
        });
        let json = if is_path {
            format!(r#"{{"path":"{fragment}"}}"#)
        } else {
            format!(r#"{{"command":"{fragment}"}}"#)
        };
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: id.to_owned(),
            json_fragment: json,
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: id.to_owned(),
            name: name.to_owned(),
            result: neo_agent_core::ToolResult::ok("output"),
        });
    }

    let frame = plain_frame(&mut runtime, 80, 20);
    let joined = frame.join("\n");
    assert!(
        !joined.contains("Read 2 files"),
        "bash between reads breaks grouping: {joined}"
    );
}

#[test]
fn group_header_uses_capitalized_verb_without_duplicating_tool_name() {
    // Regression: the header must read `● Read 3 files` (capitalized verb,
    // count) and NOT `● read Read 3 files` (duplicated, lowercase).
    let mut runtime = NeoTuiRuntime::new(80, 20);
    for (id, path) in ["a.rs", "b.rs", "c.rs"].into_iter().enumerate() {
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: format!("read-{id}"),
            name: "Read".to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: format!("read-{id}"),
            json_fragment: format!(r#"{{"path":"{path}"}}"#),
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: format!("read-{id}"),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("one\ntwo"),
        });
    }
    let frame = plain_frame(&mut runtime, 80, 20);
    let joined = frame.join("\n");
    assert!(
        joined.contains("● Read 3 files"),
        "capitalized header: {joined}"
    );
    assert!(
        !joined.contains("read Read"),
        "header must not duplicate the tool name: {joined}"
    );
    assert!(
        !joined.contains("● read"),
        "header must not show lowercase tool name: {joined}"
    );
}

#[test]
fn list_group_uses_list_verb_not_read() {
    // `list` calls must show `● List 2 files`, not `● list Read 2 files`.
    let mut runtime = NeoTuiRuntime::new(80, 20);
    for (id, path) in ["crates", "docs"].into_iter().enumerate() {
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: format!("list-{id}"),
            name: "list".to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: format!("list-{id}"),
            json_fragment: format!(r#"{{"path":"{path}"}}"#),
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: format!("list-{id}"),
            name: "list".to_owned(),
            result: neo_agent_core::ToolResult::ok("entry one\nentry two"),
        });
    }
    let frame = plain_frame(&mut runtime, 80, 20);
    let joined = frame.join("\n");
    assert!(joined.contains("● List 2 files"), "list header: {joined}");
    assert!(
        !joined.contains("Read 2 files"),
        "list group must not say Read: {joined}"
    );
    assert!(
        !joined.contains("list List"),
        "no duplicated name: {joined}"
    );
}

#[test]
fn finalized_tool_block_is_separated_from_live_assistant_text() {
    // A tool card (drained into history) must not touch the live assistant
    // text that follows it — there should be a blank line between them.
    let mut runtime = NeoTuiRuntime::new(80, 20);
    runtime.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "list-0".to_owned(),
        name: "list".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "list-0".to_owned(),
        json_fragment: r#"{"path":"crates"}"#.to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "list-0".to_owned(),
        name: "list".to_owned(),
        result: neo_agent_core::ToolResult::ok("entry one\nentry two"),
    });
    // Now a live assistant message streams in after the tool finished.
    runtime.apply_agent_event(AgentEvent::MessageStarted {
        turn: 1,
        id: "a-1".to_owned(),
    });
    runtime.apply_agent_event(AgentEvent::TextDelta {
        turn: 1,
        text: "Here is the summary.".to_owned(),
    });

    let frame = plain_frame(&mut runtime, 80, 20);
    let joined = frame.join("\n");
    // Both the tool card and the assistant text are present.
    assert!(
        joined.contains("Used list") || joined.contains("List 1 files"),
        "tool card present: {joined}"
    );
    assert!(
        joined.contains("Here is the summary."),
        "assistant text present: {joined}"
    );
    // Find the tool card and assistant text row indices (whichever order).
    let tool_idx = frame
        .iter()
        .position(|l| l.contains("Used list") || l.contains("List 1 files"))
        .expect("tool card in frame");
    let text_idx = frame
        .iter()
        .position(|l| l.contains("Here is the summary."))
        .expect("assistant text in frame");
    let (lo, hi) = if tool_idx < text_idx {
        (tool_idx, text_idx)
    } else {
        (text_idx, tool_idx)
    };
    // A blank line must separate the two blocks, regardless of order.
    let gap = &frame[lo + 1..hi];
    assert!(
        gap.iter().any(|l| l.trim().is_empty()),
        "blank line separates tool card from assistant text, got gap: {gap:?}"
    );
}

#[test]
fn multiple_consecutive_tool_cards_are_each_separated_by_blank_lines() {
    // Regression for the "cards touch each other" bug: two finished list
    // calls that do NOT group (different names, or broken by a non-groupable
    // tool) must each be separated by a blank line.
    let mut runtime = NeoTuiRuntime::new(80, 20);
    // list, then bash (non-groupable, breaks any list run), then list.
    for (id, name, fragment) in [
        ("list-0", "list", r#"{"path":"crates"}"#),
        ("bash-0", "bash", r#"{"command":"echo hi"}"#),
        ("list-1", "list", r#"{"path":"docs"}"#),
    ] {
        runtime.apply_agent_event(AgentEvent::ToolCallStarted {
            turn: 1,
            id: id.to_owned(),
            name: name.to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolCallArgumentsDelta {
            turn: 1,
            id: id.to_owned(),
            json_fragment: fragment.to_owned(),
        });
        runtime.apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: id.to_owned(),
            name: name.to_owned(),
            result: neo_agent_core::ToolResult::ok("output line"),
        });
    }

    let frame = plain_frame(&mut runtime, 80, 20);
    // Each card header is present.
    let list0 = frame.iter().position(|l| l.contains("Used list (crates)"));
    let bash0 = frame.iter().position(|l| l.contains("Used bash"));
    let list1 = frame.iter().position(|l| l.contains("Used list (docs)"));
    let (list0, bash0, list1) = (
        list0.expect("list0"),
        bash0.expect("bash0"),
        list1.expect("list1"),
    );
    // Between every pair of adjacent cards there must be a blank line.
    let gap_a = &frame[list0 + 1..bash0];
    let gap_b = &frame[bash0 + 1..list1];
    assert!(
        gap_a.iter().any(|l| l.trim().is_empty()),
        "blank between list0 and bash0, got: {gap_a:?}"
    );
    assert!(
        gap_b.iter().any(|l| l.trim().is_empty()),
        "blank between bash0 and list1, got: {gap_b:?}"
    );
}
