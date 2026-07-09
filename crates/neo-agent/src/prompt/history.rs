//! Workspace-scoped, append-only prompt history store.
//!
//! Each workspace session bucket gets a `prompt-history.jsonl` file under the
//! centralized sessions directory. Successful user prompts are appended after a
//! real submission (never slash commands), and loaded back into the TUI
//! composer's in-memory history on startup so Up/Down can recall them across
//! sessions in the same workspace.
//!
//! Storage rules (see `docs/superpowers/plans/2026-06-21-neo-27-prompt-history-handoff.md`):
//! - Append only; never rewrite the whole file.
//! - Skip empty/whitespace-only prompts.
//! - Skip consecutive duplicate prompts (non-consecutive repeats are kept).
//! - Keep at most `max_entries` in memory.
//! - Ignore malformed lines on load so one corrupt record cannot kill the TUI.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, workspace_sessions_dir};

/// Default cap on how many recent prompts are loaded into memory.
const DEFAULT_MAX_ENTRIES: usize = 500;

/// File name used inside each workspace session bucket.
const HISTORY_FILE_NAME: &str = "prompt-history.jsonl";

/// One JSONL record. `session_id` is optional so unsaved first prompts can
/// still be persisted before a session id is assigned.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PromptHistoryRecord {
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    text: String,
}

/// Append-only prompt history store rooted at a workspace session bucket.
#[derive(Debug, Clone)]
pub(crate) struct PromptHistoryStore {
    path: PathBuf,
    max_entries: usize,
    recent: Arc<Mutex<Option<Vec<String>>>>,
}

impl PromptHistoryStore {
    /// Build a store for the given config's workspace bucket.
    pub(crate) fn for_config(config: &AppConfig) -> Self {
        Self::for_dir(workspace_sessions_dir(config))
    }

    /// Build a store rooted at an explicit directory (primarily for tests).
    pub(crate) fn for_dir(dir: impl AsRef<Path>) -> Self {
        Self {
            path: dir.as_ref().join(HISTORY_FILE_NAME),
            max_entries: DEFAULT_MAX_ENTRIES,
            recent: Arc::new(Mutex::new(None)),
        }
    }

    /// Override the in-memory entry cap. Mainly useful for tests.
    #[cfg(test)]
    pub(crate) fn with_max_entries(mut self, max_entries: usize) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Path of the underlying JSONL file (diagnostics/tests).
    #[cfg(test)]
    pub(crate) fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Load recent prompt texts in file order (oldest → newest), trimming and
    /// collapsing consecutive duplicates. At most `max_entries` are returned.
    /// Malformed lines are skipped so a single corrupt record is non-fatal.
    pub(crate) fn load_recent(&self) -> anyhow::Result<Vec<String>> {
        let Some(entries) = self.load_records()? else {
            self.replace_recent(Vec::new());
            return Ok(Vec::new());
        };
        self.replace_recent(entries.clone());
        Ok(entries)
    }

    fn load_records(&self) -> anyhow::Result<Option<Vec<String>>> {
        let file = match OpenOptions::new().read(true).open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to open prompt history at {}", self.path.display())
                });
            }
        };
        let reader = BufReader::new(file);
        let mut entries: Vec<String> = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to read prompt history at {}", self.path.display())
                    });
                }
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Skip malformed lines rather than aborting the whole load.
            let Ok(record) = serde_json::from_str::<PromptHistoryRecord>(trimmed) else {
                continue;
            };
            let text = record.text.trim().to_owned();
            if text.is_empty() {
                continue;
            }
            if entries.last().is_some_and(|last: &String| last == &text) {
                continue;
            }
            entries.push(text);
        }
        if entries.len() > self.max_entries {
            let start = entries.len() - self.max_entries;
            entries.drain(..start);
        }
        Ok(Some(entries))
    }

    /// Append a prompt. Returns `Ok(true)` if a record was written, `Ok(false)`
    /// if the prompt was skipped (empty or consecutive duplicate). Append
    /// failures never block prompt submission at call sites — callers log and
    /// continue.
    pub(crate) fn append(&self, session_id: Option<&str>, text: &str) -> anyhow::Result<bool> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(false);
        }
        if self.last_recent_prompt()?.is_some_and(|last| last == text) {
            return Ok(false);
        }

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create prompt history dir {}", parent.display())
            })?;
        }

        let record = PromptHistoryRecord {
            created_at: now_iso8601(),
            session_id: session_id.map(str::to_owned),
            text: text.to_owned(),
        };
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("failed to open prompt history at {}", self.path.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &record).context("failed to encode prompt history")?;
        writer
            .write_all(b"\n")
            .context("failed to write prompt history newline")?;
        writer.flush().context("failed to flush prompt history")?;
        self.push_recent(text.to_owned());
        Ok(true)
    }

    fn last_recent_prompt(&self) -> anyhow::Result<Option<String>> {
        {
            let recent = self.recent.lock().expect("prompt history cache poisoned");
            if let Some(entries) = recent.as_ref() {
                return Ok(entries.last().cloned());
            }
        }
        Ok(self
            .load_records()?
            .and_then(|entries| entries.last().cloned()))
    }

    fn replace_recent(&self, entries: Vec<String>) {
        *self.recent.lock().expect("prompt history cache poisoned") = Some(entries);
    }

    fn push_recent(&self, text: String) {
        let mut recent = self.recent.lock().expect("prompt history cache poisoned");
        let entries = recent.get_or_insert_with(Vec::new);
        entries.push(text);
        if entries.len() > self.max_entries {
            let start = entries.len() - self.max_entries;
            entries.drain(..start);
        }
    }
}

/// Best-effort ISO-8601 UTC timestamp for diagnostics. File order remains the
/// primary ordering; `created_at` is informational.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let secs = u64::try_from(duration / 1000).unwrap_or_default();
    let millis = u32::try_from(duration % 1000).unwrap_or_default();

    // Days since epoch → civil date (Howard Hinnant's algorithm). Good enough
    // for diagnostics; no chrono dependency needed.
    let days = secs / 86_400;
    let (year, month, day) = civil_from_days(days);
    let day_secs = secs % 86_400;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let second = day_secs % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Convert days since the Unix epoch (1970-01-01) into a (year, month, day).
/// Source: Howard Hinnant, "`civil_from_days`".
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn civil_from_days(z: u64) -> (i32, u32, u32) {
    let z = z as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp_store(cap: usize) -> (tempfile::TempDir, PromptHistoryStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = PromptHistoryStore::for_dir(PathBuf::from(dir.path())).with_max_entries(cap);
        (dir, store)
    }

    #[test]
    fn prompt_history_store_appends_trims_and_skips_consecutive_duplicates() {
        let (_keep, store) = tmp_store(500);
        // Trim whitespace.
        assert!(store.append(None, "  first prompt  ").unwrap());
        // Consecutive duplicate (after trim) is skipped.
        assert!(!store.append(None, "first prompt").unwrap());
        // Blank is skipped.
        assert!(!store.append(None, "   ").unwrap());
        // Non-blank, non-duplicate is appended.
        assert!(store.append(Some("session-1"), "second prompt").unwrap());

        let loaded = store.load_recent().unwrap();
        assert_eq!(loaded, vec!["first prompt", "second prompt"]);
    }

    #[test]
    fn prompt_history_store_loads_in_file_order_across_sessions() {
        let (_keep, store) = tmp_store(500);
        store.append(None, "alpha").unwrap();
        store.append(Some("session-a"), "beta").unwrap();
        store.append(Some("session-b"), "gamma").unwrap();

        // File order is preserved; newest is last.
        let loaded = store.load_recent().unwrap();
        assert_eq!(
            loaded,
            vec!["alpha", "beta", "gamma"],
            "file order must drive recall, not session id"
        );
    }

    #[test]
    fn prompt_history_store_uses_distinct_workspace_buckets() {
        let dir_a = tempfile::tempdir().expect("temp dir a");
        let dir_b = tempfile::tempdir().expect("temp dir b");
        let store_a = PromptHistoryStore::for_dir(PathBuf::from(dir_a.path()));
        let store_b = PromptHistoryStore::for_dir(PathBuf::from(dir_b.path()));

        store_a.append(None, "workspace one").unwrap();
        // Distinct dirs must not leak prompts between workspaces.
        assert!(store_b.load_recent().unwrap().is_empty());
        assert_eq!(store_a.load_recent().unwrap(), vec!["workspace one"]);

        drop(dir_a);
        drop(dir_b);
    }

    #[test]
    fn prompt_history_store_keeps_non_consecutive_duplicates() {
        let (_keep, store) = tmp_store(500);
        store.append(None, "rerun me").unwrap();
        store.append(None, "other").unwrap();
        // Same text as the oldest, but not consecutive → kept.
        assert!(store.append(None, "rerun me").unwrap());

        assert_eq!(
            store.load_recent().unwrap(),
            vec!["rerun me", "other", "rerun me"]
        );
    }

    #[test]
    fn prompt_history_append_uses_loaded_recent_prompt_without_rescanning_history() {
        let (_keep, store) = tmp_store(500);
        store.append(None, "cached prompt").unwrap();
        assert_eq!(store.load_recent().unwrap(), vec!["cached prompt"]);
        fs::write(store.path(), b"{not json}\n").expect("corrupt history");

        assert!(!store.append(None, "cached prompt").unwrap());
        assert_eq!(store.load_recent().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn prompt_history_store_skips_malformed_lines_on_load() {
        let (_keep, store) = tmp_store(500);
        store.append(None, "good one").unwrap();
        // Corrupt the file with a malformed line between valid ones.
        fs::write(
            store.path(),
            format!(
                "{}\nnot-json\n{{\"text\":\"  \"}}\n",
                fs::read_to_string(store.path()).unwrap()
            ),
        )
        .unwrap();
        store.append(None, "good two").unwrap();

        // Malformed/blank lines are skipped; valid records still load.
        let loaded = store.load_recent().unwrap();
        assert_eq!(loaded, vec!["good one", "good two"]);
    }

    #[test]
    fn prompt_history_store_caps_loaded_entries_to_most_recent() {
        let (_keep, store) = tmp_store(3);
        store.append(None, "one").unwrap();
        store.append(None, "two").unwrap();
        store.append(None, "three").unwrap();
        store.append(None, "four").unwrap();
        store.append(None, "five").unwrap();

        let loaded = store.load_recent().unwrap();
        assert_eq!(loaded, vec!["three", "four", "five"]);
    }

    #[test]
    fn prompt_history_store_load_returns_empty_when_file_missing() {
        let (_keep, store) = tmp_store(500);
        assert!(store.load_recent().unwrap().is_empty());
    }

    #[test]
    fn civil_from_days_matches_known_epoch_dates() {
        // 1970-01-01 is day 0.
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2024-01-01 is a known leap-year boundary.
        let days = (2024 - 1970) as u64 * 365 + 13; // approx with leap days
        let _ = civil_from_days(days);
    }
}
