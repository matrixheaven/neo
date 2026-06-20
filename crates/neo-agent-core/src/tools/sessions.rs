use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::session::validate_session_id;
use crate::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SummarizeScope {
    SessionId { session_id: String },
    Days { days: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeSessionsArgs {
    #[serde(flatten)]
    pub scope: SummarizeScope,
}

pub struct SummarizeSessionsTool {
    neo_home: PathBuf,
}

impl SummarizeSessionsTool {
    #[must_use]
    pub fn new(neo_home: impl Into<PathBuf>) -> Self {
        Self {
            neo_home: neo_home.into(),
        }
    }
}

impl Tool for SummarizeSessionsTool {
    fn name(&self) -> &'static str {
        "SummarizeSessions"
    }

    fn description(&self) -> &'static str {
        "Read one or more local session transcripts and return a compact, human-readable summary suitable for turning the work into a reusable skill."
    }

    fn input_schema(&self) -> serde_json::Value {
        neo_ai::tool_schema::schema_for::<SummarizeSessionsArgs>()
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        let args = serde_json::from_value::<SummarizeSessionsArgs>(input).map_err(|err| {
            ToolError::InvalidInput {
                tool: "SummarizeSessions".to_owned(),
                message: err.to_string(),
            }
        });
        let neo_home = self.neo_home.clone();
        Box::pin(async move {
            let args = args?;
            let mut summaries = Vec::new();
            match args.scope {
                SummarizeScope::SessionId { session_id } => {
                    let path = find_session_path(&neo_home, &session_id).await?;
                    summaries.push(summarize_session(&path).await?);
                }
                SummarizeScope::Days { days } => {
                    let paths = list_recent_sessions(&neo_home, days).await?;
                    if paths.is_empty() {
                        return Ok(ToolResult::ok(
                            "No sessions found in the requested time range.".to_owned(),
                        ));
                    }
                    for path in paths {
                        summaries.push(summarize_session(&path).await?);
                    }
                }
            }
            Ok(ToolResult::ok(summaries.join("\n\n---\n\n")))
        })
    }
}

async fn find_session_path(neo_home: &Path, session_id: &str) -> Result<PathBuf, ToolError> {
    validate_tool_session_id(session_id)?;
    let index_path = neo_home.join("session_index.jsonl");
    if !index_path.exists() {
        return Err(ToolError::InvalidInput {
            tool: "SummarizeSessions".to_owned(),
            message: "session index not found".to_owned(),
        });
    }
    let content = fs::read_to_string(&index_path)
        .await
        .map_err(ToolError::Io)?;
    for line in content.lines() {
        let entry: serde_json::Value =
            serde_json::from_str(line).map_err(|err| ToolError::InvalidInput {
                tool: "SummarizeSessions".to_owned(),
                message: err.to_string(),
            })?;
        if entry["session_id"].as_str() == Some(session_id) {
            let session_dir =
                entry["session_dir"]
                    .as_str()
                    .ok_or_else(|| ToolError::InvalidInput {
                        tool: "SummarizeSessions".to_owned(),
                        message: "session index entry missing session_dir".to_owned(),
                    })?;
            return Ok(PathBuf::from(session_dir).join(format!("{session_id}.jsonl")));
        }
    }
    Err(ToolError::InvalidInput {
        tool: "SummarizeSessions".to_owned(),
        message: format!("session {session_id} not found in index"),
    })
}

async fn list_recent_sessions(neo_home: &Path, days: u32) -> Result<Vec<PathBuf>, ToolError> {
    let index_path = neo_home.join("session_index.jsonl");
    if !index_path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&index_path)
        .await
        .map_err(ToolError::Io)?;
    let cutoff = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
    .saturating_sub(u64::from(days) * 24 * 60 * 60 * 1000);
    let mut paths = Vec::new();
    for line in content.lines() {
        let entry: serde_json::Value =
            serde_json::from_str(line).map_err(|err| ToolError::InvalidInput {
                tool: "SummarizeSessions".to_owned(),
                message: err.to_string(),
            })?;
        let ts = entry["timestamp_ms"].as_u64().unwrap_or(0);
        if ts >= cutoff
            && let (Some(session_id), Some(session_dir)) =
                (entry["session_id"].as_str(), entry["session_dir"].as_str())
            && validate_session_id(session_id).is_ok()
        {
            paths.push(PathBuf::from(session_dir).join(format!("{session_id}.jsonl")));
        }
    }
    Ok(paths)
}

fn validate_tool_session_id(session_id: &str) -> Result<(), ToolError> {
    validate_session_id(session_id).map_err(|_| ToolError::InvalidInput {
        tool: "SummarizeSessions".to_owned(),
        message: format!("invalid session id {session_id:?}"),
    })
}

async fn summarize_session(path: &Path) -> Result<String, ToolError> {
    if !path.exists() {
        return Err(ToolError::InvalidInput {
            tool: "SummarizeSessions".to_owned(),
            message: format!("session file not found: {}", path.display()),
        });
    }
    let content = fs::read_to_string(path).await.map_err(ToolError::Io)?;
    let mut lines = vec![format!(
        "Session: {}",
        path.file_stem().unwrap_or_default().to_string_lossy()
    )];
    let mut turn = 0u32;
    for line in content.lines() {
        let event: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
        match event["type"].as_str() {
            Some("user") => {
                turn += 1;
                if let Some(text) = event["content"]["text"].as_str() {
                    lines.push(format!("Turn {turn} user: {text}"));
                }
            }
            Some("assistant") => {
                if let Some(text) = event["content"]["text"].as_str() {
                    lines.push(format!("Turn {turn} assistant: {text}"));
                }
            }
            Some("tool_result") => {
                if let Some(name) = event["name"].as_str() {
                    lines.push(format!("  tool {name} executed"));
                }
            }
            _ => {}
        }
    }
    Ok(lines.join("\n"))
}
