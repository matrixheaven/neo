use super::*;
use crate::AgentMessage;
use crate::Content;

/// `lines` numbered lines, each terminated by a newline — matching real
/// newline-terminated tool output.
fn numbered_content(lines: usize) -> String {
    (1..=lines)
        .map(|i| format!("{i}\tline {i}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
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
