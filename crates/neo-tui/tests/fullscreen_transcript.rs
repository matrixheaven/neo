//! Fullscreen transcript document: incremental layout, the logical scroll
//! anchor, tail-follow vs. locked scroll, and bounded visible-slice
//! resolution.

use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_tui::primitive::strip_ansi;
use neo_tui::transcript::{DelegateGroupComponent, TranscriptEntry, TranscriptPane};

/// The non-blank content lines of a rendered slice, in order.
fn non_blank_lines(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| strip_ansi(row))
        .filter(|row| !row.trim().is_empty())
        .collect()
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
