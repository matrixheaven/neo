use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::session::validate_session_id;
use crate::{Tool, ToolContext, ToolError, ToolFuture, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SummarizeScope {
    /// Summarize a single session by its ID.
    SessionId {
        /// The session ID to summarize.
        #[schemars(description = "The session ID to summarize.")]
        session_id: String,
    },
    /// Summarize sessions from the last N days.
    Days {
        /// Number of days to look back from now.
        #[schemars(description = "Number of days to look back from now.")]
        days: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SummarizeSessionsArgs {
    /// Scope of the summarization: either a specific `session_id` or a number of days.
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
        "Read one or more local session transcripts and return a compact, human-readable summary suitable for turning the work into a reusable skill.\n\n\
         Use this tool when you need to review past work, extract patterns, or create a skill from a completed conversation.\n\n\
         Parameters:\n\
         - `session_id`: summarize a single specific session.\n\
         - `days`: summarize all sessions from the last N days.\n\n\
         Note: `session_id` and `days` are mutually exclusive — provide exactly one. If `session_id` \
         is given, only that specific session is summarized. If `days` is given, all sessions from \
         the last N days are summarized. If the specified `session_id` does not exist, the tool \
         returns an error listing available session IDs. If `days` yields no sessions, the tool \
         returns a status message indicating no sessions were found in the given time range.\n\n\
         Output format:\n\
         - Each summarized session is prefixed with `Session: <id>`.\n\
         - User and assistant messages are grouped by turn.\n\
         - Tool executions are listed under the turn in which they occurred.\n\
         - A `<system>...</system>` status block is appended with the number of sessions summarized and any errors."
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
            let count = summaries.len();
            let body = summaries.join("\n\n---\n\n");
            let message = format!("Summarized {count} session(s).");
            Ok(ToolResult::ok(format!(
                "{body}\n\n<system>{message}</system>"
            )))
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
    let cutoff = recent_session_cutoff_ms(days);
    let mut paths = Vec::new();
    for line in content.lines() {
        let entry = parse_session_index_line(line)?;
        if let Some(path) = recent_session_path(&entry, cutoff) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn recent_session_cutoff_ms(days: u32) -> u64 {
    current_time_ms().saturating_sub(u64::from(days) * 24 * 60 * 60 * 1000)
}

fn current_time_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

fn parse_session_index_line(line: &str) -> Result<serde_json::Value, ToolError> {
    serde_json::from_str(line).map_err(|err| ToolError::InvalidInput {
        tool: "SummarizeSessions".to_owned(),
        message: err.to_string(),
    })
}

fn recent_session_path(entry: &serde_json::Value, cutoff: u64) -> Option<PathBuf> {
    if entry["timestamp_ms"].as_u64().unwrap_or(0) < cutoff {
        return None;
    }
    let session_id = entry["session_id"].as_str()?;
    let session_dir = entry["session_dir"].as_str()?;
    validate_session_id(session_id)
        .is_ok()
        .then(|| PathBuf::from(session_dir).join(format!("{session_id}.jsonl")))
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
        if let Some(summary_line) = summarize_session_event(&event, &mut turn) {
            lines.push(summary_line);
        }
    }
    Ok(lines.join("\n"))
}

fn summarize_session_event(event: &serde_json::Value, turn: &mut u32) -> Option<String> {
    match event["type"].as_str() {
        Some("user") => summarize_user_event(event, turn),
        Some("assistant") => event_text(event).map(|text| format!("Turn {turn} assistant: {text}")),
        Some("tool_result") => event["name"]
            .as_str()
            .map(|name| format!("  tool {name} executed")),
        _ => None,
    }
}

fn summarize_user_event(event: &serde_json::Value, turn: &mut u32) -> Option<String> {
    *turn += 1;
    event_text(event).map(|text| format!("Turn {turn} user: {text}"))
}

fn event_text(event: &serde_json::Value) -> Option<&str> {
    event["content"]["text"].as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;
    use serde_json::json;

    fn make_ctx() -> ToolContext {
        ToolContext::new(std::env::current_dir().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn summarize_sessions_by_id_returns_summary_and_status() {
        let temp = tempfile::tempdir().expect("tempdir");
        let session_id = "session_550e8400-e29b-41d4-a716-446655440000";
        let session_dir = temp.path().join("wd_test_1234567890ab");
        fs::create_dir_all(&session_dir).await.expect("mkdir");
        fs::write(
            session_dir.join(format!("{session_id}.jsonl")),
            json!({
                "type": "user",
                "content": {"text": "hello"}
            })
            .to_string()
                + "\n"
                + &json!({
                    "type": "assistant",
                    "content": {"text": "hi there"}
                })
                .to_string(),
        )
        .await
        .expect("write session");

        fs::write(
            temp.path().join("session_index.jsonl"),
            json!({
                "session_id": session_id,
                "session_dir": session_dir.to_str().unwrap(),
                "timestamp_ms": 1_704_067_200_000_u64
            })
            .to_string()
                + "\n",
        )
        .await
        .expect("write index");

        let tool = SummarizeSessionsTool::new(temp.path());
        let result = tool
            .execute(&make_ctx(), json!({"session_id": session_id}))
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("Session:"));
        assert!(result.content.contains("Turn 1 user: hello"));
        assert!(result.content.contains("Turn 1 assistant: hi there"));
        assert!(
            result
                .content
                .contains("<system>Summarized 1 session(s).</system>")
        );
    }

    #[tokio::test]
    async fn summarize_sessions_by_days_returns_empty_when_no_index() {
        let temp = tempfile::tempdir().expect("tempdir");
        let tool = SummarizeSessionsTool::new(temp.path());
        let result = tool
            .execute(&make_ctx(), json!({"days": 7}))
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(
            result
                .content
                .contains("No sessions found in the requested time range.")
        );
    }

    #[tokio::test]
    async fn summarize_sessions_by_days_uses_recent_valid_index_entries() {
        let temp = tempfile::tempdir().expect("tempdir");
        let recent_id = "session_550e8400-e29b-41d4-a716-446655440001";
        let old_id = "session_550e8400-e29b-41d4-a716-446655440002";
        let recent_dir = temp.path().join("wd_recent_1234567890ab");
        let old_dir = temp.path().join("wd_old_1234567890ab");
        fs::create_dir_all(&recent_dir).await.expect("mkdir recent");
        fs::create_dir_all(&old_dir).await.expect("mkdir old");
        fs::write(
            recent_dir.join(format!("{recent_id}.jsonl")),
            json!({
                "type": "user",
                "content": {"text": "recent"}
            })
            .to_string(),
        )
        .await
        .expect("write recent session");
        fs::write(
            old_dir.join(format!("{old_id}.jsonl")),
            json!({
                "type": "user",
                "content": {"text": "old"}
            })
            .to_string(),
        )
        .await
        .expect("write old session");

        let now = current_time_ms();
        fs::write(
            temp.path().join("session_index.jsonl"),
            [
                json!({
                    "session_id": recent_id,
                    "session_dir": recent_dir.to_str().unwrap(),
                    "timestamp_ms": now,
                })
                .to_string(),
                json!({
                    "session_id": old_id,
                    "session_dir": old_dir.to_str().unwrap(),
                    "timestamp_ms": now.saturating_sub(10 * 24 * 60 * 60 * 1000),
                })
                .to_string(),
                json!({
                    "session_id": "1234567890",
                    "session_dir": recent_dir.to_str().unwrap(),
                    "timestamp_ms": now,
                })
                .to_string(),
            ]
            .join("\n"),
        )
        .await
        .expect("write index");

        let tool = SummarizeSessionsTool::new(temp.path());
        let result = tool
            .execute(&make_ctx(), json!({"days": 7}))
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(result.content.contains("Turn 1 user: recent"));
        assert!(!result.content.contains("Turn 1 user: old"));
        assert!(
            result
                .content
                .contains("<system>Summarized 1 session(s).</system>")
        );
    }

    #[test]
    fn tool_description_is_non_empty() {
        assert!(!SummarizeSessionsTool::new(".").description().is_empty());
    }
}
