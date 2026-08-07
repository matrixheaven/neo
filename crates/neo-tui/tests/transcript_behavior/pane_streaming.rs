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
fn finishing_streaming_assistant_preserves_body_row_shape() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hello".to_owned(),
    });

    let live = plain_frame(&mut transcript_pane, 80, 12);
    let live_user = live
        .iter()
        .position(|line| line.contains("✨") && line.contains("hello"))
        .expect("live user row");
    let live_assistant = live
        .iter()
        .position(|line| line.contains("●") && line.contains("Hello"))
        .expect("live assistant row");
    assert_eq!(
        live_assistant,
        live_user + 2,
        "live assistant should be separated from the user by one blank row: {live:?}"
    );
    assert_eq!(live[live_user + 1], "");

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });

    let finished = plain_frame(&mut transcript_pane, 80, 12);
    let finished_user = finished
        .iter()
        .position(|line| line.contains("✨") && line.contains("hello"))
        .expect("finished user row");
    let finished_assistant = finished
        .iter()
        .position(|line| line.contains("●") && line.contains("Hello"))
        .expect("finished assistant row");
    assert_eq!(
        finished_assistant,
        finished_user + 2,
        "finished assistant should keep the live row shape: {finished:?}"
    );
    assert_eq!(finished[finished_user + 1], "");
}

#[test]
fn message_started_does_not_create_empty_assistant_entry() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });

    assert!(
        transcript_pane.transcript().entries().is_empty(),
        "assistant entry should be created by the first text delta, not MessageStarted"
    );
}

#[test]
fn streaming_assistant_grows_past_ten_viewports_without_omission() {
    let mut pane = TranscriptPane::new(80, 10);
    pane.start_assistant_message();
    // Grow one streaming assistant far beyond ten viewports of body height.
    for index in 0..200 {
        pane.append_assistant_delta(&format!("complete paragraph {index}\n\n"));
    }
    // The physical slice stays bounded...
    let slice = pane.render_visible_slice(80, 6);
    assert_eq!(slice.len(), 6, "physical slice must stay bounded");
    // ...while the document retained every row: full-frame composition must
    // match the virtual geometry exactly.
    let full = pane.render_frame(80, 10).expect("full frame");
    assert_eq!(pane.document().total_rows(), full.len());
    assert!(
        pane.document().total_rows() > 6 * 10,
        "content must grow far past ten viewports: {}",
        pane.document().total_rows()
    );
    // Tail follow shows the newest content.
    let tail_text = slice
        .iter()
        .map(|line| plain(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        tail_text.contains("complete paragraph 199"),
        "tail:\n{tail_text}"
    );
    // Scrolling to the very top still finds the oldest content: nothing was
    // omitted by the bounded physical slice.
    pane.scroll_transcript_up(usize::MAX);
    let top = pane.render_visible_slice(80, 6);
    let top_text = top
        .iter()
        .map(|line| plain(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        top_text.contains("complete paragraph 0"),
        "top:\n{top_text}"
    );
}

#[test]
fn streaming_assistant_slice_shows_only_the_bounded_tail() {
    let mut pane = TranscriptPane::new(40, 8);
    pane.start_assistant_message();
    for index in 0..8 {
        pane.append_assistant_delta(&format!("complete paragraph {index}\n\n"));
    }
    pane.append_assistant_delta("mutable tail that is still streaming");

    let slice = pane.render_visible_slice(40, 8);
    let text = slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        slice.len() <= 8,
        "physical slice must stay bounded: {}",
        slice.len()
    );
    assert!(!text.contains("complete paragraph 0"));
    assert!(text.contains("mutable tail"), "slice:\n{text}");
}

#[test]
fn text_after_tool_starts_a_new_assistant_entry_after_the_tool() {
    let mut transcript_pane = TranscriptPane::new(80, 16);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingStarted {
        turn: 1,
        id: "thinking-1".to_owned(),
        kind: neo_ai::ThinkingKind::Unknown,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingDelta {
        turn: 1,
        text: "I should inspect files".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ThinkingFinished {
        turn: 1,
        signature: None,
        redacted: false,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "pwd" }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok("Cargo.toml"),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Final answer".to_owned(),
    });

    let frame = plain_frame(&mut transcript_pane, 80, 16);
    let thinking = frame
        .iter()
        .position(|l| l.contains("I should inspect files"))
        .expect("thinking");
    let tool = frame
        .iter()
        .position(|l| l.contains("Used Bash"))
        .expect("tool");
    let answer = frame
        .iter()
        .position(|l| l.contains("●") && l.contains("Final answer"))
        .expect("answer");
    assert!(
        thinking < tool,
        "thinking should stay above the tool: {frame:?}"
    );
    assert!(
        tool < answer,
        "answer should render after the tool: {frame:?}"
    );
}

#[test]
fn transcript_pane_finishes_streaming_assistant_once_without_duplicate() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "hello".to_owned(),
    });
    let live = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        live.iter()
            .filter(|l| l.contains("●") && l.contains("hello"))
            .count(),
        1,
        "live assistant text appears once with bullet: {live:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let finished = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        finished
            .iter()
            .filter(|l| l.contains("●") && l.contains("hello"))
            .count(),
        1,
        "finished assistant text appears exactly once: {finished:?}"
    );
}

#[test]
fn transcript_pane_keeps_streaming_assistant_in_transcript_until_finished() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Hel".to_owned(),
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "lo".to_owned(),
    });

    let first = plain_frame(&mut transcript_pane, 80, 12);
    assert!(
        first
            .iter()
            .any(|l| l.contains("✨") && l.contains("hello"))
    );
    assert!(
        first.iter().any(|l| l.contains("●") && l.contains("Hello")),
        "live assistant text should already use the finished assistant layout: {first:?}"
    );

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::MessageFinished {
        phase: neo_ai::MessagePhase::Unknown,
        turn: 1,
        id: "assistant-1".to_owned(),
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    let second = plain_frame(&mut transcript_pane, 80, 12);
    assert_eq!(
        second
            .iter()
            .filter(|l| l.contains("●") && l.contains("Hello"))
            .count(),
        1,
        "finished assistant text appears exactly once: {second:?}"
    );
}

#[test]
fn transcript_pane_maps_user_and_assistant_events_to_transcript_entries() {
    let mut transcript_pane = TranscriptPane::new(80, 12);

    transcript_pane.push_user_message("hello");
    transcript_pane.push_assistant_message("world");
    transcript_pane.mark_dirty();
    let frame = plain_frame(&mut transcript_pane, 80, 12);

    // User message is bullet-led (✨), assistant final is bullet-led (●).
    assert!(
        frame
            .iter()
            .any(|l| l.contains("✨") && l.contains("hello"))
    );
    assert!(frame.iter().any(|l| l.contains("●")));
    assert!(frame.iter().any(|l| l.contains("world")));
}
