use neo_agent_core::workflow::{WorkflowChildKey, WorkflowStepKey};
use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::visible_width;
use neo_tui::tasks_browser::{
    TaskBrowserAction, TaskBrowserFocus, TaskBrowserItem, TaskBrowserKind,
    TaskBrowserPendingUserRequest, TaskBrowserRenderer, TaskBrowserSnapshot, TaskBrowserState,
    TaskBrowserStatus, TaskBrowserWorkflowChild, TaskBrowserWorkflowChildPage,
    TaskBrowserWorkflowMeta, TaskBrowserWorkflowRowState, TaskBrowserWorkflowStep,
    WorkflowAnswerControl, WorkflowSaveDestination,
};
use serde_json::{Value, json};

fn browser_item(id: &str, status: TaskBrowserStatus) -> TaskBrowserItem {
    TaskBrowserItem {
        id: id.to_owned(),
        kind: TaskBrowserKind::Bash,
        status,
        title: format!("task {id}"),
        description: format!("command for {id}"),
        elapsed: "00:01".to_owned(),
        detail_lines: vec![format!("Task: {id}")],
        preview_lines: Vec::new(),
        can_stop: status.is_active(),
        human_handle: None,
        list_cursor: None,
        workflow: None,
    }
}
fn child(id: &str, title: &str, usage: Option<Value>) -> TaskBrowserWorkflowChild {
    TaskBrowserWorkflowChild {
        key: WorkflowChildKey::DirectDelegate {
            invocation_id: id.to_owned(),
        },
        title: title.to_owned(),
        role: Some("worker".to_owned()),
        state: TaskBrowserWorkflowRowState::Working,
        elapsed: "00:04".to_owned(),
        actual_usage: usage,
        latest_activity: Some("Working".to_owned()),
        terminal_summary: None,
    }
}
fn workflow_item(
    pending_user: Option<TaskBrowserPendingUserRequest>,
    inline_unsaved: bool,
    child_page: TaskBrowserWorkflowChildPage,
) -> TaskBrowserItem {
    TaskBrowserItem {
        id: "workflow-run".to_owned(),
        kind: TaskBrowserKind::Workflow,
        status: TaskBrowserStatus::Running,
        title: "deep-research".to_owned(),
        description: "Research and summarize".to_owned(),
        elapsed: "00:12".to_owned(),
        detail_lines: vec!["Workflow summary".to_owned()],
        preview_lines: Vec::new(),
        can_stop: true,
        human_handle: Some("deep-research".to_owned()),
        list_cursor: None,
        workflow: Some(TaskBrowserWorkflowMeta {
            run_id: "run-1".to_owned(),
            display_name: "deep-research".to_owned(),
            purpose: "Research and summarize".to_owned(),
            elapsed_ms: 12_000,
            current_step_key: Some(WorkflowStepKey {
                phase_id: Some("execute".to_owned()),
                phase_marker_sequence: 2,
            }),
            steps: vec![step("plan", 1, "Plan"), step("execute", 2, "Execute")],
            child_page,
            pending_user,
            inline_unsaved,
        }),
    }
}
fn initial_child_page() -> TaskBrowserWorkflowChildPage {
    TaskBrowserWorkflowChildPage {
        items: vec![
            child("research", "Source worker", Some(json!({"tokens": 120}))),
            child("verify", "Verify", None),
        ],
        next_cursor: Some("children-page-2".to_owned()),
        has_more: true,
        query_hash: "step-children".to_owned(),
    }
}
fn open_workflow(item: TaskBrowserItem) -> TaskBrowserState {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));
    assert_eq!(state.handle_action(TaskBrowserAction::OpenWorkflow), None);
    state
}
fn render_plain(state: &TaskBrowserState, width: usize, height: usize) -> Vec<String> {
    TaskBrowserRenderer::new(state, TuiTheme::default())
        .render(width, height)
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect()
}
fn step(id: &str, sequence: u64, title: &str) -> TaskBrowserWorkflowStep {
    TaskBrowserWorkflowStep {
        key: WorkflowStepKey {
            phase_id: Some(id.to_owned()),
            phase_marker_sequence: sequence,
        },
        title: title.to_owned(),
        state: TaskBrowserWorkflowRowState::Working,
        done_count: 1,
        working_count: 1,
        queued_count: 0,
        failed_count: 0,
    }
}

#[test]
fn browser_keeps_keyed_task_selection_and_filters_active_tasks() {
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        browser_item("done", TaskBrowserStatus::Completed),
        browser_item("running", TaskBrowserStatus::Running),
    ]));
    assert_eq!(state.selected_task_id(), Some("done"));

    state.handle_action(TaskBrowserAction::SelectDown);
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![
        browser_item("running", TaskBrowserStatus::Running),
        browser_item("later", TaskBrowserStatus::Waiting),
    ]));
    assert_eq!(state.selected_task_id(), Some("running"));

    state.handle_action(TaskBrowserAction::ToggleFilter);
    assert_eq!(state.visible_items().len(), 2);
    assert!(state.list_refresh_requested());

    // The header always shows all three choices with only the active one
    // bracketed, whichever filter is selected.
    let active_header = render_plain(&state, 99, 20).join("\n");
    assert!(active_header.contains(" TASKS  ALL  [ACTIVE]  WORKFLOWS  2 tasks "));

    state.handle_action(TaskBrowserAction::ToggleFilter);
    let workflow_header = render_plain(&state, 99, 20).join("\n");
    assert!(workflow_header.contains(" TASKS  ALL  ACTIVE  [WORKFLOWS]  0 tasks "));
}

#[test]
fn browser_opens_non_workflow_details_and_scrolls_its_output() {
    let mut task = browser_item("bash", TaskBrowserStatus::Running);
    let long_command = format!("run --flag {}", "very-long-command-word-".repeat(12));
    task.detail_lines = vec![format!("Command: {long_command}")];
    task.preview_lines = (0..24).map(|line| format!("output {line}")).collect();
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task]));

    state.handle_action(TaskBrowserAction::OpenTaskDetails);
    assert!(state.task_details_open());

    // Wide split: the inspector is always visible and follows the selection.
    let wide = render_plain(&state, 120, 20).join("\n");
    assert!(wide.contains(" DETAILS "));
    assert!(wide.contains(" LATEST OUTPUT · Preview 7/24 "));
    assert!(wide.contains("running  bash"));
    assert!(wide.contains("output 0"));

    // Medium width swaps to one full-width Details page.
    let medium_details = render_plain(&state, 99, 20).join("\n");
    assert!(medium_details.contains(" DETAILS "));
    assert!(medium_details.contains("running  bash"));
    assert!(!medium_details.contains(" LATEST OUTPUT · Preview "));

    // The long command wraps instead of being replaced by an ellipsis: its
    // final chunk is fully visible in the wide inspector, and the medium
    // Details page keeps every character (only line breaks are added).
    assert!(wide.contains(&long_command[long_command.len() - 46..]));
    assert!(
        wide.lines()
            .filter(|line| line.contains("very-long"))
            .count()
            >= 3
    );
    let compact_frame = |frame: &str| {
        frame
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>()
    };
    let compact_command = long_command
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    assert!(compact_frame(&medium_details).contains(&compact_command));

    state.handle_action(TaskBrowserAction::ToggleOutputFocus);
    assert_eq!(state.focus(), TaskBrowserFocus::Output);
    state.handle_action(TaskBrowserAction::SelectPageDown);
    assert_eq!(state.output_scroll(), 10);

    // Medium and small widths render the same single full-width page; the
    // divider reports how much of the wrapped preview is shown.
    let medium = render_plain(&state, 99, 20).join("\n");
    assert!(medium.contains(" LATEST OUTPUT · Preview 14/24 "));
    assert!(medium.contains("output 10"));
    assert!(!medium.contains(" DETAILS "));
    let small = render_plain(&state, 69, 20).join("\n");
    assert!(small.contains(" LATEST OUTPUT · Preview 14/24 "));
    assert!(small.contains("output 10"));

    // The wide split keeps both sections; scrolling only moves the preview.
    let scrolled = render_plain(&state, 120, 20).join("\n");
    assert!(scrolled.contains("output 10"));
    assert!(!scrolled.contains("output 0"));
    assert!(scrolled.contains(" DETAILS "));

    // Esc closes details first (back to the list page), then closes the
    // browser.
    state.handle_action(TaskBrowserAction::Cancel);
    assert!(!state.task_details_open());
    assert_eq!(state.focus(), TaskBrowserFocus::Tasks);
    let list = render_plain(&state, 99, 20).join("\n");
    assert!(list.contains(" Tasks "));
    assert!(!list.contains(" LATEST OUTPUT · Preview "));
    assert!(!list.contains(" DETAILS "));
    assert_eq!(
        state.handle_action(TaskBrowserAction::Cancel),
        Some("__close__".to_owned())
    );
}

#[test]
fn browser_renderer_matches_general_layout_at_supported_sizes() {
    let mut task = browser_item("bash", TaskBrowserStatus::Running);
    task.detail_lines = vec![format!("Command: {}", "x".repeat(200))];
    task.preview_lines = (0..24).map(|line| format!("output {line}")).collect();
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![task]));

    for width in [32, 69, 70, 99, 100, 120, 180] {
        for height in [12, 20, 40] {
            let lines = render_plain(&state, width, height);
            assert_eq!(lines.len(), height, "width={width}, height={height}");
            assert!(
                lines.iter().all(|line| visible_width(line) <= width),
                "width={width}, height={height}:\n{}",
                lines.join("\n")
            );
            let frame = lines.join("\n");
            if width >= 100 {
                // Wide split: the inspector is always present, even with
                // details closed, and the list never overlaps it.
                let list_width = (width / 3).clamp(30, 42);
                for line in lines.iter().skip(1).take(height - 2) {
                    assert_eq!(
                        line.chars().nth(list_width),
                        Some(' '),
                        "width={width}, height={height}, line={line:?}"
                    );
                }
                assert!(
                    frame.contains(" DETAILS "),
                    "width={width}, height={height}"
                );
                assert!(
                    frame.contains(" LATEST OUTPUT · Preview "),
                    "width={width}, height={height}"
                );
                assert!(
                    frame.contains("running  bash"),
                    "width={width}, height={height}"
                );
            } else {
                // Exactly one full-width page: the top content row spans the
                // whole terminal and no inspector sections exist.
                assert_eq!(
                    visible_width(&lines[1]),
                    width,
                    "width={width}, height={height}, line={:?}",
                    lines[1]
                );
                assert!(frame.contains(" Tasks "), "width={width}, height={height}");
                assert!(
                    !frame.contains(" DETAILS "),
                    "width={width}, height={height}"
                );
                assert!(
                    !frame.contains(" LATEST OUTPUT · Preview "),
                    "width={width}, height={height}"
                );
            }
        }
    }

    // A width narrower than the marker must stay panic-free (a zero-width
    // task row truncates to empty before painting) with an exact frame
    // height and no overflow.
    let tiny = render_plain(&state, 2, 12);
    assert_eq!(tiny.len(), 12);
    assert!(
        tiny.iter().all(|line| visible_width(line) <= 2),
        "width=2:\n{}",
        tiny.join("\n")
    );

    // Details open: small and medium widths swap to the single Details page.
    state.handle_action(TaskBrowserAction::OpenTaskDetails);
    assert!(state.task_details_open());
    for width in [32, 69, 70, 99] {
        let frame = render_plain(&state, width, 20).join("\n");
        assert!(frame.contains(" DETAILS "), "width={width}");
        assert!(frame.contains("┌ DETAILS"), "width={width}");
        assert!(
            !frame.contains(" LATEST OUTPUT · Preview "),
            "width={width}"
        );
    }

    // Output focus: the single page becomes the Latest output preview, and
    // the divider reports how much of the wrapped output is shown.
    state.handle_action(TaskBrowserAction::ToggleOutputFocus);
    assert_eq!(state.focus(), TaskBrowserFocus::Output);
    for width in [32, 69, 70, 99] {
        let frame = render_plain(&state, width, 20).join("\n");
        assert!(frame.contains(" LATEST OUTPUT · Preview "), "width={width}");
        assert!(frame.contains("┌ LATEST OUTPUT"), "width={width}");
        assert!(!frame.contains(" DETAILS "), "width={width}");
    }
    assert!(
        render_plain(&state, 99, 20)
            .join("\n")
            .contains("Preview 17/24"),
        "frame:\n{}",
        render_plain(&state, 99, 20).join("\n")
    );
    assert!(
        render_plain(&state, 99, 12)
            .join("\n")
            .contains("Preview 9/24")
    );
    // Tall enough to show every line: no fraction is appended to the divider.
    let full = render_plain(&state, 99, 40).join("\n");
    let divider = full
        .lines()
        .find(|line| line.starts_with("┌"))
        .expect("output divider");
    assert!(!divider.contains('/'), "divider:\n{divider}");
}

#[test]
fn workflow_opens_in_place_and_esc_returns_to_the_selected_task() {
    let workflow = workflow_item(None, false, initial_child_page());
    let mut state = TaskBrowserState::new();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![workflow]));

    assert_eq!(state.handle_action(TaskBrowserAction::OpenWorkflow), None);
    assert_eq!(
        state.workflow_item().map(|item| item.id.as_str()),
        Some("workflow-run")
    );
    assert!(state.take_child_refresh_request());

    assert_eq!(state.handle_action(TaskBrowserAction::Cancel), None);
    assert!(state.workflow_item().is_none());
    assert_eq!(state.selected_task_id(), Some("workflow-run"));
    assert!(state.open_workflow_for_task("workflow-run"));
    assert!(state.workflow_item().is_some());
    assert!(!state.open_workflow_for_task("missing"));
}

#[test]
fn workflow_opens_at_the_runtime_current_step_when_it_is_available() {
    let state = open_workflow(workflow_item(None, false, initial_child_page()));
    assert_eq!(
        state
            .selected_workflow_step()
            .map(|step| step.title.as_str()),
        Some("Execute")
    );
}

#[test]
fn workflow_steps_and_agents_keep_their_keyed_selection_after_a_refresh() {
    let item = workflow_item(None, false, initial_child_page());
    let mut state = open_workflow(item.clone());

    state.handle_action(TaskBrowserAction::SelectDown);
    assert_eq!(
        state
            .selected_workflow_step()
            .map(|value| value.title.as_str()),
        Some("Execute")
    );
    assert_eq!(
        state.handle_action(TaskBrowserAction::ToggleWorkflowFocus),
        None
    );
    assert_eq!(state.focus(), TaskBrowserFocus::Agents);
    state.handle_action(TaskBrowserAction::SelectDown);
    assert_eq!(
        state
            .selected_workflow_child()
            .map(|value| value.title.as_str()),
        Some("Verify")
    );

    let mut refreshed = item;
    let workflow = refreshed.workflow.as_mut().expect("workflow metadata");
    workflow.steps.reverse();
    workflow.child_page.items.reverse();
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![refreshed]));

    assert_eq!(
        state
            .selected_workflow_step()
            .map(|value| value.title.as_str()),
        Some("Execute")
    );
    assert_eq!(
        state
            .selected_workflow_child()
            .map(|value| value.title.as_str()),
        Some("Verify")
    );
    assert_eq!(
        state.handle_action(TaskBrowserAction::OpenWorkflowChildDetails),
        None
    );
    assert!(state.child_details_open());
}

#[test]
fn workflow_child_pages_follow_cursors_without_retaining_previous_children() {
    let mut state = open_workflow(workflow_item(None, false, initial_child_page()));
    assert_eq!(
        state
            .workflow_child_page_intent()
            .map(|intent| intent.cursor),
        Some(None)
    );

    state.handle_action(TaskBrowserAction::RequestNextChildPage);
    assert!(state.take_child_refresh_request());
    assert_eq!(
        state
            .workflow_child_page_intent()
            .map(|intent| intent.cursor),
        Some(Some("children-page-2".to_owned()))
    );

    let second_page = TaskBrowserWorkflowChildPage {
        items: vec![child("review", "Review", None)],
        next_cursor: None,
        has_more: false,
        query_hash: "step-children".to_owned(),
    };
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![workflow_item(
        None,
        false,
        second_page,
    )]));
    assert_eq!(
        state
            .selected_workflow_child()
            .map(|value| value.title.as_str()),
        Some("Review")
    );
    assert!(
        !render_plain(&state, 100, 20)
            .join("\n")
            .contains("Source worker")
    );

    state.handle_action(TaskBrowserAction::RequestPrevChildPage);
    assert!(state.take_child_refresh_request());
    assert_eq!(
        state
            .workflow_child_page_intent()
            .map(|intent| intent.cursor),
        Some(None)
    );
}

#[test]
fn workflow_pause_stop_and_save_actions_only_emit_valid_intents() {
    let mut state = open_workflow(workflow_item(None, false, initial_child_page()));
    assert_eq!(
        state.handle_action(TaskBrowserAction::TogglePauseResume),
        Some("workflow-run".to_owned())
    );
    assert_eq!(state.handle_action(TaskBrowserAction::RequestStop), None);
    assert_eq!(state.stop_confirmation_task_id(), Some("workflow-run"));
    assert_eq!(
        state.handle_action(TaskBrowserAction::ConfirmStop),
        Some("workflow-run".to_owned())
    );
    assert_eq!(state.handle_action(TaskBrowserAction::RequestSave), None);
    assert!(state.save_draft().is_none());
    assert!(state.footer_message().is_none());

    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![workflow_item(
        None,
        true,
        initial_child_page(),
    )]));
    assert_eq!(state.handle_action(TaskBrowserAction::RequestSave), None);
    assert_eq!(
        state.save_draft().map(|draft| draft.destination),
        Some(WorkflowSaveDestination::Project)
    );
    let save = render_plain(&state, 120, 20).join("\n");
    assert!(save.contains("Save workflow"));
    assert!(save.contains("Save to: This project"));
    state.set_save_name("release-notes");
    state.handle_action(TaskBrowserAction::ToggleSaveDestination);
    state.handle_action(TaskBrowserAction::SubmitSave);
    assert_eq!(
        state.take_save_submission(),
        Some(neo_tui::tasks_browser::WorkflowSaveSubmission {
            task_id: "workflow-run".to_owned(),
            name: "release-notes".to_owned(),
            destination: WorkflowSaveDestination::AllProjects,
            replace: false,
        })
    );

    state.handle_action(TaskBrowserAction::RequestSave);
    state.request_save_replace(
        "release-notes",
        WorkflowSaveDestination::AllProjects,
        "Existing release notes",
        "New release notes",
        "/home/example/.neo/workflows",
    );
    let replace = render_plain(&state, 120, 20).join("\n");
    assert!(replace.contains("Replace workflow?"));
    assert!(replace.contains("Existing: Existing release notes"));
    assert!(replace.contains("New: New release notes"));
    assert!(replace.contains("Location: /home/example/.neo/workflows"));
    assert!(replace.contains("Enter replace"));
    state.handle_action(TaskBrowserAction::SubmitSave);
    assert!(
        state
            .take_save_submission()
            .is_some_and(|save| save.replace)
    );
}

#[test]
fn workflow_answer_defaults_cover_schema_shapes_and_validate_submission() {
    let defaults = [
        (json!({"type": "boolean"}), json!(false)),
        (json!({"enum": ["first", "second"]}), json!("first")),
        (
            json!({"type": "object", "properties": {"name": {"type": "string"}}}),
            json!({"name": ""}),
        ),
        (
            json!({"type": "object", "properties": {"nested": {"type": "object", "properties": {"enabled": {"type": "boolean"}}}}}),
            json!({"nested": {"enabled": false}}),
        ),
        (
            json!({"type": "array", "items": {"type": "string"}}),
            json!([]),
        ),
        (
            json!({"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}}}}),
            json!([]),
        ),
        (
            json!({"oneOf": [{"type": "boolean"}, {"type": "string"}]}),
            json!(false),
        ),
        (
            json!({"anyOf": [{"enum": ["yes", "no"]}, {"type": "number"}]}),
            json!("yes"),
        ),
    ];

    for (schema, expected) in defaults {
        let pending = TaskBrowserPendingUserRequest {
            request_id: "request-default".to_owned(),
            prompt: "Provide input".to_owned(),
            answer_schema: schema,
            default: None,
            title: None,
            answer_policy: "required".to_owned(),
        };
        let item = workflow_item(Some(pending), false, initial_child_page());
        let mut state = open_workflow(item.clone());
        state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));
        assert_eq!(
            state.answer_draft().map(|draft| &draft.value),
            Some(&expected)
        );
    }

    let schema = json!({
        "type": "object",
        "required": ["enabled", "tags", "workers"],
        "properties": {
            "enabled": {"type": "boolean"},
            "tags": {"type": "array", "items": {"type": "string"}},
            "workers": {"type": "array", "items": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}}}}
        }
    });
    let pending = TaskBrowserPendingUserRequest {
        request_id: "request-submit".to_owned(),
        prompt: "Provide settings".to_owned(),
        answer_schema: schema,
        default: None,
        title: None,
        answer_policy: "required".to_owned(),
    };
    let item = workflow_item(Some(pending), false, initial_child_page());
    let mut state = open_workflow(item.clone());
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));

    state.set_answer_json("{\"enabled\": true, \"tags\": \"wrong\", \"workers\": []}");
    state.handle_action(TaskBrowserAction::SubmitAnswer);
    assert!(
        state
            .answer_draft()
            .is_some_and(|draft| !draft.field_errors.is_empty())
    );

    state.set_answer_json(
        "{\"enabled\": true, \"tags\": [\"release\"], \"workers\": [{\"name\": \"Ada\"}]}",
    );
    state.handle_action(TaskBrowserAction::SubmitAnswer);
    assert!(state.answer_draft().is_none());
    assert_eq!(
        state
            .take_answer_submission()
            .map(|submission| submission.value),
        Some(json!({"enabled": true, "tags": ["release"], "workers": [{"name": "Ada"}]}))
    );
}

#[test]
fn workflow_answer_dismissal_does_not_reopen_until_requested_again() {
    let pending = TaskBrowserPendingUserRequest {
        request_id: "request-1".to_owned(),
        prompt: "Choose".to_owned(),
        answer_schema: json!({"type": "boolean"}),
        default: None,
        title: None,
        answer_policy: "required".to_owned(),
    };
    let item = workflow_item(Some(pending), false, initial_child_page());
    let mut state = open_workflow(item.clone());
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item.clone()]));
    assert!(state.answer_draft().is_some());

    state.handle_action(TaskBrowserAction::Cancel);
    assert!(state.answer_draft().is_none());
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));
    assert!(state.answer_draft().is_none());

    state.handle_action(TaskBrowserAction::OpenAnswer);
    assert!(state.answer_draft().is_some());
}

#[test]
fn workflow_answer_renders_human_fields_and_keeps_advanced_input_as_a_fallback() {
    let pending = TaskBrowserPendingUserRequest {
        request_id: "request-form".to_owned(),
        prompt: "Choose the release settings.".to_owned(),
        answer_schema: json!({
            "type": "object",
            "properties": {
                "enabled": {"title": "Publish now", "type": "boolean"},
                "strategy": {"title": "Strategy", "enum": ["Safe", "Fast"]},
                "labels": {"title": "Labels", "type": "array", "items": {"enum": ["docs", "code"]}},
                "review": {"title": "Review", "type": "object", "properties": {"owner": {"title": "Owner", "type": "string"}}},
                "retries": {"title": "Retries", "type": "integer"},
                "workers": {"title": "Workers", "type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}}}}
            }
        }),
        default: None,
        title: Some("Release settings".to_owned()),
        answer_policy: "required".to_owned(),
    };
    let item = workflow_item(Some(pending), false, initial_child_page());
    let mut state = open_workflow(item.clone());
    state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));

    let draft = state.answer_draft().expect("answer draft");
    assert!(!draft.form.structured_fallback);
    assert!(
        draft
            .form
            .fields
            .iter()
            .any(|field| matches!(field.control, WorkflowAnswerControl::ObjectArray))
    );
    let rendered = render_plain(&state, 99, 40).join("\n");
    assert!(rendered.contains("Publish now"));
    assert!(rendered.contains("Strategy"));
    assert!(rendered.contains("Owner"));
    assert!(!rendered.contains("JSON Schema"));

    let select_field = |state: &mut TaskBrowserState, path: &str| {
        let draft = state.answer_draft().expect("answer draft");
        let index = draft
            .form
            .fields
            .iter()
            .position(|field| field.path == path)
            .expect("field path");
        state.move_answer_field(index as isize - draft.selected_field as isize);
    };
    select_field(&mut state, "/enabled");
    assert!(state.toggle_selected_answer_value());
    select_field(&mut state, "/strategy");
    assert!(state.cycle_selected_answer_value(1));
    select_field(&mut state, "/labels");
    assert!(state.toggle_selected_answer_value());
    select_field(&mut state, "/review/owner");
    assert!(state.paste_selected_answer_value("Ada"));
    select_field(&mut state, "/retries");
    assert!(state.append_selected_answer_char('3'));
    select_field(&mut state, "/workers");
    assert!(state.paste_selected_answer_value(r#"[{"name":"Ada"}]"#));
    assert_eq!(
        state.answer_draft().map(|draft| &draft.value),
        Some(
            &json!({"enabled": true, "strategy": "Fast", "labels": ["docs"], "review": {"owner": "Ada"}, "retries": 3, "workers": [{"name": "Ada"}]})
        )
    );

    assert!(state.set_answer_field_value("/enabled", json!("not a boolean")));
    state.handle_action(TaskBrowserAction::SubmitAnswer);
    assert!(state.answer_draft().is_some_and(|draft| {
        draft
            .field_errors
            .iter()
            .any(|error| error.starts_with("/enabled:"))
    }));
    assert!(
        render_plain(&state, 99, 40)
            .join("\n")
            .contains("/enabled:")
    );

    let branch = TaskBrowserPendingUserRequest {
        request_id: "request-branch".to_owned(),
        prompt: "Choose a rollout.".to_owned(),
        answer_schema: json!({
            "oneOf": [
                {"title": "Safe rollout", "type": "boolean"},
                {"title": "Fast rollout", "type": "string"}
            ]
        }),
        default: None,
        title: None,
        answer_policy: "required".to_owned(),
    };
    let branch_item = workflow_item(Some(branch), false, initial_child_page());
    let mut branch_state = open_workflow(branch_item.clone());
    branch_state.apply_snapshot(&TaskBrowserSnapshot::new(vec![branch_item]));
    assert!(branch_state.answer_draft().is_some_and(|draft| {
        matches!(
            draft.form.fields.first().map(|field| &field.control),
            Some(WorkflowAnswerControl::BranchChoice(options)) if options == &["Safe rollout", "Fast rollout"]
        )
    }));
    assert!(branch_state.cycle_selected_answer_value(1));
    assert!(branch_state.append_selected_answer_char('x'));
    assert_eq!(
        branch_state.answer_draft().map(|draft| &draft.value),
        Some(&json!("x"))
    );
    assert!(branch_state.cycle_selected_answer_value(-1));
    assert!(branch_state.toggle_selected_answer_value());
    assert!(branch_state.cycle_selected_answer_value(1));
    assert_eq!(
        branch_state.answer_draft().map(|draft| &draft.value),
        Some(&json!("x"))
    );
    assert!(branch_state.cycle_selected_answer_value(-1));
    assert_eq!(
        branch_state.answer_draft().map(|draft| &draft.value),
        Some(&json!(true))
    );

    let advanced = TaskBrowserPendingUserRequest {
        request_id: "request-advanced".to_owned(),
        prompt: "Provide the advanced value.".to_owned(),
        answer_schema: json!({"type": "string", "pattern": "[A-Z]+"}),
        default: None,
        title: None,
        answer_policy: "required".to_owned(),
    };
    let advanced_item = workflow_item(Some(advanced), false, initial_child_page());
    let mut advanced_state = open_workflow(advanced_item.clone());
    advanced_state.apply_snapshot(&TaskBrowserSnapshot::new(vec![advanced_item]));
    assert!(
        advanced_state
            .answer_draft()
            .is_some_and(|draft| draft.form.structured_fallback)
    );
}

#[test]
fn workflow_answer_edits_object_branches_and_object_array_rows_without_losing_drafts() {
    let answer_state = |schema: Value| {
        let pending = TaskBrowserPendingUserRequest {
            request_id: "request-structured-form".to_owned(),
            prompt: "Complete the form.".to_owned(),
            answer_schema: schema,
            default: None,
            title: None,
            answer_policy: "required".to_owned(),
        };
        let item = workflow_item(Some(pending), false, initial_child_page());
        let mut state = open_workflow(item.clone());
        state.apply_snapshot(&TaskBrowserSnapshot::new(vec![item]));
        state
    };
    let select_field = |state: &mut TaskBrowserState, path: &str| {
        state.move_answer_field(isize::MIN);
        for _ in 0..64 {
            if state
                .selected_answer_field()
                .map(|field| field.path.as_str())
                == Some(path)
            {
                return;
            }
            state.move_answer_field(1);
        }
        panic!("field {path} is not visible");
    };

    let mut text_branches = answer_state(json!({
        "oneOf": [
            {"title": "First", "type": "string"},
            {"title": "Second", "type": "string"}
        ]
    }));
    assert!(text_branches.append_selected_answer_char('a'));
    assert!(text_branches.cycle_selected_answer_value(1));
    assert!(text_branches.append_selected_answer_char('b'));
    assert!(text_branches.cycle_selected_answer_value(-1));
    assert!(text_branches.append_selected_answer_char('c'));
    assert_eq!(
        text_branches.answer_draft().map(|draft| &draft.value),
        Some(&json!("ac"))
    );
    assert!(text_branches.cycle_selected_answer_value(1));
    assert_eq!(
        text_branches.answer_draft().map(|draft| &draft.value),
        Some(&json!("b"))
    );

    let mut object_branches = answer_state(json!({
        "oneOf": [
            {
                "title": "Basic",
                "type": "object",
                "properties": {
                    "name/alias": {"title": "Name", "type": "string"},
                    "settings~group": {
                        "title": "Settings",
                        "type": "object",
                        "properties": {"enabled": {"title": "Enabled", "type": "boolean"}}
                    }
                }
            },
            {
                "title": "Advanced",
                "type": "object",
                "properties": {"label~/": {"title": "Label", "type": "string"}}
            }
        ]
    }));
    select_field(&mut object_branches, "/name~1alias");
    assert!(object_branches.paste_selected_answer_value("Ada"));
    select_field(&mut object_branches, "/settings~0group/enabled");
    assert!(object_branches.toggle_selected_answer_value());
    let basic_form = render_plain(&object_branches, 99, 40).join("\n");
    assert!(basic_form.contains("Basic > Settings > Enabled"));
    select_field(&mut object_branches, "");
    assert!(object_branches.cycle_selected_answer_value(1));
    select_field(&mut object_branches, "/label~0~1");
    assert!(object_branches.paste_selected_answer_value("Careful"));
    assert_eq!(
        object_branches.answer_draft().map(|draft| &draft.value),
        Some(&json!({"label~/": "Careful"}))
    );
    select_field(&mut object_branches, "");
    assert!(object_branches.cycle_selected_answer_value(-1));
    assert_eq!(
        object_branches.answer_draft().map(|draft| &draft.value),
        Some(&json!({"name/alias": "Ada", "settings~group": {"enabled": true}}))
    );

    let mut rows = answer_state(json!({
        "type": "object",
        "properties": {
            "workers/list": {
                "title": "Workers",
                "type": "array",
                "items": {
                    "title": "Worker",
                    "type": "object",
                    "properties": {
                        "name~/alias": {"title": "Name", "type": "string"},
                        "settings/group": {
                            "title": "Settings",
                            "type": "object",
                            "properties": {"enabled": {"title": "Enabled", "type": "boolean"}}
                        }
                    }
                }
            }
        }
    }));
    select_field(&mut rows, "/workers~1list");
    assert!(rows.append_selected_answer_object_row());
    select_field(&mut rows, "/workers~1list/name~0~1alias");
    assert!(rows.paste_selected_answer_value("Ada"));
    select_field(&mut rows, "/workers~1list/settings~1group/enabled");
    assert!(rows.toggle_selected_answer_value());
    select_field(&mut rows, "/workers~1list");
    let row_form = render_plain(&rows, 99, 40).join("\n");
    assert!(row_form.contains("Workers > Worker > Settings > Enabled"));
    assert!(row_form.contains("Delete remove row"));
    select_field(&mut rows, "/workers~1list");
    assert!(rows.append_selected_answer_object_row());
    select_field(&mut rows, "/workers~1list/name~0~1alias");
    assert!(rows.paste_selected_answer_value("Bob"));
    select_field(&mut rows, "/workers~1list");
    assert!(rows.cycle_selected_answer_value(-1));
    select_field(&mut rows, "/workers~1list/name~0~1alias");
    assert!(rows.paste_selected_answer_value("Ada updated"));
    select_field(&mut rows, "/workers~1list");
    assert!(rows.cycle_selected_answer_value(1));
    assert!(rows.remove_selected_answer_object_row());
    assert_eq!(
        rows.answer_draft().map(|draft| &draft.value),
        Some(
            &json!({"workers/list": [{"name~/alias": "Ada updated", "settings/group": {"enabled": true}}]})
        )
    );
}

#[test]
fn workflow_renderer_fits_supported_widths_and_places_usage_in_details() {
    let mut state = open_workflow(workflow_item(None, true, initial_child_page()));
    for width in [32, 70, 99, 100, 120, 180] {
        for height in [12, 20, 40] {
            let lines = render_plain(&state, width, height);
            assert_eq!(lines.len(), height, "width={width}, height={height}");
            assert!(
                lines.iter().all(|line| visible_width(line) <= width),
                "width={width}:\n{}",
                lines.join("\n")
            );
        }
    }

    let agents = render_plain(&state, 120, 40).join("\n");
    let agent_row = agents
        .lines()
        .find(|line| line.contains("Source worker"))
        .expect("agent row");
    assert!(!agent_row.contains("120"));

    state.handle_action(TaskBrowserAction::ToggleWorkflowFocus);
    state.handle_action(TaskBrowserAction::OpenWorkflowChildDetails);
    let details = render_plain(&state, 120, 40).join("\n");
    assert!(details.contains("Usage: {\"tokens\":120}"));
    assert!(details.contains("S save"));
}
