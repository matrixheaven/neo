use std::{
    env, fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use neo_agent_core::AgentMessage;
use neo_agent_core::session::export::{ExportConversation, ExportMessage, HtmlExportOptions};
use neo_agent_core::session::{
    JsonlSessionReader, SessionCompactionOptions, SessionIndex, SessionMetadataStore,
    SessionSummary, compact_jsonl_session, validate_session_id,
};
use serde::Serialize;
use serde_json::Value;

use crate::config::{AppConfig, workspace_sessions_dir};

pub(crate) const MAX_RESUME_SESSION_BYTES: u64 = 512 * 1024 * 1024;

pub fn list(config: &AppConfig) -> anyhow::Result<String> {
    let bucket_dir = workspace_sessions_dir(config);
    let sessions = metadata_store(config)
        .list()
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?;

    if sessions.is_empty() {
        Ok("no sessions\n".to_owned())
    } else {
        let lines = sessions
            .iter()
            .map(|session| {
                let mut parts = vec![session.id.clone()];
                if let Some(name) = &session.name {
                    parts.push(name.clone());
                }
                if let Some(parent_id) = &session.parent_id {
                    parts.push(format!("parent={parent_id}"));
                }
                if let Some(summary) = &session.summary {
                    parts.push(format!("summary={summary}"));
                }
                if !session.children.is_empty() {
                    parts.push(format!("children={}", session.children.join(",")));
                }
                parts.join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!("{lines}\n"))
    }
}

/// Scope for the session picker data layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionPickerScope {
    Workspace,
    All,
}

/// Return session summaries for the current workspace or every known workspace.
pub(crate) fn session_summaries(
    config: &AppConfig,
    scope: SessionPickerScope,
) -> anyhow::Result<Vec<SessionSummary>> {
    match scope {
        SessionPickerScope::Workspace => metadata_store(config)
            .list_summaries(&config.project_dir)
            .map_err(|error| anyhow::anyhow!("failed to list workspace sessions: {error}")),
        SessionPickerScope::All => {
            let neo_home = config.sessions_dir.parent().ok_or_else(|| {
                anyhow::anyhow!(
                    "sessions dir {} has no parent directory",
                    config.sessions_dir.display()
                )
            })?;
            let index = SessionIndex::new(neo_home);
            index
                .list_all_with_metadata(&config.sessions_dir)
                .map_err(|error| anyhow::anyhow!("failed to list all sessions: {error}"))
        }
    }
}

pub fn rename(session_ref: &str, name: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let session = metadata_store(config)
        .rename(&session_id, name.to_owned())
        .with_context(|| format!("failed to rename session {session_ref}"))?;
    Ok(format!(
        "renamed {} {}\n",
        session.id,
        session.name.unwrap_or_default()
    ))
}

pub fn fork(session_ref: &str, name: Option<&str>, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let session = metadata_store(config)
        .fork(&session_id, name.map(str::to_owned))
        .with_context(|| format!("failed to fork session {session_ref}"))?;
    let mut output = format!("forked {session_id} -> {}", session.id);
    if let Some(name) = session.name {
        output.push(' ');
        output.push_str(&name);
    }
    output.push('\n');
    Ok(output)
}

pub fn show(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let path = session_path(session_ref, config)?;
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session {}", path.display()))?;
    Ok(format!("{content}\n"))
}

pub async fn transcript(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let path = session_path(&session_id, config)?;
    ensure_session_can_be_replayed(&session_id, &path)?;
    let context = JsonlSessionReader::replay_context(path)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;
    let mut lines = Vec::new();
    if let Some(summary) = context.compaction_summary() {
        lines.push(format!("compaction: {}", summary.summary));
    }
    if let Some(summary) = metadata_store(config).list().ok().and_then(|sessions| {
        sessions
            .into_iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.summary)
    }) {
        lines.push(format!("branch summary: {summary}"));
    }
    lines.extend(
        context
            .messages()
            .iter()
            .map(format_message)
            .collect::<Vec<_>>(),
    );
    let lines = lines
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");

    if lines.is_empty() {
        Ok("empty session\n".to_owned())
    } else {
        Ok(format!("{lines}\n"))
    }
}

pub(crate) fn ensure_session_can_be_replayed(session_id: &str, path: &Path) -> anyhow::Result<()> {
    let size = fs::metadata(path)
        .with_context(|| format!("failed to stat session {}", path.display()))?
        .len();
    if size > MAX_RESUME_SESSION_BYTES {
        anyhow::bail!(
            "session {session_id} main wire is too large to resume safely ({size} bytes). Run `neo sessions slim {session_id} --write` and retry."
        );
    }
    Ok(())
}

pub async fn compact(
    session_ref: &str,
    keep_recent: usize,
    config: &AppConfig,
) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let result = compact_jsonl_session(
        session_path(&session_id, config)?,
        SessionCompactionOptions {
            keep_recent_messages: keep_recent,
        },
    )
    .await
    .with_context(|| format!("failed to compact session {session_ref}"))?;

    Ok(format!(
        "compacted {session_id}: compacted {}, kept {}\n{}\n",
        result.compacted_message_count, result.kept_message_count, result.summary.summary
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionSlimOptions {
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionSlimResult {
    pub original_bytes: u64,
    pub estimated_bytes: u64,
    pub prior_messages_removed: usize,
    pub prior_message_bytes_removed: u64,
    pub delegate_updates_input: usize,
    pub delegate_updates_kept: usize,
    pub delegate_updates_coalesced: usize,
    pub backup_path: Option<PathBuf>,
}

pub async fn slim(session_ref: &str, write: bool, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let result = slim_jsonl_session_file(
        session_path(&session_id, config)?,
        SessionSlimOptions { write },
    )
    .await
    .with_context(|| format!("failed to slim session {session_ref}"))?;
    let mode = if write { "rewrote" } else { "dry-run" };
    let backup = result
        .backup_path
        .as_ref()
        .map_or_else(String::new, |path| format!("backup={}\n", path.display()));

    Ok(format!(
        "slim {session_id}: {mode}\noriginal_bytes={}\nestimated_bytes={}\nprior_messages_removed={}\nprior_message_bytes_removed={}\ndelegate_updates_input={}\ndelegate_updates_kept={}\ndelegate_updates_coalesced={}\n{backup}",
        result.original_bytes,
        result.estimated_bytes,
        result.prior_messages_removed,
        result.prior_message_bytes_removed,
        result.delegate_updates_input,
        result.delegate_updates_kept,
        result.delegate_updates_coalesced,
    ))
}

pub(crate) async fn slim_jsonl_session_file(
    path: impl AsRef<Path>,
    options: SessionSlimOptions,
) -> anyhow::Result<SessionSlimResult> {
    let path = path.as_ref();
    let original_bytes = fs::metadata(path)
        .with_context(|| format!("failed to stat session {}", path.display()))?
        .len();
    let temp_path = options.write.then(|| slim_temp_path(path));
    let mut writer = match &temp_path {
        Some(temp_path) => Some(
            fs::File::create(temp_path)
                .with_context(|| format!("failed to create {}", temp_path.display()))?,
        ),
        None => None,
    };
    let input = fs::File::open(path)
        .with_context(|| format!("failed to open session {}", path.display()))?;
    let mut slimmer = JsonlSlimmer::new(writer.as_mut());

    for line in std::io::BufReader::new(input).lines() {
        slimmer.push_line(&line?)?;
    }
    slimmer.finish()?;
    let mut result = slimmer.into_result(original_bytes);
    drop(writer);
    if let Some(temp_path) = temp_path {
        if let Err(error) = JsonlSessionReader::read_all(&temp_path).await {
            let _ = fs::remove_file(&temp_path);
            return Err(error).with_context(|| {
                format!("slimmed session {} did not replay", temp_path.display())
            });
        }
        let backup_path = slim_backup_path(path);
        fs::rename(path, &backup_path).with_context(|| {
            format!(
                "failed to move original session {} to {}",
                path.display(),
                backup_path.display()
            )
        })?;
        fs::rename(&temp_path, path).with_context(|| {
            let _ = fs::rename(&backup_path, path);
            format!(
                "failed to install slimmed session {} to {}",
                temp_path.display(),
                path.display()
            )
        })?;
        result.backup_path = Some(backup_path);
    }

    Ok(result)
}

pub async fn export_html(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let session_id = resolve_session_id(session_ref, config)?;
    let messages = JsonlSessionReader::replay_messages(session_path(&session_id, config)?)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;
    render_messages_html(format!("neo session {session_id}"), &messages)
}

pub async fn export_json(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    let artifact = export_json_artifact(session_ref, config).await?;
    let mut json = serde_json::to_string_pretty(&artifact)?;
    json.push('\n');
    Ok(json)
}

pub(crate) async fn export_json_artifact(
    session_ref: &str,
    config: &AppConfig,
) -> anyhow::Result<SessionExportJsonArtifact> {
    let session_id = resolve_session_id(session_ref, config)?;
    let path = session_path(&session_id, config)?;
    anyhow::ensure!(path.exists(), "session {session_ref:?} does not exist");

    let bucket_dir = workspace_sessions_dir(config);
    let record = metadata_store(config)
        .list()
        .with_context(|| format!("failed to read sessions directory {}", bucket_dir.display()))?
        .into_iter()
        .find(|session| session.id == session_id)
        .ok_or_else(|| anyhow::anyhow!("session {session_ref:?} does not exist"))?;
    let messages = JsonlSessionReader::replay_messages(path)
        .await
        .with_context(|| format!("failed to replay session {session_ref}"))?;

    Ok(SessionExportJsonArtifact {
        format: "neo.session.export_json",
        schema_version: 1,
        metadata: SessionExportJsonMetadata {
            id: record.id,
            name: record.name,
            summary: record.summary,
            parent_id: record.parent_id,
            children: record.children,
            message_count: messages.len(),
        },
        messages,
    })
}

fn render_messages_html(title: String, messages: &[AgentMessage]) -> anyhow::Result<String> {
    let export_messages = messages
        .iter()
        .map(|message| ExportMessage::new(message_role(message), message.text()))
        .collect();
    let conversation = ExportConversation::new(title, export_messages);
    neo_agent_core::session::export::export_html(&conversation, &HtmlExportOptions::default())
        .map_err(anyhow::Error::from)
}

pub fn session_path(session_ref: &str, config: &AppConfig) -> anyhow::Result<PathBuf> {
    let session_id = resolve_session_id(session_ref, config)?;
    let bucket_dir = workspace_sessions_dir(config);
    Ok(neo_agent_core::session::main_agent_wire_path(
        &bucket_dir.join(&session_id),
    ))
}

pub fn resolve_session_id(session_ref: &str, config: &AppConfig) -> anyhow::Result<String> {
    if let Some(session_id) = session_id_from_jsonl_path(session_ref, config)? {
        return Ok(session_id);
    }

    validate_session_id(session_ref)
        .map_err(|_| anyhow::anyhow!("invalid session id {session_ref:?}"))?;
    Ok(session_ref.to_owned())
}

fn session_id_from_jsonl_path(
    session_ref: &str,
    config: &AppConfig,
) -> anyhow::Result<Option<String>> {
    let raw = Path::new(session_ref);

    // Determine the effective path to check.
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        env::current_dir()?.join(raw)
    };

    let bucket_dir = workspace_sessions_dir(config);
    if !path_is_inside_session_bucket(&path, &bucket_dir) {
        return Ok(None);
    }

    if path.file_name().and_then(|n| n.to_str()) == Some("wire.jsonl")
        && path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("main")
        && path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            == Some("agents")
        && let Some(session_id) = path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
    {
        validate_session_id(session_id)
            .map_err(|_| anyhow::anyhow!("invalid session id {session_id:?}"))?;
        return Ok(Some(session_id.to_owned()));
    }

    Ok(None)
}

fn path_is_inside_session_bucket(path: &Path, bucket_dir: &Path) -> bool {
    match path.canonicalize() {
        Ok(canonical_path) => bucket_dir.canonicalize().map_or_else(
            |_| path.starts_with(bucket_dir),
            |canonical_bucket| canonical_path.starts_with(canonical_bucket),
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.starts_with(bucket_dir),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingSlimUpdate {
    key: String,
    signature: Value,
    line: String,
}

struct JsonlSlimmer<'a> {
    writer: Option<&'a mut fs::File>,
    pending_update: Option<PendingSlimUpdate>,
    estimated_bytes: u64,
    prior_messages_removed: usize,
    prior_message_bytes_removed: u64,
    delegate_updates_input: usize,
    delegate_updates_kept: usize,
    delegate_updates_coalesced: usize,
}

impl<'a> JsonlSlimmer<'a> {
    fn new(writer: Option<&'a mut fs::File>) -> Self {
        Self {
            writer,
            pending_update: None,
            estimated_bytes: 0,
            prior_messages_removed: 0,
            prior_message_bytes_removed: 0,
            delegate_updates_input: 0,
            delegate_updates_kept: 0,
            delegate_updates_coalesced: 0,
        }
    }

    fn push_line(&mut self, line: &str) -> anyhow::Result<()> {
        if line.trim().is_empty() {
            return Ok(());
        }
        let mut value: Value = serde_json::from_str(line)?;
        let event_kind = event_kind(&value);
        let removed = strip_prior_messages(&mut value);
        self.prior_messages_removed += removed.count;
        self.prior_message_bytes_removed = self
            .prior_message_bytes_removed
            .saturating_add(removed.bytes);

        if matches!(
            event_kind.as_deref(),
            Some("DelegateUpdated" | "DelegateSwarmUpdated")
        ) {
            self.delegate_updates_input += 1;
            let Some(update) = slim_update_from_value(&value)? else {
                self.flush_pending()?;
                self.write_value(&value)?;
                return Ok(());
            };
            self.push_update(update)?;
        } else {
            self.flush_pending()?;
            self.write_value(&value)?;
        }
        Ok(())
    }

    fn push_update(&mut self, update: PendingSlimUpdate) -> anyhow::Result<()> {
        if let Some(pending) = &mut self.pending_update
            && pending.key == update.key
        {
            if pending.signature == update.signature {
                pending.line = update.line;
                self.delegate_updates_coalesced += 1;
            } else {
                self.flush_pending()?;
                self.pending_update = Some(update);
            }
            return Ok(());
        }
        self.flush_pending()?;
        self.pending_update = Some(update);
        Ok(())
    }

    fn flush_pending(&mut self) -> anyhow::Result<()> {
        let Some(pending) = self.pending_update.take() else {
            return Ok(());
        };
        self.delegate_updates_kept += 1;
        self.write_line(&pending.line)
    }

    fn write_value(&mut self, value: &Value) -> anyhow::Result<()> {
        let line = serde_json::to_string(value)?;
        self.write_line(&line)
    }

    fn write_line(&mut self, line: &str) -> anyhow::Result<()> {
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_add(u64::try_from(line.len() + 1).unwrap_or(u64::MAX));
        if let Some(writer) = &mut self.writer {
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
        }
        Ok(())
    }

    fn finish(&mut self) -> anyhow::Result<()> {
        self.flush_pending()?;
        if let Some(writer) = &mut self.writer {
            writer.flush()?;
        }
        Ok(())
    }

    fn into_result(self, original_bytes: u64) -> SessionSlimResult {
        SessionSlimResult {
            original_bytes,
            estimated_bytes: self.estimated_bytes,
            prior_messages_removed: self.prior_messages_removed,
            prior_message_bytes_removed: self.prior_message_bytes_removed,
            delegate_updates_input: self.delegate_updates_input,
            delegate_updates_kept: self.delegate_updates_kept,
            delegate_updates_coalesced: self.delegate_updates_coalesced,
            backup_path: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RemovedPriorMessages {
    count: usize,
    bytes: u64,
}

fn strip_prior_messages(value: &mut Value) -> RemovedPriorMessages {
    match value {
        Value::Object(map) => {
            let mut count = 0;
            let mut bytes = 0_u64;
            if let Some(removed) = map.remove("prior_messages") {
                count += 1;
                bytes = bytes.saturating_add(
                    serde_json::to_vec(&removed)
                        .map_or(0, |value| u64::try_from(value.len()).unwrap_or(u64::MAX)),
                );
            }
            for child in map.values_mut() {
                let removed = strip_prior_messages(child);
                count += removed.count;
                bytes = bytes.saturating_add(removed.bytes);
            }
            RemovedPriorMessages { count, bytes }
        }
        Value::Array(items) => items.iter_mut().fold(
            RemovedPriorMessages { count: 0, bytes: 0 },
            |mut acc, item| {
                let removed = strip_prior_messages(item);
                acc.count += removed.count;
                acc.bytes = acc.bytes.saturating_add(removed.bytes);
                acc
            },
        ),
        _ => RemovedPriorMessages { count: 0, bytes: 0 },
    }
}

fn slim_update_from_value(value: &Value) -> anyhow::Result<Option<PendingSlimUpdate>> {
    let Some(kind) = event_kind(value) else {
        return Ok(None);
    };
    let key = match kind.as_str() {
        "DelegateUpdated" => value
            .pointer("/DelegateUpdated/agent/id")
            .and_then(Value::as_str)
            .map(|id| format!("delegate:{id}")),
        "DelegateSwarmUpdated" => value
            .pointer("/DelegateSwarmUpdated/swarm/swarm_id")
            .and_then(Value::as_str)
            .map(|id| format!("swarm:{id}")),
        _ => None,
    };
    let Some(key) = key else {
        return Ok(None);
    };
    Ok(Some(PendingSlimUpdate {
        key,
        signature: update_signature(value, &kind),
        line: serde_json::to_string(value)?,
    }))
}

fn update_signature(value: &Value, kind: &str) -> Value {
    match kind {
        "DelegateUpdated" => agent_signature(value.pointer("/DelegateUpdated/agent")),
        "DelegateSwarmUpdated" => {
            let children = value
                .pointer("/DelegateSwarmUpdated/swarm/children")
                .and_then(Value::as_array)
                .map(|children| {
                    children
                        .iter()
                        .map(|child| agent_signature(child.get("agent")))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            serde_json::json!({
                "state": value.pointer("/DelegateSwarmUpdated/swarm/state").cloned(),
                "aggregate": value.pointer("/DelegateSwarmUpdated/swarm/aggregate").cloned(),
                "children": children,
            })
        }
        _ => Value::Null,
    }
}

fn agent_signature(agent: Option<&Value>) -> Value {
    let last_tool = agent
        .and_then(|agent| agent.get("activity"))
        .and_then(Value::as_array)
        .and_then(|items| items.iter().rev().find_map(tool_activity_signature))
        .unwrap_or(Value::Null);
    serde_json::json!({
        "state": agent.and_then(|agent| agent.get("state")).cloned(),
        "tool_count": agent.and_then(|agent| agent.get("tool_count")).cloned(),
        "last_tool": last_tool,
    })
}

fn tool_activity_signature(activity: &Value) -> Option<Value> {
    let tool = activity.get("kind").and_then(|kind| kind.get("Tool"))?;
    Some(serde_json::json!({
        "id": tool.get("id"),
        "name": tool.get("name"),
        "phase": tool.get("phase"),
    }))
}

fn event_kind(value: &Value) -> Option<String> {
    value
        .as_object()?
        .keys()
        .find(|key| *key != "kind")
        .cloned()
}

fn slim_temp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.slim.tmp.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wire.jsonl"),
        current_unix_timestamp()
    ))
}

fn slim_backup_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(
        "{}.bak.{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wire.jsonl"),
        current_unix_timestamp()
    ))
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionExportJsonArtifact {
    pub format: &'static str,
    pub schema_version: u32,
    pub metadata: SessionExportJsonMetadata,
    pub messages: Vec<AgentMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SessionExportJsonMetadata {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub children: Vec<String>,
    pub message_count: usize,
}

fn metadata_store(config: &AppConfig) -> SessionMetadataStore {
    let bucket_dir = workspace_sessions_dir(config);
    SessionMetadataStore::new(&bucket_dir)
}

fn format_message(message: &AgentMessage) -> String {
    let role = message_role(message);

    format!("{role}: {}", message.text())
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

#[cfg(test)]
mod tests {
    use super::*;
    use neo_agent_core::{
        AgentEvent, AgentMessage,
        multi_agent::{
            AgentLifecycleState, AgentPathKind, AgentRole, AgentRunMode, MultiAgentRuntime,
            SwarmAggregate, SwarmChildSnapshot, SwarmSnapshot,
        },
        session::JsonlSessionReader,
    };
    use serde_json::{Value, json};

    #[tokio::test]
    async fn slim_jsonl_session_strips_prior_messages_and_coalesces_updates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("wire.jsonl");
        let runtime = MultiAgentRuntime::new();
        let swarm_id = runtime.new_swarm_id();
        let mut child = runtime.start_delegate(
            "write docs",
            Some("docs"),
            AgentRole::Coder,
            AgentRunMode::Foreground,
            neo_agent_core::multi_agent::DelegateContext::None,
            AgentPathKind::SwarmChild(&swarm_id),
        );
        child.latest_text = Some("first chunk".to_owned());
        child.prior_messages = vec![AgentMessage::user_text("large child prompt")];

        let first_swarm = SwarmSnapshot {
            swarm_id: swarm_id.clone(),
            description: "docs".to_owned(),
            role: AgentRole::Coder,
            mode: AgentRunMode::Foreground,
            state: AgentLifecycleState::Running,
            max_concurrency: 1,
            aggregate: SwarmAggregate::from_states([AgentLifecycleState::Running]),
            children: vec![SwarmChildSnapshot {
                item_index: 0,
                item: "docs".to_owned(),
                agent: child.clone(),
            }],
        };
        child.latest_text = Some("second chunk".to_owned());
        child.prior_messages = vec![AgentMessage::user_text("large child prompt 2")];
        let second_swarm = SwarmSnapshot {
            children: vec![SwarmChildSnapshot {
                item_index: 0,
                item: "docs".to_owned(),
                agent: child.clone(),
            }],
            ..first_swarm.clone()
        };
        let mut finished_child = child;
        finished_child.state = AgentLifecycleState::Completed;
        finished_child.prior_messages = vec![AgentMessage::user_text("large child prompt 3")];
        let finished_swarm = SwarmSnapshot {
            state: AgentLifecycleState::Completed,
            aggregate: SwarmAggregate::from_states([AgentLifecycleState::Completed]),
            children: vec![SwarmChildSnapshot {
                item_index: 0,
                item: "docs".to_owned(),
                agent: finished_child,
            }],
            ..second_swarm.clone()
        };

        let bloated_lines = [
            with_legacy_prior_messages(AgentEvent::DelegateSwarmUpdated {
                turn: 1,
                swarm: first_swarm,
            }),
            with_legacy_prior_messages(AgentEvent::DelegateSwarmUpdated {
                turn: 1,
                swarm: second_swarm,
            }),
            with_legacy_prior_messages(AgentEvent::DelegateSwarmFinished {
                turn: 1,
                swarm: finished_swarm,
            }),
        ]
        .into_iter()
        .map(|value| serde_json::to_string(&value).expect("serialize line"))
        .collect::<Vec<_>>()
        .join("\n");
        fs::write(&path, format!("{bloated_lines}\n")).expect("write bloated session");

        let dry_run = slim_jsonl_session_file(&path, SessionSlimOptions { write: false })
            .await
            .expect("dry-run slim");
        assert_eq!(dry_run.prior_messages_removed, 3);
        assert_eq!(dry_run.delegate_updates_input, 2);
        assert_eq!(dry_run.delegate_updates_kept, 1);
        assert_eq!(dry_run.backup_path, None);
        assert!(dry_run.estimated_bytes < dry_run.original_bytes);
        assert!(
            fs::read_to_string(&path)
                .expect("read original")
                .contains("prior_messages"),
            "dry-run must not rewrite the session"
        );

        let written = slim_jsonl_session_file(&path, SessionSlimOptions { write: true })
            .await
            .expect("write slim");
        assert_eq!(written.prior_messages_removed, 3);
        assert!(
            written
                .backup_path
                .as_ref()
                .is_some_and(|path| path.is_file())
        );

        let slimmed = fs::read_to_string(&path).expect("read slimmed");
        assert!(!slimmed.contains("prior_messages"), "{slimmed}");
        assert_eq!(slimmed.matches("DelegateSwarmUpdated").count(), 1);
        assert_eq!(slimmed.matches("DelegateSwarmFinished").count(), 1);
        let replayed = JsonlSessionReader::read_all(&path)
            .await
            .expect("slimmed session replays");
        assert!(
            replayed
                .iter()
                .any(|event| matches!(event, AgentEvent::DelegateSwarmFinished { .. })),
            "{replayed:#?}"
        );
    }

    fn with_legacy_prior_messages(event: AgentEvent) -> Value {
        let mut value = serde_json::to_value(event).expect("event value");
        if let Some(agent) = value.pointer_mut("/DelegateSwarmUpdated/swarm/children/0/agent") {
            agent["prior_messages"] = json!([{"User":{"content":[{"Text":"large"}]}}]);
        }
        if let Some(agent) = value.pointer_mut("/DelegateSwarmFinished/swarm/children/0/agent") {
            agent["prior_messages"] = json!([{"User":{"content":[{"Text":"large"}]}}]);
        }
        value
    }
}
