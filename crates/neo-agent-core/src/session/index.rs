//! Global session index — an append-only JSONL file mapping session IDs
//! to their on-disk locations and original workspace paths.
//!
//! This enables `neo resume <session_id>` to locate a session even if the
//! user is in a different workspace than where the session was created.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::AsyncBufReadExt;

use super::{SessionError, SessionMetadataFile, SessionSummary};

const INDEX_FILENAME: &str = "session_index.jsonl";

/// One entry in the global session index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub workdir: PathBuf,
}

#[derive(Debug, Error)]
pub enum SessionIndexError {
    #[error("session index I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session index JSON failed: {source}")]
    Json {
        #[source]
        source: serde_json::Error,
    },
}

/// Append-only JSONL index at `<neo_home>/session_index.jsonl`.
pub struct SessionIndex {
    index_path: PathBuf,
}

impl SessionIndex {
    /// Create a handle for the index file inside the given neo home directory.
    #[must_use]
    pub fn new(neo_home: &Path) -> Self {
        Self {
            index_path: neo_home.join(INDEX_FILENAME),
        }
    }

    /// Create a handle from an explicit index file path (useful for tests).
    #[must_use]
    pub fn from_path(index_path: PathBuf) -> Self {
        Self { index_path }
    }

    /// Append a single entry to the index. Creates the file if it does not exist.
    pub fn append(&self, entry: &SessionIndexEntry) -> Result<(), SessionIndexError> {
        use std::io::Write;
        if let Some(parent) = self.index_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.index_path)?;
        let line =
            serde_json::to_string(entry).map_err(|source| SessionIndexError::Json { source })?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// Find the most recent entry for the given session ID.
    /// Scans from the end of the file so that the latest appended entry wins.
    pub fn find(&self, session_id: &str) -> Result<Option<SessionIndexEntry>, SessionIndexError> {
        let entries = self.list_all()?;
        Ok(entries
            .into_iter()
            .rev()
            .find(|entry| entry.session_id == session_id))
    }

    /// Read all entries from the index file. Malformed lines are silently skipped.
    pub fn list_all(&self) -> Result<Vec<SessionIndexEntry>, SessionIndexError> {
        let content = match std::fs::read_to_string(&self.index_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            }
            Err(error) => return Err(SessionIndexError::Io(error)),
        };

        let mut entries = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(line) {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Read every indexed session and enrich it with its per-workspace metadata.
    ///
    /// Returns summaries sorted by `updated_at` descending. Entries whose
    /// metadata file is missing or corrupted are skipped silently.
    pub fn list_all_with_metadata(
        &self,
        sessions_root: &Path,
    ) -> Result<Vec<SessionSummary>, SessionIndexError> {
        let entries = self.list_all()?;
        let mut summaries = Vec::new();

        for entry in entries {
            let bucket_dir = if entry.session_dir.is_absolute() {
                entry.session_dir.clone()
            } else {
                sessions_root.join(&entry.session_dir)
            };
            let metadata_path = bucket_dir.join("sessions.metadata.json");

            let content = match std::fs::read_to_string(&metadata_path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(SessionIndexError::Io(error)),
            };

            let metadata = match serde_json::from_str::<SessionMetadataFile>(&content) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };

            let Some(stored) = metadata.sessions.get(&entry.session_id) else {
                continue;
            };

            let record = crate::session::SessionRecord {
                id: entry.session_id.clone(),
                name: stored.name.clone(),
                summary: stored.summary.clone(),
                parent_id: stored.parent_id.clone(),
                summary_record: stored.summary_record.clone(),
                title: stored.title.clone(),
                title_model: stored.title_model.clone(),
                title_updated_at: stored.title_updated_at.clone(),
                workspace: stored.workspace.clone(),
                last_user_prompt: stored.last_user_prompt.clone(),
                updated_at: stored.updated_at.clone(),
                children: Vec::new(),
            };
            summaries.push(SessionSummary::from_record(record, &entry.workdir));
        }

        summaries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(summaries)
    }
}

/// Async variant of `list_all` for use in async contexts.
///
/// Reads the index file line by line using tokio async I/O.
pub async fn list_all_async(index_path: &Path) -> Result<Vec<SessionIndexEntry>, SessionError> {
    use tokio::fs::File;

    let file = match File::open(index_path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => return Err(SessionError::Io(error)),
    };
    let mut reader = tokio::io::BufReader::new(file);
    let mut entries = Vec::new();
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        let n = reader
            .read_line(&mut line_buf)
            .await
            .map_err(SessionError::Io)?;
        if n == 0 {
            break;
        }
        let trimmed = line_buf.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(trimmed) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn append_and_find() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        let entry = SessionIndexEntry {
            session_id: "1234567890".to_owned(),
            session_dir: tmp.path().join("wd_neo_abc123/1234567890.jsonl"),
            workdir: PathBuf::from("/home/user/neo"),
        };
        index.append(&entry).unwrap();

        let found = index.find("1234567890").unwrap();
        assert_eq!(found.as_ref().unwrap().session_id, "1234567890");
        assert_eq!(
            found.as_ref().unwrap().workdir,
            PathBuf::from("/home/user/neo")
        );
    }

    #[test]
    fn find_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        let found = index.find("nonexistent").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn find_latest_wins() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        index
            .append(&SessionIndexEntry {
                session_id: "s1".to_owned(),
                session_dir: tmp.path().join("old.jsonl"),
                workdir: PathBuf::from("/old"),
            })
            .unwrap();

        index
            .append(&SessionIndexEntry {
                session_id: "s1".to_owned(),
                session_dir: tmp.path().join("new.jsonl"),
                workdir: PathBuf::from("/new"),
            })
            .unwrap();

        let found = index.find("s1").unwrap().unwrap();
        assert_eq!(found.workdir, PathBuf::from("/new"));
    }

    #[test]
    fn list_all_skips_malformed() {
        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);

        std::fs::write(
            &index_path,
            "{invalid json\n\
             {\"session_id\":\"s1\",\"session_dir\":\"/a.jsonl\",\"workdir\":\"/a\"}\n",
        )
        .unwrap();

        let index = SessionIndex::from_path(index_path);
        let entries = index.list_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_id, "s1");
    }

    #[test]
    fn list_all_on_missing_file() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());
        let entries = index.list_all().unwrap();
        assert!(entries.is_empty());
    }
}
