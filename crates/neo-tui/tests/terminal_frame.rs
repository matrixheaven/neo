use std::path::PathBuf;
use std::time::Instant;

use neo_tui::NeoTui;
use neo_tui::dialogs::{ChoiceItem, ChoicePickerOptions, ConfirmDialogOptions, HelpPanelCommand};
use neo_tui::input::{InputEvent, KeybindingAction};
use neo_tui::primitive::{TuiTheme, strip_ansi, visible_width};
use neo_tui::screen_output::TerminalFrame;
use neo_tui::shell::{NeoChromeState, PromptEdit};
use neo_tui::tasks_browser::TaskBrowserState;
use neo_tui::transcript::TranscriptEntry;

#[test]
fn interactive_frame_is_one_bounded_fullscreen_document() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_status("committed status");
    transcript.start_assistant_message();
    transcript.append_assistant_delta("streaming tail");
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame(80, 12);
    let text = frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    // One bounded frame: the visible document slice plus fitted chrome.
    assert!(frame.lines.len() <= 12, "frame rows: {}", frame.lines.len());
    assert!(text.contains("committed status"), "frame: {text}");
    assert!(text.contains("streaming tail"), "frame: {text}");
    assert!(
        frame
            .cursor
            .is_none_or(|cursor| cursor.row < frame.lines.len() && cursor.row < 12),
        "cursor must stay inside the bounded frame: {:?}",
        frame.cursor
    );
    assert!(
        frame.lines.iter().all(|line| visible_width(line) <= 80),
        "every frame line must fit the terminal width"
    );
}

#[test]
fn visible_footer_working_state_requests_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    let mut tui = NeoTui::new(chrome, transcript);
    tui.chrome_mut().set_shell_running(true);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn rendering_at_the_same_instant_does_not_advance_a_thinking_spinner() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_streaming("working it out"));
    let mut tui = NeoTui::new(chrome, transcript);
    let now = Instant::now();

    let first = tui.render_terminal_frame_at(80, 12, now).lines.join("\n");
    let second = tui.render_terminal_frame_at(80, 12, now).lines.join("\n");

    assert_eq!(first, second);
}

#[test]
fn frame_is_bounded_when_chrome_exhausts_terminal_height() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 4);
    transcript.start_assistant_message();
    transcript.append_assistant_delta("live assistant output");
    let mut tui = NeoTui::new(chrome, transcript);

    for height in 1..=4 {
        let frame = tui.render_terminal_frame(40, height);
        assert!(
            frame.lines.len() <= height,
            "height {height} produced {} rows",
            frame.lines.len()
        );
        // The defensive fit trims above the footer, never the actionable
        // status line at the bottom of the chrome.
        let last = strip_ansi(frame.lines.last().expect("bounded frame has a footer"));
        assert!(
            last.contains("[ask]"),
            "footer status must survive the fit at height {height}: {last:?}"
        );
    }
}

#[test]
fn streaming_thinking_requests_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_streaming("still thinking"));
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_some());
}

#[test]
fn running_file_write_advances_transcript_animation_state() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
    });
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolCallArgumentsDelta {
        turn: 1,
        id: "write-1".to_owned(),
        json_fragment: r#"{"path":"notes.txt","content":"draft"}"#.to_owned(),
    });
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt", "content": "draft"}),

        workflow_origin: None,
        output_ref: None,
    });
    let mut tui = NeoTui::new(chrome, transcript);
    let now = Instant::now();
    let frame = tui.render_terminal_frame_at(80, 12, now);
    assert!(frame.next_animation_deadline.is_some());
    assert!(!tui.is_transcript_dirty());

    tui.advance_animation_at(now);

    assert!(tui.is_transcript_dirty());
}

#[test]
fn running_static_tool_does_not_request_an_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt"}),

        workflow_origin: None,
        output_ref: None,
    });
    let mut tui = NeoTui::new(chrome, transcript);

    let frame = tui.render_terminal_frame_at(80, 12, Instant::now());

    assert!(frame.next_animation_deadline.is_none());
}

#[test]
fn running_sleep_requests_animation_deadline() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "sleep-anim".to_owned(),
        name: "Sleep".to_owned(),
        arguments: serde_json::json!({
            "duration_seconds": 45,
            "reason": "wait for cooldown"
        }),

        workflow_origin: None,
        output_ref: None,
    });
    let mut tui = NeoTui::new(chrome, transcript);

    let running = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(
        running.next_animation_deadline.is_some(),
        "running Sleep must request animation deadline"
    );

    tui.transcript_mut()
        .apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "sleep-anim".to_owned(),
            name: "Sleep".to_owned(),
            result: neo_agent_core::ToolResult::ok("Waited 45 seconds: wait for cooldown"),

            workflow_origin: None,
            output_ref: None,
        });
    let finished = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(
        finished.next_animation_deadline.is_none(),
        "completed Sleep must not request animation deadline"
    );
}

fn push_overflowing_live_suffix(transcript: &mut neo_tui::transcript::TranscriptPane) {
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "overflow-live-tool".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "overflow-living-command" }),

        workflow_origin: None,
        output_ref: None,
    });
    let body = (0..40)
        .map(|index| format!("overflow-source-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    transcript.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: "overflow-live-tool".to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(body),

        workflow_origin: None,
        output_ref: None,
    });
}

#[test]
fn tall_document_slice_stays_bounded_in_the_fullscreen_frame() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 8);
    push_overflowing_live_suffix(&mut transcript);
    let mut tui = NeoTui::new(chrome, transcript);

    // A tall live workload renders as one bounded document slice inside the
    // already-active fullscreen surface: no second surface, no mouse flag,
    // and the visible slice stays frame-safe.
    let frame = tui.render_terminal_frame_at(40, 8, Instant::now());
    let text = frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!frame.lines.is_empty());
    assert!(
        frame.lines.len() <= 8,
        "slice must stay bounded by the terminal height: {}",
        frame.lines.len()
    );
    assert!(
        frame
            .cursor
            .is_none_or(|cursor| cursor.row < frame.lines.len() && cursor.row < 8),
        "cursor must stay inside the bounded frame: {:?}",
        frame.cursor
    );
    assert!(
        text.contains("[ask]") || text.contains("ask"),
        "chrome missing: {text}"
    );
    assert!(!text.contains("earlier rows omitted"), "frame: {text}");
    // Tail follow shows the newest output rows of the tall card; the card
    // header stays reachable by scrolling up the document.
    assert!(
        text.contains("overflow-source-sentinel-39"),
        "newest output row missing: {text}"
    );
    tui.transcript_mut().scroll_transcript_up(usize::MAX);
    let top = tui.render_terminal_frame_at(40, 8, Instant::now());
    let top_text = top
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        top_text.contains("Using Bash") || top_text.contains("overflow-living"),
        "the living card header is reachable by scrolling: {top_text}"
    );
    assert!(top.lines.len() <= 8);
}

#[test]
fn blocking_overlays_render_inside_the_active_fullscreen_frame() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(40, 8);
    transcript.push_status("history status");
    let mut tui = NeoTui::new(chrome, transcript);

    let plain = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(plain.lines.len() <= 8);

    // Task Browser is an overlay inside the already-fullscreen session: the
    // frame is still one bounded line set with no physical transition.
    tui.chrome_mut()
        .push_task_browser_overlay(TaskBrowserState::new());
    let overlay = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(overlay.lines.len() <= 8);
    assert!(!overlay.lines.is_empty());

    tui.chrome_mut().close_focused_overlay();
    let restored = tui.render_terminal_frame_at(40, 8, Instant::now());
    assert!(restored.lines.len() <= 8);

    tui.chrome_mut().open_help_panel(vec![HelpPanelCommand::new(
        "/help",
        Some("Show help information"),
    )]);
    let dialog = tui.render_terminal_frame_at(40, 8, Instant::now());
    let dialog_text = dialog
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(dialog.lines.len() <= 8);
    assert!(
        dialog_text.contains(" help "),
        "help title must stay visible inside the fullscreen frame: {dialog_text}"
    );
    assert!(
        dialog_text.contains("help · Esc / Enter / q close"),
        "help action hint must stay visible inside the fullscreen frame: {dialog_text}"
    );
}

#[test]
fn short_terminal_preserves_dialog_title_selection_and_actions() {
    // Blocking rich dialogs must slice themselves to the actual available
    // height: the title, the current selection, and the action hints stay
    // visible (or scroll-reachable) instead of being drained from the top by
    // `fit_chrome_to_height`.
    for height in [5usize, 8usize] {
        // Help panel: title and action hint are visible up front; the command
        // list is reachable by scrolling and the title survives scrolling.
        let mut tui = NeoTui::new(
            NeoChromeState::new("neo", "session", "model", PathBuf::from(".")),
            neo_tui::transcript::TranscriptPane::new(40, height),
        );
        tui.chrome_mut().open_help_panel(vec![
            HelpPanelCommand::new("/model", Some("Choose model")),
            HelpPanelCommand::new("/skill:rust", Some("Use Rust skill")),
        ]);
        let frame = tui.render_terminal_frame_at(40, height, Instant::now());
        let text = frame_text(&frame);
        assert!(
            frame.lines.len() <= height,
            "help panel frame rows at height {height}: {}",
            frame.lines.len()
        );
        assert!(
            text.contains(" help "),
            "help title missing at height {height}: {text}"
        );
        assert!(
            text.contains("help · Esc / Enter / q close"),
            "help action hint missing at height {height}: {text}"
        );
        // Eight scroll steps reach the slash-commands section at both heights
        // (5-row viewport: one content line, offset 8; 8-row viewport: four
        // content lines, offset clamped to the tail window).
        for _ in 0..8 {
            let _ = tui
                .chrome_mut()
                .handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectDown));
        }
        let scrolled = tui.render_terminal_frame_at(40, height, Instant::now());
        let scrolled_text = frame_text(&scrolled);
        assert!(
            scrolled.lines.len() <= height,
            "scrolled help panel frame rows at height {height}: {}",
            scrolled.lines.len()
        );
        assert!(
            scrolled_text.contains("Slash Commands"),
            "help body must be scroll-reachable at height {height}: {scrolled_text}"
        );
        assert!(
            scrolled_text.contains(" help "),
            "help title must survive scrolling at height {height}: {scrolled_text}"
        );

        // Confirm dialog: title and action hint always visible; the body
        // lines stay visible when the terminal has room for them.
        let mut tui = NeoTui::new(
            NeoChromeState::new("neo", "session", "model", PathBuf::from(".")),
            neo_tui::transcript::TranscriptPane::new(40, height),
        );
        tui.chrome_mut().open_confirm_dialog(ConfirmDialogOptions {
            id: "toggle-write:/tmp/shared".to_owned(),
            title: "Confirm Write Access".to_owned(),
            hint: "Y approve · N cancel · Esc cancel".to_owned(),
            lines: vec![
                " Enable write access for this directory?".to_owned(),
                " /tmp/shared".to_owned(),
            ],
            theme: TuiTheme::default(),
        });
        let frame = tui.render_terminal_frame_at(40, height, Instant::now());
        let text = frame_text(&frame);
        assert!(
            frame.lines.len() <= height,
            "confirm dialog frame rows at height {height}: {}",
            frame.lines.len()
        );
        assert!(
            text.contains("Confirm Write Access"),
            "confirm title missing at height {height}: {text}"
        );
        assert!(
            text.contains("Y approve"),
            "confirm action hint missing at height {height}: {text}"
        );
        if height == 8 {
            assert!(
                text.contains("/tmp/shared"),
                "confirm body must stay visible at height {height}: {text}"
            );
        }

        // Choice picker: title, current selection, and action hint stay
        // visible, and navigation keeps the selection on screen.
        let mut tui = NeoTui::new(
            NeoChromeState::new("neo", "session", "model", PathBuf::from(".")),
            neo_tui::transcript::TranscriptPane::new(40, height),
        );
        tui.chrome_mut().open_choice_picker(ChoicePickerOptions {
            title: "Choose an option".to_owned(),
            items: vec![
                ChoiceItem::new("a", "Option A"),
                ChoiceItem::new("b", "Option B"),
                ChoiceItem::new("c", "Option C"),
                ChoiceItem::new("d", "Option D"),
            ],
            initial_id: Some("b".to_owned()),
            page_size: 0,
            current_id: Some("b".to_owned()),
            theme: TuiTheme::default(),
        });
        let frame = tui.render_terminal_frame_at(40, height, Instant::now());
        let text = frame_text(&frame);
        assert!(
            frame.lines.len() <= height,
            "choice picker frame rows at height {height}: {}",
            frame.lines.len()
        );
        assert!(
            text.contains("Choose an option"),
            "picker title missing at height {height}: {text}"
        );
        assert!(
            text.contains("B ← current"),
            "current selection missing at height {height}: {text}"
        );
        assert!(
            text.contains("↑↓ navigate · Enter select"),
            "picker action hint missing at height {height}: {text}"
        );
        let _ = tui
            .chrome_mut()
            .handle_focused_dialog_input(InputEvent::Action(KeybindingAction::SelectDown));
        let moved = tui.render_terminal_frame_at(40, height, Instant::now());
        assert!(
            moved.lines.len() <= height,
            "moved picker frame rows at height {height}: {}",
            moved.lines.len()
        );
        assert!(
            frame_text(&moved).contains("▸ Option C"),
            "selection must stay visible after navigation at height {height}: {}",
            frame_text(&moved)
        );
    }
}

/// The frame's visible text with ANSI escapes stripped, for content asserts.
fn frame_text(frame: &TerminalFrame) -> String {
    frame
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn transcript_selection_does_not_reduce_visible_body_height() {
    let chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    // One status block of twenty short lines: a contiguous, deterministic
    // document run whose last visible row measures the body height.
    transcript.push_status(
        (0..20)
            .map(|index| format!("line-{index:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let mut tui = NeoTui::new(chrome, transcript);

    // Establish the visible body height while following the tail.
    let tail = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(tail.lines.len() <= 12);

    // Locking the viewport and receiving later output must not add status rows.
    tui.transcript_mut().scroll_transcript_up(usize::MAX);
    let locked = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert!(locked.lines.len() <= 12);
    tui.transcript_mut().push_status("new-1");
    let updated = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert_eq!(
        last_visible_line_row(&locked),
        last_visible_line_row(&updated),
        "new output below a locked viewport must not reduce the body height"
    );

    // Selecting transcript text must keep the exact same visible body range.
    tui.transcript_mut().select_visible_transcript_entry();
    let selected = tui.render_terminal_frame_at(80, 12, Instant::now());
    assert_eq!(
        last_visible_line_row(&updated),
        last_visible_line_row(&selected),
        "selection must not displace transcript rows"
    );
    assert!(
        selected
            .lines
            .iter()
            .map(|line| strip_ansi(line))
            .all(|line| !line.contains("selected ·") && !line.contains("new activity")),
        "selection and activity help must not consume frame rows"
    );
}

/// The index of the last visible `line-NN` document row in a frame, used to
/// prove a state change does not alter body height (the locked window shows a
/// contiguous run of the same status block).
fn last_visible_line_row(frame: &TerminalFrame) -> usize {
    frame
        .lines
        .iter()
        .rev()
        .find_map(|line| {
            strip_ansi(line)
                .split_whitespace()
                .rev()
                .find_map(|word| word.strip_prefix("line-").and_then(|n| n.parse().ok()))
        })
        .expect("the locked status block is visible")
}

#[test]
fn expansion_toggle_resizes_the_primary_document_slice() {
    let mut chrome = NeoChromeState::new("neo", "session", "model", PathBuf::from("."));
    chrome.prompt_mut().apply_edit(PromptEdit::Insert("draft"));
    let mut transcript = neo_tui::transcript::TranscriptPane::new(80, 12);
    transcript.push_transcript(TranscriptEntry::thinking_complete(
        (1..=20)
            .map(|index| format!("expanded-line-{index}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    let mut tui = NeoTui::new(chrome, transcript);

    let collapsed = tui.render_terminal_frame_at(80, 12, Instant::now());
    let collapsed_text = collapsed
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !collapsed_text.contains("expanded-line-20"),
        "collapsed frame: {collapsed_text}"
    );

    tui.transcript_mut().toggle_tool_output_expanded();
    let expanded = tui.render_terminal_frame_at(80, 12, Instant::now());
    let expanded_text = expanded
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        expanded_text.contains("expanded-line-20"),
        "expanded frame: {expanded_text}"
    );
    assert!(expanded_text.contains("draft"), "frame: {expanded_text}");
    assert!(expanded_text.contains("[ask]"), "frame: {expanded_text}");
    assert_eq!(
        expanded.lines.len(),
        12,
        "fullscreen frame fills the height"
    );
}
