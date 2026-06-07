use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use uuid::Uuid;

use crate::{AgentContext, AgentEvent, AgentMessage};

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
    #[error("invalid session id {0:?}")]
    InvalidId(String),
    #[error("session {0:?} does not exist")]
    MissingSession(String),
}

pub struct JsonlSessionWriter {
    writer: BufWriter<File>,
}

impl JsonlSessionWriter {
    pub async fn create(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .await?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
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
        let line = serde_json::to_string(event)
            .map_err(|source| SessionError::Json { line: 0, source })?;
        self.writer.write_all(line.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
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
            let event = serde_json::from_str(&line).map_err(|source| SessionError::Json {
                line: line_number,
                source,
            })?;
            events.push(event);
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub id: String,
    pub name: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SessionMetadataStore {
    sessions_dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionMetadataFile {
    #[serde(default)]
    sessions: BTreeMap<String, StoredSessionMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredSessionMetadata {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
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

    pub fn fork(
        &self,
        parent_id: &str,
        name: Option<String>,
    ) -> Result<SessionRecord, SessionError> {
        validate_session_id(parent_id)?;
        self.ensure_session_exists(parent_id)?;
        std::fs::create_dir_all(&self.sessions_dir)?;

        let child_id = self.next_child_id(parent_id)?;
        std::fs::copy(self.session_path(parent_id), self.session_path(&child_id))?;

        let mut metadata = self.read_metadata()?;
        metadata.sessions.entry(parent_id.to_owned()).or_default();
        metadata.sessions.insert(
            child_id.clone(),
            StoredSessionMetadata {
                name,
                parent_id: Some(parent_id.to_owned()),
            },
        );
        self.write_metadata(&metadata)?;

        Ok(self
            .list()?
            .into_iter()
            .find(|session| session.id == child_id)
            .expect("forked session should be listable"))
    }

    fn metadata_path(&self) -> PathBuf {
        self.sessions_dir.join("sessions.metadata.json")
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{session_id}.jsonl"))
    }

    fn ensure_session_exists(&self, session_id: &str) -> Result<(), SessionError> {
        if self.session_path(session_id).is_file() {
            Ok(())
        } else {
            Err(SessionError::MissingSession(session_id.to_owned()))
        }
    }

    fn next_child_id(&self, parent_id: &str) -> Result<String, SessionError> {
        loop {
            let uuid = Uuid::new_v4().simple().to_string();
            let child_id = format!("{parent_id}-fork-{}", &uuid[..8]);
            if !self.session_path(&child_id).exists() {
                return Ok(child_id);
            }
        }
    }

    fn read_metadata(&self) -> Result<SessionMetadataFile, SessionError> {
        let path = self.metadata_path();
        if !path.exists() {
            return Ok(SessionMetadataFile::default());
        }
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|source| SessionError::Json { line: 0, source })
    }

    fn write_metadata(&self, metadata: &SessionMetadataFile) -> Result<(), SessionError> {
        std::fs::create_dir_all(&self.sessions_dir)?;
        let content = serde_json::to_string_pretty(metadata)
            .map_err(|source| SessionError::Json { line: 0, source })?;
        std::fs::write(self.metadata_path(), format!("{content}\n"))?;
        Ok(())
    }

    fn session_ids(&self) -> Result<BTreeSet<String>, SessionError> {
        let mut ids = BTreeSet::new();
        if self.sessions_dir.exists() {
            for entry in std::fs::read_dir(&self.sessions_dir)? {
                let path = entry?.path();
                if path.extension() == Some(OsStr::new("jsonl"))
                    && let Some(id) = path.file_stem().and_then(OsStr::to_str)
                    && validate_session_id(id).is_ok()
                {
                    ids.insert(id.to_owned());
                }
            }
        }
        Ok(ids)
    }
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
            SessionRecord {
                children: children_by_parent.remove(&id).unwrap_or_default(),
                id,
                name: stored.and_then(|record| record.name.clone()),
                parent_id: stored.and_then(|record| record.parent_id.clone()),
            }
        })
        .collect()
}

pub fn validate_session_id(session_id: &str) -> Result<(), SessionError> {
    if !session_id.is_empty()
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
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
