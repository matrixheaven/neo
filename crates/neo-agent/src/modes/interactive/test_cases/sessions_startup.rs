//! Session startup/trust behavior (split from `sessions.rs`).

use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    fs,
    rc::Rc,
};

use neo_agent_core::AgentEvent;
use neo_tui::input::{InputEvent, KeybindingAction};

use super::super::*;
use super::*;

#[tokio::test]
async fn startup_trust_dialog_opens_when_unknown_and_trusts_workspace() {
    use std::collections::VecDeque;

    struct ScriptedEvents(VecDeque<InputEvent>);
    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .context("expected scripted trust dialog input")
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

    let trust_path = temp.path().join("trust.json");
    let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    config.project_trusted = false;
    controller.local_config = Some(config);
    controller.set_trust_store(store);

    let data = crate::trust::trust_dialog_data_from_inputs(
        crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
    );
    let mut events = ScriptedEvents(VecDeque::from([
        // Default is ContinueUntrusted; move up once to TrustCurrent.
        InputEvent::Action(KeybindingAction::SelectUp),
        InputEvent::Action(KeybindingAction::SelectConfirm),
    ]));
    controller
        .resolve_trust_dialog_at_startup(data, &mut events, |_| Ok(()))
        .await
        .expect("resolve trust dialog");

    assert!(controller.local_config.as_ref().unwrap().project_trusted);
    assert!(matches!(
        controller.local_config.as_ref().unwrap().project_trust,
        crate::trust::ProjectTrustState::Trusted { .. }
    ));
    assert!(controller.render_snapshot().contains("Workspace trusted"));
    assert_eq!(
        crate::trust::ProjectTrustStore::new(trust_path)
            .get(&project_dir)
            .expect("read trust"),
        Some(true)
    );
}

#[tokio::test]
async fn startup_trust_idle_poll_does_not_render_another_frame() {
    struct IdleThenConfirm {
        idle: bool,
    }

    impl TerminalEvents for IdleThenConfirm {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Action(KeybindingAction::SelectConfirm))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            if self.idle {
                self.idle = false;
                Ok(None)
            } else {
                Ok(Some(InputEvent::Action(KeybindingAction::SelectConfirm)))
            }
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    config.project_trusted = false;
    controller.local_config = Some(config);
    controller.set_trust_store(crate::trust::ProjectTrustStore::new(
        temp.path().join("trust.json"),
    ));
    let data = crate::trust::trust_dialog_data_from_inputs(
        crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
    );
    let mut render_count = 0;

    controller
        .resolve_trust_dialog_at_startup(data, IdleThenConfirm { idle: true }, |_| {
            render_count += 1;
            Ok(())
        })
        .await
        .expect("resolve trust dialog");

    assert_eq!(render_count, 2, "idle timeout must not render");
}

#[tokio::test]
async fn startup_mcp_keeps_composer_responsive_and_escape_interrupts() {
    struct ScriptedTerminalEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedTerminalEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0.pop_front().context("expected scripted input")
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self.0.pop_front())
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "slow".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Stdio,
        command: Some("neo-missing-mcp-server-for-test".to_owned()),
        url: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: Some(5_000),
        tool_timeout_ms: None,
    });
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config.clone());
    let saw_text = Rc::new(Cell::new(false));
    let saw_text_on_render = Rc::clone(&saw_text);
    let saw_hint = Rc::new(Cell::new(false));
    let saw_hint_on_render = Rc::clone(&saw_hint);

    tokio::time::timeout(
        Duration::from_secs(1),
        run_tty_lifecycle_with_event_factory(
            &mut controller,
            &config,
            &StartupAction::None,
            |_keybindings| {
                ScriptedTerminalEvents(VecDeque::from([
                    InputEvent::Insert('x'),
                    InputEvent::Backspace,
                    InputEvent::Cancel,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]))
            },
            move |tui, _| {
                saw_text_on_render
                    .set(saw_text_on_render.get() || tui.chrome().prompt().text == "x");
                saw_hint_on_render.set(
                    saw_hint_on_render.get()
                        || tui.chrome().working_label().as_deref()
                            == Some("MCP connecting · esc to interrupt"),
                );
                Ok(None)
            },
            || Ok(()),
            |_| Ok(()),
        ),
    )
    .await
    .expect("MCP startup must not block terminal input")
    .expect("terminal lifecycle succeeds");

    assert!(saw_text.get(), "composer input was never rendered");
    assert!(saw_hint.get(), "MCP interrupt hint was never rendered");
    let snapshot = controller
        .mcp_manager
        .as_ref()
        .expect("MCP manager exists")
        .snapshot("slow")
        .await
        .expect("slow MCP snapshot exists");
    assert_eq!(snapshot.status, McpServerStatus::Cancelled);
}

#[tokio::test]
async fn startup_trust_and_main_loop_share_one_terminal_event_source() {
    struct CountingTerminalEvents {
        events: VecDeque<InputEvent>,
        polls: Rc<Cell<usize>>,
    }

    impl TerminalEvents for CountingTerminalEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.polls.set(self.polls.get() + 1);
            self.events
                .pop_front()
                .context("expected scripted terminal input")
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    config.project_trusted = false;
    controller.local_config = Some(config.clone());
    controller.set_trust_store(crate::trust::ProjectTrustStore::new(
        temp.path().join("trust.json"),
    ));

    let factory_calls = Rc::new(Cell::new(0));
    let polls = Rc::new(Cell::new(0));
    let factory_calls_for_factory = Rc::clone(&factory_calls);
    let polls_for_factory = Rc::clone(&polls);
    run_tty_lifecycle_with_event_factory(
        &mut controller,
        &config,
        &StartupAction::None,
        move |_keybindings| {
            factory_calls_for_factory.set(factory_calls_for_factory.get() + 1);
            CountingTerminalEvents {
                events: VecDeque::from([
                    InputEvent::Action(KeybindingAction::SelectConfirm),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]),
                polls: Rc::clone(&polls_for_factory),
            }
        },
        |_, _| Ok(None),
        || Ok(()),
        |_| Ok(()),
    )
    .await
    .expect("run startup and main terminal lifecycle");

    assert_eq!(factory_calls.get(), 1);
    assert_eq!(polls.get(), 3);
}

#[tokio::test]
async fn startup_trust_dialog_opens_when_unknown_and_continues_untrusted() {
    use std::collections::VecDeque;

    struct ScriptedEvents(VecDeque<InputEvent>);
    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .context("expected scripted trust dialog input")
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

    let trust_path = temp.path().join("trust.json");
    let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    config.project_trusted = false;
    controller.local_config = Some(config);
    controller.set_trust_store(store);

    let data = crate::trust::trust_dialog_data_from_inputs(
        crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
    );
    let mut events = ScriptedEvents(VecDeque::from([InputEvent::Action(
        KeybindingAction::SelectConfirm,
    )]));
    controller
        .resolve_trust_dialog_at_startup(data, &mut events, |_| Ok(()))
        .await
        .expect("resolve trust dialog");

    assert!(!controller.local_config.as_ref().unwrap().project_trusted);
    assert!(matches!(
        controller.local_config.as_ref().unwrap().project_trust,
        crate::trust::ProjectTrustState::Untrusted { .. }
    ));
    assert!(controller.render_snapshot().contains("Workspace untrusted"));
    assert_eq!(
        crate::trust::ProjectTrustStore::new(trust_path)
            .get(&project_dir)
            .expect("read trust"),
        Some(false)
    );
}

#[test]
fn startup_trust_dialog_data_is_some_for_unknown_and_none_otherwise() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");

    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.project_trust = crate::trust::ProjectTrustState::NotRequired;
    assert!(trust_dialog_data_for_startup(&config).is_none());

    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    let data = trust_dialog_data_for_startup(&config);
    assert!(data.is_some());
    assert_eq!(
        data.unwrap().current_dir,
        project_dir.canonicalize().expect("canonicalize")
    );

    config.project_trust = crate::trust::ProjectTrustState::Trusted {
        target: project_dir.clone(),
    };
    assert!(trust_dialog_data_for_startup(&config).is_none());

    config.project_trust = crate::trust::ProjectTrustState::Untrusted {
        target: project_dir.clone(),
    };
    assert!(trust_dialog_data_for_startup(&config).is_none());
}

#[tokio::test]
async fn startup_trust_dialog_cancels_to_untrusted() {
    use std::collections::VecDeque;

    struct ScriptedEvents(VecDeque<InputEvent>);
    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .context("expected scripted trust dialog input")
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    fs::write(project_dir.join("AGENTS.md"), "rules").expect("write agents");

    let trust_path = temp.path().join("trust.json");
    let store = crate::trust::ProjectTrustStore::new(trust_path.clone());

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    let inputs = crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs");
    config.project_trust = crate::trust::ProjectTrustState::Unknown { inputs };
    config.project_trusted = false;
    controller.local_config = Some(config);
    controller.set_trust_store(store);

    let data = crate::trust::trust_dialog_data_from_inputs(
        crate::trust::collect_project_trust_inputs(&project_dir).expect("collect inputs"),
    );
    let mut events = ScriptedEvents(VecDeque::from([InputEvent::Action(
        KeybindingAction::SelectCancel,
    )]));
    controller
        .resolve_trust_dialog_at_startup(data, &mut events, |_| Ok(()))
        .await
        .expect("resolve trust dialog");

    assert!(!controller.local_config.as_ref().unwrap().project_trusted);
    assert!(matches!(
        controller.local_config.as_ref().unwrap().project_trust,
        crate::trust::ProjectTrustState::Untrusted { .. }
    ));
    assert!(controller.render_snapshot().contains("Workspace untrusted"));
    assert_eq!(
        crate::trust::ProjectTrustStore::new(trust_path)
            .get(&project_dir)
            .expect("read trust"),
        Some(false)
    );
}
