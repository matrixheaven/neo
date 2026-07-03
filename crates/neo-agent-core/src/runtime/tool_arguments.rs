//! Tool-argument parsing — turns the raw canonical JSON string stored on
//! `AgentToolCall` into the `serde_json::Value` that permission checks and tool
//! execution expect.
//!
//! The module also implements a guarded "object-prefix" repair: when the raw
//! arguments are a truncated JSON object whose complete top-level pairs include
//! every schema-required field, the incomplete trailing pair is discarded and
//! the recovered object is returned with a repair warning. Any other malformed
//! JSON is surfaced as a `ToolResult` error *before* permission checks or
//! execution run, so the model sees the failure and can retry.

use crate::{AgentToolCall, ToolResult};
use neo_ai::ToolSpec;

/// A fully prepared tool call: the parsed arguments plus the raw canonical
/// string they were derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
    pub arguments: serde_json::Value,
    /// When the arguments were recovered via guarded repair, a human-readable
    /// warning describing what happened. `None` for cleanly parsed arguments.
    pub warning: Option<String>,
}

/// Outcome of attempting to parse raw tool-call arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolArgumentsOutcome {
    /// The raw JSON parsed successfully.
    Valid(serde_json::Value),
    /// The raw JSON was partial but all required fields were recovered.
    Repaired {
        arguments: serde_json::Value,
        warning: String,
    },
    /// The raw JSON is irrecoverably malformed.
    Invalid {
        message: String,
        raw_excerpt: String,
    },
}

/// Parse raw arguments into a `PreparedToolCall`, or return a `ToolResult`
/// error when the JSON is irrecoverably invalid.
pub fn prepare_tool_arguments(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> Result<PreparedToolCall, ToolResult> {
    match parse_tool_arguments(tool_call, tool_specs) {
        ToolArgumentsOutcome::Valid(arguments) => Ok(PreparedToolCall {
            id: tool_call.id.to_string(),
            name: tool_call.name.to_string(),
            raw_arguments: tool_call.raw_arguments.to_string(),
            arguments,
            warning: None,
        }),
        ToolArgumentsOutcome::Repaired { arguments, warning } => Ok(PreparedToolCall {
            id: tool_call.id.to_string(),
            name: tool_call.name.to_string(),
            raw_arguments: tool_call.raw_arguments.to_string(),
            arguments,
            warning: Some(warning),
        }),
        ToolArgumentsOutcome::Invalid {
            message,
            raw_excerpt,
        } => Err(ToolResult::error(message).with_details(serde_json::json!({
            "kind": "invalid_tool_arguments",
            "raw_arguments_excerpt": raw_excerpt,
            "repair_attempted": true,
        }))),
    }
}

/// Parse the raw arguments, applying the guarded object-prefix repair when
/// possible.
pub fn parse_tool_arguments(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> ToolArgumentsOutcome {
    match serde_json::from_str::<serde_json::Value>(&tool_call.raw_arguments) {
        Ok(arguments) => ToolArgumentsOutcome::Valid(arguments),
        Err(strict_err) => {
            if let Some(repaired) = repair_partial_object(tool_call, tool_specs) {
                return ToolArgumentsOutcome::Repaired {
                    arguments: repaired,
                    warning: "recovered complete required fields from partial JSON object"
                        .to_owned(),
                };
            }
            ToolArgumentsOutcome::Invalid {
                message: format!(
                    "Tool arguments were invalid JSON: {strict_err}. Please retry the tool call with complete JSON arguments."
                ),
                raw_excerpt: raw_excerpt(&tool_call.raw_arguments),
            }
        }
    }
}

fn raw_excerpt(raw: &str) -> String {
    const MAX: usize = 512;
    raw.chars().take(MAX).collect()
}

// ---------------------------------------------------------------------------
// Guarded object-prefix repair
// ---------------------------------------------------------------------------

fn repair_partial_object(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> Option<serde_json::Value> {
    let required = required_fields(tool_call, tool_specs)?;
    let object = complete_top_level_pairs(&tool_call.raw_arguments)?;
    if required.iter().all(|field| object.get(field).is_some()) {
        Some(serde_json::Value::Object(object))
    } else {
        None
    }
}

fn required_fields(tool_call: &AgentToolCall, tool_specs: &[ToolSpec]) -> Option<Vec<String>> {
    let spec = tool_specs
        .iter()
        .find(|spec| spec.name == tool_call.name.as_ref())?;
    Some(
        spec.input_schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect(),
    )
}

/// Walk a possibly-truncated JSON object and return every complete top-level
/// key/value pair. A pair is complete when its value is a closed JSON token
/// (string, number, bool, null, array, or nested object). The trailing
/// incomplete pair is silently discarded.
fn complete_top_level_pairs(raw: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let raw = raw.trim_start();
    if !raw.starts_with('{') {
        return None;
    }
    let mut object = serde_json::Map::new();
    let bytes = raw.as_bytes();
    let mut index = 1;
    loop {
        skip_ws_and_commas(bytes, &mut index);
        if index >= bytes.len() || bytes[index] == b'}' {
            return Some(object);
        }
        let key_start = index;
        let (key, after_key) = parse_json_string(raw, key_start)?;
        index = after_key;
        skip_ws(bytes, &mut index);
        if bytes.get(index).copied()? != b':' {
            return Some(object);
        }
        index += 1;
        skip_ws(bytes, &mut index);
        let value_start = index;
        let Some(value_end) = complete_value_end(raw, value_start) else {
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

/// Find the end offset (exclusive) of a complete JSON value starting at `start`,
/// or `None` if the value is truncated.
fn complete_value_end(raw: &str, start: usize) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bash_spec() -> ToolSpec {
        ToolSpec {
            name: "Bash".to_owned(),
            description: "Run command".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": { "type": "string" },
                    "description": { "type": "string" }
                }
            }),
        }
    }

    fn call(raw_arguments: &str) -> AgentToolCall {
        AgentToolCall {
            id: "call-1".into(),
            name: "Bash".into(),
            raw_arguments: raw_arguments.into(),
        }
    }

    #[test]
    fn repairs_optional_tail_when_required_field_is_complete() {
        let outcome = parse_tool_arguments(
            &call(r#"{"command":"uname -a","description": "#),
            &[bash_spec()],
        );
        assert_eq!(
            outcome,
            ToolArgumentsOutcome::Repaired {
                arguments: json!({ "command": "uname -a" }),
                warning: "recovered complete required fields from partial JSON object".to_owned(),
            }
        );
    }

    #[test]
    fn repairs_when_object_ends_after_complete_required_field_without_comma() {
        let outcome = parse_tool_arguments(&call(r#"{"command":"uname -a""#), &[bash_spec()]);
        assert_eq!(
            outcome,
            ToolArgumentsOutcome::Repaired {
                arguments: json!({ "command": "uname -a" }),
                warning: "recovered complete required fields from partial JSON object".to_owned(),
            }
        );
    }

    #[test]
    fn rejects_incomplete_required_field() {
        let outcome = parse_tool_arguments(&call(r#"{"command":"uname -"#), &[bash_spec()]);
        assert!(matches!(outcome, ToolArgumentsOutcome::Invalid { .. }));
    }

    #[test]
    fn rejects_truncated_numeric_required_field() {
        let spec = ToolSpec {
            name: "NumberTool".to_owned(),
            description: "Use number".to_owned(),
            input_schema: json!({
                "type": "object",
                "required": ["limit"],
                "properties": {
                    "limit": { "type": "number" }
                }
            }),
        };
        let outcome = parse_tool_arguments(
            &AgentToolCall {
                id: "call-1".into(),
                name: "NumberTool".into(),
                raw_arguments: r#"{"limit": 1"#.into(),
            },
            &[spec],
        );

        assert!(matches!(outcome, ToolArgumentsOutcome::Invalid { .. }));
    }

    #[test]
    fn rejects_unknown_tool_partial_json() {
        let outcome = parse_tool_arguments(
            &AgentToolCall {
                id: "call-1".into(),
                name: "Unknown".into(),
                raw_arguments: r#"{"command":"uname -a","description": "#.into(),
            },
            &[bash_spec()],
        );
        assert!(matches!(outcome, ToolArgumentsOutcome::Invalid { .. }));
    }
}
