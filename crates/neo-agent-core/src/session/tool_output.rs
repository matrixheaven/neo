//! Complete per-task tool output store.
//!
//! Each tool execution with streaming display text owns one append-only log
//! file (`agents/<agent_id>/tasks/<task_id>.log`) plus a derived sparse index
//! (`<task_id>.log.idx`). The log is the source of truth; the index records a
//! byte offset every [`INDEX_STRIDE`] logical lines plus final byte/line
//! counts, and can be rebuilt from the log at any time with bounded memory.
//!
//! Display text is stored verbatim in call order: newlines and printable
//! content are preserved, and the text is never interpreted as terminal
//! control sequences or as model context. Reads return only the requested
//! line range (plus a one-line look-ahead) and never load the complete file
//! into memory.
//!
//! This module is pure filesystem I/O and has no configured byte or line
//! ceiling; append failures are returned to the caller instead of silently
//! dropping output.

use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::atomic_file::{replace_existing_file_atomic_with_status, write_file_atomic_create_new};
use super::layout::{TOOL_OUTPUT_INDEX_EXT, TOOL_OUTPUT_LOG_EXT, agent_tasks_dir};

/// Logical lines between sparse index entries.
const INDEX_STRIDE: u64 = 256;
const INDEX_HEADER: &str = "neo-tool-output-index 1";
const INDEX_READ_BUFFER_SIZE: usize = 64 * 1024;

/// Snapshot of one task's complete output artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(default)]
pub struct ToolOutputRef {
    pub agent_id: String,
    pub task_id: String,
    pub byte_len: u64,
    pub line_count: u64,
    pub complete: bool,
}

/// A contiguous line range read from a task's output artifact.
///
/// `text` holds the raw display lines (trailing newlines preserved, except a
/// final line without one), `start_line` echoes the requested start,
/// `next_line` is the first line after the returned text, and `reached_end`
/// reports whether the file's end was observed during the read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolOutputRange {
    pub text: String,
    pub start_line: u64,
    pub next_line: u64,
    pub reached_end: bool,
}

/// Owns complete tool display output for one session.
///
/// The store is stateless apart from its session directory: every operation
/// resolves task paths through [`agent_tasks_dir`], rejects invalid IDs, and
/// reports failures as `io::Error` to the caller. The artifact must be opened
/// before the producer launches ([`ToolOutputStore::open`]); append failures
/// surface immediately instead of silently losing output.
#[derive(Debug, Clone)]
pub struct ToolOutputStore {
    session_dir: PathBuf,
}

impl ToolOutputStore {
    #[must_use]
    pub fn new(session_dir: PathBuf) -> Self {
        Self { session_dir }
    }

    /// Create the output artifact for a task before its producer starts.
    ///
    /// Creates `agents/<agent_id>/tasks/<task_id>.log` (and its index) when
    /// missing and leaves an existing artifact untouched. Idempotent.
    pub fn open(&self, agent_id: &str, task_id: &str) -> io::Result<()> {
        let log_path = self.log_path(agent_id, task_id)?;
        let index_path = self.index_path(agent_id, task_id)?;
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        ensure_fresh_index(&log_path, &index_path)?;
        Ok(())
    }

    /// Append display text to a task's output artifact.
    ///
    /// Text is stored byte-for-byte in call order. The log is written before
    /// the derived index; a failure after the log write leaves the index
    /// stale, and the next operation rebuilds it from the log.
    pub fn append(&self, agent_id: &str, task_id: &str, text: &str) -> io::Result<()> {
        let log_path = self.log_path(agent_id, task_id)?;
        let index_path = self.index_path(agent_id, task_id)?;
        if text.is_empty() {
            return Ok(());
        }
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let log_len = match regular_file_len(&log_path) {
            Ok(len) => len,
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error),
        };
        let mut index = match load_index(&index_path)? {
            Some(index) if index.byte_len == log_len => index,
            // A fresh artifact has nothing to rebuild: the empty index matches
            // a missing or empty log exactly.
            _ if log_len == 0 => ToolOutputIndex::empty(),
            _ => rebuild_index(&log_path, &index_path)?,
        };

        let bytes = text.as_bytes();
        let completed = index
            .line_count
            .saturating_sub(u64::from(!index.terminated));
        let mut newline_index = 0u64;
        let mut ends_with_newline = false;
        let mut new_sparse = Vec::new();
        for (position, &byte) in bytes.iter().enumerate() {
            if byte == b'\n' {
                newline_index += 1;
                ends_with_newline = true;
                let line = completed + newline_index;
                if line.is_multiple_of(INDEX_STRIDE) {
                    new_sparse.push(SparseEntry {
                        line,
                        byte_offset: log_len + position as u64 + 1,
                    });
                }
            } else {
                ends_with_newline = false;
            }
        }
        let new_completed = completed + newline_index;
        index.byte_len = log_len + bytes.len() as u64;
        index.line_count = new_completed + u64::from(!ends_with_newline);
        index.terminated = ends_with_newline;
        index.sparse.extend(new_sparse);

        {
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)?;
            file.write_all(bytes)?;
            file.flush()?;
        }
        write_index_atomic(&index_path, &serialize_index(&index))?;
        Ok(())
    }

    /// Mark a task's artifact complete and return its final metadata.
    ///
    /// Idempotent. The log is synced before completion is recorded.
    pub fn finish(&self, agent_id: &str, task_id: &str) -> io::Result<ToolOutputRef> {
        let log_path = self.log_path(agent_id, task_id)?;
        let index_path = self.index_path(agent_id, task_id)?;
        let mut index = ensure_fresh_index(&log_path, &index_path)?;
        {
            let file = OpenOptions::new().append(true).open(&log_path)?;
            file.sync_all()?;
        }
        index.complete = true;
        write_index_atomic(&index_path, &serialize_index(&index))?;
        Ok(index.into_ref(agent_id, task_id))
    }

    /// Return the current metadata snapshot for a task's artifact.
    ///
    /// Rebuilds a missing, corrupt, or stale index from the log so the counts
    /// are exact. Errors with `NotFound` when the artifact was never opened.
    pub fn metadata(&self, agent_id: &str, task_id: &str) -> io::Result<ToolOutputRef> {
        let log_path = self.log_path(agent_id, task_id)?;
        let index_path = self.index_path(agent_id, task_id)?;
        let index = ensure_fresh_index(&log_path, &index_path)?;
        Ok(index.into_ref(agent_id, task_id))
    }

    /// Read `max_lines` logical lines starting at `start_line`.
    ///
    /// Seeks to the nearest sparse offset at or before `start_line`, streams
    /// forward, and returns the requested lines plus a one-line look-ahead
    /// (`reached_end`). The complete file is never loaded into memory; the
    /// returned text is bounded by the requested range. A stale index (the
    /// log grew past it) is tolerated for reads; a corrupt one is rebuilt.
    pub fn read_range(
        &self,
        agent_id: &str,
        task_id: &str,
        start_line: u64,
        max_lines: u64,
    ) -> io::Result<ToolOutputRange> {
        let log_path = self.log_path(agent_id, task_id)?;
        let index_path = self.index_path(agent_id, task_id)?;
        let log_len = regular_file_len(&log_path)?;
        let index = match load_index(&index_path)? {
            Some(index) if index.byte_len <= log_len => index,
            _ => rebuild_index(&log_path, &index_path)?,
        };

        let mut range = ToolOutputRange {
            text: String::new(),
            start_line,
            next_line: start_line,
            reached_end: false,
        };
        if max_lines == 0 {
            return Ok(range);
        }
        if index.byte_len == log_len && start_line >= index.line_count {
            range.reached_end = true;
            return Ok(range);
        }
        let start = index
            .sparse
            .iter()
            .rev()
            .find(|entry| entry.line <= start_line)
            .copied()
            .unwrap_or(SparseEntry {
                line: 0,
                byte_offset: 0,
            });

        let mut file = fs::File::open(&log_path)?;
        file.seek(SeekFrom::Start(start.byte_offset))?;
        let mut reader = BufReader::with_capacity(INDEX_READ_BUFFER_SIZE, file);

        let mut skip = Vec::new();
        let mut line = start.line;
        while line < start_line {
            skip.clear();
            if reader.read_until(b'\n', &mut skip)? == 0 {
                // The log ends before `start_line` (stale index or torn file).
                range.reached_end = true;
                return Ok(range);
            }
            line += 1;
        }

        let mut collected = 0u64;
        let mut text_bytes = Vec::new();
        let mut line_buffer = Vec::new();
        while collected < max_lines {
            line_buffer.clear();
            if reader.read_until(b'\n', &mut line_buffer)? == 0 {
                break;
            }
            text_bytes.extend_from_slice(&line_buffer);
            collected += 1;
        }
        if collected < max_lines {
            range.reached_end = true;
        } else {
            // Bounded look-ahead: one more line decides whether the file ends
            // exactly at the range boundary.
            skip.clear();
            range.reached_end = reader.read_until(b'\n', &mut skip)? == 0;
        }
        range.text = String::from_utf8_lossy(&text_bytes).into_owned();
        range.next_line = start_line + collected;
        Ok(range)
    }

    /// Resolve and validate the log path for a task.
    fn log_path(&self, agent_id: &str, task_id: &str) -> io::Result<PathBuf> {
        validate_agent_id(agent_id)?;
        validate_task_id(task_id)?;
        Ok(agent_tasks_dir(&self.session_dir, agent_id)
            .join(format!("{task_id}.{TOOL_OUTPUT_LOG_EXT}")))
    }

    /// Resolve and validate the index path for a task.
    fn index_path(&self, agent_id: &str, task_id: &str) -> io::Result<PathBuf> {
        validate_agent_id(agent_id)?;
        validate_task_id(task_id)?;
        Ok(agent_tasks_dir(&self.session_dir, agent_id)
            .join(format!("{task_id}.{TOOL_OUTPUT_INDEX_EXT}")))
    }
}

/// Load the index, rebuilding it when missing, corrupt, or stale.
fn ensure_fresh_index(log_path: &Path, index_path: &Path) -> io::Result<ToolOutputIndex> {
    let log_len = regular_file_len(log_path)?;
    match load_index(index_path)? {
        Some(index) if index.byte_len == log_len => Ok(index),
        _ => rebuild_index(log_path, index_path),
    }
}

/// One sparse entry: logical `line` starts at `byte_offset` in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SparseEntry {
    line: u64,
    byte_offset: u64,
}

/// Parsed index contents; the final entry is the last line written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolOutputIndex {
    sparse: Vec<SparseEntry>,
    byte_len: u64,
    line_count: u64,
    /// Whether the log currently ends with a newline (line-count bookkeeping).
    terminated: bool,
    complete: bool,
}

impl ToolOutputIndex {
    fn empty() -> Self {
        Self {
            sparse: vec![SparseEntry {
                line: 0,
                byte_offset: 0,
            }],
            byte_len: 0,
            line_count: 0,
            terminated: true,
            complete: false,
        }
    }

    fn into_ref(self, agent_id: &str, task_id: &str) -> ToolOutputRef {
        ToolOutputRef {
            agent_id: agent_id.to_owned(),
            task_id: task_id.to_owned(),
            byte_len: self.byte_len,
            line_count: self.line_count,
            complete: self.complete,
        }
    }
}

/// The index file could not be parsed as a valid derived index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CorruptIndex(String);

impl CorruptIndex {
    fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// Reject IDs that could escape the per-agent tasks directory.
fn validate_agent_id(agent_id: &str) -> io::Result<()> {
    validate_id(agent_id, "agent id")
}

fn validate_task_id(task_id: &str) -> io::Result<()> {
    validate_id(task_id, "task id")
}

fn validate_id(id: &str, what: &str) -> io::Result<()> {
    if id.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} must not be empty"),
        ));
    }
    if id == "." || id == ".." {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} {id:?} is not a valid file component"),
        ));
    }
    if id.contains('/') || id.contains('\\') || id.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{what} {id:?} contains a path separator"),
        ));
    }
    Ok(())
}

fn regular_file_len(path: &Path) -> io::Result<u64> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path is not a regular file: {}", path.display()),
        )),
        Err(error) => Err(error),
    }
}

/// Load the index from disk; `Ok(None)` means it is missing.
fn load_index(index_path: &Path) -> io::Result<Option<ToolOutputIndex>> {
    match fs::read(index_path) {
        Ok(content) => Ok(parse_index(&content).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Rebuild the derived index by streaming the log with bounded memory and
/// atomically replacing the index file. The log itself is never modified.
fn rebuild_index(log_path: &Path, index_path: &Path) -> io::Result<ToolOutputIndex> {
    let mut file = fs::File::open(log_path)?;
    let mut sparse = vec![SparseEntry {
        line: 0,
        byte_offset: 0,
    }];
    let mut byte_len = 0u64;
    let mut newlines = 0u64;
    let mut ends_with_newline = false;
    let mut buffer = vec![0u8; INDEX_READ_BUFFER_SIZE];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for (position, &byte) in buffer[..read].iter().enumerate() {
            if byte == b'\n' {
                newlines += 1;
                ends_with_newline = true;
                if newlines.is_multiple_of(INDEX_STRIDE) {
                    sparse.push(SparseEntry {
                        line: newlines,
                        byte_offset: byte_len + position as u64 + 1,
                    });
                }
            } else {
                ends_with_newline = false;
            }
        }
        byte_len += read as u64;
    }
    let line_count = if byte_len == 0 || ends_with_newline {
        newlines
    } else {
        newlines + 1
    };
    let index = ToolOutputIndex {
        sparse,
        byte_len,
        line_count,
        terminated: byte_len == 0 || ends_with_newline,
        complete: false,
    };
    write_index_atomic(index_path, &serialize_index(&index))?;
    Ok(index)
}

/// Atomically write the index, replacing an existing regular file or creating
/// a missing one. The index is derived, so a committed-but-unsynced write is
/// accepted; a later operation rebuilds it if it is lost.
fn write_index_atomic(index_path: &Path, content: &[u8]) -> io::Result<()> {
    match fs::symlink_metadata(index_path) {
        Ok(metadata) if metadata.is_file() => {
            replace_existing_file_atomic_with_status(index_path, |file| file.write_all(content))?;
            Ok(())
        }
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to replace non-regular index file {}",
                index_path.display()
            ),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_file_atomic_create_new(index_path, content)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn serialize_index(index: &ToolOutputIndex) -> Vec<u8> {
    let mut out = String::with_capacity(64 + index.sparse.len() * 24);
    out.push_str(INDEX_HEADER);
    out.push('\n');
    for entry in &index.sparse {
        out.push_str(&entry.line.to_string());
        out.push('\t');
        out.push_str(&entry.byte_offset.to_string());
        out.push('\n');
    }
    out.push_str("final\t");
    out.push_str(&index.byte_len.to_string());
    out.push('\t');
    out.push_str(&index.line_count.to_string());
    out.push('\t');
    out.push_str(if index.terminated { "1" } else { "0" });
    out.push('\t');
    out.push_str(if index.complete { "1" } else { "0" });
    out.push('\n');
    out.into_bytes()
}

/// Parse a serialized index. Every malformed byte makes the whole index
/// unusable, which is the signal to rebuild it from the log.
fn parse_index(content: &[u8]) -> Result<ToolOutputIndex, CorruptIndex> {
    let text = std::str::from_utf8(content).map_err(|_| CorruptIndex::new("index is not UTF-8"))?;
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let mut lines = lines.into_iter();
    let header = lines
        .next()
        .ok_or_else(|| CorruptIndex::new("index is empty"))?;
    if header != INDEX_HEADER {
        return Err(CorruptIndex::new(format!(
            "unexpected index header {header:?}"
        )));
    }
    let mut sparse: Vec<SparseEntry> = Vec::new();
    let mut final_entry: Option<(u64, u64, bool, bool)> = None;
    for line in lines {
        if line.is_empty() {
            return Err(CorruptIndex::new("blank line in index"));
        }
        if final_entry.is_some() {
            return Err(CorruptIndex::new("entry after final entry"));
        }
        if let Some(entry) = parse_sparse_line(line) {
            if sparse.last().is_some_and(|last| last.line >= entry.line) {
                return Err(CorruptIndex::new(
                    "sparse entries are not strictly increasing",
                ));
            }
            if sparse.is_empty() && entry.line != 0 {
                return Err(CorruptIndex::new("first sparse entry is not line 0"));
            }
            sparse.push(entry);
        } else if let Some(final_entry_value) = parse_final_line(line) {
            final_entry = Some(final_entry_value);
        } else {
            return Err(CorruptIndex::new(format!(
                "unparseable index line {line:?}"
            )));
        }
    }
    let (byte_len, line_count, terminated, complete) =
        final_entry.ok_or_else(|| CorruptIndex::new("missing final entry"))?;
    if sparse.first()
        != Some(&SparseEntry {
            line: 0,
            byte_offset: 0,
        })
    {
        return Err(CorruptIndex::new("index must start at line 0, byte 0"));
    }
    if sparse.iter().any(|entry| entry.line > line_count) {
        return Err(CorruptIndex::new(
            "sparse entry beyond the final line count",
        ));
    }
    if sparse.iter().any(|entry| entry.byte_offset > byte_len) {
        return Err(CorruptIndex::new(
            "sparse offset beyond the final byte length",
        ));
    }
    Ok(ToolOutputIndex {
        sparse,
        byte_len,
        line_count,
        terminated,
        complete,
    })
}

fn parse_sparse_line(line: &str) -> Option<SparseEntry> {
    let (line_text, offset_text) = line.split_once('\t')?;
    let line_number = parse_u64_strict(line_text)?;
    let byte_offset = parse_u64_strict(offset_text)?;
    if line_number % INDEX_STRIDE != 0 {
        return None;
    }
    Some(SparseEntry {
        line: line_number,
        byte_offset,
    })
}

fn parse_final_line(line: &str) -> Option<(u64, u64, bool, bool)> {
    let mut parts = line.split('\t');
    if parts.next() != Some("final") {
        return None;
    }
    let byte_len = parse_u64_strict(parts.next()?)?;
    let line_count = parse_u64_strict(parts.next()?)?;
    let terminated = parse_bool_strict(parts.next()?)?;
    let complete = parse_bool_strict(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((byte_len, line_count, terminated, complete))
}

fn parse_u64_strict(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_bool_strict(text: &str) -> Option<bool> {
    match text {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_index() -> ToolOutputIndex {
        ToolOutputIndex {
            sparse: vec![
                SparseEntry {
                    line: 0,
                    byte_offset: 0,
                },
                SparseEntry {
                    line: 256,
                    byte_offset: 4096,
                },
            ],
            byte_len: 16_384,
            line_count: 1_024,
            terminated: true,
            complete: false,
        }
    }

    #[test]
    fn parse_index_round_trips_serialized_index() {
        let index = sample_index();
        let parsed = parse_index(&serialize_index(&index)).expect("round trip");
        assert_eq!(parsed, index);
    }

    #[test]
    fn parse_index_round_trips_partial_tail() {
        let index = ToolOutputIndex {
            sparse: vec![SparseEntry {
                line: 0,
                byte_offset: 0,
            }],
            byte_len: 3,
            line_count: 1,
            terminated: false,
            complete: false,
        };
        let parsed = parse_index(&serialize_index(&index)).expect("round trip");
        assert_eq!(parsed, index);
    }

    #[test]
    fn parse_index_accepts_empty_output_index() {
        let index = ToolOutputIndex {
            sparse: vec![SparseEntry {
                line: 0,
                byte_offset: 0,
            }],
            byte_len: 0,
            line_count: 0,
            terminated: true,
            complete: true,
        };
        let parsed = parse_index(&serialize_index(&index)).expect("empty index parses");
        assert_eq!(parsed, index);
    }

    #[test]
    fn parse_index_rejects_corrupt_input() {
        let corrupt: Vec<&[u8]> = vec![
            b"",
            b"neo-tool-output-index 2\n0\t0\nfinal\t0\t0\t1\t0\n",
            b"neo-tool-output-index 1\nfinal\t0\t0\t1\t0\n",
            b"neo-tool-output-index 1\n0\t0\n",
            b"neo-tool-output-index 1\n0\t0\nfinal\tx\t0\t1\t0\n",
            b"neo-tool-output-index 1\n0\t0\n256\t4\nfinal\t4\t255\t1\t0\n",
            b"neo-tool-output-index 1\n0\t0\n512\t4\nfinal\t4\t256\t1\t0\n",
            b"neo-tool-output-index 1\n0\t0\n0\t4\nfinal\t4\t256\t1\t0\n",
            b"neo-tool-output-index 1\n0\t0\nfinal\t4\t256\t1\t0\nextra\n",
            b"neo-tool-output-index 1\n0\t0\nfinal\t4\t256\t2\t0\n",
            b"neo-tool-output-index 1\n0\t0\nfinal\t4\t256\t1\t0\n\n",
            b"neo-tool-output-index 1\n0\t0\nfinal\xff\xfe\t4\t256\t1\t0\n",
        ];
        for content in corrupt {
            assert!(
                parse_index(content).is_err(),
                "corrupt index {content:?} must be rejected"
            );
        }
    }
}
