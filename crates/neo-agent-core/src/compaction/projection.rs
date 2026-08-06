//! Request-time and summary-time projection of large historical tool results.
//!
//! Projection is an ephemeral context-budgeting transform. It operates on owned
//! message copies for model inputs and never mutates durable agent history.

use std::collections::HashMap;

use crate::runtime::estimate_messages_tokens;
use crate::tools::{SnipHint, snip_hint_for};
use crate::{AgentMessage, Content};

/// Projection mode for large historical tool result content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMode {
    /// Projection disabled by mode.
    None,
    /// Projection applied to a normal model request.
    Request,
    /// Projection applied to messages fed into summary generation.
    SummaryInput,
}

/// Projection plan for old, large tool results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionPlan {
    /// Whether projection should run.
    pub enabled: bool,
    /// First message index that must remain verbatim.
    pub cutoff_index: usize,
    /// Minimum estimated tool-result tokens before content is omitted.
    pub min_tool_result_tokens: usize,
    /// Number of newest messages to keep verbatim regardless of cutoff.
    pub keep_recent_messages: usize,
    /// Whether the stale-result snip/dedup maintenance pass runs.
    pub snip_enabled: bool,
    /// Minimum estimated tool-result tokens before a stale result is snipped.
    pub snip_min_tokens: usize,
    /// Number of newest messages exempt from snip.
    pub snip_keep_recent: usize,
    /// Projection mode.
    pub mode: ProjectionMode,
}

impl ProjectionPlan {
    /// Return a disabled projection plan.
    #[must_use]
    pub const fn disabled() -> Self {
        Self {
            enabled: false,
            cutoff_index: 0,
            min_tool_result_tokens: usize::MAX,
            keep_recent_messages: usize::MAX,
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
            mode: ProjectionMode::None,
        }
    }
}

/// Result of projecting messages for an ephemeral model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionResult {
    /// Projected message copy.
    pub messages: Vec<AgentMessage>,
    /// Estimated tokens removed from omitted tool results.
    pub omitted_tokens: usize,
    /// Estimated token count of the projected message copy.
    pub projected_tokens: usize,
}

/// Project messages for a normal model request.
#[must_use]
pub fn project_for_request(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::Request)
}

/// Project messages for summary generation input.
#[must_use]
pub fn project_for_summary(messages: &[AgentMessage], plan: &ProjectionPlan) -> ProjectionResult {
    project_messages(messages, plan, ProjectionMode::SummaryInput)
}

fn project_messages(
    messages: &[AgentMessage],
    plan: &ProjectionPlan,
    mode: ProjectionMode,
) -> ProjectionResult {
    if !plan.enabled || plan.mode == ProjectionMode::None || plan.mode != mode {
        return unchanged(messages);
    }
    let recent_start = messages.len().saturating_sub(plan.keep_recent_messages);
    let cutoff = plan.cutoff_index.min(messages.len());
    let snip_cutoff = messages.len().saturating_sub(plan.snip_keep_recent);
    let mut omitted_tokens = 0;
    // (tool name, content hash) -> (first visible full index, original text).
    let mut seen_full: HashMap<(String, u64), (usize, String)> = HashMap::new();
    let projected = messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            if index >= cutoff || index >= recent_start {
                note_full_result(message, index, &mut seen_full);
                return message.clone();
            }
            project_tool_result(
                message,
                plan,
                mode,
                index,
                snip_cutoff,
                &mut omitted_tokens,
                &mut seen_full,
            )
        })
        .collect::<Vec<_>>();
    let projected_tokens = estimate_messages_tokens(&projected);
    ProjectionResult {
        messages: projected,
        omitted_tokens,
        projected_tokens,
    }
}

fn unchanged(messages: &[AgentMessage]) -> ProjectionResult {
    let messages = messages.to_vec();
    let projected_tokens = estimate_messages_tokens(&messages);
    ProjectionResult {
        messages,
        omitted_tokens: 0,
        projected_tokens,
    }
}

fn project_tool_result(
    message: &AgentMessage,
    plan: &ProjectionPlan,
    mode: ProjectionMode,
    index: usize,
    snip_cutoff: usize,
    omitted_tokens: &mut usize,
    seen_full: &mut HashMap<(String, u64), (usize, String)>,
) -> AgentMessage {
    let AgentMessage::ToolResult {
        tool_call_id,
        tool_name,
        content,
        is_error,
    } = message
    else {
        return message.clone();
    };
    if *is_error {
        return message.clone();
    }
    let content_tokens = estimate_content_tokens(content);
    let hint = snip_hint_for(tool_name);
    let text = text_only(content);

    // 1. Byte-identical duplicate of a still-visible earlier result -> note.
    if plan.snip_enabled
        && let Some(text) = text
        && text.len() >= DEDUP_MIN_BYTES
        && hint.is_some()
    {
        let key = (tool_name.to_string(), fnv1a(text));
        if let Some((first_index, first_text)) = seen_full.get(&key)
            && first_text == text
        {
            let marker = dedup_marker(mode, tool_name, *first_index);
            let replacement_tokens = marker.len().div_ceil(4);
            *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
            return AgentMessage::tool_result(
                tool_call_id.clone(),
                tool_name.clone(),
                vec![Content::text(marker)],
                *is_error,
            );
        }
    }

    // 2. Head/tail snip for stale oversized results from hinted tools.
    if plan.snip_enabled
        && index < snip_cutoff
        && content_tokens >= plan.snip_min_tokens
        && let Some(hint) = hint
        && let Some(body) = snip_text(text, hint)
        && body.len() < text.unwrap_or_default().len()
    {
        let marker = format!(
            "[tool result snipped: tool={tool_name}, approx_tokens={content_tokens}; \
             first {} and last {} lines shown; full content retained in session history; \
             re-run {tool_name} to restore]\n{body}",
            hint.head_lines, hint.tail_lines
        );
        let replacement_tokens = marker.len().div_ceil(4);
        *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
        return AgentMessage::tool_result(
            tool_call_id.clone(),
            tool_name.clone(),
            vec![Content::text(marker)],
            *is_error,
        );
    }

    // 3. Historical full-omission path (micro compaction, non-hinted tools).
    if content_tokens >= plan.min_tool_result_tokens {
        let marker = omission_marker(mode, tool_name, content_tokens);
        let replacement_tokens = marker.len().div_ceil(4);
        *omitted_tokens += content_tokens.saturating_sub(replacement_tokens);
        return AgentMessage::tool_result(
            tool_call_id.clone(),
            tool_name.clone(),
            vec![Content::text(marker)],
            *is_error,
        );
    }

    // 4. Kept verbatim: a full, visible result later duplicates can match.
    note_full_result(message, index, seen_full);
    message.clone()
}

/// Record a result that stays full and visible in this request, so a later
/// byte-identical duplicate can be collapsed to a short pointer note.
fn note_full_result(
    message: &AgentMessage,
    index: usize,
    seen_full: &mut HashMap<(String, u64), (usize, String)>,
) {
    let AgentMessage::ToolResult {
        tool_name,
        content,
        is_error,
        ..
    } = message
    else {
        return;
    };
    if *is_error {
        return;
    }
    if snip_hint_for(tool_name).is_none() {
        return;
    }
    let Some(text) = text_only(content) else {
        return;
    };
    if text.len() < DEDUP_MIN_BYTES {
        return;
    }
    seen_full
        .entry((tool_name.to_string(), fnv1a(text)))
        .or_insert_with(|| (index, text.to_owned()));
}

/// Byte threshold before a tool result joins the dedup index / dedup check.
const DEDUP_MIN_BYTES: usize = 1024;

fn text_only(content: &[Content]) -> Option<&str> {
    let mut texts = content.iter().filter_map(Content::as_text);
    let first = texts.next()?;
    if texts.next().is_some() {
        // Mixed or multi-part content (e.g. images): keep verbatim.
        return None;
    }
    Some(first)
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Head/tail snippet of a stale tool result. Returns `None` when the text is
/// not single-part text. The returned body is strictly smaller than the input.
fn snip_text(text: Option<&str>, hint: SnipHint) -> Option<String> {
    let text = text?;
    // A single trailing newline must not count as an extra empty line: it
    // would shift the tail window off the last real line of newline-terminated
    // tool output (the common case for Bash/file reads).
    let text = text.strip_suffix('\n').unwrap_or(text);
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() <= hint.head_lines + hint.tail_lines {
        // A giant single line (or few lines): keep rune windows from both ends.
        let total = text.chars().count();
        if total <= hint.head_chars + hint.tail_chars {
            return Some(text.to_owned());
        }
        let head_end = text
            .char_indices()
            .nth(hint.head_chars)
            .map_or(text.len(), |(i, _)| i);
        let tail_start = text
            .char_indices()
            .nth(total - hint.tail_chars)
            .map_or(text.len(), |(i, _)| i);
        let head = &text[..head_end];
        let tail = &text[tail_start..];
        let omitted_chars = total - hint.head_chars - hint.tail_chars;
        return Some(format!(
            "{head}\n[... {omitted_chars} chars omitted ...]\n{tail}"
        ));
    }
    let head = lines[..hint.head_lines].join("\n");
    let tail = lines[lines.len() - hint.tail_lines..].join("\n");
    let omitted = lines.len() - hint.head_lines - hint.tail_lines;
    Some(format!("{head}\n[... {omitted} lines omitted ...]\n{tail}"))
}

fn dedup_marker(mode: ProjectionMode, tool_name: &str, first_index: usize) -> String {
    match mode {
        ProjectionMode::None => unreachable!("disabled projection must not build markers"),
        ProjectionMode::Request => format!(
            "[duplicate of an earlier {tool_name} result in this request (message index {first_index}); byte-identical content omitted; re-run {tool_name} to restore full content]"
        ),
        ProjectionMode::SummaryInput => format!("[duplicate {tool_name} {first_index}]"),
    }
}

fn omission_marker(mode: ProjectionMode, tool_name: &str, content_tokens: usize) -> String {
    match mode {
        ProjectionMode::None => unreachable!("disabled projection must not build markers"),
        ProjectionMode::Request => {
            format!("[tool result omitted: tool={tool_name}, approx_tokens={content_tokens}]")
        }
        ProjectionMode::SummaryInput => format!("[omitted {tool_name} {content_tokens}t]"),
    }
}

fn estimate_content_tokens(content: &[Content]) -> usize {
    content
        .iter()
        .map(|part| match part {
            Content::Text { text } | Content::Thinking { text, .. } => text.len().div_ceil(4),
            Content::Image { .. } => 4_800,
        })
        .sum()
}

#[cfg(test)]
#[path = "test_cases/projection_variants.rs"]
mod projection_variants;

#[cfg(test)]
#[path = "test_cases/snip.rs"]
mod snip;

#[cfg(test)]
#[path = "test_cases/dedup.rs"]
mod dedup;

#[cfg(test)]
#[path = "test_cases/note.rs"]
mod note;
