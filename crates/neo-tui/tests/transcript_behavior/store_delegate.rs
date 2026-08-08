use neo_agent_core::multi_agent::{
    AgentActivityEntry, AgentActivityKind, AgentDisplayName, AgentId, AgentLifecycleState,
    AgentPath, AgentRole, AgentRunMode, AgentSnapshot, AgentTerminalOutcome,
    AgentToolActivityPhase, DelegateContext, SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
};
use neo_tui::primitive::{Finalization, strip_ansi};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane, TranscriptStore};
use std::time::Duration;

fn tool_activity(
    id: &str,
    name: &str,
    summary: &str,
    phase: AgentToolActivityPhase,
) -> AgentActivityEntry {
    AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            summary: Some(summary.to_owned()),
            phase,
            output: None,
            files: Vec::new(),
            output_ref: None,
        },
    }
}
fn agent_snapshot(id: &str, state: AgentLifecycleState) -> AgentSnapshot {
    let display_name = AgentDisplayName::new(id);
    AgentSnapshot {
        id: AgentId::from_suffix_for_test(id),
        display_name: display_name.clone(),
        path: AgentPath::root_child(&display_name),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        state,
        task: "test task".to_owned(),
        task_title: "test task".to_owned(),
        created_at_ms: 1,
        updated_at_ms: 2,
        started_at_ms: Some(1),
        terminal_at_ms: state.is_terminal().then_some(2),
        detached_from_foreground: false,
        terminal_reason: None,
        run_count: 1,
        live_messages_received: 0,
        previous_status: None,
        terminal_status_history: Vec::new(),
        resumed_from: None,
        tool_count: 0,
        token_count: 0,
        cache_read_token_count: 0,
        cache_write_token_count: 0,
        elapsed: Duration::ZERO,
        latest_text: None,
        activity: Vec::new(),
        prior_messages: Vec::new(),
        outcome: None,
    }
}
fn swarm_snapshot(id: &str, children: Vec<AgentSnapshot>) -> SwarmSnapshot {
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

#[test]
fn delegate_completed_card_keeps_latest_four_tools_from_its_snapshot() {
    let mut pane = TranscriptPane::new(100, 24);
    let mut completed = agent_snapshot("delegate-a", AgentLifecycleState::Completed);
    completed.activity = vec![
        tool_activity("read-1", "Read", "one.rs", AgentToolActivityPhase::Done),
        tool_activity("bash-1", "Bash", "make", AgentToolActivityPhase::Failed),
        tool_activity("grep-1", "Grep", "pattern", AgentToolActivityPhase::Done),
        tool_activity("find-1", "Find", "src", AgentToolActivityPhase::Done),
        tool_activity("edit-1", "Edit", "two.rs", AgentToolActivityPhase::Done),
        tool_activity("write-1", "Write", "three.rs", AgentToolActivityPhase::Done),
    ];
    completed.tool_count = 6;
    completed.outcome = Some(AgentTerminalOutcome {
        summary: "delegate result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, completed);
    let slice = pane
        .render_visible_slice(100, 24)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    let header = slice.find("delegate-a").expect("parent header");
    let grep = slice.find("Used Grep").expect("retained Grep fact");
    let find = slice.find("Used Find").expect("retained Find fact");
    let edit = slice.find("Used Edit").expect("retained Edit fact");
    let write = slice.find("Used Write").expect("retained Write fact");
    let result = slice.find("delegate result").expect("terminal result");
    assert!(!slice.contains("Used Read"), "slice:\n{slice}");
    assert!(!slice.contains("Failed Bash"), "slice:\n{slice}");
    assert!(
        header < grep && grep < find && find < edit && edit < write && write < result,
        "slice:\n{slice}"
    );
}

#[test]
fn delegate_group_replacement_preserves_entry_identity() {
    let mut store = TranscriptStore::new();
    store.upsert_delegate(1, agent_snapshot("first", AgentLifecycleState::Running));
    let entry_id = store.entry_ids()[0];

    store.upsert_delegate(1, agent_snapshot("second", AgentLifecycleState::Running));

    assert_eq!(store.entry_ids()[0], entry_id);
    assert!(matches!(
        store.entries()[0],
        TranscriptEntry::DelegateGroup { .. }
    ));
}

#[test]
fn delegate_swarm_terminal_block_uses_one_row_per_child() {
    let mut pane = TranscriptPane::new(120, 24);
    let mut first = agent_snapshot("swarm-first", AgentLifecycleState::Running);
    let mut second = agent_snapshot("swarm-second", AgentLifecycleState::Running);
    second.activity = vec![tool_activity(
        "bash-1",
        "Bash",
        "cargo test",
        AgentToolActivityPhase::Done,
    )];
    second.tool_count = 1;
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    assert!(
        pane.render_visible_slice(120, 24)
            .iter()
            .any(|line| strip_ansi(line).contains("DelegateSwarm")),
        "running swarm stays in the document slice"
    );

    second.activity.clear();
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    second.state = AgentLifecycleState::Completed;
    second.terminal_at_ms = Some(3);
    second.updated_at_ms = 3;
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    second.updated_at_ms = 4;
    second.outcome = Some(AgentTerminalOutcome {
        summary: "swarm second result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate_swarm(swarm_snapshot(
        "swarm-a",
        vec![first.clone(), second.clone()],
    ));
    first.state = AgentLifecycleState::Completed;
    first.terminal_at_ms = Some(5);
    first.updated_at_ms = 5;
    first.outcome = Some(AgentTerminalOutcome {
        summary: "swarm first result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut()
        .upsert_delegate_swarm(swarm_snapshot("swarm-a", vec![first, second]));

    let terminal = pane.render_visible_slice(120, 24);
    let slice = terminal
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>();
    assert!(slice.iter().any(|line| line.contains("DelegateSwarm")));
    assert!(slice.iter().any(|line| line.contains("swarm-first")));
    assert!(slice.iter().any(|line| line.contains("swarm first result")));
    assert!(slice.iter().any(|line| line.contains("swarm-second")));
    assert!(
        slice
            .iter()
            .any(|line| line.contains("swarm second result"))
    );
    assert!(!slice.iter().any(|line| line.contains("Used Bash")));
}

#[test]
fn delegate_terminal_history_keeps_latest_four_tools_after_activity_trimming() {
    let mut pane = TranscriptPane::new(100, 24);
    let mut running = agent_snapshot("delegate-a", AgentLifecycleState::Running);
    running.activity = vec![
        tool_activity("read-1", "Read", "one.rs", AgentToolActivityPhase::Done),
        tool_activity("bash-1", "Bash", "make", AgentToolActivityPhase::Failed),
        tool_activity("grep-1", "Grep", "pattern", AgentToolActivityPhase::Done),
        tool_activity("find-1", "Find", "src", AgentToolActivityPhase::Done),
        tool_activity("edit-1", "Edit", "two.rs", AgentToolActivityPhase::Done),
        tool_activity("write-1", "Write", "three.rs", AgentToolActivityPhase::Done),
    ];
    running.tool_count = 6;
    pane.transcript_mut().upsert_delegate(1, running);

    let slice = pane
        .render_visible_slice(100, 24)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!slice.contains("Used Read"), "slice:\n{slice}");
    assert!(!slice.contains("Failed Bash"), "slice:\n{slice}");
    for tool in ["Used Grep", "Used Find", "Used Edit", "Used Write"] {
        assert!(slice.contains(tool), "missing {tool}:\n{slice}");
    }

    // A later snapshot trims the completed tools away. The card re-renders
    // from the trimmed snapshot inside the same document. Real trimmed
    // snapshots always carry the ongoing latest text, so the store accepts
    // the shrink (a partial delta without text would be rejected to avoid
    // height flicker).
    let mut trimmed = agent_snapshot("delegate-a", AgentLifecycleState::Running);
    trimmed.updated_at_ms = 3;
    trimmed.latest_text = Some("working text".to_owned());
    trimmed.activity = vec![tool_activity(
        "grep-1",
        "Grep",
        "pattern",
        AgentToolActivityPhase::Ongoing,
    )];
    pane.transcript_mut().upsert_delegate(1, trimmed);
    let trimmed_slice = pane
        .render_visible_slice(100, 24)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        trimmed_slice.contains("Using Grep"),
        "trimmed card shows the surviving activity:\n{trimmed_slice}"
    );

    // The completed card renders the terminal snapshot: the header and the
    // terminal outcome, with no stale tool rows from earlier snapshots.
    let mut completed = agent_snapshot("delegate-a", AgentLifecycleState::Completed);
    completed.tool_count = 6;
    completed.outcome = Some(AgentTerminalOutcome {
        summary: "delegate result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(1, completed);
    let slice = pane
        .render_visible_slice(100, 24)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    let header = slice.find("delegate-a").expect("parent header");
    let result = slice.find("delegate result").expect("terminal result");
    assert!(
        header < result,
        "header precedes the terminal result:\n{slice}"
    );
    assert!(
        !slice.contains("Used Grep"),
        "stale activity must not survive into the completed card:\n{slice}"
    );
}

#[test]
fn delegate_to_group_replacement_preserves_progressive_fact_identity() {
    let mut pane = TranscriptPane::new(120, 24);
    let mut first = agent_snapshot("first-agent", AgentLifecycleState::Running);
    first.activity = vec![tool_activity(
        "read-1",
        "Read",
        "one.rs",
        AgentToolActivityPhase::Done,
    )];
    first.tool_count = 1;
    pane.transcript_mut().upsert_delegate(7, first.clone());

    let update = pane.render_visible_slice(120, 24);
    let slice = update
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        slice.contains("first-agent"),
        "running group shows first child: {slice}"
    );

    // A second root delegate replaces the card with a DelegateGroup in place;
    // the entry identity (and therefore the fact identity) is preserved.
    let second = agent_snapshot("second-agent", AgentLifecycleState::Running);
    pane.transcript_mut().upsert_delegate(7, second.clone());
    assert!(matches!(
        pane.transcript().entries()[0],
        TranscriptEntry::DelegateGroup { .. }
    ));

    let running = pane.render_visible_slice(120, 24);
    let slice = running
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(slice.contains("first-agent"), "slice:\n{slice}");
    assert!(slice.contains("second-agent"), "slice:\n{slice}");

    let mut second_completed = second;
    second_completed.state = AgentLifecycleState::Completed;
    second_completed.terminal_at_ms = Some(3);
    second_completed.updated_at_ms = 3;
    pane.transcript_mut()
        .upsert_delegate(7, second_completed.clone());
    second_completed.updated_at_ms = 4;
    second_completed.outcome = Some(AgentTerminalOutcome {
        summary: "second-agent result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(7, second_completed);

    let mut first_completed = first;
    first_completed.state = AgentLifecycleState::Completed;
    first_completed.terminal_at_ms = Some(5);
    first_completed.updated_at_ms = 5;
    first_completed.outcome = Some(AgentTerminalOutcome {
        summary: "first-agent result".to_owned(),
        is_error: false,
    });
    pane.transcript_mut().upsert_delegate(7, first_completed);
    let terminal = pane.render_visible_slice(120, 24);
    let slice = terminal
        .iter()
        .map(|line| strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let header = slice.find("2 agents finished").expect("parent header");
    let first_agent = slice.find("first-agent").expect("first child");
    let second_agent = slice.find("second-agent").expect("second child");
    let tool = slice.find("Used Read").expect("retained tool");
    let first_result = slice.find("first-agent result").expect("first result");
    let second_result = slice.find("second-agent result").expect("second result");
    assert!(
        header < first_agent
            && first_agent < tool
            && tool < first_result
            && first_result < second_agent
            && second_agent < second_result,
        "slice:\n{slice}"
    );
}

#[test]
fn resumed_delegate_appends_new_run_card() {
    let mut store = TranscriptStore::new();
    let completed = agent_snapshot("delegate", AgentLifecycleState::Completed);
    let agent_id = completed.id.clone();
    store.upsert_delegate(1, completed);
    let completed_entry_id = store.entry_ids()[0];

    let mut resumed = agent_snapshot("delegate", AgentLifecycleState::Running);
    resumed.run_count = 2;
    resumed.resumed_from = Some(agent_id);
    resumed.task_title = "resumed task".to_owned();
    store.upsert_delegate(2, resumed.clone());

    assert_eq!(store.entries().len(), 2);
    assert_eq!(store.entry_ids()[0], completed_entry_id);
    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
    assert_eq!(store.entry_finalization(1), Some(Finalization::Live));

    resumed.tool_count = 7;
    store.upsert_delegate_progress(2, &resumed.progress_snapshot());
    let TranscriptEntry::Delegate { component } = &store.entries()[1] else {
        panic!("resumed run should render as a new delegate card");
    };
    assert_eq!(component.snapshot().run_count, 2);
    assert_eq!(component.snapshot().tool_count, 7);
}
