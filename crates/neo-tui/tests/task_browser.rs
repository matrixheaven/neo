use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::visible_width;
use neo_tui::tasks_browser::{
    TaskBrowserAction, TaskBrowserFilter, TaskBrowserItem, TaskBrowserKind, TaskBrowserRenderer,
    TaskBrowserSnapshot, TaskBrowserState, TaskBrowserStatus,
};

fn item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: id.to_owned(),
        description: format!("command for {id}"),
        elapsed: "00:01".to_owned(),
        detail_lines: vec![format!("id:          {id}")],
        preview_lines: vec![format!("output for {id}")],
        can_stop: status.is_active(),
    }
}

#[test]
fn task_browser_defaults_to_all_filter() {
    let state = TaskBrowserState::new();
    assert_eq!(state.filter(), TaskBrowserFilter::All);
    assert!(state.selected_task_id().is_none());
}

#[test]
fn task_browser_tab_toggles_filter() {
    let mut state = TaskBrowserState::new();
    assert_eq!(state.handle_action(TaskBrowserAction::ToggleFilter), None);
    assert_eq!(state.filter(), TaskBrowserFilter::Active);
    assert_eq!(state.handle_action(TaskBrowserAction::ToggleFilter), None);
    assert_eq!(state.filter(), TaskBrowserFilter::All);
}

#[test]
fn task_browser_preserves_selection_by_task_id() {
    let mut state = TaskBrowserState::new();
    let first = TaskBrowserSnapshot::new(vec![
        item("bash-a", TaskBrowserStatus::Running),
        item("bash-b", TaskBrowserStatus::Completed),
    ]);
    state.apply_snapshot(&first);
    state.handle_action(TaskBrowserAction::SelectDown);
    assert_eq!(state.selected_task_id(), Some("bash-b"));

    let refreshed = TaskBrowserSnapshot::new(vec![
        item("bash-b", TaskBrowserStatus::Completed),
        item("bash-c", TaskBrowserStatus::Running),
    ]);
    state.apply_snapshot(&refreshed);
    assert_eq!(state.selected_task_id(), Some("bash-b"));
}

#[test]
fn task_browser_active_filter_hides_terminal_tasks_and_selects_first_visible() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        item("bash-done", TaskBrowserStatus::Completed),
        item("bash-run", TaskBrowserStatus::Running),
    ]));

    state.handle_action(TaskBrowserAction::ToggleFilter);

    assert_eq!(state.filter(), TaskBrowserFilter::Active);
    assert_eq!(state.visible_items().len(), 1);
    assert_eq!(state.selected_task_id(), Some("bash-run"));
}

#[test]
fn task_browser_stop_confirmation_requires_confirm_or_cancel() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-run",
        TaskBrowserStatus::Running,
    )]));

    assert_eq!(
        state.handle_action(TaskBrowserAction::RequestStop),
        Some("bash-run".to_owned())
    );
    assert_eq!(state.stop_confirmation_task_id(), Some("bash-run"));
    assert_eq!(
        state.handle_action(TaskBrowserAction::Cancel),
        None,
        "Esc cancels the confirmation before closing the browser"
    );
    assert!(state.stop_confirmation_task_id().is_none());
}

#[test]
fn task_browser_request_stop_on_finished_task_sets_footer_message() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-done",
        TaskBrowserStatus::Completed,
    )]));

    assert_eq!(state.handle_action(TaskBrowserAction::RequestStop), None);
    assert_eq!(state.footer_message(), Some("Task already finished."));
}

#[test]
fn task_browser_pause_resume_only_targets_workflows() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-run",
        TaskBrowserStatus::Running,
    )]));
    assert_eq!(state.handle_action(TaskBrowserAction::RequestPause), None);
    assert_eq!(
        state.footer_message(),
        Some("Only workflow tasks can be paused.")
    );

    let mut workflow = item("workflow-run", TaskBrowserStatus::Running);
    workflow.kind = TaskBrowserKind::Workflow;
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![workflow]));
    assert_eq!(
        state.handle_action(TaskBrowserAction::RequestPause),
        Some("workflow-run".to_owned())
    );
    assert_eq!(state.pause_confirmation_task_id(), Some("workflow-run"));
    assert_eq!(state.handle_action(TaskBrowserAction::Cancel), None);

    let mut workflow = item("workflow-run", TaskBrowserStatus::Paused);
    workflow.kind = TaskBrowserKind::Workflow;
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![workflow]));
    assert_eq!(
        state.handle_action(TaskBrowserAction::RequestResume),
        Some("workflow-run".to_owned())
    );
    assert_eq!(state.resume_confirmation_task_id(), Some("workflow-run"));
}

#[test]
fn task_browser_page_down_scrolls_output_when_output_focused() {
    let mut item = item("bash-run", TaskBrowserStatus::Running);
    item.preview_lines = (0..20).map(|line| format!("line {line}")).collect();
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));

    state.handle_action(TaskBrowserAction::ToggleOutputFocus);
    state.handle_action(TaskBrowserAction::SelectPageDown);

    assert_eq!(state.output_scroll(), 10);
    assert_eq!(state.selected_task_id(), Some("bash-run"));
}

#[test]
fn task_browser_renderer_keeps_selection_visible_and_scrolls_output() {
    let mut items = Vec::new();
    for index in 0..18 {
        let mut task = item(&format!("bash-{index:02}"), TaskBrowserStatus::Running);
        task.preview_lines = (0..20).map(|line| format!("output line {line}")).collect();
        items.push(task);
    }
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(items));
    for _ in 0..12 {
        state.handle_action(TaskBrowserAction::SelectDown);
    }
    state.handle_action(TaskBrowserAction::ToggleOutputFocus);
    state.handle_action(TaskBrowserAction::SelectPageDown);

    let rendered = render_plain(&state, 120, 18).join("\n");

    assert!(rendered.contains("bash-00"));
    assert!(
        rendered
            .lines()
            .any(|line| line.contains('>') && line.contains("bash-12"))
    );
    assert!(rendered.contains("output line 10"));
    assert!(!rendered.contains("output line 0"));
}

fn render_plain(state: &TaskBrowserState, width: usize, height: usize) -> Vec<String> {
    TaskBrowserRenderer::new(state, TuiTheme::default())
        .render(width, height)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}

#[test]
fn task_browser_empty_all_renderer_shows_product_empty_state() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(Vec::new()));

    let rendered = render_plain(&state, 120, 18).join("\n");

    assert!(rendered.contains("TASK BROWSER"));
    assert!(rendered.contains("filter=ALL"));
    assert!(rendered.contains("0 total"));
    assert!(rendered.contains("Tasks [all]"));
    assert!(rendered.contains("No background tasks in this"));
    assert!(rendered.contains("session."));
    assert!(rendered.contains("Select a task from the list."));
    assert!(rendered.contains("No task selected."));
    assert!(!rendered.contains("active_background_tasks"));
}

#[test]
fn task_browser_empty_active_renderer_points_to_all_filter() {
    let mut state = TaskBrowserState::new();
    state.handle_action(TaskBrowserAction::ToggleFilter);
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item(
        "bash-done",
        TaskBrowserStatus::Completed,
    )]));

    let rendered = render_plain(&state, 120, 18).join("\n");

    assert!(rendered.contains("filter=ACTIVE"));
    assert!(rendered.contains("No active tasks. Tab = show all."));
}

#[test]
fn task_browser_left_tasks_pane_consumes_full_content_height() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(Vec::new()));
    let lines = render_plain(&state, 120, 18);
    let tasks_top = lines
        .iter()
        .position(|line| line.contains("Tasks [all]"))
        .expect("tasks pane top");
    let footer = lines
        .iter()
        .position(|line| line.contains("Q/Esc close"))
        .expect("footer");
    let left_bottom = lines
        .iter()
        .position(|line| line.starts_with("└") && line.contains("┘"))
        .expect("left tasks pane bottom border");

    assert!(
        left_bottom + 1 == footer,
        "left pane should run down to the footer, lines:\n{}",
        lines.join("\n")
    );
    assert!(
        footer.saturating_sub(tasks_top) >= 10,
        "left pane should be tall, lines:\n{}",
        lines.join("\n")
    );
}

#[test]
fn task_browser_populated_renderer_shows_counts_detail_preview_and_footer() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        item("bash-run", TaskBrowserStatus::Running),
        item("bash-done", TaskBrowserStatus::Completed),
        item("bash-fail", TaskBrowserStatus::Failed),
    ]));

    let rendered = render_plain(&state, 130, 20).join("\n");

    assert!(rendered.contains("1 running"));
    assert!(rendered.contains("1 completed"));
    assert!(rendered.contains("1 interrupted"));
    assert!(rendered.contains("3 total"));
    assert!(rendered.contains("> ● bash-run"));
    assert!(rendered.contains("id:          bash-run"));
    assert!(rendered.contains("output for bash-run"));
    assert!(rendered.contains("Enter/O output"));
    assert!(rendered.contains("S stop"));
    assert!(rendered.contains("Tab filter"));
}

#[test]
fn task_browser_narrow_renderer_keeps_lines_within_width() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![TaskBrowserItem {
        id: "bash-with-a-very-long-id-that-should-truncate".to_owned(),
        kind: TaskBrowserKind::Bash,
        status: TaskBrowserStatus::Running,
        title: "a very long cargo command with many arguments that should not overflow".to_owned(),
        description: "long command".to_owned(),
        elapsed: "12:34".to_owned(),
        detail_lines: vec![
            "id:          bash-with-a-very-long-id-that-should-truncate".to_owned(),
            "description: a very long cargo command with many arguments".to_owned(),
        ],
        preview_lines: vec![
            "this is a very long output line that must be clipped to terminal width".to_owned(),
        ],
        can_stop: true,
    }]));

    let lines = render_plain(&state, 48, 12);

    assert_eq!(lines.len(), 12);
    assert!(lines.iter().any(|line| line.contains("TASK BROWSER")));
    assert!(lines.iter().any(|line| line.contains("Q/Esc close")));
    assert!(
        lines.iter().all(|line| visible_width(line) <= 48),
        "all lines must fit width:\n{}",
        lines.join("\n")
    );
}

#[test]
fn task_browser_tiny_renderer_preserves_header_and_footer() {
    let state = TaskBrowserState::new();
    let lines = render_plain(&state, 36, 4);

    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("TASK BROWSER"));
    assert!(lines.iter().any(|line| line.contains("Q/Esc")));
    assert!(
        lines.iter().all(|line| visible_width(line) <= 36),
        "tiny lines must fit width:\n{}",
        lines.join("\n")
    );
}
