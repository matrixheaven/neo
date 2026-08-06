use super::*;
use crate::AgentMessage;
use crate::Content;
use crate::tools::SnipHint;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolFuture;
use crate::tools::ToolRegistry;
use crate::tools::ToolResult;

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
