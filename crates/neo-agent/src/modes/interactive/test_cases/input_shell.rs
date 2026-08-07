//! Shell-mode input behavior (split from `input.rs`).

use std::collections::VecDeque;

use neo_agent_core::{AgentEvent, AgentMessage, Content, PermissionMode, StopReason};
use neo_tui::input::{InputEvent, KeyId};

use super::super::*;
use super::*;

#[tokio::test]
async fn shell_mode_bang_empty_prompt_enters_and_empty_cancel_exits() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("bang enters shell mode");

    assert!(controller.chrome().shell_mode_active());
    assert_eq!(controller.chrome().prompt().text, "");

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("empty cancel exits shell mode");

    assert!(!controller.chrome().shell_mode_active());
}

#[tokio::test]
async fn idle_shell_mode_workflow_slash_returns_to_model_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = demo_named_workflow_config(&temp, PermissionMode::Yolo);
    let requests = Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        config.project_dir.clone(),
        move |request| {
            seen_requests.lock().expect("requests lock").push(request);
            async {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            }
        },
    );
    controller.local_config = Some(config);

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("/workflow run this");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit workflow slash");
    controller
        .wait_for_active_turn()
        .await
        .expect("workflow turn completes");

    assert!(!controller.chrome().shell_mode_active());
    assert_eq!(
        requests.lock().expect("requests lock")[0].prompt,
        vec![Content::text("/workflow run this")]
    );
}

#[tokio::test]
async fn shell_mode_paste_bang_command_enters_and_strips_prefix() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Paste("!pwd".to_owned()))
        .await
        .expect("paste bang command");

    assert!(controller.chrome().shell_mode_active());
    assert_eq!(controller.chrome().prompt().text, "pwd");
}

#[tokio::test]
async fn shell_mode_enter_executes_persists_and_does_not_start_model_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let model_turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed_turns = Arc::clone(&model_turns);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        move |_request| {
            let observed_turns = Arc::clone(&observed_turns);
            async move {
                observed_turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));
    let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_commands = Arc::clone(&commands);
    controller.set_shell_driver(Arc::new(move |request| {
        let observed_commands = Arc::clone(&observed_commands);
        Box::pin(async move {
            observed_commands
                .lock()
                .expect("command lock")
                .push(request.command);
            Ok(completed_shell_result("neo\n"))
        })
    }));

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("printf neo");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("run shell command");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("shell command completes");

    assert_eq!(
        commands.lock().expect("command lock").as_slice(),
        ["printf neo"]
    );
    assert_eq!(model_turns.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().shell_mode_active());
    assert!(!controller.chrome().shell_running());
    assert_eq!(
        controller.chrome().working_label(),
        None,
        "finished shell command should return chrome to editing state"
    );
    assert!(
        replay_session_messages(&controller)
            .await
            .iter()
            .any(|message| matches!(
                message,
                AgentMessage::ShellCommand {
                    command,
                    stdout,
                    outcome: neo_agent_core::ShellCommandOutcome::Completed,
                    ..
                } if command.as_ref() == "printf neo" && stdout.as_ref() == "neo\n"
            )),
        "shell command result should be persisted as AgentMessage::ShellCommand"
    );
    assert!(
        !controller
            .prompt_history
            .as_ref()
            .is_some_and(|_| transcript_has_status(&controller, "printf neo")),
        "shell commands must not be persisted to prompt history"
    );
}

#[tokio::test]
async fn shell_mode_omits_execution_timeout_for_user_commands() {
    let observed_timeout = Arc::new(std::sync::Mutex::new(Some(Duration::from_secs(1))));
    let captured_timeout = Arc::clone(&observed_timeout);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    controller.local_config = Some(config);
    controller.set_shell_driver(Arc::new(move |request| {
        let captured_timeout = Arc::clone(&captured_timeout);
        Box::pin(async move {
            *captured_timeout.lock().expect("timeout lock") = request.timeout;
            Ok(completed_shell_result(""))
        })
    }));

    controller.type_text("!true");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("run shell command");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("shell completes");

    assert_eq!(*observed_timeout.lock().expect("timeout lock"), None);
}

#[tokio::test]
async fn shell_mode_enter_while_shell_busy_queues_and_drains_fifo() {
    let releases = Arc::new(std::sync::Mutex::new(VecDeque::from([
        tokio::sync::oneshot::channel::<()>().1,
        tokio::sync::oneshot::channel::<()>().1,
    ])));
    let (first_tx, first_rx) = tokio::sync::oneshot::channel::<()>();
    let (second_tx, second_rx) = tokio::sync::oneshot::channel::<()>();
    *releases.lock().expect("release lock") = VecDeque::from([first_rx, second_rx]);
    let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_commands = Arc::clone(&commands);
    let observed_releases = Arc::clone(&releases);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_shell_driver(Arc::new(move |request| {
        let observed_commands = Arc::clone(&observed_commands);
        let release = observed_releases
            .lock()
            .expect("release lock")
            .pop_front()
            .expect("release receiver");
        Box::pin(async move {
            observed_commands
                .lock()
                .expect("command lock")
                .push(request.command);
            let _ = release.await;
            Ok(completed_shell_result(""))
        })
    }));

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("one");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start first shell command");
    controller.type_text("two");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("queue second shell command");

    assert!(controller.chrome().shell_running());
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_shell_commands()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["two"]
    );

    first_tx.send(()).expect("release first");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("drain queued shell command");
    assert_eq!(
        commands.lock().expect("command lock").as_slice(),
        ["one", "two"]
    );
    assert!(controller.chrome().shell_running());
    second_tx.send(()).expect("release second");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("second shell command completes");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_shell_commands()
            .len(),
        0
    );
}

#[tokio::test]
async fn shell_mode_ctrl_s_does_not_steer_and_alt_up_edits_recent_shell_queue() {
    let mut controller = running_turn_controller().await;

    controller.tui.chrome_mut().enter_shell_mode();
    controller.type_text("not a steer");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s in shell mode is ignored");
    assert_eq!(
        controller.chrome().prompt().text,
        "not a steer",
        "Ctrl+S must not steer shell text"
    );

    controller
        .tui
        .chrome_mut()
        .pending_input_mut()
        .queue_follow_up("follow up");
    controller
        .tui
        .chrome_mut()
        .pending_input_mut()
        .queue_shell_command("shell queued");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("alt+up edits queued shell command");

    assert_eq!(controller.chrome().prompt().text, "shell queued");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["follow up"],
        "Alt+Up should prefer queued shell commands in shell mode"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn shell_mode_ctrl_b_detaches_running_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.type_text("!sleep 5");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start shell command");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let tasks = controller
                .local_config
                .as_ref()
                .expect("config")
                .background_tasks
                .list(true, 10)
                .await;
            if !tasks.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("shell task should register before ctrl+b");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+b").expect("valid key")))
        .await
        .expect("ctrl+b detaches");

    assert!(!controller.chrome().shell_running());
    assert!(
        replay_session_messages(&controller)
            .await
            .iter()
            .any(|message| matches!(
                message,
                AgentMessage::ShellCommand {
                    outcome: neo_agent_core::ShellCommandOutcome::Backgrounded { .. },
                    ..
                }
            )),
        "detached shell command should persist as backgrounded"
    );
}

#[tokio::test]
async fn shell_mode_ctrl_b_detaches_current_shell_task_not_other_background_task() {
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
        .start_question("question-1".to_owned(), "Existing question".to_owned())
        .await;
    controller.local_config = Some(config);

    controller.type_text("!sleep 5");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start shell command");
    let (question_before, shell_task_id) = loop {
        let tasks = controller
            .local_config
            .as_ref()
            .expect("config")
            .background_tasks
            .list(true, 10)
            .await;
        let question = tasks
            .iter()
            .find(|task| task.task_id == "question-1")
            .cloned();
        let shell = tasks
            .iter()
            .find(|task| task.task_id != "question-1")
            .cloned();
        if let (Some(question), Some(shell)) = (question, shell) {
            break (question, shell.task_id);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+b").expect("valid key")))
        .await
        .expect("ctrl+b detaches");

    let question_after = controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .snapshot("question-1")
        .await
        .expect("question remains");
    assert!(question_after.elapsed >= question_before.elapsed);
    assert!(
        replay_session_messages(&controller)
            .await
            .iter()
            .any(|message| matches!(
                message,
                AgentMessage::ShellCommand {
                    outcome: neo_agent_core::ShellCommandOutcome::Backgrounded { task_id },
                    ..
                } if task_id.as_ref() == shell_task_id.as_str()
            )),
        "ctrl+b should persist the actual foreground shell task id"
    );
    let _ = controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .stop(&shell_task_id, "test cleanup", 1024)
        .await;
}

#[tokio::test]
async fn shell_mode_detach_uses_shared_background_tasks_for_next_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let captured_task_count = Arc::new(std::sync::Mutex::new(None));
    let observed_task_count = Arc::clone(&captured_task_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        move |request| {
            let observed_task_count = Arc::clone(&observed_task_count);
            async move {
                let count = match request.base_config {
                    Some(config) => config.background_tasks.list(true, 10).await.len(),
                    None => 0,
                };
                *observed_task_count.lock().expect("task count") = Some(count);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.type_text("!sleep 5");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start shell command");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let tasks = controller
                .local_config
                .as_ref()
                .expect("config")
                .background_tasks
                .list(true, 10)
                .await;
            if !tasks.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("shell task should register before ctrl+b");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+b").expect("valid key")))
        .await
        .expect("ctrl+b detaches");

    let shared_tasks = controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .list(true, 10)
        .await;
    assert_eq!(shared_tasks.len(), 1);

    controller.tui.chrome_mut().exit_shell_mode();
    controller.type_text("inspect tasks");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start follow-up turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("follow-up completes");

    assert_eq!(
        *captured_task_count.lock().expect("task count"),
        Some(1),
        "next model turn should see detached shell task via shared manager"
    );
    let _ = controller
        .local_config
        .as_ref()
        .expect("config")
        .background_tasks
        .stop(&shared_tasks[0].task_id, "test cleanup", 1024)
        .await;
}

#[tokio::test]
async fn ctrl_b_detaches_foreground_delegate_into_shared_background_tasks() {
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
    let running = config
        .multi_agent
        .start_foreground_delegate_for_test("detach foreground delegate");
    controller.local_config = Some(config);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+b").expect("valid key")))
        .await
        .expect("ctrl+b handled");

    let config = controller.local_config.as_ref().expect("config");
    let tasks = config.background_tasks.list(false, 10).await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, running.id.as_str());
    assert_eq!(
        tasks[0].kind,
        neo_agent_core::tools::BackgroundTaskKind::Delegate
    );
    let runtime_snapshot = config
        .multi_agent
        .snapshot(&running.id)
        .expect("shared runtime has delegate");
    assert_eq!(
        runtime_snapshot.mode,
        neo_agent_core::multi_agent::AgentRunMode::Background
    );
}

#[tokio::test]
async fn shell_mode_slash_tasks_opens_browser_instead_of_running_shell_command() {
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
    controller.tui.chrome_mut().enter_shell_mode();

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
    assert!(!controller.chrome().shell_running());
    assert!(!transcript_has_status(
        &controller,
        "active_background_tasks: 1"
    ));
}

#[tokio::test]
async fn shell_mode_esc_cancels_running_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.type_text("!sleep 5");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start shell command");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let tasks = controller
                .local_config
                .as_ref()
                .expect("config")
                .background_tasks
                .list(true, 10)
                .await;
            if !tasks.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("shell task should register before esc");
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("esc cancels");

    assert!(!controller.chrome().shell_running());
    assert!(
        replay_session_messages(&controller)
            .await
            .iter()
            .any(|message| matches!(
                message,
                AgentMessage::ShellCommand {
                    outcome: neo_agent_core::ShellCommandOutcome::Cancelled,
                    ..
                }
            )),
        "cancelled shell command should persist as cancelled"
    );
}

#[tokio::test]
async fn shell_mode_drains_chat_followup_after_shell_queue() {
    let model_prompts = Arc::new(std::sync::Mutex::new(Vec::<Vec<Content>>::new()));
    let observed_prompts = Arc::clone(&model_prompts);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let observed_prompts = Arc::clone(&observed_prompts);
            async move {
                observed_prompts
                    .lock()
                    .expect("prompt lock")
                    .push(request.prompt);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release = Arc::new(std::sync::Mutex::new(Some(release_rx)));
    let observed_release = Arc::clone(&release);
    controller.set_shell_driver(Arc::new(move |_request| {
        let release = observed_release
            .lock()
            .expect("release lock")
            .take()
            .expect("release receiver");
        Box::pin(async move {
            let _ = release.await;
            Ok(completed_shell_result(""))
        })
    }));

    controller.type_text("!sleeping");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start shell command");
    controller.tui.chrome_mut().exit_shell_mode();
    controller.type_text("chat after shell");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("queue chat follow-up");

    assert!(controller.active_turn.is_none());
    release_tx.send(()).expect("release shell");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("shell completes and starts follow-up");

    controller
        .wait_for_active_turn()
        .await
        .expect("follow-up turn completes");
    assert_eq!(
        model_prompts.lock().expect("prompt lock").as_slice(),
        [vec![Content::text("chat after shell")]]
    );
}

#[tokio::test]
async fn shell_mode_queued_during_active_turn_runs_after_turn_finishes() {
    let release_turn = Arc::new(std::sync::Mutex::new(None));
    let observed_release_turn = Arc::clone(&release_turn);
    let run_turn: TurnDriver = Arc::new(move |_request, _channels| {
        let observed_release_turn = Arc::clone(&observed_release_turn);
        Box::pin(async move {
            let release = observed_release_turn
                .lock()
                .expect("turn release lock")
                .take()
                .expect("turn release receiver");
            let _ = release.await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    let (turn_tx, turn_rx) = tokio::sync::oneshot::channel::<()>();
    *release_turn.lock().expect("turn release lock") = Some(turn_rx);
    let shell_commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_shell_commands = Arc::clone(&shell_commands);
    controller.set_shell_driver(Arc::new(move |request| {
        let observed_shell_commands = Arc::clone(&observed_shell_commands);
        Box::pin(async move {
            observed_shell_commands
                .lock()
                .expect("shell commands lock")
                .push(request.command);
            Ok(completed_shell_result(""))
        })
    }));

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("start turn");
    assert!(controller.active_turn.is_some());
    controller.tui.chrome_mut().enter_shell_mode();
    controller.type_text("echo queued");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("queue shell command");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_shell_commands()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["echo queued"]
    );
    turn_tx.send(()).expect("release turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("queued shell completes");

    assert_eq!(
        shell_commands
            .lock()
            .expect("shell commands lock")
            .as_slice(),
        ["echo queued"]
    );
}
