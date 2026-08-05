//! Integration tests for the complete tool output store (`ToolOutputStore`).
//!
//! Every test uses a temporary session directory; the user's real session
//! directory is never touched.

use std::io::Write as _;
use std::path::PathBuf;

use neo_agent_core::session::{ToolOutputRef, ToolOutputStore};
use tempfile::TempDir;

fn test_store() -> (TempDir, ToolOutputStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = ToolOutputStore::new(dir.path().to_path_buf());
    (dir, store)
}

fn task_log_path(dir: &TempDir, agent_id: &str, task_id: &str) -> PathBuf {
    dir.path()
        .join("agents")
        .join(agent_id)
        .join("tasks")
        .join(format!("{task_id}.log"))
}

fn task_index_path(dir: &TempDir, agent_id: &str, task_id: &str) -> PathBuf {
    dir.path()
        .join("agents")
        .join(agent_id)
        .join("tasks")
        .join(format!("{task_id}.log.idx"))
}

fn numbered_lines(count: u64) -> String {
    (0..count).map(|i| format!("line {i}\n")).collect()
}

#[test]
fn tool_output_store_rejects_invalid_ids_and_path_traversal() {
    let (dir, store) = test_store();
    for id in ["", ".", "..", "a/b", "a\\b", "../escape", "a/../b", "a\0b"] {
        let agent_error = store.append(id, "t", "x\n").unwrap_err();
        assert_eq!(
            agent_error.kind(),
            std::io::ErrorKind::InvalidInput,
            "agent id {id:?} must be rejected"
        );
        let task_error = store.append("main", id, "x\n").unwrap_err();
        assert_eq!(
            task_error.kind(),
            std::io::ErrorKind::InvalidInput,
            "task id {id:?} must be rejected"
        );
        assert!(
            store.read_range(id, "t", 0, 1).is_err(),
            "read agent {id:?}"
        );
        assert!(
            store.read_range("main", id, 0, 1).is_err(),
            "read task {id:?}"
        );
        assert!(store.metadata(id, "t").is_err(), "metadata agent {id:?}");
        assert!(store.finish("main", id).is_err(), "finish task {id:?}");
        assert!(store.open(id, "t").is_err(), "open agent {id:?}");
    }
    // No directory outside the validated layout was created.
    let entries = match std::fs::read_dir(dir.path().join("agents")) {
        Ok(entries) => entries.collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("read agents dir: {error}"),
    };
    assert!(entries.is_empty(), "nothing may be created for invalid IDs");
}

#[test]
fn tool_output_store_append_finish_metadata_round_trip() {
    let (dir, store) = test_store();
    let mut expected = String::new();
    let mut previous = ToolOutputRef::default();
    for chunk in ["alpha\n", "beta", "\ngamma\n", "delta"] {
        store.append("main", "round-trip", chunk).unwrap();
        expected.push_str(chunk);
        let meta = store.metadata("main", "round-trip").unwrap();
        assert!(
            meta.byte_len >= previous.byte_len,
            "byte metadata must be monotonic"
        );
        assert!(
            meta.line_count >= previous.line_count,
            "line metadata must be monotonic"
        );
        previous = meta;
    }
    let meta = store.metadata("main", "round-trip").unwrap();
    assert_eq!(meta.agent_id, "main");
    assert_eq!(meta.task_id, "round-trip");
    assert!(!meta.complete);
    assert_eq!(meta.byte_len, expected.len() as u64);
    // The log stores the appended bytes verbatim.
    assert_eq!(
        std::fs::read(task_log_path(&dir, "main", "round-trip")).unwrap(),
        expected.as_bytes()
    );
    // The derived index sits next to the log.
    assert!(task_index_path(&dir, "main", "round-trip").is_file());
    // Everything reads back in source order.
    let range = store.read_range("main", "round-trip", 0, 100).unwrap();
    assert_eq!(range.text, expected);
    assert_eq!(range.next_line, meta.line_count);
    assert!(range.reached_end);
    // Finish marks the artifact complete and is idempotent.
    let done = store.finish("main", "round-trip").unwrap();
    assert!(done.complete);
    assert_eq!(done.byte_len, meta.byte_len);
    assert_eq!(done.line_count, meta.line_count);
    assert_eq!(done, store.finish("main", "round-trip").unwrap());
    assert!(store.metadata("main", "round-trip").unwrap().complete);
    // Reads still work after completion.
    assert_eq!(
        store.read_range("main", "round-trip", 1, 1).unwrap().text,
        "beta\n"
    );
    assert_eq!(
        store.read_range("main", "round-trip", 3, 1).unwrap().text,
        "delta"
    );
}

#[test]
fn tool_output_store_handles_chunk_split_utf8_and_newlines() {
    let (dir, store) = test_store();
    store.append("main", "split", "第一行").unwrap(); // partial line, no newline yet
    store.append("main", "split", " 续写\n").unwrap(); // completes line 0
    store.append("main", "split", "第二行\n第三").unwrap(); // line 1 done, line 2 partial
    let meta = store.metadata("main", "split").unwrap();
    assert_eq!(meta.line_count, 3);
    assert_eq!(meta.byte_len, "第一行 续写\n第二行\n第三".len() as u64);
    assert_eq!(
        std::fs::read(task_log_path(&dir, "main", "split")).unwrap(),
        "第一行 续写\n第二行\n第三".as_bytes()
    );
    let range = store.read_range("main", "split", 0, 3).unwrap();
    assert_eq!(range.text, "第一行 续写\n第二行\n第三");
    assert_eq!(range.next_line, 3);
    assert!(range.reached_end);
    // A later chunk completes the partial line without splitting it.
    store.append("main", "split", "行尾\n").unwrap();
    assert_eq!(
        store.read_range("main", "split", 2, 1).unwrap().text,
        "第三行尾\n"
    );
    assert_eq!(store.metadata("main", "split").unwrap().line_count, 3);
    assert_eq!(
        store.read_range("main", "split", 0, 1).unwrap().text,
        "第一行 续写\n"
    );
}

#[test]
fn tool_output_store_reads_torn_utf8_tail_without_error() {
    let (dir, store) = test_store();
    store.append("main", "torn", "ok\n").unwrap();
    // Simulate a write torn mid-sequence: two of the three bytes of "你".
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(task_log_path(&dir, "main", "torn"))
        .unwrap();
    file.write_all(b"\xe4\xbd").unwrap();
    drop(file);
    // The log grew past the index; reads still stream to the true end.
    let range = store.read_range("main", "torn", 0, 10).unwrap();
    assert_eq!(range.text, "ok\n\u{FFFD}");
    assert!(range.reached_end);
    // Metadata rebuilds the index so counts are exact again.
    let meta = store.metadata("main", "torn").unwrap();
    assert_eq!(meta.byte_len, 5);
    assert_eq!(meta.line_count, 2);
    // Appends after the torn tail keep working and never lose bytes.
    store.append("main", "torn", "tail\n").unwrap();
    assert_eq!(store.metadata("main", "torn").unwrap().byte_len, 10);
    let range = store.read_range("main", "torn", 0, 10).unwrap();
    assert!(range.text.starts_with("ok\n"));
    assert!(range.text.ends_with("tail\n"));
}

#[test]
fn tool_output_store_reads_output_beyond_10_mib() {
    let (dir, store) = test_store();
    let line = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\n";
    let chunk_lines = 256usize;
    let total_lines = 200_000u64;
    let mut written = 0u64;
    while written < total_lines {
        let take = (total_lines - written).min(chunk_lines as u64) as usize;
        store.append("main", "big", &line.repeat(take)).unwrap();
        written += take as u64;
    }
    let expected_bytes = total_lines * line.len() as u64;
    assert!(
        expected_bytes > 10 * 1024 * 1024,
        "test data must exceed 10 MiB"
    );
    let meta = store.metadata("main", "big").unwrap();
    assert_eq!(meta.byte_len, expected_bytes);
    assert_eq!(meta.line_count, total_lines);
    assert!(!meta.complete);
    // The sparse index recorded one entry per 256-line stride plus line 0.
    let index_text = std::fs::read_to_string(task_index_path(&dir, "main", "big")).unwrap();
    let non_final_lines = index_text
        .lines()
        .filter(|line| !line.starts_with("final\t"))
        .count();
    assert_eq!(non_final_lines, (total_lines / 256 + 2) as usize);
    // Ranges at the start, middle, and tail all resolve through sparse offsets.
    assert_eq!(
        store.read_range("main", "big", 0, 2).unwrap().text,
        line.repeat(2)
    );
    let middle = store.read_range("main", "big", 100_000, 3).unwrap();
    assert_eq!(middle.text, line.repeat(3));
    assert_eq!(middle.next_line, 100_003);
    assert!(!middle.reached_end);
    let tail_start = total_lines - 5;
    let tail = store.read_range("main", "big", tail_start, 10).unwrap();
    assert_eq!(tail.text, line.repeat(5));
    assert_eq!(tail.next_line, total_lines);
    assert!(tail.reached_end);
    // Finish reports exact final counts.
    let done = store.finish("main", "big").unwrap();
    assert!(done.complete);
    assert_eq!(done.byte_len, expected_bytes);
    assert_eq!(done.line_count, total_lines);
}

#[test]
fn tool_output_store_read_range_boundaries() {
    let (_dir, store) = test_store();
    let mut text = String::new();
    for i in 0..100 {
        text.push_str(&format!("l{i}\n"));
    }
    store.append("main", "t", &text).unwrap();
    let range = store.read_range("main", "t", 0, 100).unwrap();
    assert_eq!(range.text, text);
    assert_eq!(range.start_line, 0);
    assert_eq!(range.next_line, 100);
    assert!(range.reached_end);
    // A partial range does not reach the end.
    let range = store.read_range("main", "t", 0, 50).unwrap();
    assert_eq!(
        range.text,
        (0..50).map(|i| format!("l{i}\n")).collect::<String>()
    );
    assert_eq!(range.next_line, 50);
    assert!(!range.reached_end);
    // The final line.
    let range = store.read_range("main", "t", 99, 5).unwrap();
    assert_eq!(range.text, "l99\n");
    assert_eq!(range.next_line, 100);
    assert!(range.reached_end);
    // Ranges starting at or beyond the end are empty and report the end.
    for start in [100, 101, 1_000] {
        let range = store.read_range("main", "t", start, 5).unwrap();
        assert_eq!(range.text, "");
        assert_eq!(range.next_line, start);
        assert!(range.reached_end);
    }
    // A zero-line request reads nothing and does not claim the end.
    let range = store.read_range("main", "t", 10, 0).unwrap();
    assert_eq!(range.text, "");
    assert_eq!(range.next_line, 10);
    assert!(!range.reached_end);
}

#[test]
fn tool_output_store_ranges_cross_sparse_boundaries() {
    let (_dir, store) = test_store();
    let mut text = String::new();
    for i in 0..1_000 {
        text.push_str(&format!("l{i}\n"));
    }
    store.append("main", "t", &text).unwrap();
    let lines = |start: u64, end: u64| (start..end).map(|i| format!("l{i}\n")).collect::<String>();
    // Exactly on a sparse offset.
    assert_eq!(
        store.read_range("main", "t", 256, 1).unwrap().text,
        "l256\n"
    );
    // Just after a sparse offset.
    assert_eq!(
        store.read_range("main", "t", 257, 1).unwrap().text,
        "l257\n"
    );
    // Crossing a sparse offset.
    assert_eq!(
        store.read_range("main", "t", 255, 2).unwrap().text,
        lines(255, 257)
    );
    // A window spanning two sparse offsets.
    assert_eq!(
        store.read_range("main", "t", 255, 10).unwrap().text,
        lines(255, 265)
    );
    // Nearest-sparse-offset seeks (300 sits between 256 and 512).
    assert_eq!(
        store.read_range("main", "t", 300, 50).unwrap().text,
        lines(300, 350)
    );
    // A full-file read resolves every boundary.
    let range = store.read_range("main", "t", 0, 1_000).unwrap();
    assert_eq!(range.text, text);
    assert_eq!(range.next_line, 1_000);
    assert!(range.reached_end);
}

#[test]
fn tool_output_store_rebuilds_missing_index() {
    let (dir, store) = test_store();
    store.append("main", "t", &numbered_lines(600)).unwrap();
    let log = task_log_path(&dir, "main", "t");
    let index = task_index_path(&dir, "main", "t");
    assert!(index.is_file());
    std::fs::remove_file(&index).unwrap();
    // Reads rebuild the index on demand.
    assert_eq!(
        store.read_range("main", "t", 300, 5).unwrap().text,
        "line 300\nline 301\nline 302\nline 303\nline 304\n"
    );
    assert!(index.is_file(), "read must rebuild the missing index");
    // Metadata agrees with the log after the rebuild.
    let meta = store.metadata("main", "t").unwrap();
    assert_eq!(meta.line_count, 600);
    assert_eq!(meta.byte_len, std::fs::metadata(&log).unwrap().len());
    assert!(!meta.complete);
    // Appends continue from the rebuilt state without losing or doubling lines.
    store.append("main", "t", "line 600\n").unwrap();
    let done = store.finish("main", "t").unwrap();
    assert_eq!(done.line_count, 601);
    assert!(done.complete);
}

#[test]
fn tool_output_store_rebuilds_corrupt_index() {
    let (dir, store) = test_store();
    store.append("main", "t", &numbered_lines(300)).unwrap();
    let log = task_log_path(&dir, "main", "t");
    let index = task_index_path(&dir, "main", "t");
    // Garbage that is not an index at all.
    std::fs::write(&index, b"this is not a tool output index\n").unwrap();
    assert_eq!(
        store.read_range("main", "t", 200, 2).unwrap().text,
        "line 200\nline 201\n"
    );
    // A plausible header with broken entries.
    std::fs::write(&index, b"neo-tool-output-index 1\n0\t0\n512\tboom\n").unwrap();
    let meta = store.metadata("main", "t").unwrap();
    assert_eq!(meta.line_count, 300);
    // An index claiming more bytes than the log holds is corrupt.
    std::fs::write(
        &index,
        b"neo-tool-output-index 1\n0\t0\nfinal\t999999\t300\t1\t1\n",
    )
    .unwrap();
    assert_eq!(
        store.metadata("main", "t").unwrap().byte_len,
        std::fs::metadata(&log).unwrap().len()
    );
    // After the rebuilds the index parses again.
    let rebuilt = std::fs::read_to_string(&index).unwrap();
    assert!(rebuilt.starts_with("neo-tool-output-index 1\n"));
    assert!(rebuilt.contains("\nfinal\t"));
    // Finish after corruption still yields exact counts.
    let done = store.finish("main", "t").unwrap();
    assert_eq!(done.line_count, 300);
    assert!(done.complete);
}

#[test]
fn tool_output_store_recovers_stale_index_after_partial_log_growth() {
    let (dir, store) = test_store();
    store.append("main", "t", "a\nb\nc\n").unwrap();
    assert_eq!(store.metadata("main", "t").unwrap().line_count, 3);
    // Simulate a crash between the log write and the index write: the log
    // grows directly, leaving the index stale.
    let log = task_log_path(&dir, "main", "t");
    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    file.write_all(b"d\ne\n").unwrap();
    drop(file);
    // Stale indexes are tolerated for reads.
    assert_eq!(
        store.read_range("main", "t", 0, 10).unwrap().text,
        "a\nb\nc\nd\ne\n"
    );
    // Metadata and appends rebuild first, so counts never double.
    assert_eq!(store.metadata("main", "t").unwrap().line_count, 5);
    store.append("main", "t", "f\n").unwrap();
    let meta = store.metadata("main", "t").unwrap();
    assert_eq!(meta.line_count, 6);
    assert_eq!(meta.byte_len, std::fs::metadata(&log).unwrap().len());
}

#[test]
fn tool_output_store_missing_log_is_reported() {
    let (_dir, store) = test_store();
    let err = store.read_range("main", "t", 0, 5).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    let err = store.metadata("main", "t").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    let err = store.finish("main", "t").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn tool_output_store_append_failure_is_returned() {
    let (dir, store) = test_store();
    let tasks = dir.path().join("agents").join("main").join("tasks");
    // A directory squatting on the log path must surface as an error.
    std::fs::create_dir_all(tasks.join("blocked.log")).unwrap();
    let err = store.append("main", "blocked", "data\n").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    // An index path occupied by a directory cannot be read or replaced, so
    // append and reads surface the failure.
    std::fs::create_dir_all(tasks.join("idxblocked.log.idx")).unwrap();
    assert!(store.append("main", "idxblocked", "data\n").is_err());
    assert!(store.read_range("main", "idxblocked", 0, 1).is_err());
    // Unrelated tasks keep working.
    store.append("main", "fine", "ok\n").unwrap();
    assert_eq!(store.read_range("main", "fine", 0, 1).unwrap().text, "ok\n");
}

#[cfg(unix)]
#[test]
fn tool_output_store_append_failure_on_readonly_log_is_returned() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, store) = test_store();
    store.append("main", "t", "data\n").unwrap();
    let log = task_log_path(&dir, "main", "t");
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o444)).unwrap();
    let err = store.append("main", "t", "more\n").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    // Restore permissions so tempdir cleanup can remove the file.
    std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn tool_output_store_open_then_finish_empty_output() {
    let (_dir, store) = test_store();
    // An empty append is a no-op and must not fabricate an artifact.
    store.append("main", "t", "").unwrap();
    assert_eq!(
        store.metadata("main", "t").unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    // Opening precedes the producer; finish then reports exact zero counts.
    store.open("main", "t").unwrap();
    let meta = store.metadata("main", "t").unwrap();
    assert_eq!((meta.byte_len, meta.line_count), (0, 0));
    assert!(!meta.complete);
    let range = store.read_range("main", "t", 0, 10).unwrap();
    assert_eq!(
        (range.text.as_str(), range.next_line, range.reached_end),
        ("", 0, true)
    );
    let done = store.finish("main", "t").unwrap();
    assert_eq!(
        (done.byte_len, done.line_count, done.complete),
        (0, 0, true)
    );
    // Open is idempotent, even after completion.
    store.open("main", "t").unwrap();
    assert!(store.metadata("main", "t").unwrap().complete);
}
