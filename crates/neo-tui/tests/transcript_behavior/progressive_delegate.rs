//! End-to-end presentation regressions for the progressive native-scrollback
//! transcript behavior: a live area actually bounded by `live_budget`, stable
//! facts keeping canonical order behind ordinary live entries, and a single
//! canonical commit per entry at finalization.

use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentToolActivityPhase, DelegateContext,
    SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::transcript::TranscriptPane;

fn done_tool(id: &str, name: &str, summary: &str) -> AgentActivityEntry {
    AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: Some(summary.to_owned()),
            phase: AgentToolActivityPhase::Done,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }
}
fn running_agent(id: &str, activity: Vec<AgentActivityEntry>) -> AgentSnapshot {
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: AgentDisplayName::new(id),
        path: AgentPath::root_child(&AgentDisplayName::new(id)),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state: AgentLifecycleState::Running,
        task: format!("{id} task"),
        task_title: format!("{id} task"),
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
        tool_count: activity
            .iter()
            .filter(|entry| matches!(entry.kind, AgentActivityKind::Tool { .. }))
            .count(),
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: std::time::Duration::ZERO,
        latest_text: None,
        activity,
        prior_messages: Vec::new(),
        outcome: None,
    }
}
fn completed_agent(id: &str, activity: Vec<AgentActivityEntry>) -> AgentSnapshot {
    let mut agent = running_agent(id, activity);
    agent.state = AgentLifecycleState::Completed;
    agent.terminal_at_ms = Some(3);
    agent.updated_at_ms = 3;
    agent.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: format!("{id} done"),
        is_error: false,
    });
    agent
}
fn running_swarm(id: &str, children: Vec<AgentSnapshot>) -> SwarmSnapshot {
    let children = children
        .into_iter()
        .enumerate()
        .map(|(item_index, agent)| SwarmChildSnapshot {
            item_index,
            item: format!("item {item_index}"),
            agent,
        })
        .collect::<Vec<_>>();
    let aggregate = SwarmAggregate::from_states(children.iter().map(|child| child.agent.state));
    SwarmSnapshot {
        swarm_id: id.to_owned(),
        description: "test swarm".to_owned(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        state: aggregate.status(),
        max_concurrency: 2,
        aggregate,
        children,
    }
}
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
fn slice_text(pane: &mut TranscriptPane, width: usize, height: usize) -> String {
    pane.render_visible_slice(width, height)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n")
}
fn start_wait_delegate(pane: &mut TranscriptPane, id: &str, target: &str) {
    pane.transcript_mut().push_tool_run(
        id,
        "WaitDelegate",
        Some(serde_json::json!({ "ids": [target] }).to_string()),
    );
}
fn finish_wait_delegate(pane: &mut TranscriptPane, id: &str, outcome: &str, is_error: bool) {
    assert!(pane.transcript_mut().mutate_tool(id, |tool| {
        tool.set_result(
            Some(format!("outcome: {outcome}")),
            Some(serde_json::json!({
                "kind": "delegate_wait",
                "outcome": outcome,
                "aggregate": {
                    "total": 1,
                    "terminal": 0,
                    "pending": 1,
                    "not_found": usize::from(outcome == "not_found"),
                },
                "items": []
            })),
            is_error,
            None,
        )
    }));
}
fn assert_wait_group_order(live: &str) {
    let wait = live.find("Waiting for").expect("wait row missing");
    let group = live
        .find("Delegate group")
        .unwrap_or_else(|| panic!("group header missing: {live}"));
    assert!(wait < group, "wait must render above group: {live}");
    assert_eq!(
        live.matches("Waiting for").count(),
        1,
        "duplicate wait: {live}"
    );
    assert_eq!(
        live.matches("Delegate group").count(),
        1,
        "duplicate group: {live}"
    );
}

#[test]
fn delegate_group_completion_order_keeps_done_rows_in_group() {
    let tools = |agent: &str| {
        (0..6)
            .map(|index| {
                done_tool(
                    &format!("{agent}-tool-{index}"),
                    "Read",
                    &format!("{agent}-{index}"),
                )
            })
            .collect::<Vec<_>>()
    };
    let tools_with_new_content = |agent: &str| {
        let mut activity = tools(agent);
        activity.push(done_tool(
            &format!("{agent}-tool-6"),
            "Read",
            &format!("{agent}-6"),
        ));
        activity
    };
    let mut pane = TranscriptPane::new(100, 40);
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools("B")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-c", tools("C")));
    let initial = pane.render_visible_slice(100, 40);
    assert!(
        !initial.iter().any(|line| line.contains("more rows")),
        "initial group was silently shortened: {:?}",
        initial
    );
    let initial_text = initial
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    // The running group keeps every child and their latest activity.
    assert!(
        initial_text.contains("agent-a")
            && initial_text.contains("agent-b")
            && initial_text.contains("agent-c"),
        "group keeps every child: {initial_text}"
    );

    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-a", tools_with_new_content("A")));
    pane.transcript_mut()
        .upsert_delegate(1, running_agent("agent-b", tools_with_new_content("B")));

    let completed_c = completed_agent("agent-c", tools("C"));
    pane.transcript_mut().upsert_delegate(1, completed_c);
    let c_slice = pane.render_visible_slice(100, 40);
    let c_text = c_slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        c_slice.iter().any(|line| {
            let line = strip_ansi(line);
            line.contains("agent-c") && line.contains("done")
        }),
        "completed C lost its status row: {:?}",
        c_slice
    );
    assert!(
        c_text.contains("A-6") && c_text.contains("B-6"),
        "A/B live activity: {c_text}"
    );
    assert!(
        c_slice
            .iter()
            .all(|line| !strip_ansi(line).contains("more rows")),
        "C-first group was silently shortened: {:?}",
        c_slice
    );

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-a", tools_with_new_content("A")));
    let a_slice = pane.render_visible_slice(100, 40);
    assert!(
        a_slice
            .iter()
            .any(|line| strip_ansi(line).contains("agent-a")),
        "completed A stays in the group: {:?}",
        a_slice
    );

    pane.transcript_mut()
        .upsert_delegate(1, completed_agent("agent-b", tools_with_new_content("B")));
    let b_slice = pane.render_visible_slice(100, 40);
    let b_text = b_slice
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        b_text.contains("Delegate group")
            && b_text.contains("agent-a")
            && b_text.contains("agent-b")
            && b_text.contains("agent-c"),
        "completed group must keep one row per child: {b_text}"
    );
    assert_eq!(
        b_text.matches("Delegate group").count(),
        1,
        "group summary was duplicated: {b_text}"
    );
    assert!(
        b_text.contains("A-6") && b_text.contains("B-6"),
        "archived results lost the latest child activity: {b_text}"
    );
}

#[test]
fn delegate_group_live_status_rows_survive_a_short_viewport() {
    let tools = |agent: &str| {
        (0..6)
            .map(|index| {
                done_tool(
                    &format!("{agent}-tool-{index}"),
                    "Read",
                    &format!("{agent}-{index}"),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut pane = TranscriptPane::new(100, 9);
    for agent in ["agent-a", "agent-b", "agent-c", "agent-d", "agent-e"] {
        pane.transcript_mut()
            .upsert_delegate(1, running_agent(agent, tools(agent)));
    }

    let slice = pane
        .render_visible_slice(100, 9)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert!(slice.len() <= 9, "slice must stay bounded: {slice:?}");
    // Tail follow shows the newest child rows; earlier children stay
    // reachable by scrolling up.
    assert!(
        slice.iter().any(|line| line.contains("agent-e")),
        "missing newest agent status row: {slice:?}"
    );
    assert!(
        slice.iter().all(|line| !line.contains("more rows")),
        "status rows were replaced by a generic truncation summary: {slice:?}"
    );
    pane.scroll_transcript_up(usize::MAX);
    let top = pane
        .render_visible_slice(100, 9)
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert!(
        top.iter().any(|line| line.contains("agent-a")),
        "agent-a stays reachable: {top:?}"
    );
}

#[test]
fn ended_wait_delegate_does_not_remove_running_group() {
    for (outcome, is_error) in [("wait_timed_out", false), ("not_found", true)] {
        let mut pane = TranscriptPane::new(120, 12);
        let agent = running_agent(
            "wait-ended",
            vec![done_tool("wait-ended-tool", "Read", "ended.rs")],
        );
        let target = agent.id.as_str().to_owned();
        let other = running_agent("wait-ended-other", Vec::new());
        pane.transcript_mut().upsert_delegate(1, agent);
        pane.transcript_mut().upsert_delegate(1, other);
        start_wait_delegate(&mut pane, "wait-ended-call", &target);
        finish_wait_delegate(&mut pane, "wait-ended-call", outcome, is_error);

        let slice = slice_text(&mut pane, 120, 12);
        assert!(
            slice.contains("Wait timed out") || slice.contains("Target not found"),
            "wait outcome missing for {outcome}: {slice}"
        );
        assert!(
            slice.contains("Delegate group"),
            "group disappeared: {slice}"
        );
    }
}

#[test]
fn ordinary_delegate_commits_one_complete_card_with_header_before_child_tools() {
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
                output_ref: None,
            },
        }],
        prior_messages: Vec::new(),
        outcome: None,
    };
    pane.transcript_mut().upsert_delegate(1, running.clone());

    // The delegate card is one document entry: nothing is split into an
    // earlier partition.
    let slice = slice_text(&mut pane, 120, 24);
    assert!(slice.contains("agent-a"), "slice:\n{slice}");

    // Completion keeps the existing complete card once.
    running.state = AgentLifecycleState::Completed;
    running.terminal_at_ms = Some(3);
    running.updated_at_ms = 3;
    running.outcome = Some(neo_agent_core::multi_agent::AgentTerminalOutcome {
        summary: "feature implemented".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, running);

    let summary = slice_text(&mut pane, 120, 24);
    assert!(summary.contains("agent-a"), "summary:\n{summary}");
    assert!(summary.contains("Used Read"), "summary:\n{summary}");
    assert!(
        summary.contains("feature implemented"),
        "summary:\n{summary}"
    );
    assert!(
        summary.find("Delegate").unwrap() < summary.find("Used Read").unwrap(),
        "delegate header must precede child tools:\n{summary}"
    );
}

#[test]
fn pending_wait_delegate_is_above_running_swarm() {
    let mut pane = TranscriptPane::new(120, 14);
    let swarm = running_swarm(
        "swarm-wait",
        vec![
            running_agent("swarm-wait-a", Vec::new()),
            running_agent("swarm-wait-b", Vec::new()),
        ],
    );
    let target = swarm.swarm_id.clone();
    start_wait_delegate(&mut pane, "wait-swarm-call", &target);
    pane.transcript_mut().upsert_delegate_swarm(swarm);

    let slice = slice_text(&mut pane, 120, 14);
    let wait = slice.find("Waiting for").expect("wait row missing");
    let swarm = slice.find("DelegateSwarm").expect("swarm header missing");
    assert!(wait < swarm, "wait must render above swarm: {slice}");
}

#[test]
fn pending_wait_delegate_renders_in_entry_order_around_the_running_group() {
    let mut before = TranscriptPane::new(120, 12);
    let agent = running_agent(
        "wait-before",
        vec![done_tool("wait-before-tool", "Read", "before.rs")],
    );
    let target = agent.id.as_str().to_owned();
    let other = running_agent("wait-before-other", Vec::new());
    start_wait_delegate(&mut before, "wait-before-call", &target);
    before.transcript_mut().upsert_delegate(1, agent);
    before.transcript_mut().upsert_delegate(1, other);
    let before_slice = slice_text(&mut before, 120, 12);
    assert_wait_group_order(&before_slice);

    // Entry order is canonical: when the wait call arrives after the group,
    // it renders after it.
    let mut after = TranscriptPane::new(120, 12);
    let agent = running_agent(
        "wait-after",
        vec![done_tool("wait-after-tool", "Read", "after.rs")],
    );
    let target = agent.id.as_str().to_owned();
    let other = running_agent("wait-after-other", Vec::new());
    after.transcript_mut().upsert_delegate(1, agent);
    after.transcript_mut().upsert_delegate(1, other);
    start_wait_delegate(&mut after, "wait-after-call", &target);
    let after_slice = slice_text(&mut after, 120, 12);
    assert!(
        after_slice.find("Delegate group").unwrap() < after_slice.find("Waiting for").unwrap(),
        "wait call renders after the group it follows in entry order: {after_slice}"
    );
}
