use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Finalization, strip_ansi};
use neo_tui::transcript::{
    ThinkingPart, ThinkingPhase, TranscriptEntry, TranscriptPane, TranscriptStore,
};

fn thinking_contents(store: &TranscriptStore) -> Vec<String> {
    store
        .entries()
        .iter()
        .filter_map(TranscriptEntry::thinking_content)
        .collect()
}
fn plain_rows(store: &TranscriptStore) -> Vec<String> {
    store
        .render_rows(80, &TuiTheme::default())
        .into_iter()
        .map(|row| strip_ansi(&row.to_ansi()).trim_end().to_owned())
        .collect()
}

#[test]
fn assistant_text_blocks_thinking_coalescing() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    store.append_assistant_delta("visible answer");
    store.finish_assistant();
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking(false);

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
}

#[test]
fn completed_thinking_stays_finalized_when_adjacent_thinking_starts() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    let completed_id = store.entry_ids()[0];
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));

    // Adjacent thinking reopens the completed block so consecutive reasoning
    // events render as one card. The entry is no longer finalized.
    store.start_thinking();
    store.append_thinking_delta("second");

    assert_eq!(thinking_contents(&store), vec!["firstsecond"]);
    assert_eq!(store.entries().len(), 1);
    assert_eq!(store.entry_ids()[0], completed_id);
    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));
}

#[test]
fn empty_thinking_delta_does_not_create_an_entry() {
    let mut store = TranscriptStore::new();

    store.append_thinking_delta("");

    assert!(store.entries().is_empty());
}

#[test]
fn live_and_replayed_redacted_thinking_keep_raw_text_and_render_parity() {
    let mut live = TranscriptPane::new(80, 20);
    live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "redacted-thinking".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: true,
    });

    let TranscriptEntry::ThinkingBlock { parts, .. } = &live.transcript().entries()[0] else {
        panic!("expected one live thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert!(parts[0].text.is_empty());
    assert!(parts[0].redacted);
    assert_eq!(
        live.transcript().entries()[0].thinking_content(),
        Some("[Reasoning redacted]".to_owned())
    );
    let live_rows = plain_rows(live.transcript());
    assert!(
        live_rows
            .iter()
            .any(|row| row.contains("[Reasoning redacted]"))
    );

    let mut replay = TranscriptPane::new(80, 20);
    replay.replay_assistant_content(&[neo_agent_core::Content::thinking_with_kind_and_id(
        "",
        Some("opaque-signature".into()),
        true,
        neo_ai::ThinkingKind::Unknown,
        Some("redacted-thinking".into()),
    )]);

    let TranscriptEntry::ThinkingBlock { parts, .. } = &replay.transcript().entries()[0] else {
        panic!("expected one replayed thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert!(parts[0].text.is_empty());
    assert!(parts[0].redacted);
    assert_eq!(
        replay.transcript().entries()[0].thinking_content(),
        live.transcript().entries()[0].thinking_content()
    );
    assert_eq!(plain_rows(replay.transcript()), live_rows);
}

#[test]
fn multi_part_unknown_thinking_wraps_as_one_display_stream() {
    let parts = vec![
        ThinkingPart::new("abc", None),
        ThinkingPart::new("defgh", None),
    ];

    let complete = TranscriptEntry::ThinkingBlock {
        parts: parts.clone(),
        kind: neo_ai::ThinkingKind::Unknown,
        phase: ThinkingPhase::Complete,
        expanded: false,
    };
    let complete_rows = complete
        .render(6, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    assert_eq!(complete_rows, vec!["● abcd", "   efgh"]);
    assert!(
        complete_rows
            .iter()
            .all(|line| !line.contains("ctrl+o to expand"))
    );

    let streaming = TranscriptEntry::ThinkingBlock {
        parts,
        kind: neo_ai::ThinkingKind::Unknown,
        phase: ThinkingPhase::Streaming,
        expanded: false,
    };
    let streaming_rows = streaming
        .render(6, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    assert_eq!(streaming_rows, vec!["⠋ thinking...", "  abcd", "  efgh"]);
}

#[test]
fn replayed_empty_id_thinking_part_is_retained() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[neo_agent_core::Content::thinking_with_kind_and_id(
        "",
        None,
        false,
        neo_ai::ThinkingKind::Summary,
        Some("empty-summary".into()),
    )]);

    let entries = pane.transcript().entries();
    assert_eq!(entries.len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &entries[0] else {
        panic!("expected one empty thinking block");
    };
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].id.as_deref(), Some("empty-summary"));
    assert!(parts[0].text.is_empty());

    let mut historical = TranscriptPane::new(80, 20);
    historical.replay_assistant_content(&[neo_agent_core::Content::thinking("", None, false)]);
    assert!(historical.transcript().entries().is_empty());
}

#[test]
fn thinking_finishes_in_place_without_creating_a_second_entry() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("alpha\nbeta\ngamma");
    assert_eq!(store.entries().len(), 1);

    store.finish_thinking(false);
    let rows = plain_rows(&store);

    assert_eq!(store.entries().len(), 1);
    assert!(rows.iter().any(|row| row.contains("● alpha")));
    assert!(rows.iter().any(|row| row.contains("1 more lines")));
}

#[test]
fn tool_runs_block_thinking_coalescing() {
    let mut store = TranscriptStore::new();

    store.start_thinking();
    store.append_thinking_delta("first");
    store.finish_thinking(false);
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));
    store.start_thinking();
    store.append_thinking_delta("second");
    store.finish_thinking(false);

    assert_eq!(thinking_contents(&store), vec!["first", "second"]);
    assert_eq!(store.entries().len(), 3);
}
