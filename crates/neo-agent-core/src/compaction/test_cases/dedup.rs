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
