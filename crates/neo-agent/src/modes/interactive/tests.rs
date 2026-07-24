use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    rc::Rc,
};

use clap::Parser as _;
use neo_agent_core::{
    AgentEvent, AgentMessage, ApprovalAction, ApprovalCancelReason, ApprovalOption,
    ApprovalPresentation, ApprovalRequest, ApprovalResolution, ApprovalResponse, Content,
    FileWriteApprovalOperation, MessageOrigin, PendingQuestion, PermissionMode,
    PermissionOperation, PrefixApprovalRule, SessionApprovalKey, SessionApprovalScope,
    ShellCommandOrigin, StopReason, ToolResult,
    skills::{
        LoadedSkill, SkillHostMetadata, SkillInterface, SkillManifest, SkillSource, SkillStore,
        SkillToolDependency,
    },
};
use neo_tui::{
    input::{InputEvent, KeyId, KeybindingAction},
    screen_output::InlineTerminal,
    shell::{ChromeMode, CommandPaletteState, CommandSpec, Overlay, OverlayKind},
    transcript::{ApprovalDisplayState, TranscriptEntry, TranscriptPane},
};
use tokio::sync::oneshot;
use tracing_subscriber::prelude::*;

use super::git_status::{
    count_untracked_changes, git_status_label_with_program, parse_git_numstat,
    parse_git_status_porcelain, parse_git_untracked_files_z,
};
use super::snapshot::{compose_tui_frame, render_overlay_snapshot};
use super::*;
use crate::config::{Defaults, McpConfig, ModelConfig, ProviderConfig, RuntimeConfig, TuiConfig};

const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000601";
const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000602";
const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000603";
const SESSION_NEW: &str = "session_00000000-0000-4000-8000-000000000604";

struct OptionalScriptedEvents {
    events: VecDeque<Option<InputEvent>>,
}

impl TerminalEvents for OptionalScriptedEvents {
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
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
            },
            body: "Refactor safely.".to_owned(),
            source: SkillSource::Builtin,
            host_metadata: SkillHostMetadata::default(),
        }],
    )
}

fn skill_store_with_two_prompt_skills() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![
            LoadedSkill {
                name: "skill_one".to_owned(),
                root: test_workspace_root().join("builtin/skill_one"),
                manifest: SkillManifest {
                    name: "skill_one".to_owned(),
                    description: "First skill".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                },
                body: "ONE: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata {
                    interface: None,
                    dependencies: vec![SkillToolDependency {
                        value: "reviewServer".to_owned(),
                        description: Some("Review MCP server".to_owned()),
                    }],
                },
            },
            LoadedSkill {
                name: "skill_two".to_owned(),
                root: test_workspace_root().join("builtin/skill_two"),
                manifest: SkillManifest {
                    name: "skill_two".to_owned(),
                    description: "Second skill".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                },
                body: "TWO: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
        ],
    )
}

fn skill_store_with_interactive_preflight_skills() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![
            LoadedSkill {
                name: "self-evo".to_owned(),
                root: PathBuf::from("builtin/self-evo"),
                manifest: SkillManifest {
                    name: "self-evo".to_owned(),
                    description: "Distill session learning into reusable skills".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: true,
                    arguments: Vec::new(),
                },
                body: "SELF EVO: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
            LoadedSkill {
                name: "create-skill".to_owned(),
                root: PathBuf::from("builtin/create-skill"),
                manifest: SkillManifest {
                    name: "create-skill".to_owned(),
                    description: "Create a reusable skill from instructions".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: true,
                    arguments: Vec::new(),
                },
                body: "CREATE SKILL: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
        ],
    )
}

fn ordinary_approval_options(
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> Vec<ApprovalOption> {
    let mut options = vec![ApprovalOption {
        label: "Approve once".to_owned(),
        description: None,
        action: ApprovalAction::PermitOnce,
    }];
    if let Some(scope) = session_scope.filter(|scope| !scope.is_empty()) {
        options.push(ApprovalOption {
            label: scope.label.clone(),
            description: Some(scope.detail.clone()),
            action: ApprovalAction::PermitForSession { scope },
        });
    }
    if let Some(rule) = prefix_rule {
        options.push(ApprovalOption {
            label: format!("Approve commands starting with {}", rule.label),
            description: None,
            action: ApprovalAction::PermitForPrefix { rule },
        });
    }
    options.push(ApprovalOption {
        label: "Reject".to_owned(),
        description: None,
        action: ApprovalAction::Reject,
    });
    options
}

fn ordinary_tool_request(
    id: &str,
    subject: &str,
    path: &str,
    session_scope: Option<SessionApprovalScope>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Tool,
        presentation: ApprovalPresentation::Tool {
            title: "Run tool?".to_owned(),
            details: vec![format!("tool: {subject}"), format!("path: {path}")],
        },
        options: ordinary_approval_options(session_scope, None),
    }
}

fn ordinary_shell_request(
    id: &str,
    command: &str,
    session_scope: Option<SessionApprovalScope>,
    prefix_rule: Option<PrefixApprovalRule>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: command.to_owned(),
            cwd: None,
        },
        options: ordinary_approval_options(session_scope, prefix_rule),
    }
}

fn background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
    }
}

fn plan_review_request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::PlanTransition,
        presentation: ApprovalPresentation::Plan {
            title: "Plan Review".to_owned(),
            path: None,
            markdown: "Ready to build with this plan?".to_owned(),
            summary: Some("Ready to build with this plan?".to_owned()),
        },
        options: vec![
            ApprovalOption {
                label: "Approve".to_owned(),
                description: None,
                action: ApprovalAction::ApprovePlan { selection: None },
            },
            ApprovalOption {
                label: "Reject with feedback".to_owned(),
                description: None,
                action: ApprovalAction::RevisePlan {
                    preset_feedback: None,
                },
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::RejectPlan,
            },
        ],
    }
}

fn goal_review_request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::GoalTransition,
        presentation: ApprovalPresentation::Goal {
            title: "Start goal?".to_owned(),
            objective: "Ship the feature".to_owned(),
            completion_criterion: Some("Tests pass".to_owned()),
            phases: vec!["Plan".to_owned(), "Implement".to_owned()],
        },
        options: vec![
            ApprovalOption {
                label: "Start goal".to_owned(),
                description: None,
                action: ApprovalAction::StartGoal,
            },
            ApprovalOption {
                label: "Reject with feedback".to_owned(),
                description: None,
                action: ApprovalAction::ReviseGoal {
                    preset_feedback: None,
                },
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::RejectGoal,
            },
        ],
    }
}

fn make_pending_approval(
    request: ApprovalRequest,
) -> (
    crate::modes::run::PendingApproval,
    oneshot::Receiver<ApprovalResponse>,
) {
    let (response_tx, response_rx) = oneshot::channel();
    (
        crate::modes::run::PendingApproval {
            request,
            response_tx,
        },
        response_rx,
    )
}

fn file_write_session_scope(path: &str) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::FileWrite {
            workspace: test_workspace_root().display().to_string(),
            path: test_workspace_root().join(path).display().to_string(),
            operation: FileWriteApprovalOperation::Write,
        }],
        label: "Approve writes to this file for this session".to_owned(),
        detail: path.to_owned(),
    }
}

fn shell_session_scope(command: &[&str]) -> SessionApprovalScope {
    SessionApprovalScope {
        keys: vec![SessionApprovalKey::Shell {
            workspace: test_workspace_root().display().to_string(),
            cwd: test_workspace_root().display().to_string(),
            command: command.iter().map(|part| (*part).to_owned()).collect(),
        }],
        label: "Approve this exact command for this session".to_owned(),
        detail: test_workspace_root().display().to_string(),
    }
}

/// Keep the `_goal` helper referenced so Plan/Goal builders stay compiled even
/// when no current test exercises Goal review.
#[allow(dead_code)]
fn _goal_review_request_for_builders() -> ApprovalRequest {
    goal_review_request("goal-1")
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
    let (layer, rx) = crate::log_capture::capture_channel(8);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::warn!(server_id = "linear", "MCP server unavailable");
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
    let (layer, rx) = crate::log_capture::capture_channel(8);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::error!("critical failure");
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
fn drain_log_events_caps_ticks_and_session_transcript() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (layer, rx) = crate::log_capture::capture_channel(
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION,
    );
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    let entries_before = transcript_entries(&controller).len();
    for batch in 0..=super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION
        / super::log_events::MAX_LOG_EVENTS_PER_TICK
        + 1
    {
        for offset in 0..super::log_events::MAX_LOG_EVENTS_PER_TICK {
            tracing::warn!("captured log {batch}-{offset}");
        }
        controller.drain_log_events();
        if batch == 0 {
            let captured = transcript_entries(&controller)
                .iter()
                .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.starts_with("captured log")))
                .count();
            assert_eq!(captured, super::log_events::MAX_LOG_EVENTS_PER_TICK);
        }
    }

    let entries = transcript_entries(&controller);
    let captured = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text.starts_with("captured log")))
        .count();
    let suppression_notices = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Status { text, .. } if text == "Some captured log events were suppressed for this session"))
        .count();
    assert_eq!(
        captured,
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION
    );
    assert_eq!(suppression_notices, 1);
    assert_eq!(
        entries.len() - entries_before,
        super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION + 1
    );
}

#[test]
fn rebuilding_session_transcript_resets_captured_log_budget() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.captured_log_status_count = super::log_events::MAX_CAPTURED_LOG_STATUSES_PER_SESSION;
    controller.captured_log_suppression_notified = true;

    controller.rebuild_transcript_from_session(&LoadedSessionTranscript::new(
        "new-session",
        Vec::new(),
        Vec::new(),
    ));

    assert_eq!(controller.captured_log_status_count, 0);
    assert!(!controller.captured_log_suppression_notified);
    let (layer, rx) = crate::log_capture::capture_channel(1);
    controller.set_log_event_receiver(rx);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    tracing::warn!("new session warning");
    controller.drain_log_events();
    assert_eq!(controller.captured_log_status_count, 1);
    assert!(transcript_has_status(&controller, "new session warning"));
}

#[test]
fn git_status_badge_formats_dirty_and_sync() {
    let mut badge =
        parse_git_status_porcelain("## main...origin/main [ahead 2, behind 1]\n M src/app.rs\n")
            .expect("git badge");
    (badge.added, badge.deleted) = parse_git_numstat("12\t3\tsrc/app.rs\n");

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
fn git_status_badge_counts_untracked_changes() {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("new.rs"), "first\nsecond\n").expect("write text file");
    fs::write(dir.path().join("image.bin"), b"neo\0image").expect("write binary file");

    let mut badge =
        parse_git_status_porcelain("## feature\n?? new.rs\n?? image.bin\n").expect("git badge");
    let paths = parse_git_untracked_files_z(b"new.rs\0image.bin\0");
    let (added, untracked) = count_untracked_changes(dir.path(), &paths);
    badge.added = added;
    badge.untracked = untracked;

    assert_eq!(badge.format(), "feature [+2 -0 ?1]");
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
fn git_status_badge_resolves_repository_from_nested_workspace() {
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

    assert_eq!(
        git_status_label_with_program("git", &workspace),
        Some("main [init]".to_owned())
    );

    let add_status = Command::new("git")
        .arg("-C")
        .arg(parent.path())
        .args(["add", "nested-workspace/untracked.txt"])
        .status()
        .expect("run git add");
    assert!(add_status.success(), "git add should succeed");
    let commit_status = Command::new("git")
        .arg("-C")
        .arg(parent.path())
        .args([
            "-c",
            "user.name=Neo",
            "-c",
            "user.email=neo@example.invalid",
            "commit",
            "-m",
            "initial",
        ])
        .status()
        .expect("run git commit");
    assert!(commit_status.success(), "git commit should succeed");
    fs::write(workspace.join("untracked.txt"), "new file\nsecond line\n")
        .expect("modify tracked file");

    assert_eq!(
        git_status_label_with_program("git", &workspace),
        Some("main [+1 -0]".to_owned())
    );
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

#[tokio::test]
async fn completed_git_status_is_applied_before_queued_refresh_starts() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(|_| Some("main [second]".to_owned())));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main".to_owned()));
    let completed = tokio::spawn(async { Some("main [first]".to_owned()) });
    while !completed.is_finished() {
        tokio::task::yield_now().await;
    }
    controller.pending_git_status = Some(completed);
    controller.git_status_refresh_queued = true;

    assert!(controller.poll_pending_git_status().await);
    assert_eq!(controller.chrome().git_status_label(), Some("main [first]"));
    assert!(controller.pending_git_status.is_some());
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

#[test]
fn unchanged_git_status_refresh_does_not_report_visible_change() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_git_status_provider(Arc::new(|_| Some("main [+1 -1]".into())));
    controller
        .tui
        .chrome_mut()
        .set_git_status_label(Some("main [+1 -1]".into()));

    assert!(!controller.refresh_git_status_now());

    controller.set_git_status_provider(Arc::new(|_| Some("main [+2 -1]".into())));
    assert!(controller.refresh_git_status_now());
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

async fn wait_for_file_completion(controller: &mut InteractiveController) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if controller.poll_pending_file_completion().await {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("file completion did not finish");
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
    let temp = tempfile::tempdir().expect("tempdir");
    let project_dir = temp.path().join("workspace");
    std::fs::create_dir_all(&project_dir).expect("workspace");
    let config = test_config(&project_dir, temp.path().join("sessions"));
    let session_dir = workspace_sessions_dir(&config).join(SESSION_A);
    assert!(config.workflow_runtime.notification_queue().enqueue(
        neo_agent_core::WorkflowNotification::new(
            &session_dir,
            neo_agent_core::workflow::WorkflowId("wf_background_question".to_owned()),
            neo_agent_core::workflow::WorkflowState::Completed,
            "worker completed",
        ),
    ));
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
    controller.local_config = Some(config.clone());
    controller.set_active_session_id(SESSION_A.to_owned());
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
    assert_eq!(requests[0].session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        requests[0].prompt_origin,
        MessageOrigin::injection("background_question")
    );
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
    assert_eq!(
        config
            .workflow_runtime
            .notification_queue()
            .pending_for_session(&session_dir)
            .len(),
        1,
        "background-question continuation must not consume workflow notifications"
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
async fn image_prompt_submit_renders_user_transcript_with_attachment() {
    let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x02\x00\x00\x00\x90wS\xde"
        .to_vec();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |request| async move {
            assert_eq!(request.prompt.len(), 2);
            assert_eq!(request.prompt[0], Content::text("look "));
            assert!(matches!(request.prompt[1], Content::Image { .. }));
            Ok(vec![AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            }])
        },
    );
    controller.image_attachment_store.add(
        "sha256".to_owned(),
        "image/png".to_owned(),
        1,
        1,
        Some(png),
    );

    controller.type_text("look [image #1 (1x1)]");
    controller.submit_prompt().await.expect("prompt succeeds");

    let image_entry = transcript_entries(&controller)
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, images }
                if content == "look [image #1 (1x1)]" =>
            {
                Some(images)
            }
            _ => None,
        })
        .expect("user transcript entry with image placeholder");
    assert_eq!(image_entry.len(), 1);
    assert_eq!(image_entry[0].mime_type, "image/png");
    assert_eq!(image_entry[0].placeholder, "[image #1 (1x1)]");
    assert!(!image_entry[0].payload.is_empty());
}

#[tokio::test]
async fn idle_terminal_polling_does_not_render_repeated_frames() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }

            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let _ = tui.render_frame(80, 24);
                render_count += 1;
                Ok(None)
            },
            || Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 3,
            },
        )
        .await
        .expect("event loop exits after idle polls");

    assert_eq!(
        render_count, 1,
        "idle timeout polls must not request frames"
    );
}

#[tokio::test]
async fn animation_deadline_requests_one_follow_up_frame_without_input() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }
            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
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
        .set_custom_working_label(Some("testing animation".to_owned()));
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, animation_due| {
                render_count += 1;
                if animation_due {
                    tui.advance_animation_at(Instant::now());
                }
                let deadline = tui
                    .chrome()
                    .working_label()
                    .map(|_| Instant::now() + Duration::from_millis(1));
                Ok(deadline)
            },
            || Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 1,
            },
        )
        .await
        .expect("event loop exits after deadline frame");

    assert_eq!(render_count, 2, "one startup and one deadline frame");
}

#[tokio::test]
async fn cleared_animation_deadline_does_not_render_again_while_idle() {
    struct IdleThenInterruptEvents {
        remaining_idle_polls: usize,
    }

    impl TerminalEvents for IdleThenInterruptEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.remaining_idle_polls == 0 {
                return Ok(Some(InputEvent::Interrupt));
            }
            self.remaining_idle_polls -= 1;
            std::thread::sleep(timeout);
            Ok(None)
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
        .set_custom_working_label(Some("testing animation".to_owned()));
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    let mut animation_render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, animation_due| {
                render_count += 1;
                if animation_due {
                    animation_render_count += 1;
                    tui.chrome_mut().set_custom_working_label(None);
                    tui.advance_animation_at(Instant::now());
                }
                let frame = tui.render_terminal_frame_at(80, 24, Instant::now());
                Ok(frame.next_animation_deadline)
            },
            || Ok(()),
            IdleThenInterruptEvents {
                remaining_idle_polls: 3,
            },
        )
        .await
        .expect("event loop exits after idle polls");

    assert_eq!(
        animation_render_count, 1,
        "only one deadline frame advances animation"
    );
    assert!(render_count >= 2);
}

#[tokio::test]
async fn chrome_only_btw_update_requests_a_frame() {
    struct IdleThenInterrupt {
        idle: bool,
    }

    impl TerminalEvents for IdleThenInterrupt {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            Ok(InputEvent::Interrupt)
        }

        fn poll_input_event(&mut self, timeout: Duration) -> Result<Option<InputEvent>> {
            if self.idle {
                self.idle = false;
                std::thread::sleep(timeout);
                Ok(None)
            } else {
                Ok(Some(InputEvent::Interrupt))
            }
        }
    }

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_btw_panel_state(Some(
        neo_tui::widgets::btw_panel::BtwPanelState::new(
            neo_tui::widgets::btw_panel::BtwSidecar::new("sidecar-1"),
        ),
    ));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(crate::modes::btw::BtwEvent::Started {
            sidecar_id: "sidecar-1".to_owned(),
            prompt: "question".to_owned(),
        })
        .expect("send sidecar event");
    controller.btw_receiver = Some(receiver);
    assert!(
        !controller
            .handle_input_event(InputEvent::Interrupt)
            .await
            .expect("first interrupt requests confirmation")
    );

    let mut render_count = 0;
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let _ = tui.render_terminal_frame_at(80, 24, Instant::now());
                render_count += 1;
                Ok(None)
            },
            || Ok(()),
            IdleThenInterrupt { idle: true },
        )
        .await
        .expect("event loop exits");

    assert_eq!(render_count, 2, "BTW event must request one frame");
}

#[tokio::test]
async fn empty_async_polls_report_no_visible_change() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    assert!(!controller.poll_pending_catalog_fetch().await);
    assert!(!controller.poll_pending_custom_endpoint_fetch().await);
    assert!(!controller.poll_pending_custom_endpoint_test().await);
    assert!(!controller.poll_pending_mcp_probe().await);
}

#[test]
fn blocking_dialog_events_request_an_immediate_frame() {
    let question = AgentEvent::QuestionRequested {
        turn: 1,
        id: "question-1".to_owned(),
        questions: Vec::new(),
    };
    let text = AgentEvent::TextDelta {
        turn: 1,
        text: "delta".to_owned(),
    };

    assert_eq!(
        InteractiveController::frame_request_for_agent_event(&question),
        FrameRequest::Immediate
    );
    assert_eq!(
        InteractiveController::frame_request_for_agent_event(&text),
        FrameRequest::Coalesced
    );
}

#[tokio::test]
async fn ctrl_o_renders_before_queued_tool_finish() {
    struct ScriptedEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let (finish_queued_tx, finish_queued_rx) = tokio::sync::oneshot::channel();
    let finish_queued_tx = Arc::new(std::sync::Mutex::new(Some(finish_queued_tx)));
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let finish_queued_tx = Arc::clone(&finish_queued_tx);
        Box::pin(async move {
            channels.send_event(AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "write-1".to_owned(),
                name: "Write".to_owned(),
                result: ToolResult::ok("write complete"),
            });
            let sender = finish_queued_tx.lock().expect("finish sender lock").take();
            if let Some(sender) = sender {
                let _ = sender.send(());
            }
            channels.cancel_token.cancelled().await;
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
    let content = (1..=12)
        .map(|line| format!("live-line-{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "write-1".to_owned(),
            name: "Write".to_owned(),
            arguments: serde_json::json!({
                "files": [{
                    "path": "artifact.txt",
                    "content": content,
                }],
            }),
        });
    controller.start_turn_with_prompt_origin(Vec::new(), MessageOrigin::User);
    finish_queued_rx.await.expect("finish event queued");

    let mut rendered = Vec::new();
    controller
        .run_terminal_loop_with_suspend(
            |tui, _| {
                let frame = tui.render_terminal_frame_at(80, 24, Instant::now());
                let text = frame
                    .history
                    .iter()
                    .flat_map(|block| block.lines.iter())
                    .chain(frame.live.iter())
                    .map(|line| neo_tui::primitive::strip_ansi(line))
                    .collect::<Vec<_>>()
                    .join("\n");
                tui.acknowledge_history(&frame);
                rendered.push((frame.review_surface, text));
                Ok(frame.next_animation_deadline)
            },
            || Ok(()),
            ScriptedEvents(VecDeque::from([
                InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")),
                InputEvent::Interrupt,
                InputEvent::Interrupt,
                InputEvent::Interrupt,
            ])),
        )
        .await
        .expect("event loop exits");

    let (review_surface, first_after_ctrl_o) = rendered.get(1).expect("frame after ctrl-o");
    assert!(!*review_surface);
    assert!(first_after_ctrl_o.contains("Using Write"));
    assert!(first_after_ctrl_o.contains("1 files · unverified intent"));
    assert!(first_after_ctrl_o.contains("artifact.txt"));
    assert!(!first_after_ctrl_o.contains("Used Write"));
    assert!(controller.transcript().tool_output_expanded());
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
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
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
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
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
            |tui, _| {
                rendered.push(render_tui_snapshot(tui));
                Ok(None)
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
async fn event_loop_ctrl_d_cancels_active_shell_without_starting_queued_commands() {
    struct ScriptedEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("expected scripted input"))
        }
    }

    let commands = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let observed_commands = Arc::clone(&commands);
    let cancel_token = Arc::new(std::sync::Mutex::new(None::<CancellationToken>));
    let observed_cancel_token = Arc::clone(&cancel_token);
    let cancellation_observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let driver_cancellation_observed = Arc::clone(&cancellation_observed);
    let driver_started = Arc::new(tokio::sync::Notify::new());
    let observed_driver_started = Arc::clone(&driver_started);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_shell_driver(Arc::new(move |request| {
        let observed_commands = Arc::clone(&observed_commands);
        let observed_cancel_token = Arc::clone(&observed_cancel_token);
        let driver_cancellation_observed = Arc::clone(&driver_cancellation_observed);
        let observed_driver_started = Arc::clone(&observed_driver_started);
        Box::pin(async move {
            observed_commands
                .lock()
                .expect("command lock")
                .push(request.command.clone());
            *observed_cancel_token.lock().expect("cancel token lock") =
                Some(request.cancel_token.clone());
            // Emit ShellCommandStarted so the transcript records a ShellRun
            // entry. Production ShellDrivers emit this themselves; the test
            // driver must simulate it.
            let _ = request.event_tx.send(AgentEvent::ShellCommandStarted {
                turn: 0,
                id: request.id.clone(),
                command: request.command.clone(),
                cwd: std::path::PathBuf::new(),
                origin: ShellCommandOrigin::UserShellMode,
            });
            observed_driver_started.notify_one();
            request.cancel_token.cancelled().await;
            driver_cancellation_observed.store(true, std::sync::atomic::Ordering::SeqCst);
            let mut result = completed_shell_result("");
            result.exit_code = None;
            result.outcome = neo_agent_core::ShellCommandOutcome::Cancelled;
            Ok(result)
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
    tokio::time::timeout(Duration::from_secs(1), driver_started.notified())
        .await
        .expect("shell driver starts");

    controller
        .run_terminal_loop_with_suspend(
            |_, _| Ok(None),
            || Ok(()),
            ScriptedEvents(VecDeque::from([
                InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")),
                InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")),
            ])),
        )
        .await
        .expect("event loop exits");

    assert!(
        cancel_token
            .lock()
            .expect("cancel token lock")
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled),
        "terminal exit must cancel the active shell"
    );
    assert!(cancellation_observed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(commands.lock().expect("command lock").as_slice(), ["one"]);
    assert!(controller.chrome().pending_input().is_empty());
    assert!(controller.active_shell_command.is_none());
    assert!(!controller.chrome().shell_running());
    assert!(transcript_entries(&controller).iter().any(|entry| {
        matches!(
            entry,
            TranscriptEntry::ShellRun { component }
                if component.command() == "one"
                    && component.finalization()
                        == neo_tui::primitive::Finalization::Finalized
        )
    }));
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
async fn event_loop_ctrl_p_toggles_slash_completion() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p opens slash completion");

    assert_eq!(controller.chrome().prompt().text, "");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p closes slash completion");

    assert_eq!(controller.chrome().prompt().text, "");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_ctrl_p_toggles_slash_completion_without_editing_existing_prompt() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.type_text("hello");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p opens slash completion");

    assert_eq!(controller.chrome().prompt().text, "hello");
    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::PromptCompletion(_))
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+p").expect("valid key")))
        .await
        .expect("ctrl+p closes slash completion");

    assert_eq!(controller.chrome().prompt().text, "hello");
    assert!(controller.chrome().focused_overlay().is_none());
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

    for ch in "/btw".chars() {
        controller
            .handle_input_event(InputEvent::Insert(ch))
            .await
            .expect("typing slash command updates completion");
    }

    let rendered = controller.chrome().focused_overlay_lines(80).join("\n");
    assert!(
        rendered.contains("/btw"),
        "slash completion should include /btw; got:\n{rendered}"
    );
}

#[tokio::test]
async fn slash_completion_refreshes_skills_from_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let extra_skills = temp.path().join("extra-skills");
    fs::create_dir_all(&extra_skills).expect("create extra skills");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.extra_skill_dirs = vec![extra_skills.to_string_lossy().into_owned()];

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);

    let skill_dir = extra_skills.join("fresh-skill");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: fresh-skill\ndescription: Fresh from disk\n---\n\nUse me.",
    )
    .expect("write skill");

    for ch in "/skill:f".chars() {
        controller
            .handle_input_event(InputEvent::Insert(ch))
            .await
            .expect("typing skill prefix updates completion");
    }

    let rendered = render_overlay_snapshot(controller.chrome(), 100).join("\n");
    assert!(
        rendered.contains("/skill:fresh-skill"),
        "slash completion should include freshly reloaded skill; got:\n{rendered}"
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
}

#[test]
fn auto_image_protocol_detects_ghostty_as_kitty_graphics() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-ghostty".to_owned()),
        "TERM_PROGRAM" => Ok("ghostty".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities = terminal_image_capabilities_for_policy(ImageProtocolPreference::Auto, env);

    assert!(capabilities.kitty());
    assert!(!capabilities.iterm2());
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
}

#[test]
fn terminal_capabilities_dumb_term_disables_ansi_and_images() {
    let env = |name: &str| match name {
        "TERM" => Ok("dumb".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(!capabilities.ansi.cursor_addressing);
    assert!(!capabilities.ansi.color);
    assert!(!capabilities.image.kitty());
    assert!(!capabilities.can_run_tui());
}

#[test]
fn terminal_capabilities_wt_session_disables_images_keeps_ansi() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-256color".to_owned()),
        "WT_SESSION" => Ok("00000000-0000-0000-0000-000000000000".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(capabilities.ansi.cursor_addressing);
    assert!(capabilities.ansi.color);
    assert!(capabilities.can_run_tui());
    assert!(!capabilities.image.kitty());
    assert!(!capabilities.image.iterm2());
}

#[test]
fn terminal_capabilities_ci_disables_tui_and_images() {
    let env = |name: &str| match name {
        "TERM" => Ok("xterm-256color".to_owned()),
        "CI" => Ok("true".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, env);

    assert!(!capabilities.ansi.cursor_addressing);
    assert!(!capabilities.can_run_tui());
    assert!(!capabilities.image.kitty());
}

#[test]
fn terminal_capabilities_no_color_only_disables_color() {
    let no_color_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        "NO_COLOR" => Ok("1".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };
    let color_env = |name: &str| match name {
        "TERM" => Ok("xterm-kitty".to_owned()),
        _ => Err(env::VarError::NotPresent),
    };

    let capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, no_color_env);
    let color_capabilities =
        detect_terminal_capabilities_with_env(ImageProtocolPreference::Auto, true, color_env);

    assert!(capabilities.ansi.cursor_addressing);
    assert!(capabilities.ansi.bracketed_paste);
    assert!(capabilities.can_run_tui());
    assert!(!capabilities.ansi.color);
    assert_eq!(capabilities.image.kitty(), color_capabilities.image.kitty());
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
async fn manual_skill_context_uses_shared_path_aware_envelope() {
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
        Some(TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Manual,
            body,
            ..
        }) if names == &vec!["skill_one".to_owned(), "skill_two".to_owned()] && body == stripped
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
        skill_context.contains(&format!(
            "<neo-skill-loaded name=\"skill_one\" source=\"builtin\" root=\"{}\">",
            test_workspace_root().join("builtin/skill_one").display()
        )),
        "{skill_context}"
    );
    assert!(
        skill_context.contains(
            "<dependencies>\n  <mcp value=\"reviewServer\">Review MCP server</mcp>\n</dependencies>"
        ),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<neo-user-request>\nfoo\nbar\ntest test test"),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("<instructions>\nONE: test test test\n</instructions>"),
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
            .any(|entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == stripped)),
        "skill activation body should not be rendered again as a user message"
    );
}

#[tokio::test]
async fn automatic_skill_invocation_renders_one_semantic_card() {
    use futures::StreamExt as _;
    use neo_agent_core::harness::FakeHarness;

    let harness = FakeHarness::from_turns([
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallStart {
                id: "skill-1".to_owned(),
                name: "Skill".to_owned(),
            },
            neo_ai::AiStreamEvent::ToolCallEnd {
                id: "skill-1".to_owned(),
                raw_arguments: serde_json::json!({"skill": "refactor"}).to_string(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            neo_ai::AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            neo_ai::AiStreamEvent::TextDelta {
                text: "done".to_owned(),
            },
            neo_ai::AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let model = harness.model();
    let client = harness.client();
    let run_turn: TurnDriver = Arc::new(move |request, channels| {
        let model = model.clone();
        let client = Arc::clone(&client);
        Box::pin(async move {
            let runtime = neo_agent_core::AgentRuntime::with_tools_and_skills(
                neo_agent_core::AgentConfig::for_model(model),
                client,
                neo_agent_core::ToolRegistry::new(),
                skill_store_with_refactor_skill(),
            );
            let mut context = neo_agent_core::AgentContext::new();
            let mut events =
                runtime.run_turn(&mut context, AgentMessage::user_content(request.prompt));
            while let Some(event) = events.next().await {
                channels.send_event(event?);
            }
            Ok(TurnOutcome::default())
        })
    });
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "fake/model",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("use refactor skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("automatic skill turn completes");

    let entries = transcript_entries(&controller);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
            .count(),
        1,
        "automatic invocation should render exactly one semantic card"
    );
    assert!(entries.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Activated,
            ..
        } if names == &["refactor".to_owned()]
    )));
    assert!(
        entries
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::ToolRun { .. })),
        "the hidden Skill tool must not create a duplicate generic card"
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
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == &expected_display)
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

    let completions = prompt_completions(temp.path(), "/", None, true).expect("slash completions");
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
    assert_eq!(
        by_value["/sessions"].description.as_deref(),
        Some("Alias for /resume")
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

#[tokio::test]
async fn slash_completion_no_match_keeps_the_current_catalog() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(prompts_dir.join("alpha.md"), "alpha").expect("write alpha");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async { Ok(Vec::<AgentEvent>::new()) },
    );

    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("open completion");
    controller
        .handle_input_event(InputEvent::Insert('z'))
        .await
        .expect("hide unmatched completion");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.slash_completion_catalog.is_some());
    fs::write(prompts_dir.join("zzz.md"), "zzz").expect("write zzz");

    controller
        .handle_input_event(InputEvent::Insert('z'))
        .await
        .expect("continue unmatched completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(
        controller
            .slash_completion_catalog
            .as_ref()
            .expect("catalog remains loaded")
            .slash_prompts
            .iter()
            .all(|item| item.value != "/zzz")
    );
}

#[tokio::test]
async fn escape_ends_the_slash_completion_catalog_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let prompts_dir = temp.path().join(".neo/prompts");
    fs::create_dir_all(&prompts_dir).expect("create prompts");
    fs::write(prompts_dir.join("first.md"), "first").expect("write first");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async { Ok(Vec::<AgentEvent>::new()) },
    );
    controller
        .handle_input_event(InputEvent::Insert('/'))
        .await
        .expect("open completion");

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel completion");

    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.slash_completion_catalog.is_none());
    fs::write(prompts_dir.join("second.md"), "second").expect("write second");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("open next completion session");
    assert!(
        controller
            .slash_completion_catalog
            .as_ref()
            .expect("new catalog loaded")
            .slash_prompts
            .iter()
            .any(|item| item.value == "/second")
    );
}

#[test]
fn slash_completion_descriptions_hide_internal_metadata() {
    let completions =
        prompt_completions(&test_workspace_root(), "/ask", None, true).expect("slash completions");
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
    let completions =
        prompt_completions(&test_workspace_root(), "/skill:", Some(&skill_store), true)
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

#[tokio::test]
async fn completion_keeps_full_skill_command_and_uses_host_description_fallback() {
    let skill_store = SkillStore::load(
        &[],
        &[],
        vec![LoadedSkill {
            name: "schema-review".to_owned(),
            root: test_workspace_root().join("schema-review"),
            manifest: SkillManifest {
                name: "schema-review".to_owned(),
                description: "Manifest fallback".to_owned(),
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
            },
            body: "Review schemas.".to_owned(),
            source: SkillSource::User,
            host_metadata: SkillHostMetadata {
                interface: Some(SkillInterface {
                    display_name: Some("Schema Review".to_owned()),
                    short_description: None,
                }),
                dependencies: Vec::new(),
            },
        }],
    );
    let completions = prompt_completions(
        &test_workspace_root(),
        "/skill:schema",
        Some(&skill_store),
        true,
    )
    .expect("skill completions resolve");
    let skill = completions
        .iter()
        .find(|item| item.value == "/skill:schema-review")
        .expect("missing host-labelled skill command");

    assert_eq!(skill.value, "/skill:schema-review");
    assert_eq!(skill.label, "/skill:schema-review");
    assert_eq!(
        skill.description.as_deref(),
        Some("Schema Review: Manifest fallback")
    );

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.skill_store = Some(skill_store);
    controller.type_text("/skill:schema");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab completes canonical skill command");

    assert_eq!(controller.chrome().prompt().text, "/skill:schema-review");
}

#[test]
fn slash_completions_include_help_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let help = completions
        .iter()
        .find(|item| item.value == "/help")
        .expect("missing /help completion");

    assert_eq!(help.label, "/help");
    assert_eq!(help.description.as_deref(), Some("Show help information"));
}

#[test]
fn slash_completions_include_init_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("slash completions");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();

    assert!(values.contains(&"/init"), "missing /init: {values:?}");
}

#[test]
fn init_command_prompt_includes_instruction_and_structure_guardrails() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: Some("abc1234"),
        instruction: Some("排除掉 generated 目录"),
        auto_mode_best_effort: false,
    });

    assert!(prompt.contains("Target file: /workspace/neo/AGENTS.md"));
    assert!(prompt.contains("Current date: 2026-07-07"));
    assert!(prompt.contains("Source commit: abc1234"));
    assert!(prompt.contains("User instruction after /init: 排除掉 generated 目录"));
    assert!(prompt.contains("Required top-level sections, in order:"));
    assert!(prompt.contains("1. Reference"));
    assert!(prompt.contains("14. Metadata"));
    assert!(prompt.contains("Before writing AGENTS.md, prepare a concise outline plan"));
    assert!(prompt.contains("Golden style example"));
    assert!(
        prompt
            .to_lowercase()
            .contains("ask the user where reference projects or reference documents live")
    );
    assert!(prompt.contains("Do not treat this guide as a Neo product specification"));
    assert!(
        !prompt.contains("CLAUDE.md"),
        "legacy guide leaked: {prompt}"
    );
    assert!(
        !prompt.contains("GEMINI.md"),
        "legacy guide leaked: {prompt}"
    );
}

#[test]
fn init_command_prompt_marks_auto_mode_best_effort() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: None,
        instruction: None,
        auto_mode_best_effort: true,
    });

    assert!(prompt.contains("Source commit: unavailable"));
    assert!(prompt.contains("Auto permission mode remained active"));
    assert!(prompt.contains("proceed with best-effort assumptions"));
}

#[test]
fn agents_guide_validator_accepts_required_structure() {
    let guide = init_command::example_agents_guide_for_tests("abc1234");
    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn agents_guide_validator_reports_missing_duplicate_and_reordered_headings() {
    let guide = r"# Project Agent Guide

## Reference
text

## Workflow
text

## Project Identity
text

## Workflow
text

## Metadata
Created: 2026-07-07
Source commit: abc1234
Best valid until: 2026-10-07
";

    let issues = init_command::validate_agents_guide(guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::DuplicateHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::HeadingOrder));
}

#[test]
fn agents_guide_validator_reports_missing_metadata_and_placeholders() {
    let guide = init_command::REQUIRED_AGENTS_HEADINGS
        .iter()
        .map(|heading| format!("## {heading}\nplaceholder content\n"))
        .collect::<Vec<_>>()
        .join("\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingMetadataField));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::PlaceholderText));
}

#[test]
fn agents_guide_validator_reports_hardcoded_reference_default_variants() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nReferences are in `.references`.\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::HardcodedReferenceDefault));
}

#[test]
fn agents_guide_validator_allows_negative_reference_default_framing() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nDo not assume .references/ is the reference location.\n");

    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn agents_guide_validator_reports_product_spec_framing_variants() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nThis guide is the Neo product specification.\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::ProductSpecFraming));
}

#[test]
fn agents_guide_validator_allows_negative_product_spec_framing() {
    let mut guide = init_command::example_agents_guide_for_tests("abc1234");
    guide.push_str("\nDo not treat this guide as a Neo product specification.\n");

    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn init_repair_prompt_lists_validator_issues() {
    let issues = vec![
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingHeading,
            message: "Missing required heading: Security".to_owned(),
        },
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingMetadataField,
            message: "Missing Metadata field: Best valid until:".to_owned(),
        },
    ];

    let prompt = init_command::build_agents_guide_repair_prompt(&issues);

    assert!(prompt.contains("AGENTS.md structure validation failed"));
    assert!(prompt.contains("Missing required heading: Security"));
    assert!(prompt.contains("Missing Metadata field: Best valid until:"));
    assert!(prompt.contains("Update AGENTS.md in place"));
    assert!(prompt.contains("Do not claim success until the issues are fixed"));
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

    let file_references =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("file references");
    assert!(
        file_references
            .iter()
            .all(|candidate| candidate.value != "@anthropic/claude-sonnet")
    );
    assert!(
        file_references
            .iter()
            .all(|candidate| candidate.source == CompletionSource::FileReference)
    );
}

#[test]
fn at_file_reference_completion_fuzzy_ranks_basename_matches() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("crates/neo-agent/src/modes/interactive");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("prompt_completion.rs"), "").expect("write prompt completion");
    fs::write(src.join("completion_prompt.rs"), "").expect("write weaker match");

    let catalog = CompletionCatalog::default();
    let candidates =
        completion_source_candidates(temp.path(), "@prom", &catalog).expect("file references");

    assert_eq!(
        candidates[0].value,
        "@crates/neo-agent/src/modes/interactive/prompt_completion.rs"
    );
    assert_eq!(candidates[0].label, "prompt_completion.rs");
    assert_eq!(
        candidates[0].description.as_deref(),
        Some("crates/neo-agent/src/modes/interactive/")
    );
    assert_eq!(candidates[0].source, CompletionSource::FileReference);
}

#[test]
fn at_file_reference_completion_preserves_match_ranking_over_value_sort() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("aaa")).expect("mkdir aaa");
    fs::create_dir_all(temp.path().join("zzz")).expect("mkdir zzz");
    fs::write(temp.path().join("aaa/not_prompt.rs"), "").expect("write weaker match");
    fs::write(temp.path().join("zzz/prompt_completion.rs"), "").expect("write stronger match");

    let catalog = CompletionCatalog::default();
    let candidates =
        completion_source_candidates(temp.path(), "@prom", &catalog).expect("file references");

    assert_eq!(candidates[0].value, "@zzz/prompt_completion.rs");
    assert_eq!(candidates[0].label, "prompt_completion.rs");
    assert_eq!(candidates[0].source, CompletionSource::FileReference);
}

#[test]
fn at_file_reference_completion_caps_large_walks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let inspected_cap = 7;
    for index in 0..(inspected_cap + 5) {
        fs::write(temp.path().join(format!("prompt_{index:04}.rs")), "").expect("write match");
    }

    let candidates = super::prompt_completion::file_reference_completion_candidates_with_limits(
        temp.path(),
        "@prompt",
        inspected_cap,
        super::prompt_completion::MAX_FILE_REFERENCE_COMPLETIONS,
    );

    assert_eq!(candidates.len(), inspected_cap);
    assert!(candidates.iter().all(|candidate| {
        candidate.value.starts_with("@prompt_")
            && candidate.source == CompletionSource::FileReference
    }));
}

#[test]
fn at_file_reference_completion_hides_dotfiles_until_dot_query() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(temp.path().join(".env"), "secret").expect("write env");
    fs::write(temp.path().join("Cargo.toml"), "").expect("write cargo");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir src");
    fs::write(temp.path().join("src/.env"), "nested secret").expect("write nested env");

    let catalog = CompletionCatalog::default();
    let hidden = completion_source_candidates(temp.path(), "@e", &catalog).expect("hidden query");
    assert!(hidden.iter().all(|candidate| candidate.label != ".env"));

    let visible = completion_source_candidates(temp.path(), "@.e", &catalog).expect("dot query");
    assert!(visible.iter().any(|candidate| candidate.label == ".env"));

    let nested_visible =
        completion_source_candidates(temp.path(), "@src/.e", &catalog).expect("nested dot query");
    assert!(
        nested_visible
            .iter()
            .any(|candidate| candidate.value == "@src/.env")
    );
}

#[test]
fn at_file_reference_completion_no_longer_returns_provider_models() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("docs")).expect("mkdir docs");
    fs::write(temp.path().join("docs/anthology.md"), "notes\n").expect("write file");
    let catalog = CompletionCatalog::default();

    let candidates =
        completion_source_candidates(temp.path(), "@anth", &catalog).expect("file references");

    assert!(!candidates.is_empty());
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.value != "@anthropic/claude-sonnet")
    );
    assert!(
        candidates
            .iter()
            .all(|candidate| candidate.source == CompletionSource::FileReference)
    );
}

fn slash_test_catalog() -> CompletionCatalog {
    CompletionCatalog {
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
        session_commands: vec![
            PickerItem::new("/resume", "/resume", Some("Resume a local session")),
            PickerItem::new("/new", "/new", Some("Start a fresh local session")),
            PickerItem::new("/clear", "/clear", Some("Alias for /new")),
            PickerItem::new("/fork", "/fork", Some("Fork the current session")),
            PickerItem::new("/help", "/help", Some("Show help information")),
            PickerItem::new("/model", "/model", Some("Switch active model")),
            PickerItem::new("/provider", "/provider", Some("View configured providers")),
            PickerItem::new("/mcp", "/mcp", Some("View and manage MCP servers")),
            PickerItem::new("/tasks", "/tasks", Some("View active background tasks")),
            PickerItem::new("/plan", "/plan", Some("Toggle plan mode")),
            PickerItem::new(
                "/compact",
                "/compact",
                Some("Request manual context compaction"),
            ),
            PickerItem::new(
                "/permissions",
                "/permissions",
                Some("select permission mode"),
            ),
            PickerItem::new("/ask", "/ask", Some("ask permission mode")),
            PickerItem::new("/auto", "/auto", Some("auto permission mode")),
            PickerItem::new("/yolo", "/yolo", Some("yolo permission mode")),
            PickerItem::new("/btw", "/btw", Some("Open a temporary side-question panel")),
            PickerItem::new(
                "/skill:code-simplifier",
                "/skill:code-simplifier",
                Some("Simplify and refine code"),
            ),
        ],
    }
}

fn slash_values_for(prefix: &str, catalog: &CompletionCatalog) -> Vec<String> {
    completion_source_candidates(&test_workspace_root(), prefix, catalog)
        .expect("slash completions")
        .into_iter()
        .map(|candidate| candidate.value)
        .collect()
}

#[test]
fn slash_fuzzy_completions_keep_empty_query_order() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/", &catalog);

    assert_eq!(
        values[..5],
        ["/review", "/review-package", "/resume", "/new", "/clear"],
        "empty slash query keeps source order and curated command order"
    );
}

#[test]
fn slash_fuzzy_completions_rank_prefix_before_fuzzy() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/m", &catalog);

    let model_index = values
        .iter()
        .position(|value| value == "/model")
        .expect("/model present");
    let permissions_index = values
        .iter()
        .position(|value| value == "/permissions")
        .expect("/permissions present as a weaker fuzzy match");

    assert!(
        model_index < permissions_index,
        "prefix match /model should rank before fuzzy-only /permissions"
    );
}

#[test]
fn slash_fuzzy_completions_match_command_abbreviations() {
    let catalog = slash_test_catalog();

    assert_eq!(
        slash_values_for("/mdl", &catalog).first(),
        Some(&"/model".to_owned())
    );
    assert_eq!(
        slash_values_for("/prv", &catalog).first(),
        Some(&"/provider".to_owned())
    );
    assert_eq!(
        slash_values_for("/perm", &catalog).first(),
        Some(&"/permissions".to_owned())
    );
}

#[test]
fn slash_fuzzy_completions_match_skill_name_without_skill_prefix() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/code", &catalog);

    assert_eq!(
        values.first(),
        Some(&"/skill:code-simplifier".to_owned()),
        "skill commands should be searchable by skill name without typing /skill:"
    );
}

#[test]
fn slash_fuzzy_completions_match_prompt_templates() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/rvw", &catalog);

    assert_eq!(values.first(), Some(&"/review".to_owned()));
}

#[test]
fn slash_fuzzy_completions_return_empty_for_miss() {
    let catalog = slash_test_catalog();
    let values = slash_values_for("/zzzznotacommand", &catalog);

    assert!(values.is_empty());
}

#[tokio::test]
async fn event_loop_slash_resume_and_sessions_open_local_session_picker() {
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

    for command in ["/resume", "/sessions"] {
        controller.type_text(command);
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
            .await
            .expect("session picker command runs locally");

        assert!(matches!(
            controller
                .chrome()
                .focused_overlay()
                .map(|overlay| &overlay.kind),
            Some(OverlayKind::SessionPicker(_))
        ));
        assert!(controller.chrome().prompt().text.is_empty());
        assert!(requests.lock().expect("recorded requests").is_empty());
        let _ = controller.tui.chrome_mut().close_focused_overlay();
    }
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
    let completions = prompt_completions(temp.path(), "/", None, true).expect("slash completions");
    assert!(
        !completions.iter().any(|item| item.value == "/tree"),
        "/tree should not appear in slash completion items"
    );
}

#[tokio::test]
async fn event_loop_tab_coalesces_latest_file_completion_and_inserts_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
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

    controller
        .handle_input_event(InputEvent::Insert('@'))
        .await
        .expect("start file completion");
    controller
        .handle_input_event(InputEvent::Paste("main".to_owned()))
        .await
        .expect("queue latest file completion");
    let (queued, complete_on_finish) = controller
        .queued_file_completion
        .as_ref()
        .expect("latest file completion is queued");
    assert_eq!(queued.text, "@main");
    assert!(!*complete_on_finish);
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab inserts file reference");
    let (queued, complete_on_finish) = controller
        .queued_file_completion
        .as_ref()
        .expect("tab upgrades the latest queued completion");
    assert_eq!(queued.text, "@main");
    assert!(*complete_on_finish);
    wait_for_file_completion(&mut controller).await;

    assert_eq!(controller.chrome().prompt().text, "[file #1 main.rs]");
    assert!(controller.chrome().focused_overlay().is_none());
}

#[tokio::test]
async fn event_loop_rejects_parent_dir_file_reference_completion() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("mkdir workspace");
    fs::write(temp.path().join("outside.txt"), "outside\n").expect("write outside");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        &workspace,
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("@bad");
    controller.tui.chrome_mut().open_prompt_completion_picker(
        PromptCompletionPrefix {
            start: 0,
            end: 4,
            text: "@bad".to_owned(),
        },
        [PickerItem::new(
            "@../outside.txt",
            "outside.txt",
            None::<String>,
        )],
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab rejects parent-dir file reference");

    assert_eq!(controller.chrome().prompt().text, "@bad");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "File reference is outside the workspace"
    ));
}

#[tokio::test]
async fn event_loop_closes_stale_file_reference_picker_without_inserting_marker() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    let main = temp.path().join("src/main.rs");
    fs::write(&main, "fn main() {}\n").expect("write main");
    fs::write(
        temp.path().join("src/main_test.rs"),
        "#[test]\nfn main_test() {}\n",
    )
    .expect("write second match");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );

    controller.type_text("@main");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab opens file reference picker");
    wait_for_file_completion(&mut controller).await;
    assert!(controller.chrome().focused_overlay().is_some());

    fs::remove_file(main).expect("remove selected completion");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab rejects stale file reference");

    assert_eq!(controller.chrome().prompt().text, "@main");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "File reference no longer exists"
    ));
}

#[tokio::test]
async fn event_loop_submits_file_reference_content() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(temp.path().join("src")).expect("mkdir");
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").expect("write file");

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
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
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(LoadedSessionTranscript::new(
                session_id,
                Vec::new(),
                Vec::new(),
            ))
        },
    );

    controller.type_text("review @main");
    controller.tui.chrome_mut().open_prompt_completion_picker(
        PromptCompletionPrefix {
            start: 7,
            end: 12,
            text: "@main".to_owned(),
        },
        [PickerItem::new("@src/main.rs", "main.rs", None::<String>)],
    );
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("insert file reference");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit turn");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text(
            "review <file path=\"src/main.rs\">\nfn main() {}\n</file>"
        )]
    );
}

#[tokio::test]
async fn event_loop_file_reference_marker_keeps_chip_in_user_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("crates/neo-agent/src/modes/interactive");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("prompt_completion.rs"), "").expect("write prompt completion");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
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
                        text: "file reference expanded".to_owned(),
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

    controller.type_text("@prom");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputTab))
        .await
        .expect("tab inserts file reference marker");
    wait_for_file_completion(&mut controller).await;
    assert_eq!(
        controller.chrome().prompt().text,
        "[file #1 prompt_completion.rs]"
    );
    controller.type_text(" explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits with file reference");
    controller
        .wait_for_active_turn()
        .await
        .expect("file reference turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].prompt,
        vec![Content::text(
            "<file path=\"crates/neo-agent/src/modes/interactive/prompt_completion.rs\">\n</file> explain this file"
        )]
    );
    assert_eq!(
        requests[0].prompt_display_text.as_deref(),
        Some("@[prompt_completion.rs] explain this file")
    );
    assert_eq!(requests[0].model, None);
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. }
            if content == "@[prompt_completion.rs] explain this file"
    )));
    assert!(
        transcript_entries(&controller)
            .iter()
            .all(|entry| !matches!(
                entry,
                TranscriptEntry::UserMessage { content, .. } if content.contains("<file path=")
            ))
    );
}

#[tokio::test]
async fn queued_file_reference_keeps_chip_when_appended() {
    let mut controller = running_turn_controller().await;
    controller.type_text("review [file #1 main.rs]");

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
        vec!["review @[main.rs]"]
    );
    controller.apply_turn_event(AgentEvent::FollowUpQueued {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "review <file path=\"src/main.rs\">snapshot</file>",
            )],
            "review @[main.rs]",
        ),
    });
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .len(),
        1,
        "runtime ack must consume the compact optimistic preview"
    );
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "review <file path=\"src/main.rs\">snapshot</file>",
            )],
            "review @[main.rs]",
        ),
    });
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. } if content == "review @[main.rs]"
    )));

    controller.cancel_active_turn().await.expect("cancel turn");
}

#[tokio::test]
async fn steered_file_reference_keeps_chip_when_appended() {
    let mut controller = running_turn_controller().await;
    controller.type_text("inspect [file #1 lib.rs]");

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("ctrl+s steers");

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["inspect @[lib.rs]"]
    );
    controller.apply_turn_event(AgentEvent::SteeringQueued {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "inspect <file path=\"src/lib.rs\">snapshot</file>",
            )],
            "inspect @[lib.rs]",
        ),
    });
    assert_eq!(
        controller.chrome().pending_input().pending_steers().len(),
        1,
        "runtime ack must consume the compact optimistic preview"
    );
    controller.apply_turn_event(AgentEvent::MessageAppended {
        message: AgentMessage::user_content_with_display(
            [Content::text(
                "inspect <file path=\"src/lib.rs\">snapshot</file>",
            )],
            "inspect @[lib.rs]",
        ),
    });
    assert!(transcript_entries(&controller).iter().any(|entry| matches!(
        entry,
        TranscriptEntry::UserMessage { content, .. } if content == "inspect @[lib.rs]"
    )));

    controller.cancel_active_turn().await.expect("cancel turn");
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
async fn event_loop_at_model_token_submits_as_plain_text() {
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

    controller.type_text("@anthropic/claude-sonnet explain this file");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("turn submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("recorded requests");
    assert_eq!(
        requests[0].prompt,
        vec![Content::text("@anthropic/claude-sonnet explain this file")]
    );
    assert_eq!(requests[0].model, None);
}

#[tokio::test]
async fn event_loop_at_model_token_without_prompt_submits_as_plain_text() {
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
    controller.keybindings.set_user_bindings([(
        KeybindingAction::EditorCursorUp,
        vec![KeyId::new("k").expect("valid key")],
    )]);
    for index in 0..30 {
        controller
            .transcript_mut()
            .push_status(format!("browser-row-{index}"));
    }
    controller.tui.chrome_mut().open_transcript_browser(false);
    let initial = controller.tui.render_terminal_frame(80, 6).live;

    controller
        .handle_input_event(InputEvent::ScrollUp(3))
        .await
        .expect("wheel up scrolls browser toward older rows");
    let wheel_up = controller.tui.render_terminal_frame(80, 6).live;
    assert_ne!(wheel_up, initial);

    controller
        .handle_input_event(InputEvent::ScrollDown(3))
        .await
        .expect("wheel down returns browser to newest rows");
    assert_eq!(controller.tui.render_terminal_frame(80, 6).live, initial);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("k").expect("valid key")))
        .await
        .expect("configured cursor-up scrolls browser toward older rows");
    assert_ne!(controller.tui.render_terminal_frame(80, 6).live, initial);
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down returns browser to newest rows");
    assert_eq!(controller.tui.render_terminal_frame(80, 6).live, initial);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("pageup").expect("valid key")))
        .await
        .expect("page up scrolls browser toward older rows");
    assert_ne!(controller.tui.render_terminal_frame(80, 6).live, initial);
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("pagedown").expect("valid key")))
        .await
        .expect("page down returns browser to newest rows");
    assert_eq!(controller.tui.render_terminal_frame(80, 6).live, initial);
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
async fn automatic_transcript_overflow_scrolls_without_blocking_prompt() {
    let submitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&submitted);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let observed = Arc::clone(&observed);
            async move {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    // Commit one expandable tool so Ctrl+O can open manual review later.
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "committed-read".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "README.md" }),
        });
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "committed-read".to_owned(),
            name: "Read".to_owned(),
            result: ToolResult::ok("committed expandable content"),
        });
    let committed = controller.tui.render_terminal_frame(80, 24);
    controller.tui.acknowledge_history(&committed);
    assert!(controller.transcript().has_committed_expandable_entries());

    // Living tool with a tall body latches automatic overflow.
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "overflow-tool".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "overflow-controller-command" }),
        });
    let body = (0..40)
        .map(|index| format!("overflow-controller-sentinel-{index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    controller
        .transcript_mut()
        .apply_agent_event(AgentEvent::ToolExecutionUpdate {
            turn: 1,
            id: "overflow-tool".to_owned(),
            name: "Bash".to_owned(),
            partial_result: ToolResult::ok(body),
        });

    let frame = controller.tui.render_terminal_frame(40, 8);
    assert!(controller.tui.automatic_overflow_active());
    assert!(frame.review_surface);
    let before = frame
        .live
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::EditorPageUp))
        .await
        .expect("pageup scrolls automatic overflow");
    let scrolled = controller.tui.render_terminal_frame(40, 8);
    let after = scrolled
        .live
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(controller.tui.automatic_overflow_active());
    assert_ne!(
        before, after,
        "pageup must move the automatic overflow viewport"
    );

    controller
        .handle_input_event(InputEvent::Insert('h'))
        .await
        .expect("prompt remains editable during overflow");
    controller
        .handle_input_event(InputEvent::Insert('i'))
        .await
        .expect("prompt remains editable during overflow");
    assert_eq!(controller.chrome().prompt().text, "hi");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("submit works during overflow");
    controller
        .wait_for_active_turn()
        .await
        .expect("submitted turn completes");
    assert!(submitted.load(std::sync::atomic::Ordering::SeqCst));
    assert!(controller.tui.automatic_overflow_active());

    // Manual Ctrl+O review takes logical precedence while the latch remains.
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o opens manual review during overflow");
    assert!(controller.chrome().transcript_browser_state().is_some());
    assert!(controller.tui.automatic_overflow_active());
    let manual = controller.tui.render_terminal_frame(40, 8);
    assert!(manual.review_surface);

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape closes manual review");
    assert!(controller.chrome().transcript_browser_state().is_none());
    assert!(controller.tui.automatic_overflow_active());
    let restored = controller.tui.render_terminal_frame(40, 8);
    assert!(restored.review_surface);
}

#[tokio::test]
async fn ctrl_o_enters_and_leaves_transcript_browser() {
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
    let frame = controller.tui.render_terminal_frame(80, 24);
    controller.tui.acknowledge_history(&frame);
    assert!(controller.transcript().has_committed_expandable_entries());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o opens transcript browser");

    let browser = controller
        .chrome()
        .transcript_browser_state()
        .expect("transcript browser opens");
    assert!(browser.expanded());
    assert!(!controller.transcript().tool_output_expanded());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o toggles browser expansion");
    assert!(
        !controller
            .chrome()
            .transcript_browser_state()
            .expect("transcript browser remains open")
            .expanded()
    );

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("escape closes transcript browser");
    assert!(controller.chrome().transcript_browser_state().is_none());
    assert!(!controller.transcript().tool_output_expanded());
}

#[tokio::test]
async fn transcript_browser_keeps_prompt_editable_and_closes_on_submit() {
    let submitted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::clone(&submitted);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let observed = Arc::clone(&observed);
            async move {
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.tui.chrome_mut().open_transcript_browser(false);

    controller
        .handle_input_event(InputEvent::Insert('a'))
        .await
        .expect("prompt input works during review");
    assert_eq!(controller.chrome().prompt().text, "a");

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("prompt submits during review");
    controller
        .wait_for_active_turn()
        .await
        .expect("submitted turn completes");

    assert!(controller.chrome().transcript_browser_state().is_none());
    assert!(submitted.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn transcript_browser_routes_default_and_custom_suspend_exit_keys() {
    let mut default_controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    default_controller
        .tui
        .chrome_mut()
        .open_transcript_browser(false);

    let suspend = default_controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+z").expect("valid key")))
        .await
        .expect("default suspend key is handled");
    let first_exit = default_controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("default exit key requests confirmation");
    let second_exit = default_controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+d").expect("valid key")))
        .await
        .expect("default exit key confirms exit");

    assert!(!suspend);
    assert!(default_controller.take_suspend_requested());
    assert!(!first_exit);
    assert!(second_exit);

    let mut custom_controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    custom_controller.keybindings.set_user_bindings([
        (
            KeybindingAction::AppSuspend,
            vec![KeyId::new("s").expect("valid key")],
        ),
        (
            KeybindingAction::AppExit,
            vec![KeyId::new("x").expect("valid key")],
        ),
    ]);
    custom_controller
        .tui
        .chrome_mut()
        .open_transcript_browser(false);

    let suspend = custom_controller
        .handle_input_event(InputEvent::Key(KeyId::new("s").expect("valid key")))
        .await
        .expect("custom suspend key is handled");
    let first_exit = custom_controller
        .handle_input_event(InputEvent::Key(KeyId::new("x").expect("valid key")))
        .await
        .expect("custom exit key requests confirmation");
    let second_exit = custom_controller
        .handle_input_event(InputEvent::Key(KeyId::new("x").expect("valid key")))
        .await
        .expect("custom exit key confirms exit");

    assert!(!suspend);
    assert!(custom_controller.take_suspend_requested());
    assert!(!first_exit);
    assert!(second_exit);
}

#[tokio::test]
async fn transcript_browser_routes_direct_global_actions() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().open_transcript_browser(false);

    let suspend = controller
        .handle_input_event(InputEvent::Action(KeybindingAction::AppSuspend))
        .await
        .expect("direct suspend action is handled");
    let first_exit = controller
        .handle_input_event(InputEvent::Action(KeybindingAction::AppExit))
        .await
        .expect("direct exit action requests confirmation");
    let second_exit = controller
        .handle_input_event(InputEvent::Action(KeybindingAction::AppExit))
        .await
        .expect("direct exit action confirms exit");

    assert!(controller.chrome().transcript_browser_state().is_some());
    assert!(!suspend);
    assert!(controller.take_suspend_requested());
    assert!(!first_exit);
    assert!(second_exit);
}

#[tokio::test]
async fn transcript_browser_interrupt_cancels_active_turn() {
    let captured_token = Arc::new(std::sync::Mutex::new(None));
    let observed_token = Arc::clone(&captured_token);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        *observed_token.lock().expect("token lock") = Some(channels.cancel_token.clone());
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
    let frame = controller.tui.render_terminal_frame(80, 24);
    controller.tui.acknowledge_history(&frame);
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+o").expect("valid key")))
        .await
        .expect("ctrl-o opens transcript browser");
    assert!(controller.chrome().transcript_browser_state().is_some());

    controller.start_turn_with_prompt_origin(Vec::new(), MessageOrigin::User);
    let token = captured_token
        .lock()
        .expect("token lock")
        .clone()
        .expect("turn token captured");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+c").expect("valid key")))
        .await
        .expect("ctrl-c reaches global handler");

    let cancelled = token.is_cancelled();
    let active_turn_cleared = controller.active_turn.is_none();
    let interrupted_status = transcript_has_status(&controller, "Interrupted");
    if !cancelled {
        controller
            .cancel_active_turn()
            .await
            .expect("clean up swallowed interrupt");
    }
    assert!(cancelled, "browser must not swallow active-turn interrupt");
    assert!(active_turn_cleared);
    assert!(interrupted_status);
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
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "approval-1",
        "cargo test",
        None,
        None,
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("selection moves down");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::Reject)
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectUp))
        .await
        .expect("selection moves up");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::PermitOnce)
    ));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("approval confirms");
    assert!(matches!(
        response_rx.try_recv().expect("response ready"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());

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
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
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
async fn command_palette_add_workspace_opens_workspace_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::CommandPaletteOpen))
        .await
        .expect("command palette opens");
    for _ in 0..32 {
        let selected = controller
            .chrome()
            .selected_command()
            .expect("selected command");
        if selected.id == "add-workspace" {
            break;
        }
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move to add workspace command");
    }
    let selected = controller
        .chrome()
        .selected_command()
        .expect("add workspace command");
    assert_eq!(selected.id, "add-workspace");
    assert_eq!(selected.label, "Open workspace access");
    assert_eq!(
        selected.description.as_deref(),
        Some("Manage additional workspace directories")
    );

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("add workspace command runs");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::WorkspaceManager(_))
    ));
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
    let responses = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_responses = std::sync::Arc::clone(&responses);
    let run_turn: TurnDriver = Arc::new(move |_request, channels| {
        let captured_responses = std::sync::Arc::clone(&captured_responses);
        Box::pin(async move {
            let request = ordinary_tool_request("tool-1", "Write", "approved.txt", None);
            channels.send_event(AgentEvent::ApprovalRequested {
                request: request.clone(),
            });
            let (response_tx, response_rx) = oneshot::channel();
            channels
                .approvals
                .send(crate::modes::run::PendingApproval {
                    request,
                    response_tx,
                })
                .expect("approval waiter sent");
            let response = response_rx.await.expect("approval response");
            captured_responses
                .lock()
                .expect("responses lock")
                .push(response);
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("write file");
    controller
        .run_terminal_loop(
            |_app| Ok(()),
            OptionalScriptedEvents {
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

    let captured = responses.lock().expect("responses lock");
    assert_eq!(captured.len(), 1);
    assert!(matches!(
        &captured[0],
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());
    assert!(controller.render_snapshot().contains("approved"));
}

fn controller_with_pending_math_question() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let answers = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_answers = std::sync::Arc::clone(&answers);
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
    let controller = InteractiveController::new(
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
    (controller, answers)
}

#[tokio::test]
async fn event_loop_shows_and_resolves_pending_question_from_running_turn() {
    let (mut controller, answers) = controller_with_pending_math_question();
    let frames = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_frames = std::sync::Arc::clone(&frames);

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
            OptionalScriptedEvents {
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
    let scope = file_write_session_scope("approved.txt");
    let (pending, response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(scope),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Insert('2'))
        .await
        .expect("number shortcut handles approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));
    assert!(!controller.chrome().approval_is_pending());
    assert!(
        controller
            .render_snapshot()
            .contains("Approve writes to this file for this session")
            || controller
                .render_snapshot()
                .to_lowercase()
                .contains("approve")
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
    let scope = shell_session_scope(&["cargo", "test"]);
    let rule = PrefixApprovalRule {
        prefix: vec!["cargo".to_owned(), "test".to_owned()],
        label: "cargo test".to_owned(),
    };
    let (pending, response_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "cargo test",
        Some(scope),
        Some(rule),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Insert('3'))
        .await
        .expect("number shortcut handles prefix approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForPrefix { .. },
            ..
        }
    ));
    assert!(
        controller
            .render_snapshot()
            .contains("Approve commands starting with cargo test")
            || controller.render_snapshot().contains("cargo test")
    );
}

fn controller_with_keyboard_routing_question() -> InteractiveController {
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
}

#[tokio::test]
async fn question_dialog_consumes_keyboard_before_prompt_editing() {
    let mut controller = controller_with_keyboard_routing_question();

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
    let scope = file_write_session_scope("approved.txt");
    let (pending, response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(scope),
    ));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects approval option");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::PermitForSession { .. })
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("enter").expect("valid key")))
        .await
        .expect("enter confirms approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));
    assert_eq!(controller.chrome().prompt().text, "draft");
    assert!(!controller.chrome().approval_is_pending());
}

#[tokio::test]
async fn approval_mouse_wheel_scrolls_transcript_without_moving_selection() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    for index in 0..30 {
        controller
            .transcript_mut()
            .push_status(format!("approval-scroll-row-{index}"));
    }
    controller.transcript_mut().sync_transcript_view(30, 6);
    let (pending, _response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        Some(file_write_session_scope("approved.txt")),
    ));
    controller.register_pending_approval(pending);
    let selected = controller.chrome().approval_selected_action().cloned();

    controller
        .handle_input_event(InputEvent::ScrollUp(3))
        .await
        .expect("wheel scrolls transcript while approval stays focused");

    assert!(transcript_scrollback(&controller) > 0);
    assert_eq!(
        controller.chrome().approval_selected_action(),
        selected.as_ref()
    );
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
    let (pending, response_rx) = make_pending_approval(plan_review_request("tool-1"));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("down").expect("valid key")))
        .await
        .expect("down selects revise option");
    assert!(matches!(
        controller.chrome().approval_selected_action(),
        Some(ApprovalAction::RevisePlan { .. })
    ));

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
    match response_rx.await.expect("approval response") {
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback: Some(feedback),
            ..
        } => assert_eq!(feedback, "no thank"),
        other => panic!("expected revise response, got {other:?}"),
    }
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
    let (pending, response_rx) =
        make_pending_approval(ordinary_tool_request("tool-1", "Write", "denied.txt", None));
    controller.register_pending_approval(pending);

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects approval");

    assert!(matches!(
        response_rx.await.expect("approval response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Escape,
            ..
        }
    ));
    let snapshot = controller.render_snapshot().to_lowercase();
    assert!(
        snapshot.contains("cancel") || snapshot.contains("reject"),
        "snapshot should show cancelled/rejected approval"
    );
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
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("first approval confirms");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitOnce,
            ..
        }
    ));
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|(id, _, _, _)| id),
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
    let (first, _first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

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
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    let (second, _second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel rejects current approval");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Escape,
            ..
        }
    ));
    let snapshot = controller.render_snapshot();
    assert!(snapshot.contains("printf two"));
    assert!(!snapshot.contains("queued:"));
}

#[tokio::test]
async fn approval_interrupt_cancels_all_pending_approvals() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    let (first, first_rx) =
        make_pending_approval(ordinary_shell_request("tool-1", "printf one", None, None));
    let (second, second_rx) =
        make_pending_approval(ordinary_shell_request("tool-2", "printf two", None, None));
    controller.register_pending_approval(first);
    controller.register_pending_approval(second);

    controller
        .handle_input_event(InputEvent::Interrupt)
        .await
        .expect("interrupt cancels pending approvals");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
            ..
        }
    ));
    assert!(matches!(
        second_rx.await.expect("second response"),
        ApprovalResponse::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
            ..
        }
    ));
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
fn replay_session_into_transcript_uses_persisted_skill_invocation_outcome() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::SkillInvocation {
            names: vec!["missing".to_owned()],
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Failed,
            body: "skill `missing` is not available".to_owned(),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    assert!(matches!(
        transcript.transcript().entries(),
        [TranscriptEntry::SkillActivation {
            names,
            source: neo_agent_core::SkillInvocationSource::Auto,
            outcome: neo_agent_core::SkillInvocationOutcome::Failed,
            ..
        }] if names == &["missing".to_owned()]
    ));
}

#[test]
fn replay_session_into_transcript_restores_persisted_shell_command() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::shell_command(
                "printf shell-resume",
                "shell-resume-output",
                "",
                Some(0),
                neo_agent_core::ShellCommandOutcome::Completed,
                false,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replayed shell command")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("printf shell-resume"), "{rendered}");
    assert!(rendered.contains("shell-resume-output"), "{rendered}");
}

#[test]
fn replay_session_into_transcript_restores_aggregate_messages_when_no_detail_events_exist() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("aggregate-user"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("aggregate-assistant")],
                [neo_agent_core::AgentToolCall {
                    id: "aggregate-tool".into(),
                    name: "Read".into(),
                    raw_arguments: r#"{"path":"aggregate.txt"}"#.into(),
                }],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "aggregate-tool",
                "Read",
                [Content::text("aggregate-result")],
                false,
            ),
        },
        AgentEvent::TokenUsage {
            turn: 1,
            usage: neo_agent_core::AgentTokenUsage {
                input_tokens: 1,
                output_tokens: 1,
                input_cache_read_tokens: 0,
                input_cache_write_tokens: 0,
            },
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render aggregate-only replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("aggregate-user"), "{rendered}");
    assert!(rendered.contains("aggregate-assistant"), "{rendered}");
    assert!(rendered.contains("aggregate.txt"), "{rendered}");
    assert!(rendered.contains("aggregate-result"), "{rendered}");
}

#[test]
fn replay_session_into_transcript_prefers_user_display_text() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::user_content_with_display(
                [Content::text("<file path=\"src/main.rs\">snapshot</file>")],
                "review @[main.rs]",
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    assert!(matches!(
        transcript.transcript().entries(),
        [TranscriptEntry::UserMessage { content, .. }] if content == "review @[main.rs]"
    ));
}

#[test]
fn replay_session_into_transcript_does_not_duplicate_text_delta_aggregate_without_finish() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::TextDelta {
            turn: 1,
            text: "truncated-assistant".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("truncated-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render truncated replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        rendered.matches("truncated-assistant").count(),
        1,
        "{rendered}"
    );
}

#[test]
fn rebuild_transcript_sets_workspace_root_before_replaying_instruction_cards() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let workspace = workspace.canonicalize().expect("canonical workspace");
    let config = test_config(&workspace, temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    let nested = workspace.join("crates/neo-tui");
    let epoch = neo_agent_core::instructions::InstructionEpochData {
        agent_id: "main".to_owned(),
        generation: 1,
        outcome: neo_agent_core::instructions::InstructionEpochOutcome::Activated,
        scopes: vec![neo_agent_core::instructions::InstructionScopeData {
            display_path: nested.clone(),
            kind: neo_agent_core::instructions::InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 1_024,
        }],
        selected_bundles: vec![neo_agent_core::instructions::InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 1_024,
            byte_size: 4_096,
            source_count: 1,
            import_count: 0,
            import_paths: Vec::new(),
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: Vec::new(),
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        model_content: Some("SECRET INSTRUCTION BODY".to_owned()),
    };
    let loaded = LoadedSessionTranscript::new(SESSION_A, Vec::new(), Vec::new())
        .with_events([AgentEvent::InstructionEpoch { epoch }]);

    controller.rebuild_transcript_from_session(&loaded);

    let card = controller
        .tui
        .transcript()
        .transcript()
        .entries()
        .iter()
        .find_map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => Some(component.copy_text()),
            _ => None,
        })
        .expect("instruction card");
    let nested_label = format!("{}/**", Path::new("crates").join("neo-tui").display());
    assert!(card.contains(&nested_label), "{card}");
    assert!(!card.contains("<outside-workspace>"), "{card}");
    assert!(!card.contains(&temp.path().display().to_string()), "{card}");
    assert!(!card.contains("SECRET INSTRUCTION BODY"), "{card}");
}

#[test]
fn replay_session_into_transcript_restores_only_retry_exhaustion() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::RetryScheduled {
            turn: 1,
            retry: 1,
            max_retries: 1,
            delay_ms: 500,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: connection reset".to_owned(),
        },
        AgentEvent::RetryStarted {
            turn: 1,
            retry: 1,
            max_retries: 1,
        },
        AgentEvent::RetryResumed { turn: 1, retry: 1 },
        AgentEvent::RetryExhausted {
            turn: 1,
            retries_used: 1,
            error_code: "provider.transport_error".to_owned(),
            message: "transport error: connection reset".to_owned(),
        },
        AgentEvent::Error {
            turn: 1,
            message: "transport error: connection reset".to_owned(),
            code: Some("provider.transport_error".to_owned()),
            retry_after: None,
        },
        AgentEvent::TurnFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        },
        AgentEvent::RunFinished {
            turn: 1,
            stop_reason: StopReason::Error,
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);

    let retry_entries = transcript
        .transcript()
        .entries()
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::RetryStatus { .. }))
        .collect::<Vec<_>>();
    assert_eq!(retry_entries.len(), 1);
    assert!(matches!(
        retry_entries[0],
        TranscriptEntry::RetryStatus { data }
            if data.phase == neo_tui::transcript::entry::RetryPhase::Exhausted
    ));
    assert_eq!(
        retry_entries[0].finalization(),
        neo_tui::primitive::Finalization::Finalized
    );
    assert!(
        transcript
            .transcript()
            .entries()
            .iter()
            .all(|entry| !matches!(entry, TranscriptEntry::Status { .. }))
    );
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render retry replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        rendered.matches("Reconnect failed after 1 retry").count(),
        1,
        "{rendered}"
    );
    for unexpected in ["Reconnecting", "runtime error"] {
        assert!(!rendered.contains(unexpected), "{rendered}");
    }
}

#[test]
fn replay_session_into_transcript_consumes_assistant_coverage_per_occurrence() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("same-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
        AgentEvent::TextDelta {
            turn: 2,
            text: "same-assistant".to_owned(),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("same-assistant")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(100, 20)
        .expect("render repeated aggregate replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("same-assistant").count(), 2, "{rendered}");
}

#[test]
fn replay_session_into_transcript_keeps_uncovered_image_without_repeating_text() {
    let mut transcript = TranscriptPane::new(100, 20);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageStarted {
            turn: 1,
            id: "assistant-image".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "image-caption".to_owned(),
        },
        AgentEvent::MessageFinished {
            turn: 1,
            id: "assistant-image".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [
                    Content::text("image-caption"),
                    Content::Image {
                        mime_type: "image/png".into(),
                        data: neo_agent_core::ImageRef::Base64("aGVsbG8=".into()),
                    },
                ],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let entries = transcript.transcript().entries();
    let assistant_text_count = entries
        .iter()
        .filter(|entry| {
            matches!(
                entry,
                TranscriptEntry::AssistantMessage { content } if content == "image-caption"
            )
        })
        .count();
    let image_count = entries
        .iter()
        .filter(|entry| matches!(entry, TranscriptEntry::Image { .. }))
        .count();

    assert_eq!(assistant_text_count, 1, "{entries:?}");
    assert_eq!(image_count, 1, "{entries:?}");
}

#[test]
fn replay_session_into_transcript_does_not_carry_tool_lifecycle_into_next_assistant() {
    let tool_call = neo_agent_core::AgentToolCall {
        id: "ordered-tool".into(),
        name: "Read".into(),
        raw_arguments: r#"{"path":"ordered.txt"}"#.into(),
    };
    let mut transcript = TranscriptPane::new(110, 24);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::MessageStarted {
            turn: 1,
            id: "assistant-before".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "before-tool".to_owned(),
        },
        AgentEvent::ToolCallStarted {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
        },
        AgentEvent::ToolCallFinished {
            turn: 1,
            tool_call: tool_call.clone(),
        },
        AgentEvent::MessageFinished {
            turn: 1,
            id: "assistant-before".to_owned(),
            stop_reason: StopReason::ToolUse,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("before-tool")],
                [tool_call.clone()],
                StopReason::ToolUse,
            ),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({"path": "ordered.txt"}),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 1,
            id: "ordered-tool".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("ordered-result"),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "ordered-tool",
                "Read",
                [Content::text("ordered-result")],
                false,
            ),
        },
        AgentEvent::MessageStarted {
            turn: 2,
            id: "assistant-after".to_owned(),
        },
        AgentEvent::TextDelta {
            turn: 2,
            text: "after-tool".to_owned(),
        },
        AgentEvent::MessageFinished {
            turn: 2,
            id: "assistant-after".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::assistant(
                [Content::text("after-tool")],
                [],
                StopReason::EndTurn,
            ),
        },
    ]);

    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(110, 24)
        .expect("render ordered replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("before-tool").count(), 1, "{rendered}");
    assert_eq!(rendered.matches("after-tool").count(), 1, "{rendered}");
    assert_eq!(rendered.matches("ordered-result").count(), 1, "{rendered}");
}

#[test]
fn replay_finalizes_dangling_shell_queue_without_restart() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ToolCallStarted {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
        },
        AgentEvent::ToolCallFinished {
            turn: 1,
            tool_call: neo_agent_core::AgentToolCall {
                id: "call-1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"cargo test"}"#.into(),
            },
        },
        AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({"command": "cargo test"}),
        },
    ]);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Interrupted when terminal exited"),
        "{rendered}"
    );
    assert!(!rendered.contains("Queued Bash"), "{rendered}");
}

fn replay_background_bash_request() -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: "background-bash".to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: "sleep 5".to_owned(),
            cwd: None,
        },
        options: vec![
            ApprovalOption {
                label: "Approve once".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            },
            ApprovalOption {
                label: "Reject".to_owned(),
                description: None,
                action: ApprovalAction::Reject,
            },
        ],
    }
}

#[test]
fn replay_renders_resolved_approval_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ApprovalRequested {
            request: request.clone(),
        },
        AgentEvent::ApprovalResolved {
            turn: 1,
            request_id: request.id.clone(),
            resolution: ApprovalResolution::Selected {
                action: ApprovalAction::Reject,
                label: "Reject".to_owned(),
                feedback: None,
            },
        },
    ]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Rejected"), "frame: {rendered}");
    assert!(
        transcript.transcript().entries().iter().all(|entry| {
            !matches!(
                entry,
                TranscriptEntry::ApprovalPrompt(data)
                    if matches!(data.state, ApprovalDisplayState::Pending)
            )
        }),
        "replay must not leave a pending approval card"
    );
}

#[test]
fn replay_marks_unresolved_approval_abandoned_without_reopening_it() {
    let request = replay_background_bash_request();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new())
        .with_events([AgentEvent::ApprovalRequested { request }]);
    let mut transcript = TranscriptPane::new(80, 12);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Abandoned"), "frame: {rendered}");
    assert!(
        transcript.transcript().entries().iter().all(|entry| {
            !matches!(
                entry,
                TranscriptEntry::ApprovalPrompt(data)
                    if matches!(data.state, ApprovalDisplayState::Pending)
            )
        }),
        "unresolved replay cards must be Abandoned, not Pending"
    );
}

#[test]
fn replay_preserves_all_unresolved_approval_cards_as_abandoned() {
    let first = replay_background_bash_request();
    let mut second = replay_background_bash_request();
    second.id = "background-bash-2".to_owned();
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ApprovalRequested { request: first },
        AgentEvent::ApprovalRequested { request: second },
    ]);
    let mut transcript = TranscriptPane::new(80, 12);

    replay_session_into_transcript(&mut transcript, &loaded);

    let approvals = transcript
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) => Some(data),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        approvals.len(),
        2,
        "every requested approval must survive replay"
    );
    assert!(
        approvals
            .iter()
            .all(|data| matches!(data.state, ApprovalDisplayState::Abandoned))
    );
}

#[test]
fn rebuild_transcript_from_session_keeps_context_window_unseeded() {
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
        LoadedSessionTranscript::new("alpha", Vec::new(), [AgentMessage::user_text("hello")]);

    controller.rebuild_transcript_from_session(&loaded);

    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_000_000))
    );
}

#[tokio::test]
async fn load_session_transcript_keeps_context_usage_event_authoritative() {
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

    assert_eq!(
        loaded.messages,
        vec![AgentMessage::user_text("remember this")]
    );
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

fn interleaved_replay_tool_calls() -> Vec<neo_agent_core::AgentToolCall> {
    vec![
        neo_agent_core::AgentToolCall {
            id: "first-tool".into(),
            name: "Read".into(),
            raw_arguments: r#"{"path":"first-order.txt"}"#.into(),
        },
        neo_agent_core::AgentToolCall {
            id: "failed-delegate".into(),
            name: "Delegate".into(),
            raw_arguments: r#"{"task":"failed delegate marker"}"#.into(),
        },
        neo_agent_core::AgentToolCall {
            id: "later-tool".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"later-order-command"}"#.into(),
        },
    ]
}

fn interleaved_replay_prelude_events() -> Vec<AgentEvent> {
    let runtime = neo_agent_core::multi_agent::MultiAgentRuntime::new();
    let running = runtime.start_foreground_delegate_for_test("restored delegate card");
    let delegate_id = running.id.clone();
    let completed = runtime.complete_delegate_for_test(&delegate_id, "done");

    vec![
        AgentEvent::MessageAppended {
            message: AgentMessage::user_text("resume-user"),
        },
        AgentEvent::MessageStarted {
            turn: 1,
            id: "assistant-one".to_owned(),
        },
        AgentEvent::ThinkingStarted {
            turn: 1,
            id: "thinking-one".to_owned(),
        },
        AgentEvent::ThinkingDelta {
            turn: 1,
            text: "resume-thinking".to_owned(),
        },
        AgentEvent::ThinkingFinished {
            turn: 1,
            signature: None,
            redacted: false,
        },
        AgentEvent::TextDelta {
            turn: 1,
            text: "resume-output".to_owned(),
        },
        AgentEvent::MessageFinished {
            turn: 1,
            id: "assistant-one".to_owned(),
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::DelegateStarted {
            turn: 1,
            agent: running,
        },
        AgentEvent::DelegateFinished {
            turn: 1,
            agent: completed,
        },
    ]
}

fn interleaved_replay_execution_events() -> Vec<AgentEvent> {
    vec![
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "first-tool".to_owned(),
            name: "Read".to_owned(),
            arguments: serde_json::json!({ "path": "first-order.txt" }),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "first-tool".to_owned(),
            name: "Read".to_owned(),
            result: neo_agent_core::ToolResult::ok("first result"),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "failed-delegate".to_owned(),
            name: "Delegate".to_owned(),
            arguments: serde_json::json!({ "task": "failed delegate marker" }),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "failed-delegate".to_owned(),
            name: "Delegate".to_owned(),
            result: neo_agent_core::ToolResult::error("failed delegate marker"),
        },
        AgentEvent::ToolExecutionStarted {
            turn: 2,
            id: "later-tool".to_owned(),
            name: "Bash".to_owned(),
            arguments: serde_json::json!({ "command": "later-order-command" }),
        },
        AgentEvent::ToolExecutionFinished {
            turn: 2,
            id: "later-tool".to_owned(),
            name: "Bash".to_owned(),
            result: neo_agent_core::ToolResult::ok("later result"),
        },
    ]
}

fn interleaved_replay_message_events(
    tool_calls: Vec<neo_agent_core::AgentToolCall>,
) -> Vec<AgentEvent> {
    let assistant_message = AgentMessage::assistant(
        [
            Content::thinking("resume-thinking", None, false),
            Content::text("resume-output"),
            Content::text("resume-summary"),
        ],
        tool_calls,
        StopReason::ToolUse,
    );

    vec![
        AgentEvent::TextDelta {
            turn: 2,
            text: "resume-summary".to_owned(),
        },
        AgentEvent::TurnFinished {
            turn: 2,
            stop_reason: StopReason::EndTurn,
        },
        AgentEvent::MessageAppended {
            message: assistant_message,
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "first-tool",
                "Read",
                [Content::text("first result")],
                false,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "failed-delegate",
                "Delegate",
                [Content::text("failed delegate marker")],
                true,
            ),
        },
        AgentEvent::MessageAppended {
            message: AgentMessage::tool_result(
                "later-tool",
                "Bash",
                [Content::text("later result")],
                false,
            ),
        },
    ]
}

async fn write_interleaved_replay_session(config: &AppConfig) {
    let bucket_dir = workspace_sessions_dir(config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    let mut writer = neo_agent_core::session::JsonlSessionWriter::create(&session_path)
        .await
        .expect("create session");
    let mut events = interleaved_replay_prelude_events();
    events.extend(interleaved_replay_execution_events());
    events.extend(interleaved_replay_message_events(
        interleaved_replay_tool_calls(),
    ));
    for event in &events {
        writer.append(event).await.expect("append replay event");
    }
    writer.flush().await.expect("flush session");
}

#[tokio::test]
async fn load_session_replay_preserves_interleaved_visible_entry_order() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    write_interleaved_replay_session(&config).await;

    let loaded = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect("load transcript");
    let mut transcript = TranscriptPane::new(140, 200);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(140, 200)
        .expect("render replayed transcript")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    let markers = [
        "resume-user",
        "resume-thinking",
        "resume-output",
        "restored delegate card",
        "first-order.txt",
        "failed delegate marker",
        "later-order-command",
        "resume-summary",
    ];
    let positions = markers
        .iter()
        .map(|marker| {
            rendered
                .find(marker)
                .unwrap_or_else(|| panic!("missing {marker:?}: {rendered}"))
        })
        .collect::<Vec<_>>();

    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "resume replay order mismatch: {rendered}"
    );
}

#[tokio::test]
async fn load_session_transcript_rejects_oversized_main_wire_before_replay() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let config = test_config(temp.path(), sessions_dir);
    let bucket_dir = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket_dir).expect("create sessions bucket dir");
    let session_path = main_wire_path_for_session(bucket_dir.join(SESSION_A));
    fs::create_dir_all(session_path.parent().expect("main wire parent")).expect("create parent");
    let file = fs::File::create(&session_path).expect("create oversized session");
    file.set_len(crate::modes::sessions::MAX_RESUME_SESSION_BYTES + 1)
        .expect("make sparse oversized session");

    let error = load_session_transcript(SESSION_A.to_owned(), &config)
        .await
        .expect_err("oversized session should be rejected before replay");
    let message = error.to_string();

    assert!(message.contains("too large to resume safely"), "{message}");
    assert!(!message.contains("neo sessions slim"), "{message}");
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
        .with_main_agent_token_usage(usage);

    controller.rebuild_transcript_from_session(&loaded);

    let footer = controller
        .render_snapshot()
        .lines()
        .find(|line| line.contains("ctx "))
        .expect("footer contains context")
        .to_owned();

    assert!(footer.contains("ctx --/512k"));
    assert!(footer.contains("↑33.9k"));
    assert!(footer.contains("↓2.8k"));
    assert!(footer.contains("cache 169.2k read"));
}

#[tokio::test]
async fn load_session_at_startup_sets_terminal_title_from_loaded_title() {
    let mut controller = InteractiveController::new_with_event_driver(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
        PickerCatalogs::default(),
        |session_id| async move {
            Ok(
                LoadedSessionTranscript::new(session_id, Vec::new(), Vec::new())
                    .with_terminal_title("Resume Title"),
            )
        },
    );

    controller
        .load_session_at_startup(SESSION_A)
        .await
        .expect("session loads at startup");

    assert_eq!(controller.chrome().session_label(), SESSION_A);
    assert_eq!(controller.chrome().terminal_title(), "Resume Title");
}

fn session_picker_continuation_controller() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_requests = std::sync::Arc::clone(&requests);
    let controller = InteractiveController::new_with_event_driver(
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
    (controller, requests)
}

#[tokio::test]
async fn event_loop_opens_session_picker_and_continues_selected_transcript() {
    let (mut controller, requests) = session_picker_continuation_controller();

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
        matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "hello")
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
    let workspace_root = test_workspace_root();
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
        workspace_root.clone(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.local_config = Some(test_config(
        &workspace_root,
        workspace_root.join(".neo/sessions"),
    ));

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
    let first_registry = requests[0]
        .instruction_registry
        .as_ref()
        .expect("first turn registry");
    let followup_registry = requests[1]
        .instruction_registry
        .as_ref()
        .expect("followup turn registry");
    assert!(Arc::ptr_eq(first_registry, followup_registry));
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        EventDriverCallbacks {
            run_turn: move |request| {
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
            load_session: |_session_id| async move {
                panic!("fork action should not use the plain session loader");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            fork_session: |parent_id| async move {
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
        },
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+n").expect("valid key")))
        .await
        .expect("ctrl+n forks selected session");

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
        matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "hello")
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

fn selected_model_local_config() -> AppConfig {
    test_config_with_models(
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
    )
}

fn model_picker_submission_controller() -> (
    InteractiveController,
    std::sync::Arc<std::sync::Mutex<Vec<TurnRequest>>>,
) {
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

    controller.local_config = Some(selected_model_local_config());
    (controller, requests)
}

#[tokio::test]
async fn event_loop_opens_model_picker_and_submits_with_selected_model() {
    let (mut controller, requests) = model_picker_submission_controller();

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

async fn capture_configured_interactive_turn_reasoning(
    reasoning: neo_ai::ReasoningSelection,
) -> neo_ai::ReasoningSelection {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.runtime.reasoning = reasoning;
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_request = std::sync::Arc::clone(&captured);
    let mut controller = controller_for_config(&config);
    controller.run_turn = Arc::new(move |request, _channels| {
        let captured_request = std::sync::Arc::clone(&captured_request);
        Box::pin(async move {
            *captured_request.lock().expect("capture request") = Some(request);
            Ok(TurnOutcome::default())
        })
    });

    controller.type_text("hello");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    captured
        .lock()
        .expect("captured request")
        .take()
        .expect("turn request captured")
        .reasoning
}

#[tokio::test]
async fn configured_low_reasoning_reaches_interactive_turn_unchanged() {
    let expected = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::low(),
    };

    let actual = capture_configured_interactive_turn_reasoning(expected.clone()).await;

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn configured_max_reasoning_reaches_interactive_turn_unchanged() {
    let expected = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::max(),
    };

    let actual = capture_configured_interactive_turn_reasoning(expected.clone()).await;

    assert_eq!(actual, expected);
}

#[tokio::test]
async fn configured_budget_reasoning_reaches_interactive_turn_unchanged() {
    let expected = neo_ai::ReasoningSelection::BudgetTokens {
        budget_tokens: 12_000,
    };

    let actual = capture_configured_interactive_turn_reasoning(expected.clone()).await;

    assert_eq!(actual, expected);
}

#[test]
fn model_selection_with_thinking_preserves_current_structured_reasoning() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let expected = neo_ai::ReasoningSelection::BudgetTokens {
        budget_tokens: 12_000,
    };
    config.runtime.reasoning = expected.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(expected.clone());

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "openai/gpt-4.1".to_owned(),
        thinking: true,
        reasoning: expected.clone(),
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        expected
    );
}

#[test]
fn model_selection_persists_reasoning_and_provider_across_reload() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let config_path = config.config_path.clone();
    config.runtime.reasoning = neo_ai::ReasoningSelection::Off;
    let expected_reasoning = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::medium(),
    };
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::Off);

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "anthropic/claude-sonnet-4".to_owned(),
        thinking: true,
        reasoning: expected_reasoning.clone(),
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        expected_reasoning
    );

    let reloaded = crate::config::AppConfig::load(crate::config::ConfigOverrides {
        config_path: Some(config_path),
        project_dir: Some(temp.path().to_path_buf()),
        ..crate::config::ConfigOverrides::default()
    })
    .expect("reload config");
    assert_eq!(reloaded.runtime.reasoning, expected_reasoning);
    assert_eq!(reloaded.default_model, "anthropic/claude-sonnet-4");
    assert_eq!(reloaded.default_provider, "anthropic");
}

#[test]
fn idle_model_and_provider_refresh_replace_bound_workflow_dispatch_client() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.providers.insert(
        "anthropic".to_owned(),
        ProviderConfig {
            display_name: Some("Anthropic test".to_owned()),
            provider_type: Some(neo_ai::ApiType::Anthropic),
            base_url: None,
            api_key: Some("test-key".to_owned()),
            api_key_env: None,
        },
    );
    config.models.insert(
        "selected-model".to_owned(),
        ModelConfig {
            provider: "anthropic".to_owned(),
            model: "claude-test".to_owned(),
            max_context_tokens: Some(200_000),
            max_output_tokens: Some(8_192),
            capabilities: vec!["streaming".to_owned(), "tools".to_owned()],
            reasoning: neo_ai::ReasoningCapability::default(),
            display_name: Some("Selected model".to_owned()),
        },
    );
    let initial_harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let initial_client = initial_harness.client();
    let initial_agent_config = neo_agent_core::AgentConfig::for_model(initial_harness.model())
        .with_workspace_root(temp.path())
        .expect("workspace root");
    config
        .workflow_dispatch_resolver
        .replace(neo_agent_core::runtime::WorkflowDispatchSnapshot {
            config: initial_agent_config,
            model_client: Arc::clone(&initial_client),
            registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
            skills: None,
            process_supervisor: neo_agent_core::ProcessSupervisor::default(),
            context: neo_agent_core::AgentContext::new(),
        })
        .expect("bind initial workflow dispatch");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "selected-model".to_owned(),
        thinking: false,
        reasoning: neo_ai::ReasoningSelection::Off,
    });

    let snapshot = controller
        .local_config
        .as_ref()
        .expect("local config")
        .workflow_dispatch_resolver
        .resolve()
        .expect("updated workflow dispatch");
    assert_eq!(snapshot.config.model.provider.0, "anthropic");
    assert_eq!(snapshot.config.model.model, "claude-test");
    assert!(
        !Arc::ptr_eq(&initial_client, &snapshot.model_client),
        "idle selection must replace the bound workflow client before another tool batch",
    );
    let selected_client = Arc::clone(&snapshot.model_client);

    controller.active_model = None;
    let config_path = controller
        .local_config
        .as_ref()
        .expect("local config")
        .config_path
        .clone();
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("config directory");
    fs::write(
        &config_path,
        r#"
default_model = "provider_refreshed_model"
default_provider = "refreshed-provider"

[providers.refreshed-provider]
type = "anthropic"
base_url = "https://api.anthropic.com"
api_key = "test-key"

[models.provider_refreshed_model]
provider = "refreshed-provider"
model = "provider-refreshed-model"
"#,
    )
    .expect("provider config");
    controller.refresh_config();
    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("refreshed config")
            .default_provider,
        "refreshed-provider"
    );
    assert_eq!(controller.active_session_id(), None);
    let resolved_model = crate::modes::run::resolve_model(
        controller.local_config.as_ref().expect("refreshed config"),
    )
    .expect("refreshed model resolves");
    assert_eq!(resolved_model.provider.0, "refreshed-provider");
    assert_eq!(resolved_model.model, "provider-refreshed-model");
    crate::modes::run::resolve_model_client(
        controller.local_config.as_ref().expect("refreshed config"),
        &resolved_model,
    )
    .expect("refreshed client resolves");

    let refreshed = controller
        .local_config
        .as_ref()
        .expect("refreshed config")
        .workflow_dispatch_resolver
        .resolve()
        .expect("provider-refreshed workflow dispatch");
    assert_eq!(refreshed.config.model.provider.0, "refreshed-provider");
    assert_eq!(refreshed.config.model.model, "provider-refreshed-model");
    assert!(
        !Arc::ptr_eq(&selected_client, &refreshed.model_client),
        "idle provider refresh must replace the matching workflow client",
    );
}

#[tokio::test]
async fn refresh_config_preserves_live_task_and_multi_agent_state() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    fs::write(&config_path, "").expect("write config");
    let mut config = test_config(temp.path(), temp.path().join("sessions"));
    config.config_path = config_path;
    *config.workspace_policy.write().expect("workspace policy") = Some(
        neo_agent_core::WorkspaceAccessPolicy::new(temp.path()).expect("workspace access policy"),
    );
    config
        .background_tasks
        .start_question("question-1".to_owned(), "Pick one".to_owned())
        .await;
    let agent = config
        .multi_agent
        .start_foreground_delegate_for_test("preserve delegate");
    let live_permission_mode = Arc::clone(&config.live_permission_mode);
    let workspace_policy = Arc::clone(&config.workspace_policy);
    let original_shell = config.runtime.shell;
    let original_runtime_root = config.runtime.shell_runtime.runtime_root().to_path_buf();
    fs::write(
        &config.config_path,
        "[runtime.shell]\nmax_active_commands = 3\n",
    )
    .expect("write refreshed config");

    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.live_permission_mode = Arc::clone(&config.live_permission_mode);
    controller.workspace_policy = Arc::clone(&config.workspace_policy);
    controller.local_config = Some(config);
    controller.set_permission_mode(PermissionMode::Yolo);

    controller.refresh_config();

    let reloaded = controller.local_config.as_ref().expect("reloaded config");
    assert_eq!(reloaded.background_tasks.list(false, 10).await.len(), 1);
    assert!(reloaded.multi_agent.snapshot(&agent.id).is_some());
    assert!(Arc::ptr_eq(
        &reloaded.live_permission_mode,
        &live_permission_mode
    ));
    assert!(Arc::ptr_eq(&reloaded.workspace_policy, &workspace_policy));
    assert_eq!(reloaded.permission_mode, PermissionMode::Yolo);
    // Live config refresh preserves the running ShellRuntime; shell-limit file
    // changes take effect on the next Neo process start.
    assert_eq!(reloaded.runtime.shell, original_shell);
    assert_eq!(
        reloaded.runtime.shell_runtime.runtime_root(),
        original_runtime_root.as_path()
    );
    assert_eq!(
        *reloaded
            .live_permission_mode
            .read()
            .expect("live permission mode"),
        PermissionMode::Yolo
    );
    assert!(reloaded.project_trusted);
    assert_eq!(
        reloaded.project_trust,
        crate::trust::ProjectTrustState::NotRequired
    );
}

#[test]
fn model_selection_without_thinking_sets_reasoning_off() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.runtime.reasoning = neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::max(),
    };
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path().to_path_buf(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::Effort {
        effort: neo_ai::ReasoningEffort::max(),
    });

    controller.apply_model_selection(&neo_tui::dialogs::ModelSelection {
        alias: "openai/gpt-4.1".to_owned(),
        thinking: false,
        reasoning: neo_ai::ReasoningSelection::Off,
    });

    assert_eq!(
        controller
            .local_config
            .as_ref()
            .expect("local config")
            .runtime
            .reasoning,
        neo_ai::ReasoningSelection::Off
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

#[tokio::test]
async fn slash_model_opens_picker_when_no_config_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.config_file_exists = false;
    config.models.clear();
    config.providers.clear();
    let mut controller = controller_for_config(&config);

    controller.type_text("/model");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/model submits");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::TabbedModelSelector(_))
    ));
}

#[tokio::test]
async fn slash_provider_opens_picker_when_no_config_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.config_file_exists = false;
    config.providers.clear();
    config.models.clear();
    let mut controller = controller_for_config(&config);

    controller.type_text("/provider");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/provider submits");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::ProviderManager(_))
    ));
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
    assert_eq!(loaded.terminal_title.as_deref(), Some("Alpha Session"));
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

    // Seed parent metadata so we can verify it is inherited by the fork.
    SessionMetadataStore::new(&bucket_dir)
        .record_activity(
            SESSION_A,
            Some("/fake/workspace".to_owned()),
            Some("what is neo?".to_owned()),
            "1000.000000000Z".to_owned(),
        )
        .expect("record parent activity");
    SessionMetadataStore::new(&bucket_dir)
        .record_title(
            SESSION_A,
            "Intro to neo".to_owned(),
            Some("test-model".to_owned()),
            "1000.000000000Z".to_owned(),
        )
        .expect("record parent title");

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
    // Fork inherits parent title with [fork] prefix.
    assert_eq!(
        child.title.as_deref(),
        Some("[fork] Intro to neo"),
        "child title should be [fork]-prefixed parent title"
    );
    // Fork inherits parent workspace and last_user_prompt.
    assert_eq!(
        child.workspace.as_deref(),
        Some("/fake/workspace"),
        "child inherits parent workspace"
    );
    assert_eq!(
        child.last_user_prompt.as_deref(),
        Some("what is neo?"),
        "child inherits parent last_user_prompt"
    );
    // Fork updated_at is set (not empty / not epoch zero).
    let child_ts = child.updated_at.as_deref().unwrap_or("");
    assert!(
        !child_ts.is_empty() && child_ts != "0" && child_ts != "0.000000000Z",
        "child updated_at should be a real timestamp, got: {child_ts}"
    );
}

fn add_indexed_session_fixture(
    sessions_dir: &Path,
    project: &Path,
    session_id: &str,
    prompt: &str,
    timestamp: &str,
) -> AppConfig {
    fs::create_dir_all(project).expect("create project");
    let config = test_config(project, sessions_dir.to_path_buf());
    let bucket = workspace_sessions_dir(&config);
    fs::create_dir_all(&bucket).expect("create session bucket");
    write_main_wire(
        &bucket,
        session_id,
        r#"{"MessageAppended":{"message":{"User":{"content":[{"Text":{"text":"hello"}}]}}}}"#,
    );
    SessionMetadataStore::new(&bucket)
        .record_activity(
            session_id,
            Some(project.display().to_string()),
            Some(prompt.to_owned()),
            timestamp.to_owned(),
        )
        .expect("record session metadata");
    neo_agent_core::session::SessionIndex::new(sessions_dir.parent().expect("neo home"))
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: session_id.to_owned(),
            session_dir: bucket,
            workdir: project.to_path_buf(),
        })
        .expect("index session");
    config
}

#[tokio::test]
async fn session_picker_ctrl_a_toggles_scope() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let project_a = temp.path().join("project_a");
    let config_a =
        add_indexed_session_fixture(&sessions_dir, &project_a, SESSION_A, "alpha prompt", "200");

    let project_b = temp.path().join("project_b");
    add_indexed_session_fixture(&sessions_dir, &project_b, SESSION_B, "beta prompt", "100");

    let mut controller = controller_for_config(&config_a);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
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
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a toggles scope");
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
async fn session_picker_ctrl_a_empty_target_scope_can_toggle_back() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sessions_dir = temp.path().join(".neo/sessions");
    fs::create_dir_all(&sessions_dir).expect("create sessions dir");

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

    let mut controller = controller_for_config(&config_a);

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+r").expect("valid key")))
        .await
        .expect("ctrl+r opens session picker");
    let overlay = controller.chrome().focused_overlay().expect("picker open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope on open"
    );

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a switches to empty all-sessions scope");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("empty picker stays open");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::All
        ),
        "all scope remains toggleable when empty"
    );
    assert!(transcript_has_status(
        &controller,
        "No sessions in all sessions. Press Ctrl+A again to switch back to current workspace."
    ));

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+a").expect("valid key")))
        .await
        .expect("ctrl+a toggles back to workspace scope");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("workspace picker opens again");
    assert!(
        matches!(
            &overlay.kind,
            OverlayKind::SessionPicker(p) if p.scope() == SessionPickerScope::Workspace
        ),
        "workspace scope after toggling back"
    );
    let snapshot = controller.render_snapshot();
    assert!(
        snapshot.to_lowercase().contains("alpha"),
        "workspace scope should show alpha after toggling back: {snapshot}"
    );
}

#[tokio::test]
async fn cross_workspace_picker_emits_parseable_product_resume_command() {
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

    let expected = format!("neo resume {SESSION_A}");
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(&controller, &expected));
    let parsed = crate::cli::Cli::try_parse_from(expected.split_whitespace())
        .expect("resume command should be parseable by Neo");
    assert!(matches!(
        parsed.command,
        Some(crate::cli::Command::Resume { session_id: Some(id) }) if id == SESSION_A
    ));
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
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
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
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/compact"), "missing /compact: {values:?}");
}

#[test]
fn slash_completions_include_add_workspace_command() {
    let completions =
        prompt_completions(&test_workspace_root(), "/", None, true).expect("completions resolve");
    let add_workspace = completions
        .iter()
        .find(|item| item.value == "/add-workspace")
        .expect("missing /add-workspace completion");

    assert_eq!(add_workspace.label, "/add-workspace");
    assert_eq!(
        add_workspace.description.as_deref(),
        Some("Manage additional workspace directories")
    );
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
    let (pending, response_rx) = make_pending_approval(plan_review_request("exit-plan-1"));
    controller.register_pending_approval(pending);

    // Select "Reject with feedback" (index 1) and enter feedback, then confirm.
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
    match response_rx.await.expect("response") {
        ApprovalResponse::Selected {
            action: ApprovalAction::RevisePlan { .. },
            feedback: Some(feedback),
            ..
        } => assert_eq!(feedback, "r"),
        other => panic!("expected revise with feedback, got {other:?}"),
    }
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
    let (first, first_rx) = make_pending_approval(ordinary_shell_request(
        "tool-1",
        "printf one",
        Some(shell_session_scope(&["printf", "one"])),
        None,
    ));
    controller.register_pending_approval(first);

    // Select "Approve for this session" (index 1) and confirm.
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("move to always-approve");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm always-approve");

    assert!(matches!(
        first_rx.await.expect("first response"),
        ApprovalResponse::Selected {
            action: ApprovalAction::PermitForSession { .. },
            ..
        }
    ));

    // Tool-session approval is scoped by the runtime. The TUI must not
    // turn one approval into a global bypass for later ask prompts.
    let (second, mut second_rx) =
        make_pending_approval(ordinary_tool_request("tool-2", "Write", "later.txt", None));
    controller.register_pending_approval(second);
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

#[tokio::test]
async fn add_provider_picker_includes_custom_endpoint() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.open_add_provider_picker();

    let visible = controller
        .tui
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Known third-party provider"), "{visible}");
    assert!(visible.contains("Custom endpoint"), "{visible}");
    assert!(visible.contains("Custom registry (api.json)"), "{visible}");
    let known = visible
        .find("Known third-party provider")
        .expect("known provider row");
    let custom_endpoint = visible
        .find("Custom endpoint")
        .expect("custom endpoint row");
    let custom_registry = visible
        .find("Custom registry (api.json)")
        .expect("custom registry row");
    assert!(known < custom_endpoint, "{visible}");
    assert!(custom_endpoint < custom_registry, "{visible}");
}

#[tokio::test]
async fn add_provider_custom_endpoint_choice_opens_wizard() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.open_add_provider_picker();
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select custom endpoint row");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("open custom endpoint wizard");

    assert!(matches!(
        controller
            .chrome()
            .focused_overlay()
            .map(|overlay| &overlay.kind),
        Some(OverlayKind::CustomEndpointWizard(_))
    ));
    let visible = controller
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Custom Endpoint 1/4"), "{visible}");
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
        workspace_policy: Arc::new(RwLock::new(None)),
        defaults: Defaults {
            mode: "interactive".to_owned(),
        },
        runtime: RuntimeConfig::default(),
        background_tasks: neo_agent_core::BackgroundTaskManager::new(),
        workflow_capability: neo_agent_core::workflow::WorkflowCapability::default(),
        workflow_runtime: neo_agent_core::workflow::WorkflowRuntime::new(
            neo_agent_core::workflow::WorkflowLimits::default(),
        ),
        workflow_dispatch_resolver: neo_agent_core::runtime::WorkflowDispatchResolver::default(),
        multi_agent: neo_agent_core::multi_agent::MultiAgentRuntime::new(),
        tui: TuiConfig::default(),
        theme: crate::themes::ResolvedTheme::default(),
        mcp: McpConfig::default(),
        prompt_templates: Vec::new(),
        system_prompt_file: None,
        extra_skill_dirs: Vec::new(),
        skill_path: Vec::new(),
        project_trusted: true,
        project_trust: crate::trust::ProjectTrustState::NotRequired,
        project_dir: project_dir.to_path_buf(),
        config_path: project_dir.join(".neo/config.toml"),
        config_file_exists: true,
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

#[test]
fn instruction_registry_cache_is_scoped_by_session_id() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_A.to_owned());
    let session_a = controller
        .instruction_registry_for_turn()
        .expect("session A registry")
        .expect("configured controller registry");
    let session_a_again = controller
        .instruction_registry_for_turn()
        .expect("session A registry")
        .expect("configured controller registry");
    assert!(Arc::ptr_eq(&session_a, &session_a_again));

    controller.set_active_session_id(SESSION_B.to_owned());
    let session_b = controller
        .instruction_registry_for_turn()
        .expect("session B registry")
        .expect("configured controller registry");
    assert!(!Arc::ptr_eq(&session_a, &session_b));

    controller.active_session_id = None;
    let new_session = controller
        .instruction_registry_for_turn()
        .expect("new session registry")
        .expect("configured controller registry");
    let another_new_session = controller
        .instruction_registry_for_turn()
        .expect("another new session registry")
        .expect("configured controller registry");
    assert!(!Arc::ptr_eq(&new_session, &another_new_session));

    controller.set_active_session_id(SESSION_A.to_owned());
    let session_a_after_switch = controller
        .instruction_registry_for_turn()
        .expect("session A registry after switch")
        .expect("configured controller registry");
    assert!(Arc::ptr_eq(&session_a, &session_a_after_switch));
}

#[test]
fn persisted_workflow_events_apply_only_to_matching_session_generation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.set_active_session_id(SESSION_A.to_owned());
    let first_generation_a = controller.workflow_event_generation;
    controller.set_active_session_id(SESSION_B.to_owned());
    controller.set_active_session_id(SESSION_A.to_owned());
    let current_generation_a = controller.workflow_event_generation;
    let (persisted, workflow_events) = tokio::sync::mpsc::unbounded_channel();
    controller.workflow_events = workflow_events;
    let error_event = |message: &str| AgentEvent::Error {
        turn: 3,
        message: message.to_owned(),
        code: None,
        retry_after: None,
    };
    persisted
        .send(crate::modes::run::PersistedSessionWorkflowEvent::Event(
            Box::new(crate::modes::run::SessionWorkflowEvent {
                session_id: SESSION_A.to_owned(),
                generation: first_generation_a,
                event: error_event("stale session A generation must stay hidden"),
            }),
        ))
        .expect("session A delivery");
    persisted
        .send(crate::modes::run::PersistedSessionWorkflowEvent::Event(
            Box::new(crate::modes::run::SessionWorkflowEvent {
                session_id: SESSION_A.to_owned(),
                generation: current_generation_a,
                event: error_event("current session A generation is visible"),
            }),
        ))
        .expect("session B delivery");
    let entries_before = controller.tui.transcript().transcript().entries().len();

    controller.drain_workflow_events();

    let entries_after = controller.tui.transcript().transcript().entries().len();
    assert_eq!(entries_after, entries_before + 1);
}

#[tokio::test]
async fn workflow_event_routes_are_retained_across_switch_and_released_on_exit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_A.to_owned());
    controller.set_active_session_id(SESSION_B.to_owned());
    let generation_b = controller.workflow_event_generation;
    controller.set_active_session_id(SESSION_A.to_owned());
    assert_eq!(controller.workflow_event_routes.len(), 2);
    assert_eq!(controller.workflow_approval_routes.len(), 2);
    assert!(controller.workflow_event_generation > generation_b);

    controller.finalize_terminal_exit();

    assert!(controller.workflow_event_routes.is_empty());
    assert!(controller.workflow_approval_routes.is_empty());
    assert!(controller.workflow_event_ingress.is_none());
}

#[tokio::test]
async fn inactive_session_workflow_approval_is_backlogged_until_reactivated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);

    controller.set_active_session_id(SESSION_B.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "session-a-approval",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("session A delivery");

    assert_eq!(controller.drain_workflow_approvals(), FrameRequest::None);
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(
        controller
            .pending_approvals
            .contains_key("session-a-approval")
    );
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.0),
        Some("session-a-approval")
    );
    assert!(!controller.workflow_approval_backlog.contains_key(SESSION_A));
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn registered_workflow_approval_is_parked_and_restored_across_session_navigation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "parked-approval",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("session A delivery");
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );
    assert!(controller.chrome().approval_is_pending());

    controller.set_active_session_id(SESSION_B.to_owned());

    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(controller.pending_approvals.contains_key("parked-approval"));
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|value| value.0),
        Some("parked-approval")
    );
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
}

async fn spawn_workflow_approval_invocation(
    config: &AppConfig,
    session_id: &str,
) -> (
    neo_agent_core::workflow::WorkflowHandle,
    tokio::task::JoinHandle<
        Result<
            neo_agent_core::workflow::WorkflowInvocationOutcome,
            neo_agent_core::workflow::WorkflowError,
        >,
    >,
    PathBuf,
) {
    let session_directory = workspace_sessions_dir(config).join(session_id);
    let runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let handle = runtime
        .create_run(
            &session_directory,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "approval-stop".to_owned(),
                description: "approval stop cleanup".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "verify".to_owned(),
                    description: "verify".to_owned(),
                }],
                script: "neo.phase('verify')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
            },
        )
        .await
        .expect("create workflow");
    let harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let agent_config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(&config.project_dir)
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Ask)
        .with_workflow_dispatch_resolver(config.workflow_dispatch_resolver.clone());
    let dispatch = neo_agent_core::runtime::WorkflowDispatchHandle {
        config: agent_config,
        model_client: harness.client(),
        registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
        process_supervisor: neo_agent_core::ProcessSupervisor::default(),
        context: neo_agent_core::AgentContext::new(),
    };
    let invocation_handle = handle.clone();
    let invocation = tokio::spawn(async move {
        invocation_handle
            .invoke(
                0,
                neo_agent_core::workflow::WorkflowInvocationKind::VerifyCommand,
                serde_json::json!({"command": "sudo --version"}),
                false,
                move |context| async move {
                    dispatch
                        .run_one(
                            context,
                            "Bash",
                            serde_json::json!({"command": "sudo --version"}),
                        )
                        .await
                },
            )
            .await
    });
    let journal_path = session_directory
        .join("workflows")
        .join(&handle.run_id.0)
        .join("journal.jsonl");
    (handle, invocation, journal_path)
}

fn assert_cancelled_workflow_invocation_journal(journal_path: &Path) {
    let records = neo_agent_core::workflow::read_journal(journal_path).expect("read journal");
    assert!(records.iter().any(|record| {
        matches!(
            record,
            neo_agent_core::workflow::JournalRecord::InvocationFinished { outcome, .. }
                if outcome.status == neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
        )
    }));
}

#[tokio::test]
async fn workflow_stop_before_approval_delivery_drain_removes_closed_responder() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (handle, invocation, journal_path) =
        spawn_workflow_approval_invocation(&config, SESSION_A).await;

    tokio::time::timeout(Duration::from_secs(5), async {
        while controller.workflow_approvals.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval delivery reaches controller queue");
    handle
        .stop(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("stop workflow");
    let outcome = invocation
        .await
        .expect("invocation task")
        .expect("workflow invocation");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
    );

    controller.drain_workflow_approvals();

    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
    assert!(controller.pending_approvals.is_empty());
    assert!(controller.workflow_approval_backlog.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_cancelled_workflow_invocation_journal(&journal_path);
}

#[tokio::test]
async fn workflow_stop_after_modal_registration_clears_chrome_and_resolves_transcript() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (handle, invocation, journal_path) =
        spawn_workflow_approval_invocation(&config, SESSION_A).await;

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            controller.drain_workflow_approvals();
            if controller.pending_approvals.len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("approval modal is registered");
    let request_id = controller
        .pending_approvals
        .keys()
        .next()
        .expect("pending approval")
        .clone();
    assert!(controller.chrome().approval_is_pending());
    assert!(matches!(
        controller
            .tui
            .transcript()
            .transcript()
            .approval(&request_id)
            .expect("approval transcript")
            .state,
        ApprovalDisplayState::Pending
    ));

    handle
        .stop(neo_agent_core::workflow::WorkflowActor::Human)
        .await
        .expect("stop workflow");
    let outcome = invocation
        .await
        .expect("invocation task")
        .expect("workflow invocation");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
    );
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );

    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Cancelled
    );
    assert!(!controller.pending_approvals.contains_key(&request_id));
    assert!(!controller.chrome().approval_is_pending());
    assert!(matches!(
        controller
            .tui
            .transcript()
            .transcript()
            .approval(&request_id)
            .expect("resolved approval transcript")
            .state,
        ApprovalDisplayState::Resolved(ApprovalResolution::Cancelled {
            reason: ApprovalCancelReason::Interrupt,
        })
    ));
    assert_cancelled_workflow_invocation_journal(&journal_path);
}

#[tokio::test]
async fn idle_workflow_approval_uses_current_controller_route_without_model_turn() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    controller.set_active_session_id(SESSION_B.to_owned());
    controller.set_active_session_id(SESSION_A.to_owned());

    let session_directory = workspace_sessions_dir(&config).join(SESSION_A);
    let harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let stale_handler_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called = Arc::clone(&stale_handler_called);
    let agent_config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(temp.path())
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Ask)
        .with_workflow_dispatch_resolver(config.workflow_dispatch_resolver.clone())
        .with_async_approval_handler(move |request| {
            let called = Arc::clone(&called);
            async move {
                called.store(true, std::sync::atomic::Ordering::SeqCst);
                ApprovalResponse::Cancelled {
                    request_id: request.id,
                    reason: ApprovalCancelReason::SessionEnded,
                }
            }
        });
    let handle = neo_agent_core::runtime::WorkflowDispatchHandle {
        config: agent_config,
        model_client: harness.client(),
        registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
        process_supervisor: neo_agent_core::ProcessSupervisor::default(),
        context: neo_agent_core::AgentContext::new(),
    };
    let dispatch = tokio::spawn(async move {
        handle
            .run_one(
                neo_agent_core::workflow::WorkflowInvocationContext {
                    invocation_id: "idle-workflow-bash".to_owned(),
                    cancel_token: tokio_util::sync::CancellationToken::new(),
                },
                "Bash",
                serde_json::json!({"command": "sudo --version"}),
            )
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            controller.drain_workflow_approvals();
            if controller
                .pending_approvals
                .contains_key("idle-workflow-bash")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("idle workflow approval reaches controller");
    controller.resolve_approval(ApprovalResponse::Selected {
        request_id: "idle-workflow-bash".to_owned(),
        action: ApprovalAction::Reject,
        feedback: None,
    });

    let outcome = dispatch.await.expect("workflow dispatch task");
    assert_eq!(
        outcome.status,
        neo_agent_core::workflow::WorkflowOutcomeStatus::Denied
    );
    assert!(!stale_handler_called.load(std::sync::atomic::Ordering::SeqCst));
    assert!(
        harness.requests().is_empty(),
        "approval must not run a model turn"
    );
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

#[tokio::test]
async fn turn_request_carries_workspace_policy() {
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_policy = std::sync::Arc::clone(&captured);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let captured_policy = std::sync::Arc::clone(&captured_policy);
            async move {
                *captured_policy.lock().expect("capture policy") =
                    Some(std::sync::Arc::clone(&request.workspace_policy));
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

    let expected_policy = std::sync::Arc::clone(&controller.workspace_policy);
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
    let captured_policy = captured.expect("workspace policy was forwarded to the driver");
    assert!(std::sync::Arc::ptr_eq(&captured_policy, &expected_policy));
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
    controller.tui.chrome_mut().set_context_window(Some(
        ContextWindow::new(1_000_000)
            .with_used_tokens(57_000)
            .with_projected_tokens(Some(61_000)),
    ));

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
    assert_eq!(
        controller.chrome().context_window(),
        Some(ContextWindow::new(1_000_000))
    );
}

#[tokio::test]
async fn slash_new_parks_workflow_approval_until_origin_session_reactivated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());
    let (pending, mut response_rx) = make_pending_approval(ordinary_shell_request(
        "workflow-before-new",
        "sudo --version",
        None,
        None,
    ));
    controller
        .workflow_approval_ingress
        .send(SessionWorkflowApproval {
            session_id: SESSION_A.to_owned(),
            pending,
        })
        .expect("workflow approval delivery");
    assert_eq!(
        controller.drain_workflow_approvals(),
        FrameRequest::Immediate
    );
    assert!(controller.chrome().approval_is_pending());

    controller.start_new_session_from_slash();

    assert_eq!(controller.active_session_id(), None);
    assert!(controller.pending_approvals.is_empty());
    assert!(!controller.chrome().approval_is_pending());
    assert_eq!(controller.workflow_approval_backlog[SESSION_A].len(), 1);
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));

    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(
        controller
            .pending_approvals
            .contains_key("workflow-before-new")
    );
    assert_eq!(
        controller
            .chrome()
            .approval_selection()
            .map(|selection| selection.0),
        Some("workflow-before-new")
    );
    assert!(matches!(
        response_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
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
async fn workflow_capability_reaches_agent_config_and_clear_revokes_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join("sessions"));
    let mut controller = controller_for_config(&config);

    controller.type_text("/workflow");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/workflow submits");
    assert!(config.workflow_capability.is_available());

    controller.refresh_config();
    let refreshed = controller.local_config.as_ref().expect("refreshed config");
    assert!(refreshed.workflow_capability.is_available());
    assert!(
        refreshed
            .workflow_dispatch_resolver
            .shares_state_with(&config.workflow_dispatch_resolver),
        "config refresh must retain the session workflow resolver",
    );
    let agent_config = crate::modes::run::agent_config_for_app(
        neo_ai::ModelSpec {
            provider: neo_ai::ProviderId("test-provider".to_owned()),
            model: "test-model".to_owned(),
            api: neo_ai::ApiKind::Local,
            capabilities: neo_ai::ModelCapabilities::tool_chat(),
        },
        refreshed,
        None,
        None,
    )
    .expect("agent config");
    assert!(agent_config.workflow_capability.is_available());
    assert!(
        agent_config
            .workflow_dispatch_resolver
            .shares_state_with(&refreshed.workflow_dispatch_resolver),
        "each fresh turn config must use the session workflow resolver",
    );

    controller.type_text("/clear");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/clear submits");
    assert!(!agent_config.workflow_capability.is_available());
}

#[tokio::test]
async fn workflow_slash_arguments_do_not_grant_capability() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join("sessions"));
    let mut controller = controller_for_config(&config);

    controller.type_text("/workflow anything");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash submits");

    assert!(!config.workflow_capability.is_available());
}

#[tokio::test]
async fn workflow_capability_is_revoked_only_when_session_identity_changes() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join("sessions"));
    let mut controller = controller_for_config(&config);
    controller.set_active_session_id(SESSION_A.to_owned());

    config.workflow_capability.grant();
    controller.set_active_session_id(SESSION_A.to_owned());
    assert!(config.workflow_capability.inspect());

    controller.set_active_session_id(SESSION_B.to_owned());
    assert!(!config.workflow_capability.inspect());

    config.workflow_capability.grant();
    controller.clear_active_session_id();
    assert!(!config.workflow_capability.inspect());
}

#[tokio::test]
async fn workflow_capability_survives_first_session_materialization() {
    let temp = tempfile::tempdir().expect("tempdir");
    let config = test_config(temp.path(), temp.path().join("sessions"));
    let mut controller = controller_for_config(&config);
    assert_eq!(controller.active_session_id(), None);

    config.workflow_capability.grant();
    controller.set_active_session_id(SESSION_A.to_owned());

    assert!(config.workflow_capability.inspect());
}

#[tokio::test]
async fn slash_clear_does_not_request_terminal_scrollback_purge() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    let mut terminal = InlineTerminal::for_test(80, 24);

    let before_clear = controller.tui.render_terminal_frame(80, 24);
    terminal
        .render_to(&mut Vec::new(), &before_clear)
        .expect("render initial terminal frame");
    controller.tui.acknowledge_history(&before_clear);

    controller.type_text("/clear");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/clear submits");

    let after_clear = controller.tui.render_terminal_frame(80, 24);
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &after_clear)
        .expect("render cleared terminal frame");
    let output = String::from_utf8(output).expect("terminal output is UTF-8");

    assert!(!output.contains("\x1b[2J"));
    assert!(!output.contains("\x1b[3J"));
}

#[test]
fn terminal_exit_commits_interrupted_live_entries_before_leave() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.apply_turn_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "write-1".to_owned(),
        name: "Write".to_owned(),
        arguments: serde_json::json!({"path": "notes.txt", "content": "draft"}),
    });
    controller.tui.transcript_mut().start_assistant_message();
    controller
        .tui
        .transcript_mut()
        .append_assistant_delta("unfinished assistant text");
    let mut terminal = InlineTerminal::for_test(80, 24);
    let initial = controller.tui.render_terminal_frame(80, 24);
    terminal
        .render_to(&mut Vec::new(), &initial)
        .expect("render initial live frame");
    controller.tui.acknowledge_history(&initial);

    let mut final_frame = None;
    controller
        .finalize_and_render_terminal_exit(|tui| {
            final_frame = Some(tui.render_terminal_frame(80, 24));
            Ok(())
        })
        .expect("finalize and render terminal exit");
    let final_frame = final_frame.expect("final exit frame");
    let history = final_frame
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");
    let live = final_frame
        .live
        .iter()
        .map(|line| neo_tui::primitive::strip_ansi(line))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        history.contains("unfinished assistant text"),
        "history:\n{history}\nlive:\n{live}"
    );
    assert!(history.contains("Write"), "history:\n{history}");
    assert!(!live.contains("unfinished assistant text"));
    let mut output = Vec::new();
    terminal
        .render_to(&mut output, &final_frame)
        .expect("commit interrupted frame");
    controller.tui.acknowledge_history(&final_frame);
    terminal.leave(&mut output).expect("leave terminal");
    let output = String::from_utf8(output).expect("terminal output is UTF-8");
    assert!(output.contains("unfinished assistant text"));
    assert!(!output.contains("\x1b[3J"));
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
async fn slash_new_preserves_model_permission_reasoning_and_plan_mode() {
    let (mut controller, _requests) = controller_with_session_for_new_tests();
    // Configure preserved state.
    controller.set_permission_mode(PermissionMode::Yolo);
    controller.set_current_reasoning(neo_ai::ReasoningSelection::On);
    controller.set_plan_mode_from_user(true);

    controller.type_text("/new");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/new submits");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert_eq!(
        controller.current_reasoning,
        neo_ai::ReasoningSelection::On,
        "structured reasoning selection is preserved across /new"
    );
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
async fn permission_picker_keeps_working_status_while_turn_is_running() {
    let mut controller = running_turn_controller().await;

    controller.open_permission_picker();
    for _ in 0..2 {
        controller
            .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
            .await
            .expect("move permission selection");
    }
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("select permission mode");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Yolo);
    assert_eq!(
        controller.chrome().working_label().as_deref(),
        Some("working · esc interrupt")
    );

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
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
fn slash_completions_include_new_clear_and_workflow() {
    let items = session_completion_items(None);
    let values: Vec<&str> = items.iter().map(|item| item.value.as_str()).collect();
    assert!(values.contains(&"/new"), "completions include /new");
    assert!(values.contains(&"/clear"), "completions include /clear");
    assert!(
        values.contains(&"/workflow"),
        "completions include /workflow"
    );
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

#[tokio::test]
async fn command_palette_new_session_revokes_capability_before_session_materialization() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "new",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    assert_eq!(controller.active_session_id(), None);
    controller.workflow_capability.grant();

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

    assert!(!controller.workflow_capability.inspect());
    assert_eq!(controller.active_session_id(), None);
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
async fn slash_init_submits_generated_workflow_prompt() {
    let (mut controller, requests) = controller_with_session_for_new_tests();

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("init command submits");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let requests = requests.lock().expect("requests captured");
    assert_eq!(requests.len(), 1);
    let request = requests.first().expect("captured request");
    let prompt = request
        .prompt
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(prompt.contains("<system-reminder>"), "{prompt}");
    assert!(
        prompt.contains("You are running Neo's /init workflow."),
        "{prompt}"
    );
    assert!(prompt.contains("排除掉 generated 目录"), "{prompt}");
    assert!(prompt.contains("Target file:"), "{prompt}");
    assert!(prompt.contains("AGENTS.md"), "{prompt}");
    assert_eq!(request.prompt_origin, MessageOrigin::injection("init"));
}

#[tokio::test]
async fn slash_init_is_not_persisted_to_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));

    let mut controller = controller_with_history_store(store);

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("init command handled");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        persisted.is_empty() && !persisted.contains("/init") && !persisted.contains("generated"),
        "generated /init prompt must not be persisted: {persisted}"
    );
    drop(dir);
}

#[tokio::test]
async fn slash_init_in_auto_opens_generic_preflight_without_starting_turn() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/init opens preflight");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}

#[tokio::test]
async fn slash_self_evo_without_args_in_auto_opens_required_preflight() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}

#[tokio::test]
async fn self_evo_preflight_switch_to_ask_starts_skill_workflow() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    assert_eq!(controller.pending_local_user_message_to_suppress, None);
    assert_eq!(controller.pending_skill_user_message_to_suppress, None);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("self-evo"), "{prompt}");
    assert!(
        prompt.contains("Ask me which session scope to distill"),
        "{prompt}"
    );
}

#[tokio::test]
async fn slash_self_evo_with_scope_in_auto_skips_preflight() {
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
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo 7");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo scope starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"self-evo\""),
        "{skill_context}"
    );
    assert!(skill_context.contains("SELF EVO: 7"), "{skill_context}");
}

#[tokio::test]
async fn slash_create_skill_without_instruction_in_auto_opens_required_preflight() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_some());
}

#[tokio::test]
async fn slash_create_skill_with_instruction_in_auto_skips_preflight() {
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
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:create-skill make a rust panic review skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill instruction starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    let skill_context = requests[0].skill_context.as_deref().expect("skill context");
    assert!(
        skill_context.contains("<neo-skill-loaded name=\"create-skill\""),
        "{skill_context}"
    );
    assert!(
        skill_context.contains("CREATE SKILL: make a rust panic review skill"),
        "{skill_context}"
    );
}

#[tokio::test]
async fn multiple_required_preflight_skills_return_status_without_turn() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);
    controller.skill_store = Some(skill_store_with_interactive_preflight_skills());

    controller.type_text("/skill:self-evo /skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "Run one interactive skill workflow at a time"
    ));
}

#[tokio::test]
async fn init_preflight_switch_to_ask_starts_workflow() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(
        prompt.contains("User instruction after /init: 参考 docs"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Interactive clarification is allowed"),
        "{prompt}"
    );
}

#[tokio::test]
async fn init_preflight_continue_keeps_auto_and_starts_best_effort() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select continue");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm continue");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(
        prompt.contains("Auto permission mode remained active"),
        "{prompt}"
    );
}

#[tokio::test]
async fn init_preflight_dialog_cancel_starts_no_workflow() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Cancel)
        .await
        .expect("cancel preflight");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(controller.active_turn.is_none());
}

#[tokio::test]
async fn slash_init_runs_one_repair_turn_when_agents_guide_validation_fails() {
    let dir = tempfile::tempdir().expect("temp dir");
    let agents_path = dir.path().join("AGENTS.md");
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let requests_clone = std::sync::Arc::clone(&requests);
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let agents_path_clone = agents_path.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        dir.path().to_path_buf(),
        move |request| {
            let requests = std::sync::Arc::clone(&requests_clone);
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            let agents_path = agents_path_clone.clone();
            async move {
                let prompt = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                requests.lock().expect("requests lock").push(request);
                let turn = turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if prompt.contains("AGENTS.md structure validation failed") {
                    std::fs::write(
                        agents_path,
                        init_command::example_agents_guide_for_tests("abc1234"),
                    )
                    .expect("write repaired AGENTS");
                } else {
                    std::fs::write(
                        agents_path,
                        "## Reference\ntext\n\n## Metadata\nCreated: 2026-07-07\n",
                    )
                    .expect("write invalid AGENTS");
                }
                assert!(turn < 2, "only initial and repair turns should run");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/init");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("/init handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    let requests = requests.lock().expect("requests lock");
    assert_eq!(
        requests[1].prompt_origin,
        MessageOrigin::injection("init"),
        "repair turn must remain hidden injection"
    );
    assert!(transcript_has_status(
        &controller,
        "AGENTS.md structure validation passed after repair"
    ));
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
async fn slash_add_workspace_opens_workspace_manager_overlay() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async { Ok(vec![]) },
    );
    let project_dir = test_workspace_root();
    controller.local_config = Some(test_config(&project_dir, project_dir.join(".neo/sessions")));
    controller.type_text("/add-workspace");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash command handled");
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("/add-workspace should open an overlay");
    assert!(
        matches!(overlay.kind, OverlayKind::WorkspaceManager(_)),
        "/add-workspace should open the workspace manager overlay, got {:?}",
        overlay.kind
    );
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
        joined.contains("▸ Name:")
            && joined.contains("Program:")
            && joined.contains("Arguments (JSON string per line):")
            && joined.contains("Env:"),
        "rendered frame should show stdio fields: {joined}"
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

    // Fill Name, Program, Arguments, and Env.
    controller
        .handle_input_event(InputEvent::Paste("fs".to_owned()))
        .await
        .expect("type name");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to command");
    controller
        .handle_input_event(InputEvent::Paste("npx".to_owned()))
        .await
        .expect("type program");
    controller
        .handle_input_event(InputEvent::Insert('\t'))
        .await
        .expect("switch to arguments");
    controller
        .handle_input_event(InputEvent::Paste(
            "\"-y\"\n\"@server/filesystem\"\n\"/repo\"".to_owned(),
        ))
        .await
        .expect("type arguments");
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

    let (pending, _response_rx) = make_pending_approval(ordinary_tool_request(
        "tool-1",
        "Write",
        "approved.txt",
        None,
    ));
    controller.register_pending_approval(pending);

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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
async fn mcp_startup_queues_prompt_then_starts_it_when_settled() {
    let run_turn: TurnDriver = Arc::new(|_request, channels| {
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );
    controller.tui.chrome_mut().set_mcp_startup_active(true);

    controller.type_text("queued during MCP startup");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("prompt should queue while MCP starts");

    assert!(
        controller.active_turn.is_none(),
        "MCP startup should defer the prompt instead of creating a turn"
    );
    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["queued during MCP startup"]
    );

    controller.tui.chrome_mut().set_mcp_startup_active(false);
    controller
        .start_next_mcp_startup_prompt()
        .expect("queued prompt should start after MCP settles");

    assert!(
        controller.active_turn.is_some(),
        "settled MCP startup should promote the queued prompt"
    );
    assert!(
        controller
            .chrome()
            .pending_input()
            .queued_follow_ups()
            .is_empty()
    );
    controller
        .cancel_active_turn()
        .await
        .expect("cleanup active turn");
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "queued transcript content")
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
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "long running"),
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn: Arc::new(|_request, channels| {
                Box::pin(async move {
                    channels.cancel_token.cancelled().await;
                    Ok(TurnOutcome::default())
                })
            }),
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
    );

    controller.type_text("wait for runtime append");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("submit starts turn");

    let matching_entries = transcript_entries(&controller)
        .iter()
        .filter(|entry| {
            matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "wait for runtime append")
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
            matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "wait for runtime append")
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "steer this")
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
            |entry| matches!(entry, TranscriptEntry::UserMessage { content, .. } if content == "steer this")
        ),
        "steered user prompt should render when the runtime appends it"
    );
}

#[tokio::test]
async fn steer_preview_is_cleared_when_turn_is_cancelled() {
    let mut controller = InteractiveController::new(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn: Arc::new(|_request, channels| {
                Box::pin(async move {
                    channels.cancel_token.cancelled().await;
                    Ok(TurnOutcome::default())
                })
            }),
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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

    assert_eq!(
        controller
            .chrome()
            .pending_input()
            .pending_steers()
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["steer this"],
        "steer should be visible before cancellation"
    );

    controller
        .cancel_active_turn()
        .await
        .expect("cancel active turn");

    assert!(
        controller.chrome().pending_input().is_empty(),
        "pending input should be cleared after the turn is interrupted"
    );
}

async fn controller_with_queued_follow_ups()
-> (InteractiveController, neo_agent_core::SteerInputHandle) {
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
    let steer_handle = captured_steer.lock().expect("steer lock").clone();
    (controller, steer_handle)
}

async fn assert_oldest_follow_up_promoted_before_composer(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("first ctrl+s promotes oldest queued follow-up");

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
}

async fn assert_remaining_follow_ups_promoted_before_composer(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
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
}

async fn assert_composer_promoted_after_follow_ups(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
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
}

fn assert_steers_render_after_runtime_append(controller: &mut InteractiveController) {
    let steered_user_messages = transcript_entries(controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, .. }
                if matches!(
                    content.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(content.as_str())
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
    let steered_user_messages = transcript_entries(controller)
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::UserMessage { content, .. }
                if matches!(
                    content.as_str(),
                    "queued one" | "queued two" | "queued D" | "current steer"
                ) =>
            {
                Some(content.as_str())
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
async fn active_turn_ctrl_s_promotes_one_follow_up_per_press_before_current_prompt() {
    let (mut controller, steer_handle) = controller_with_queued_follow_ups().await;
    controller.type_text("current steer");
    assert_oldest_follow_up_promoted_before_composer(&mut controller, &steer_handle).await;
    assert_remaining_follow_ups_promoted_before_composer(&mut controller, &steer_handle).await;
    assert_composer_promoted_after_follow_ups(&mut controller, &steer_handle).await;
    assert_steers_render_after_runtime_append(&mut controller);
}

async fn assert_first_empty_follow_up_promotion(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+s").expect("valid key")))
        .await
        .expect("empty ctrl+s promotes oldest queued follow-up");

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
}

async fn assert_second_empty_follow_up_promotion(
    controller: &mut InteractiveController,
    steer_handle: &neo_agent_core::SteerInputHandle,
) {
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
async fn empty_ctrl_s_promotes_one_follow_up_per_press_without_local_duplication() {
    let (mut controller, steer_handle) = controller_with_queued_follow_ups().await;
    assert_first_empty_follow_up_promotion(&mut controller, &steer_handle).await;
    assert_second_empty_follow_up_promotion(&mut controller, &steer_handle).await;
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        PickerCatalogs::default(),
        ControllerCallbacks {
            run_turn,
            load_session: Arc::new(|session_id| Box::pin(empty_session_loader(session_id))),
            fork_session: Arc::new(|session_id| Box::pin(empty_session_forker(session_id))),
        },
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
        resource_limit: None,
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

    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .unwrap()
            .selected_task_id(),
        Some("question-1")
    );
    controller
        .handle_input_event(InputEvent::ScrollDown(3))
        .await
        .expect("wheel moves selection");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .unwrap()
            .selected_task_id(),
        Some("question-2")
    );
    assert!(controller.chrome().prompt().text.is_empty());
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

    controller.last_task_browser_refresh = Some(
        Instant::now()
            .checked_sub(TASK_BROWSER_REFRESH_INTERVAL)
            .and_then(|instant| instant.checked_sub(Duration::from_millis(1)))
            .expect("now is far enough in the past"),
    );
    assert!(
        !controller.maybe_refresh_task_browser().await,
        "an unchanged refresh must not request a frame"
    );
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
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "work".to_owned(),
                    description: "work".to_owned(),
                }],
                script: "neo.phase('work')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                parent_run_id: None,
            },
        )
        .await
        .expect("create workflow");
    let run_id = handle.run_id.clone();
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

    controller
        .handle_input_event(InputEvent::Insert('p'))
        .await
        .expect("request pause");
    assert_eq!(
        controller
            .chrome()
            .task_browser_state()
            .expect("browser open")
            .pause_confirmation_task_id(),
        Some(run_id.0.as_str())
    );
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm pause");
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
        .handle_input_event(InputEvent::Insert('u'))
        .await
        .expect("request resume");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm resume");
    assert_eq!(
        handle.snapshot().await.state,
        neo_agent_core::workflow::WorkflowState::Running
    );

    controller
        .handle_input_event(InputEvent::Insert('s'))
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

#[test]
fn draining_btw_sidecar_reports_only_real_chrome_updates() {
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.tui.chrome_mut().set_btw_panel_state(Some(
        neo_tui::widgets::btw_panel::BtwPanelState::new(
            neo_tui::widgets::btw_panel::BtwSidecar::new("sidecar-1"),
        ),
    ));
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    controller.btw_receiver = Some(receiver);

    assert!(!controller.drain_btw_sidecar());
    sender
        .send(crate::modes::btw::BtwEvent::Started {
            sidecar_id: "sidecar-1".to_owned(),
            prompt: "question".to_owned(),
        })
        .expect("send sidecar event");
    assert!(controller.drain_btw_sidecar());
    assert!(!controller.drain_btw_sidecar());
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
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        EventDriverCallbacks {
            run_turn: move |_request| async move {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            },
            load_session: |_session_id| async move {
                panic!("fork should not use the load_session callback");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            fork_session: |parent_id| async move {
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
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());
    controller.workflow_capability.grant();

    let consumed = controller.handle_slash_command("/fork").await;
    assert!(consumed, "/fork should be consumed as a slash command");

    assert_eq!(
        controller.active_session_id(),
        Some(SESSION_CHILD),
        "active session switched to fork child"
    );
    assert_eq!(controller.chrome().session_label(), SESSION_CHILD);
    assert!(!controller.workflow_capability.inspect());
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

#[tokio::test]
async fn ctrl_n_forks_current_session_and_enters_child() {
    let mut controller = InteractiveController::new_with_event_driver_and_forker(
        "neo",
        SESSION_A,
        "openai/gpt-4.1",
        test_workspace_root(),
        PickerCatalogs {
            session_items: Vec::new(),
            session_error: None,
            model_items: Vec::new(),
        },
        EventDriverCallbacks {
            run_turn: move |_request| async move {
                Ok(vec![AgentEvent::TurnFinished {
                    turn: 1,
                    stop_reason: StopReason::EndTurn,
                }])
            },
            load_session: |_session_id| async move {
                panic!("fork should not use the load_session callback");
                #[allow(unreachable_code)]
                Ok(LoadedSessionTranscript::new("", Vec::new(), Vec::new()))
            },
            fork_session: |parent_id| async move {
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
        },
    );
    controller.active_session_id = Some(SESSION_A.to_owned());

    controller
        .handle_input_event(InputEvent::Key(KeyId::new("ctrl+n").expect("valid key")))
        .await
        .expect("ctrl+n forks current session");

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
