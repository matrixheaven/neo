# Tools

Tools are model-visible capabilities that the runtime can authorize and execute
inside a workspace.

## Model-Facing Shape

`neo-ai` currently exposes:

- `ToolSpec { name, description, input_schema }`
- `ToolCall { id, name, arguments }`
- `ChatMessage::ToolResult { tool_call_id, content, is_error }`

Use `neo_ai::tool_schema::schema_for<T>()` to generate JSON Schema from small serializable Rust input types.

## Schema Rules

- Prefer one clear operation per tool.
- Keep arguments small and operational.
- Avoid provider, sandbox, tracing, or internal runtime metadata in model-facing schemas.
- Use descriptive field names rather than overloaded strings.
- Return errors as tool results when the model can recover.

## Implemented Built-In Tools

`neo_agent_core::ToolRegistry::with_builtin_tools()` currently registers:

| Tool | Arguments | Permission |
| --- | --- | --- |
| `read` | `{ "path": "docs/index.md" }` | file read |
| `list` | `{ "path": "." }` | file read |
| `grep` | `{ "pattern": "ToolSpec", "path": "crates", "limit": 20 }` | file read |
| `find` | `{ "pattern": "config", "path": ".", "limit": 20 }` | file read |
| `write` | `{ "path": "tmp.txt", "content": "hello" }` | file write |
| `edit` | `{ "path": "tmp.txt", "old": "hello", "new": "hi", "replace_all": false }` | file write |
| `bash` | `{ "command": "cargo test -p xtask", "timeout_ms": 30000, "max_output_bytes": 65536 }` or `{ "mode": "start", "command": "cargo test -p xtask" }` then `{ "mode": "poll", "handle": "..." }` | shell |

All file paths are resolved inside `ToolContext::workspace_root()`. Attempts to
escape the workspace fail before execution.

`bash` defaults to foreground mode for compatibility. Background mode is compact:
`start` launches a real child process and returns a handle; `poll` returns the
current status, exit code when finished, and captured stdout/stderr. It is not a
PTY and does not support interactive stdin.

## Runtime Boundary

The `neo-agent-core` tool layer separates:

- `ToolRegistry`: lists available tools and their schemas.
- `PermissionPolicy`: records `Allow`, `Ask`, or `Deny` for file reads, file
  writes, shell, and general tools.
- `Tool`: owns schema generation and execution.
- `ToolResult`: returns text content, error state, and optional details.

Recoverable `ToolError` values from tool lookup, input parsing, or execution
are converted into `ToolResult::error` records and appended as model-visible
tool-result messages. Runtime setup failures, cancellation, and max-turn
boundaries still remain runtime errors or terminal lifecycle events.

When a policy is `Ask`, the runtime emits `AgentEvent::ApprovalRequested` and
executes only if the configured approval handler returns `Allow`. Without a
handler, ask-mode operations return an approval-required tool result instead of
silently executing.

## Example

See [examples/tools/read-file-schema.json](../examples/tools/read-file-schema.json)
for the current `read` tool shape, and
[examples/rust/tool_schema.rs](../examples/rust/tool_schema.rs) for the Rust
schema-generation API.
