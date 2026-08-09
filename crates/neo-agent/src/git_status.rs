//! Shared git status collection: the interactive chrome badge and the web
//! workspace-change surface both use this single implementation. Pure parsing
//! plus cross-platform `std::process::Command` argument-form invocations of
//! the `git` program only — never an external diff tool, never a shell
//! string. Every failure resolves to "no status" (`None`), never error text
//! or paths.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use neo_agent_core::AgentEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitStatusBadge {
    pub(crate) branch: String,
    pub(crate) unborn: bool,
    pub(crate) dirty: bool,
    pub(crate) ahead: u32,
    pub(crate) behind: u32,
    pub(crate) added: u32,
    pub(crate) deleted: u32,
    pub(crate) untracked: u32,
}

impl GitStatusBadge {
    pub(crate) fn format(&self) -> String {
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

pub(crate) fn git_status_label(workspace_root: &Path) -> Option<String> {
    git_status_label_with_program("git", workspace_root)
}

pub(crate) fn event_should_refresh_git_status(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::ToolExecutionFinished { .. }
            | AgentEvent::ShellCommandFinished { .. }
            | AgentEvent::TerminalSessionFinished { .. }
            | AgentEvent::TurnFinished { .. }
            | AgentEvent::RunFinished { .. }
    )
}

pub(crate) fn git_status_label_with_program(
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

pub(crate) fn parse_git_status_porcelain(stdout: &str) -> Option<GitStatusBadge> {
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

pub(crate) fn parse_git_numstat(stdout: &str) -> (u32, u32) {
    stdout.lines().fold((0, 0), |(added, deleted), line| {
        let mut parts = line.split('\t');
        (
            added.saturating_add(parse_git_numstat_count(parts.next())),
            deleted.saturating_add(parse_git_numstat_count(parts.next())),
        )
    })
}

pub(crate) fn parse_git_untracked_files_z(stdout: &[u8]) -> Vec<PathBuf> {
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

pub(crate) fn count_untracked_changes(workspace_root: &Path, paths: &[PathBuf]) -> (u32, u32) {
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

/// Whether a path is relative and stays inside its base once joined: no
/// root, and no parent, prefix or current-dir components.
fn is_contained_relative_path(path: &Path) -> bool {
    !path.has_root()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

/// Join a git-relative path to the workspace root, accepting it only if it is
/// relative and contains no parent-directory escapes.
fn contained_join(workspace_root: &Path, path: &Path) -> Option<PathBuf> {
    if !is_contained_relative_path(path) {
        return None;
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

// ── Workspace change collection (web top bar surface) ────────────────────

/// Kind of one workspace change row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
}

/// One workspace change entry. `path` is always relative to the repository
/// root (git emits forward-slash relative paths) and never absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitWorkspaceChange {
    pub(crate) path: PathBuf,
    pub(crate) kind: GitChangeKind,
    pub(crate) added: u32,
    pub(crate) deleted: u32,
}

/// Structured workspace status: the branch label plus every change entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitWorkspaceStatus {
    pub(crate) branch: String,
    pub(crate) unborn: bool,
    pub(crate) changes: Vec<GitWorkspaceChange>,
}

/// Collect the structured workspace status on demand (no polling). Returns
/// `None` when the workspace is not a repository or git fails.
pub(crate) fn collect_workspace_status(workspace_root: &Path) -> Option<GitWorkspaceStatus> {
    collect_workspace_status_with_program("git", workspace_root)
}

pub(crate) fn collect_workspace_status_with_program(
    program: &str,
    workspace_root: &Path,
) -> Option<GitWorkspaceStatus> {
    let output = Command::new(program)
        .arg("-C")
        .arg(workspace_root)
        .args(["status", "--porcelain=v1", "--branch", "-z", "-uall"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let ParsedWorkspacePorcelain {
        branch,
        unborn,
        mut changes,
    } = parse_git_workspace_porcelain_z(&output.stdout)?;
    let mut counts: HashMap<PathBuf, (u32, u32)> = HashMap::new();
    if !unborn
        && let Ok(output) = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(["diff", "--numstat", "-z", "HEAD", "--"])
            .output()
        && output.status.success()
    {
        counts = parse_git_numstat_entries_z(&output.stdout);
    }
    for change in &mut changes {
        let (added, deleted) = match change.kind {
            // Untracked files have no numstat; count text lines through the
            // same symlink-safe inspection the badge uses. Binary or
            // unreadable files keep zero counts.
            GitChangeKind::Untracked => {
                let lines = contained_join(workspace_root, &change.path)
                    .and_then(|full_path| count_text_file_lines(&full_path))
                    .unwrap_or(0);
                (lines, 0)
            }
            _ => counts.get(&change.path).copied().unwrap_or((0, 0)),
        };
        change.added = added;
        change.deleted = deleted;
    }
    Some(GitWorkspaceStatus {
        branch,
        unborn,
        changes,
    })
}

/// Parsed `git status --porcelain=v1 --branch -z -uall` output: the branch
/// label plus every change entry with zeroed counts (filled in afterwards
/// from numstat or the untracked line counting).
struct ParsedWorkspacePorcelain {
    branch: String,
    unborn: bool,
    changes: Vec<GitWorkspaceChange>,
}

/// Parse `git status --porcelain=v1 --branch -z -uall`. Rename/copy records
/// are two NUL fields (the current path, then the source path); the current
/// path is kept.
fn parse_git_workspace_porcelain_z(stdout: &[u8]) -> Option<ParsedWorkspacePorcelain> {
    let mut branch = None;
    let mut unborn = false;
    let mut entries = Vec::new();
    let mut segments = stdout.split(|byte| *byte == 0).filter(|s| !s.is_empty());
    while let Some(segment) = segments.next() {
        if let Some(header) = segment.strip_prefix(b"## ") {
            let header = String::from_utf8_lossy(header);
            let parsed = parse_git_branch_header(&header);
            branch = Some(parsed.branch);
            unborn = parsed.unborn;
            continue;
        }
        if segment.len() < 4 || segment[2] != b' ' {
            continue;
        }
        let x = segment[0];
        let y = segment[1];
        let path = decode_git_path_segment(&segment[3..]);
        let kind = if x == b'?' && y == b'?' {
            GitChangeKind::Untracked
        } else if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            // The source path follows as its own NUL field; drop it.
            let _ = segments.next();
            GitChangeKind::Renamed
        } else if x == b'A' || y == b'A' {
            GitChangeKind::Added
        } else if x == b'D' || y == b'D' {
            GitChangeKind::Deleted
        } else {
            GitChangeKind::Modified
        };
        entries.push(GitWorkspaceChange {
            path,
            kind,
            added: 0,
            deleted: 0,
        });
    }
    branch
        .filter(|name| !name.is_empty())
        .map(|branch| ParsedWorkspacePorcelain {
            branch,
            unborn,
            changes: entries,
        })
}

/// Parse `git diff --numstat -z HEAD --`: each record is `added<TAB>deleted
/// <TAB>` followed by the path inline, or — for renames — an empty inline
/// path and two NUL fields (the source path, then the current path). Counts
/// are keyed by the current path so they match the porcelain entries.
fn parse_git_numstat_entries_z(stdout: &[u8]) -> HashMap<PathBuf, (u32, u32)> {
    let mut counts = HashMap::new();
    let mut segments = stdout.split(|byte| *byte == 0);
    while let Some(segment) = segments.next() {
        if segment.is_empty() {
            continue;
        }
        let mut parts = segment.splitn(3, |byte| *byte == b'\t');
        let added = parse_git_numstat_count(parts.next().and_then(|p| std::str::from_utf8(p).ok()));
        let deleted =
            parse_git_numstat_count(parts.next().and_then(|p| std::str::from_utf8(p).ok()));
        let inline = parts.next().unwrap_or(&[]);
        let path_segment = if inline.is_empty() {
            // Rename/copy: the source path and the current path follow as
            // their own NUL fields.
            let _source = segments.next();
            match segments.next() {
                Some(current) => current,
                None => break,
            }
        } else {
            inline
        };
        if path_segment.is_empty() {
            continue;
        }
        counts.insert(decode_git_path_segment(path_segment), (added, deleted));
    }
    counts
}

/// Opaque web change reference: URL-safe base64 of the workspace-relative
/// path bytes. Only the service generates it; the browser passes it back
/// verbatim.
pub(crate) fn encode_change_id(path: &Path) -> Option<String> {
    use base64::Engine as _;
    let bytes = path_bytes(path)?;
    Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Decode an opaque change reference back into a validated workspace-relative
/// path: never empty, never absolute, never containing parent, prefix or
/// current-dir components. Anything else is rejected.
pub(crate) fn decode_change_id(change_id: &str) -> Option<PathBuf> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(change_id)
        .ok()?;
    let path = decode_git_path_segment(&bytes);
    if path.as_os_str().is_empty() || !is_contained_relative_path(&path) {
        return None;
    }
    Some(path)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;

    Some(path.as_os_str().as_bytes().to_vec())
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Option<Vec<u8>> {
    // Git emits UTF-8 paths on Windows; a path that is not valid UTF-8 cannot
    // round-trip through the web wire form and is refused instead.
    Some(path.to_str()?.as_bytes().to_vec())
}

/// Byte cap for one unified-diff preview served to the web.
pub(crate) const DIFF_PREVIEW_MAX_BYTES: usize = 64 * 1024;

/// Length-bounded unified-diff preview for one change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitChangeDiff {
    pub(crate) diff: String,
    pub(crate) truncated: bool,
}

/// Build the bounded unified-diff preview for one change. Uses the `git`
/// program's own diff (never an external diff tool) with the validated
/// relative path as a single argument; untracked or unborn-added files get a
/// synthesized new-file preview through the symlink-safe file read.
pub(crate) fn change_diff_preview(
    workspace_root: &Path,
    change: &GitWorkspaceChange,
) -> Option<GitChangeDiff> {
    change_diff_preview_with_program("git", workspace_root, change)
}

pub(crate) fn change_diff_preview_with_program(
    program: &str,
    workspace_root: &Path,
    change: &GitWorkspaceChange,
) -> Option<GitChangeDiff> {
    if change.kind == GitChangeKind::Untracked {
        return Some(new_file_diff_preview(workspace_root, &change.path));
    }
    for args in [
        ["diff", "HEAD", "--"].as_slice(),
        ["diff", "--cached", "--"].as_slice(),
    ] {
        if let Ok(output) = Command::new(program)
            .arg("-C")
            .arg(workspace_root)
            .args(args)
            .arg(&change.path)
            .output()
            && output.status.success()
        {
            return Some(truncate_diff(&output.stdout));
        }
    }
    // A staged-new file without a diff base (unborn branch) still gets a
    // preview from its content.
    if change.kind == GitChangeKind::Added {
        return Some(new_file_diff_preview(workspace_root, &change.path));
    }
    None
}

/// Cap one diff at [`DIFF_PREVIEW_MAX_BYTES`], flagging truncation.
fn truncate_diff(bytes: &[u8]) -> GitChangeDiff {
    let truncated = bytes.len() > DIFF_PREVIEW_MAX_BYTES;
    let slice = if truncated {
        &bytes[..DIFF_PREVIEW_MAX_BYTES]
    } else {
        bytes
    };
    GitChangeDiff {
        diff: String::from_utf8_lossy(slice).into_owned(),
        truncated,
    }
}

/// Synthesize a unified-diff-style preview for a file without a diff base.
/// Never follows symlinks or reparse points; binary or unreadable files
/// produce an empty preview.
fn new_file_diff_preview(workspace_root: &Path, path: &Path) -> GitChangeDiff {
    let empty = || GitChangeDiff {
        diff: String::new(),
        truncated: false,
    };
    let Some(full_path) = contained_join(workspace_root, path) else {
        return empty();
    };
    let Ok(mut file) = open_inspection_file(&full_path) else {
        return empty();
    };
    let Ok(metadata) = file.metadata() else {
        return empty();
    };
    if metadata.is_symlink() || !metadata.is_file() {
        return empty();
    }
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        // Read at most the cap plus one chunk to detect truncation.
        let Ok(read) = file.read(&mut buffer) else {
            return empty();
        };
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if bytes.len() > DIFF_PREVIEW_MAX_BYTES {
            break;
        }
    }
    if bytes.contains(&0) {
        return empty();
    }
    let truncated = bytes.len() > DIFF_PREVIEW_MAX_BYTES;
    bytes.truncate(DIFF_PREVIEW_MAX_BYTES);
    let text = String::from_utf8_lossy(&bytes);
    let mut diff = format!("--- /dev/null\n+++ b/{}\n", path.display());
    for line in text.lines() {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    GitChangeDiff { diff, truncated }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;

    #[test]
    fn git_status_workspace_porcelain_z_parses_branch_rename_and_untracked() {
        let stdout = b"## main\0R  new.txt\0old.txt\0 M tracked.txt\0?? un tracked.txt\0";
        let parsed = parse_git_workspace_porcelain_z(stdout).expect("porcelain parses");
        assert_eq!(parsed.branch, "main");
        assert!(!parsed.unborn);
        let entries: Vec<(PathBuf, GitChangeKind)> = parsed
            .changes
            .iter()
            .map(|change| (change.path.clone(), change.kind))
            .collect();
        assert_eq!(
            entries,
            vec![
                (PathBuf::from("new.txt"), GitChangeKind::Renamed),
                (PathBuf::from("tracked.txt"), GitChangeKind::Modified),
                (PathBuf::from("un tracked.txt"), GitChangeKind::Untracked),
            ]
        );
    }

    #[test]
    fn git_status_numstat_z_maps_counts_to_current_paths() {
        let stdout = b"0\t0\t\0old.txt\0new.txt\01\t2\ttracked.txt\0-\t-\tbin.dat\0";
        let counts = parse_git_numstat_entries_z(stdout);
        assert_eq!(counts.get(&PathBuf::from("new.txt")), Some(&(0, 0)));
        assert_eq!(counts.get(&PathBuf::from("tracked.txt")), Some(&(1, 2)));
        assert_eq!(counts.get(&PathBuf::from("bin.dat")), Some(&(0, 0)));
        assert!(!counts.contains_key(&PathBuf::from("old.txt")));
    }

    #[test]
    fn git_status_change_id_round_trips_and_rejects_escape_references() {
        let id = encode_change_id(Path::new("src/app.rs")).expect("encode");
        assert_eq!(decode_change_id(&id), Some(PathBuf::from("src/app.rs")));

        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let absolute = engine.encode(b"/etc/passwd");
        assert_eq!(decode_change_id(&absolute), None);
        let parent = engine.encode(b"../outside.txt");
        assert_eq!(decode_change_id(&parent), None);
        assert_eq!(decode_change_id("!!!not-base64!!!"), None);
        assert_eq!(decode_change_id(""), None);
    }
}
