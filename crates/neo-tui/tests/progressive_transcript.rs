//! End-to-end presentation regressions for the progressive native-scrollback
//! transcript behavior: a live area actually bounded by `live_budget`, stable
//! facts keeping canonical order behind ordinary live entries, and a single
//! canonical commit per entry at finalization.

use neo_tui::transcript::{TranscriptEntry, TranscriptPane};

/// Strip ANSI escape sequences so assertions can match rendered text.
fn strip_ansi(line: &str) -> String {
    let mut plain = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(ch);
        }
    }
    plain
}

fn live_text(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
    pane.render_terminal_update(width, height)
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn start_tool(pane: &mut TranscriptPane, id: &str, command: &str) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": command }),
        workflow_origin: None,
    });
}

fn stream_tool_output(pane: &mut TranscriptPane, id: &str, lines: usize) {
    let body = (0..lines)
        .map(|index| format!("tool-output-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionUpdate {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        partial_result: neo_agent_core::ToolResult::ok(body),
        workflow_origin: None,
    });
}

fn finish_tool(pane: &mut TranscriptPane, id: &str, result: &str) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        result: neo_agent_core::ToolResult::ok(result),
        workflow_origin: None,
    });
}

/// A live entry without a progressive projection (an ordinary running tool)
/// must stay bounded by `live_budget` and commit its canonical final card
/// exactly once at finalization — never an unbounded mutable suffix.
#[test]
fn unsupported_live_entry_stays_bounded_and_commits_once() {
    let mut pane = TranscriptPane::new(60, 8);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 20);

    let live = live_text(&mut pane, 60, 8);
    let update = pane.render_terminal_update(60, 8);
    assert!(update.history.is_empty());
    assert!(
        update.live.len() <= 4,
        "live must be bounded by live_budget: {} rows\n{live}",
        update.live.len()
    );
    assert!(live.contains("Using Bash"), "live:\n{live}");

    // Finalization commits the canonical card once and removes the live area.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = pane.render_terminal_update(60, 8);
    assert_eq!(finished.history.len(), 1, "one canonical commit");
    assert!(
        finished.history[0]
            .lines
            .iter()
            .any(|line| strip_ansi(line).contains("Bash")),
        "history: {:?}",
        finished.history[0].lines
    );
    assert!(
        finished.history[0]
            .lines
            .iter()
            .any(|line| strip_ansi(line).contains("done")),
        "history: {:?}",
        finished.history[0].lines
    );
    assert!(finished.live.is_empty());

    // Acknowledged history never replays.
    pane.acknowledge_history(&finished.history);
    assert!(pane.render_terminal_update(60, 8).history.is_empty());
}

/// Stable facts after an ordinary live entry commit to native history in
/// canonical order while the live entry remains mutable and bounded.
#[test]
fn stable_facts_after_ordinary_live_entry_keep_canonical_order() {
    let mut pane = TranscriptPane::new(60, 12);
    start_tool(&mut pane, "bash-1", "make");
    stream_tool_output(&mut pane, "bash-1", 4);
    pane.push_status("later-status-0");
    pane.push_status("later-status-1");

    let update = pane.render_terminal_update(60, 12);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let live = update
        .live
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(history.contains("later-status-0"), "history:\n{history}");
    assert!(history.contains("later-status-1"), "history:\n{history}");
    assert!(
        history.find("later-status-0").unwrap() < history.find("later-status-1").unwrap(),
        "later stable facts must keep canonical order"
    );
    assert!(!history.contains("Using Bash"), "history:\n{history}");
    assert!(live.contains("Using Bash"), "live:\n{live}");

    // Completion appends the canonical card once, after the committed facts.
    finish_tool(&mut pane, "bash-1", "done");
    let finished = pane.render_terminal_update(60, 12);
    assert_eq!(finished.history.len(), 3, "statuses then the tool card");
    assert!(finished.live.is_empty());
}

/// A Delegate entry whose stable tool facts were already emitted must finish
/// with exactly one terminal status — never a complete duplicate card.
#[test]
fn delegate_family_completion_appends_one_terminal_status_without_complete_card_duplicate() {
    use neo_agent_core::multi_agent::{
        AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
        AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentToolActivityPhase, DelegateContext,
    };

    let mut pane = TranscriptPane::new(120, 24);
    let mut running = AgentSnapshot {
        id: AgentId::from_suffix_for_test("agent-a"),
        display_name: AgentDisplayName::new("agent-a"),
        path: AgentPath::root_child(&AgentDisplayName::new("agent-a")),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: "implement feature".to_owned(),
        task_title: "implement feature".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: None,
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 1,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity: vec![AgentActivityEntry {
            kind: AgentActivityKind::Tool {
                id: "read-1".to_owned(),
                name: "Read".to_owned(),
                summary: Some("one.rs".to_owned()),
                phase: AgentToolActivityPhase::Done,
                output: None,
                files: Vec::new(),
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // The stable tool row enters history while the card is live.
    let update = pane.render_terminal_update(120, 24);
    let history = update
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("Used Read"), "history:\n{history}");
    pane.acknowledge_history(&update.history);

    // Completion: remaining facts (none) plus ONE terminal status; the full
    // card (with its tool row) must not be appended again.
    running.state = AgentLifecycleState::Completed;
    running.terminal_at_ms = Some(3);
    running.updated_at_ms = 3;
    running.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "feature implemented".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, running);

    let finished = pane.render_terminal_update(120, 24);
    assert_eq!(finished.history.len(), 1, "one terminal status");
    let summary = finished.history[0]
        .lines
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(summary.contains("agent-a"), "summary:\n{summary}");
    assert!(summary.contains("done"), "summary:\n{summary}");
    assert!(
        !summary.contains("Used Read"),
        "complete card must not be replayed after progressive facts:\n{summary}"
    );
    assert!(finished.live.is_empty());
}
