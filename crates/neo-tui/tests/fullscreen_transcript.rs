//! Fullscreen transcript document: incremental layout, the logical scroll
//! anchor, tail-follow vs. locked scroll, and bounded visible-slice
//! resolution.

use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, PermissionOperation,
};
use neo_tui::primitive::strip_ansi;
use neo_tui::screen_output::{FullscreenTerminal, TerminalFrame};
use neo_tui::shell::ToolStatusKind;
use neo_tui::transcript::{
    ApprovalDisplayState, ApprovalPromptData, DelegateGroupComponent, TranscriptEntry,
    TranscriptPane,
};

/// The non-blank content lines of a rendered slice, in order.
fn non_blank_lines(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| strip_ansi(row))
        .filter(|row| !row.trim().is_empty())
        .collect()
}

/// The number of tool-card header rows in a composed frame.
fn count_tool_cards(rows: &[String]) -> usize {
    rows.iter()
        .filter(|row| {
            let text = strip_ansi(row);
            ["Using Bash", "Used Bash", "Preparing Bash"]
                .iter()
                .any(|verb| text.contains(verb))
        })
        .count()
}

/// Assert the tool group is framed by exactly one document-owned blank row
/// above and below: `before`, one blank, the group block, one blank, `after`.
fn assert_single_blank_framing(rows: &[String], before: &str, after: &str) {
    let before_idx = rows
        .iter()
        .position(|row| strip_ansi(row).contains(before))
        .expect("leading entry");
    let after_idx = rows
        .iter()
        .rposition(|row| strip_ansi(row).contains(after))
        .expect("trailing entry");
    assert!(
        before_idx + 2 < after_idx,
        "group sits between the framing entries: {rows:?}"
    );
    assert!(
        strip_ansi(&rows[before_idx + 1]).trim().is_empty(),
        "exactly one blank row below the leading entry: {rows:?}"
    );
    assert!(
        !strip_ansi(&rows[before_idx + 2]).trim().is_empty(),
        "the group starts immediately after the single blank row: {rows:?}"
    );
    assert!(
        strip_ansi(&rows[after_idx - 1]).trim().is_empty(),
        "exactly one blank row above the trailing entry: {rows:?}"
    );
    assert!(
        !strip_ansi(&rows[after_idx - 2]).trim().is_empty(),
        "the group ends immediately before the single blank row: {rows:?}"
    );
}

fn approval_for_tool(tool_id: &str) -> ApprovalPromptData {
    ApprovalPromptData {
        request: ApprovalRequest {
            turn: 1,
            id: tool_id.to_owned(),
            operation: PermissionOperation::Tool,
            presentation: ApprovalPresentation::Tool {
                title: "Approve Bash".to_owned(),
                details: vec!["run the tool?".to_owned()],
            },
            options: vec![ApprovalOption {
                label: "Approve".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            }],
            workflow_origin: None,
        },
        selected: 0,
        feedback_input: String::new(),
        feedback_active: false,
        expanded: false,
        state: ApprovalDisplayState::Pending,
    }
}

#[test]
fn fullscreen_lifecycle_enters_and_restores_once() {
    let mut pane = TranscriptPane::new(80, 12);
    for index in 0..8 {
        pane.push_status(format!("row-{index}"));
    }
    let frame = TerminalFrame::new(pane.render_visible_slice(80, 12), None);
    let mut terminal = FullscreenTerminal::for_test(80, 12);
    let mut output = Vec::new();

    // Every frame is one bounded slice written at the fullscreen origin with
    // absolute CUP — never a native-history scroll into the alternate screen.
    terminal
        .render_to(&mut output, &frame)
        .expect("render frame");
    let frame_text = String::from_utf8_lossy(&output);
    assert!(
        !frame_text.contains("\r\n"),
        "native history write detected: {frame_text:?}"
    );
    assert!(
        !output.windows(4).any(|window| window == b"\x1b[2J")
            && !output.windows(4).any(|window| window == b"\x1b[3J"),
        "frame must not erase the surface: {frame_text:?}"
    );
    assert!(
        frame_text.contains("row-7"),
        "bounded slice content missing: {frame_text:?}"
    );

    // Suspend clears the live surface without erasing; resume repaints the
    // fresh alternate screen fully; leave clears once and restores the
    // cursor. Repeated frame content emits no bytes.
    let mut suspend = Vec::new();
    terminal
        .suspend_prepare(&mut suspend)
        .expect("prepare suspend");
    let suspend_text = String::from_utf8_lossy(&suspend);
    assert!(
        !suspend_text.contains("\r\n"),
        "suspend wrote native history: {suspend_text:?}"
    );
    assert!(
        !suspend.windows(4).any(|window| window == b"\x1b[2J")
            && !suspend.windows(4).any(|window| window == b"\x1b[3J"),
        "suspend must not erase the normal screen: {suspend_text:?}"
    );

    terminal
        .resume(80, 12, 0, 0, 1)
        .expect("resume fullscreen modes");
    let mut redraw = Vec::new();
    terminal
        .render_to(&mut redraw, &frame)
        .expect("redraw after resume");
    assert!(
        !redraw.is_empty(),
        "resumed surface must repaint from the fullscreen origin"
    );

    let mut unchanged = Vec::new();
    terminal
        .render_to(&mut unchanged, &frame)
        .expect("unchanged frame");
    assert!(
        unchanged.is_empty(),
        "repeated frame content must emit no bytes"
    );

    let mut leave = Vec::new();
    terminal.leave(&mut leave).expect("leave fullscreen");
    let leave_text = String::from_utf8_lossy(&leave);
    assert!(
        !leave_text.contains("\r\n"),
        "leave wrote native history: {leave_text:?}"
    );
    assert!(
        leave_text.contains("\x1b[?25h"),
        "leave must restore the cursor: {leave_text:?}"
    );
}

#[test]
fn logical_anchor_survives_growth_removal_resize_and_wrap() {
    let mut pane = TranscriptPane::new(40, 10);
    for index in 0..20 {
        pane.push_status(format!("row-{index}"));
    }
    // Build the layout, then lock upward so the anchor points into a middle
    // entry (row-2's separator row).
    let _ = pane.render_visible_slice(40, 6);
    pane.scroll_transcript_up(30);
    let anchored = pane.render_visible_slice(40, 6);
    assert_eq!(anchored.len(), 6, "the physical slice stays bounded");
    let first_content = non_blank_lines(&anchored);
    assert_eq!(first_content[0], "row-2", "locked slice: {first_content:?}");
    let anchor = pane
        .document()
        .view()
        .anchor
        .expect("upward scroll locks an anchor");
    assert!(!pane.document().is_following_tail());

    // Growth above the anchor: row-0 becomes a long wrapped status. The
    // anchored content must stay put (the anchor is keyed by entry identity
    // and logical position, not an absolute row offset).
    pane.transcript_mut().mutate_entry(0, |entry| {
        if let TranscriptEntry::Status { text, .. } = entry {
            *text = format!("row-0 {}", "grew ".repeat(40));
            true
        } else {
            false
        }
    });
    let grown = pane.render_visible_slice(40, 6);
    let grown_content = non_blank_lines(&grown);
    assert_eq!(grown_content[0], "row-2", "grown slice: {grown_content:?}");
    assert_eq!(
        pane.document().view().anchor,
        Some(anchor),
        "growth above the anchor must not move it"
    );

    // Retry removal of the anchored provisional entry: the anchor falls back
    // to the nearest preceding surviving entry (row-1) while staying locked.
    let anchored_index = pane
        .transcript()
        .entry_ids()
        .iter()
        .position(|id| *id == anchor.entry_id)
        .expect("anchored entry exists");
    pane.transcript_mut().remove(anchored_index);
    let after_removal = pane.render_visible_slice(40, 6);
    let removal_content = non_blank_lines(&after_removal);
    assert_eq!(
        removal_content[0], "row-1",
        "fallback slice: {removal_content:?}"
    );
    assert!(
        !pane.document().is_following_tail(),
        "fallback must remain locked"
    );

    // Resize/reflow: the same anchor resolves against the new wrapping at a
    // narrower width, and the anchored content stays the top logical point.
    let fallback_anchor = pane.document().view().anchor.expect("fallback anchor");
    pane.resize(20, 10);
    let reflowed = pane.render_visible_slice(20, 6);
    let reflow_content = non_blank_lines(&reflowed);
    assert_eq!(
        reflow_content[0], "row-1",
        "reflowed slice: {reflow_content:?}"
    );
    assert_eq!(
        pane.document().view().anchor,
        Some(fallback_anchor),
        "reflow must resolve the same anchor against new wrapping"
    );
    assert!(!pane.document().is_following_tail());
}

#[test]
fn tail_follow_and_locked_scroll_have_one_activity_indicator() {
    let mut pane = TranscriptPane::new(40, 10);
    for index in 0..10 {
        pane.push_status(format!("row-{index}"));
    }
    let _ = pane.render_visible_slice(40, 6);
    assert!(
        !pane.document().view().new_activity,
        "tail-following content growth is visible and needs no indicator"
    );

    // Lock upward, then let later revisions arrive while locked: exactly one
    // Boolean activity indicator is set and the anchor never moves.
    pane.scroll_transcript_up(4);
    assert!(!pane.document().view().new_activity);
    pane.push_status("new-1");
    let _ = pane.render_visible_slice(40, 6);
    let locked_anchor = pane.document().view().anchor;
    assert!(
        pane.document().view().new_activity,
        "one indicator while locked"
    );

    pane.push_status("new-2");
    let _ = pane.render_visible_slice(40, 6);
    assert!(
        pane.document().view().new_activity,
        "still one Boolean, not a counter"
    );
    assert_eq!(
        pane.document().view().anchor,
        locked_anchor,
        "later revisions never move the locked anchor"
    );

    // The indicator is consumed once after output and stays off.
    assert!(pane.consume_new_activity());
    assert!(!pane.document().view().new_activity);

    // Returning to the tail (or any follow-bottom) clears the indicator and
    // later growth stays invisible-to-indicator again.
    pane.scroll_transcript_down(usize::MAX);
    assert!(pane.document().is_following_tail());
    pane.push_status("new-3");
    let _ = pane.render_visible_slice(40, 6);
    assert!(
        !pane.document().view().new_activity,
        "tail follow resolves directly to the new bottom"
    );
    assert!(
        pane.document().total_rows() > 0,
        "the document retains every appended row"
    );
}

#[test]
fn background_delegate_group_updates_offscreen_and_latest_state_is_reachable() {
    let mut pane = TranscriptPane::new(80, 10);
    let runtime = MultiAgentRuntime::new();
    let alpha = runtime.start_foreground_delegate_for_test("alpha task");
    let beta = runtime.start_foreground_delegate_for_test("beta task");

    // The DelegateGroup starts with one child and renders its display name
    // (deterministic pool: "Euclid", then "Archimedes").
    pane.push_transcript(TranscriptEntry::DelegateGroup {
        component: DelegateGroupComponent::new(1, vec![alpha.clone()]),
    });
    for index in 0..4 {
        pane.push_status(format!("after-group-{index}"));
    }
    let _ = pane.render_visible_slice(80, 6);
    // Lock upward into the later status rows: the group sits above the
    // viewport.
    pane.scroll_transcript_up(1);
    let locked_anchor = pane
        .document()
        .view()
        .anchor
        .expect("upward scroll locks an anchor");
    assert!(!pane.document().is_following_tail());
    let status_start_before = pane
        .document()
        .entry_layout(1)
        .expect("first status")
        .start_row;
    let group_height_before = pane.document().entry_layout(0).expect("group").height;

    // More tool activity lands after the group, pushing it further above the
    // viewport; then the background group updates in place while off-screen.
    for index in 4..20 {
        pane.push_status(format!("after-group-{index}"));
    }
    pane.transcript_mut().mutate_entry(0, |entry| {
        if let TranscriptEntry::DelegateGroup { component } = entry {
            *component = DelegateGroupComponent::new(1, vec![alpha.clone(), beta.clone()]);
            true
        } else {
            false
        }
    });
    let _ = pane.render_visible_slice(80, 6);

    // (a) The view stays locked, the anchor never moves, and later revisions
    // set exactly one new-activity indicator.
    assert_eq!(
        pane.document().view().anchor,
        Some(locked_anchor),
        "an off-screen update must not move the locked anchor"
    );
    assert!(!pane.document().is_following_tail());
    assert!(
        pane.document().view().new_activity,
        "off-screen revisions set the activity indicator"
    );

    // (c) The document geometry stays consistent: existing later entries'
    // start rows shift by exactly the group's height delta.
    let status_start_after = pane
        .document()
        .entry_layout(1)
        .expect("first status")
        .start_row;
    let group_height_after = pane.document().entry_layout(0).expect("group").height;
    let height_delta = group_height_after.saturating_sub(group_height_before);
    assert!(
        height_delta > 0,
        "the updated group must grow: {height_delta}"
    );
    assert_eq!(
        status_start_after.saturating_sub(status_start_before),
        height_delta,
        "existing later entries shift by exactly the group's height delta"
    );

    // (b) Scrolling back to the group renders its LATEST full state: the
    // second child appears even though the update happened off-screen.
    pane.scroll_transcript_up(usize::MAX);
    let top = pane.render_visible_slice(80, 6);
    let top_text = non_blank_lines(&top).join("\n");
    assert!(top_text.contains("Euclid"), "group slice: {top_text}");
    assert!(
        top_text.contains("Archimedes"),
        "latest full state must be reachable: {top_text}"
    );
    // The full document composes to the same virtual geometry and also
    // carries the latest state.
    let full = pane.render_frame(80, 10).expect("full frame");
    assert_eq!(pane.document().total_rows(), full.len());
    let full_text = full
        .iter()
        .map(|row| strip_ansi(row))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        full_text.contains("Archimedes"),
        "latest state in full document: {full_text}"
    );
}

#[test]
fn tool_run_suppression_toggle_remesures_heights_and_keeps_geometry_exact() {
    let mut pane = TranscriptPane::new(80, 10);
    // Three consecutive solo tool cards render as one grouped block; a
    // trailing status tracks the shift of later entries.
    for id in ["tool-1", "tool-2", "tool-3"] {
        pane.transcript_mut().push_tool_run(id, "Bash", None);
    }
    pane.push_status("after tools");
    let full_before = pane.render_frame(80, 10).expect("full frame");
    let total_before = pane.document().total_rows();
    assert_eq!(total_before, full_before.len());
    assert_eq!(
        full_before
            .iter()
            .filter(|row| strip_ansi(row).contains("Using Bash"))
            .count(),
        3,
        "all three tool cards render before suppression"
    );
    let status_start_before = pane
        .document()
        .entry_layout(3)
        .expect("trailing status")
        .start_row;

    // Suppressing the middle tool re-shapes the group without any content
    // mutation: the document must re-measure the affected span so the
    // virtual geometry stays exact.
    pane.transcript_mut().suppress_tool_run("tool-2");
    let suppressed_full = pane.render_frame(80, 10).expect("full frame");
    let total_after = pane.document().total_rows();
    assert!(
        total_after < total_before,
        "suppression must shrink the document: {total_before} -> {total_after}"
    );
    assert_eq!(total_after, suppressed_full.len());
    assert_eq!(
        suppressed_full
            .iter()
            .filter(|row| strip_ansi(row).contains("Using Bash"))
            .count(),
        2,
        "the suppressed placeholder card is gone"
    );
    // The trailing status shifts up by exactly the removed rows.
    let status_start_after = pane
        .document()
        .entry_layout(3)
        .expect("trailing status")
        .start_row;
    assert_eq!(
        status_start_before.saturating_sub(status_start_after),
        total_before.saturating_sub(total_after),
        "later entries shift by exactly the re-measured height delta"
    );

    // Unsuppressing restores the exact original geometry.
    pane.transcript_mut().unsuppress_tool_run("tool-2");
    let restored_full = pane.render_frame(80, 10).expect("full frame");
    assert_eq!(pane.document().total_rows(), total_before);
    assert_eq!(restored_full.len(), total_before);
    assert_eq!(
        restored_full
            .iter()
            .filter(|row| strip_ansi(row).contains("Using Bash"))
            .count(),
        3,
        "all three tool cards render again after unsuppression"
    );
}

#[test]
fn dynamic_tool_group_remeasures_after_append_and_member_update() {
    let mut pane = TranscriptPane::new(80, 16);
    pane.push_status("before");
    pane.transcript_mut().push_tool_run("tool-1", "Bash", None);

    // Stage 1 — a solo tool card: the document is exact, and the group is
    // framed by exactly one document-owned blank row below the leading entry.
    let full_solo = pane.render_frame(80, 16).expect("solo frame");
    let solo_total = pane.document().total_rows();
    assert_eq!(solo_total, full_solo.len(), "solo geometry exact");
    assert_eq!(count_tool_cards(&full_solo), 1);
    assert!(
        strip_ansi(&full_solo[1]).trim().is_empty(),
        "one blank row below the leading entry: {full_solo:?}"
    );
    let solo_group_height = pane.document().entry_layout(1).expect("group").height;

    // Stage 2 — appending a second tool joins the group: the first member
    // re-measures, the document grows by exactly the group's height delta.
    pane.transcript_mut().push_tool_run("tool-2", "Bash", None);
    let full_pair = pane.render_frame(80, 16).expect("pair frame");
    let pair_total = pane.document().total_rows();
    assert!(
        pair_total > solo_total,
        "group grows: {solo_total} -> {pair_total}"
    );
    assert_eq!(pair_total, full_pair.len(), "pair geometry exact");
    assert_eq!(count_tool_cards(&full_pair), 2, "both cards visible");
    let pair_group_height = pane.document().entry_layout(1).expect("group").height;
    assert_eq!(
        pair_total.saturating_sub(solo_total),
        pair_group_height.saturating_sub(solo_group_height),
        "the growth is entirely the group's re-measured block"
    );

    // Stage 3 — a trailing entry lands after the group: it starts exactly at
    // the re-measured group's end, one blank row below the group block.
    pane.push_status("after");
    let full_trailed = pane.render_frame(80, 16).expect("trailed frame");
    let trailed_total = pane.document().total_rows();
    assert_eq!(trailed_total, full_trailed.len(), "trailed geometry exact");
    assert_single_blank_framing(&full_trailed, "before", "after");
    let trailed_trailing_start = pane.document().entry_layout(3).expect("trailing").start_row;
    assert_eq!(
        trailed_trailing_start.saturating_sub(pair_total),
        0,
        "the trailing entry starts at the group's re-measured end"
    );

    // Stage 4 — the second member becomes a Preparing tool: the member
    // content change re-measures the span without disturbing geometry.
    pane.transcript_mut().mutate_tool("tool-2", |tool| {
        tool.update_call_state("Bash".to_owned(), None, ToolStatusKind::Pending)
    });
    let full_preparing = pane.render_frame(80, 16).expect("preparing frame");
    let preparing_total = pane.document().total_rows();
    assert_eq!(
        preparing_total,
        full_preparing.len(),
        "preparing geometry exact"
    );
    assert!(
        full_preparing
            .iter()
            .any(|row| strip_ansi(row).contains("Preparing Bash")),
        "the second card renders its new status"
    );
    assert_single_blank_framing(&full_preparing, "before", "after");
    let preparing_trailing_start = pane.document().entry_layout(3).expect("trailing").start_row;
    assert_eq!(
        preparing_trailing_start.saturating_sub(trailed_trailing_start),
        preparing_total.saturating_sub(trailed_total),
        "trailing entry tracks the re-measured group"
    );

    // Stage 5 — the second member completes with a two-line result: the
    // group block grows and the trailing entry shifts by exactly that delta.
    pane.transcript_mut().mutate_tool("tool-2", |tool| {
        tool.set_result(
            Some("first line\nsecond line".to_owned()),
            None,
            false,
            Some(0),
        )
    });
    let full_result = pane.render_frame(80, 16).expect("result frame");
    let result_total = pane.document().total_rows();
    assert!(
        result_total > preparing_total,
        "result body grows the group"
    );
    assert_eq!(result_total, full_result.len(), "result geometry exact");
    assert_single_blank_framing(&full_result, "before", "after");
    let result_trailing_start = pane.document().entry_layout(3).expect("trailing").start_row;
    assert_eq!(
        result_trailing_start.saturating_sub(preparing_trailing_start),
        result_total.saturating_sub(preparing_total),
        "trailing entry shifts by exactly the group's height delta"
    );

    // Stage 6 — an approval inserted between the two tools splits the group
    // into two solo groups: both new first members re-measure, the document
    // stays byte-exact, and the trailing entry still tracks the delta.
    pane.transcript_mut()
        .insert_approval_after_tool_or_push(approval_for_tool("tool-1"));
    let full_split = pane.render_frame(80, 16).expect("split frame");
    let split_total = pane.document().total_rows();
    assert!(split_total > result_total, "approval card adds rows");
    assert_eq!(split_total, full_split.len(), "split geometry exact");
    assert_eq!(
        count_tool_cards(&full_split),
        2,
        "both tool cards survive the split"
    );
    assert_single_blank_framing(&full_split, "before", "after");
    let using_idx = full_split
        .iter()
        .position(|row| strip_ansi(row).contains("Using Bash"))
        .expect("first tool card");
    let approval_idx = full_split
        .iter()
        .position(|row| strip_ansi(row).contains("Approve Bash"))
        .expect("approval card");
    let used_idx = full_split
        .iter()
        .position(|row| strip_ansi(row).contains("Used Bash"))
        .expect("second tool card");
    assert!(
        using_idx < approval_idx && approval_idx < used_idx,
        "approval sits between the two tool cards: {full_split:?}"
    );
    let split_trailing_start = pane.document().entry_layout(4).expect("trailing").start_row;
    assert_eq!(
        split_trailing_start.saturating_sub(result_trailing_start),
        split_total.saturating_sub(result_total),
        "trailing entry shifts by exactly the re-measured entries"
    );

    // The tail stays reachable: the bounded visible slice resolves to the
    // document bottom and still shows the last entry.
    assert!(
        pane.document().total_rows() > 6,
        "document is taller than the viewport"
    );
    let tail = pane.render_visible_slice(80, 6);
    assert_eq!(tail.len(), 6, "the physical slice stays bounded");
    assert!(
        tail.iter().any(|row| strip_ansi(row).contains("after")),
        "tail-following slice reaches the last entry: {tail:?}"
    );
}

#[test]
fn removing_tool_run_members_remesures_shrunk_and_handed_off_groups() {
    let mut pane = TranscriptPane::new(80, 16);
    pane.push_status("before");
    for id in ["tool-1", "tool-2", "tool-3"] {
        pane.transcript_mut().push_tool_run(id, "Bash", None);
    }
    pane.push_status("after");

    // Stage 1 — a three-member group; remove the middle member: the group
    // shrinks, the first member re-measures, and the trailing entry shifts
    // up by exactly the removed rows.
    let full_before = pane.render_frame(80, 16).expect("three-member frame");
    let total_before = pane.document().total_rows();
    assert_eq!(
        total_before,
        full_before.len(),
        "three-member geometry exact"
    );
    assert_eq!(count_tool_cards(&full_before), 3);
    assert_single_blank_framing(&full_before, "before", "after");
    let trailing_start_before = pane.document().entry_layout(4).expect("trailing").start_row;

    pane.transcript_mut().remove(2); // drop "tool-2" from the middle
    let full_shrunk = pane.render_frame(80, 16).expect("shrunk frame");
    let total_shrunk = pane.document().total_rows();
    assert!(
        total_shrunk < total_before,
        "group shrinks: {total_before} -> {total_shrunk}"
    );
    assert_eq!(total_shrunk, full_shrunk.len(), "shrunk geometry exact");
    assert_eq!(
        count_tool_cards(&full_shrunk),
        2,
        "the two remaining cards render"
    );
    assert_single_blank_framing(&full_shrunk, "before", "after");
    let trailing_start_shrunk = pane.document().entry_layout(3).expect("trailing").start_row;
    assert_eq!(
        trailing_start_before.saturating_sub(trailing_start_shrunk),
        total_before.saturating_sub(total_shrunk),
        "trailing entry shifts up by exactly the removed rows"
    );

    // Stage 2 — remove the group's first member: the block ownership hands
    // to the following member, which re-measures from zero to a solo card.
    pane.transcript_mut().remove(1); // drop "tool-1"
    let full_handed = pane.render_frame(80, 16).expect("handed-off frame");
    let total_handed = pane.document().total_rows();
    assert!(
        total_handed < total_shrunk,
        "handoff shrinks: {total_shrunk} -> {total_handed}"
    );
    assert_eq!(total_handed, full_handed.len(), "handoff geometry exact");
    assert_eq!(
        count_tool_cards(&full_handed),
        1,
        "the survivor renders its own card"
    );
    assert_single_blank_framing(&full_handed, "before", "after");
    let trailing_start_handed = pane.document().entry_layout(2).expect("trailing").start_row;
    assert_eq!(
        trailing_start_shrunk.saturating_sub(trailing_start_handed),
        total_shrunk.saturating_sub(total_handed),
        "trailing entry shifts up by exactly the handed-off rows"
    );

    // The tail stays reachable through the bounded visible slice.
    let tail = pane.render_visible_slice(80, 6);
    assert_eq!(
        tail.len(),
        pane.document().total_rows().min(6),
        "the physical slice stays bounded"
    );
    assert!(
        tail.iter().any(|row| strip_ansi(row).contains("after")),
        "tail-following slice reaches the last entry: {tail:?}"
    );
}

#[test]
fn removing_non_tool_between_tool_groups_merges_and_remesures() {
    let mut pane = TranscriptPane::new(80, 16);
    pane.push_status("before");
    pane.transcript_mut().push_tool_run("tool-1", "Bash", None);
    pane.transcript_mut().push_tool_run("tool-2", "Bash", None);
    pane.push_status("after");

    // Split the group with an approval between the two tools: two solo
    // groups with an approval card between them.
    pane.transcript_mut()
        .insert_approval_after_tool_or_push(approval_for_tool("tool-1"));
    let full_split = pane.render_frame(80, 16).expect("split frame");
    let total_split = pane.document().total_rows();
    assert_eq!(total_split, full_split.len(), "split geometry exact");
    assert_eq!(count_tool_cards(&full_split), 2);
    assert_single_blank_framing(&full_split, "before", "after");
    let trailing_start_split = pane.document().entry_layout(4).expect("trailing").start_row;
    let approval_index = pane
        .transcript()
        .entries()
        .iter()
        .position(|entry| matches!(entry, TranscriptEntry::ApprovalPrompt { .. }))
        .expect("approval entry");

    // Removing the approval merges the two groups into one: the first
    // member re-measures the merged block and the trailing entry shifts up
    // by exactly the removed rows.
    pane.transcript_mut().remove(approval_index);
    let full_merged = pane.render_frame(80, 16).expect("merged frame");
    let total_merged = pane.document().total_rows();
    assert!(
        total_merged < total_split,
        "merge shrinks: {total_split} -> {total_merged}"
    );
    assert_eq!(total_merged, full_merged.len(), "merged geometry exact");
    assert_eq!(
        count_tool_cards(&full_merged),
        2,
        "both cards render in the merged block"
    );
    assert_single_blank_framing(&full_merged, "before", "after");
    let trailing_start_merged = pane.document().entry_layout(3).expect("trailing").start_row;
    assert_eq!(
        trailing_start_split.saturating_sub(trailing_start_merged),
        total_split.saturating_sub(total_merged),
        "trailing entry shifts up by exactly the removed rows"
    );

    // The tail stays reachable through the bounded visible slice.
    let tail = pane.render_visible_slice(80, 6);
    assert_eq!(tail.len(), 6, "the physical slice stays bounded");
    assert!(
        tail.iter().any(|row| strip_ansi(row).contains("after")),
        "tail-following slice reaches the last entry: {tail:?}"
    );
}
