use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;

use super::SessionError;
use super::atomic_file::write_file_atomic;
use super::layout::{MAIN_AGENT_ID, relative_agent_record_dir, session_state_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAgentKind {
    Main,
    Sub,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    pub kind: SessionAgentKind,
    pub record_dir: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swarm_item: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub schema_version: u32,
    pub agents: BTreeMap<String, SessionAgentRecord>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: 1,
            agents: BTreeMap::new(),
        }
    }

    pub fn ensure_main_agent(&mut self) {
        self.agents
            .entry(MAIN_AGENT_ID.to_owned())
            .or_insert_with(|| SessionAgentRecord {
                kind: SessionAgentKind::Main,
                record_dir: relative_agent_record_dir(MAIN_AGENT_ID),
                parent_agent_id: None,
                role: None,
                swarm_id: None,
                swarm_item: None,
            });
    }

    pub fn upsert_agent(&mut self, record: SessionAgentRecord) {
        let Some(agent_id) = record
            .record_dir
            .file_name()
            .and_then(|file_name| file_name.to_str())
            .map(str::to_owned)
        else {
            return;
        };

        self.agents.insert(agent_id, record);
    }
}

#[derive(Debug, Clone)]
pub struct SessionStateStore {
    session_dir: PathBuf,
}

impl SessionStateStore {
    #[must_use]
    pub fn new(session_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_dir: session_dir.into(),
        }
    }

    #[must_use]
    pub fn path(&self) -> PathBuf {
        session_state_path(&self.session_dir)
    }

    pub async fn read(&self) -> Result<SessionState, SessionError> {
        let path = self.path();
        let content = match fs::read_to_string(&path).await {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut state = SessionState::new();
                state.ensure_main_agent();
                return Ok(state);
            }
            Err(error) => return Err(SessionError::Io(error)),
        };

        let mut state: SessionState =
            serde_json::from_str(&content).map_err(|source| SessionError::Json {
                line: source.line(),
                source,
            })?;
        state.ensure_main_agent();
        Ok(state)
    }

    pub async fn write(&self, state: &SessionState) -> Result<(), SessionError> {
        let path = self.path();
        let content = serde_json::to_string_pretty(state)
            .map_err(|source| SessionError::Json { line: 0, source })?;
        write_file_atomic(&path, content.as_bytes())?;
        Ok(())
    }
}
