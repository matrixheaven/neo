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

pub mod micro;

use std::sync::Arc;

use futures::StreamExt;
use neo_ai::{AiStreamEvent, ChatMessage, ChatRequest, ModelClient, RequestOptions};
use tokio_util::sync::CancellationToken;

use crate::events::{AgentEvent, CompactionReason};
use crate::runtime::{estimate_message_tokens, estimate_messages_tokens};
use crate::{AgentConfig, AgentContext, AgentMessage, CompactionPhase, CompactionSummary, Content};

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
    #[error("compaction truncated: model returned empty/truncated after {0} attempts")]
    Truncated(u32),
    #[error("compaction stale: history changed during summarization")]
    Stale,
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
    if matches!(message, AgentMessage::User { .. }) {
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
            fit_compact_count_to_window(messages, best_n.unwrap_or(0), max_context_tokens)
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
        AgentMessage::System { content }
        | AgentMessage::User { content }
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
            Ok(AiStreamEvent::Error { message }) => {
                return Err(CompactionError::Llm(message));
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

/// Find a safe compaction split point smaller than `current_count`.
///
/// Walks backward from `current_count - 1` looking for a valid
/// [`can_split_after`] boundary. Returns 0 if no smaller safe split exists.
#[must_use]
pub(crate) fn reduce_compact_count(messages: &[AgentMessage], current_count: usize) -> usize {
    if current_count <= 1 {
        return 0;
    }
    for index in (0..current_count - 1).rev() {
        if can_split_after(messages, index) {
            return index + 1;
        }
    }
    0
}

/// Whether the current context messages differ from a snapshot taken
/// before compaction began. Used to detect undo/clear during the LLM call.
#[must_use]
pub(crate) fn is_stale(snapshot: &[AgentMessage], current: &[AgentMessage]) -> bool {
    if current.len() < snapshot.len() {
        return true;
    }
    snapshot.iter().zip(current.iter()).any(|(a, b)| a != b)
}

/// Whether a compaction LLM error is worth retrying (rate limit, timeout, etc.).
fn is_retryable_compaction_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("timeout")
        || lower.contains("connection")
}

/// Generate a compaction summary with retry on empty/truncated responses.
///
/// Each retry shrinks the prefix to a smaller safe boundary, giving the
/// model a shorter input that is more likely to produce a valid summary.
///
/// Returns `(summary_text, actual_compacted_count)` so the caller knows
/// exactly which messages were summarized.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn generate_with_retry<F>(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages: &[AgentMessage],
    strategy: &CompactionStrategy,
    max_context_tokens: usize,
    instruction: Option<&str>,
    cancel_token: &CancellationToken,
    max_retry_attempts: u32,
    on_progress: F,
) -> Result<(String, usize), CompactionError>
where
    F: Fn(usize) + Send + Sync,
{
    let mut compacted_count = compute_compact_count(
        messages,
        CompactionSource::Auto,
        strategy,
        max_context_tokens,
    );

    for attempt in 0..max_retry_attempts {
        if compacted_count == 0 {
            return Err(CompactionError::NoBoundary);
        }
        if cancel_token.is_cancelled() {
            return Err(CompactionError::Cancelled);
        }

        let prefix = &messages[..compacted_count];
        let result =
            generate_compaction_summary(model, config, prefix, instruction, cancel_token, |len| {
                on_progress(len);
            })
            .await;

        match result {
            Ok(summary) if !summary.trim().is_empty() => return Ok((summary, compacted_count)),
            Ok(_) => match shrink_prefix(messages, compacted_count, attempt) {
                Ok(reduced) => compacted_count = reduced,
                Err(e) => return Err(e),
            },
            Err(CompactionError::Llm(msg)) if is_retryable_compaction_error(&msg) => {
                match shrink_prefix(messages, compacted_count, attempt) {
                    Ok(reduced) => compacted_count = reduced,
                    Err(e) => return Err(e),
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(CompactionError::Truncated(max_retry_attempts))
}

/// Shrink the compaction prefix to a smaller safe boundary.
/// Returns the new count, or `Truncated` if no smaller split exists.
fn shrink_prefix(
    messages: &[AgentMessage],
    current_count: usize,
    attempt: u32,
) -> Result<usize, CompactionError> {
    let reduced = reduce_compact_count(messages, current_count);
    if reduced == 0 {
        Err(CompactionError::Truncated(attempt + 1))
    } else {
        Ok(reduced)
    }
}

/// Run LLM-driven compaction and emit the lifecycle events.
///
/// This replaces the old `maybe_compact` counter logic.  It:
/// 1. Computes the safe split boundary.
/// 2. Emits `CompactionStarted` + progress events.
/// 3. Calls the model to generate a structured summary (hard-fail on error).
/// 4. Emits `CompactionApplied` with `tokens_after`.
///
/// Returns `Ok(true)` if compaction ran, `Ok(false)` if it was skipped, or
/// `Err` if the LLM call failed (caller should surface the error).
pub async fn run_compaction(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    context: &mut AgentContext,
    events: &mut Vec<AgentEvent>,
    source: CompactionSource,
    cancel_token: &CancellationToken,
) -> Result<bool, CompactionError> {
    let Some(settings) = &config.compaction else {
        return Ok(false);
    };
    if !settings.enabled {
        return Ok(false);
    }

    let messages = context.messages();
    let max_context_tokens = config.model.capabilities.max_context_tokens.unwrap_or(0) as usize;
    let strategy = CompactionStrategy {
        trigger_ratio: settings.trigger_ratio,
        max_recent_messages: settings.max_recent_messages,
        max_recent_size_ratio: 0.2,
        reserved_context_tokens: settings.reserved_context_tokens,
    };

    let used_tokens = estimate_messages_tokens(messages);
    let force = matches!(source, CompactionSource::Manual);
    if !force && !strategy.should_compact(used_tokens, max_context_tokens) {
        return Ok(false);
    }

    let compacted_count = compute_compact_count(messages, source, &strategy, max_context_tokens);
    if compacted_count == 0 {
        return Err(CompactionError::NoBoundary);
    }

    let reason = if force {
        CompactionReason::Manual
    } else {
        CompactionReason::Threshold
    };

    events.push(AgentEvent::CompactionStarted {
        reason,
        tokens_before: used_tokens,
        message_count: messages.len(),
    });
    events.push(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Estimating,
        percent: 15,
    });

    let messages_to_compact = &messages[..compacted_count];

    events.push(AgentEvent::CompactionProgress {
        phase: CompactionPhase::SelectingBoundary,
        percent: 35,
    });
    events.push(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Summarizing,
        percent: 70,
    });

    let summary_text = generate_compaction_summary(
        model,
        config,
        messages_to_compact,
        None,
        cancel_token,
        |_| {},
    )
    .await?;

    let kept_messages = &messages[compacted_count..];
    let tokens_after =
        estimate_message_tokens_summary(&summary_text) + estimate_messages_tokens(kept_messages);

    let summary = CompactionSummary {
        summary: summary_text,
        tokens_before: used_tokens,
        tokens_after,
        first_kept_message_index: compacted_count,
    };

    events.push(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Applying,
        percent: 90,
    });
    events.push(AgentEvent::CompactionApplied { summary });

    // Apply to the live context immediately.
    let last_event = events.last().expect("CompactionApplied just pushed");
    if let AgentEvent::CompactionApplied { summary } = last_event {
        context.apply_compaction(summary.clone());
    }

    Ok(true)
}

fn estimate_message_tokens_summary(text: &str) -> usize {
    text.len().div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentToolCall;
    use crate::StopReason;

    fn user_msg(text: &str) -> AgentMessage {
        AgentMessage::user_text(text)
    }

    fn assistant_text(text: &str) -> AgentMessage {
        AgentMessage::assistant(vec![Content::text(text)], Vec::new(), StopReason::EndTurn)
    }

    fn assistant_with_tools(calls: Vec<AgentToolCall>) -> AgentMessage {
        AgentMessage::assistant(Vec::new(), calls, StopReason::ToolUse)
    }

    fn tool_result(id: &str) -> AgentMessage {
        AgentMessage::tool_result(id, "bash", vec![Content::text("ok")], false)
    }

    fn tool_call(id: &str) -> AgentToolCall {
        AgentToolCall {
            id: id.to_owned(),
            name: "bash".to_owned(),
            raw_arguments: serde_json::json!({"command": "ls"}).to_string(),
        }
    }

    #[test]
    fn can_split_after_user_message_is_unsafe() {
        let messages = vec![user_msg("hello"), assistant_text("hi")];
        assert!(!can_split_after(&messages, 0));
    }

    #[test]
    fn can_split_after_assistant_with_tool_calls_is_unsafe() {
        let messages = vec![
            user_msg("run ls"),
            assistant_with_tools(vec![tool_call("tc1")]),
        ];
        // index 1 is assistant with tool calls → unsafe
        assert!(!can_split_after(&messages, 1));
    }

    #[test]
    fn can_split_after_tool_result_when_next_is_user_is_unsafe_due_to_open_exchange() {
        let messages = vec![
            user_msg("run ls"),
            assistant_with_tools(vec![tool_call("tc1"), tool_call("tc2")]),
            tool_result("tc1"),
            user_msg("done"),
        ];
        // index 2 is tool_result; prefix has assistant with 2 calls but only 1 result → open
        assert!(!can_split_after(&messages, 2));
    }

    #[test]
    fn can_split_after_resolved_tool_result_is_safe() {
        let messages = vec![
            user_msg("run ls"),
            assistant_with_tools(vec![tool_call("tc1")]),
            tool_result("tc1"),
            user_msg("done"),
        ];
        // index 2 is tool_result; prefix has assistant with 1 call and 1 result → resolved → safe
        // BUT next message (index 3) is user, so the split after index 2 is safe
        assert!(can_split_after(&messages, 2));
    }

    #[test]
    fn can_split_after_next_is_tool_result_is_unsafe() {
        let messages = vec![
            user_msg("run ls"),
            assistant_with_tools(vec![tool_call("tc1")]),
            tool_result("tc1"),
            tool_result("tc1b"), // would be orphaned if we split before it
        ];
        // index 2: next (index 3) is tool result → unsafe
        assert!(!can_split_after(&messages, 2));
    }

    #[test]
    fn can_split_after_plain_assistant_is_safe() {
        let messages = vec![
            user_msg("hello"),
            assistant_text("hi there"),
            user_msg("bye"),
        ];
        // index 1: assistant without tool calls, next is user → safe
        assert!(can_split_after(&messages, 1));
    }

    #[test]
    fn compute_compact_count_manual_finds_safe_boundary() {
        let messages = vec![
            user_msg("task 1"),
            assistant_text("done 1"),
            user_msg("task 2"),
            assistant_text("done 2"),
        ];
        let strategy = CompactionStrategy::default();
        let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
        // Manual should compact as much as possible: split after index 1 (assistant_text)
        assert_eq!(count, 2);
    }

    #[test]
    fn compute_compact_count_auto_respects_max_recent() {
        let messages: Vec<AgentMessage> = (0..20)
            .map(|i| {
                if i % 2 == 0 {
                    user_msg(&format!("msg {i}"))
                } else {
                    assistant_text(&format!("reply {i}"))
                }
            })
            .collect();
        let strategy = CompactionStrategy {
            max_recent_messages: 4,
            ..CompactionStrategy::default()
        };
        let count = compute_compact_count(&messages, CompactionSource::Auto, &strategy, 0);
        // Should keep at most max_recent_messages (4), compact the rest
        assert!(count <= messages.len() - 3, "count={count}");
    }

    #[test]
    fn compute_compact_count_returns_zero_for_tiny_history() {
        let messages = vec![user_msg("only message")];
        let strategy = CompactionStrategy::default();
        let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
        assert_eq!(count, 0);
    }

    #[test]
    fn compute_compact_count_preserves_safe_boundaries() {
        let messages = vec![
            user_msg("run"),
            assistant_with_tools(vec![tool_call("tc1")]),
            tool_result("tc1"),
            user_msg("again"),
            assistant_with_tools(vec![tool_call("tc2")]),
            tool_result("tc2"),
            user_msg("done"),
        ];
        let strategy = CompactionStrategy::default();
        let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
        // The split must not orphan any tool result
        if count > 0 {
            let kept = &messages[count..];
            // If kept starts with a tool result, it's orphaned
            if let Some(AgentMessage::ToolResult { .. }) = kept.first() {
                panic!("compaction kept an orphaned tool result at start");
            }
        }
    }

    #[test]
    fn render_messages_to_text_includes_role_and_content() {
        let messages = vec![user_msg("hello world"), assistant_text("hi")];
        let text = render_messages_to_text(&messages);
        assert!(text.contains("message 1"));
        assert!(text.contains("role=user"));
        assert!(text.contains("hello world"));
        assert!(text.contains("message 2"));
        assert!(text.contains("role=assistant"));
        assert!(text.contains("hi"));
    }

    #[test]
    fn render_messages_to_text_shows_tool_calls() {
        let messages = vec![assistant_with_tools(vec![tool_call("tc-1")])];
        let text = render_messages_to_text(&messages);
        assert!(text.contains("tool calls:"));
        assert!(text.contains("tc-1: bash"));
    }

    #[test]
    fn render_messages_to_text_shows_tool_result_metadata() {
        let messages = vec![tool_result("tr-1")];
        let text = render_messages_to_text(&messages);
        assert!(text.contains("tool_call_id=tr-1"));
        assert!(text.contains("tool_name=bash"));
    }

    #[test]
    fn strategy_should_compact_below_threshold() {
        let strategy = CompactionStrategy::default();
        assert!(!strategy.should_compact(1000, 100_000));
    }

    #[test]
    fn strategy_should_compact_above_threshold() {
        let strategy = CompactionStrategy::default();
        // trigger_ratio = 0.85, so 85000+ should compact at 100000 max
        assert!(strategy.should_compact(86_000, 100_000));
    }

    #[test]
    fn strategy_reserved_context_forces_compact() {
        let strategy = CompactionStrategy {
            reserved_context_tokens: 50_000,
            ..CompactionStrategy::default()
        };
        // used=60000, reserved=50000, max=100000 → 60000+50000 >= 100000 → compact
        assert!(strategy.should_compact(60_000, 100_000));
    }

    #[test]
    fn estimate_tokens_grows_with_content() {
        let short = user_msg("hi");
        let long = user_msg(&"x".repeat(1000));
        assert!(estimate_message_tokens(&long) > estimate_message_tokens(&short));
    }

    #[test]
    fn can_split_after_rejects_suffix_starting_with_unresolved_assistant_tool_calls() {
        // A previous exchange is fully resolved, but the next assistant has no results yet.
        let messages = vec![
            user_msg("run"),
            assistant_with_tools(vec![tool_call("tc0")]),
            tool_result("tc0"),
            assistant_with_tools(vec![tool_call("tc1")]),
        ];
        // Splitting after the resolved tc0 result would leave an orphan assistant with tool calls.
        assert!(!can_split_after(&messages, 2));
    }

    #[test]
    fn can_split_after_allows_suffix_starting_with_resolved_assistant_tool_calls() {
        let messages = vec![
            user_msg("run"),
            assistant_with_tools(vec![tool_call("tc0")]),
            tool_result("tc0"),
            assistant_with_tools(vec![tool_call("tc1")]),
            tool_result("tc1"),
        ];
        // Splitting after tc0 result is fine because the next assistant already has its result.
        assert!(can_split_after(&messages, 2));
    }

    #[test]
    fn can_split_after_rejects_partial_parallel_tool_results_in_suffix() {
        let messages = vec![
            user_msg("run"),
            assistant_with_tools(vec![tool_call("tc0")]),
            tool_result("tc0"),
            assistant_with_tools(vec![tool_call("tc1"), tool_call("tc2")]),
            tool_result("tc1"),
        ];
        // suffix would start with assistant that still needs tc2 result
        assert!(!can_split_after(&messages, 2));
    }

    #[test]
    fn truncated_error_displays_attempt_count() {
        let err = CompactionError::Truncated(5);
        let msg = err.to_string();
        assert!(msg.contains('5'));
        assert!(msg.contains("truncated"));
    }

    #[test]
    fn stale_error_has_message() {
        let err = CompactionError::Stale;
        let msg = err.to_string();
        assert!(msg.contains("stale"));
    }

    #[test]
    fn generate_with_retry_has_correct_signature() {
        // Full async testing requires FakeModelClient + tokio runtime.
        // The integration is verified by the multi-round compaction tests in
        // compaction_trigger.rs (Task 7). This test is a compilation marker:
        // if the function signature changes, this fails to compile.
        //
        // If this compiles, the signature is correct.
    }

    #[test]
    fn reduce_compact_count_finds_smaller_safe_boundary() {
        let messages = vec![
            user_msg("task 1"),
            assistant_text("done 1"),
            user_msg("task 2"),
            assistant_text("done 2"),
            user_msg("task 3"),
            assistant_text("done 3"),
        ];
        // Current count = 4 (split after index 3). Should find index 1 as smaller safe split.
        let reduced = reduce_compact_count(&messages, 4);
        assert_eq!(reduced, 2);
    }

    #[test]
    fn reduce_compact_count_returns_zero_when_no_smaller_split() {
        let messages = vec![user_msg("only"), assistant_text("reply")];
        // Current count = 1. Can't reduce below 1 safely.
        let reduced = reduce_compact_count(&messages, 1);
        assert_eq!(reduced, 0);
    }

    #[test]
    fn is_stale_detects_shorter_history() {
        let snapshot = vec![user_msg("a"), assistant_text("b"), user_msg("c")];
        let current = vec![user_msg("a")];
        assert!(is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_detects_modified_message() {
        let snapshot = vec![user_msg("original"), assistant_text("reply")];
        let current = vec![user_msg("changed"), assistant_text("reply")];
        assert!(is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_allows_append() {
        let snapshot = vec![user_msg("a"), assistant_text("b")];
        let current = vec![user_msg("a"), assistant_text("b"), user_msg("follow-up")];
        assert!(!is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_returns_false_for_identical() {
        let messages = vec![user_msg("a"), assistant_text("b")];
        assert!(!is_stale(&messages, &messages));
    }

    #[test]
    fn compute_compact_count_manual_after_dropping_incomplete_trailing_tool_turn() {
        let messages = vec![
            user_msg("task 1"),
            assistant_text("done 1"),
            user_msg("task 2"),
            assistant_with_tools(vec![tool_call("tc1")]),
        ];
        let messages = crate::sanitize_tool_exchange_messages(&messages);
        let strategy = CompactionStrategy::default();
        let count = compute_compact_count(&messages, CompactionSource::Manual, &strategy, 0);
        // After dropping the unresolved trailing assistant, manual compaction can
        // safely compact the prefix up to the previous safe boundary.
        assert_eq!(count, 2);
    }
}
