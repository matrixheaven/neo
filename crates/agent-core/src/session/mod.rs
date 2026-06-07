use std::path::Path;

use thiserror::Error;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::{AgentEvent, AgentMessage};

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
