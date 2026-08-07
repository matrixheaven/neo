//! Transcript git-status behavior (split from `transcript.rs`).

use std::{collections::VecDeque, fs, process::Command};

use neo_agent_core::{AgentEvent, ToolResult};

use super::super::git_status::{
    count_untracked_changes, git_status_label_with_program, parse_git_numstat,
    parse_git_status_porcelain, parse_git_untracked_files_z,
};
use super::super::*;
use super::*;

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

#[cfg(unix)]
#[test]
fn git_status_untracked_fifo_returns_without_blocking() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().expect("tempdir");
    let fifo_path = dir.path().join("fifo-file");
    let status = Command::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("run mkfifo");
    assert!(status.success(), "mkfifo should succeed");

    let paths = parse_git_untracked_files_z(b"fifo-file\0");
    let start = Instant::now();
    let (added, untracked) = count_untracked_changes(dir.path(), &paths);
    let elapsed = start.elapsed();

    assert_eq!(added, 0, "FIFO should not contribute line counts");
    assert_eq!(untracked, 1, "FIFO should count as one unknown entry");
    assert!(
        elapsed < Duration::from_secs(2),
        "FIFO inspection should return without blocking, took {elapsed:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn git_status_untracked_non_utf8_path_is_counted_losslessly() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let raw_name: Vec<u8> = vec![0xC0, 0xAF, b'n', b'.', b'r', b's'];
    let name = OsString::from_vec(raw_name.clone());
    fs::write(dir.path().join(&name), "line one\nline two\n").expect("write non-utf8 file");

    let mut bytes = raw_name.clone();
    bytes.push(0);
    let paths = parse_git_untracked_files_z(&bytes);
    let (added, untracked) = count_untracked_changes(dir.path(), &paths);

    assert_eq!(added, 2, "non-UTF-8 path should be inspected losslessly");
    assert_eq!(untracked, 0, "non-UTF-8 path should not count as unknown");
}

#[cfg(windows)]
#[test]
fn git_status_untracked_windows_unicode_path_is_counted_losslessly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let name = "\u{1F980}\u{1F41E}.rs";
    fs::write(dir.path().join(name), "first\nsecond\nthird\n").expect("write unicode file");

    let mut bytes = name.as_bytes().to_vec();
    bytes.push(0);
    let paths = parse_git_untracked_files_z(&bytes);
    let (added, untracked) = count_untracked_changes(dir.path(), &paths);

    assert_eq!(added, 3, "unicode path should be inspected losslessly");
    assert_eq!(untracked, 0, "unicode path should not count as unknown");
}

#[test]
fn git_status_untracked_file_over_limit_counts_as_unknown() {
    let dir = tempfile::tempdir().expect("tempdir");
    let oversized = vec![b'a'; 1024 * 1024 + 1];
    fs::write(dir.path().join("big.txt"), &oversized).expect("write oversized file");

    let paths = parse_git_untracked_files_z(b"big.txt\0");
    let (added, untracked) = count_untracked_changes(dir.path(), &paths);

    assert_eq!(added, 0, "oversized file should not contribute line counts");
    assert_eq!(
        untracked, 1,
        "oversized file should count as one unknown entry"
    );
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
        workflow_origin: None,
        output_ref: None,
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
        workflow_origin: None,
        output_ref: None,
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
        output_ref: None,
    });
    assert_eq!(controller.chrome().git_status_label(), Some("main [↑1]"));

    controller.apply_turn_event(AgentEvent::TerminalSessionFinished {
        turn: 1,
        id: "terminal-1".to_owned(),
        handle: "terminal".to_owned(),
        status: "exited".to_owned(),
        output_ref: None,
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
