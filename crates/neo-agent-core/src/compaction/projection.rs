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
mod tests {
    use super::*;
    use crate::tools::{SnipHint, Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult};
    use crate::{AgentMessage, Content, StopReason};

    #[test]
    fn request_projection_truncates_old_large_tool_results() {
        let messages = vec![
            AgentMessage::user_text("start"),
            AgentMessage::tool_result(
                "old_call",
                "Read",
                vec![Content::text("x".repeat(8_000))],
                false,
            ),
            AgentMessage::tool_result(
                "new_call",
                "Read",
                vec![Content::text("y".repeat(8_000))],
                false,
            ),
        ];
        let plan = ProjectionPlan {
            enabled: true,
            cutoff_index: 2,
            min_tool_result_tokens: 100,
            keep_recent_messages: 1,
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
            mode: ProjectionMode::Request,
        };

        let result = project_for_request(&messages, &plan);

        assert_eq!(messages[1].text().len(), 8_000);
        assert!(result.messages[1].text().contains("[tool result omitted"));
        assert_eq!(result.messages[2].text().len(), 8_000);
        assert!(result.omitted_tokens > 1_000);
        assert!(result.projected_tokens < crate::runtime::estimate_messages_tokens(&messages));
    }

    #[test]
    fn projection_never_changes_user_or_assistant_messages() {
        let assistant = AgentMessage::assistant(
            vec![Content::text("assistant payload")],
            Vec::new(),
            StopReason::EndTurn,
        );
        let user = AgentMessage::user_text("user payload");
        let messages = vec![user.clone(), assistant.clone()];
        let plan = ProjectionPlan {
            enabled: true,
            cutoff_index: messages.len(),
            min_tool_result_tokens: 1,
            keep_recent_messages: 0,
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
            mode: ProjectionMode::Request,
        };

        let result = project_for_request(&messages, &plan);

        assert_eq!(result.messages, messages);
    }

    #[test]
    fn summary_projection_can_be_more_aggressive_than_request_projection() {
        let messages = vec![
            AgentMessage::tool_result("a", "Read", vec![Content::text("a".repeat(4_000))], false),
            AgentMessage::tool_result("b", "Read", vec![Content::text("b".repeat(4_000))], false),
        ];
        let request_plan = ProjectionPlan {
            enabled: true,
            cutoff_index: 1,
            min_tool_result_tokens: 100,
            keep_recent_messages: 1,
            snip_enabled: false,
            snip_min_tokens: 0,
            snip_keep_recent: 0,
            mode: ProjectionMode::Request,
        };
        let summary_plan = ProjectionPlan {
            mode: ProjectionMode::SummaryInput,
            keep_recent_messages: 0,
            ..request_plan
        };

        let request = project_for_request(&messages, &request_plan);
        let summary = project_for_summary(&messages, &summary_plan);

        assert!(summary.omitted_tokens > request.omitted_tokens);
        assert!(summary.projected_tokens < request.projected_tokens);
    }

    #[test]
    fn micro_projection_never_changes_instruction_messages() {
        let instruction = AgentMessage::Instruction {
            generation: 7,
            content: vec![Content::text("pinned rules ".repeat(4_000))],
        };
        let messages = vec![
            AgentMessage::tool_result(
                "old_call",
                "Read",
                vec![Content::text("x".repeat(8_000))],
                false,
            ),
            instruction.clone(),
            AgentMessage::tool_result(
                "new_call",
                "Read",
                vec![Content::text("y".repeat(8_000))],
                false,
            ),
        ];

        for mode in [ProjectionMode::Request, ProjectionMode::SummaryInput] {
            let plan = ProjectionPlan {
                enabled: true,
                cutoff_index: messages.len(),
                min_tool_result_tokens: 100,
                keep_recent_messages: 0,
                snip_enabled: false,
                snip_min_tokens: 0,
                snip_keep_recent: 0,
                mode,
            };
            let result = match mode {
                ProjectionMode::Request => project_for_request(&messages, &plan),
                ProjectionMode::SummaryInput => project_for_summary(&messages, &plan),
                ProjectionMode::None => unreachable!("test only exercises active modes"),
            };

            // The large tool results around the epoch are projected...
            assert!(result.messages[0].text().contains("omitted"), "{mode:?}");
            assert!(result.messages[2].text().contains("omitted"), "{mode:?}");
            assert!(result.omitted_tokens > 0, "{mode:?}");
            // ...but the pinned instruction message passes through byte-for-byte.
            assert_eq!(result.messages[1], instruction, "{mode:?}");
        }
    }

    const TEST_HINT: SnipHint = SnipHint {
        head_lines: 3,
        tail_lines: 2,
        head_chars: 100,
        tail_chars: 100,
    };

    /// Registers a hinted tool named "HintedRead" through the real registration
    /// path (idempotent) so `snip_hint_for("HintedRead")` resolves in this
    /// process. A unique name is used on purpose: the built-in `ReadTool` is
    /// registered with its own geometry (head 120 / tail 12) by many parallel
    /// lib tests via `ToolRegistry::with_builtin_tools()`, which would
    /// unpredictably overwrite the hint this module's assertions depend on.
    fn register_hinted_read() {
        struct HintedRead {
            hint: SnipHint,
        }
        impl Tool for HintedRead {
            fn name(&self) -> &str {
                "HintedRead"
            }
            fn description(&self) -> &str {
                "hinted read"
            }
            fn input_schema(&self) -> serde_json::Value {
                serde_json::json!({"type": "object"})
            }
            fn execute<'a>(
                &'a self,
                _ctx: &'a ToolContext,
                _input: serde_json::Value,
            ) -> ToolFuture<'a> {
                Box::pin(async { Ok(ToolResult::ok("ok")) })
            }
            fn snip_hint(&self) -> Option<SnipHint> {
                Some(self.hint)
            }
        }
        let mut registry = ToolRegistry::default();
        registry.register(HintedRead { hint: TEST_HINT });
    }

    fn read_result(content: &str) -> AgentMessage {
        AgentMessage::tool_result(
            "call_1",
            "HintedRead",
            vec![Content::text(content.to_owned())],
            false,
        )
    }

    /// `lines` numbered lines, each terminated by a newline — matching real
    /// newline-terminated tool output.
    fn numbered_content(lines: usize) -> String {
        (1..=lines)
            .map(|i| format!("{i}\tline {i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    fn snip_plan(len: usize, keep_recent: usize, min_tokens: usize) -> ProjectionPlan {
        ProjectionPlan {
            enabled: true,
            cutoff_index: len,
            min_tool_result_tokens: usize::MAX,
            keep_recent_messages: 0,
            snip_enabled: true,
            snip_min_tokens: min_tokens,
            snip_keep_recent: keep_recent,
            mode: ProjectionMode::Request,
        }
    }

    #[test]
    fn snip_keeps_head_and_tail_of_stale_read_result() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 10);
        let result = project_for_request(&messages, &plan);
        let text = result.messages[0].text();
        assert!(text.contains("[tool result snipped: tool=HintedRead"));
        assert!(text.contains("1\tline 1"));
        assert!(text.contains("3\tline 3"));
        assert!(text.contains("9\tline 9"));
        assert!(text.contains("10\tline 10"));
        assert!(text.contains("[... 5 lines omitted ...]"));
    }

    #[test]
    fn snip_protects_recent_messages() {
        register_hinted_read();
        // Large enough to pass the snip token gate; the keep-recent protection
        // is the only thing keeping the result verbatim.
        let content = numbered_content(120);
        let messages = vec![read_result(&content), AgentMessage::user_text("recent")];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
        assert_eq!(result.omitted_tokens, 0);
    }

    #[test]
    fn snip_skips_small_results() {
        register_hinted_read();
        // Below snip_min_tokens (10_000); the threshold is the only gate.
        let content = numbered_content(120);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 10_000);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
        assert_eq!(result.omitted_tokens, 0);
    }

    #[test]
    fn snip_skips_error_results() {
        register_hinted_read();
        // Large enough to pass the snip token gate; is_error is the only gate.
        let content = numbered_content(120);
        let messages = vec![AgentMessage::tool_result(
            "call_1",
            "HintedRead",
            vec![Content::text(content.clone())],
            true,
        )];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn snip_skips_non_hinted_tools() {
        // Large enough to pass the snip_min_tokens gate (>= 100 tokens), so the
        // only thing keeping the result verbatim is the missing hint.
        let content = numbered_content(120);
        let messages = vec![AgentMessage::tool_result(
            "call_1",
            "NoHintTool",
            vec![Content::text(content.clone())],
            false,
        )];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn snip_windows_giant_single_line() {
        register_hinted_read();
        let content = "x".repeat(1_000);
        let messages = vec![read_result(&content)];
        let plan = snip_plan(messages.len(), 0, 100);
        let result = project_for_request(&messages, &plan);
        let text = result.messages[0].text();
        assert!(text.contains("chars omitted"));
    }

    #[test]
    fn dedup_collapses_identical_later_read() {
        register_hinted_read();
        let content = numbered_content(120);
        let messages = vec![read_result(&content), read_result(&content)];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert!(
            result.messages[1]
                .text()
                .contains("[duplicate of an earlier HintedRead result")
        );
        assert!(result.messages[1].text().contains("message index 0"));
        assert_eq!(result.messages[0].text(), content);
        assert!(result.omitted_tokens > 0);
    }

    #[test]
    fn dedup_keeps_different_content() {
        register_hinted_read();
        let a = numbered_content(120);
        let b = numbered_content(121);
        let messages = vec![read_result(&a), read_result(&b)];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[0].text(), a);
        assert_eq!(result.messages[1].text(), b);
    }

    #[test]
    fn dedup_never_collapses_errors() {
        register_hinted_read();
        let content = numbered_content(120);
        let messages = vec![
            AgentMessage::tool_result(
                "c1",
                "HintedRead",
                vec![Content::text(content.clone())],
                true,
            ),
            read_result(&content),
        ];
        let plan = snip_plan(messages.len(), 2, 100);
        let result = project_for_request(&messages, &plan);
        assert_eq!(result.messages[1].text(), content);
    }

    #[test]
    fn dedup_respects_sniped_earlier_result() {
        register_hinted_read();
        let content = numbered_content(120);
        let messages = vec![read_result(&content), read_result(&content)];
        let plan = snip_plan(messages.len(), 1, 100);
        let result = project_for_request(&messages, &plan);
        assert!(result.messages[0].text().contains("[tool result snipped"));
        assert_eq!(result.messages[1].text(), content);
    }

    #[test]
    fn disabled_plan_snips_nothing() {
        register_hinted_read();
        // Large enough that an accidentally enabled plan would snip it, so the
        // disabled plan is the only thing keeping the result verbatim.
        let content = numbered_content(120);
        let messages = vec![read_result(&content)];
        let result = project_for_request(&messages, &ProjectionPlan::disabled());
        assert_eq!(result.messages[0].text(), content);
    }

    #[test]
    fn summary_input_mode_snips_like_request() {
        register_hinted_read();
        let content = numbered_content(10);
        let messages = vec![read_result(&content)];
        let mut plan = snip_plan(messages.len(), 0, 10);
        plan.mode = ProjectionMode::SummaryInput;
        let result = project_for_summary(&messages, &plan);
        assert!(result.messages[0].text().contains("[tool result snipped"));
    }

    #[test]
    fn note_full_result_skips_error_messages() {
        let content = numbered_content(10);
        let mut seen_full = std::collections::HashMap::new();
        note_full_result(
            &AgentMessage::tool_result("c1", "HintedRead", vec![Content::text(content)], true),
            0,
            &mut seen_full,
        );
        assert!(seen_full.is_empty());
    }
}
