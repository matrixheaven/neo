//! Extracted: git-status badge rendering and parsing helpers.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use neo_agent_core::AgentEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GitStatusBadge {
    pub(super) branch: String,
    pub(super) unborn: bool,
    pub(super) dirty: bool,
    pub(super) ahead: u32,
    pub(super) behind: u32,
}

impl GitStatusBadge {
    pub(super) fn format(&self) -> String {
        if self.unborn {
            return format!("{} [init]", self.branch);
        }

        let mut parts = Vec::new();
        if self.dirty {
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
    parse_git_status_porcelain(&status).map(|badge| badge.format())
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
