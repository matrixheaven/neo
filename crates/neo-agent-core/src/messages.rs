use std::collections::HashSet;
use std::sync::Arc;

use neo_ai::{ChatMessage, ContentPart, ImageData, ToolCall};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::StopReason;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ShellCommandOutcome {
    Completed,
    Cancelled,
    TimedOut,
    Backgrounded { task_id: Arc<str> },
}

impl ShellCommandOutcome {
    #[must_use]
    pub const fn as_model_status(&self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Backgrounded { .. } => "backgrounded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Content {
    Text {
        text: Arc<str>,
    },
    Thinking {
        text: Arc<str>,
        signature: Option<Arc<str>>,
        redacted: bool,
    },
    Image {
        mime_type: Arc<str>,
        data: ImageRef,
    },
}

impl Content {
    #[must_use]
    pub fn text(text: impl Into<Arc<str>>) -> Self {
        Self::Text { text: text.into() }
    }

    #[must_use]
    pub fn thinking(
        text: impl Into<Arc<str>>,
        signature: Option<Arc<str>>,
        redacted: bool,
    ) -> Self {
        Self::Thinking {
            text: text.into(),
            signature,
            redacted,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Thinking { .. } | Self::Image { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ImageRef {
    Base64(Arc<str>),
    Url(Arc<str>),
    /// SHA-256 reference to a blob file stored in the session directory.
    Blob(Arc<str>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolCall {
    pub id: Arc<str>,
    pub name: Arc<str>,
    pub raw_arguments: Arc<str>,
}

impl From<ToolCall> for AgentToolCall {
    fn from(value: ToolCall) -> Self {
        Self {
            id: value.id.into(),
            name: value.name.into(),
            raw_arguments: value.raw_arguments.into(),
        }
    }
}

impl From<AgentToolCall> for ToolCall {
    fn from(value: AgentToolCall) -> Self {
        Self {
            id: value.id.to_string(),
            name: value.name.to_string(),
            raw_arguments: value.raw_arguments.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AgentMessage {
    System {
        content: Vec<Content>,
    },
    User {
        content: Vec<Content>,
    },
    Assistant {
        content: Vec<Content>,
        tool_calls: Vec<AgentToolCall>,
        stop_reason: StopReason,
    },
    ToolResult {
        tool_call_id: Arc<str>,
        tool_name: Arc<str>,
        content: Vec<Content>,
        is_error: bool,
    },
    ShellCommand {
        command: Arc<str>,
        stdout: Arc<str>,
        stderr: Arc<str>,
        exit_code: Option<i32>,
        outcome: ShellCommandOutcome,
        #[serde(default)]
        truncated: bool,
    },
}

impl AgentMessage {
    #[must_use]
    pub fn system_text(text: impl Into<Arc<str>>) -> Self {
        Self::System {
            content: vec![Content::text(text)],
        }
    }

    #[must_use]
    pub fn user_text(text: impl Into<Arc<str>>) -> Self {
        Self::User {
            content: vec![Content::text(text)],
        }
    }

    #[must_use]
    pub fn system_reminder(text: impl AsRef<str>) -> Self {
        Self::user_text(format!(
            "<system-reminder>\n{}\n</system-reminder>",
            text.as_ref().trim()
        ))
    }

    #[must_use]
    pub fn assistant(
        content: impl Into<Vec<Content>>,
        tool_calls: impl Into<Vec<AgentToolCall>>,
        stop_reason: StopReason,
    ) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: tool_calls.into(),
            stop_reason,
        }
    }

    #[must_use]
    pub fn tool_result(
        tool_call_id: impl Into<Arc<str>>,
        tool_name: impl Into<Arc<str>>,
        content: impl Into<Vec<Content>>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            content: content.into(),
            is_error,
        }
    }

    #[must_use]
    pub fn shell_command(
        command: impl Into<Arc<str>>,
        stdout: impl Into<Arc<str>>,
        stderr: impl Into<Arc<str>>,
        exit_code: Option<i32>,
        outcome: ShellCommandOutcome,
        truncated: bool,
    ) -> Self {
        Self::ShellCommand {
            command: command.into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code,
            outcome,
            truncated,
        }
    }

    /// Extract all `Text` content parts from this message and join them.
    ///
    /// Returns an empty string for variants without text content.
    #[must_use]
    pub fn text(&self) -> String {
        let content = match self {
            Self::System { content }
            | Self::User { content }
            | Self::Assistant { content, .. }
            | Self::ToolResult { content, .. } => content,
            Self::ShellCommand {
                command,
                stdout,
                stderr,
                exit_code,
                outcome,
                truncated,
            } => {
                return shell_command_model_text(
                    command, stdout, stderr, *exit_code, outcome, *truncated,
                );
            }
        };
        content
            .iter()
            .filter_map(Content::as_text)
            .collect::<Vec<_>>()
            .join("")
    }

    #[must_use]
    pub fn to_chat_message(&self) -> ChatMessage {
        match self {
            Self::System { content } => ChatMessage::System {
                content: content.iter().map(to_content_part).collect(),
            },
            Self::User { content } => ChatMessage::User {
                content: content.iter().map(to_content_part).collect(),
            },
            Self::Assistant {
                content,
                tool_calls,
                stop_reason: _,
            } => ChatMessage::Assistant {
                content: content.iter().map(to_content_part).collect(),
                tool_calls: tool_calls
                    .iter()
                    .map(provider_safe_tool_call)
                    .map(Into::into)
                    .collect(),
            },
            Self::ToolResult {
                tool_call_id,
                tool_name: _,
                content,
                is_error,
            } => ChatMessage::ToolResult {
                tool_call_id: tool_call_id.to_string(),
                content: content.iter().map(to_content_part).collect(),
                is_error: *is_error,
            },
            Self::ShellCommand {
                command,
                stdout,
                stderr,
                exit_code,
                outcome,
                truncated,
            } => ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: shell_command_model_text(
                        command, stdout, stderr, *exit_code, outcome, *truncated,
                    ),
                }],
            },
        }
    }
}

fn provider_safe_tool_call(tool_call: &AgentToolCall) -> AgentToolCall {
    AgentToolCall {
        id: tool_call.id.clone(),
        name: tool_call.name.clone(),
        raw_arguments: provider_safe_tool_arguments(&tool_call.raw_arguments).into(),
    }
}

fn provider_safe_tool_arguments(raw_arguments: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(raw_arguments).is_ok() {
        return raw_arguments.to_owned();
    }
    let Some(object) = complete_top_level_argument_pairs(raw_arguments) else {
        return "{}".to_owned();
    };
    serde_json::to_string(&serde_json::Value::Object(object)).unwrap_or_else(|_| "{}".to_owned())
}

fn complete_top_level_argument_pairs(
    raw: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = raw.trim_start();
    if !raw.starts_with('{') {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut object = serde_json::Map::new();
    let mut index = 1;
    loop {
        skip_ws_and_commas(bytes, &mut index);
        if index >= bytes.len() || bytes[index] == b'}' {
            return Some(object);
        }
        let (key, after_key) = parse_json_string(raw, index)?;
        index = after_key;
        skip_ws(bytes, &mut index);
        if bytes.get(index).copied()? != b':' {
            return Some(object);
        }
        index += 1;
        skip_ws(bytes, &mut index);
        let value_start = index;
        let Some(value_end) = complete_json_value_end(raw, value_start) else {
            return Some(object);
        };
        let value = serde_json::from_str::<serde_json::Value>(&raw[value_start..value_end]).ok()?;
        object.insert(key, value);
        index = value_end;
    }
}

fn skip_ws_and_commas(bytes: &[u8], index: &mut usize) {
    while let Some(byte) = bytes.get(*index) {
        if byte.is_ascii_whitespace() || *byte == b',' {
            *index += 1;
        } else {
            break;
        }
    }
}

fn skip_ws(bytes: &[u8], index: &mut usize) {
    while bytes.get(*index).is_some_and(u8::is_ascii_whitespace) {
        *index += 1;
    }
}

fn parse_json_string(raw: &str, start: usize) -> Option<(String, usize)> {
    if raw.as_bytes().get(start).copied()? != b'"' {
        return None;
    }
    let mut escaped = false;
    for (offset, ch) in raw[start + 1..].char_indices() {
        let pos = start + 1 + offset;
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => {
                let end = pos + ch.len_utf8();
                let parsed = serde_json::from_str::<String>(&raw[start..end]).ok()?;
                return Some((parsed, end));
            }
            _ => {}
        }
    }
    None
}

fn complete_json_value_end(raw: &str, start: usize) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0_i32;
    let mut saw_value = false;
    let mut top_level_string = false;
    let mut top_level_string_complete = false;
    let mut top_level_composite = false;
    let mut top_level_composite_complete = false;
    for (offset, ch) in raw[start..].char_indices() {
        let pos = start + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
                if top_level_string && depth == 0 {
                    top_level_string_complete = true;
                }
            }
            continue;
        }
        match ch {
            '"' => {
                if !saw_value && depth == 0 {
                    top_level_string = true;
                }
                in_string = true;
                saw_value = true;
            }
            '{' | '[' => {
                if !saw_value && depth == 0 {
                    top_level_composite = true;
                }
                depth += 1;
                saw_value = true;
            }
            '}' | ']' => {
                if depth == 0 {
                    return saw_value.then_some(pos);
                }
                depth -= 1;
                if top_level_composite && depth == 0 {
                    top_level_composite_complete = true;
                }
            }
            ',' if depth == 0 => return saw_value.then_some(pos),
            c if c.is_ascii_whitespace() => {}
            _ => saw_value = true,
        }
    }
    if !in_string
        && depth == 0
        && saw_value
        && (top_level_string_complete || top_level_composite_complete)
    {
        return Some(raw.len());
    }
    None
}

/// Validate and repair tool-call/tool-result exchanges.
///
/// Every `Assistant` message with non-empty `tool_calls` must be immediately
/// followed by `ToolResult` messages whose `tool_call_id`s cover every tool
/// call. Any incomplete exchange is dropped, as are orphaned `ToolResult`
/// messages that do not belong to the immediately preceding assistant turn.
/// This prevents provider 400 errors such as "an assistant message with
/// `tool_calls` must be followed by tool messages responding to each
/// `tool_call_id`".
///
/// Returns [`Cow::Borrowed`] when no repair is needed (the common case),
/// avoiding a full-Vec clone.
#[must_use]
pub fn sanitize_tool_exchange_messages(
    messages: &[AgentMessage],
) -> std::borrow::Cow<'_, [AgentMessage]> {
    // First pass: detect whether any repair is needed.
    let mut needs_repair = false;
    let mut i = 0;
    while i < messages.len() {
        match &messages[i] {
            AgentMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                let ids: HashSet<&str> = tool_calls
                    .iter()
                    .map(|tool_call| tool_call.id.as_ref())
                    .collect();
                let mut j = i + 1;
                let mut seen = HashSet::new();
                while j < messages.len() {
                    if let AgentMessage::ToolResult { tool_call_id, .. } = &messages[j] {
                        if ids.contains(tool_call_id.as_ref()) {
                            seen.insert(tool_call_id.as_ref());
                            j += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                if seen.len() != ids.len() {
                    needs_repair = true;
                    break;
                }
                i = j;
            }
            AgentMessage::ToolResult { .. } => {
                // Orphaned tool result (no preceding assistant with matching ids).
                needs_repair = true;
                break;
            }
            _ => {
                i += 1;
            }
        }
    }

    if !needs_repair {
        return std::borrow::Cow::Borrowed(messages);
    }

    // Second pass: build the repaired Vec.
    let mut out = Vec::with_capacity(messages.len());
    i = 0;
    while i < messages.len() {
        match &messages[i] {
            AgentMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty() => {
                let ids: HashSet<&str> = tool_calls
                    .iter()
                    .map(|tool_call| tool_call.id.as_ref())
                    .collect();
                let mut j = i + 1;
                let mut seen = HashSet::new();
                while j < messages.len() {
                    if let AgentMessage::ToolResult { tool_call_id, .. } = &messages[j] {
                        if ids.contains(tool_call_id.as_ref()) {
                            seen.insert(tool_call_id.as_ref());
                            j += 1;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                out.push(messages[i].clone());
                out.extend_from_slice(&messages[i + 1..j]);
                for tool_call in tool_calls {
                    if !seen.contains(tool_call.id.as_ref()) {
                        out.push(missing_tool_result(tool_call));
                    }
                }
                i = j;
            }
            AgentMessage::ToolResult { .. } => {
                out.push(orphan_tool_result_reminder(&messages[i]));
                i += 1;
            }
            _ => {
                out.push(messages[i].clone());
                i += 1;
            }
        }
    }
    std::borrow::Cow::Owned(out)
}

fn missing_tool_result(tool_call: &AgentToolCall) -> AgentMessage {
    AgentMessage::tool_result(
        tool_call.id.clone(),
        tool_call.name.clone(),
        vec![Content::text(
            "[Missing tool result repaired by runtime: the original result was not present in session history]",
        )],
        true,
    )
}

fn orphan_tool_result_reminder(message: &AgentMessage) -> AgentMessage {
    let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        ..
    } = message
    else {
        return AgentMessage::system_reminder("orphaned tool result omitted");
    };
    AgentMessage::system_reminder(format!(
        "orphaned tool result omitted: tool_call_id={tool_call_id}, tool_name={tool_name}"
    ))
}

fn shell_command_model_text(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
    outcome: &ShellCommandOutcome,
    truncated: bool,
) -> String {
    let exit_code = exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string());
    format!(
        "<bash-input>\n{}\n</bash-input>\n<bash-stdout>\n{}\n</bash-stdout>\n<bash-stderr>\n{}\n</bash-stderr>\n<bash-status exit_code=\"{}\" outcome=\"{}\" truncated=\"{}\" />",
        escape_xml_text(command),
        escape_xml_text(stdout),
        escape_xml_text(stderr),
        escape_xml_attr(&exit_code),
        outcome.as_model_status(),
        truncated,
    )
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value).replace('"', "&quot;")
}

fn to_content_part(content: &Content) -> ContentPart {
    match content {
        Content::Text { text } => ContentPart::Text {
            text: text.to_string(),
        },
        Content::Thinking {
            text,
            signature,
            redacted,
        } => ContentPart::Thinking {
            text: text.to_string(),
            signature: signature.as_ref().map(|s| s.to_string()),
            redacted: *redacted,
        },
        Content::Image { mime_type, data } => match data {
            ImageRef::Base64(value) => ContentPart::Image {
                mime_type: mime_type.to_string(),
                data: ImageData::Base64(value.to_string()),
            },
            ImageRef::Url(value) => ContentPart::Image {
                mime_type: mime_type.to_string(),
                data: ImageData::Url(value.to_string()),
            },
            // Blob references must be resolved to base64 before conversion.
            // If an unresolved blob reaches here, emit a text placeholder
            // instead of invalid empty base64 image data.
            ImageRef::Blob(sha) => ContentPart::Text {
                text: format!("[unavailable image: blob {sha}]"),
            },
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_chat_message_repairs_partial_tool_arguments_before_provider_request() {
        let message = AgentMessage::assistant(
            vec![],
            vec![AgentToolCall {
                id: "tc1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"ls -la","cwd":"#.into(),
            }],
            StopReason::ToolUse,
        );

        let ChatMessage::Assistant { tool_calls, .. } = message.to_chat_message() else {
            panic!("expected assistant chat message");
        };

        assert_eq!(tool_calls[0].raw_arguments, r#"{"command":"ls -la"}"#);
    }

    #[test]
    fn to_chat_message_replaces_unrecoverable_tool_arguments_before_provider_request() {
        let message = AgentMessage::assistant(
            vec![],
            vec![AgentToolCall {
                id: "tc1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"ls"#.into(),
            }],
            StopReason::ToolUse,
        );

        let ChatMessage::Assistant { tool_calls, .. } = message.to_chat_message() else {
            panic!("expected assistant chat message");
        };

        assert_eq!(tool_calls[0].raw_arguments, "{}");
    }

    #[test]
    fn sanitize_keeps_complete_tool_exchange() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".into(),
                    name: "Bash".into(),
                    raw_arguments: "null".into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("thanks"),
        ];
        let out = sanitize_tool_exchange_messages(&messages);
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[1], AgentMessage::Assistant { .. }));
    }

    #[test]
    fn sanitize_fills_missing_tool_result() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".into(),
                    name: "Bash".into(),
                    raw_arguments: "null".into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::user_text("never mind"),
        ];
        let out = sanitize_tool_exchange_messages(&messages);
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], AgentMessage::User { .. }));
        assert!(matches!(&out[1], AgentMessage::Assistant { .. }));
        assert!(matches!(&out[2], AgentMessage::ToolResult { .. }));
        assert!(matches!(&out[3], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_fills_incomplete_exchange_even_with_partial_result() {
        let messages = vec![
            AgentMessage::assistant(
                vec![],
                vec![
                    AgentToolCall {
                        id: "tc1".into(),
                        name: "Bash".into(),
                        raw_arguments: "null".into(),
                    },
                    AgentToolCall {
                        id: "tc2".into(),
                        name: "Bash".into(),
                        raw_arguments: "null".into(),
                    },
                ],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("stop"),
        ];
        let out = sanitize_tool_exchange_messages(&messages);
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], AgentMessage::Assistant { .. }));
        assert!(matches!(&out[1], AgentMessage::ToolResult { .. }));
        assert!(matches!(&out[2], AgentMessage::ToolResult { .. }));
        assert!(matches!(&out[3], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_converts_orphan_tool_result_to_reminder() {
        let messages = vec![
            AgentMessage::user_text("hi"),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::user_text("bye"),
        ];
        let out = sanitize_tool_exchange_messages(&messages);
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], AgentMessage::User { .. }));
        assert!(matches!(&out[1], AgentMessage::User { .. }));
        assert!(out[1].text().contains("orphaned tool result"));
        assert!(matches!(&out[2], AgentMessage::User { .. }));
    }

    #[test]
    fn sanitize_drops_unknown_tool_result_id_in_exchange() {
        let messages = vec![
            AgentMessage::assistant(
                vec![],
                vec![AgentToolCall {
                    id: "tc1".into(),
                    name: "Bash".into(),
                    raw_arguments: "null".into(),
                }],
                StopReason::ToolUse,
            ),
            AgentMessage::tool_result("tc1", "Bash", vec![Content::text("ok")], false),
            AgentMessage::tool_result("tc2", "Bash", vec![Content::text("orphan")], false),
            AgentMessage::user_text("next"),
        ];
        let out = sanitize_tool_exchange_messages(&messages);
        assert_eq!(out.len(), 4);
        assert!(matches!(&out[0], AgentMessage::Assistant { .. }));
        assert!(matches!(&out[1], AgentMessage::ToolResult { .. }));
        assert!(matches!(&out[2], AgentMessage::User { .. }));
        assert!(out[2].text().contains("orphaned tool result"));
        assert!(matches!(&out[3], AgentMessage::User { .. }));
    }
}
