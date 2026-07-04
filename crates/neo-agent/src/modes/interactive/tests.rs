use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use neo_agent_core::{
    AgentEvent, AgentMessage, Content, PermissionMode, StopReason, ToolResult,
    skills::{LoadedSkill, SkillManifest, SkillSource, SkillStore, SkillType},
};
use neo_tui::{
    input::KeybindingAction,
    shell::{ApprovalChoice, ChromeMode, CommandPaletteState, CommandSpec, Overlay, OverlayKind},
    transcript::TranscriptEntry,
};

use super::git_status::{
    count_untracked_changes, git_status_label_with_program, parse_git_numstat,
    parse_git_status_porcelain, parse_git_untracked_files_z,
};
use super::snapshot::{compose_tui_frame, render_overlay_snapshot};
use super::*;
use crate::config::{Defaults, McpConfig, ModelConfig, RuntimeConfig, TuiConfig};

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000601";
const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000602";
const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000603";
const SESSION_NEW: &str = "session_00000000-0000-4000-8000-000000000604";

#[tokio::test]
async fn interactive_session_path_uses_main_agent_wire() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let path = super::controller_factory::create_interactive_session_path(&config)
        .await
        .expect("session path");

    assert!(path.ends_with(Path::new("agents").join("main").join("wire.jsonl")));
    let session_root = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("session root");
    assert!(session_root.join("state.json").is_file());

    let state = neo_agent_core::session::SessionStateStore::new(session_root)
        .read()
        .await
        .expect("read session state");
    let main = state.agents.get("main").expect("main agent record");
    assert_eq!(
        main.record_dir,
        neo_agent_core::session::relative_agent_record_dir("main")
    );
    assert_eq!(main.kind, neo_agent_core::session::SessionAgentKind::Main);
    assert_eq!(main.parent_agent_id, None);
    assert_eq!(main.role, None);
    assert_eq!(main.swarm_id, None);
    assert_eq!(main.swarm_item, None);
}

fn test_workspace_root() -> PathBuf {
    let dir = std::env::temp_dir().join("neo-test-workspace");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn main_wire_path_for_session(session_dir: impl AsRef<Path>) -> PathBuf {
    let path = neo_agent_core::session::main_agent_wire_path(session_dir.as_ref());
    fs::create_dir_all(path.parent().expect("wire parent")).expect("create wire dir");
    path
}

fn write_main_wire(bucket_dir: &Path, session_id: &str, content: &str) {
    let path = main_wire_path_for_session(bucket_dir.join(session_id));
    fs::write(path, content).expect("write main wire");
}

fn skill_store_with_refactor_skill() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![LoadedSkill {
            name: "refactor".to_owned(),
            root: PathBuf::from("builtin/refactor"),
            manifest: SkillManifest {
                name: "refactor".to_owned(),
                description: "Refactor with project conventions".to_owned(),
                skill_type: SkillType::Prompt,
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
                slash_commands: Vec::new(),
            },
            body: "Refactor safely.".to_owned(),
            source: SkillSource::Builtin,
        }],
    )
    .expect("skill store")
}

fn skill_store_with_two_prompt_skills() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![
            LoadedSkill {
                name: "skill_one".to_owned(),
                root: PathBuf::from("builtin/skill_one"),
                manifest: SkillManifest {
                    name: "skill_one".to_owned(),
                    description: "First skill".to_owned(),
                    skill_type: SkillType::Prompt,
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                    slash_commands: Vec::new(),
                },
                body: "ONE: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
            },
            LoadedSkill {
                name: "skill_two".to_owned(),
                root: PathBuf::from("builtin/skill_two"),
                manifest: SkillManifest {
                    name: "skill_two".to_owned(),
                    description: "Second skill".to_owned(),
                    skill_type: SkillType::Prompt,
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                    slash_commands: Vec::new(),
                },
                body: "TWO: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
            },
        ],
    )
    .expect("skill store")
}

fn pending_approval_response(
    decision_tx: oneshot::Sender<PermissionApprovalDecision>,
) -> PendingApprovalResponse {
    PendingApprovalResponse {
        decision_tx,
        feedback_tx: None,
        selected_label_tx: None,
        session_option_label: None,
        prefix_option_label: None,
    }
}

#[test]
fn drain_log_events_pushes_warn_as_status() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::log_capture::CapturedEvent>();
    controller.set_log_event_receiver(rx);
    tx.send(crate::log_capture::CapturedEvent {
        level: "WARN".to_owned(),
        message: "MCP server unavailable server_id=linear".to_owned(),
    })
    .expect("send event");
    controller.drain_log_events();
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("MCP server unavailable"),
        "transcript should show captured WARN, got:\n{snapshot}"
    );
}

#[test]
fn drain_log_events_pushes_error_as_status() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<crate::log_capture::CapturedEvent>();
    controller.set_log_event_receiver(rx);
    tx.send(crate::log_capture::CapturedEvent {
        level: "ERROR".to_owned(),
        message: "critical failure".to_owned(),
    })
    .expect("send event");
    controller.drain_log_events();
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("critical failure"),
        "transcript should show captured ERROR, got:\n{snapshot}"
    );
}

#[test]
fn drain_log_events_does_not_crash_without_receiver() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    // No set_log_event_receiver — should be a no-op, not a panic.
    controller.drain_log_events();
}

#[test]
fn git_status_badge_formats_branch_diff_and_sync() {
    let mut badge =
        parse_git_status_porcelain("## main...origin/main [ahead 2, behind 1]\n M src/app.rs\n")
            .expect("git badge");
    let (added, deleted) = parse_git_numstat("12\t3\tsrc/app.rs\n-\t-\tassets/image.png\n");
    badge.added = added;
    badge.deleted = deleted;

    assert_eq!(badge.format(), "main [+12 -3 ↑2↓1]");
}

#[test]
fn git_status_badge_formats_dirty_without_line_counts() {
    let badge = parse_git_status_porcelain("## feature\n?? new-file.rs\n").expect("git badge");

    assert_eq!(badge.format(), "feature [±]");
}

#[test]
fn git_status_badge_formats_unborn_branch_as_init() {
    let badge = parse_git_status_porcelain("## No commits yet on main\n?? new-file.rs\n")
        .expect("git badge");

    assert_eq!(badge.format(), "main [init]");
}

#[test]
fn git_status_badge_counts_untracked_text_file_lines() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(dir.path().join("src")).expect("create source dir");
    fs::write(dir.path().join("src/new.rs"), "first\nsecond\n").expect("write source file");

    let mut badge = parse_git_status_porcelain("## feature\n?? src/new.rs\n").expect("git badge");
    let untracked_files = parse_git_untracked_files_z(b"src/new.rs\0");
    let (added, untracked) = count_untracked_changes(dir.path(), &untracked_files);
    badge.added = added;
    badge.untracked = untracked;

    assert_eq!(badge.format(), "feature [+2 -0]");
}

#[test]
fn git_status_badge_counts_untracked_files_without_line_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("image.bin"), b"neo\0image").expect("write binary file");

    let mut badge = parse_git_status_porcelain("## feature\n?? image.bin\n").expect("git badge");
    let untracked_files = parse_git_untracked_files_z(b"image.bin\0");
    let (added, untracked) = count_untracked_changes(dir.path(), &untracked_files);
    badge.added = added;
    badge.untracked = untracked;

    assert_eq!(badge.format(), "feature [?1]");
}

#[test]
fn git_status_badge_is_absent_when_git_program_is_missing() {
    let missing = git_status_label_with_program(
        "definitely-not-a-real-git-binary-for-neo-tests",
        &test_workspace_root(),
    );

    assert_eq!(missing, None);
}

#[test]
fn git_status_badge_is_absent_when_workspace_has_no_git_dir() {
    let parent = tempfile::tempdir().expect("tempdir");
    let workspace = parent.path().join("nested-workspace");
    fs::create_dir(&workspace).expect("create workspace");
    fs::write(workspace.join("untracked.txt"), "new file\n").expect("write untracked file");

    let init_status = Command::new("git")
        .arg("-C")
        .arg(parent.path())
        .args(["init", "--initial-branch=main"])
        .status()
        .expect("run git init");
    assert!(init_status.success(), "git init should succeed");

    assert_eq!(git_status_label_with_program("git", &workspace), None);
}

#[test]
fn refresh_git_status_now_updates_after_write_tool_finished() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(|_| Some("main [+2 -1]".into())));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main [+1 -1]".into()));

    controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Write".to_owned(),
        result: ToolResult::ok("wrote file"),
    });

    assert_eq!(controller.chrome().git_status_label(), Some("main [+2 -1]"));
}

#[test]
fn refresh_git_status_now_updates_after_edit_tool_finished() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(|_| Some("main [+3 -2]".into())));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main [+1 -1]".into()));

    controller.apply_turn_event(AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Edit".to_owned(),
        result: ToolResult::ok("edited file"),
    });

    assert_eq!(controller.chrome().git_status_label(), Some("main [+3 -2]"));
}

#[test]
fn refresh_git_status_now_updates_after_shell_and_terminal_finished() {
    let statuses = Arc::new(std::sync::Mutex::new(VecDeque::from([
        Some("main [↑1]".into()),
        Some("main".into()),
    ])));
    let provider_statuses = Arc::clone(&statuses);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(move |_| {
        provider_statuses
            .lock()
            .expect("status queue lock")
            .pop_front()
            .flatten()
    }));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main [+1 -1]".into()));

    controller.apply_turn_event(AgentEvent::ShellCommandFinished {
        turn: 1,
        id: "shell-1".to_owned(),
        exit_code: Some(0),
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
        truncated: false,
        origin: neo_agent_core::ShellCommandOrigin::ModelBashTool,
        outcome: neo_agent_core::ShellCommandOutcome::Completed,
    });
    assert_eq!(controller.chrome().git_status_label(), Some("main [↑1]"));

    controller.apply_turn_event(AgentEvent::TerminalSessionFinished {
        turn: 1,
        id: "terminal-1".to_owned(),
        handle: "terminal".to_owned(),
        status: "exited".to_owned(),
        exit_code: Some(0),
    });
    assert_eq!(controller.chrome().git_status_label(), Some("main"));
}

#[test]
fn refresh_git_status_if_due_uses_30s_interval() {
    let refresh_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let provider_refresh_count = Arc::clone(&refresh_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(move |_| {
        let count = provider_refresh_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        Some(format!("main [refresh-{count}]"))
    }));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main".into()));

    controller.set_last_git_status_refresh(Some(
        Instant::now()
            .checked_sub(Duration::from_secs(29))
            .expect("instant before now"),
    ));
    controller.refresh_git_status_if_due();
    assert_eq!(refresh_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(controller.chrome().git_status_label(), Some("main"));

    controller.set_last_git_status_refresh(Some(
        Instant::now()
            .checked_sub(Duration::from_secs(30))
            .expect("instant before now"),
    ));
    controller.refresh_git_status_if_due();
    assert_eq!(refresh_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        controller.chrome().git_status_label(),
        Some("main [refresh-1]")
    );
}

#[test]
fn refresh_git_status_now_clears_badge_when_git_unavailable() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(|_| None));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main [+1 -1]".into()));

    controller.refresh_git_status_now();

    assert_eq!(controller.chrome().git_status_label(), None);
}

fn test_session_summary(
    id: impl Into<String>,
    title: impl Into<String>,
    work_dir: impl Into<PathBuf>,
    last_prompt: impl Into<String>,
) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: Some(title.into()),
        last_prompt: Some(last_prompt.into()),
        work_dir: work_dir.into(),
        updated_at: String::new(),
        metadata: None,
    }
}

fn transcript_entries(controller: &InteractiveController) -> &[TranscriptEntry] {
    controller.transcript().transcript().entries()
}

fn transcript_has_status(controller: &InteractiveController, expected: &str) -> bool {
    transcript_entries(controller).iter().any(
        |entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.contains(expected)),
    )
}

fn transcript_scrollback(controller: &InteractiveController) -> usize {
    controller.transcript().transcript().viewport().scrollback()
}

/// Replay the active session's JSONL to recover `AgentMessage` values for
/// assertions.  Used in place of the removed `session_messages` field.
async fn replay_session_messages(controller: &InteractiveController) -> Vec<AgentMessage> {
    let config = controller.local_config.as_ref().expect("config");
    let session_id = controller.active_session_id.as_ref().expect("session id");
    let path = crate::modes::sessions::session_path(session_id, config).expect("session path");
    neo_agent_core::session::JsonlSessionReader::replay_context(&path)
        .await
        .map(|ctx| ctx.messages().to_vec())
        .unwrap_or_default()
}

fn render_tui_snapshot(tui: &neo_tui::NeoTui) -> String {
    let mut transcript = tui.transcript().clone();
    render_transcript_snapshot(tui.chrome(), &mut transcript, 80, 24)
}

#[test]
fn exit_message_prints_resume_command_when_session_exists() {
    assert_eq!(exit_message(None), "Bye\n");
    assert_eq!(
        exit_message(Some("session_550e8400-e29b-41d4-a716-446655440000")),
        "Bye\nneo resume session_550e8400-e29b-41d4-a716-446655440000\n"
    );
}

#[test]
fn transcript_pane_exposes_live_rows_for_neo_tui_draw() {
    let app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 0,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });

    let lines = compose_tui_frame(&app, &mut transcript, 80, 12).expect("non-zero terminal size");

    let plain: Vec<String> = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect();
    assert!(plain.iter().any(|line| line.contains("Using Bash")));
    assert_eq!(compose_tui_frame(&app, &mut transcript, 0, 12), None);
}

#[tokio::test]
async fn resolving_question_records_collected_answers_in_transcript() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "session",
        "model",
        test_workspace_root(),
        |_| async { Ok(Vec::new()) },
    );
    let (response_tx, mut response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick a side?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "Left".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Right".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
    });

    controller
        .resolve_question("question-1", vec!["Left".to_owned()])
        .await
        .expect("question resolves");

    assert_eq!(
        response_rx
            .try_recv()
            .expect("response should be sent")
            .answers,
        vec!["Left"]
    );
    assert!(transcript_has_status(&controller, "Collected your answers"));
    assert!(transcript_has_status(&controller, "Pick a side?"));
    assert!(transcript_has_status(&controller, "Left"));
}

#[tokio::test]
async fn background_question_answer_starts_followup_turn() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "session",
        "model",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("requests").push(request);
                Ok(Vec::new())
            }
        },
    );
    let (response_tx, mut response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick a side?".to_owned(),
            header: Some("Choice".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "Left".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Right".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
    });

    controller
        .resolve_question("question-1", vec!["Left".to_owned()])
        .await
        .expect("question resolves");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup completes");

    assert_eq!(
        response_rx
            .try_recv()
            .expect("response should be sent")
            .answers,
        vec!["Left"]
    );
    let requests = requests.lock().expect("requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id.as_deref(), None);
    assert!(
        requests[0].prompt[0]
            .as_text()
            .unwrap()
            .contains("Background question `question-1`")
    );
    assert!(
        requests[0].prompt[0]
            .as_text()
            .unwrap()
            .contains("TaskOutput")
    );
}

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
    });

    assert!(!controller.chrome().question_dialog_is_focused());
    assert!(!controller.pending_questions.contains_key("question-1"));
    assert!(
        !controller
            .pending_question_prompts
            .contains_key("question-1")
    );
}

#[test]
fn neo_tui_draw_composes_body_then_chrome_in_one_frame() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    app.prompt_mut().text = "next".to_owned();
    app.prompt_mut().cursor = 4;
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_banner("Welcome to neo");
    transcript.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 0,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "cargo test" }),
    });

    let lines = compose_tui_frame(&app, &mut transcript, 80, 12)
        .expect("transcript frame composes body + chrome");

    let joined = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    // Banner (finalized) appears in the body before the running tool card,
    // which appears before the prompt chrome.
    let welcome = joined.find("Welcome to neo").expect("welcome in body");
    let tool = joined.find("Using Bash").expect("running tool in body");
    let prompt = joined.find("> next").expect("prompt chrome at tail");
    assert!(welcome < tool, "banner should precede the tool card");
    assert!(tool < prompt, "tool card should precede the prompt chrome");
    // The running tool card is live (● Using), not finalized (● Used).
    assert!(!joined.contains("Used Bash"));
}

#[test]
fn neo_tui_draw_replays_finished_tool_before_prompt_chrome() {
    let mut app = NeoChromeState::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
    );
    app.prompt_mut().text = "next".to_owned();
    app.prompt_mut().cursor = 4;
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        Vec::new(),
        [
            AgentMessage::user_text("inspect"),
            AgentMessage::assistant(
                [Content::text("reading")],
                [neo_agent_core::AgentToolCall {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"README.md"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tool-1", "Read", [Content::text("README contents")], false),
        ],
    );
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_banner("Welcome to neo");
    replay_session_into_transcript(&mut transcript, &loaded);

    let lines =
        compose_tui_frame(&app, &mut transcript, 80, 12).expect("transcript frame composes replay");

    // Tool header spans are individually ANSI-colored, so strip codes
    // before substring searching for the committed tool card.
    let plain: Vec<String> = lines
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect();
    let joined = plain.join("\n");
    let welcome = joined.find("Welcome to neo").expect("welcome in body");
    let prompt = joined.find("> next").expect("prompt chrome live row");
    let tool = joined
        .find("Used Read (README.md)")
        .expect("tool committed");
    assert!(welcome < tool);
    assert!(tool < prompt);
    assert!(!joined.contains("Using Read"));
}

#[tokio::test]
async fn controller_snapshot_uses_transcript_tool_card_rendering() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move {
            Ok(vec![
                AgentEvent::ToolExecutionStarted {
                    turn: 1,
                    id: "tool-1".to_owned(),
                    name: "Read".to_owned(),
                    arguments: serde_json::json!({ "path": "README.md" }),
                },
                AgentEvent::ToolExecutionFinished {
                    turn: 1,
                    id: "tool-1".to_owned(),
                    name: "Read".to_owned(),
                    result: ToolResult::ok("line one\nline two"),
                },
            ])
        },
    );

    controller.type_text("inspect");
    let snapshot = controller.submit_prompt().await.expect("prompt succeeds");

    assert!(
        snapshot.contains("● Used Read (README.md)"),
        "transcript snapshot should include finalized tool card, got:\n{snapshot}"
    );
    assert!(snapshot.contains("> "));
}

#[tokio::test]
async fn controller_submits_prompt_reduces_turn_events_and_renders_snapshot() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("hello neo")]);
            assert_eq!(request.session_id, None);
            assert_eq!(request.model, None);
            Ok(vec![
                AgentEvent::MessageStarted {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                },
                AgentEvent::TextDelta {
                    turn: 1,
                    text: "Hello".to_owned(),
                },
                AgentEvent::TextDelta {
                    turn: 1,
                    text: ", Neo".to_owned(),
                },
                AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                },
            ])
        },
    );

    controller.type_text("hello neo");
    let snapshot = controller.submit_prompt().await.expect("turn succeeds");

    assert!(snapshot.contains("Welcome to neo"));
    assert!(snapshot.contains("test-session"));
    assert!(snapshot.contains("openai/gpt-4.1"));
    // The user prompt and assistant reply appear in the rendered frame.
    assert!(snapshot.contains("hello neo"));
    assert!(snapshot.contains("Hello, Neo"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn event_loop_types_submits_renders_and_exits_without_a_real_terminal() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("hi")]);
            assert_eq!(request.session_id, None);
            assert_eq!(request.model, None);
            Ok(vec![
                AgentEvent::MessageStarted {
                    turn: 1,
                    id: "assistant-1".to_owned(),
                },
                AgentEvent::TextDelta {
                    turn: 1,
                    text: "hello from controller".to_owned(),
                },
                AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                },
            ])
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui| {
                rendered.push(render_tui_snapshot(tui));
                Ok(())
            },
            || Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Insert('h'),
                    InputEvent::Insert('i'),
                    InputEvent::Submit,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(rendered.iter().any(|snapshot| snapshot.contains("> hi")));
    assert!(
        rendered
            .last()
            .expect("final render")
            .contains("hello from controller")
    );
}

#[tokio::test]
async fn event_loop_reports_turn_error_and_keeps_running() {
    use std::collections::VecDeque;

    struct ScriptedEvents {
        events: VecDeque<InputEvent>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { anyhow::bail!("provider stream error: http status 400") },
    );
    let mut prompt_snapshots = Vec::new();

    controller.type_text("trigger error");
    controller
        .run_terminal_loop(
            |app| {
                prompt_snapshots.push(app.prompt().text.clone());
                Ok(())
            },
            ScriptedEvents {
                events: VecDeque::from([
                    InputEvent::Submit,
                    InputEvent::Insert('o'),
                    InputEvent::Insert('k'),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]),
            },
        )
        .await
        .expect("turn error should not exit the interactive loop");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Error: provider stream error: http status 400"));
    assert!(prompt_snapshots.iter().any(|prompt| prompt == "ok"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn event_loop_inserts_paste_newlines_without_submitting_until_enter() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt, vec![Content::text("alpha\nbeta")]);
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui| {
                rendered.push(render_tui_snapshot(tui));
                Ok(())
            },
            || Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Paste("alpha\nbeta".to_owned()),
                    InputEvent::Submit,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert!(rendered.iter().any(|snapshot| snapshot.contains("alpha")));
    assert!(rendered.iter().any(|snapshot| snapshot.contains("beta")));
}

#[tokio::test]
async fn event_loop_renders_after_terminal_resize_without_submitting_prompt() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut rendered = Vec::new();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move {
            panic!("resize should not submit a turn");
            #[allow(unreachable_code)]
            Ok(Vec::<AgentEvent>::new())
        },
    );

    controller
        .run_terminal_loop_with_suspend(
            |tui| {
                rendered.push(render_tui_snapshot(tui));
                Ok(())
            },
            || Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Insert('h'),
                    InputEvent::Resize {
                        columns: 100,
                        rows: 30,
                    },
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(rendered.len(), 4);
    assert!(rendered[1].contains("> h"));
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
}

#[tokio::test]
async fn event_loop_dispatches_editor_keybinding_actions_to_prompt_edits() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );
    controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

    for character in "hello brave world".chars() {
        controller
            .handle_input_event(InputEvent::Insert(character))
            .await
            .expect("insert succeeds");
    }

    let mut last_prompt_text = String::new();
    let mut last_prompt_cursor = 0usize;

    controller
        .run_terminal_loop(
            |app| {
                let prompt = app.prompt();
                if !prompt.text.is_empty() {
                    last_prompt_text = prompt.text.clone();
                    last_prompt_cursor = prompt.cursor;
                }
                Ok(())
            },
            FakeEvents {
                events: vec![
                    InputEvent::Action(KeybindingAction::InputCopy),
                    InputEvent::Action(KeybindingAction::EditorCursorWordLeft),
                    InputEvent::Action(KeybindingAction::EditorDeleteWordBackward),
                    InputEvent::Action(KeybindingAction::EditorDeleteToLineEnd),
                    InputEvent::Action(KeybindingAction::EditorYank),
                    InputEvent::Action(KeybindingAction::EditorUndo),
                    InputEvent::Action(KeybindingAction::EditorUndo),
                    InputEvent::Action(KeybindingAction::InputTab),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop succeeds");

    assert_eq!(controller.chrome().copy_buffer(), Some("hello brave world"));
    assert_eq!(last_prompt_text, "hello \tworld");
    assert_eq!(last_prompt_cursor, 7);
}

#[tokio::test]
async fn event_loop_default_ctrl_c_clears_prompt_instead_of_copying() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

    controller.type_text("copy through keybinding");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("clear keybinding handled");

    assert_eq!(controller.chrome().copy_buffer(), None);
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn event_loop_copy_action_writes_prompt_to_injected_clipboard() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );
    controller.set_clipboard_writer(Arc::new(move |text| {
        recorded
            .lock()
            .expect("record clipboard text")
            .push(text.to_owned());
        Ok(())
    }));

    controller.type_text("copy to system clipboard");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("copy action succeeds");

    assert_eq!(
        copied.lock().expect("clipboard writes").as_slice(),
        ["copy to system clipboard"]
    );
    assert_eq!(
        controller.chrome().copy_buffer(),
        Some("copy to system clipboard")
    );
}

#[tokio::test]
async fn event_loop_ctrl_c_prefers_selected_transcript_region() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(move |text| {
        recorded
            .lock()
            .expect("record clipboard text")
            .push(text.to_owned());
        Ok(())
    }));
    controller
        .transcript_mut()
        .push_user_message("selected user prompt");
    controller
        .transcript_mut()
        .push_assistant_message("selected assistant reply");
    controller.type_text("prompt text stays out of clipboard");

    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionStart,
        ))
        .await
        .expect("selection starts");
    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::TranscriptSelectionExtendUp,
        ))
        .await
        .expect("selection extends");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("copy action succeeds");

    assert_eq!(
        copied.lock().expect("clipboard writes").as_slice(),
        ["You\nselected user prompt\n\nAssistant\nselected assistant reply"]
    );
    assert_eq!(controller.chrome().copy_buffer(), None);
    assert_eq!(
        controller.chrome().prompt().text,
        "prompt text stays out of clipboard"
    );
}

#[tokio::test]
async fn event_loop_clipboard_failure_keeps_internal_copy_buffer() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_clipboard_writer(Arc::new(|_text| {
        Err(anyhow::anyhow!("clipboard unavailable"))
    }));

    controller.type_text("copy fallback");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputCopy))
        .await
        .expect("clipboard failure is non-fatal");

    assert_eq!(controller.chrome().copy_buffer(), Some("copy fallback"));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Status { text, .. }
                if text.contains("Clipboard copy failed")
                    && text.contains("clipboard unavailable")
        )
    }));
}

#[tokio::test]
async fn event_loop_ctrl_c_cancels_overlay_without_copying_prompt() {
    let copied = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&copied);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "session",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );
    controller.set_clipboard_writer(Arc::new(move |text| {
        recorded
            .lock()
            .expect("record clipboard text")
            .push(text.to_owned());
        Ok(())
    }));

    controller.type_text("do not copy while overlay is focused");
    controller.open_session_picker();
    assert!(controller.chrome().focused_overlay().is_some());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("overlay cancel succeeds");

    assert!(controller.chrome().focused_overlay().is_none());
    assert_eq!(controller.chrome().copy_buffer(), None);
    assert!(copied.lock().expect("clipboard writes").is_empty());
}

#[tokio::test]
async fn event_loop_ctrl_c_clears_prompt_before_confirming_exit() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("draft prompt");
    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c handles prompt clear");

    assert!(!should_exit);
    assert_eq!(controller.chrome().prompt().text, "");
    assert_eq!(
        controller.chrome().exit_confirmation_label(),
        Some("Press Ctrl-C again to exit")
    );
    assert!(!transcript_has_status(
        &controller,
        "Press Ctrl-C again to exit"
    ));
}

#[tokio::test]
async fn event_loop_ctrl_c_requires_second_press_to_exit_when_prompt_is_empty() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    let first = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("first ctrl-c prompts");
    let second = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("second ctrl-c exits");

    assert!(!first);
    assert!(second);
}

#[tokio::test]
async fn event_loop_ctrl_c_key_cancels_active_turn_instead_of_confirming_exit() {
    let captured_token = Arc::new(std::sync::Mutex::new(None));
    let observed_token = Arc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = Arc::clone(&observed_token);
        *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("cancel me");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("prompt submits");

    assert!(controller.active_turn.is_some());

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c cancels active turn");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(!should_exit);
    assert!(token.is_cancelled());
    assert_eq!(controller.chrome().exit_confirmation_label(), None);
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn event_loop_ctrl_c_clears_stale_working_state_before_exit_confirmation() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .tui
        .chrome_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "ask".to_owned(),
            name: "AskUserQuestion".to_owned(),
            arguments: serde_json::json!({ "questions": [] }),
        });
    assert!(controller.chrome().working_label().is_some());

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c clears stale working state");

    assert!(!should_exit);
    assert!(controller.chrome().working_label().is_none());
    assert_eq!(controller.chrome().exit_confirmation_label(), None);

    controller
        .handle_input_event(InputEvent::Insert('o'))
        .await
        .expect("typing after stale interrupt succeeds");
    controller
        .handle_input_event(InputEvent::Insert('k'))
        .await
        .expect("typing after stale interrupt succeeds");
    assert_eq!(controller.chrome().prompt().text, "ok");
}

#[tokio::test]
async fn event_loop_ctrl_d_deletes_forward_until_prompt_is_empty_then_confirms_exit() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("ab");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLineStart))
        .await
        .expect("move cursor to start");
    let delete = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("ctrl-d deletes while prompt has text");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("ctrl-d deletes final char");
    let first_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("first empty ctrl-d prompts");
    let second_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("second empty ctrl-d exits");

    assert!(!delete);
    assert_eq!(controller.chrome().prompt().text, "");
    assert!(!first_exit);
    assert!(second_exit);
    assert_eq!(controller.chrome().exit_confirmation_label(), None);
    assert!(!transcript_has_status(
        &controller,
        "Press Ctrl-D again to exit"
    ));
}

#[tokio::test]
async fn event_loop_ctrl_z_reports_suspend_request() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    let should_exit = controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+z").expect("valid key")))
        .await
        .expect("ctrl-z is handled");

    assert!(!should_exit);
    assert!(controller.take_suspend_requested());
}

#[tokio::test]
async fn event_loop_tabs_through_real_filesystem_prompt_completions() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("src")).expect("create src");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");
    fs::write(temp.path().join("src/matrix.rs"), "pub fn matrix() {}\n").expect("write matrix");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("open src/ma");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens completion picker");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("completion confirms");

    assert_eq!(controller.chrome().prompt().text, "open src/main.rs");
    assert_eq!(controller.chrome().prompt().cursor, 16);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_opens_slash_completion_after_typing_slash() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");

    assert_eq!(controller.chrome().prompt().text, "/");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));
    assert!(
        controller.chrome().selected_prompt_completion().is_some(),
        "slash completion should select the first local command"
    );
}

#[tokio::test]
async fn event_loop_opens_slash_completion_after_whitespace() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("foo ");
    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("inline slash insert opens completion");

    assert_eq!(controller.chrome().prompt().text, "foo /");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));
}

#[tokio::test]
async fn slash_completion_includes_btw_command() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");

    let rendered = controller.chrome().focused_overlay_lines(80).join("\n");
    assert!(
        rendered.contains("/btw"),
        "slash completion should include /btw; got:\n{rendered}"
    );
}

#[tokio::test]
async fn event_loop_backspace_deletes_slash_while_completion_is_open() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits prompt");

    assert_eq!(controller.chrome().prompt().text, "");
    assert_eq!(controller.chrome().prompt().cursor, 0);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_escape_closes_slash_completion_without_exiting() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("slash insert opens completion");
    let should_exit = controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape closes completion");

    assert!(!should_exit);
    assert_eq!(controller.chrome().prompt().text, "/");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_escape_cancels_active_turn() {
    use std::{collections::VecDeque, sync::Arc as StdArc};

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let captured_token = StdArc::new(std::sync::Mutex::new(None));
    let observed_token = StdArc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = StdArc::clone(&observed_token);
        Box::pin(async move {
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("cancel me");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    // ESC should cancel the active turn
                    Some(InputEvent::Cancel),
                    // After cancellation the app is idle; two Interrupts to exit
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("escape cancels turn and loop exits");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn event_loop_escape_is_noop_when_idle() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("hello");

    // ESC when idle (no overlay, no active turn) should be a no-op
    let should_exit = controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape is no-op when idle");

    assert!(!should_exit, "ESC should not exit the app when idle");
    // Prompt text should be preserved (ESC is not clearing it)
    assert_eq!(controller.chrome().prompt().text, "hello");
}

#[tokio::test]
async fn controller_for_config_applies_tui_keybinding_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions");
    let mut config = test_config(temp.path(), sessions_dir);
    config
        .tui
        .keybindings
        .insert("tui.command.open".to_owned(), vec!["ctrl+g".to_owned()]);
    let mut controller = controller_for_config(&config);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+g").expect("valid key")))
        .await
        .expect("configured keybinding runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::CommandPalette(_))
    ));
}

#[test]
fn auto_image_protocol_uses_positive_runtime_hints_on_local_terminals() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
        "KITTY_WINDOW_ID" => Ok("1".to_owned()),
        "WEZTERM_PANE" => Ok("2".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
    assert!(!capabilities.sixel());
}

#[test]
fn auto_image_protocol_falls_back_inside_tmux_screen_or_ssh() {
    let tmux_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "KITTY_WINDOW_ID" | "TMUX" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };
    let ssh_env = |name: &str| match name {
        "TERM_PROGRAM" => Ok("iTerm.app".to_owned()),
        "SSH_CONNECTION" => Ok("127.0.0.1 1 127.0.0.1 2".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    assert_eq!(
        terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, tmux_env),
        TerminalImageCapabilities::default()
    );
    assert_eq!(
        terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, ssh_env),
        TerminalImageCapabilities::default()
    );
}

#[test]
fn explicit_image_protocol_uses_matching_static_terminal_hints() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "TERM_PROGRAM" => Ok("WezTerm".to_owned()),
        "KITTY_WINDOW_ID" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Kitty, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
    assert!(!capabilities.sixel());
}

#[tokio::test]
async fn event_loop_tabs_through_local_slash_prompt_template_completions() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review the current change\n---\nReview this change.\n",
    )
    .expect("write review prompt");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("/rev");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes slash prompt");

    assert_eq!(controller.chrome().prompt().text, "/review");
    assert_eq!(controller.chrome().prompt().cursor, 7);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn tab_confirms_selected_prompt_completion() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("/");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens completion picker");

    assert!(controller.chrome().focused_overlay().is_some());
    assert!(controller.chrome().selected_prompt_completion().is_some());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab confirms selected completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(!controller.chrome().prompt().text.is_empty());
}

#[tokio::test]
async fn slash_picker_commands_do_not_enter_streaming_mode() {
    for command in ["/model", "/provider"] {
        let mut controller = InteractiveController::new_for_test(
            "neo",
            "test-session",
            "openai/gpt-4.1",
            test_workspace_root(),
            |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        );
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .unwrap_or_else(|e| panic!("{command} submit failed: {e}"));
        assert_eq!(
            controller.chrome().mode(),
            ChromeMode::Editing,
            "{command} should keep editing mode"
        );
        assert!(
            controller.chrome().prompt().text.is_empty(),
            "{command} should leave the prompt empty"
        );
    }
}

#[tokio::test]
async fn inline_multi_skill_directives_activate_one_card_and_submit_stripped_prompt() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let stripped = "\
foo
bar
test test test
bonjour
hello
test test test test
hola
amigo";
    let stripped_for_event = stripped.to_owned();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            let stripped_for_event = stripped_for_event.clone();
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text(stripped_for_event),
                }])
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());
    let prompt = "\
foo
bar
/skill:skill_one test test test
bonjour
hello
/skill:skill_two test test test test
hola
amigo";

    controller.type_text(prompt);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("skill activation succeeds");

    assert!(controller.chrome().prompt().text.is_empty());
    let entries = transcript_entries(&controller);
    let skill_cards = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .count();
    assert_eq!(skill_cards, 1);
    assert!(matches!(
        entries.last(),
        Some(TranscriptEntry::SkillActivation { names, body, .. })
            if names == &vec!["skill_one".to_owned(), "skill_two".to_owned()] && body == stripped
    ));

    controller
        .wait_for_active_turn()
        .await
        .expect("stripped prompt turn completes");
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text(stripped)]);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("User activated the skills \"skill_one\", \"skill_two\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"skill_one\" trigger=\"user-slash\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<neo-user-request>\nfoo\nbar\ntest test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("ONE: test test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("TWO: test test test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.find("ONE:").expect("first skill")
            < skill_context.find("TWO:").expect("second skill"),
        "{skill_context}"
    );
    assert!(
        !transcript_entries(&controller)
            .iter()
            .any(|entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == stripped)),
        "skill activation body should not be rendered again as a user message"
    );
}

#[tokio::test]
async fn inline_skill_directive_with_paste_marker_renders_one_card() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let paste_text = "line one\nline two\nline three";
    let expected_display = format!("{paste_text}review this");
    let expanded_for_event = expected_display.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            let expanded_for_event = expanded_for_event.clone();
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text(expanded_for_event),
                }])
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());
    controller.paste_store.insert(1, paste_text.to_owned());

    controller.type_text("/skill:skill_one [paste #1 +3 lines]review this");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("skill activation succeeds");

    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let entries = transcript_entries(&controller);
    let skill_card_count = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .count();
    assert_eq!(
        skill_card_count, 1,
        "expected exactly one skill activation card"
    );
    let skill_card = entries
        .iter()
        .find(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .expect("one skill activation card");
    assert!(matches!(
        skill_card,
        TranscriptEntry::SkillActivation { names, body, .. }
            if names == &vec!["skill_one".to_owned()] && body == &expected_display
    ));

    assert!(
        !entries.iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == &expected_display)
        ),
        "expanded skill activation body should not be rendered again as a user message"
    );

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text(expected_display)]);
}

#[tokio::test]
async fn inline_skill_directive_without_whitespace_prefix_submits_as_plain_prompt() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());

    controller.type_text("abc/skill:skill_one test");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("plain prompt submits");

    controller
        .wait_for_active_turn()
        .await
        .expect("plain prompt turn completes");
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("abc/skill:skill_one test")]
    );
    assert_eq!(requests[0].skill_context, None);
}

#[tokio::test]
async fn inline_skill_directive_unknown_skill_reports_status_without_submitting() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());

    controller.type_text("foo /skill:missing test");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("unknown skill handled");

    assert!(requests.lock().expect("requests lock").is_empty());
    assert!(transcript_has_status(
        &controller,
        "skill `missing` not found"
    ));
    assert_eq!(controller.chrome().prompt().text, "foo /skill:missing test");
}

#[test]
fn prompt_completions_merges_real_prompt_package_and_session_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(prompts_dir.join("review-pack")).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review local changes\n---\nReview $1.\n",
    )
    .expect("write local prompt");
    fs::write(
        prompts_dir.join("review-pack/refactor.md"),
        "---\ndescription: Refactor from package\n---\nRefactor $1.\n",
    )
    .expect("write packaged prompt");

    let completions =
        prompt_completions(temp.path(), "/", &[], None, true).expect("slash completions");
    let by_value = completions
        .iter()
        .map(|item| (item.value.as_str(), item))
        .collect::<BTreeMap<_, _>>();

    assert_eq!(
        by_value["/review"].description.as_deref(),
        Some("Review local changes")
    );
    assert_eq!(
        by_value["/refactor"].description.as_deref(),
        Some("Refactor from package")
    );
    assert_eq!(
        by_value["/resume"].description.as_deref(),
        Some("Resume a local session")
    );
    for item in by_value.values() {
        let description = item.description.as_deref().unwrap_or_default();
        assert!(!description.contains("source:"));
        assert!(!description.contains("provider:"));
        assert!(!description.contains("trust:"));
    }
    assert!(!by_value.contains_key("/tree"));
    assert!(!by_value.contains_key("/sync"));
}

#[test]
fn slash_completion_descriptions_hide_internal_metadata() {
    let completions = prompt_completions(&test_workspace_root(), "/ask", &[], None, true)
        .expect("slash completions");
    let ask = completions
        .iter()
        .find(|item| item.value == "/ask")
        .expect("missing /ask completion");

    assert_eq!(ask.label, "/ask");
    assert_eq!(ask.description.as_deref(), Some("ask permission mode"));
    let description = ask.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"));
    assert!(!description.contains("trust:"));
    assert!(!description.contains("source:"));
}

#[test]
fn slash_completions_include_dynamic_skill_commands_without_metadata() {
    let skill_store = skill_store_with_refactor_skill();
    let completions = prompt_completions(
        &test_workspace_root(),
        "/skill:",
        &[],
        Some(&skill_store),
        true,
    )
    .expect("skill completions resolve");
    let skill = completions
        .iter()
        .find(|item| item.value == "/skill:refactor")
        .expect("missing dynamic skill command");

    assert_eq!(skill.label, "/skill:refactor");
    assert_eq!(
        skill.description.as_deref(),
        Some("Refactor with project conventions")
    );
    let description = skill.description.as_deref().unwrap_or_default();
    assert!(!description.contains("provider:"), "{description}");
    assert!(!description.contains("trust:"), "{description}");
    assert!(!description.contains("source:"), "{description}");
}

#[test]
fn slash_completions_include_help_command() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("completions resolve");
    let help = completions
        .iter()
        .find(|item| item.value == "/help")
        .expect("missing /help completion");

    assert_eq!(help.label, "/help");
    assert_eq!(help.description.as_deref(), Some("Show help information"));
}

#[test]
fn verbose_startup_mentions_local_keybinding_overrides() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config
        .tui
        .keybindings
        .insert("tui.input.submit".to_owned(), vec!["ctrl+j".to_owned()]);

    let mut controller = controller_for_config(&config);
    controller.apply_startup_options(
        &config,
        InteractiveOptions {
            verbose_startup: true,
        },
    );
    assert!(transcript_has_status(
        &controller,
        "keybindings: 1 override"
    ));
}

#[test]
fn completion_catalog_excludes_extension_commands() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(temp.path().join("src")).expect("create src");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write main");

    let catalog = CompletionCatalog {
        slash_prompts: vec![PickerItem::new(
            "/review",
            "/review",
            Some("Review project changes"),
        )],
        prompt_packages: vec![PickerItem::new(
            "/review-package",
            "/review-package",
            Some("Packaged review prompt"),
        )],
        session_commands: vec![PickerItem::new(
            "/review-session",
            "/review-session",
            Some("Session command"),
        )],
        model_items: vec![PickerItem::new(
            "anthropic/claude-sonnet",
            "anthropic/claude-sonnet",
            Some("Messages"),
        )],
    };

    let files =
        completion_source_candidates(temp.path(), "src/ma", &catalog).expect("file completions");
    assert!(files.iter().any(|candidate| {
        candidate.value == "src/main.rs" && candidate.source == CompletionSource::LocalFile
    }));

    let slash =
        completion_source_candidates(temp.path(), "/rev", &catalog).expect("slash completions");
    let slash_sources = slash
        .iter()
        .map(|candidate| candidate.source)
        .collect::<Vec<_>>();
    assert!(slash_sources.contains(&CompletionSource::SlashPrompt));
    assert!(slash_sources.contains(&CompletionSource::PromptPackage));
    assert!(slash_sources.contains(&CompletionSource::SessionCommand));
    assert!(slash.iter().all(|candidate| {
        candidate
            .to_picker_item()
            .description
            .as_deref()
            .is_none_or(|description| !description.contains("extension command"))
    }));

    let models =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("model completions");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].value, "@anthropic/claude-sonnet");
    assert_eq!(models[0].source, CompletionSource::ProviderModel);
}

#[tokio::test]
async fn event_loop_slash_resume_opens_local_session_picker() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                "alpha",
                "Alpha",
                test_workspace_root(),
                "root",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("/resume");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash resume command runs locally");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::SessionPicker(_))
    ));
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(requests.lock().expect("recorded requests").is_empty());
}

#[tokio::test]
async fn slash_help_opens_help_panel_overlay() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured = std::sync::Arc::clone(&captured);
            async move {
                captured.lock().expect("recorded requests").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::HelpPanel(_))
    ));
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(requests.lock().expect("recorded requests").is_empty());

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("scroll help panel");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageUp))
        .await
        .expect("scroll help panel back up");

    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("help · Esc / Enter / q close"),
        "{snapshot}"
    );
    assert!(snapshot.contains("/help"), "{snapshot}");
    assert!(snapshot.contains("/ask"), "{snapshot}");
}

#[tokio::test]
async fn slash_help_panel_includes_dynamic_skill_commands() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.skill_store = Some(skill_store_with_refactor_skill());

    controller.type_text("/help");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash help command runs locally");

    for _ in 0..8 {
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
            .await
            .expect("scroll help panel");
    }

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("/skill:refactor"), "{snapshot}");
    assert!(
        snapshot.contains("Refactor with project conventions"),
        "{snapshot}"
    );
}

#[test]
fn event_loop_slash_tree_absent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let completions =
        prompt_completions(temp.path(), "/", &[], None, true).expect("slash completions");
    assert!(
        !completions.iter().any(|item| item.value == "/tree"),
        "/tree should not appear in slash completion items"
    );
}

#[tokio::test]
async fn event_loop_tab_completes_provider_model_prefix() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                ),
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anth");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes provider/model prefix");

    assert_eq!(
        controller.chrome().prompt().text,
        "@anthropic/claude-sonnet"
    );
    assert_eq!(controller.chrome().prompt().cursor, 24);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_inline_provider_model_prefix_overrides_submitted_turn() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "inline model selected".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new(
                    "anthropic/claude-sonnet",
                    "anthropic/claude-sonnet",
                    Some("Messages"),
                ),
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anth");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes provider/model prefix");
    controller.type_text(" explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with inline model");
    controller
        .wait_for_active_turn()
        .await
        .expect("inline model turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text("explain this file")]);
    let selected = requests[0].model.as_ref().expect("inline model");
    assert_eq!(selected.provider, "anthropic");
    assert_eq!(selected.model, "claude-sonnet");
}

#[tokio::test]
async fn event_loop_keeps_unknown_at_prefix_as_prompt_text() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@src/main.rs explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with file mention");
    controller
        .wait_for_active_turn()
        .await
        .expect("file mention turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@src/main.rs explain this file")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_inline_model_token_without_prompt_does_not_override_model() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("Messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("@anthropic/claude-sonnet");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits literal model token");
    controller
        .wait_for_active_turn()
        .await
        .expect("literal model token turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@anthropic/claude-sonnet")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_tab_extends_common_filesystem_completion_prefix() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join("README.md"), "readme\n").expect("write readme");
    fs::write(temp.path().join("RELEASE.md"), "release\n").expect("write release");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller.type_text("open R");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab extends common prefix");

    assert_eq!(controller.chrome().prompt().text, "open RE");
    assert_eq!(controller.chrome().prompt().cursor, 7);
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_dispatches_editor_scroll_actions_to_transcript_view() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    for index in 0..10 {
        controller
            .transcript_mut()
            .push_status(format!("line {index}"));
    }
    controller.transcript_mut().sync_transcript_view(10, 2);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageUp))
        .await
        .expect("page up scrolls transcript");
    assert_eq!(transcript_scrollback(&controller), 8);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorDown))
        .await
        .expect("cursor down scrolls transcript toward bottom");
    assert_eq!(transcript_scrollback(&controller), 8);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageDown))
        .await
        .expect("page down returns transcript to bottom");
    assert_eq!(transcript_scrollback(&controller), 0);
}

#[tokio::test]
async fn event_loop_uses_up_down_keys_for_prompt_history() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("first turn completes");

    controller.type_text("second prompt");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("second prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("second turn completes");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls latest prompt");
    assert_eq!(controller.chrome().prompt().text, "second prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls older prompt");
    assert_eq!(controller.chrome().prompt().text, "first prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves toward newer prompt");
    assert_eq!(controller.chrome().prompt().text, "second prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down restores empty draft");
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn event_loop_dispatches_mouse_wheel_to_transcript_view() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.transcript_mut().sync_transcript_view(30, 6);

    controller
        .handle_input_event(InputEvent::ScrollUp(3))
        .await
        .expect("wheel up scrolls transcript toward older rows");
    assert_eq!(transcript_scrollback(&controller), 3);

    controller
        .handle_input_event(InputEvent::ScrollDown(2))
        .await
        .expect("wheel down scrolls transcript toward newest rows");
    assert_eq!(transcript_scrollback(&controller), 1);

    controller
        .handle_input_event(InputEvent::ScrollDown(3))
        .await
        .expect("wheel down follows tail at bottom");
    assert_eq!(transcript_scrollback(&controller), 0);
}

#[tokio::test]
async fn event_loop_submit_restores_transcript_follow_tail() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.transcript_mut().sync_transcript_view(30, 6);

    controller
        .handle_input_event(InputEvent::ScrollUp(5))
        .await
        .expect("wheel up scrolls transcript");
    assert!(transcript_scrollback(&controller) > 0);
    assert!(
        !controller
            .transcript()
            .transcript()
            .viewport()
            .is_following_tail()
    );

    controller
        .handle_input_event(InputEvent::Insert('h'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("typing works");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit restores tail before sending");

    assert_eq!(transcript_scrollback(&controller), 0);
    assert!(
        controller
            .transcript()
            .transcript()
            .viewport()
            .is_following_tail()
    );
}

#[tokio::test]
async fn event_loop_ctrl_o_toggles_tool_detail() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "README.md" }),
        });
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "tool-1".to_owned(),
            name: "Read".to_owned(),
            result: ToolResult::ok("expanded file content"),
        });
    controller
        .transcript_mut()
        .select_visible_transcript_entry();

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o key toggles tool detail");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.transcript().tool_output_expanded());
}

#[tokio::test]
async fn event_loop_ctrl_t_expands_overflowing_todo_panel() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_todo_items(
        (0..7)
            .map(|index| {
                neo_tui::widgets::TodoDisplayItem::new(
                    format!("todo {index}"),
                    neo_tui::widgets::TodoDisplayStatus::Pending,
                )
            })
            .collect(),
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t expands overflowing todo panel");

    assert!(controller.chrome().todo_panel_expanded());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t collapses overflowing todo panel");

    assert!(!controller.chrome().todo_panel_expanded());
}

#[tokio::test]
async fn event_loop_ctrl_t_is_noop_when_todo_panel_does_not_overflow() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
            "todo",
            neo_tui::widgets::TodoDisplayStatus::Pending,
        )]);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+t").expect("valid key")))
        .await
        .expect("ctrl-t no-ops without todo overflow");

    assert!(!controller.chrome().todo_panel_expanded());
    assert!(controller.chrome().prompt().text.is_empty());
}

#[tokio::test]
async fn event_loop_model_picker_action_opens_model_picker() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "openai/gpt-4.1",
                "openai/gpt-4.1",
                Some("test model"),
            )],
        },
        empty_session_loader,
    );

    controller.local_config = Some(test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "openai/gpt-4.1".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                display_name: Some("test model".into()),
                ..ModelConfig::default()
            },
        )]),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
        .await
        .expect("model picker action opens model picker");

    assert!(
        controller.chrome().tabbed_model_selector_result().is_some()
            || controller.chrome().focused_overlay().is_some()
    );
}

#[tokio::test]
async fn event_loop_dispatches_select_keybinding_actions_to_overlay_primitives() {
    struct FakeEvents {
        events: std::vec::IntoIter<InputEvent>,
    }

    impl TerminalEvents for FakeEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.events
                .next()
                .ok_or_else(|| anyhow::anyhow!("expected test event"))
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .request_approval("approval-1", "Run command?", "cargo test");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("selection moves down");
    assert_eq!(
        controller.chrome().approval_choice(),
        Some(ApprovalChoice::Deny)
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectUp))
        .await
        .expect("selection moves up");
    assert_eq!(
        controller.chrome().approval_choice(),
        Some(ApprovalChoice::Approve)
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("approval confirms");
    assert!(controller.chrome().focused_overlay().is_none());

    controller.tui.chrome_mut().push_overlay(Overlay::new(
        "palette",
        OverlayKind::CommandPalette(CommandPaletteState::new((0..10).map(|index| {
            CommandSpec::new(
                format!("command-{index}"),
                format!("Command {index}"),
                None::<String>,
            )
        }))),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageDown))
        .await
        .expect("selection pages down");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-8");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectPageUp))
        .await
        .expect("selection pages up");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "command-0");
    let _ = controller.tui.chrome_mut().close_focused_overlay();

    controller.tui.chrome_mut().push_overlay(Overlay::new(
        "custom",
        OverlayKind::Message("Body".to_owned()),
    ));
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            FakeEvents {
                events: vec![
                    InputEvent::Action(KeybindingAction::SelectCancel),
                    InputEvent::Interrupt,
                    InputEvent::Interrupt,
                ]
                .into_iter(),
            },
        )
        .await
        .expect("event loop exits after canceling overlay and receiving cancel again");

    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_opens_command_palette_and_runs_local_model_command() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![PickerItem::new(
                "anthropic/claude-sonnet",
                "anthropic/claude-sonnet",
                Some("messages"),
            )],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.local_config = Some(test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "anthropic/claude-sonnet".to_owned(),
            ModelConfig {
                provider: "anthropic".to_owned(),
                model: "claude-sonnet".to_owned(),
                display_name: Some("messages".into()),
                ..ModelConfig::default()
            },
        )]),
    ));
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("command palette opens");
    let Some(OverlayKind::CommandPalette(palette)) = controller
        .chrome()
        .focused_overlay()
        .map(|overlay| &overlay.kind)
    else {
        panic!("expected command palette overlay");
    };
    assert_eq!(palette.selected_command().expect("command").id, "sessions");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("moves to model command");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("command runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::TabbedModelSelector(_))
    ));
}

#[tokio::test]
async fn command_palette_inserts_project_prompt_template_command() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(
        prompts_dir.join("review.md"),
        "---\ndescription: Review a target\nargument-hint: <path>\n---\nReview $1.\n",
    )
    .expect("write review prompt");

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.completion_root = temp.path().to_path_buf();

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "prompt-template.review" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to review command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("review command")
            .id,
        "prompt-template.review"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("prompt template command inserts invocation");

    assert_eq!(controller.chrome().prompt().text, "/review ");
    assert_eq!(controller.chrome().prompt().cursor, 8);
    assert!(controller.chrome().focused_overlay().is_none());

    controller.type_text("src/lib.rs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt template command submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("prompt template turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("Review src/lib.rs.")]
    );
}

#[tokio::test]
async fn command_palette_exports_active_session_to_html() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir.clone());
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello <script>alert(1)</script>\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"use **bold** safely\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let config = test_config(temp.path(), sessions_dir.clone());
    let mut controller = controller_for_config(&config);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("session loads");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.exportHtml" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to export command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("export command")
            .id,
        "session.exportHtml"
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("export command runs");

    let export_path = neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(SESSION_A))
        .with_extension("html");
    let html = fs::read_to_string(&export_path).expect("read exported html");
    assert!(html.contains(&format!("<title>neo session {SESSION_A}</title>")));
    assert!(html.contains("<strong>bold</strong>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::Status { text, .. }
                if text.contains(&format!("Exported session {SESSION_A} to"))
                    && text.contains(&export_path.display().to_string())
        )
    }));
}

#[tokio::test]
async fn command_palette_export_html_without_active_session_shows_local_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let config = test_config(temp.path(), sessions_dir);
    let mut controller = controller_for_config(&config);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.exportHtml" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to export command");
    }

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("export command handles missing session locally");

    assert!(transcript_has_status(
        &controller,
        "No active session to export"
    ));
}

#[tokio::test]
async fn event_loop_confirms_approval_choice_to_running_turn() {
    use std::collections::VecDeque;

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let decisions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_decisions = std::sync::Arc::clone(&decisions);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let captured_decisions = std::sync::Arc::clone(&captured_decisions);
        Box::pin(async move {
            channels.send_event(AgentEvent::ApprovalRequested {
                turn: 1,
                id: "tool-1".to_owned(),
                operation: neo_agent_core::PermissionOperation::Tool,
                subject: "Write".to_owned(),
                arguments: serde_json::json!({"path": "approved.txt"}),
                session_scope: None,
                prefix_rule: None,
                suggestions: Vec::new(),
            });
            let (decision_tx, decision_rx) = oneshot::channel();
            channels
                .approvals
                .send(crate::modes::run::PromptApprovalRequest {
                    id: "tool-1".to_owned(),
                    decision_tx,
                    feedback_tx: None,
                    selected_label_tx: None,
                    session_option_label: None,
                    prefix_option_label: None,
                })
                .expect("approval waiter sent");
            let decision = decision_rx.await.expect("approval decision");
            captured_decisions
                .lock()
                .expect("decisions lock")
                .push(decision);
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "approved".to_owned(),
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("write file");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    None,
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("approval loop completes");

    assert_eq!(
        *decisions.lock().expect("decisions lock"),
        vec![PermissionApprovalDecision::AllowOnce]
    );
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.render_snapshot().contains("approved"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn event_loop_shows_and_resolves_pending_question_from_running_turn() {
    use std::collections::VecDeque;

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_answers = std::sync::Arc::clone(&answers);
    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_frames = std::sync::Arc::clone(&frames);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let captured_answers = std::sync::Arc::clone(&captured_answers);
        Box::pin(async move {
            let (response_tx, response_rx) = oneshot::channel();
            channels
                .questions
                .send(PendingQuestion {
                    id: "question-1".to_owned(),
                    questions: vec![neo_agent_core::QuestionEventData {
                        question: "1 + 1 = ?".to_owned(),
                        header: Some("Math".into()),
                        body: None,
                        options: vec![
                            neo_agent_core::QuestionOptionData {
                                label: "2".to_owned(),
                                description: Some("Correct".into()),
                            },
                            neo_agent_core::QuestionOptionData {
                                label: "3".to_owned(),
                                description: Some("Too high".into()),
                            },
                        ],
                        multi_select: false,
                    }],
                    response_tx,
                })
                .expect("question sent");
            let response = response_rx.await.expect("question response");
            captured_answers
                .lock()
                .expect("answers lock")
                .extend(response.answers);
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "answered".to_owned(),
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("ask me");
    controller
        .run_terminal_loop(
            move |app| {
                captured_frames
                    .lock()
                    .expect("frames lock")
                    .push(render_overlay_snapshot(app, 80).join("\n"));
                Ok(())
            },
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    Some(InputEvent::Action(KeybindingAction::InputTab)),
                    Some(InputEvent::Action(KeybindingAction::SelectConfirm)),
                    None,
                    Some(InputEvent::Interrupt),
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("question loop completes");

    assert_eq!(*answers.lock().expect("answers lock"), vec!["2"]);
    assert!(
        frames
            .lock()
            .expect("frames lock")
            .iter()
            .any(|frame| frame.contains("1 + 1 = ?") && frame.contains("[1] 2")),
        "pending question should be visible before it is answered"
    );
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(
        controller
            .render_snapshot()
            .contains("Collected your answers")
    );
    assert!(controller.render_snapshot().contains("answered"));
}

#[tokio::test]
async fn approval_number_shortcut_confirms_session_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "approved.txt"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::FileWrite {
                workspace: test_workspace_root().display().to_string(),
                path: test_workspace_root()
                    .join("approved.txt")
                    .display()
                    .to_string(),
                operation: neo_agent_core::FileWriteApprovalOperation::Write,
            }],
            label: "Approve writes to this file for this session".to_owned(),
            detail: "approved.txt".to_owned(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    controller.pending_approvals.insert(
        "tool-1".to_owned(),
        PendingApprovalResponse {
            decision_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: Some("Approve writes to this file for this session".into()),
            prefix_option_label: None,
        },
    );

    controller
        .handle_input_event(InputEvent::Insert('2'))
        .await
        .expect("number shortcut handles approval");

    assert_eq!(
        decision_rx.await.expect("approval decision"),
        PermissionApprovalDecision::AllowForSession
    );
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(
        controller
            .render_snapshot()
            .contains("Approved writes to this file for this session")
    );
}

#[tokio::test]
async fn prefix_approval_choice_dispatches_prefix_decision() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "cargo test".to_owned(),
        arguments: serde_json::json!({"command": "cargo test"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                workspace: test_workspace_root().display().to_string(),
                cwd: test_workspace_root().display().to_string(),
                command: vec!["cargo".to_owned(), "test".to_owned()],
            }],
            label: "Approve this exact command for this session".to_owned(),
            detail: test_workspace_root().display().to_string(),
        }),
        prefix_rule: Some(neo_agent_core::PrefixApprovalRule {
            prefix: vec!["cargo".to_owned(), "test".to_owned()],
            label: "cargo test".to_owned(),
        }),
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    controller.pending_approvals.insert(
        "tool-1".to_owned(),
        PendingApprovalResponse {
            decision_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: Some("Approve this exact command for this session".into()),
            prefix_option_label: Some("Approve commands starting with cargo test".into()),
        },
    );

    controller
        .handle_input_event(InputEvent::Insert('3'))
        .await
        .expect("number shortcut handles prefix approval");

    assert_eq!(
        decision_rx.await.expect("approval decision"),
        PermissionApprovalDecision::AllowForPrefix
    );
    assert!(
        controller
            .render_snapshot()
            .contains("Approved commands starting with cargo test")
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn question_dialog_consumes_keyboard_before_prompt_editing() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![
            neo_agent_core::QuestionEventData {
                question: "2 + 2 = ?".to_owned(),
                header: Some("Single".into()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "3".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "4".to_owned(),
                        description: None,
                    },
                ],
                multi_select: false,
            },
            neo_agent_core::QuestionEventData {
                question: "Pick primes".to_owned(),
                header: Some("Multi".into()),
                body: None,
                options: vec![
                    neo_agent_core::QuestionOptionData {
                        label: "2".to_owned(),
                        description: None,
                    },
                    neo_agent_core::QuestionOptionData {
                        label: "4".to_owned(),
                        description: None,
                    },
                ],
                multi_select: true,
            },
        ],
        response_tx,
    });

    controller
        .handle_input_event(InputEvent::Insert('2'))
        .await
        .expect("number shortcut selects a question option");
    assert_eq!(controller.chrome().prompt().text, "draft");
    {
        let state = controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused");
        assert_eq!(state.active_tab, 1);
        assert!(state.questions[0].selected[1]);
    }

    controller
        .handle_input_event(InputEvent::Insert('a'))
        .await
        .expect("letters are consumed by the question dialog");
    assert_eq!(controller.chrome().prompt().text, "draft");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorRight))
        .await
        .expect("right arrow action switches to submit");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorCursorLeft))
        .await
        .expect("left arrow action switches back to the question");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert_eq!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .active_tab,
        1
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab switches to submit instead of editing the prompt");
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );
}

#[tokio::test]
async fn question_dialog_prioritizes_real_keybindings_before_prompt_editing() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick one".to_owned(),
            header: Some("Single".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "First".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Second".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
    });

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("down selects Other");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("down selects Other");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("enter starts Other editing");
    controller
        .handle_input_event(InputEvent::Insert('x'))
        .await
        .expect("typed text goes to Other");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits Other text");
    {
        let state = controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused");
        assert_eq!(state.questions[0].other_text, "");
    }
    assert_eq!(controller.chrome().prompt().text, "draft");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("right").expect("valid key")))
        .await
        .expect("right switches to submit");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("left").expect("valid key")))
        .await
        .expect("left switches back to question");
    assert_eq!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .active_tab,
        0
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("tab").expect("valid key")))
        .await
        .expect("tab switches to submit");
    assert!(
        controller
            .chrome()
            .question_dialog_state()
            .expect("question stays focused")
            .on_submit_tab()
    );
}

#[tokio::test]
async fn approval_uses_selection_priority_for_real_keys() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "approved.txt"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::FileWrite {
                workspace: test_workspace_root().display().to_string(),
                path: test_workspace_root()
                    .join("approved.txt")
                    .display()
                    .to_string(),
                operation: neo_agent_core::FileWriteApprovalOperation::Write,
            }],
            label: "Approve writes to this file for this session".to_owned(),
            detail: "approved.txt".to_owned(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects approval option");
    assert_eq!(
        controller.chrome().approval_choice(),
        Some(ApprovalChoice::AlwaysApprove)
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter confirms approval");

    assert_eq!(
        decision_rx.await.expect("approval decision"),
        PermissionApprovalDecision::AllowForSession
    );
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn approval_revise_collects_feedback_without_editing_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("draft");
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "denied.txt"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects deny option");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects revise option");
    assert_eq!(
        controller.chrome().approval_choice(),
        Some(ApprovalChoice::Revise)
    );

    // First Enter enters feedback collection mode.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter begins feedback collection");

    controller
        .handle_input_event(InputEvent::Insert('n'))
        .await
        .expect("typed feedback is captured by approval dialog");
    controller
        .handle_input_event(InputEvent::Paste("o thanks".to_owned()))
        .await
        .expect("pasted feedback is captured by approval dialog");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("backspace").expect("valid key")))
        .await
        .expect("backspace edits approval feedback");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter confirms revise");

    assert_eq!(controller.chrome().prompt().text, "draft");
    assert_eq!(
        decision_rx.await.expect("approval decision"),
        PermissionApprovalDecision::Reject
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("Revision feedback: no thank"),
        "feedback should be surfaced after resolve: {snapshot}"
    );
}

#[tokio::test]
async fn approval_cancel_rejects_pending_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "denied.txt"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects approval");

    assert_eq!(
        decision_rx.await.expect("approval decision"),
        PermissionApprovalDecision::Reject
    );
    assert!(controller.render_snapshot().contains("Rejected"));
}

#[tokio::test]
async fn approval_requests_are_handled_one_at_a_time() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                workspace: test_workspace_root().display().to_string(),
                cwd: test_workspace_root().display().to_string(),
                command: vec!["printf".to_owned(), "one".to_owned()],
            }],
            label: "Approve this exact command for this session".to_owned(),
            detail: test_workspace_root().display().to_string(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-2".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf two".to_owned(),
        arguments: serde_json::json!({"command": "printf two"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, _second_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(first_tx));
    controller
        .pending_approvals
        .insert("tool-2".to_owned(), pending_approval_response(second_tx));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("first approval confirms");

    assert_eq!(
        first_rx.await.expect("first decision"),
        PermissionApprovalDecision::AllowOnce
    );
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|(id, _, _, _, _)| id),
        Some("tool-2")
    );
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf two"));
}

#[tokio::test]
async fn approval_transcript_only_shows_active_request() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                workspace: test_workspace_root().display().to_string(),
                cwd: test_workspace_root().display().to_string(),
                command: vec!["printf".to_owned(), "one".to_owned()],
            }],
            label: "Approve this exact command for this session".to_owned(),
            detail: test_workspace_root().display().to_string(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-2".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf two".to_owned(),
        arguments: serde_json::json!({"command": "printf two"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf one"));
    assert!(!snapshot.contains("printf two"));
    assert!(snapshot.contains("queued: 1 approval waiting"));
}

#[tokio::test]
async fn approval_cancel_advances_next_visible_request() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                workspace: test_workspace_root().display().to_string(),
                cwd: test_workspace_root().display().to_string(),
                command: vec!["printf".to_owned(), "one".to_owned()],
            }],
            label: "Approve this exact command for this session".to_owned(),
            detail: test_workspace_root().display().to_string(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-2".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf two".to_owned(),
        arguments: serde_json::json!({"command": "printf two"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, _second_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(first_tx));
    controller
        .pending_approvals
        .insert("tool-2".to_owned(), pending_approval_response(second_tx));

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects current approval");

    assert_eq!(
        first_rx.await.expect("first decision"),
        PermissionApprovalDecision::Reject
    );
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Rejected"));
    assert!(snapshot.contains("printf two"));
    assert!(!snapshot.contains("queued:"));
}

#[tokio::test]
async fn approval_interrupt_rejects_all_pending_approvals() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-2".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf two".to_owned(),
        arguments: serde_json::json!({"command": "printf two"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (first_tx, first_rx) = oneshot::channel();
    let (second_tx, second_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(first_tx));
    controller
        .pending_approvals
        .insert("tool-2".to_owned(), pending_approval_response(second_tx));

    controller
        .handle_input_event(InputEvent::Interrupt)
        .await
        .expect("interrupt rejects pending approvals");

    assert_eq!(
        first_rx.await.expect("first decision"),
        PermissionApprovalDecision::Reject
    );
    assert_eq!(
        second_rx.await.expect("second decision"),
        PermissionApprovalDecision::Reject
    );
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
}

#[tokio::test]
async fn approval_interrupt_preserves_rejection_for_late_channel_registration() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });

    controller
        .handle_input_event(InputEvent::Interrupt)
        .await
        .expect("interrupt rejects visible approval");
    let (decision_tx, decision_rx) = oneshot::channel();
    controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
        id: "tool-1".to_owned(),
        decision_tx,
        feedback_tx: None,
        selected_label_tx: None,
        session_option_label: None,
        prefix_option_label: None,
    });

    assert_eq!(
        decision_rx.await.expect("late approval decision"),
        PermissionApprovalDecision::Reject
    );
    assert!(controller.pending_approvals.is_empty());
}

#[tokio::test]
async fn event_loop_interrupt_cancels_active_turn_token() {
    use std::{collections::VecDeque, sync::Arc as StdArc};

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let captured_token = StdArc::new(std::sync::Mutex::new(None));
    let observed_token = StdArc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = StdArc::clone(&observed_token);
        Box::pin(async move {
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("cancel me");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("interrupt exits terminal loop");

    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn event_loop_interrupt_drains_cancelled_barriers_before_exit() {
    use std::{collections::VecDeque, sync::Arc as StdArc};

    struct ScriptedEvents {
        events: VecDeque<Option<InputEvent>>,
    }

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.poll_input_event(Duration::from_millis(0))?
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self
                .events
                .pop_front()
                .unwrap_or(Some(InputEvent::Interrupt)))
        }
    }

    let captured_token = StdArc::new(std::sync::Mutex::new(None));
    let observed_token = StdArc::clone(&captured_token);
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let finished_tx = StdArc::new(std::sync::Mutex::new(Some(finished_tx)));
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_token = StdArc::clone(&observed_token);
        let finished_tx = StdArc::clone(&finished_tx);
        Box::pin(async move {
            *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
            channels.send_event(AgentEvent::MessageStarted {
                turn: 1,
                id: "assistant-1".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "started".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            channels.send_event(AgentEvent::MessageFinished {
                turn: 1,
                id: "assistant-1".to_owned(),
                stop_reason: StopReason::Cancelled,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            });
            channels.send_event(AgentEvent::RunFinished {
                turn: 1,
                stop_reason: StopReason::Cancelled,
            });
            if let Some(finished_tx) = finished_tx.lock().expect("finished lock").take() {
                let _ = finished_tx.send(());
            }
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("cancel me");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            ScriptedEvents {
                events: VecDeque::from([
                    Some(InputEvent::Submit),
                    None,
                    Some(InputEvent::Interrupt),
                ]),
            },
        )
        .await
        .expect("interrupt exits terminal loop after draining cancellation");

    tokio::time::timeout(Duration::from_secs(1), finished_rx)
        .await
        .expect("turn driver should finish after cancellation")
        .expect("finished sender should not be dropped before sending");
    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    assert!(token.is_cancelled());
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(controller.active_turn.is_none());
}

#[test]
fn rebuild_transcript_from_session_replays_tool_calls_and_results() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        ["branch summary: inspected project".to_owned()],
        [
            AgentMessage::user_text("inspect"),
            AgentMessage::assistant(
                [Content::text("reading")],
                [neo_agent_core::AgentToolCall {
                    id: "tool-1".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"README.md"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tool-1", "Read", [Content::text("README contents")], false),
        ],
    );

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("branch summary: inspected project"));
    assert!(rendered.contains("inspect"));
    assert!(rendered.contains("reading"));
    assert!(rendered.contains("Used Read (README.md)"));
    assert!(rendered.contains("README contents"));
    assert!(!rendered.contains("Using Read"));
}

#[test]
fn replay_session_into_transcript_suppresses_delegate_tool_when_delegate_card_replays() {
    let mut transcript = TranscriptPane::new(100, 16);
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("audit replay duplication");
    let agent_id = snapshot.id.as_str().to_owned();
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        Vec::new(),
        [
            AgentMessage::assistant(
                [Content::text("delegating")],
                [neo_agent_core::AgentToolCall {
                    id: "delegate-tool-1".into(),
                    name: "Delegate".into(),
                    raw_arguments: r#"{"task":"audit replay duplication"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result(
                "delegate-tool-1",
                "Delegate",
                [Content::text(format!("agent_id: {agent_id}"))],
                false,
            ),
        ],
    )
    .with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("delegating")],
                [neo_agent_core::AgentToolCall {
                    id: "delegate-tool-1".into(),
                    name: "Delegate".into(),
                    raw_arguments: r#"{"task":"audit replay duplication"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::DelegateStarted {
            turn: 7,
            agent: snapshot.clone(),
        },
        AgentEvent::DelegateFinished {
            turn: 7,
            agent: snapshot,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "delegate-tool-1",
                "Delegate",
                [Content::text(format!("agent_id: {agent_id}"))],
                false,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 16)
        .expect("render frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("· Delegate"), "{rendered}");
    assert!(rendered.contains("audit replay duplication"), "{rendered}");
    assert!(
        !rendered.contains("Used Delegate"),
        "Delegate tool run should be represented by the delegate card only: {rendered}"
    );
    assert!(rendered.contains("delegating"), "{rendered}");
}

#[test]
fn replay_session_into_transcript_keeps_delegate_card_in_event_order() {
    let mut transcript = TranscriptPane::new(120, 30);
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("audit after read");
    let agent_id = snapshot.id.as_str().to_owned();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::<AgentMessage>::new())
        .with_events([
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [Content::text("inspect then delegate")],
                    [
                        neo_agent_core::AgentToolCall {
                            id: "read-tool-1".into(),
                            name: "Read".into(),
                            raw_arguments: r#"{"path":"README.md"}"#.into(),
                        },
                        neo_agent_core::AgentToolCall {
                            id: "delegate-tool-1".into(),
                            name: "Delegate".into(),
                            raw_arguments: r#"{"task":"audit after read"}"#.into(),
                        },
                    ],
                    StopReason::ToolUse,
                ),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::tool_result(
                    "read-tool-1",
                    "Read",
                    [Content::text("README contents")],
                    false,
                ),
            },
            AgentEvent::DelegateStarted {
                turn: 7,
                agent: snapshot.clone(),
            },
            AgentEvent::DelegateFinished {
                turn: 7,
                agent: snapshot,
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::tool_result(
                    "delegate-tool-1",
                    "Delegate",
                    [Content::text(format!("agent_id: {agent_id}"))],
                    false,
                ),
            },
        ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(120, 30)
        .expect("render frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    let read_index = rendered
        .find("README contents")
        .expect("read result visible");
    let delegate_index = rendered.find("· Delegate").expect("delegate card visible");
    assert!(
        read_index < delegate_index,
        "delegate card should replay at its original event position: {rendered}"
    );
    assert!(rendered.contains("Used Read (README.md)"), "{rendered}");
    assert!(
        !rendered.contains("Used Delegate"),
        "restored delegate should be represented by the delegate card only: {rendered}"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn replay_session_into_transcript_suppresses_only_matching_successful_delegate_tool() {
    let mut transcript = TranscriptPane::new(120, 20);
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("successful restored delegate");
    let agent_id = snapshot.id.as_str().to_owned();
    let loaded = LoadedSessionTranscript::new(
        "alpha",
        Vec::new(),
        [
            AgentMessage::assistant(
                [Content::text("trying two delegates")],
                [
                    neo_agent_core::AgentToolCall {
                        id: "delegate-failed".into(),
                        name: "Delegate".into(),
                        raw_arguments: r#"{"task":""}"#.into(),
                    },
                    neo_agent_core::AgentToolCall {
                        id: "delegate-success".into(),
                        name: "Delegate".into(),
                        raw_arguments: r#"{"task":"successful restored delegate"}"#.into(),
                    },
                ],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result(
                "delegate-failed",
                "Delegate",
                [Content::text(
                    "invalid input for Delegate: task must not be empty",
                )],
                true,
            ),
            AgentMessage::tool_result(
                "delegate-success",
                "Delegate",
                [Content::text(format!("agent_id: {agent_id}"))],
                false,
            ),
        ],
    )
    .with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("trying two delegates")],
                [
                    neo_agent_core::AgentToolCall {
                        id: "delegate-failed".into(),
                        name: "Delegate".into(),
                        raw_arguments: r#"{"task":""}"#.into(),
                    },
                    neo_agent_core::AgentToolCall {
                        id: "delegate-success".into(),
                        name: "Delegate".into(),
                        raw_arguments: r#"{"task":"successful restored delegate"}"#.into(),
                    },
                ],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "delegate-failed",
                "Delegate",
                [Content::text(
                    "invalid input for Delegate: task must not be empty",
                )],
                true,
            ),
        },
        AgentEvent::DelegateStarted {
            turn: 7,
            agent: snapshot.clone(),
        },
        AgentEvent::DelegateFinished {
            turn: 7,
            agent: snapshot,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "delegate-success",
                "Delegate",
                [Content::text(format!("agent_id: {agent_id}"))],
                false,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(120, 20)
        .expect("render frame")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("· Delegate"), "{rendered}");
    assert!(
        rendered.contains("successful restored delegate"),
        "{rendered}"
    );
    assert!(rendered.contains("Failed Delegate"), "{rendered}");
    assert!(
        rendered.contains("invalid input for Delegate: task must not be empty"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("Used Delegate"),
        "successful Delegate tool run should be represented by the delegate card only: {rendered}"
    );
}

#[test]
fn rebuild_transcript_from_session_initializes_context_window_usage() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "new",
        "deepseek/deepseek-v4-pro",
        test_workspace_root(),
        |_| async { Ok(Vec::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .set_context_window(Some(ContextWindow::new(1_000_000)));

    let loaded =
        LoadedSessionTranscript::new("alpha", Vec::new(), [AgentMessage::user_text("hello")])
            .with_estimated_context_tokens(393);

    controller.rebuild_transcript_from_session(&loaded);

    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_000_000).with_used_tokens(393))
    );
}

#[tokio::test]
async fn load_session_transcript_estimates_context_usage_for_replayed_session() {
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
        .append(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("remember this"),
        })
        .await
        .expect("append user message");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert_eq!(loaded.estimated_context_tokens, Some(5));
}

#[tokio::test]
async fn load_session_transcript_replays_token_usage_for_footer() {
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
        .append(&AgentEvent::TokenUsage {
            turn: 1,
            usage: neo_agent_core::AgentTokenUsage {
                input_tokens: 33_900,
                output_tokens: 2_800,
                input_cache_read_tokens: 169_200,
                input_cache_write_tokens: 0,
            },
        })
        .await
        .expect("append token usage");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert_eq!(loaded.main_agent_token_usage.input_tokens, 33_900);
    assert_eq!(loaded.main_agent_token_usage.output_tokens, 2_800);
    assert_eq!(
        loaded.main_agent_token_usage.input_cache_read_tokens,
        169_200
    );
    assert_eq!(loaded.main_agent_token_usage.input_cache_write_tokens, 0);
}

#[tokio::test]
async fn load_session_transcript_preserves_delegate_events_for_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    let snapshot = neo_agent_core::multi_agent::MultiAgentRuntime::new()
        .start_foreground_delegate_for_test("audit paths");
    writer
        .append(&AgentEvent::DelegateStarted {
            turn: 1,
            agent: snapshot,
        })
        .await
        .expect("append delegate");
    writer.flush().await.expect("flush session");

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");

    assert!(
        loaded
            .events
            .iter()
            .any(|event| matches!(event, AgentEvent::DelegateStarted { .. })),
        "delegate events should be preserved for transcript replay"
    );
}

#[test]
fn rebuild_transcript_from_session_restores_footer_token_usage() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .tui
        .chrome_mut()
        .set_context_window(Some(ContextWindow::new(512_000)));

    let mut usage = neo_tui::shell::MainAgentTokenUsage::default();
    usage.add(neo_agent_core::AgentTokenUsage {
        input_tokens: 33_900,
        output_tokens: 2_800,
        input_cache_read_tokens: 169_200,
        input_cache_write_tokens: 0,
    });
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_estimated_context_tokens(5_000)
        .with_main_agent_token_usage(usage);

    controller.rebuild_transcript_from_session(&loaded);

    let footer = controller
        .render_snapshot()
        .lines()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context")
        .to_owned();

    assert!(footer.contains("ctx 5k/512k"));
    assert!(footer.contains("↑33.9k"));
    assert!(footer.contains("↓2.8k"));
    assert!(footer.contains("cache 169.2k read"));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn event_loop_opens_session_picker_and_continues_selected_transcript() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 2,
                        id: "assistant-2".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 2,
                        text: "continued".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 2,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                SESSION_A,
                "Alpha session",
                test_workspace_root(),
                "branch summary",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |session_id| async move {
            assert_eq!(session_id, SESSION_A);
            Ok(LoadedSessionTranscript::new(
                SESSION_A,
                ["branch summary: Local branch summary".to_owned()],
                [
                    AgentMessage::user_text("hello"),
                    AgentMessage::assistant(
                        [Content::text("hi back")],
                        Vec::new(),
                        StopReason::EndTurn,
                    ),
                ],
            ))
        },
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::SessionPicker(_))
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("session loads");

    assert_eq!(controller.chrome().session_label(), SESSION_A);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "branch summary: Local branch summary"
    ));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::UserMessage(content) if content == "hello")
    }));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "hi back")
    }));

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("continued prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("continued turn completes");
    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_A));
    assert_eq!(requests[0].model, None);
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::AssistantMessage { content } if content == "continued")
    }));
}

#[tokio::test]
async fn event_loop_keeps_new_session_active_for_followup_prompt() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            captured_requests
                .lock()
                .expect("record request")
                .push(request);
            channels.send_event(AgentEvent::MessageStarted {
                turn: 1,
                id: "assistant-1".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "ok".to_owned(),
            });
            channels.send_event(AgentEvent::MessageFinished {
                turn: 1,
                id: "assistant-1".to_owned(),
                stop_reason: StopReason::EndTurn,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::session(SESSION_NEW))
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("read project");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("first turn completes");

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("followup prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
    assert_eq!(requests[0].session_id, None);
    assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
    assert_eq!(controller.chrome().session_label(), SESSION_NEW);
}

#[tokio::test]
async fn event_loop_keeps_started_session_active_after_failed_turn() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            let request_index = {
                let mut requests = captured_requests.lock().expect("record request");
                requests.push(request);
                requests.len()
            };
            if request_index == 1 {
                channels
                    .session_ids
                    .send(SESSION_NEW.to_owned())
                    .expect("session id sent");
                channels.send_event(AgentEvent::TextDelta {
                    turn: 1,
                    text: "started".to_owned(),
                });
                anyhow::bail!("provider stream error after tool execution");
            }
            channels.send_event(AgentEvent::MessageStarted {
                turn: 2,
                id: "assistant-2".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 2,
                text: "continued".to_owned(),
            });
            channels.send_event(AgentEvent::MessageFinished {
                turn: 2,
                id: "assistant-2".to_owned(),
                stop_reason: StopReason::EndTurn,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 2,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::session(SESSION_NEW))
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("read project");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("first prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("failed first turn is drained");

    assert_eq!(controller.chrome().session_label(), SESSION_NEW);
    assert!(
        controller
            .render_snapshot()
            .contains("provider stream error")
    );

    controller.type_text("continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("followup prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("followup turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].prompt, vec![Content::text("read project")]);
    assert_eq!(requests[0].session_id, None);
    assert_eq!(requests[1].prompt, vec![Content::text("continue")]);
    assert_eq!(requests[1].session_id.as_deref(), Some(SESSION_NEW));
}

#[tokio::test]
async fn event_loop_forks_selected_session_and_continues_child_session() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 3,
                        id: "assistant-3".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 3,
                        text: "continued on fork".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 3,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: vec![test_session_summary(
                SESSION_A,
                "Alpha session",
                test_workspace_root(),
                "branch summary",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        |_session_id| async move {
            panic!("fork action should not use the plain session loader");
            #[allow(unreachable_code)]
            Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
        },
        |parent_id| async move {
            assert_eq!(parent_id, SESSION_A);
            Ok(ForkedSessionTranscript::new(
                SESSION_CHILD,
                LoadedSessionTranscript::new(
                    SESSION_CHILD,
                    [],
                    [
                        AgentMessage::user_text("hello"),
                        AgentMessage::assistant(
                            [Content::text("hi back")],
                            Vec::new(),
                            StopReason::EndTurn,
                        ),
                    ],
                ),
            ))
        },
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionFork))
        .await
        .expect("session fork loads child transcript");

    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        &format!("fork from session {SESSION_A}")
    ));
    assert!(transcript_has_status(
        &controller,
        &format!("switch to fork session {SESSION_CHILD}")
    ));
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(entry, TranscriptEntry::UserMessage(content) if content == "hello")
    }));

    controller.type_text("continue fork");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("continued prompt submits on fork");
    controller
        .wait_for_active_turn()
        .await
        .expect("continued fork turn completes");
    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text("continue fork")]);
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_CHILD));
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn event_loop_opens_model_picker_and_submits_with_selected_model() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "anthropic/claude-sonnet-4-5",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "model switched".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: vec![
                PickerItem::new("openai/gpt-4.1", "openai/gpt-4.1", Some("Responses")),
                PickerItem::new(
                    "anthropic/claude-sonnet-4-5",
                    "anthropic/claude-sonnet-4-5",
                    Some("Messages · ctx 200000"),
                ),
            ],
        },
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.local_config = Some(test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([
            (
                "openai/gpt-4.1".to_owned(),
                ModelConfig {
                    provider: "openai".to_owned(),
                    model: "gpt-4.1".to_owned(),
                    display_name: Some("Responses".into()),
                    ..ModelConfig::default()
                },
            ),
            (
                "anthropic/claude-sonnet-4-5".to_owned(),
                ModelConfig {
                    provider: "anthropic".to_owned(),
                    model: "claude-sonnet-4-5".to_owned(),
                    display_name: Some("Messages · ctx 200000".into()),
                    max_context_tokens: Some(200_000),
                    ..ModelConfig::default()
                },
            ),
        ]),
    ));
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::ModelPickerOpen))
        .await
        .expect("model picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("model selection applies");

    assert_eq!(
        controller.chrome().model_label(),
        "anthropic/claude-sonnet-4-5"
    );
    controller.type_text("use selected model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with selected model");
    controller
        .wait_for_active_turn()
        .await
        .expect("selected model turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    let selected = requests[0].model.as_ref().expect("selected model");
    assert_eq!(selected.provider, "anthropic");
    assert_eq!(selected.model, "claude-sonnet-4-5");
    assert_eq!(selected.max_context_tokens, Some(200_000));
    assert_eq!(requests[0].session_id, None);
}

#[test]
fn model_picker_catalog_for_config_applies_cli_models_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.model_scope = vec!["sonnet".to_owned()];

    let catalog = model_picker_catalog_for_config(&config);

    assert_eq!(catalog.error, None);
    assert!(!catalog.items.is_empty());
    assert!(
        catalog
            .items
            .iter()
            .all(|item| item.value.contains("sonnet"))
    );
    assert!(
        catalog
            .items
            .iter()
            .all(|item| !item.value.contains("openai/gpt-4.1"))
    );
}

#[test]
fn controller_for_config_exposes_default_model_context_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

    let controller = controller_for_config(&config);

    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_047_576))
    );
}

#[test]
fn controller_for_config_loads_builtin_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));

    let controller = controller_for_config(&config);

    let skill_store = controller
        .skill_store
        .as_ref()
        .expect("skill store should load");
    assert!(
        skill_store.get("sub-skill").is_some(),
        "builtin sub-skill skill should be loaded"
    );
    assert!(
        skill_store.get("self-evo").is_some(),
        "builtin self-evo skill should be loaded"
    );
}

#[test]
fn model_picker_items_include_parseable_context_window() {
    let item = model_to_picker_item(&neo_ai::ModelSpec {
        provider: neo_ai::ProviderId("test".to_owned()),
        model: "huge".to_owned(),
        api: neo_ai::ApiKind::OpenAiResponse,
        capabilities: neo_ai::ModelCapabilities::tool_chat().with_max_context_tokens(128_000),
    });

    assert!(
        item.description
            .as_deref()
            .is_some_and(|text| text.contains("ctx 128000"))
    );
    assert_eq!(context_window_from_picker_item(&item), Some(128_000));
}

#[tokio::test]
async fn session_catalog_and_loader_use_real_local_session_store() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    // Compute the workspace-scoped bucket directory that the code will use.
    let bucket_dir = workspace_sessions_dir(&test_config(temp.path(), sessions_dir.clone()));
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let store = SessionMetadataStore::new(&bucket_dir);
    store
        .rename(SESSION_A, "Alpha Session".to_owned())
        .expect("rename session");
    store
        .summarize(SESSION_A, "Local branch summary".to_owned())
        .expect("summarize session");
    let child = store
        .fork(SESSION_A, Some("Parser branch".into()))
        .expect("fork session");
    store
        .record_activity(
            SESSION_A,
            Some(temp.path().display().to_string()),
            Some("hello".into()),
            "100".to_owned(),
        )
        .expect("record session activity");
    store
        .record_activity(
            &child.id,
            Some(temp.path().display().to_string()),
            Some("child prompt".into()),
            "200".to_owned(),
        )
        .expect("record child activity");

    let config = test_config(temp.path(), sessions_dir);
    let catalog = session_catalog_for_config(&config);
    assert_eq!(catalog.error, None);
    assert_eq!(catalog.items.len(), 2);
    assert_eq!(catalog.items[0].id, child.id);
    assert_eq!(catalog.items[0].title.as_deref(), Some("Parser branch"));
    assert!(
        catalog.items[0]
            .last_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("child prompt"))
    );
    assert_eq!(catalog.items[1].id, SESSION_A);
    assert_eq!(catalog.items[1].title.as_deref(), Some("Alpha Session"));
    assert!(
        catalog.items[1]
            .last_prompt
            .as_deref()
            .is_some_and(|prompt| prompt.contains("hello"))
    );

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load session transcript");
    assert_eq!(loaded.label, SESSION_A);
    assert_eq!(
        loaded.notices,
        vec!["branch summary: Local branch summary".to_owned()]
    );
    assert_eq!(loaded.messages.len(), 2);
    assert!(matches!(
        &loaded.messages[0],
        AgentMessage::User { content, .. } if content[0].as_text() == Some("hello")
    ));
    assert!(matches!(
        &loaded.messages[1],
        AgentMessage::Assistant { content, .. } if content[0].as_text() == Some("hi back")
    ));
}

#[tokio::test]
async fn fork_session_transcript_copies_jsonl_metadata_and_loads_child() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir.clone());
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    write_main_wire(
        &bucket_dir,
        SESSION_A,
        concat!(
            "{\"MessageAppended\":{\"message\":{\"User\":{\"content\":[{\"Text\":{\"text\":\"hello\"}}]}}}}\n",
            "{\"MessageAppended\":{\"message\":{\"Assistant\":{\"content\":[{\"Text\":{\"text\":\"hi back\"}}],\"tool_calls\":[],\"stop_reason\":\"EndTurn\"}}}}\n"
        ),
    );

    let forked = fork_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("fork session");

    assert!(forked.session_id.starts_with("session_"));
    assert_eq!(forked.transcript.label, forked.session_id);
    assert!(
        forked.transcript.notices.is_empty(),
        "fork notices are pushed by the controller, not by fork_session_transcript"
    );
    assert_eq!(forked.transcript.messages.len(), 2);
    assert!(
        neo_agent_core::session::main_agent_wire_path(&bucket_dir.join(&forked.session_id))
            .is_file()
    );

    let sessions = SessionMetadataStore::new(&bucket_dir)
        .list()
        .expect("list sessions");
    let parent = sessions
        .iter()
        .find(|session| session.id == SESSION_A)
        .expect("parent listed");
    assert!(parent.children.contains(&forked.session_id));
    let child = sessions
        .iter()
        .find(|session| session.id == forked.session_id)
        .expect("child listed");
    assert_eq!(child.parent_id.as_deref(), Some(SESSION_A));
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn session_picker_ctrl_a_toggles_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");
    let neo_home = sessions_dir.parent().expect("neo home");

    let project_a = temp.path().join("project_a");
    fs::create_dir_all(&project_a).expect("create project_a");
    let config_a = test_config(&project_a, sessions_dir.clone());
    let bucket_a = workspace_sessions_dir(&config_a);
    fs::create_dir_all(&bucket_a).expect("create bucket_a");
    write_main_wire(
        &bucket_a,
        SESSION_A,
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
    );
    let store_a = SessionMetadataStore::new(&bucket_a);
    store_a
        .record_activity(
            SESSION_A,
            Some(project_a.display().to_string()),
            Some("alpha prompt".into()),
            "200".to_owned(),
        )
        .expect("record alpha");

    let project_b = temp.path().join("project_b");
    fs::create_dir_all(&project_b).expect("create project_b");
    let config_b = test_config(&project_b, sessions_dir.clone());
    let bucket_b = workspace_sessions_dir(&config_b);
    fs::create_dir_all(&bucket_b).expect("create bucket_b");
    write_main_wire(
        &bucket_b,
        SESSION_B,
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
    );
    let store_b = SessionMetadataStore::new(&bucket_b);
    store_b
        .record_activity(
            SESSION_B,
            Some(project_b.display().to_string()),
            Some("beta prompt".into()),
            "100".to_owned(),
        )
        .expect("record beta");

    let index = neo_agent_core::session::SessionIndex::new(neo_home);
    index
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: SESSION_A.to_owned(),
            session_dir: bucket_a.clone(),
            workdir: project_a.clone(),
        })
        .expect("index alpha");
    index
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: SESSION_B.to_owned(),
            session_dir: bucket_b.clone(),
            workdir: project_b.clone(),
        })
        .expect("index beta");

    let mut controller = controller_for_config(&config_a);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    let overlay = controller.chrome().focused_overlay().expect("picker open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope on open"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "workspace scope should show alpha: {snapshot}"
    );
    assert!(
        !snapshot.to_lowercase().contains("beta"),
        "workspace scope should not show beta: {snapshot}"
    );

    controller
        .handle_input_event(InputEvent::Action(
            KeybindingAction::SessionPickerToggleScope,
        ))
        .await
        .expect("scope toggles");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("picker still open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::All
        ),
        "all scope after toggle"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "all scope should show alpha: {snapshot}"
    );
    assert!(
        snapshot.to_lowercase().contains("beta"),
        "all scope should show beta: {snapshot}"
    );
}

#[tokio::test]
async fn session_picker_cross_cwd_shows_resume_command() {
    let other_dir = tempfile::tempdir().expect("tempdir");
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs {
            session_items: vec![SessionSummary {
                id: SESSION_A.to_owned(),
                title: Some("Alpha session".into()),
                last_prompt: Some("hello".into()),
                work_dir: other_dir.path().to_path_buf(),
                updated_at: String::new(),
                metadata: None,
            }],
            session_error: None,
            model_items: Vec::new(),
        },
        |_session_id| async move {
            panic!("load_session should not be called for a cross-cwd session");
            #[allow(unreachable_code)]
            Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
        },
    );
    controller.set_clipboard_writer(Arc::new(|_text| Ok(())));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SessionPickerOpen))
        .await
        .expect("session picker opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("select cross-cwd session");

    let expected = format!(
        "cd '{}' && neo --resume '{SESSION_A}'",
        other_dir.path().display(),
    );
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(&controller, &expected));
}

#[tokio::test]
async fn slash_ask_sets_ask_permission_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/ask");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash command handled");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert!(transcript_has_status(&controller, "Permission Mode: ask"));
    assert!(controller.render_snapshot().contains("[ask]"));
}

#[tokio::test]
async fn slash_auto_sets_auto_permission_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/auto");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash command handled");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(controller.render_snapshot().contains("[auto]"));
}

#[tokio::test]
async fn slash_yolo_sets_yolo_permission_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/yolo");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash command handled");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert!(transcript_has_status(&controller, "Permission Mode: yolo"));
    assert!(controller.render_snapshot().contains("[yolo]"));
}

#[tokio::test]
async fn permissions_picker_selects_auto_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/permissions");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("opens permission picker");
    assert!(controller.chrome().focused_overlay().is_some());

    // Move from Ask (index 0) to Auto (index 1) and confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move selection");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm selection");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(controller.chrome().focused_overlay().is_none());
}

#[test]
fn slash_completions_include_permission_commands() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("completions resolve");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert!(
        values.contains(&"/permissions"),
        "missing /permissions: {values:?}"
    );
    assert!(values.contains(&"/ask"), "missing /ask: {values:?}");
    assert!(values.contains(&"/auto"), "missing /auto: {values:?}");
    assert!(values.contains(&"/yolo"), "missing /yolo: {values:?}");
}

#[test]
fn slash_completions_include_compact_command() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("completions resolve");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/compact"), "missing /compact: {values:?}");
}

#[tokio::test]
async fn slash_plan_toggles_plan_mode_and_footer() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("/plan");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("toggles plan mode on");
    assert!(controller.chrome().is_plan_mode());
    assert!(transcript_has_status(&controller, "Plan Mode On"));
    assert!(controller.render_snapshot().contains("[plan]"));
    assert!(!controller.render_snapshot().contains("[PLAN MODE]"));

    controller.type_text("/plan");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("toggles plan mode off");
    assert!(!controller.chrome().is_plan_mode());
    assert!(transcript_has_status(&controller, "Plan Mode Off"));
}

#[tokio::test]
async fn shift_tab_cycles_development_mode_without_changing_permission_mode() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Normal
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to plan");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Plan
    );
    assert!(transcript_has_status(&controller, "Plan Mode On"));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to goal");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Goal(GoalModeStatus::Pending)
    );
    assert!(transcript_has_status(&controller, "Goal Mode On"));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to normal");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Normal
    );
    assert!(transcript_has_status(&controller, "Goal Mode Off"));
}

#[tokio::test]
async fn shift_tab_key_uses_development_cycle() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("shift+tab").expect("valid key")))
        .await
        .expect("shift tab cycles");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(
        controller.chrome().development_mode(),
        DevelopmentMode::Plan
    );
}

#[tokio::test]
async fn slash_plan_turn_request_uses_runtime_plan_mode() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                let active = request
                    .plan_mode
                    .read()
                    .expect("plan mode lock")
                    .is_active();
                captured_requests.lock().expect("lock").push(active);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/plan on");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("plan on");
    controller.type_text("plan this");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert_eq!(*requests.lock().expect("lock"), vec![true]);
}

#[tokio::test]
async fn goal_development_mode_sets_turn_authoring_flag() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("lock")
                    .push(request.goal_mode_authoring);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to plan");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CycleDevelopmentMode))
        .await
        .expect("cycle to goal");
    controller.type_text("draft a goal");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit goal-mode turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert_eq!(*requests.lock().expect("lock"), vec![true]);
}

#[tokio::test]
async fn slash_goal_starts_goal_and_submits_objective_as_first_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        temp.path(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests.lock().expect("lock").push(request.prompt);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("/goal fix checkout tests");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit goal command");
    controller
        .wait_for_active_turn()
        .await
        .expect("goal turn completes");

    let statuses = transcript_entries(&controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Status { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        *requests.lock().expect("lock"),
        vec![vec![Content::text("fix checkout tests")]],
        "statuses: {statuses:?}"
    );
    assert!(transcript_has_status(
        &controller,
        "Goal started: fix checkout tests"
    ));
}

#[tokio::test]
async fn revise_exit_plan_mode_feedback_is_forwarded_with_current_approval() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "exit-plan-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::PlanTransition,
        subject: "Exit plan mode".to_owned(),
        arguments: serde_json::json!({}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, decision_rx) = oneshot::channel();
    let (feedback_tx, feedback_rx) = oneshot::channel();
    controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
        id: "exit-plan-1".to_owned(),
        decision_tx,
        feedback_tx: Some(feedback_tx),
        selected_label_tx: None,
        session_option_label: None,
        prefix_option_label: None,
    });

    // Select "Revise" (index 2) and enter feedback, then confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to revise");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to revise");
    // First confirm enters feedback collection mode.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("begin feedback collection");
    controller
        .handle_input_event(InputEvent::Insert('r'))
        .await
        .expect("type feedback");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm revise");

    assert!(transcript_has_status(&controller, "Revision feedback: r"));
    assert_eq!(
        decision_rx.await.expect("decision"),
        PermissionApprovalDecision::Reject
    );
    assert_eq!(feedback_rx.await.expect("feedback"), Some("r".into()));
}

#[tokio::test]
async fn approve_for_session_does_not_globally_skip_later_ask_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Shell,
        subject: "printf one".to_owned(),
        arguments: serde_json::json!({"command": "printf one"}),
        session_scope: Some(neo_agent_core::SessionApprovalScope {
            keys: vec![neo_agent_core::SessionApprovalKey::Shell {
                workspace: test_workspace_root().display().to_string(),
                cwd: test_workspace_root().display().to_string(),
                command: vec!["printf".to_owned(), "one".to_owned()],
            }],
            label: "Approve this exact command for this session".to_owned(),
            detail: test_workspace_root().display().to_string(),
        }),
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (first_tx, first_rx) = oneshot::channel();
    controller.pending_approvals.insert(
        "tool-1".to_owned(),
        PendingApprovalResponse {
            decision_tx: first_tx,
            feedback_tx: None,
            selected_label_tx: None,
            session_option_label: Some("Approve this exact command for this session".into()),
            prefix_option_label: None,
        },
    );

    // Select "Approve for this session" (index 1) and confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to always-approve");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm always-approve");

    assert_eq!(
        first_rx.await.expect("first decision"),
        PermissionApprovalDecision::AllowForSession
    );

    // Tool-session approval is scoped by the runtime. The TUI must not
    // turn one approval into a global bypass for later ask prompts.
    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-2".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "later.txt"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (second_tx, mut second_rx) = oneshot::channel();
    controller.register_pending_approval(crate::modes::run::PromptApprovalRequest {
        id: "tool-2".to_owned(),
        decision_tx: second_tx,
        feedback_tx: None,
        selected_label_tx: None,
        session_option_label: None,
        prefix_option_label: None,
    });
    assert!(
        second_rx.try_recv().is_err(),
        "later approval requests should remain pending in the TUI"
    );
    assert!(controller.pending_approvals.contains_key("tool-2"));
}

#[test]
fn composed_frame_lines_do_not_exceed_content_width() {
    let app = NeoChromeState::new("neo", "s", "openai/gpt-4.1", "/tmp");
    let mut transcript = TranscriptPane::new(80, 12);
    transcript.push_welcome_banner("neo", "s", "m", "~Workspace/neo", "0.1.0", None);
    let lines = compose_tui_frame(&app, &mut transcript, 80, 12).expect("frame composes");
    let expected = 80usize;
    for (i, line) in lines.iter().enumerate() {
        let w = neo_tui::primitive::visible_width(line);
        assert!(
            w < expected,
            "line {i} reaches terminal autowrap column {expected}: {w}: {line:?}"
        );
    }
}

fn test_config(project_dir: &Path, sessions_dir: PathBuf) -> AppConfig {
    AppConfig {
        default_model: "gpt-4.1".to_owned(),
        default_provider: "openai".to_owned(),
        api_key_env: None,
        providers: BTreeMap::new(),
        models: BTreeMap::new(),
        model_scope: Vec::new(),
        sessions_dir,
        permission_mode: PermissionMode::default(),
        live_permission_mode: Arc::new(RwLock::new(PermissionMode::default())),
        defaults: Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        mcp: McpConfig::default(),
        prompt_templates: Vec::new(),
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: project_dir.to_path_buf(),
        config_path: project_dir.join(".neo/config.toml"),
    }
}

fn test_config_with_models(
    project_dir: &Path,
    sessions_dir: PathBuf,
    models: BTreeMap<String, ModelConfig>,
) -> AppConfig {
    let mut config = test_config(project_dir, sessions_dir);
    config.models = models;
    config
}

/// Regression: the turn driver must receive the controller's *live*
/// `local_config` (via `TurnRequest.base_config`), not the stale snapshot
/// captured at construction. Without this, a provider added at runtime via
/// `/provider` is written to disk but the next turn fails with
/// "unknown model" because the stale registry is used.
#[tokio::test]
async fn turn_request_carries_live_local_config() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_config = std::sync::Arc::clone(&captured);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_config = std::sync::Arc::clone(&captured_config);
            async move {
                *captured_config.lock().expect("capture config") = request.base_config;
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "m".to_owned(),
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: neo_agent_core::StopReason::EndTurn,
                    },
                ])
            }
        },
    );

    // Simulate a runtime config change (e.g. provider added via `/provider`)
    // by setting local_config AFTER the controller was built.
    let live_config = test_config_with_models(
        &test_workspace_root(),
        test_workspace_root().join(".neo/sessions"),
        BTreeMap::from([(
            "minimax-cn-coding-plan/MiniMax-M3".to_owned(),
            ModelConfig {
                provider: "minimax-cn-coding-plan".to_owned(),
                model: "MiniMax-M3".to_owned(),
                ..ModelConfig::default()
            },
        )]),
    );
    controller.local_config = Some(live_config);

    controller.type_text("hello");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let captured = captured.lock().expect("captured").take();
    let config = captured.expect("base_config was forwarded to the driver");
    assert_eq!(config.default_provider, "openai");
    assert!(
        config
            .models
            .contains_key("minimax-cn-coding-plan/MiniMax-M3")
    );
}

fn controller_with_session_for_new_tests() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_requests = std::sync::Arc::clone(&captured_requests);
            async move {
                captured_requests
                    .lock()
                    .expect("record request")
                    .push(request);
                Ok(vec![
                    AgentEvent::MessageStarted {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                    },
                    AgentEvent::TextDelta {
                        turn: 1,
                        text: "hi back".to_owned(),
                    },
                    AgentEvent::MessageFinished {
                        turn: 1,
                        id: "assistant-1".to_owned(),
                        stop_reason: StopReason::EndTurn,
                    },
                    AgentEvent::TurnFinished {
                        turn: 1,
                        stop_reason: StopReason::EndTurn,
                    },
                ])
            }
        },
    );
    // Seed an active session id, transcript content, prompt text, and todos
    // so the reset tests can prove all of them are cleared.
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());
    controller
        .transcript_mut()
        .push_user_message("continue the permission refactor");
    controller
        .transcript_mut()
        .push_assistant_message("I found the old policy conversion path...");
    controller
        .tui
        .chrome_mut()
        .set_todo_items(vec![neo_tui::widgets::TodoDisplayItem::new(
            "Step 1",
            neo_tui::widgets::TodoDisplayStatus::Pending,
        )]);
    (controller, requests)
}

#[tokio::test]
async fn slash_new_resets_to_unsaved_fresh_session_without_streaming() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.contains("Welcome to neo!"),
        "snapshot shows welcome banner"
    );
    assert!(
        snapshot.contains("Started fresh session"),
        "snapshot shows fresh session status"
    );
    assert!(
        !snapshot.contains("permission refactor"),
        "old transcript content is gone"
    );
    assert!(
        !snapshot.contains("policy conversion"),
        "old assistant content is gone"
    );
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(controller.chrome().todo_items().is_empty());
}

#[tokio::test]
async fn slash_clear_alias_resets_to_unsaved_fresh_session() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();

    controller.type_text("/clear");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/clear submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Started fresh session"));
    assert!(!snapshot.contains("permission refactor"));
}

#[tokio::test]
async fn slash_new_does_not_enter_streaming_mode() {
    let (mut controller, requests) = controller_with_session_for_new_tests();

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.chrome().mode(), ChromeMode::Editing);
    assert!(requests.lock().expect("recorded requests").is_empty());
}

#[tokio::test]
async fn slash_new_preserves_model_permission_thinking_and_plan_mode() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    // Configure preserved state.
    controller.set_permission_mode(PermissionMode::Yolo);
    controller.current_thinking = true;
    controller.tui.chrome_mut().set_thinking_enabled(true);
    controller.set_plan_mode_from_user(true);

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert!(controller.chrome().thinking_enabled());
    assert_eq!(controller.chrome().model_label(), "openai/gpt-4.1");
    assert!(
        controller.chrome().is_plan_mode(),
        "user-enabled plan mode is preserved across /new"
    );
}

#[tokio::test]
async fn slash_new_clears_transcript_todos_prompt_and_pending_overlays() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Welcome to neo!"));
    assert!(
        !snapshot.contains("permission refactor"),
        "old transcript content is cleared"
    );
    assert!(controller.chrome().prompt().text.is_empty());
    assert!(controller.chrome().todo_items().is_empty());
    assert!(controller.active_session_id().is_none());
}

#[tokio::test]
async fn slash_new_preserves_loaded_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(Some(SESSION_A), "remembered prompt").unwrap();
    let mut controller = controller_with_history_store(store);
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls history after /new");
    assert_eq!(controller.chrome().prompt().text, "remembered prompt");
}

#[tokio::test]
async fn slash_new_is_blocked_while_turn_is_running_and_preserves_prompt() {
    // Use a driver that blocks forever until cancelled, so the turn stays
    // active while we submit /new.
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, _channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            captured_requests
                .lock()
                .expect("record request")
                .push(request);
            // Never complete: holds the turn open.
            std::future::pending::<Result<TurnOutcome>>().await
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("long running");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt submits");
    // Let the turn task spawn and register itself.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(controller.active_turn.is_some(), "turn is running");

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submit handles blocking");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_A),
        "active session id is unchanged when blocked"
    );
    assert!(
        transcript_has_status(
            &controller,
            "Cannot start a new session while a turn is running"
        ),
        "blocked status is shown"
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "/new",
        "blocked /new preserves the command text for retry"
    );

    // Clean up the dangling turn.
    controller.cancel_active_turn().await.expect("cancel turn");
}

async fn running_turn_controller() -> InteractiveController {
    let run_turn: TurnDriver = Arc::new(move |_request, _channels| {
        Box::pin(async move {
            // Never complete: holds the turn open for live-slash tests.
            std::future::pending::<Result<TurnOutcome>>().await
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller.type_text("long running");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt submits");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(controller.active_turn.is_some(), "turn is running");
    controller
}

#[tokio::test]
async fn slash_auto_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.type_text("/auto");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    assert!(transcript_has_status(&controller, "Permission Mode: auto"));
    assert!(
        !transcript_has_status(&controller, "A turn is already running"),
        "live slash must not be blocked by the active-turn guard"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_ask_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;
    // Flip to Auto first so /ask is a real change.
    controller.type_text("/auto");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    controller.type_text("/ask");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert!(transcript_has_status(&controller, "Permission Mode: ask"));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_yolo_updates_permission_mode_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.type_text("/yolo");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert!(transcript_has_status(&controller, "Permission Mode: yolo"));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_permissions_degrades_to_hint_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.type_text("/permissions");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("slash handled");

    assert!(controller.active_turn.is_some(), "turn should keep running");
    // The picker must NOT open during an active turn to avoid racing with
    // approval/question overlays from the running turn.
    assert!(
        controller.chrome().focused_overlay().is_none(),
        "picker overlay must not open during an active turn"
    );
    assert!(transcript_has_status(
        &controller,
        "Use /ask, /auto, or /yolo while a turn is running"
    ));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn slash_new_preserves_old_session_for_resume_picker_and_next_prompt_creates_new_session() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let captured_requests = std::sync::Arc::clone(&captured_requests);
        Box::pin(async move {
            let is_first = {
                let mut requests = captured_requests.lock().expect("record request");
                let is_first = requests.is_empty();
                requests.push(request);
                is_first
            };
            if is_first {
                // First prompt after /new should carry session_id = None and
                // report a brand-new session id.
                channels
                    .session_ids
                    .send(SESSION_NEW.to_owned())
                    .expect("session id sent");
            }
            channels.send_event(AgentEvent::MessageStarted {
                turn: 1,
                id: "assistant-1".to_owned(),
            });
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "ok".to_owned(),
            });
            channels.send_event(AgentEvent::MessageFinished {
                turn: 1,
                id: "assistant-1".to_owned(),
                stop_reason: StopReason::EndTurn,
            });
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs {
            session_items: vec![test_session_summary(
                SESSION_A,
                "Alpha",
                test_workspace_root(),
                "permission refactor",
            )],
            session_error: None,
            model_items: Vec::new(),
        },
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    // The old session is still advertised in the picker catalog.
    assert!(
        controller
            .session_items
            .iter()
            .any(|item| item.id == SESSION_A),
        "old session remains in the picker catalog"
    );

    // The next real prompt should carry session_id = None so the runtime
    // creates a brand-new JSONL session.
    controller.type_text("hello new session");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("next prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("next turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("hello new session")],
        "next prompt text is forwarded"
    );
    assert_eq!(
        requests[0].session_id, None,
        "next prompt carries no session id so a new session is created"
    );
    assert_eq!(
        controller.chrome().session_label(),
        SESSION_NEW,
        "new session id becomes active"
    );
    assert_eq!(controller.active_session_id(), Some(SESSION_NEW));
}

#[test]
fn slash_completions_include_new_and_clear() {
    let items = session_completion_items(None);
    let values: Vec<&str> = items.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/new"), "completions include /new");
    assert!(values.contains(&"/clear"), "completions include /clear");
}

#[test]
fn configured_model_picker_preserves_unqualified_alias() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config_with_models(
        temp.path(),
        temp.path().join(".neo/sessions"),
        BTreeMap::from([(
            "fast".to_owned(),
            ModelConfig {
                provider: "openai".to_owned(),
                model: "gpt-4.1".to_owned(),
                max_context_tokens: Some(1_000_000),
                ..ModelConfig::default()
            },
        )]),
    );

    let items = model_picker_items_from_config(&config);
    assert_eq!(items[0].value, "fast");
    let selected =
        SelectedModel::from_alias("fast", Some(&config), &items).expect("alias resolves");
    assert_eq!(selected.alias, "fast");
    assert_eq!(selected.provider, "openai");
    assert_eq!(selected.model, "gpt-4.1");
    assert_eq!(selected.max_context_tokens, Some(1_000_000));
}

#[tokio::test]
async fn command_palette_new_session_resets_to_fresh_session() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller
        .tui
        .chrome_mut()
        .set_session_label(SESSION_A.to_owned());
    controller
        .transcript_mut()
        .push_user_message("old session content");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..64 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "session.new" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to next command");
    }
    assert_eq!(
        controller
            .chrome()
            .selected_command()
            .expect("new session command")
            .id,
        "session.new"
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("new session command runs");

    assert_eq!(controller.active_session_id(), None);
    assert_eq!(controller.chrome().session_label(), "new");
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("Started fresh session"));
    assert!(!snapshot.contains("old session content"));
}

// --- NEO-23: cross-session prompt history -----------------------------

/// Build a test controller with a temp-backed prompt history store so tests
/// exercise the real load/append path without touching the user's home.
fn controller_with_history_store(
    store: crate::prompt::history::PromptHistoryStore,
) -> InteractiveController {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_prompt_history_store(store);
    controller.load_prompt_history();
    controller
}

#[tokio::test]
async fn controller_loads_workspace_prompt_history_on_startup() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store
        .append(Some("prior-session"), "earlier prompt")
        .expect("seed earlier");
    store
        .append(Some("prior-session"), "latest prompt")
        .expect("seed latest");

    let mut controller = controller_with_history_store(store);

    // Empty composer: first Up recalls the most recent persisted prompt.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls latest persisted prompt");
    assert_eq!(controller.chrome().prompt().text, "latest prompt");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls older persisted prompt");
    assert_eq!(controller.chrome().prompt().text, "earlier prompt");
}

#[tokio::test]
async fn submitted_prompt_is_persisted_to_workspace_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    controller.type_text("real prompt from this session");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let persisted = std::fs::read_to_string(&path).expect("history file exists");
    assert!(
        persisted.contains("real prompt from this session"),
        "prompt should be persisted: {persisted}"
    );

    // A fresh controller on the same workspace bucket recalls it.
    let store2 = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    let controller2 = controller_with_history_store(store2);
    assert_eq!(
        controller2
            .chrome()
            .prompt()
            .history_snapshot()
            .last()
            .map(String::as_str),
        Some("real prompt from this session")
    );
    drop(dir);
}

#[tokio::test]
async fn slash_commands_are_not_persisted_to_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    // `/model` opens the model picker overlay and never becomes a user
    // turn, so it must not be written to prompt history.
    controller.type_text("/model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");

    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        !persisted.contains("/model"),
        "slash commands must not be persisted: {persisted}"
    );
    drop(dir);
}

#[tokio::test]
async fn slash_mcp_opens_mcp_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("/mcp should open an overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::McpManager(_)),
        "/mcp should open the MCP manager overlay, got {:?}",
        overlay.kind
    );
}

#[tokio::test]
async fn slash_mcp_renders_mcp_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let mut transcript = controller.tui.transcript().clone();
    let lines =
        compose_tui_frame(controller.chrome(), &mut transcript, 80, 24).expect("frame composes");
    let joined = lines.join("\n");
    assert!(
        joined.contains("MCP Servers"),
        "rendered frame should contain MCP manager title: {joined}"
    );
}

#[tokio::test]
async fn mcp_manager_auth_action_shows_status_on_oauth_failure() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let temp = tempfile::tempdir().expect("temp dir");
    let project_dir = temp.path().to_path_buf();
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "example".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Http,
        command: None,
        url: Some("https://example.com/mcp".into()),
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        headers: std::collections::BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
    });
    controller.local_config = Some(config);
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open /mcp");
    controller
        .handle_input_event(InputEvent::Insert('O'))
        .await
        .expect("auth key");
    assert!(transcript_has_status(&controller, "OAuth flow failed"));
}

#[tokio::test]
async fn mcp_manager_auth_action_ignored_for_stdio_server() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let temp = tempfile::tempdir().expect("temp dir");
    let project_dir = temp.path().to_path_buf();
    let mut config = test_config(&project_dir, project_dir.join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "fs".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Stdio,
        command: Some("mcp-server".into()),
        url: None,
        args: Vec::new(),
        env: std::collections::BTreeMap::new(),
        headers: std::collections::BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: None,
        tool_timeout_ms: None,
    });
    controller.local_config = Some(config);
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open /mcp");
    controller
        .handle_input_event(InputEvent::Insert('O'))
        .await
        .expect("auth key");
    assert!(!transcript_has_status(
        &controller,
        "No OAuth provider configured"
    ));
}

#[tokio::test]
async fn mcp_add_transport_opens_form() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open the MCP manager.
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be focused"
    );

    // Press 'A' to add a server.
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("add key handled");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::ChoicePicker(_))
        ),
        "transport choice picker should be focused"
    );

    // Press Enter to select the first transport (real TUI sends Key("enter")).
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("selecting a transport should open the next overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::McpAddForm(_)),
        "expected MCP add form overlay after selecting transport, got {:?}",
        overlay.kind
    );

    // The form must actually be rendered in a single composed frame,
    // and the title should reflect the selected transport so the user
    // knows which transport-specific params are being collected.
    let mut transcript = controller.tui.transcript().clone();
    let lines =
        compose_tui_frame(controller.chrome(), &mut transcript, 80, 24).expect("frame composes");
    let joined = lines.join("\n");
    assert!(
        joined.contains("Add Local stdio MCP Server"),
        "rendered frame should contain contextual form title: {joined}"
    );
    assert!(
        joined.contains("▸ Name:") && joined.contains("Command:"),
        "rendered frame should show Name and Command fields for stdio: {joined}"
    );
}

#[tokio::test]
async fn mcp_add_form_stdio_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select stdio.
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select stdio");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpAddForm(_))
        ),
        "form should be focused"
    );

    // Fill Name, Command, and Env.
    controller
        .handle_input_event(InputEvent::Paste("fs".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to command");
    controller
        .handle_input_event(InputEvent::Paste(
            "npx -y @server/filesystem /repo".to_owned(),
        ))
        .await
        .expect("type command");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to env");
    controller
        .handle_input_event(InputEvent::Paste("KEY=value".to_owned()))
        .await
        .expect("type env");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    // The MCP manager overlay should be reopened after a successful add.
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be reopened after submit"
    );

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1, "expected one saved MCP server");
    assert_eq!(servers[0].id, "fs");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Stdio);
    assert_eq!(
        servers[0].command,
        Some("npx".into()),
        "command is parsed into program"
    );
    assert_eq!(
        servers[0].args,
        vec![
            "-y".to_owned(),
            "@server/filesystem".to_owned(),
            "/repo".to_owned()
        ]
    );
    assert_eq!(
        servers[0].env.get("KEY"),
        Some(&"value".to_owned()),
        "env key is parsed"
    );
    assert!(servers[0].enabled);
}

#[tokio::test]
async fn mcp_add_form_http_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select HTTP (second item -> one Down + Enter).
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to HTTP");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select http");

    // Fill Name, URL, Bearer Token, and Headers.
    controller
        .handle_input_event(InputEvent::Paste("linear".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to url");
    controller
        .handle_input_event(InputEvent::Paste("https://example.invalid/mcp".to_owned()))
        .await
        .expect("type url");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to token");
    controller
        .handle_input_event(InputEvent::Paste("secret".to_owned()))
        .await
        .expect("type token");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to headers");
    controller
        .handle_input_event(InputEvent::Paste("X-Custom=foo".to_owned()))
        .await
        .expect("type headers");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "linear");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Http);
    assert_eq!(servers[0].url, Some("https://example.invalid/mcp".into()));
    assert_eq!(
        servers[0].headers.get("Authorization"),
        Some(&"Bearer secret".to_owned()),
        "bearer token is prepended as Authorization header"
    );
    assert_eq!(servers[0].headers.get("X-Custom"), Some(&"foo".to_owned()));
}

#[tokio::test]
async fn mcp_add_form_sse_submits_to_config() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    // Open manager, start add, select SSE (third item -> two Down + Enter).
    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to HTTP");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to SSE");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select sse");

    // Fill Name and URL only; leave optional fields empty.
    controller
        .handle_input_event(InputEvent::Paste("events".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to url");
    controller
        .handle_input_event(InputEvent::Paste("https://events.invalid/sse".to_owned()))
        .await
        .expect("type url");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit form");

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read saved config");
    let servers = config.mcp.expect("mcp section").servers;
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0].id, "events");
    assert_eq!(servers[0].transport, crate::config::McpTransport::Sse);
    assert_eq!(servers[0].url, Some("https://events.invalid/sse".into()));
    assert!(servers[0].headers.is_empty());
}

#[tokio::test]
async fn mcp_add_form_cancel_returns_to_manager() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project dir");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async { Ok(vec![]) },
    );
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    controller.type_text("/mcp");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("open manager");
    controller
        .handle_input_event(InputEvent::Insert('A'))
        .await
        .expect("start add");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("select stdio");
    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpAddForm(_))
        ),
        "form should be focused"
    );

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel form");

    assert!(
        matches!(
            controller.chrome().focused_overlay().map(|o| &o.kind),
            Some(OverlayKind::McpManager(_))
        ),
        "MCP manager should be reopened after cancel"
    );

    let config = crate::config::read_file_config(&project_dir.join(".neo/config.toml"))
        .expect("read config");
    assert!(
        config.mcp.is_none() || config.mcp.unwrap().servers.is_empty(),
        "no server should be saved on cancel"
    );
}

#[tokio::test]
async fn prompt_history_is_shared_across_sessions_in_same_workspace() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store_a = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    // Session A submits a prompt.
    let mut controller_a = controller_with_history_store(store_a);
    controller_a.type_text("first from session a");
    controller_a
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("session a submits");
    controller_a
        .wait_for_active_turn()
        .await
        .expect("session a turn completes");

    // Session B starts fresh in the same workspace bucket and recalls A's
    // prompt via Up from an empty composer.
    let store_b = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    let mut controller_b = controller_with_history_store(store_b);
    controller_b
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up recalls cross-session prompt");
    assert_eq!(controller_b.chrome().prompt().text, "first from session a");
    drop(dir);
}

#[tokio::test]
async fn prompt_history_is_isolated_by_workspace_bucket() {
    let dir_one = tempfile::tempdir().expect("temp dir one");
    let dir_two = tempfile::tempdir().expect("temp dir two");

    let store_one =
        crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir_one.path()));
    let mut controller_one = controller_with_history_store(store_one);
    controller_one.type_text("workspace one");
    controller_one
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("workspace one submits");
    controller_one
        .wait_for_active_turn()
        .await
        .expect("workspace one turn completes");

    // A different workspace bucket must not recall workspace one's prompt.
    let store_two =
        crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir_two.path()));
    let controller_two = controller_with_history_store(store_two);
    assert!(
        controller_two
            .chrome()
            .prompt()
            .history_snapshot()
            .is_empty(),
        "history must be isolated per workspace bucket"
    );
    drop(dir_one);
    drop(dir_two);
}

#[tokio::test]
async fn approval_up_down_does_not_recall_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(None, "old prompt").expect("seed history");
    let mut controller = controller_with_history_store(store);
    // Composer is empty so any leaked Up would otherwise recall "old prompt".

    controller.apply_turn_event(AgentEvent::ApprovalRequested {
        turn: 1,
        id: "tool-1".to_owned(),
        operation: neo_agent_core::PermissionOperation::Tool,
        subject: "Write".to_owned(),
        arguments: serde_json::json!({"path": "approved.txt"}),
        session_scope: None,
        prefix_rule: None,
        suggestions: Vec::new(),
    });
    let (decision_tx, _decision_rx) = oneshot::channel();
    controller
        .pending_approvals
        .insert("tool-1".to_owned(), pending_approval_response(decision_tx));

    // Up/Down while approval is focused must move the dialog, not history.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up moves approval selection");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves approval selection");

    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "approval Up/Down must not leak into PromptState"
    );
    drop(dir);
}

#[tokio::test]
async fn question_up_down_does_not_recall_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    store.append(None, "old prompt").expect("seed history");
    let mut controller = controller_with_history_store(store);

    let (response_tx, _response_rx) = oneshot::channel();
    controller.register_pending_question(PendingQuestion {
        id: "question-1".to_owned(),
        questions: vec![neo_agent_core::QuestionEventData {
            question: "Pick one".to_owned(),
            header: Some("Single".into()),
            body: None,
            options: vec![
                neo_agent_core::QuestionOptionData {
                    label: "First".to_owned(),
                    description: None,
                },
                neo_agent_core::QuestionOptionData {
                    label: "Second".to_owned(),
                    description: None,
                },
            ],
            multi_select: false,
        }],
        response_tx,
    });

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("up").expect("valid key")))
        .await
        .expect("up moves question selection");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down moves question selection");

    assert_eq!(
        controller.chrome().prompt().text,
        "",
        "question Up/Down must not leak into PromptState"
    );
    drop(dir);
}

#[tokio::test]
async fn active_turn_enter_enqueues_follow_up_instead_of_rejecting() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "working".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    assert!(controller.active_turn.is_some(), "turn should be active");

    // While the turn is running, typing + Enter must enqueue (not reject).
    controller.type_text("queued follow up");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(
        steer_handle.pending(),
        1,
        "follow-up should be pushed into the steer input handle"
    );
    // Composer should be cleared after queuing.
    assert_eq!(controller.chrome().prompt().text, "");
    assert!(
        controller.active_turn.is_some(),
        "turn must still be running after enqueue"
    );
}

#[tokio::test]
async fn active_turn_enter_updates_pending_preview_immediately() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");

    controller.type_text("queued follow up");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued follow up"],
        "queued follow-up should appear above the composer immediately"
    );

    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued follow up"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued follow up"],
        "runtime queue ack must not duplicate the local preview"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty(),
        "one runtime drain should clear one queued preview item"
    );
}

#[tokio::test]
async fn queued_follow_up_message_appended_renders_user_transcript_entry() {
    let mut controller = running_turn_controller().await;

    controller.type_text("queued transcript content");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter while busy enqueues");

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("queued transcript content"),
    });

    assert!(
        transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == "queued transcript content")
        ),
        "queued follow-up should be rendered as a user prompt when it is appended"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn appended_user_prompt_renders_single_transcript_entry() {
    let mut controller = running_turn_controller().await;

    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("long running"),
    });

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(
            |entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == "long running"),
        )
        .count();
    assert_eq!(
        matching_entries, 1,
        "runtime ack for a locally rendered prompt should not duplicate it"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn idle_submit_renders_user_prompt_immediately_without_duplicate_runtime_append() {
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        Arc::new(|_request, channels| {
            Box::pin(async move {
                channels.cancel_token.cancelled().await;
                Ok(TurnOutcome::default())
            })
        }),
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("wait for runtime append");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit starts turn");

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(|entry| {
            matches!(entry, TranscriptEntry::UserMessage(text) if text == "wait for runtime append")
        })
        .count();
    assert_eq!(
        matching_entries, 1,
        "normal submits should render the user prompt immediately"
    );

    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("wait for runtime append"),
    });

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(|entry| {
            matches!(entry, TranscriptEntry::UserMessage(text) if text == "wait for runtime append")
        })
        .count();
    assert_eq!(
        matching_entries, 1,
        "runtime append should render the user prompt exactly once"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn active_turn_ctrl_s_steers_running_turn() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.send_event(AgentEvent::TextDelta {
                turn: 1,
                text: "working".to_owned(),
            });
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    assert!(controller.active_turn.is_some());

    // Ctrl+S while busy should steer the running turn.
    controller.type_text("steer this");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(steer_handle.pending(), 1, "steer should be pushed");
    // Composer cleared after steering.
    assert_eq!(controller.chrome().prompt().text, "");
}

#[tokio::test]
async fn active_turn_ctrl_s_updates_pending_preview_before_transcript_append() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");

    controller.type_text("steer this");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    assert!(
        !transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == "steer this")
        ),
        "Ctrl+S should wait for MessageAppended before rendering the steered user prompt"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "steer should appear above the composer immediately"
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("steer this"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "runtime steer ack must not duplicate the local preview"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::Steering,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("steer this"),
    });
    assert!(
        transcript_entries(&controller).iter().any(
            |entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == "steer this")
        ),
        "steered user prompt should render when the runtime appends it"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn active_turn_ctrl_s_promotes_one_follow_up_per_press_before_current_prompt() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued one"),
    });
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued two"),
    });

    controller.type_text("current steer");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("first ctrl+s promotes oldest queued follow-up");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(steer_handle.pending(), 1);
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "one Ctrl+S should promote only the oldest queued follow-up"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"]
    );
    assert_eq!(
        controller.chrome().prompt().text,
        "current steer",
        "composer text should wait until queued follow-ups have been promoted"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("second ctrl+s promotes second queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"]
    );
    assert_eq!(controller.chrome().prompt().text, "current steer");

    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued D"),
    });
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("third ctrl+s promotes newly queued follow-up before composer");
    assert_eq!(steer_handle.pending(), 3);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two", "queued D"]
    );
    assert_eq!(controller.chrome().prompt().text, "current steer");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("fourth ctrl+s steers current composer text");
    assert_eq!(steer_handle.pending(), 4);
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two", "queued D", "current steer"]
    );
    assert_eq!(controller.chrome().prompt().text, "");

    let steered_user_messages = transcript_entries(&controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage(text)
                if matches!(
                    text.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steered_user_messages,
        Vec::<&str>::new(),
        "promoted steers should not render in the transcript before MessageAppended"
    );

    for text in ["queued one", "queued two", "queued D", "current steer"] {
        controller.apply_turn_event(AgentEvent::MessageAppended {
            message: AgentMessage::user_text(text),
        });
    }
    let steered_user_messages = transcript_entries(&controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage(text)
                if matches!(
                    text.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        steered_user_messages,
        vec!["queued one", "queued two", "queued D", "current steer"],
        "promoted steers should render in runtime append order"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Large scenario test asserts on full steer/follow-up ordering; splitting hurts readability.
async fn empty_ctrl_s_promotes_one_follow_up_per_press_without_local_duplication() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued one"),
    });
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_text("queued two"),
    });

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("empty ctrl+s promotes oldest queued follow-up");

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    assert_eq!(
        steer_handle.pending(),
        1,
        "one Ctrl+S should enqueue one promotion request"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "only the oldest follow-up should leave the visible follow-up queue"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"],
        "promoted follow-up should appear as a pending steer immediately"
    );

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued two"],
        "runtime follow-up drain ack must not affect the next visible queued follow-up"
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("queued one"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one"],
        "runtime steer ack must not duplicate the promoted preview"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("second empty ctrl+s promotes next queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"]
    );

    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::FollowUp,
        count: 1,
    });
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_text("queued two"),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued one", "queued two"],
        "runtime steer acks must not duplicate the promoted previews"
    );
    controller.apply_turn_event(AgentEvent::QueueDrained {
        kind: neo_agent_core::QueueKind::Steering,
        count: 2,
    });
    assert!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .is_empty(),
        "one runtime steer drain should clear the promoted preview"
    );
}

#[tokio::test]
async fn alt_up_dequeues_oldest_follow_up_into_multiline_composer() {
    let captured_steer = Arc::new(std::sync::Mutex::new(
        neo_agent_core::SteerInputHandle::new(),
    ));
    let observed_steer = Arc::clone(&captured_steer);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let observed_steer = Arc::clone(&observed_steer);
        *observed_steer.lock().expect("steer lock") = channels.steer_input.clone();
        Box::pin(async move {
            channels.cancel_token.cancelled().await;
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("first prompt");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("first prompt starts turn");
    for text in ["AAAA", "BBBB", "CCCC"] {
        controller.apply_turn_event(AgentEvent::FollowUpQueued {
            message: AgentMessage::user_text(text),
        });
    }

    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("first alt+up dequeues oldest queued follow-up");
    assert_eq!(steer_handle.pending(), 1);
    assert_eq!(controller.chrome().prompt().text, "AAAA");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["BBBB", "CCCC"]
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("second alt+up appends next queued follow-up");
    assert_eq!(steer_handle.pending(), 2);
    assert_eq!(controller.chrome().prompt().text, "AAAA\nBBBB");
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["CCCC"]
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("alt+up").expect("valid key")))
        .await
        .expect("third alt+up appends final queued follow-up");
    assert_eq!(steer_handle.pending(), 3);
    assert_eq!(controller.chrome().prompt().text, "AAAA\nBBBB\nCCCC");
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
}

#[tokio::test]
async fn empty_ctrl_s_with_no_queue_reports_noop_status() {
    let mut controller = running_turn_controller().await;

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("empty ctrl+s with no queue is handled");

    assert!(
        transcript_has_status(&controller, "No queued follow-up to steer"),
        "empty Ctrl+S with no queue should be visible feedback"
    );

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn idle_ctrl_s_falls_back_to_normal_submit() {
    let prompt_seen = Arc::new(std::sync::Mutex::new(None));
    let observed_prompt = Arc::clone(&prompt_seen);
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let observed_prompt = Arc::clone(&observed_prompt);
        Box::pin(async move {
            *observed_prompt.lock().expect("prompt lock") = Some(request.prompt.clone());
            channels.send_event(AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            });
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
    );

    controller.type_text("submit via ctrl+s");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s submits when idle");
    controller
        .wait_for_active_turn()
        .await
        .expect("idle ctrl+s turn completes");

    let seen = prompt_seen.lock().expect("prompt lock").clone();
    assert_eq!(
        seen,
        Some(vec![Content::text("submit via ctrl+s")]),
        "idle Ctrl+S should behave like a normal submit"
    );
}

fn completed_shell_result(
    stdout: impl Into<String>,
) -> neo_agent_core::tools::ShellExecutionResult {
    neo_agent_core::tools::ShellExecutionResult {
        stdout: stdout.into(),
        stderr: String::new(),
        exit_code: Some(0),
        signal: None,
        stdout_truncated: false,
        stderr_truncated: false,
        truncated: false,
        outcome: neo_agent_core::ShellCommandOutcome::Completed,
        foreground_task_id: None,
    }
}

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
async fn shell_mode_uses_spec_timeouts_for_user_commands() {
    let observed_timeouts = Arc::new(std::sync::Mutex::new(None));
    let captured_timeouts = Arc::clone(&observed_timeouts);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_shell_driver(Arc::new(move |request| {
        let captured_timeouts = Arc::clone(&captured_timeouts);
        Box::pin(async move {
            *captured_timeouts.lock().expect("timeouts lock") =
                Some((request.foreground_timeout, request.background_timeout));
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

    assert_eq!(
        *observed_timeouts.lock().expect("timeouts lock"),
        Some((Duration::from_secs(120), Duration::from_secs(600)))
    );
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
async fn shell_mode_commands_do_not_enter_prompt_history() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(dir.path());
    let mut controller = controller_with_history_store(store.clone());
    controller.set_shell_driver(Arc::new(|request| {
        Box::pin(async move {
            assert_eq!(request.command, "echo hidden");
            Ok(completed_shell_result("hidden\n"))
        })
    }));

    controller.type_text("!");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("enter shell mode");
    controller.type_text("echo hidden");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("run shell command");
    controller
        .wait_for_active_shell_command()
        .await
        .expect("shell completes");

    let history = store.load_recent().expect("history loads");
    assert!(
        history.is_empty(),
        "shell commands should not be written to prompt history"
    );
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
    tokio::time::sleep(Duration::from_millis(50)).await;
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
    tokio::time::sleep(Duration::from_millis(50)).await;
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
async fn slash_tasks_opens_task_browser_with_shared_background_tasks() {
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
    controller.maybe_refresh_task_browser().await;

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
        .handle_input_event(InputEvent::Insert('s'))
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
async fn task_browser_enter_toggles_output_focus_without_stop_confirmation() {
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
        .expect("toggle output focus");

    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .focus(),
        neo_tui::tasks_browser::TaskBrowserFocus::Output
    );
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
    tokio::time::sleep(Duration::from_millis(50)).await;
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
        run_turn,
        PickerCatalogs::default(),
        Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
        Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
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
    controller
        .resolve_trust_dialog_at_startup(
            data,
            ScriptedEvents(VecDeque::from([
                // Default is ContinueUntrusted; move up once to TrustCurrent.
                InputEvent::Action(KeybindingAction::SelectUp),
                InputEvent::Action(KeybindingAction::SelectConfirm),
            ])),
            |_| Ok(()),
        )
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
    controller
        .resolve_trust_dialog_at_startup(
            data,
            ScriptedEvents(VecDeque::from([InputEvent::Action(
                KeybindingAction::SelectConfirm,
            )])),
            |_| Ok(()),
        )
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
    controller
        .resolve_trust_dialog_at_startup(
            data,
            ScriptedEvents(VecDeque::from([InputEvent::Action(
                KeybindingAction::SelectCancel,
            )])),
            |_| Ok(()),
        )
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

fn btw_test_config(project_dir: &std::path::Path) -> crate::config::AppConfig {
    test_config(project_dir, project_dir.join(".neo/sessions"))
}

fn btw_fake_client(answer: &str) -> Arc<dyn neo_ai::ModelClient> {
    use neo_ai::{AiStreamEvent, StopReason};
    Arc::new(neo_ai::providers::fake::FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: answer.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]))
}

fn chat_message_text(message: &neo_ai::ChatMessage) -> String {
    let content = match message {
        neo_ai::ChatMessage::System { content }
        | neo_ai::ChatMessage::User { content }
        | neo_ai::ChatMessage::Assistant { content, .. }
        | neo_ai::ChatMessage::ToolResult { content, .. } => content,
    };
    content
        .iter()
        .filter_map(|part| match part {
            neo_ai::ContentPart::Text { text } => Some(text.as_str()),
            neo_ai::ContentPart::Thinking { .. } | neo_ai::ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

#[tokio::test]
async fn slash_btw_opens_empty_sidecar_panel_without_starting_main_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.handle_slash_command("/btw").await;

    assert!(
        controller.chrome().has_btw_panel(),
        "/btw opens the sidecar panel"
    );
    assert!(
        controller.btw_runner.is_some(),
        "/btw creates a sidecar runner"
    );
    assert!(
        controller.active_turn.is_none(),
        "/btw must not start a main turn"
    );
}

#[tokio::test]
async fn slash_btw_question_starts_in_memory_sidecar_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("42"));

    controller.handle_slash_command("/btw what is 2+2?").await;

    assert!(controller.chrome().has_btw_panel());
    assert!(controller.btw_receiver.is_some());
    assert!(controller.active_turn.is_none());

    // Drain events so the panel state reflects the sidecar answer.
    for _ in 0..10 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].prompt, "what is 2+2?");
    assert_eq!(state.sidecar.turns[0].answer, "42");
}

#[tokio::test]
async fn slash_btw_inherits_main_context_with_single_sidecar_projection() {
    use neo_ai::{AiStreamEvent, StopReason};

    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.active_session_id = Some("session_00000000-0000-0000-0000-000000000001".into());
    // Persist the message to the session JSONL so /btw can inherit it.
    // In production, turn execution writes messages to disk; simulate that here.
    {
        let config = controller.local_config.as_ref().expect("config");
        let wire_path = crate::modes::sessions::session_path(
            "session_00000000-0000-0000-0000-000000000001",
            config,
        )
        .expect("session path");
        fs::create_dir_all(wire_path.parent().expect("wire parent")).expect("mkdir wire parent");
        let event = AgentEvent::MessageAppended {
            message: AgentMessage::user_text("main context in memory"),
        };
        let line = serde_json::to_string(&event).expect("serialize event");
        fs::write(&wire_path, format!("{line}\n")).expect("write wire");
    }
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_text("main context in memory"),
    });
    let fake = neo_ai::providers::fake::FakeModelClient::new(vec![
        AiStreamEvent::MessageStart {
            id: "msg-1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "side".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]);
    controller.set_btw_client(Arc::new(fake.clone()));

    controller
        .handle_slash_command("/btw inspect context")
        .await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let contents: Vec<String> = requests[0].messages.iter().map(chat_message_text).collect();
    assert!(
        contents
            .iter()
            .any(|content| content == "main context in memory"),
        "sidecar should inherit current in-memory main transcript: {contents:?}"
    );
    assert_eq!(
        contents
            .iter()
            .filter(|content| content.contains("side-channel conversation"))
            .count(),
        1,
        "sidecar reminder should be projected exactly once: {contents:?}"
    );
}

#[tokio::test]
async fn bare_slash_btw_while_sidecar_running_keeps_existing_panel() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.handle_slash_command("/btw").await;
    {
        let state = controller
            .tui
            .chrome_mut()
            .btw_panel_state_mut()
            .expect("panel state");
        state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
    }
    let original_id = controller
        .chrome()
        .btw_panel_state()
        .expect("panel state")
        .sidecar
        .id
        .0
        .clone();

    controller.handle_slash_command("/btw").await;

    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.id.0, original_id);
    assert_eq!(
        state.sidecar.phase,
        neo_tui::widgets::btw_panel::BtwPhase::Running
    );
    assert!(state.status_message.as_deref().is_some_and(|message| {
        message.contains("already open") || message.contains("Wait for /btw")
    }));
}

#[tokio::test]
async fn composer_routes_to_sidecar_when_panel_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("answer"));

    controller.handle_slash_command("/btw").await;
    controller.type_text("explain this");
    controller
        .submit_current_prompt()
        .await
        .expect("submit routes to sidecar");

    assert!(controller.active_turn.is_none(), "must not start main turn");
    for _ in 0..10 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].prompt, "explain this");
}

#[tokio::test]
async fn empty_composer_esc_closes_panel() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.handle_slash_command("/btw").await;
    assert!(controller.chrome().has_btw_panel());

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("esc handled");

    assert!(
        !controller.chrome().has_btw_panel(),
        "Esc closes empty panel"
    );
}

#[tokio::test]
async fn sidecar_events_do_not_append_to_main_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("side answer"));

    let entries_before = controller.tui.transcript().transcript().entries().len();
    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }
    let entries_after = controller.tui.transcript().transcript().entries().len();

    assert_eq!(
        entries_before, entries_after,
        "sidecar must not append to main transcript"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns[0].answer, "side answer");
}

#[tokio::test]
async fn slash_btw_while_main_turn_running_does_not_steer_or_queue_main_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("side answer"));

    controller.type_text("main question");
    controller
        .submit_current_prompt()
        .await
        .expect("main turn starts");
    assert!(
        controller.active_turn.is_some(),
        "main turn should be active"
    );

    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    assert!(
        controller.active_turn.is_some(),
        "/btw must not cancel or queue the main turn"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 1);
    assert_eq!(state.sidecar.turns[0].answer, "side answer");
}

#[tokio::test]
async fn escape_closes_btw_without_touching_main_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { std::future::pending::<Result<Vec<AgentEvent>>>().await },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.type_text("main question");
    controller
        .submit_current_prompt()
        .await
        .expect("main turn starts");
    assert!(controller.active_turn.is_some());

    controller.handle_slash_command("/btw").await;
    assert!(controller.chrome().has_btw_panel());

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("esc handled");

    assert!(!controller.chrome().has_btw_panel(), "Esc closes BTW panel");
    assert!(
        controller.active_turn.is_some(),
        "Esc must not cancel the main turn"
    );
}

#[tokio::test]
async fn btw_running_preserves_composer_text_and_shows_busy_notice() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    // Open an empty sidecar panel and mark it Running as if a turn were in
    // progress. This avoids coupling the test to a hanging model client.
    controller.handle_slash_command("/btw").await;
    if let Some(state) = controller.tui.chrome_mut().btw_panel_state_mut() {
        state.sidecar.phase = neo_tui::widgets::btw_panel::BtwPhase::Running;
    }

    controller.type_text("second question");
    controller
        .submit_current_prompt()
        .await
        .expect("busy check handled");

    assert_eq!(
        controller.chrome().prompt().text,
        "second question",
        "composer text must be preserved while sidecar is running"
    );
    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns.len(), 0, "no sidecar turn started");
    assert!(
        state
            .status_message
            .as_deref()
            .expect("busy notice")
            .contains("Wait for /btw to finish"),
        "busy notice should be shown"
    );
}

#[tokio::test]
async fn btw_conversation_is_not_written_to_main_session_jsonl() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let sessions_dir = project_dir.join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let session_id = "session_00000000-0000-4000-8000-000000000901";
    let session_path = main_wire_path_for_session(sessions_dir.join(session_id));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    writer
        .append_event(&AgentEvent::MessageAppended {
            message: AgentMessage::user_text("existing main message"),
        })
        .await
        .expect("append event");
    writer.flush().await.expect("flush");
    drop(writer);

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client("side answer"));
    controller.active_session_id = Some(session_id.to_owned());

    controller.handle_slash_command("/btw side question").await;
    for _ in 0..20 {
        controller.drain_btw_sidecar();
        tokio::task::yield_now().await;
    }

    let state = controller.chrome().btw_panel_state().expect("panel state");
    assert_eq!(state.sidecar.turns[0].answer, "side answer");

    let content = fs::read_to_string(&session_path).expect("read session");
    assert!(
        content.contains("existing main message"),
        "original main event should still be present"
    );
    assert!(
        !content.contains("side question"),
        "side question must not be written to main JSONL"
    );
    assert!(
        !content.contains("side answer"),
        "side answer must not be written to main JSONL"
    );
}

#[tokio::test]
async fn shift_enter_inserts_newline_while_btw_panel_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("project");
    fs::create_dir_all(&project_dir).expect("create project");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &project_dir,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(btw_test_config(&project_dir));
    controller.set_btw_client(btw_fake_client(""));

    controller.handle_slash_command("/btw").await;
    assert!(controller.chrome().has_btw_panel());

    controller.type_text("line1");
    controller
        .handle_input_event(InputEvent::NewLine)
        .await
        .expect("newline handled");
    controller.type_text("line2");

    assert_eq!(controller.chrome().prompt().text, "line1\nline2");
}

#[tokio::test]
async fn slash_fork_forks_current_session_and_enters_child() {
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| async move {
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        |_session_id| async move {
            panic!("fork should not use the load_session callback");
            #[allow(unreachable_code)]
            Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
        },
        |parent_id| async move {
            assert_eq!(parent_id, SESSION_A);
            Ok(ForkedSessionTranscript::new(
                SESSION_CHILD,
                LoadedSessionTranscript::new(
                    SESSION_CHILD,
                    [],
                    [AgentMessage::user_text("hello")],
                ),
            ))
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    let consumed = controller.handle_slash_command("/fork").await;
    assert!(consumed, "/fork should be consumed as a slash command");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_CHILD),
        "active session switched to fork child"
    );
    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(
        transcript_has_status(&controller, &format!("fork from session {SESSION_A}")),
        "transcript shows fork-from notice"
    );
    assert!(
        transcript_has_status(
            &controller,
            &format!("switch to fork session {SESSION_CHILD}")
        ),
        "transcript shows switch-to notice"
    );
}
