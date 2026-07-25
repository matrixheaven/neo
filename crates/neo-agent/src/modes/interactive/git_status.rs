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
        if let Ok(output) = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["diff", "--numstat", "HEAD", "--"])
            .output()
            && output.status.success()
        {
            (badge.added, badge.deleted) =
                parse_git_numstat(&String::from_utf8_lossy(&output.stdout));
        }

        if let Ok(output) = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            && output.status.success()
        {
            let paths = parse_git_untracked_files_z(&output.stdout);
            let (added, untracked) = count_untracked_changes(workspace_root, &paths);
            badge.added = badge.added.saturating_add(added);
            badge.untracked = untracked;
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
        (
            added.saturating_add(parse_git_numstat_count(parts.next())),
            deleted.saturating_add(parse_git_numstat_count(parts.next())),
        )
    })
}

pub(super) fn parse_git_untracked_files_z(stdout: &[u8]) -> Vec<PathBuf> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|segment| !segment.is_empty())
        .map(decode_git_path_segment)
        .collect()
}

#[cfg(unix)]
fn decode_git_path_segment(segment: &[u8]) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    PathBuf::from(OsString::from_vec(segment.to_vec()))
}

#[cfg(windows)]
fn decode_git_path_segment(segment: &[u8]) -> PathBuf {
    // `git ls-files -z` on Windows emits paths as UTF-8 bytes. If a segment is
    // not valid UTF-8, represent it as one uninspectable entry instead of a
    // lossy text path.
    match std::str::from_utf8(segment) {
        Ok(text) => PathBuf::from(text),
        Err(_) => PathBuf::new(),
    }
}

pub(super) fn count_untracked_changes(workspace_root: &Path, paths: &[PathBuf]) -> (u32, u32) {
    paths
        .iter()
        .fold((0_u32, 0_u32), |(added, untracked), path| {
            let full_path = match contained_join(workspace_root, path) {
                Some(full_path) => full_path,
                None => return (added, untracked.saturating_add(1)),
            };
            match count_text_file_lines(&full_path) {
                Some(lines) => (added.saturating_add(lines), untracked),
                None => (added, untracked.saturating_add(1)),
            }
        })
}

/// Join a git-relative path to the workspace root, accepting it only if it is
/// relative and contains no parent-directory escapes.
fn contained_join(workspace_root: &Path, path: &Path) -> Option<PathBuf> {
    if path.has_root() {
        return None;
    }
    for component in path.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return None;
        }
    }
    let joined = workspace_root.join(path);
    joined.strip_prefix(workspace_root).ok()?;
    Some(joined)
}

const MAX_INSPECT_BYTES: usize = 1024 * 1024;

fn count_text_file_lines(path: &Path) -> Option<u32> {
    let mut file = open_inspection_file(path).ok()?;

    // Recheck the opened handle: regular files only, no symlinks or reparse
    // points. This closes the race where the path is swapped between our
    // earlier checks and the read.
    let metadata = file.metadata().ok()?;
    if metadata.is_symlink() || !metadata.is_file() {
        return None;
    }

    let mut buffer = [0_u8; 8192];
    let mut total = 0_usize;
    let mut lines = 0_u32;
    let mut saw_byte = false;
    let mut last_byte = 0_u8;
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read);
        if total > MAX_INSPECT_BYTES {
            return None;
        }
        for byte in &buffer[..read] {
            if *byte == 0 {
                return None;
            }
            saw_byte = true;
            lines = lines.saturating_add(u32::from(*byte == b'\n'));
            last_byte = *byte;
        }
    }
    Some(lines.saturating_add(u32::from(saw_byte && last_byte != b'\n')))
}

#[cfg(unix)]
fn open_inspection_file(path: &Path) -> Result<File, std::io::Error> {
    use rustix::fs::{OFlags, open};

    let fd = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|errno| std::io::Error::from_raw_os_error(errno.raw_os_error()))?;
    Ok(File::from(fd))
}

#[cfg(windows)]
fn open_inspection_file(path: &Path) -> Result<File, std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    // Open reparse points without following them so a swapped-in symlink or
    // junction is inspected on the handle, not transparently resolved.
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn parse_git_numstat_count(value: Option<&str>) -> u32 {
    value
        .filter(|value| *value != "-")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}
