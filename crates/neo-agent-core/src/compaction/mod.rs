//! Context compaction for the agent runtime.
//!
//! Compaction replaces older conversation messages with an LLM-generated
//! structured summary so the agent can keep working across long sessions
//! without overflowing the model's context window.
//!
//! Architecture (ported from kimi-code's `agent/compaction/`):
//! - [`can_split_after`] / [`compute_compact_count`] — safe boundary detection
//!   that never cuts between an assistant tool-call and its tool results.
//! - [`render_messages_to_text`] — renders messages into a structured text
//!   block fed to the summariser.
//! - [`generate_compaction_summary`] — drives an LLM call to produce the
//!   structured summary.
//! - [`CompactionStrategy`] — trigger ratio and retention heuristics.

pub mod summary;

use std::sync::Arc;

use futures::StreamExt;
use neo_ai::{AiStreamEvent, ChatMessage, ChatRequest, ModelClient, RequestOptions};
use tokio_util::sync::CancellationToken;

use crate::runtime::estimate_message_tokens;
use crate::{AgentConfig, AgentMessage, Content};

pub use crate::events::CompactionSource;

/// LLM-summarisation instruction template.  See [`COMPACTION_INSTRUCTION`].
const COMPACTION_INSTRUCTION: &str = include_str!("compaction_instruction.md");

/// Error returned when LLM-driven compaction fails.  Neo uses a hard-fail
/// policy: compaction errors are surfaced to the user instead of degrading to
/// an algorithmic counter summary.
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction LLM call failed: {0}")]
    Llm(String),
    #[error("compaction produced an empty summary")]
    Empty,
    #[error("compaction cancelled")]
    Cancelled,
    #[error("no safe compaction boundary found in the current history")]
    NoBoundary,
}

/// Heuristics for when and how much to compact.
#[derive(Debug, Clone)]
pub struct CompactionStrategy {
    /// Compact once estimated tokens reach this fraction of `max_context_tokens`.
    pub trigger_ratio: f64,
    /// Maximum number of recent messages to preserve after auto compaction.
    pub max_recent_messages: usize,
    /// Maximum fraction of `max_context_tokens` that recent messages may occupy.
    pub max_recent_size_ratio: f64,
    /// Reserved headroom in tokens.  Forces compaction when
    /// `used + reserved >= max_context_tokens`.
    pub reserved_context_tokens: usize,
}

impl Default for CompactionStrategy {
    fn default() -> Self {
        Self {
            trigger_ratio: 0.85,
            max_recent_messages: 4,
            max_recent_size_ratio: 0.2,
            reserved_context_tokens: 50_000,
        }
    }
}

impl CompactionStrategy {
    /// Whether the current token usage warrants compaction.
    #[must_use]
    pub fn should_compact(&self, used_tokens: usize, max_tokens: usize) -> bool {
        if max_tokens == 0 {
            return false;
        }
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let threshold = (f64::from(u32::try_from(max_tokens).unwrap_or(u32::MAX))
            * self.trigger_ratio) as usize;
        used_tokens >= threshold || self.should_use_reserved_context(used_tokens, max_tokens)
    }

    /// Whether compaction must block the turn (synchronous).  Currently mirrors
    /// [`should_compact`](Self::should_compact) because neo runs compaction
    /// inline before the next model call.
    #[must_use]
    pub fn should_block(&self, used_tokens: usize, max_tokens: usize) -> bool {
        self.should_compact(used_tokens, max_tokens)
    }

    fn should_use_reserved_context(&self, used_tokens: usize, max_tokens: usize) -> bool {
        self.reserved_context_tokens > 0
            && self.reserved_context_tokens < max_tokens
            && used_tokens + self.reserved_context_tokens >= max_tokens
    }
}

/// Decide whether a compaction split is safe immediately *after* `messages[index]`.
///
/// A split is **unsafe** when:
/// - `messages[index]` is a user message (would cut the user's input), or
/// - `messages[index]` is an assistant message with pending tool calls (would
///   orphan the calls from their results), or
/// - the next message is a tool result (the suffix would start with an orphaned
///   result whose owning assistant is in the compacted prefix), or
/// - the compacted prefix ends with an unresolved tool exchange (a tool result
///   whose owning assistant has more calls than the suffix contains results).
///
/// This is a direct port of kimi-code's `canSplitAfter`.
#[must_use]
pub fn can_split_after(messages: &[AgentMessage], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    if matches!(message, AgentMessage::User { .. })
        && !message.is_injection_variant("instruction_epoch")
    {
        return false;
    }
    if let AgentMessage::Assistant { tool_calls, .. } = message
        && !tool_calls.is_empty()
    {
        return false;
    }
    if matches!(
        messages.get(index + 1),
        Some(AgentMessage::ToolResult { .. })
    ) {
        return false;
    }
    if prefix_ends_with_open_tool_exchange(messages, index) {
        return false;
    }
    if suffix_starts_with_unresolved_tool_calls(messages, index) {
        return false;
    }
    true
}

/// Whether the retained suffix `messages[index+1..]` starts with an assistant
/// message whose tool calls are not all followed by matching tool results.
/// Splitting before such an assistant would leave an invalid assistant-with-
/// tool-calls message in the context without the required tool results.
fn suffix_starts_with_unresolved_tool_calls(messages: &[AgentMessage], index: usize) -> bool {
    let Some(AgentMessage::Assistant { tool_calls, .. }) = messages.get(index + 1) else {
        return false;
    };
    if tool_calls.is_empty() {
        return false;
    }
    let needed = tool_calls.len();
    let mut found = 0usize;
    for message in messages.iter().skip(index + 2) {
        if matches!(message, AgentMessage::ToolResult { .. }) {
            found += 1;
            if found >= needed {
                return false;
            }
        } else {
            break;
        }
    }
    true
}

/// Whether the compacted prefix `messages[0..=index]` ends with a tool result
/// whose owning assistant emitted more tool calls than the prefix contains
/// results — i.e. the exchange is unresolved and must be kept in the suffix.
fn prefix_ends_with_open_tool_exchange(messages: &[AgentMessage], index: usize) -> bool {
    if !matches!(messages.get(index), Some(AgentMessage::ToolResult { .. })) {
        return false;
    }
    let mut tool_result_count = 0usize;
    for message in messages[..=index].iter().rev() {
        if let AgentMessage::ToolResult { .. } = message {
            tool_result_count += 1;
            continue;
        }
        if let AgentMessage::Assistant { tool_calls, .. } = message {
            return tool_calls.len() > tool_result_count;
        }
        return false;
    }
    false
}

/// Compute how many leading messages to compact (`N`), keeping
/// `messages[N..]` as the retained suffix.
///
/// - `Manual` source: walk backward from the end to the first safe split.
/// - `Auto` source: respect `max_recent_messages`, `max_recent_size_ratio`,
///   and `max_context_tokens` while keeping at least one recent message.
#[must_use]
pub fn compute_compact_count(
    messages: &[AgentMessage],
    source: CompactionSource,
    strategy: &CompactionStrategy,
    max_context_tokens: usize,
) -> usize {
    if messages.len() < 2 {
        return 0;
    }

    match source {
        CompactionSource::Manual => {
            for index in (0..messages.len() - 1).rev() {
                if can_split_after(messages, index) {
                    return fit_compact_count_to_window(messages, index + 1, max_context_tokens);
                }
            }
            0
        }
        CompactionSource::Auto => {
            let mut recent_messages = 1usize;
            let mut recent_size = estimate_message_tokens(&messages[messages.len() - 1]);
            let mut best_n: Option<usize> = None;

            while recent_messages < messages.len() {
                let split_index = messages.len() - recent_messages - 1;
                if can_split_after(messages, split_index) {
                    best_n = Some(split_index + 1);
                }
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                let reaches_max = recent_messages >= strategy.max_recent_messages
                    || (max_context_tokens > 0
                        && recent_size
                            >= (max_context_tokens as f64 * strategy.max_recent_size_ratio)
                                as usize);
                if reaches_max && best_n.is_some() {
                    break;
                }
                recent_messages += 1;
                let next_index = messages.len() - recent_messages;
                recent_size += estimate_message_tokens(&messages[next_index]);
            }
            let compacted_count = best_n.unwrap_or_else(|| {
                let last_index = messages.len() - 1;
                if can_split_after(messages, last_index) {
                    messages.len()
                } else {
                    0
                }
            });
            fit_compact_count_to_window(messages, compacted_count, max_context_tokens)
        }
    }
}

/// Shrink `compacted_count` so the compacted prefix fits within the context
/// window, never returning a value that would split a tool exchange.
fn fit_compact_count_to_window(
    messages: &[AgentMessage],
    compacted_count: usize,
    max_context_tokens: usize,
) -> usize {
    if max_context_tokens == 0 || compacted_count == 0 {
        return compacted_count;
    }
    let mut compacted_size: usize = messages[..compacted_count]
        .iter()
        .map(estimate_message_tokens)
        .sum();
    if compacted_size <= max_context_tokens {
        return compacted_count;
    }

    let mut best_n = compacted_count;
    for n in (1..compacted_count).rev() {
        compacted_size -= estimate_message_tokens(&messages[n]);
        if !can_split_after(messages, n - 1) {
            continue;
        }
        best_n = n;
        if compacted_size <= max_context_tokens {
            return n;
        }
    }
    best_n
}

/// When the initial compaction estimate overflows the window, shrink the
/// compacted prefix to the smallest safe boundary that still yields a
/// meaningful reduction.
#[must_use]
pub fn reduce_compact_on_overflow(
    messages: &[AgentMessage],
    min_reduction_ratio: f64,
    max_context_tokens: usize,
) -> usize {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let min_reduced_size = ((max_context_tokens as f64) * min_reduction_ratio).ceil() as usize;
    let mut reduced_size = 0usize;
    let mut best_n: Option<usize> = None;

    for index in (1..messages.len() - 1).rev() {
        reduced_size += estimate_message_tokens(&messages[index + 1]);
        if can_split_after(messages, index) {
            best_n = Some(index + 1);
            if reduced_size >= min_reduced_size {
                return index + 1;
            }
        }
    }
    best_n.unwrap_or(messages.len())
}

/// Render messages into a structured text block for the summariser.
///
/// Format (adapted from kimi-code `render-messages.ts`):
/// ```text
/// --- message 1 role=user ---
/// text:
///   <content>
///
/// --- message 2 role=assistant ---
/// text:
///   <content>
/// tool calls:
///   - <id>: <name>
///     arguments:
///       <json>
/// ```
#[must_use]
pub fn render_messages_to_text(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| render_single_message(message, index))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_single_message(message: &AgentMessage, index: usize) -> String {
    let role = message_role_label(message);
    let mut lines = vec![format!(
        "--- message {pos} role={role} ---",
        pos = index + 1
    )];

    match message {
        AgentMessage::User { .. } if message.is_injection_variant("instruction_epoch") => {
            lines.push("instruction update; body excluded from summary input".to_owned());
        }
        AgentMessage::System { content }
        | AgentMessage::User { content, .. }
        | AgentMessage::ToolResult { content, .. } => {
            render_content_parts(content, &mut lines);
        }
        AgentMessage::Assistant {
            content,
            tool_calls,
            ..
        } => {
            render_content_parts(content, &mut lines);
            if !tool_calls.is_empty() {
                lines.push("tool calls:".to_owned());
                for call in tool_calls {
                    lines.push(format!("  - {}: {}", call.id, call.name));
                    lines.push(format!(
                        "    arguments:\n{}",
                        indent_block(&call.raw_arguments, 6)
                    ));
                }
            }
        }
        AgentMessage::ShellCommand {
            command,
            stdout,
            stderr,
            exit_code,
            outcome,
            truncated,
        } => {
            lines.push(format!("command:\n{}", indent_block(command, 2)));
            lines.push(format!(
                "status: outcome={} exit_code={} truncated={}",
                outcome.as_model_status(),
                exit_code.map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                truncated
            ));
            if !stdout.is_empty() {
                lines.push(format!("stdout:\n{}", indent_block(stdout, 2)));
            }
            if !stderr.is_empty() {
                lines.push(format!("stderr:\n{}", indent_block(stderr, 2)));
            }
        }
    }

    if let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        is_error,
        ..
    } = message
    {
        lines.push(format!(
            "tool_call_id={tool_call_id} tool_name={tool_name} is_error={is_error}"
        ));
    }

    lines.join("\n")
}

fn render_content_parts(content: &[Content], lines: &mut Vec<String>) {
    if content.is_empty() {
        lines.push("[empty content]".to_owned());
        return;
    }
    for part in content {
        match part {
            Content::Text { text } => {
                lines.push(format!("text:\n{}", indent_block(text, 2)));
            }
            Content::Thinking { text, .. } => {
                // Thinking blocks are not sent back to the model, but we keep
                // a compact marker so the summariser knows reasoning existed.
                let preview: String = text.chars().take(120).collect();
                lines.push(format!("think:\n{}", indent_block(&preview, 2)));
            }
            Content::Image { mime_type, .. } => {
                lines.push(format!("  [image: {mime_type}]"));
            }
            Content::Video { mime_type, .. } => {
                lines.push(format!("  [video: {mime_type}]"));
            }
        }
    }
}

fn message_role_label(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
        AgentMessage::ShellCommand { .. } => "shell",
    }
}

fn indent_block(value: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drive an LLM call to produce a structured compaction summary.
///
/// Builds a minimal [`ChatRequest`] (no tools, so the model cannot call tools)
/// whose conversation is the rendered messages plus the compaction instruction,
/// streams the response, and returns the concatenated text.
///
/// `on_progress` is called periodically with the current summary length (in
/// characters) so callers can drive a progress bar based on the streaming
/// output, similar to kimi-code's swarm progress estimator.
///
/// # Hard-fail policy
/// Any LLM error, empty response, or cancellation is returned as
/// [`CompactionError`] — callers must surface it rather than degrading to a
/// counter summary.
pub async fn generate_compaction_summary<F>(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages_to_compact: &[AgentMessage],
    custom_instruction: Option<&str>,
    cancel_token: &CancellationToken,
    mut on_progress: F,
) -> Result<String, CompactionError>
where
    F: FnMut(usize) + Send,
{
    let rendered = render_messages_to_text(messages_to_compact);
    let instruction = render_instruction(custom_instruction);
    let user_prompt = format!("{rendered}\n\n{instruction}");

    let mut chat_messages = Vec::new();
    if let Some(system_prompt) = &config.system_prompt {
        chat_messages.push(ChatMessage::System {
            content: vec![neo_ai::ContentPart::Text {
                text: system_prompt.clone(),
            }],
        });
    }
    chat_messages.push(ChatMessage::User {
        content: vec![neo_ai::ContentPart::Text { text: user_prompt }],
    });

    let request = ChatRequest {
        model: config.model.clone(),
        messages: chat_messages,
        tools: Vec::new(), // no tools — summariser must not call tools
        options: RequestOptions {
            temperature: Some(0.0), // deterministic summary
            ..RequestOptions::default()
        },
    };

    let mut stream = model.stream_chat(request);
    let mut summary = String::new();
    let mut last_progress_chars = 0_usize;

    while let Some(event) = stream.next().await {
        if cancel_token.is_cancelled() {
            return Err(CompactionError::Cancelled);
        }
        match event {
            Ok(AiStreamEvent::TextDelta { text }) => {
                summary.push_str(&text);
                // Throttle progress callbacks to roughly every 200 characters
                // so we do not flood the event channel.
                if summary.len().saturating_sub(last_progress_chars) >= 200 {
                    on_progress(summary.len());
                    last_progress_chars = summary.len();
                }
            }
            Ok(_) => {}
            Err(err) => return Err(CompactionError::Llm(err.to_string())),
        }
    }

    // Final progress update so the bar reaches the estimated cap before the
    // caller switches to the Applying phase.
    if summary.len() > last_progress_chars {
        on_progress(summary.len());
    }

    if summary.trim().is_empty() {
        return Err(CompactionError::Empty);
    }
    Ok(summary)
}

/// Render the compaction instruction, optionally with a custom preamble.
fn render_instruction(custom_instruction: Option<&str>) -> String {
    let custom = custom_instruction.unwrap_or("");
    COMPACTION_INSTRUCTION.replace("{{ customInstruction }}", custom)
}

#[cfg(test)]
#[path = "test_cases/can_split.rs"]
mod can_split;

#[cfg(test)]
#[path = "test_cases/compact_count.rs"]
mod compact_count;

#[cfg(test)]
#[path = "test_cases/render.rs"]
mod render;

#[cfg(test)]
#[path = "test_cases/strategy.rs"]
mod strategy;
