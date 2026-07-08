use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use uuid::Uuid;

use crate::runtime::estimate_messages_tokens;
use crate::{AgentContext, AgentEvent, AgentMessage, CompactionSummary, Content};

pub mod agent_state;
pub(crate) mod atomic_file;
pub mod export;
pub mod index;
pub mod layout;
pub mod workspace;

pub use agent_state::{SessionAgentKind, SessionAgentRecord, SessionState, SessionStateStore};
pub use index::{SessionIndex, SessionIndexEntry, SessionIndexError};
pub use layout::{
    AGENTS_DIR, GOALS_DIR, MAIN_AGENT_ID, PLANS_DIR, SESSION_STATE_FILE, TASKS_DIR, WIRE_FILE,
    agent_goals_dir, agent_plans_dir, agent_record_dir, agent_tasks_dir, agent_wire_path,
    agents_dir, main_agent_goals_dir, main_agent_plans_dir, main_agent_wire_path,
    relative_agent_record_dir, session_state_path,
};
pub use workspace::{
    encode_workdir_key, normalize_workdir, slugify_basename, workspace_sessions_dir,
};

const SESSION_FORMAT_NAME: &str = "neo.session.jsonl";
const SESSION_SCHEMA_VERSION: u32 = 1;
const SESSION_METADATA_KIND: &str = "session_metadata";
const SESSION_ID_PREFIX: &str = "session_";

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session JSON failed on line {line}: {source}")]
    Json {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported session metadata schema version {0}")]
    UnsupportedMetadataSchemaVersion(u32),
    #[error("unsupported session metadata format {0:?}")]
    UnsupportedMetadataFormat(String),
    #[error("invalid session id {0:?}")]
    InvalidId(String),
    #[error("session {0:?} does not exist")]
    MissingSession(String),
}

pub struct JsonlSessionWriter {
    writer: BufWriter<File>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SessionSchemaMetadata {
    kind: String,
    format: String,
    schema_version: u32,
    created_at: String,
}

impl JsonlSessionWriter {
    pub async fn create(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .await?;
        let mut session = Self {
            writer: BufWriter::new(file),
        };
        session.append_metadata().await?;
        Ok(session)
    }

    pub async fn open_append(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    pub async fn append_event(&mut self, event: &AgentEvent) -> Result<(), SessionError> {
        self.append(event).await
    }

    pub async fn append(&mut self, event: &AgentEvent) -> Result<(), SessionError> {
        self.append_json_line(event).await
    }

    async fn append_metadata(&mut self) -> Result<(), SessionError> {
        self.append_json_line(&SessionSchemaMetadata {
            kind: SESSION_METADATA_KIND.to_owned(),
            format: SESSION_FORMAT_NAME.to_owned(),
            schema_version: SESSION_SCHEMA_VERSION,
            created_at: current_unix_timestamp(),
        })
        .await
    }

    async fn append_json_line<T: Serialize>(&mut self, record: &T) -> Result<(), SessionError> {
        let mut line =
            serde_json::to_vec(record).map_err(|source| SessionError::Json { line: 0, source })?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<(), SessionError> {
        self.writer.flush().await?;
        Ok(())
    }
}

pub struct JsonlSessionReader;

impl JsonlSessionReader {
    pub async fn read_all(path: impl AsRef<Path>) -> Result<Vec<AgentEvent>, SessionError> {
        let file = File::open(path).await?;
        let mut lines = BufReader::new(file).lines();
        let mut events = Vec::new();
        let mut line_number = 0;

        while let Some(line) = lines.next_line().await? {
            line_number += 1;
            if line.trim().is_empty() {
                continue;
            }
            for value in serde_json::Deserializer::from_str(&line).into_iter::<serde_json::Value>()
            {
                let value = value.map_err(|source| SessionError::Json {
                    line: line_number,
                    source,
                })?;
                if read_session_metadata_value(&value, line_number)? {
                    continue;
                }
                let event = serde_json::from_value(value).map_err(|source| SessionError::Json {
                    line: line_number,
                    source,
                })?;
                events.push(event);
            }
        }

        Ok(events)
    }

    pub async fn replay_messages(
        path: impl AsRef<Path>,
    ) -> Result<Vec<AgentMessage>, SessionError> {
        let events = Self::read_all(path).await?;
        Ok(replay_messages(events.iter()))
    }

    pub async fn replay_context(path: impl AsRef<Path>) -> Result<AgentContext, SessionError> {
        let events = Self::read_all(path).await?;
        Ok(AgentContext::from_replay(events.iter()))
    }
}

fn read_session_metadata_value(
    value: &serde_json::Value,
    line_number: usize,
) -> Result<bool, SessionError> {
    let is_metadata = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind == SESSION_METADATA_KIND);
    if !is_metadata {
        return Ok(false);
    }

    let metadata =
        serde_json::from_value::<SessionSchemaMetadata>(value.clone()).map_err(|source| {
            SessionError::Json {
                line: line_number,
                source,
            }
        })?;
    if metadata.format != SESSION_FORMAT_NAME {
        return Err(SessionError::UnsupportedMetadataFormat(metadata.format));
    }
    if metadata.schema_version != SESSION_SCHEMA_VERSION {
        return Err(SessionError::UnsupportedMetadataSchemaVersion(
            metadata.schema_version,
        ));
    }
    Ok(true)
}

fn current_unix_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}.{:09}Z", duration.as_secs(), duration.subsec_nanos())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCompactionOptions {
    pub keep_recent_messages: usize,
}

impl Default for SessionCompactionOptions {
    fn default() -> Self {
        Self {
            keep_recent_messages: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCompactionResult {
    pub summary: CompactionSummary,
    pub compacted_message_count: usize,
    pub kept_message_count: usize,
}

pub async fn compact_jsonl_session(
    path: impl AsRef<Path>,
    options: SessionCompactionOptions,
) -> Result<SessionCompactionResult, SessionError> {
    let path = path.as_ref();
    let events = JsonlSessionReader::read_all(path).await?;
    let context = AgentContext::from_replay(events.iter());
    let messages = context.messages();
    let keep_recent_messages = options.keep_recent_messages.min(messages.len());
    let first_kept_message_index = messages.len().saturating_sub(keep_recent_messages);
    let compacted_messages = &messages[..first_kept_message_index];
    let kept_messages = messages.len().saturating_sub(first_kept_message_index);
    let summary = CompactionSummary {
        summary: summarize_transcript(compacted_messages),
        tokens_before: estimate_messages_tokens(messages),
        tokens_after: estimate_messages_tokens(&messages[first_kept_message_index..]),
        first_kept_message_index,
    };

    let mut writer = JsonlSessionWriter::open_append(path).await?;
    writer
        .append(&AgentEvent::CompactionApplied {
            summary: summary.clone(),
        })
        .await?;
    writer.flush().await?;

    Ok(SessionCompactionResult {
        summary,
        compacted_message_count: compacted_messages.len(),
        kept_message_count: kept_messages,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub parent_id: Option<String>,
    pub summary_record: Option<SessionSummaryRecord>,
    pub title: Option<String>,
    pub title_model: Option<String>,
    pub title_updated_at: Option<String>,
    pub workspace: Option<String>,
    pub last_user_prompt: Option<String>,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

/// Lightweight summary of a session for picker UIs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
    pub last_prompt: Option<String>,
    pub work_dir: PathBuf,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl SessionSummary {
    #[must_use]
    pub fn from_record(record: SessionRecord, work_dir: impl AsRef<Path>) -> Self {
        Self {
            id: record.id,
            title: record
                .name
                .clone()
                .or_else(|| record.title.clone())
                .or_else(|| record.last_user_prompt.clone()),
            last_prompt: record.last_user_prompt,
            work_dir: work_dir.as_ref().to_path_buf(),
            updated_at: record.updated_at.unwrap_or_default(),
            metadata: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummaryRecord {
    pub text: String,
    pub source: SessionSummarySource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionSummarySource {
    LocalExtractive,
    ModelGenerated,
}

#[derive(Debug, Clone)]
pub struct SessionMetadataStore {
    sessions_dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct SessionMetadataFile {
    #[serde(default)]
    sessions: BTreeMap<String, StoredSessionMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct StoredSessionMetadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    summary_record: Option<SessionSummaryRecord>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    title_model: Option<String>,
    #[serde(default)]
    title_updated_at: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    last_user_prompt: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl SessionMetadataStore {
    #[must_use]
    pub fn new(sessions_dir: impl AsRef<Path>) -> Self {
        Self {
            sessions_dir: sessions_dir.as_ref().to_path_buf(),
        }
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let metadata = self.read_metadata()?;
        let session_ids = self.session_ids()?;
        Ok(records_from_metadata(&metadata, session_ids))
    }

    pub fn list_recent(&self) -> Result<Vec<SessionRecord>, SessionError> {
        let mut records = self.list()?;
        records.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        Ok(records)
    }

    pub fn list_summaries(
        &self,
        work_dir: impl AsRef<Path>,
    ) -> Result<Vec<SessionSummary>, SessionError> {
        let work_dir = work_dir.as_ref().to_path_buf();
        let records = self.list_recent()?;
        Ok(records
            .into_iter()
            .map(|record| SessionSummary::from_record(record, &work_dir))
            .collect())
    }

    pub fn rename(&self, session_id: &str, name: String) -> Result<SessionRecord, SessionError> {
        validate_session_id(session_id)?;
        self.ensure_session_exists(session_id)?;

        let mut metadata = self.read_metadata()?;
        metadata
            .sessions
            .entry(session_id.to_owned())
            .or_default()
            .name = Some(name);
        self.write_metadata(&metadata)?;

        Ok(self
            .list()?
            .into_iter()
            .find(|session| session.id == session_id)
            .expect("renamed session should be listable"))
    }

    pub fn summarize(
        &self,
        session_id: &str,
        summary: String,
    ) -> Result<SessionRecord, SessionError> {
        self.record_summary(
            session_id,
            SessionSummaryRecord {
                text: summary,
                source: SessionSummarySource::LocalExtractive,
                model: None,
                updated_at: None,
            },
        )
    }

    pub fn record_summary(
        &self,
        session_id: &str,
        summary: SessionSummaryRecord,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(session_id)?;
        self.ensure_session_exists(session_id)?;

        let mut metadata = self.read_metadata()?;
        let stored = metadata.sessions.entry(session_id.to_owned()).or_default();
        stored.summary = Some(summary.text.clone());
        stored.summary_record = Some(summary);
        self.write_metadata(&metadata)?;

        Ok(self
            .list()?
            .into_iter()
            .find(|session| session.id == session_id)
            .expect("summarized session should be listable"))
    }

    pub fn record_activity(
        &self,
        session_id: &str,
        workspace: Option<String>,
        last_user_prompt: Option<String>,
        updated_at: String,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(session_id)?;
        self.ensure_session_exists(session_id)?;

        let mut metadata = self.read_metadata()?;
        let stored = metadata.sessions.entry(session_id.to_owned()).or_default();
        stored.workspace = workspace;
        stored.last_user_prompt = last_user_prompt;
        stored.updated_at = Some(updated_at);
        self.write_metadata(&metadata)?;

        Ok(self
            .list()?
            .into_iter()
            .find(|session| session.id == session_id)
            .expect("active session should be listable"))
    }

    pub fn record_title(
        &self,
        session_id: &str,
        title: String,
        model: Option<String>,
        updated_at: String,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(session_id)?;
        self.ensure_session_exists(session_id)?;

        let mut metadata = self.read_metadata()?;
        let stored = metadata.sessions.entry(session_id.to_owned()).or_default();
        if stored.name.is_some() {
            return Ok(self
                .list()?
                .into_iter()
                .find(|session| session.id == session_id)
                .expect("named session should be listable"));
        }
        stored.title = Some(title);
        stored.title_model = model;
        stored.title_updated_at = Some(updated_at);
        self.write_metadata(&metadata)?;

        Ok(self
            .list()?
            .into_iter()
            .find(|session| session.id == session_id)
            .expect("titled session should be listable"))
    }

    pub fn fork(
        &self,
        parent_id: &str,
        name: Option<String>,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(parent_id)?;
        self.ensure_session_exists(parent_id)?;
        ensure_safe_directory_tree(&self.sessions_dir)?;

        let child_id = self.next_child_id()?;
        let parent_dir = self.session_dir(parent_id);
        let child_dir = self.session_dir(&child_id);
        if let Err(error) = copy_dir_all(&parent_dir, &child_dir) {
            let _ = fs::remove_dir_all(&child_dir);
            return Err(SessionError::Io(error));
        }

        let result = self.record_fork_metadata(parent_id, &child_id, name);
        if result.is_err() {
            let _ = fs::remove_dir_all(&child_dir);
        }
        result
    }

    fn record_fork_metadata(
        &self,
        parent_id: &str,
        child_id: &str,
        name: Option<String>,
    ) -> Result<SessionRecord, SessionError> {
        let mut metadata = self.read_metadata()?;
        let parent_stored = metadata
            .sessions
            .entry(parent_id.to_owned())
            .or_default()
            .clone();
        let now = current_unix_timestamp();
        let fork_title = parent_stored
            .name
            .clone()
            .or_else(|| parent_stored.title.clone())
            .or_else(|| parent_stored.last_user_prompt.clone())
            .map(|t| format!("[fork] {t}"));
        metadata.sessions.insert(
            child_id.to_owned(),
            StoredSessionMetadata {
                name,
                summary: parent_stored.summary.clone(),
                summary_record: parent_stored.summary_record.clone(),
                parent_id: Some(parent_id.to_owned()),
                title: fork_title,
                title_model: None,
                title_updated_at: Some(now.clone()),
                workspace: parent_stored.workspace.clone(),
                last_user_prompt: parent_stored.last_user_prompt.clone(),
                updated_at: Some(now),
            },
        );
        self.write_metadata(&metadata)?;

        self.list()?
            .into_iter()
            .find(|session| session.id == child_id)
            .ok_or_else(|| SessionError::MissingSession(child_id.to_owned()))
    }

    fn metadata_path(&self) -> PathBuf {
        self.sessions_dir.join("sessions.metadata.json")
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        main_agent_wire_path(&self.session_dir(session_id))
    }

    fn session_dir(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(session_id)
    }

    fn ensure_session_exists(&self, session_id: &str) -> Result<(), SessionError> {
        ensure_existing_safe_directory_tree(&self.sessions_dir)?;
        let path = self.session_path(session_id);
        let session_dir =
            ensure_existing_child_directory_tree(&self.sessions_dir, Path::new(session_id))?;
        ensure_existing_child_directory_tree(&session_dir, Path::new(AGENTS_DIR))?;
        let parent = path.parent().ok_or_else(|| {
            SessionError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path has no parent directory: {}", path.display()),
            ))
        })?;
        ensure_existing_safe_directory_tree(parent)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(SessionError::MissingSession(session_id.to_owned()));
            }
            Err(error) => return Err(SessionError::Io(error)),
        };
        if is_reparse_or_symlink(&metadata) {
            return Err(SessionError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing symlinked session transcript {}", path.display()),
            )));
        }
        if !metadata.is_file() {
            return Err(SessionError::MissingSession(session_id.to_owned()));
        }
        Ok(())
    }

    fn next_child_id(&self) -> Result<String, SessionError> {
        loop {
            let child_id = format!("{SESSION_ID_PREFIX}{}", Uuid::new_v4());
            match fs::symlink_metadata(self.session_dir(&child_id)) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(child_id),
                Err(error) => return Err(SessionError::Io(error)),
            }
        }
    }

    fn read_metadata(&self) -> Result<SessionMetadataFile, SessionError> {
        if !ensure_existing_safe_directory_tree_if_present(&self.sessions_dir)? {
            return Ok(SessionMetadataFile::default());
        }
        let path = self.metadata_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SessionMetadataFile::default());
            }
            Err(error) => return Err(SessionError::Io(error)),
        };
        if is_reparse_or_symlink(&metadata) {
            return Err(SessionError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing symlinked session metadata {}", path.display()),
            )));
        }
        if !metadata.is_file() {
            return Err(SessionError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("session metadata is not a file: {}", path.display()),
            )));
        }
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|source| SessionError::Json { line: 0, source })
    }

    fn write_metadata(&self, metadata: &SessionMetadataFile) -> Result<(), SessionError> {
        ensure_safe_directory_tree(&self.sessions_dir)?;
        let content = serde_json::to_string_pretty(metadata)
            .map_err(|source| SessionError::Json { line: 0, source })?;
        write_file_atomic(&self.metadata_path(), format!("{content}\n").as_bytes())?;
        Ok(())
    }

    fn session_ids(&self) -> Result<BTreeSet<String>, SessionError> {
        let mut ids = BTreeSet::new();
        if !ensure_existing_safe_directory_tree_if_present(&self.sessions_dir)? {
            return Ok(ids);
        }
        for entry in std::fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if !name.starts_with(SESSION_ID_PREFIX) {
                continue;
            }
            let wire_path = main_agent_wire_path(&path);
            let Ok(wire_metadata) = fs::symlink_metadata(&wire_path) else {
                continue;
            };
            if is_reparse_or_symlink(&wire_metadata) || !wire_metadata.is_file() {
                continue;
            }
            if validate_session_id(name).is_ok() {
                ids.insert(name.to_owned());
            }
        }
        Ok(ids)
    }
}

fn ensure_safe_directory_tree(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    validate_safe_directory(path)
}

fn ensure_existing_safe_directory_tree_if_present(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            ensure_existing_safe_directory_tree(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn ensure_existing_safe_directory_tree(path: &Path) -> io::Result<()> {
    validate_safe_directory(path)
}

fn ensure_existing_child_directory_tree(parent: &Path, child: &Path) -> io::Result<PathBuf> {
    validate_safe_directory(parent)?;
    let mut current = parent.to_path_buf();
    for component in child.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => {
                current.push(part);
                validate_safe_directory(&current)?;
            }
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("refusing unsafe child path: {}", child.display()),
                ));
            }
        }
    }
    Ok(current)
}

fn validate_safe_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing symlinked session directory {}", path.display()),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("session path is not a directory: {}", path.display()),
        ));
    }
    Ok(())
}

fn ensure_path_absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("path already exists: {}", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_new_safe_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path has no parent directory: {}", path.display()),
        )
    })?;
    ensure_existing_safe_directory_tree(parent)?;
    ensure_path_absent(path)?;
    fs::create_dir(path)?;
    validate_safe_directory(path)
}

/// Recursively copy a directory tree.
fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> std::io::Result<()> {
    let src = src.as_ref();
    let metadata = fs::symlink_metadata(src)?;
    if is_reparse_or_symlink(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to fork symlinked session root {}", src.display()),
        ));
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("session root is not a directory: {}", src.display()),
        ));
    }
    create_new_safe_directory(dst.as_ref())?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source_path = entry.path();
        let metadata = fs::symlink_metadata(&source_path)?;
        if is_reparse_or_symlink(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to fork symlinked session artifact {}",
                    source_path.display()
                ),
            ));
        }
        let ty = metadata.file_type();
        let destination = dst.as_ref().join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(source_path, destination)?;
        } else if ty.is_file() {
            fs::copy(source_path, destination)?;
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "refusing to fork non-file session artifact {}",
                    source_path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn write_file_atomic(path: &Path, content: &[u8]) -> io::Result<()> {
    atomic_file::write_file_atomic(path, content)
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || platform_reparse_point(metadata)
}

#[cfg(windows)]
fn platform_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn platform_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

fn records_from_metadata(
    metadata: &SessionMetadataFile,
    session_ids: BTreeSet<String>,
) -> Vec<SessionRecord> {
    let mut children_by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for session_id in &session_ids {
        if let Some(parent_id) = metadata
            .sessions
            .get(session_id)
            .and_then(|record| record.parent_id.as_ref())
            && session_ids.contains(parent_id)
        {
            children_by_parent
                .entry(parent_id.clone())
                .or_default()
                .push(session_id.clone());
        }
    }

    session_ids
        .into_iter()
        .map(|id| {
            let stored = metadata.sessions.get(&id);
            let summary_record = stored.and_then(summary_record_from_stored);
            let title = session_title(&id, stored);
            SessionRecord {
                children: children_by_parent.remove(&id).unwrap_or_default(),
                id,
                name: stored.and_then(|record| record.name.clone()),
                summary: summary_record
                    .as_ref()
                    .map(|summary| summary.text.clone())
                    .or_else(|| stored.and_then(|record| record.summary.clone())),
                parent_id: stored.and_then(|record| record.parent_id.clone()),
                summary_record,
                title,
                title_model: stored.and_then(|record| record.title_model.clone()),
                title_updated_at: stored.and_then(|record| record.title_updated_at.clone()),
                workspace: stored.and_then(|record| record.workspace.clone()),
                last_user_prompt: stored.and_then(|record| record.last_user_prompt.clone()),
                updated_at: stored.and_then(|record| record.updated_at.clone()),
            }
        })
        .collect()
}

fn session_title(id: &str, stored: Option<&StoredSessionMetadata>) -> Option<String> {
    stored
        .and_then(|record| record.name.clone())
        .or_else(|| stored.and_then(|record| record.title.clone()))
        .or_else(|| stored.and_then(|record| record.last_user_prompt.clone()))
        .or_else(|| Some(id.to_owned()))
}

fn summary_record_from_stored(stored: &StoredSessionMetadata) -> Option<SessionSummaryRecord> {
    stored.summary_record.clone().or_else(|| {
        stored.summary.as_ref().map(|summary| SessionSummaryRecord {
            text: summary.clone(),
            source: SessionSummarySource::LocalExtractive,
            model: None,
            updated_at: None,
        })
    })
}

pub fn validate_session_id(session_id: &str) -> Result<(), SessionError> {
    let Some(uuid) = session_id.strip_prefix(SESSION_ID_PREFIX) else {
        return Err(SessionError::InvalidId(session_id.to_owned()));
    };
    if Uuid::parse_str(uuid).is_ok_and(|parsed| parsed.to_string() == uuid) {
        Ok(())
    } else {
        Err(SessionError::InvalidId(session_id.to_owned()))
    }
}

#[must_use]
pub fn replay_messages<'a>(events: impl IntoIterator<Item = &'a AgentEvent>) -> Vec<AgentMessage> {
    events
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::MessageAppended { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn summarize_transcript(messages: &[AgentMessage]) -> String {
    if messages.is_empty() {
        return "Algorithmic transcript summary: no earlier messages were compacted.".to_owned();
    }

    let lines = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            format!(
                "{}. {}: {}",
                index + 1,
                message_role(message),
                one_line_message_text(message)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "Algorithmic transcript summary: {} earlier messages compacted by deterministic transcript extraction.\n{}",
        messages.len(),
        lines
    )
}

fn message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
        AgentMessage::ShellCommand { .. } => "shell",
    }
}

fn one_line_message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content, .. }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            exit_code,
            outcome,
            truncated,
        } => {
            return format!(
                "$ {} [{} exit={} truncated={}] {}{}",
                command,
                outcome.as_model_status(),
                exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                truncated,
                stdout.split_whitespace().collect::<Vec<_>>().join(" "),
                stderr.split_whitespace().collect::<Vec<_>>().join(" ")
            )
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        }
    };
    let text = content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
