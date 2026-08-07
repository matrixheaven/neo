//! Fullscreen transcript document: incremental layout, the logical scroll
//! anchor, tail-follow vs. locked scroll, and bounded visible-slice
//! resolution.

use neo_agent_core::multi_agent::MultiAgentRuntime;
use neo_tui::primitive::strip_ansi;
use neo_tui::transcript::{DelegateGroupComponent, TranscriptEntry, TranscriptPane};

fn non_blank_lines(rows: &[String]) -> Vec<String> {
    rows.iter()
        .map(|row| strip_ansi(row))
        .filter(|row| !row.trim().is_empty())
        .collect()
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
