use neo_agent_core::AgentEvent;
use neo_agent_core::workflow::{WorkflowId, WorkflowSnapshot, WorkflowState, WorkflowStepRecord};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Line, strip_ansi};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane, WorkflowCardComponent};

fn step(
    index: usize,
    name: &str,
    state: WorkflowState,
    summary: Option<&str>,
) -> WorkflowStepRecord {
    WorkflowStepRecord {
        index,
        name: name.to_owned(),
        state,
        summary: summary.map(str::to_owned),
        details: None,
        agent: None,
        swarm: None,
        has_failures: None,
    }
}

fn sample_snapshot() -> WorkflowSnapshot {
    WorkflowSnapshot {
        id: WorkflowId("wf-test".to_owned()),
        title: "Runtime audit and fix".to_owned(),
        state: WorkflowState::Running,
        steps: vec![
            step(
                0,
                "swarm: audit",
                WorkflowState::Completed,
                Some("3 items completed."),
            ),
            step(
                1,
                "delegate: fix issue",
                WorkflowState::Completed,
                Some("Fixed."),
            ),
        ],
    }
}

fn ansi(lines: &[Line]) -> String {
    lines
        .iter()
        .map(Line::to_ansi)
        .collect::<Vec<_>>()
        .join("\n")
}

fn assert_ansi_contains_color(ansi: &str, color: Color) {
    let expected = match color {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(n) => format!("\x1b[38;5;{n}m"),
        _ => return,
    };
    assert!(
        ansi.contains(&expected),
        "missing color {expected:?} in {ansi:?}"
    );
}

#[test]
fn workflow_card_renders_title_and_steps() {
    let mut card = WorkflowCardComponent::new(sample_snapshot());
    let lines = card.render(120);
    let text: String = lines
        .iter()
        .map(|l| strip_ansi(&l.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Workflow  Runtime audit and fix"), "{text}");
    assert!(text.contains("running"), "{text}");
    assert!(text.contains("swarm: audit"), "{text}");
    assert!(text.contains("delegate: fix issue"), "{text}");
}

#[test]
fn workflow_card_uses_theme_for_state_styles_and_summaries() {
    let theme = TuiTheme::default()
        .with_brand(Color::Rgb(120, 80, 240))
        .with_status_ok(Color::Rgb(1, 180, 90))
        .with_status_error(Color::Rgb(220, 20, 20))
        .with_status_warn(Color::Rgb(230, 160, 20))
        .with_text_muted(Color::Rgb(90, 100, 110));
    let snapshot = WorkflowSnapshot {
        id: WorkflowId("wf-style".to_owned()),
        title: "Styled workflow".to_owned(),
        state: WorkflowState::Failed,
        steps: vec![
            step(
                0,
                "completed step",
                WorkflowState::Completed,
                Some("Audit finished."),
            ),
            step(
                1,
                "running step",
                WorkflowState::Running,
                Some("Worker is editing."),
            ),
            step(
                2,
                "failed step",
                WorkflowState::Failed,
                Some("Focused test failed."),
            ),
        ],
    };
    let card = WorkflowCardComponent::new(snapshot);
    let rows = card.render_with_theme(140, &theme);
    let raw = ansi(&rows);
    let text = rows
        .iter()
        .map(|line| strip_ansi(&line.to_ansi()))
        .collect::<Vec<_>>()
        .join("\n");

    assert_ansi_contains_color(&raw, theme.brand);
    assert_ansi_contains_color(&raw, theme.status_ok);
    assert_ansi_contains_color(&raw, theme.status_warn);
    assert_ansi_contains_color(&raw, theme.status_error);
    assert!(text.contains("Audit finished."), "{text}");
    assert!(text.contains("Worker is editing."), "{text}");
    assert!(text.contains("Focused test failed."), "{text}");
}

#[test]
fn workflow_card_finalizes_on_completion() {
    use neo_tui::primitive::Finalization;
    let mut snapshot = sample_snapshot();
    snapshot.state = WorkflowState::Completed;
    let card = WorkflowCardComponent::new(snapshot);
    assert_eq!(card.finalization(), Finalization::Finalized);
}

#[test]
fn transcript_pane_upserts_workflow_card_from_events() {
    let mut pane = TranscriptPane::new(120, 20);
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: sample_snapshot(),
    });

    let _ = pane.render_frame(120, 20);
    let frame = pane.frame_ansi_lines();
    let text: String = frame
        .iter()
        .map(|l| strip_ansi(l))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(text.contains("Workflow  Runtime audit and fix"), "{text}");
}

#[test]
fn in_place_workflow_update_preserves_active_thinking() {
    let mut pane = TranscriptPane::new(120, 20);
    let mut workflow = sample_snapshot();
    pane.apply_agent_event(AgentEvent::WorkflowStarted {
        turn: 1,
        workflow: workflow.clone(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingStarted {
        turn: 2,
        id: "reasoning".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: "continuous".to_owned(),
    });

    workflow.steps[1].summary = Some("Updated in place.".to_owned());
    pane.apply_agent_event(AgentEvent::WorkflowUpdated { turn: 1, workflow });
    pane.apply_agent_event(AgentEvent::ThinkingDelta {
        turn: 2,
        text: " thinking".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ThinkingFinished {
        turn: 2,
        signature: None,
        redacted: false,
    });

    let thinking = pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ThinkingBlock { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(thinking, vec!["continuous thinking"]);
}
