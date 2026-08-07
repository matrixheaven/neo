use neo_tui::primitive::strip_ansi;
use neo_tui::transcript::TranscriptPane;

fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}

#[test]
fn persisted_message_events_do_not_duplicate_live_transcript() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::user_text("hello"),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "world".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageAppended {
        message: neo_agent_core::AgentMessage::assistant(
            [neo_agent_core::Content::text("world")],
            [],
            neo_agent_core::StopReason::EndTurn,
        ),
    });

    let frame = plain_frame(&mut transcript_pane, 80, 12);

    assert_eq!(
        frame
            .iter()
            .filter(|line| line.contains("✨") && line.contains("hello"))
            .count(),
        1,
        "user prompt should appear once: {frame:?}"
    );
    assert_eq!(
        frame
            .iter()
            .filter(|line| line.contains("●") && line.contains("world"))
            .count(),
        1,
        "assistant text should appear once: {frame:?}"
    );
}

#[test]
fn replay_renders_user_text_that_looks_like_system_reminder() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.replay_message(&neo_agent_core::AgentMessage::user_text(
        "<system-reminder>\nliteral user text\n</system-reminder>",
    ));

    let frame = plain_frame(&mut transcript_pane, 80, 12);

    assert!(
        frame.iter().any(|line| line.contains("<system-reminder>"))
            && frame.iter().any(|line| line.contains("literal user text")),
        "literal user text should render even when it resembles a system reminder: {frame:?}"
    );
}

#[test]
fn replay_skips_injection_origin_messages() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.replay_message(&neo_agent_core::AgentMessage::injection_text(
        "Plan mode is active. This should stay model-only.",
        "plan_mode",
    ));

    let Some(rendered) = transcript_pane.render_frame(80, 12) else {
        return;
    };
    let frame = rendered.iter().map(|line| plain(line)).collect::<Vec<_>>();

    assert!(
        frame.iter().all(
            |line| !line.contains("<system-reminder>") && !line.contains("Plan mode is active")
        ),
        "runtime system reminder should not be rendered in transcript: {frame:?}"
    );
}

#[test]
fn replayed_messages_render_through_same_transcript_pane_path() {
    let mut transcript_pane = TranscriptPane::new(80, 12);
    transcript_pane.replay_user_message("previous prompt");
    transcript_pane.replay_assistant_message("previous answer");
    transcript_pane.mark_dirty();

    let frame = plain_frame(&mut transcript_pane, 80, 12);
    assert!(
        frame
            .iter()
            .any(|l| l.contains("✨") && l.contains("previous prompt"))
    );
    assert!(frame.iter().any(|l| l.contains("●")));
    assert!(frame.iter().any(|l| l.contains("previous answer")));
}
