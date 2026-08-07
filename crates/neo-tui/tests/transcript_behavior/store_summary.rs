use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::transcript::{TranscriptEntry, TranscriptPane, TranscriptStore};

fn plain_rows(store: &TranscriptStore) -> Vec<String> {
    store
        .render_rows(80, &TuiTheme::default())
        .into_iter()
        .map(|row| strip_ansi(&row.to_ansi()).trim_end().to_owned())
        .collect()
}
fn plain_slice(pane: &mut TranscriptPane) -> Vec<String> {
    pane.render_visible_slice(80, 20)
        .into_iter()
        .map(|line| strip_ansi(&line).trim_end().to_owned())
        .collect()
}

#[test]
fn adjacent_summary_parts_keep_ids_and_compact_visible_projection() {
    let mut live = TranscriptPane::new(80, 20);

    for (id, text) in [("summary-1", "first"), ("summary-2", "second")] {
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
            turn: 1,
            id: id.to_owned(),
            kind: neo_ai::ThinkingKind::Summary,
        });
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
            turn: 1,
            text: text.to_owned(),
        });
        live.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        });
    }

    assert_eq!(live.transcript().entries().len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &live.transcript().entries()[0] else {
        panic!("expected one live thinking block");
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].id.as_deref(), Some("summary-1"));
    assert_eq!(parts[0].text, "first");
    assert_eq!(parts[1].id.as_deref(), Some("summary-2"));
    assert_eq!(parts[1].text, "second");
    assert_eq!(
        live.transcript().entries()[0].thinking_content().as_deref(),
        Some("firstsecond")
    );

    let replayed_parts = vec![
        neo_agent_core::Content::thinking_with_kind_and_id(
            "first",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "second",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ];
    let mut replay = TranscriptPane::new(80, 20);
    replay.replay_assistant_content(&replayed_parts);

    assert_eq!(replay.transcript().entries().len(), 1);
    let TranscriptEntry::ThinkingBlock { parts, .. } = &replay.transcript().entries()[0] else {
        panic!("expected one replayed thinking block");
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0].id.as_deref(), Some("summary-1"));
    assert_eq!(parts[0].text, "first");
    assert_eq!(parts[1].id.as_deref(), Some("summary-2"));
    assert_eq!(parts[1].text, "second");
    assert_eq!(
        plain_rows(replay.transcript()),
        plain_rows(live.transcript())
    );
}

#[test]
fn commentary_and_final_answer_render_as_separate_entries() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "commentary-1".to_owned(),
        phase: neo_ai::MessagePhase::Commentary,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Checking **the files**".to_owned(),
    });

    let live = plain_slice(&mut pane).join("\n");
    assert!(
        live.contains("▸ Checking the files"),
        "commentary live: {live}"
    );
    assert!(
        !live.contains("● Checking"),
        "commentary uses its own marker: {live}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "commentary-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
        phase: neo_ai::MessagePhase::Commentary,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "final-1".to_owned(),
        phase: neo_ai::MessagePhase::FinalAnswer,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "The final **answer**".to_owned(),
    });

    let slice = plain_slice(&mut pane).join("\n");
    let commentary_offset = slice
        .find("▸ Checking the files")
        .expect("commentary remains in the document");
    let final_offset = slice
        .find("● The final answer")
        .expect("final answer remains in the document");
    assert!(
        commentary_offset < final_offset,
        "document order is preserved: {slice}"
    );
    assert!(
        !slice.contains("● Checking"),
        "commentary uses its own marker: {slice}"
    );

    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        turn: 1,
        id: "final-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
        phase: neo_ai::MessagePhase::FinalAnswer,
    });
    let slice = plain_slice(&mut pane).join("\n");
    let commentary_offset = slice
        .find("▸ Checking the files")
        .expect("commentary remains in canonical history");
    let final_offset = slice
        .find("● The final answer")
        .expect("final answer remains in canonical history");
    assert!(
        commentary_offset < final_offset,
        "canonical order is preserved: {slice}"
    );
    let assistant_text = pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::AssistantMessage { content } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_text,
        ["Checking **the files**", "The final **answer**"]
    );
    let direct_rows = plain_rows(pane.transcript()).join("\n");
    assert!(
        direct_rows.contains("▸ Checking the files"),
        "direct store rendering keeps commentary marker: {direct_rows}"
    );
    assert!(
        direct_rows.contains("● The final answer"),
        "direct store rendering keeps final-answer marker: {direct_rows}"
    );
}

#[test]
fn summary_projection_keeps_body_after_leading_title_across_ordered_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "**Plan**\n**Cross",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            " title**\n**Plan**\n**Latest**",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    assert!(pane.toggle_tool_output_expanded());
    let rendered = plain_rows(pane.transcript()).join("\n");
    assert!(rendered.contains("● Plan"), "rendered summary: {rendered}");
    assert!(
        rendered.contains("**Cross"),
        "body marker is retained: {rendered}"
    );
    assert!(
        rendered.contains("title**"),
        "body tail is retained: {rendered}"
    );
    assert!(
        rendered.contains("**Plan**"),
        "later bold body is retained: {rendered}"
    );
    assert!(
        rendered.contains("**Latest**"),
        "later inline bold body is retained: {rendered}"
    );

    let mut streaming = TranscriptPane::new(80, 20);
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-1".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**First**".to_owned(),
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "summary-2".to_owned(),
        kind: neo_ai::ThinkingKind::Summary,
    });
    streaming.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "**Latest**".to_owned(),
    });

    let rendered = plain_rows(streaming.transcript()).join("\n");
    assert!(
        rendered.contains("thinking · Latest"),
        "rendered summary: {rendered}"
    );
}

#[test]
fn summary_projection_keeps_body_without_leading_title_across_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "first fallback\nbody",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "second fallback",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    assert!(pane.toggle_tool_output_expanded());
    let rendered = plain_rows(pane.transcript()).join("\n");
    assert!(
        rendered.contains("● first fallback"),
        "first body line is retained: {rendered}"
    );
    assert!(
        rendered.contains("body"),
        "later body line is retained: {rendered}"
    );
    assert!(
        rendered.contains("second fallback"),
        "second body part is retained: {rendered}"
    );
}

#[test]
fn summary_projection_keeps_unclosed_bold_body_across_parts() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.replay_assistant_content(&[
        neo_agent_core::Content::thinking_with_kind_and_id(
            "**Plan**\n**Pla",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-1".into()),
        ),
        neo_agent_core::Content::thinking_with_kind_and_id(
            "n",
            None,
            false,
            neo_ai::ThinkingKind::Summary,
            Some("summary-2".into()),
        ),
    ]);

    assert!(pane.toggle_tool_output_expanded());
    let rendered = plain_rows(pane.transcript());
    assert!(
        rendered.iter().any(|row| row.contains("● Plan")),
        "leading title is retained: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|row| row.contains("**Pla")),
        "unclosed bold body is retained: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|row| row.trim() == "n"),
        "later body part is retained: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|row| row.trim() == "Pla"),
        "unclosed body is not promoted to a title: {rendered:?}"
    );
}
