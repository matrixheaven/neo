//! Interactive tasks behavior (moved from `tests.rs`).

use std::fs;

use super::super::*;
use super::*;
use neo_agent_core::{AgentEvent, ApprovalAction, ApprovalResponse, PendingQuestion};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    shell::OverlayKind,
    transcript::MouseKind,
};
use tokio::sync::oneshot;

#[test]
fn task_stop_for_question_closes_pending_question_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "session",
        "model",
        test_workspace_root(),
        |_| async { Ok(Vec::new()) },
    );
    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Continue?".to_owned(),
            header: None,
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "Yes".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "No".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
        workflow_origin: None,
    });
    assert!(controller.chrome().question_dialog_is_focused());

    controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "TaskStop".to_owned(),
        result: neo_agent_core::ToolResult::ok("stopped").with_details(serde_json::json!({
            "task_id": "question-1",
            "kind": "question",
            "status": "stopped"
        })),
        workflow_origin: None,
        output_ref: None,
    });

    assert!(!controller.chrome().question_dialog_is_focused());
    assert!(!controller.pending_questions.contains_key("question-1"));
    assert!(
        !controller
            .pending_question_prompts
            .contains_key("question-1")
    );
}

#[tokio::test]
async fn background_bash_one_down_submits_the_visible_reject_action() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (pending, response_rx) = make_pending_approval(background_bash_request());
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects Reject");
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("2. Reject"),
        "visible option should be Reject after one Down: {snapshot}"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter submits Reject");
    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::Reject,
            ..
        }
    ));
}

#[tokio::test]
async fn resume_rehydrates_todo_panel_and_clears_prior_session_todos() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    writer
        .append(&AgentEvent::TodoUpdated {
            turn: 1,
            todos: vec![
                neo_agent_core::TodoEventData {
                    title: "Task 4".to_owned(),
                    status: "in_progress".to_owned(),
                },
                neo_agent_core::TodoEventData {
                    title: "Task 12".to_owned(),
                    status: "pending".to_owned(),
                },
            ],
        })
        .await
        .expect("append todo update");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");
    assert_eq!(loaded.todos.len(), 2);

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.rebuild_transcript_from_session(&loaded);
    let titles = controller
        .tui
        .chrome()
        .todo_items()
        .iter()
        .map(|todo| todo.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(titles, ["Task 4", "Task 12"]);

    controller.rebuild_transcript_from_session(&LoadedSessionTranscript::new(
        "empty",
        Vec::new(),
        Vec::new(),
    ));
    assert!(controller.tui.chrome().todo_items().is_empty());
}

#[tokio::test]
async fn add_workspace_approved_persists_enabled_read_only_entry() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let added_dir = temp.path().join("added");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::create_dir_all(&added_dir).expect("create added");
    let store = crate::workspaces::WorkspaceStore::new(temp.path().join("workspaces.json"));

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project_dir.clone(),
    };
    controller.local_config = Some(config);
    controller.set_workspace_store(store.clone());

    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open workspace manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add workspace");
    controller
        .handle_input_event(InputEvent::Paste(added_dir.display().to_string()))
        .await
        .expect("paste path");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit path");

    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::ConfirmDialog(_))
        ),
        "add must show confirmation before persistence"
    );
    assert!(
        store
            .read_project(&project_dir)
            .expect("read project before confirmation")
            .entries
            .is_empty(),
        "workspace entry must not persist before confirmation"
    );

    controller
        .handle_input_event(InputEvent::Insert('Y'))
        .await
        .expect("approve add");

    let project = store.read_project(&project_dir).expect("read project");
    assert_eq!(project.entries.len(), 1);
    let entry = &project.entries[0];
    assert_eq!(
        entry.path,
        added_dir.canonicalize().expect("canonical added")
    );
    assert!(entry.enabled);
    assert!(entry.read);
    assert!(!entry.write);
}

#[tokio::test]
async fn add_workspace_approval_returns_to_visible_manager_and_single_escape_closes_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let added_dir = temp.path().join("added");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::create_dir_all(&added_dir).expect("create added");
    let store = crate::workspaces::WorkspaceStore::new(temp.path().join("workspaces.json"));

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project_dir.clone(),
    };
    controller.local_config = Some(config);
    controller.set_workspace_store(store);

    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open workspace manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add workspace");
    controller
        .handle_input_event(InputEvent::Paste(added_dir.display().to_string()))
        .await
        .expect("paste path");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit path");
    controller
        .handle_input_event(InputEvent::Insert('Y'))
        .await
        .expect("approve add");

    assert!(
        matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::WorkspaceManager(_))
        ),
        "approval should return focus to the workspace manager"
    );
    assert!(controller.chrome().focused_overlay_blocks_prompt());
    let visible = controller
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("[on ] [R ] [W-]"), "{visible}");
    assert!(visible.contains("[read-only] · [active]"), "{visible}");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectCancel))
        .await
        .expect("close workspace manager");
    assert!(controller.chrome().focused_overlay().is_none());

    controller.type_text("hello");
    assert_eq!(controller.chrome().prompt().text, "hello");
}

#[tokio::test]
async fn workspace_write_toggle_keeps_read_enabled() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let added_dir = temp.path().join("added");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::create_dir_all(&added_dir).expect("create added");
    let store = crate::workspaces::WorkspaceStore::new(temp.path().join("workspaces.json"));
    let added_dir = added_dir.canonicalize().expect("canonical added");
    store
        .write_project(
            &project_dir,
            crate::workspaces::WorkspaceProject {
                entries: vec![crate::workspaces::WorkspaceEntry::read_only(
                    added_dir.clone(),
                )],
            },
        )
        .expect("seed workspace store");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project_dir.clone(),
    };
    controller.local_config = Some(config);
    controller.set_workspace_store(store.clone());

    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open workspace manager");
    controller
        .handle_input_event(InputEvent::Insert('W'))
        .await
        .expect("toggle write");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::ConfirmDialog(_))
        ),
        "write toggle must show confirmation"
    );
    controller
        .handle_input_event(InputEvent::Insert('Y'))
        .await
        .expect("approve write toggle");

    let project = store.read_project(&project_dir).expect("read project");
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.entries[0].path, added_dir);
    assert!(project.entries[0].read);
    assert!(project.entries[0].write);
}

#[tokio::test]
async fn workspace_read_toggle_off_turns_write_off() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    let added_dir = temp.path().join("added");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::create_dir_all(&added_dir).expect("create added");
    let store = crate::workspaces::WorkspaceStore::new(temp.path().join("workspaces.json"));
    let added_dir = added_dir.canonicalize().expect("canonical added");
    store
        .write_project(
            &project_dir,
            crate::workspaces::WorkspaceProject {
                entries: vec![crate::workspaces::WorkspaceEntry {
                    path: added_dir.clone(),
                    enabled: true,
                    read: true,
                    write: true,
                }],
            },
        )
        .expect("seed workspace store");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project_dir.clone(),
    };
    controller.local_config = Some(config);
    controller.set_workspace_store(store.clone());

    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open workspace manager");
    controller
        .handle_input_event(InputEvent::Insert('R'))
        .await
        .expect("toggle read");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::ConfirmDialog(_))
        ),
        "read toggle must show confirmation"
    );
    controller
        .handle_input_event(InputEvent::Insert('Y'))
        .await
        .expect("approve read toggle");

    let project = store.read_project(&project_dir).expect("read project");
    assert_eq!(project.entries.len(), 1);
    assert_eq!(project.entries[0].path, added_dir);
    assert!(!project.entries[0].read);
    assert!(!project.entries[0].write);
}

#[tokio::test]
async fn slash_tasks_opens_task_browser_while_main_turn_is_running() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);

    controller.type_text("main question");
    controller
        .submit_current_prompt()
        .await
        .expect("main turn starts");
    assert!(controller.active_turn.is_some());

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("task browser opens");
    assert_eq!(browser.snapshot().items().len(), 1);
    assert_eq!(browser.snapshot().items()[0].id, "question-1");
    assert!(!transcript_has_status(
        &controller,
        "active_background_tasks: 1"
    ));
    assert!(controller.active_turn.is_some());
}

#[tokio::test]
async fn task_browser_escape_closes_overlay_and_tab_toggles_filter() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("toggle filter");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .filter(),
        neo_tui::tasks_browser::TaskBrowserFilter::Active
    );

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("close browser");
    assert!(controller.chrome().task_browser_state().is_none());
}

#[tokio::test]
async fn task_browser_stays_inside_existing_fullscreen_surface() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);

    // The interactive session already owns the fullscreen surface: a normal
    // frame is one bounded line set.
    let plain = controller.tui.render_terminal_frame(80, 24);
    assert!(plain.lines.len() <= 24, "plain frame must be bounded");

    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");
    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("task browser opens");
    assert_eq!(browser.snapshot().items().len(), 1);

    // Task Browser renders as an overlay inside the already-fullscreen
    // session: one bounded frame with no physical transition.
    let overlay_frame = controller.tui.render_terminal_frame(80, 24);
    assert!(
        overlay_frame.lines.len() <= 24,
        "overlay frame must stay bounded"
    );
    let overlay_text = overlay_frame
        .lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        overlay_text.contains("TASKS"),
        "overlay frame:\n{overlay_text}"
    );

    // The browser stays operable while the surface stays the same.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("toggle filter");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .filter(),
        neo_tui::tasks_browser::TaskBrowserFilter::Active
    );

    // Closing the overlay returns the primary document to the same frame.
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("close browser");
    assert!(controller.chrome().task_browser_state().is_none());
    let restored = controller.tui.render_terminal_frame(80, 24);
    assert!(restored.lines.len() <= 24);
}

#[tokio::test]
async fn task_browser_mouse_wheel_moves_selection_without_prompt_history() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    config
        .background_tasks
        .start_question("question-2".to_owned(), "Pick two".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");
    // The wheel routes through the rendered layout, so a known frame must
    // exist before wheeling. 120x24 is wide: column 10 is inside the task
    // list column, so the wheel moves the task selection.
    let _ = controller.tui.render_terminal_frame(120, 24);

    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("browser open");
    let first_task_id = browser.snapshot().items()[0].id.clone();
    let second_task_id = browser.snapshot().items()[1].id.clone();
    assert_eq!(browser.selected_task_id(), Some(first_task_id.as_str()));
    controller
        .handle_input_event(wheel_event(MouseKind::ScrollDown))
        .await
        .expect("wheel moves selection");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .unwrap()
            .selected_task_id(),
        Some(second_task_id.as_str())
    );
    assert!(controller.chrome().prompt().text.is_empty());
}

#[tokio::test]
async fn task_browser_mouse_click_selects_rows_and_wheel_uses_pointed_pane() {
    // A click selects the task row under the pointer and a wheel moves the
    // pane under the pointer — over the task list it moves the task
    // selection, over the inspector output region it scrolls output only.
    // Pointer events never reach the prompt or the transcript selection.
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir.clone());
    for (id, question) in [
        ("question-1", "Pick one"),
        ("question-2", "Pick two"),
        ("question-3", "Pick three"),
    ] {
        config
            .background_tasks
            .start_question(id.to_owned(), question.to_owned())
            .await;
    }
    controller.local_config = Some(config);
    let mouse = |kind: MouseKind, column: u16, row: u16| {
        InputEvent::Mouse(MouseEvent {
            kind,
            button: MouseButton::Left,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    };
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");
    let frame = controller.tui.render_terminal_frame(120, 24);
    // The background-task list order is recency-based and therefore
    // timing-dependent between equally fresh tasks, so derive the expected
    // selection order from the rendered browser state instead of assuming
    // creation order.
    let ids = controller
        .chrome()
        .task_browser_state()
        .expect("browser open")
        .visible_items()
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3, "all three questions must be listed: {ids:?}");
    // The inspector shows the selected (first) task, so the second task's
    // handle appears only in its task-list row.
    let row_of_second = frame
        .lines
        .iter()
        .position(|line| neo_tui::primitive::strip_ansi(line).contains(&ids[1]))
        .expect("second task row rendered") as u16;
    assert!(
        row_of_second >= 2,
        "the second task row must render inside the task pane, got {row_of_second}"
    );

    fn selected(controller: &InteractiveController) -> Option<String> {
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_task_id()
            .map(str::to_owned)
    }
    assert_eq!(selected(&controller), Some(ids[0].clone()));

    // Press selects the row under the pointer; release changes nothing.
    controller
        .handle_input_event(mouse(MouseKind::Press, 3, row_of_second))
        .await
        .expect("click task row");
    assert_eq!(selected(&controller), Some(ids[1].clone()));
    controller
        .handle_input_event(mouse(MouseKind::Release, 3, row_of_second))
        .await
        .expect("release task row");
    assert_eq!(selected(&controller), Some(ids[1].clone()));

    // Wheel over the task list moves the task selection.
    controller
        .handle_input_event(mouse(MouseKind::ScrollDown, 3, row_of_second))
        .await
        .expect("wheel task list");
    assert_eq!(selected(&controller), Some(ids[2].clone()));
    controller
        .handle_input_event(mouse(MouseKind::ScrollUp, 3, row_of_second))
        .await
        .expect("wheel task list up");
    assert_eq!(selected(&controller), Some(ids[1].clone()));

    // Wheel over the inspector output region scrolls output only: the task
    // selection stays put and the single preview line keeps the scroll
    // clamped at zero.
    controller
        .handle_input_event(mouse(MouseKind::ScrollDown, 60, 20))
        .await
        .expect("wheel inspector output");
    assert_eq!(selected(&controller), Some(ids[1].clone()));
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .output_scroll(),
        0
    );

    // A press on the footer and a drag stay consumed without selecting
    // prompt or transcript text; the drag itself forms a frame selection
    // over the visible rows (final-frame selection covers the browser).
    controller
        .handle_input_event(mouse(MouseKind::Press, 3, 23))
        .await
        .expect("press footer");
    controller
        .handle_input_event(mouse(MouseKind::Drag, 3, row_of_second))
        .await
        .expect("drag task row");
    assert_eq!(selected(&controller), Some(ids[1].clone()));
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(
        !controller.chrome().prompt().selection_range().is_some()
            && !controller.transcript().has_transcript_selection(),
        "pointer events must never reach the prompt or transcript selection"
    );
    assert!(
        controller.tui.has_any_selection(),
        "the drag forms a frame selection over the visible browser rows"
    );

    // Workflow: with the Steps/Agents split at 120x24, a click selects the
    // step row under the pointer, the wheel over Steps moves step selection,
    // and the wheel over the Agents pane (no agents yet) changes nothing.
    let mut workflow_controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir.clone());
    let runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let handle = runtime
        .create_run(
            &sessions_dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "pointer-nav".to_owned(),
                description: "pointer nav".to_owned(),
                phases: vec![
                    neo_agent_core::workflow::WorkflowPhase {
                        id: "work".to_owned(),
                        description: "work".to_owned(),
                    },
                    neo_agent_core::workflow::WorkflowPhase {
                        id: "verify".to_owned(),
                        description: "verify".to_owned(),
                    },
                ],
                script: "neo.phase('work')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create workflow");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    let run_id = handle.run_id.0.clone();
    config
        .background_tasks
        .start_workflow(run_id, "pointer nav".to_owned(), handle)
        .await
        .expect("register workflow");
    workflow_controller.local_config = Some(config);
    workflow_controller.type_text("/tasks");
    workflow_controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show workflow");
    let frame = workflow_controller.tui.render_terminal_frame(120, 24);
    let verify_row = frame
        .lines
        .iter()
        .position(|line| neo_tui::primitive::strip_ansi(line).contains("verify"))
        .expect("verify step row rendered") as u16;
    fn selected_step(controller: &InteractiveController) -> Option<String> {
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_workflow_step()
            .and_then(|step| step.key.phase_id.clone())
    }
    assert_eq!(selected_step(&workflow_controller), Some("work".to_owned()));
    workflow_controller
        .handle_input_event(mouse(MouseKind::Press, 3, verify_row))
        .await
        .expect("click step row");
    assert_eq!(
        selected_step(&workflow_controller),
        Some("verify".to_owned())
    );
    workflow_controller
        .handle_input_event(mouse(MouseKind::ScrollUp, 3, verify_row))
        .await
        .expect("wheel steps");
    assert_eq!(selected_step(&workflow_controller), Some("work".to_owned()));
    workflow_controller
        .handle_input_event(mouse(MouseKind::ScrollDown, 80, 4))
        .await
        .expect("wheel agents pane");
    assert_eq!(selected_step(&workflow_controller), Some("work".to_owned()));
    assert_eq!(
        workflow_controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Steps,
        "the Agents-pane wheel must not move step selection or focus"
    );
    assert!(workflow_controller.chrome().prompt().text.is_empty());
    assert!(!workflow_controller.tui.has_any_selection());
}

#[tokio::test]
async fn task_browser_refresh_updates_snapshot() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .start_question("question-2".to_owned(), "Pick another".to_owned())
        .await;
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("refresh browser");

    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("browser remains open");
    assert_eq!(browser.snapshot().items().len(), 2);
    assert!(
        browser
            .snapshot()
            .items()
            .iter()
            .any(|item| item.id == "question-2")
    );
}

#[tokio::test]
async fn task_browser_reopening_updates_existing_overlay_in_place() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");
    let overlay_count = controller.chrome().overlays().len();
    let focused_overlay = controller.chrome().focused_overlay_id();

    controller.show_background_tasks().await;

    assert_eq!(controller.chrome().overlays().len(), overlay_count);
    assert_eq!(controller.chrome().focused_overlay_id(), focused_overlay);
    assert!(controller.chrome().task_browser_state().is_some());
}

#[tokio::test]
async fn task_browser_periodic_refresh_updates_open_browser() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .start_question("question-2".to_owned(), "Pick another".to_owned())
        .await;
    controller.last_task_browser_refresh = Some(
        Instant::now()
            .checked_sub(TASK_BROWSER_REFRESH_INTERVAL)
            .and_then(|instant| instant.checked_sub(Duration::from_millis(1)))
            .expect("now is far enough in the past"),
    );
    assert!(controller.maybe_refresh_task_browser().await);

    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("browser remains open");
    assert_eq!(browser.snapshot().items().len(), 2);
    assert!(controller.last_task_browser_refresh.is_some());
}

#[tokio::test]
async fn task_browser_stop_confirmation_stops_selected_task() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("request stop");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .stop_confirmation_task_id(),
        Some("question-1")
    );
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm stop");

    let browser = controller
        .chrome()
        .task_browser_state()
        .expect("browser remains open");
    assert_eq!(
        browser.snapshot().items()[0].status,
        neo_tui::tasks_browser::TaskBrowserStatus::Cancelled
    );
    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("config")
            .background_tasks
            .snapshot("question-1")
            .await
            .expect("snapshot")
            .status,
        neo_agent_core::tools::BackgroundTaskStatus::Cancelled
    );
}

#[tokio::test]
async fn task_browser_workflow_controls_use_human_handle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir.clone());
    let runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let handle = runtime
        .create_run(
            &sessions_dir,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "browser-controls".to_owned(),
                description: "browser controls".to_owned(),
                phases: vec![
                    neo_agent_core::workflow::WorkflowPhase {
                        id: "work".to_owned(),
                        description: "work".to_owned(),
                    },
                    neo_agent_core::workflow::WorkflowPhase {
                        id: "verify".to_owned(),
                        description: "verify".to_owned(),
                    },
                ],
                script: "neo.phase('work')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create workflow");
    let run_id = handle.run_id.clone();
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("enter running");
    config
        .background_tasks
        .start_workflow(
            run_id.0.clone(),
            "browser controls".to_owned(),
            handle.clone(),
        )
        .await
        .expect("register workflow");
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .and_then(neo_tui::tasks_browser::TaskBrowserState::workflow_item)
            .map(|item| item.id.as_str()),
        Some(run_id.0.as_str())
    );
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Steps
    );
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_workflow_step()
            .and_then(|step| step.key.phase_id.as_deref()),
        Some("work")
    );
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("down key")))
        .await
        .expect("down selects next workflow step");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_workflow_step()
            .and_then(|step| step.key.phase_id.as_deref()),
        Some("verify")
    );
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("up key")))
        .await
        .expect("up selects previous workflow step");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .selected_workflow_step()
            .and_then(|step| step.key.phase_id.as_deref()),
        Some("work")
    );
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("right").expect("right key")))
        .await
        .expect("right switches workflow focus");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Agents
    );
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("left").expect("left key")))
        .await
        .expect("left switches workflow focus");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("tab").expect("tab key")))
        .await
        .expect("tab switches workflow focus");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Agents
    );
    controller
        .handle_input_event(InputEvent::Insert('p'))
        .await
        .expect("pause workflow");
    assert!(handle.is_pause_requested());
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Paused
    );
    runtime
        .bind_runner(|handle, _metadata, _session_dir| async move {
            handle.stop_token().cancelled().await;
            Ok(())
        })
        .expect("bind test runner");
    controller.refresh_task_browser().await;
    controller
        .handle_input_event(InputEvent::Insert('p'))
        .await
        .expect("resume workflow");
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Running
    );

    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("request stop");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm stop");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !handle.snapshot().await.state.is_terminal() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("workflow reaches terminal state");
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
}

#[tokio::test]
async fn task_browser_plain_task_controls_remain_available() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let config = test_config(temp.path(), sessions_dir);
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    controller.local_config = Some(config);
    controller.type_text("/tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("show tasks");

    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("handle enter");

    assert!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .stop_confirmation_task_id()
            .is_none()
    );
    assert!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .task_details_open()
    );
    controller
        .handle_input_event(InputEvent::Insert('o'))
        .await
        .expect("open task output");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Output
    );
    controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .start_question("question-2".to_owned(), "Pick another".to_owned())
        .await;
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("refresh task list");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .snapshot()
            .items()
            .len(),
        2
    );
}
