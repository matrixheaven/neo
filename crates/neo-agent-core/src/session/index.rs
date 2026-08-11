//! Global session index — an append-only JSONL file mapping session IDs
//! to their on-disk locations and original workspace paths.
//!
//! This enables `neo resume <session_id>` to locate a session even if the
//! user is in a different workspace than where the session was created.

use std::fmt;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeStruct, Serializer};
use thiserror::Error;
use tokio::fs::File;
use tokio::io::AsyncBufReadExt;

use super::{SessionError, SessionMetadataFile, SessionSummary, validate_session_id};

const INDEX_FILENAME: &str = "session_index.jsonl";

/// One entry in the global session index.
#[derive(Debug, Clone)]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub session_dir: PathBuf,
    pub workdir: PathBuf,
}

/// Private versioned/tagged wire for `SessionIndexEntry`.
///
/// Old Unicode string records remain readable; new records use a tagged
/// native-path object per path field. Cross-platform wires are rejected as
/// foreign, and invalid base64 or malformed unit payloads are rejected so
/// that bad lines are skipped deterministically.
impl Serialize for SessionIndexEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("SessionIndexEntry", 4)?;
        state.serialize_field("v", &1_u32)?;
        state.serialize_field("session_id", &self.session_id)?;
        state.serialize_field("session_dir", &WirePath(&self.session_dir))?;
        state.serialize_field("workdir", &WirePath(&self.workdir))?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for SessionIndexEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = SessionIndexEntry;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a session index entry object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut version = None::<u32>;
                let mut session_id = None::<String>;
                let mut session_dir = None::<PathBuf>;
                let mut workdir = None::<PathBuf>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "v" => version = Some(map.next_value()?),
                        "session_id" => session_id = Some(map.next_value()?),
                        "session_dir" => {
                            session_dir = Some(
                                map.next_value::<PathWire>()?
                                    .into_path_buf()
                                    .map_err(serde::de::Error::custom)?,
                            );
                        }
                        "workdir" => {
                            workdir = Some(
                                map.next_value::<PathWire>()?
                                    .into_path_buf()
                                    .map_err(serde::de::Error::custom)?,
                            );
                        }
                        _ => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                    }
                }

                match version.unwrap_or(1) {
                    1 => {}
                    other => {
                        return Err(serde::de::Error::custom(format_args!(
                            "unsupported session index entry version {other}"
                        )));
                    }
                }

                let session_id =
                    session_id.ok_or_else(|| serde::de::Error::missing_field("session_id"))?;
                let session_dir =
                    session_dir.ok_or_else(|| serde::de::Error::missing_field("session_dir"))?;
                let workdir = workdir.ok_or_else(|| serde::de::Error::missing_field("workdir"))?;

                Ok(SessionIndexEntry {
                    session_id,
                    session_dir,
                    workdir,
                })
            }
        }

        deserializer.deserialize_map(EntryVisitor)
    }
}

struct WirePath<'a>(&'a Path);

impl Serialize for WirePath<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("kind", "unix")?;
            map.serialize_entry("bytes", &base64_encode(self.0.as_os_str().as_bytes()))?;
            map.end()
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;

            let units: Vec<u16> = self.0.as_os_str().encode_wide().collect();
            let bytes: Vec<u8> = units.iter().copied().flat_map(u16::to_ne_bytes).collect();

            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("kind", "windows")?;
            map.serialize_entry("units", &base64_encode(&bytes))?;
            map.end()
        }
    }
}

enum PathWire {
    Legacy(String),
    Unix(String),
    Windows(String),
}

impl PathWire {
    fn into_path_buf(self) -> Result<PathBuf, String> {
        match self {
            PathWire::Legacy(text) => Ok(PathBuf::from(text)),
            #[cfg(unix)]
            PathWire::Unix(b64) => {
                use std::os::unix::ffi::OsStringExt;

                let bytes = base64_decode(&b64)
                    .map_err(|error| format!("invalid unix path base64: {error}"))?;
                Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
            }
            #[cfg(not(unix))]
            PathWire::Unix(encoded) => {
                drop(encoded);
                Err("unix path wire on non-unix platform".to_owned())
            }
            #[cfg(windows)]
            PathWire::Windows(b64) => {
                use std::os::windows::ffi::OsStringExt;

                let bytes = base64_decode(&b64)
                    .map_err(|error| format!("invalid windows path base64: {error}"))?;
                if bytes.len() % 2 != 0 {
                    return Err(format!(
                        "windows path units length {} is not even",
                        bytes.len()
                    ));
                }
                let units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|chunk| u16::from_ne_bytes([chunk[0], chunk[1]]))
                    .collect();
                Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
            }
            #[cfg(not(windows))]
            PathWire::Windows(encoded) => {
                drop(encoded);
                Err("windows path wire on non-windows platform".to_owned())
            }
        }
    }
}

impl<'de> Deserialize<'de> for PathWire {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PathWireVisitor;

        impl<'de> Visitor<'de> for PathWireVisitor {
            type Value = PathWire;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a path string or tagged native-path wire object")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(PathWire::Legacy(value.to_owned()))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(PathWire::Legacy(value))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut kind = None::<String>;
                let mut bytes = None::<String>;
                let mut units = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => kind = Some(map.next_value()?),
                        "bytes" => bytes = Some(map.next_value()?),
                        "units" => units = Some(map.next_value()?),
                        _ => {
                            let _: IgnoredAny = map.next_value()?;
                        }
                    }
                }

                match kind.as_deref() {
                    Some("unix") => {
                        let bytes =
                            bytes.ok_or_else(|| serde::de::Error::missing_field("bytes"))?;
                        Ok(PathWire::Unix(bytes))
                    }
                    Some("windows") => {
                        let units =
                            units.ok_or_else(|| serde::de::Error::missing_field("units"))?;
                        Ok(PathWire::Windows(units))
                    }
                    Some(other) => Err(serde::de::Error::custom(format_args!(
                        "unknown path wire kind: {other}"
                    ))),
                    None => Err(serde::de::Error::missing_field("kind")),
                }
            }
        }

        deserializer.deserialize_any(PathWireVisitor)
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(text: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(text)
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
    #[error("invalid session id {0:?}")]
    InvalidId(String),
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
        validate_session_id(&entry.session_id)
            .map_err(|_| SessionIndexError::InvalidId(entry.session_id.clone()))?;
        let mut line =
            serde_json::to_vec(entry).map_err(|source| SessionIndexError::Json { source })?;
        line.push(b'\n');
        if let Some(parent) = self.index_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.index_path)?;
        file.lock()?;
        file.write_all(&line)?;
        Ok(())
    }

    /// Find the most recent entry for the given session ID.
    /// Scans from the end of the file so that the latest appended entry wins.
    pub fn find(&self, session_id: &str) -> Result<Option<SessionIndexEntry>, SessionIndexError> {
        validate_session_id(session_id)
            .map_err(|_| SessionIndexError::InvalidId(session_id.to_owned()))?;
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
            if let Ok(entry) = serde_json::from_str::<SessionIndexEntry>(line)
                && validate_session_id(&entry.session_id).is_ok()
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// Read entries whose session bucket exists under `sessions_root`.
    pub fn list_all_in_sessions_root(
        &self,
        sessions_root: &Path,
    ) -> Result<Vec<SessionIndexEntry>, SessionIndexError> {
        let canonical_root = match std::fs::canonicalize(sessions_root) {
            Ok(root) => root,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SessionIndexError::Io(error)),
        };
        let mut entries = Vec::new();

        for mut entry in self.list_all()? {
            let bucket_dir = if entry.session_dir.is_absolute() {
                entry.session_dir.clone()
            } else {
                sessions_root.join(&entry.session_dir)
            };
            let canonical_bucket = match std::fs::canonicalize(&bucket_dir) {
                Ok(bucket) => bucket,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(SessionIndexError::Io(error)),
            };
            if canonical_bucket.starts_with(&canonical_root) {
                entry.session_dir = bucket_dir;
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
        let entries = self.list_all_in_sessions_root(sessions_root)?;
        let mut summaries = Vec::new();

        for entry in entries {
            let bucket_dir = entry.session_dir;
            let metadata_path = bucket_dir.join("sessions.metadata.json");

            let content = match std::fs::read_to_string(&metadata_path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(SessionIndexError::Io(error)),
            };

            let Ok(metadata) = serde_json::from_str::<SessionMetadataFile>(&content) else {
                continue;
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
                pinned: stored.pinned,
                archived: stored.archived,
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
    let file = open_index_file_async(index_path).await?;
    let Some(file) = file else {
        return Ok(Vec::new());
    };
    let mut reader = tokio::io::BufReader::new(file);
    collect_index_entries_async(&mut reader).await
}

async fn open_index_file_async(index_path: &Path) -> Result<Option<File>, SessionError> {
    match File::open(index_path).await {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(SessionError::Io(error)),
    }
}

async fn collect_index_entries_async<R>(
    reader: &mut R,
) -> Result<Vec<SessionIndexEntry>, SessionError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
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
        if let Some(entry) = parse_index_entry_line(&line_buf) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

fn parse_index_entry_line(line: &str) -> Option<SessionIndexEntry> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let entry = serde_json::from_str::<SessionIndexEntry>(trimmed).ok()?;
    validate_session_id(&entry.session_id)
        .is_ok()
        .then_some(entry)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn append_and_find() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());
        let session_id = "session_00000000-0000-4000-8000-000000000001";

        let entry = SessionIndexEntry {
            session_id: session_id.to_owned(),
            session_dir: tmp.path().join(format!("wd_neo_abc123/{session_id}")),
            workdir: PathBuf::from("/home/user/neo"),
        };
        index.append(&entry).unwrap();

        let found = index.find(session_id).unwrap();
        assert_eq!(found.as_ref().unwrap().session_id, session_id);
        assert_eq!(
            found.as_ref().unwrap().workdir,
            PathBuf::from("/home/user/neo")
        );
    }

    #[test]
    fn concurrent_handles_append_complete_records() {
        const WRITERS: usize = 32;

        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);
        let barrier = Arc::new(Barrier::new(WRITERS));
        let mut writers = Vec::with_capacity(WRITERS);

        for writer in 0..WRITERS {
            let index = SessionIndex::from_path(index_path.clone());
            let barrier = barrier.clone();
            writers.push(std::thread::spawn(move || {
                let session_id = format!("session_00000000-0000-4000-8000-{writer:012x}");
                let entry = SessionIndexEntry {
                    session_dir: PathBuf::from(format!("bucket/{session_id}")),
                    workdir: PathBuf::from(format!("workspace/{writer}")),
                    session_id,
                };
                barrier.wait();
                index.append(&entry).unwrap();
            }));
        }

        for writer in writers {
            writer.join().unwrap();
        }

        let entries = SessionIndex::from_path(index_path).list_all().unwrap();
        assert_eq!(entries.len(), WRITERS);
    }

    #[test]
    fn append_rejects_legacy_numeric_session_ids() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        let entry = SessionIndexEntry {
            session_id: "1234567890".to_owned(),
            session_dir: tmp.path().join("wd_neo_abc123/1234567890"),
            workdir: PathBuf::from("/home/user/neo"),
        };

        assert!(matches!(
            index.append(&entry),
            Err(SessionIndexError::InvalidId(id)) if id == "1234567890"
        ));
    }

    #[test]
    fn find_missing_returns_none() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        let found = index
            .find("session_00000000-0000-4000-8000-000000000002")
            .unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn find_rejects_legacy_numeric_session_ids() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());

        assert!(matches!(
            index.find("1234567890"),
            Err(SessionIndexError::InvalidId(id)) if id == "1234567890"
        ));
    }

    #[test]
    fn find_latest_wins() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());
        let session_id = "session_00000000-0000-4000-8000-000000000003";

        index
            .append(&SessionIndexEntry {
                session_id: session_id.to_owned(),
                session_dir: tmp.path().join(session_id),
                workdir: PathBuf::from("/old"),
            })
            .unwrap();

        index
            .append(&SessionIndexEntry {
                session_id: session_id.to_owned(),
                session_dir: tmp.path().join(session_id),
                workdir: PathBuf::from("/new"),
            })
            .unwrap();

        let found = index.find(session_id).unwrap().unwrap();
        assert_eq!(found.workdir, PathBuf::from("/new"));
    }

    #[test]
    fn list_all_skips_malformed() {
        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);

        std::fs::write(
            &index_path,
            "{invalid json\n\
             {\"session_id\":\"session_00000000-0000-4000-8000-000000000004\",\"session_dir\":\"/a\",\"workdir\":\"/a\"}\n\
             {\"session_id\":\"1234567890\",\"session_dir\":\"/old\",\"workdir\":\"/old\"}\n",
        )
        .unwrap();

        let index = SessionIndex::from_path(index_path);
        let entries = index.list_all().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].session_id,
            "session_00000000-0000-4000-8000-000000000004"
        );
    }

    #[test]
    fn list_all_on_missing_file() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());
        let entries = index.list_all().unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn list_all_in_sessions_root_skips_external_and_missing_buckets() {
        let tmp = TempDir::new().unwrap();
        let sessions_root = tmp.path().join("sessions");
        let current_bucket = sessions_root.join("wd_current");
        let external_bucket = tmp.path().join("other-home/sessions/wd_external");
        std::fs::create_dir_all(&current_bucket).unwrap();
        std::fs::create_dir_all(&external_bucket).unwrap();
        let index = SessionIndex::new(tmp.path());

        for (suffix, session_dir) in [
            ("001", current_bucket.clone()),
            ("002", external_bucket),
            ("003", sessions_root.join("wd_missing")),
        ] {
            index
                .append(&SessionIndexEntry {
                    session_id: format!("session_00000000-0000-4000-8000-000000000{suffix}"),
                    session_dir,
                    workdir: tmp.path().join(format!("workspace-{suffix}")),
                })
                .unwrap();
        }

        let entries = index.list_all_in_sessions_root(&sessions_root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].session_dir, current_bucket);
    }

    #[tokio::test]
    async fn list_all_async_skips_empty_malformed_and_legacy_entries() {
        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);

        tokio::fs::write(
            &index_path,
            "\n\
             {invalid json\n\
             {\"session_id\":\"session_00000000-0000-4000-8000-000000000005\",\"session_dir\":\"/a\",\"workdir\":\"/a\"}\n\
             {\"session_id\":\"1234567890\",\"session_dir\":\"/old\",\"workdir\":\"/old\"}\n",
        )
        .await
        .unwrap();

        let entries = list_all_async(&index_path).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].session_id,
            "session_00000000-0000-4000-8000-000000000005"
        );
    }

    #[test]
    fn index_round_trips_native_non_unicode_paths() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new(tmp.path());
        let session_id = "session_00000000-0000-4000-8000-000000000010";

        #[cfg(unix)]
        let (session_dir, workdir) = {
            use std::os::unix::ffi::OsStringExt;

            let session_dir = PathBuf::from(std::ffi::OsString::from_vec(vec![
                b'/', b't', 0xff, b's', b'e', b's', b's',
            ]));
            let workdir = PathBuf::from(std::ffi::OsString::from_vec(vec![
                b'/', b'w', 0xff, b'r', b'k',
            ]));
            (session_dir, workdir)
        };

        #[cfg(windows)]
        let (session_dir, workdir) = {
            use std::os::windows::ffi::OsStringExt;

            // Unpaired UTF-16 surrogate code units that cannot be represented as
            // a Unicode string and therefore cannot survive a lossy conversion.
            let session_dir = PathBuf::from(std::ffi::OsString::from_wide(&[
                0x44, 0x65, 0x73, 0x6b, 0x74, 0x6f, 0x70, 0xD800, 0x5c, 0x73, 0x65, 0x73, 0x73,
            ]));
            let workdir = PathBuf::from(std::ffi::OsString::from_wide(&[
                0x43, 0x3a, 0x5c, 0x77, 0x6f, 0x72, 0x6b, 0xD83D,
            ]));
            (session_dir, workdir)
        };

        index
            .append(&SessionIndexEntry {
                session_id: session_id.to_owned(),
                session_dir: session_dir.clone(),
                workdir: workdir.clone(),
            })
            .unwrap();

        let found = index.find(session_id).unwrap().unwrap();
        assert_eq!(found.session_dir, session_dir);
        assert_eq!(found.workdir, workdir);

        let content = std::fs::read_to_string(&index.index_path).unwrap();
        let line = content.lines().next().unwrap();
        #[cfg(unix)]
        assert!(line.contains("\"kind\":\"unix\""));
        #[cfg(windows)]
        assert!(line.contains("\"kind\":\"windows\""));
    }

    #[test]
    fn index_reads_existing_unicode_record_without_rewrite() {
        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);
        let session_id = "session_00000000-0000-4000-8000-000000000099";
        let original = format!(
            "{{\"session_id\":\"{session_id}\",\"session_dir\":\"/old/session_dir\",\"workdir\":\"/old/workdir\"}}\n"
        );
        std::fs::write(&index_path, &original).unwrap();

        let index = SessionIndex::from_path(index_path.clone());
        let found = index.find(session_id).unwrap().unwrap();
        assert_eq!(found.session_dir, PathBuf::from("/old/session_dir"));
        assert_eq!(found.workdir, PathBuf::from("/old/workdir"));

        let after_read = std::fs::read_to_string(&index_path).unwrap();
        assert_eq!(after_read, original);

        index
            .append(&SessionIndexEntry {
                session_id: "session_00000000-0000-4000-8000-000000000100".to_owned(),
                session_dir: PathBuf::from("/new/session_dir"),
                workdir: PathBuf::from("/new/workdir"),
            })
            .unwrap();

        let content = std::fs::read_to_string(&index_path).unwrap();
        let mut lines = content.lines();
        assert_eq!(lines.next().unwrap(), original.trim_end_matches('\n'));
        let new_line = lines.next().unwrap();
        assert!(new_line.contains("\"v\":1"));
        assert!(new_line.contains("\"kind\":"));
    }

    #[test]
    fn index_rejects_foreign_and_invalid_wire_encodings() {
        let tmp = TempDir::new().unwrap();
        let index_path = tmp.path().join(INDEX_FILENAME);

        #[cfg(unix)]
        let foreign_line = r#"{"session_id":"session_00000000-0000-4000-8000-000000000020","session_dir":{"kind":"windows","units":"AA=="},"workdir":"/w"}"#;
        #[cfg(windows)]
        let foreign_line = r#"{"session_id":"session_00000000-0000-4000-8000-000000000020","session_dir":{"kind":"unix","bytes":"//8="},"workdir":"C:\\w"}"#;

        let invalid_line = r#"{"session_id":"session_00000000-0000-4000-8000-000000000021","session_dir":{"kind":"unix","bytes":"!!!"},"workdir":"/w"}"#;
        std::fs::write(&index_path, format!("{foreign_line}\n{invalid_line}\n")).unwrap();

        let entries = SessionIndex::from_path(index_path).list_all().unwrap();
        assert!(entries.is_empty());
    }
}
