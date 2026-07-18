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

use std::path::{Path, PathBuf};

use crate::tools::normalize_path;
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

// ---------------------------------------------------------------------------
// Typed instruction-scope probes
// ---------------------------------------------------------------------------

/// A typed instruction-scope probe derived from one prepared tool call.
///
/// Probes come only from typed arguments — never from shell command text,
/// MCP payloads, or additional workspace roots. `Read`/`Write`/`Edit` probe
/// the parent directory of the target file, `List`/`Grep`/`Find`/`Glob`
/// probe their explicit root (defaulting to the primary workspace), and
/// `Bash`/`Terminal`(start) probe the explicit `cwd` (defaulting to the
/// primary workspace). Anything else carries no probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionScopeProbe {
    /// Directory whose primary-workspace-to-target chain scope discovery
    /// scans for `AGENTS.md` files.
    pub target_directory: PathBuf,
}

impl InstructionScopeProbe {
    /// Derive the probe for one prepared tool call. Returns `None` when the
    /// tool class carries no probe or the typed path resolves outside the
    /// primary workspace.
    #[must_use]
    pub fn from_prepared_tool(
        name: &str,
        arguments: &serde_json::Value,
        primary_workspace: &Path,
    ) -> Option<Self> {
        let target_directory = match name {
            "Read" | "Write" | "Edit" => {
                let path = arguments.get("path").and_then(serde_json::Value::as_str)?;
                let resolved = resolve_probe_path(path, primary_workspace)?;
                // File tools probe the file's parent directory.
                probe_existing_directory(resolved.parent()?, primary_workspace)
            }
            "List" | "Grep" | "Find" | "Glob" => {
                let root = arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.trim().is_empty());
                match root {
                    Some(root) => {
                        let resolved = resolve_probe_path(root, primary_workspace)?;
                        probe_existing_directory(&resolved, primary_workspace)
                    }
                    None => Some(primary_workspace.to_path_buf()),
                }
            }
            "Bash" => probe_shell_cwd(arguments, primary_workspace),
            "Terminal" => {
                let mode = arguments
                    .get("mode")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if mode != "start" {
                    return None;
                }
                probe_shell_cwd(arguments, primary_workspace)
            }
            _ => return None,
        }?;
        Some(Self { target_directory })
    }
}

/// Probe the explicit `cwd` of a shell tool, falling back to the primary
/// workspace when no `cwd` is given. The command string is never inspected.
fn probe_shell_cwd(arguments: &serde_json::Value, primary_workspace: &Path) -> Option<PathBuf> {
    let cwd = arguments
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty());
    match cwd {
        Some(cwd) => {
            let resolved = resolve_probe_path(cwd, primary_workspace)?;
            probe_existing_directory(&resolved, primary_workspace)
        }
        None => Some(primary_workspace.to_path_buf()),
    }
}

/// Resolve a typed path argument against the primary workspace. Relative
/// paths join the workspace; absolute paths must stay inside it (lexically,
/// or through a symlinked prefix once canonicalized).
fn resolve_probe_path(raw: &str, primary_workspace: &Path) -> Option<PathBuf> {
    let candidate = Path::new(raw);
    let joined = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        primary_workspace.join(candidate)
    };
    let normalized = normalize_path(&joined);
    if normalized.starts_with(primary_workspace) {
        return Some(normalized);
    }
    // Absolute paths may reach the workspace through a symlinked prefix.
    let canonical = normalized.canonicalize().ok()?;
    canonical
        .starts_with(primary_workspace)
        .then_some(canonical)
}

/// Reduce `candidate` to the deepest existing directory on its own chain
/// that stays inside the primary workspace. A file argument probes its
/// parent; a missing directory probes its deepest existing ancestor (a
/// missing directory holds no `AGENTS.md`, but its ancestors may).
fn probe_existing_directory(candidate: &Path, primary_workspace: &Path) -> Option<PathBuf> {
    let mut current = Some(candidate);
    while let Some(dir) = current {
        if !dir.starts_with(primary_workspace) {
            return None;
        }
        if dir.is_dir() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
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

    #[test]
    fn typed_scope_probes_cover_files_roots_and_explicit_shell_cwds_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let nested = workspace.join("nested");
        std::fs::create_dir_all(&nested).expect("nested dir");
        std::fs::write(nested.join("file.txt"), "body").expect("nested file");
        let external = temp.path().join("external.txt");
        std::fs::write(&external, "outside").expect("external file");
        let workspace = workspace.canonicalize().expect("canonical workspace");
        let nested = workspace.join("nested");
        let external = external.canonicalize().expect("canonical external");

        let probe = |name: &str, arguments: serde_json::Value| {
            InstructionScopeProbe::from_prepared_tool(name, &arguments, &workspace)
                .map(|probe| probe.target_directory)
        };
        let absolute = |path: &std::path::Path| path.to_string_lossy().to_string();

        // Read/Write/Edit probe the parent directory of the typed file path.
        for name in ["Read", "Write", "Edit"] {
            assert_eq!(
                probe(name, json!({ "path": "nested/file.txt" })),
                Some(nested.clone()),
                "{name} relative path"
            );
            assert_eq!(
                probe(name, json!({ "path": absolute(&nested.join("file.txt")) })),
                Some(nested.clone()),
                "{name} absolute path"
            );
            assert_eq!(
                probe(name, json!({ "path": absolute(&external) })),
                None,
                "{name} external absolute path"
            );
        }
        // List/Grep/Find/Glob probe the explicit root; an omitted root is the
        // primary workspace.
        for name in ["List", "Grep", "Find", "Glob"] {
            assert_eq!(
                probe(name, json!({ "path": "nested" })),
                Some(nested.clone()),
                "{name} explicit root"
            );
            assert_eq!(
                probe(name, json!({})),
                Some(workspace.clone()),
                "{name} default root"
            );
            assert_eq!(
                probe(name, json!({ "path": absolute(&external) })),
                None,
                "{name} external root"
            );
        }
        // A file-valued Grep path probes the file's parent directory.
        assert_eq!(
            probe("Grep", json!({ "pattern": "x", "path": "nested/file.txt" })),
            Some(nested.clone())
        );
        // Bash and Terminal(start) probe the explicit cwd, falling back to the
        // primary workspace. Command strings are never parsed for paths.
        assert_eq!(
            probe("Bash", json!({ "command": "true" })),
            Some(workspace.clone())
        );
        assert_eq!(
            probe("Bash", json!({ "command": "true", "cwd": "nested" })),
            Some(nested.clone())
        );
        assert_eq!(
            probe("Bash", json!({ "command": "cd nested && cat AGENTS.md" })),
            Some(workspace.clone()),
            "shell command text must not influence the probe"
        );
        assert_eq!(
            probe(
                "Bash",
                json!({ "command": "true", "cwd": absolute(&external) })
            ),
            None
        );
        assert_eq!(
            probe("Terminal", json!({ "mode": "start", "command": "top" })),
            Some(workspace.clone())
        );
        assert_eq!(
            probe(
                "Terminal",
                json!({ "mode": "start", "command": "top", "cwd": "nested" })
            ),
            Some(nested.clone())
        );
        for mode in ["write", "read", "resize", "stop"] {
            assert_eq!(
                probe(
                    "Terminal",
                    json!({ "mode": mode, "handle": "h", "cwd": "nested" })
                ),
                None,
                "Terminal {mode} adds no new probe"
            );
        }
        // Other tools and MCP-style names never probe.
        for name in ["TodoList", "Delegate", "mcp__docs__search"] {
            assert_eq!(probe(name, json!({})), None, "{name}");
        }
        // Paths escaping the workspace never probe.
        assert_eq!(probe("Read", json!({ "path": "../outside.txt" })), None);
    }
}
