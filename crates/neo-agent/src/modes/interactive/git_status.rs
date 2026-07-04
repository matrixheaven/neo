//! Extracted: git-status badge rendering and parsing helpers.

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use neo_agent_core::AgentEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitStatusBadge {
    pub(super) branch: String,
    pub(super) unborn: bool,
    pub(super) dirty: bool,
    pub(super) ahead: u32,
    pub(super) behind: u32,
    pub(super) added: u32,
    pub(super) deleted: u32,
    pub(super) untracked: u32,
}

impl GitStatusBadge {
    pub(super) fn format(&self) -> String {
        if self.unborn {
            return format!("{} [init]", self.branch);
        }

        let mut parts = Vec::new();
        let has_line_counts = self.added > 0 || self.deleted > 0;
        if has_line_counts {
            parts.push(format!("+{} -{}", self.added, self.deleted));
        }
        if self.untracked > 0 {
            parts.push(format!("?{}", self.untracked));
        } else if self.dirty && !has_line_counts {
            parts.push("±".to_owned());
        }
        let mut sync = String::new();
        if self.ahead > 0 {
            let _ = write!(sync, "↑{}", self.ahead);
        }
        if self.behind > 0 {
            let _ = write!(sync, "↓{}", self.behind);
        }
        if !sync.is_empty() {
            parts.push(sync);
        }
        if parts.is_empty() {
            self.branch.clone()
        } else {
            format!("{} [{}]", self.branch, parts.join(" "))
        }
    }
}

pub(super) fn git_status_label(workspace_root: &Path) -> Option<String> {
    git_status_label_with_program("git", workspace_root)
}

pub(super) fn event_should_refresh_git_status(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandFinished { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::TurnFinished { .. }
            | AgentEvent::RunFinished { .. }
    )
}

pub(super) fn git_status_label_with_program(
    program: &str,
    workspace_root: &Path,
) -> Option<String> {
    if !workspace_root.join(".git").exists() {
        return None;
    }

    let status_output = Command::new(program)
        .arg("-C")
        .arg(workspace_root)
        .args(["status", "--porcelain=v1", "--branch"])
        .output()
        .ok()?;
    if !status_output.status.success() {
        return None;
    }
    let status = String::from_utf8_lossy(&status_output.stdout);
    let mut badge = parse_git_status_porcelain(&status)?;
    if badge.dirty && !badge.unborn {
        let numstat_output = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["diff", "--numstat", "HEAD", "--"])
            .output()
            .ok();
        if let Some(output) = numstat_output
            && output.status.success()
        {
            let numstat = String::from_utf8_lossy(&output.stdout);
            let (added, deleted) = parse_git_numstat(&numstat);
            badge.added = added;
            badge.deleted = deleted;
        }
        let untracked_output = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            .ok();
        if let Some(output) = untracked_output
            && output.status.success()
        {
            let untracked_files = parse_git_untracked_files_z(&output.stdout);
            let (added, untracked) = count_untracked_changes(workspace_root, &untracked_files);
            badge.added = badge.added.saturating_add(added);
            badge.untracked = badge.untracked.saturating_add(untracked);
        }
    }
    Some(badge.format())
}

pub(super) fn parse_git_status_porcelain(stdout: &str) -> Option<GitStatusBadge> {
    let mut branch = None;
    let mut unborn = false;
    let mut ahead = 0;
    let mut behind = 0;
    let mut dirty = false;

    for line in stdout.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            let parsed = parse_git_branch_header(header);
            branch = Some(parsed.branch);
            unborn = parsed.unborn;
            ahead = parsed.ahead;
            behind = parsed.behind;
        } else if !line.trim().is_empty() {
            dirty = true;
        }
    }

    branch
        .filter(|name| !name.is_empty())
        .map(|branch| GitStatusBadge {
            branch,
            unborn,
            dirty,
            ahead,
            behind,
            added: 0,
            deleted: 0,
            untracked: 0,
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitBranchHeader {
    branch: String,
    unborn: bool,
    ahead: u32,
    behind: u32,
}

fn parse_git_branch_header(header: &str) -> GitBranchHeader {
    let (branch_part, sync_part) = header
        .split_once(" [")
        .map_or((header, ""), |(branch, sync)| (branch, sync));
    let unborn = branch_part.starts_with("No commits yet on ");
    let stripped = branch_part
        .strip_prefix("No commits yet on ")
        .unwrap_or(branch_part);
    let branch = stripped
        .split_once("...")
        .map_or(stripped, |(b, _)| b)
        .trim()
        .to_owned();
    let ahead = parse_git_sync_count(sync_part, "ahead ");
    let behind = parse_git_sync_count(sync_part, "behind ");
    GitBranchHeader {
        branch,
        unborn,
        ahead,
        behind,
    }
}

fn parse_git_sync_count(sync_part: &str, label: &str) -> u32 {
    sync_part
        .split(label)
        .nth(1)
        .and_then(|rest| {
            rest.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

pub(super) fn parse_git_numstat(stdout: &str) -> (u32, u32) {
    stdout.lines().fold((0, 0), |(added, deleted), line| {
        let mut parts = line.split('\t');
        let line_added = parse_git_numstat_count(parts.next());
        let line_deleted = parse_git_numstat_count(parts.next());
        (added + line_added, deleted + line_deleted)
    })
}

pub(super) fn parse_git_untracked_files_z(stdout: &[u8]) -> Vec<PathBuf> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect()
}

pub(super) fn count_untracked_changes(workspace_root: &Path, paths: &[PathBuf]) -> (u32, u32) {
    paths.iter().fold(
        (0_u32, 0_u32),
        |(added, other), path| match count_text_file_lines(&workspace_root.join(path)) {
            Some(lines) => (added.saturating_add(lines), other),
            None => (added, other.saturating_add(1)),
        },
    )
}

fn count_text_file_lines(path: &Path) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }

    let mut buffer = [0_u8; 8192];
    let mut lines = 0_u32;
    let mut saw_byte = false;
    let mut last_byte = 0_u8;

    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        for byte in &buffer[..read] {
            if *byte == 0 {
                return None;
            }
            saw_byte = true;
            if *byte == b'\n' {
                lines = lines.saturating_add(1);
            }
            last_byte = *byte;
        }
    }

    if saw_byte && last_byte != b'\n' {
        lines = lines.saturating_add(1);
    }
    Some(lines)
}

fn parse_git_numstat_count(value: Option<&str>) -> u32 {
    value
        .filter(|value| *value != "-")
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}
