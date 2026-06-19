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
| `Read` | `{ "path": "docs/index.md" }` | file read |
| `List` | `{ "path": "." }` | file read |
| `Grep` | `{ "pattern": "ToolSpec", "path": "crates", "limit": 20 }` | file read |
| `Find` | `{ "pattern": "config", "path": ".", "limit": 20 }` | file read |
| `Glob` | `{ "pattern": "**/*.rs", "path": "crates", "max_matches": 50 }` | file read |
| `Write` | `{ "path": "tmp.txt", "content": "hello" }` | file write |
| `Edit` | `{ "path": "tmp.txt", "old": "hello", "new": "hi", "replace_all": false }` | file write |
| `Bash` | `{ "command": "cargo test -p xtask", "cwd": ".", "timeout": 300, "max_output_bytes": 65536 }` or `{ "command": "cargo test -p xtask", "run_in_background": true, "description": "test xtask" }` | shell |
| `TaskOutput` | `{ "task_id": "bash-...", "block": false, "timeout": 30 }` | tool |
| `TaskStop` | `{ "task_id": "bash-...", "reason": "no longer needed" }` | shell |
| `Terminal` | `{ "mode": "start", "command": "bash" }` then `{ "mode": "write", "handle": "...", "input": "ls\n" }` / `{ "mode": "read", "handle": "..." }` / `{ "mode": "resize", "handle": "...", "cols": 120, "rows": 40 }` / `{ "mode": "stop", "handle": "..." }` | shell |
| `TodoList` | `{ "todos": [{ "title": "Fix bug", "status": "in_progress" }] }` | tool |
| `EnterPlanMode` | `{}` | tool |
| `ExitPlanMode` | `{ "plan_summary": "..." }` | tool |

Additionally, `AskUserQuestion` is available for reverse-RPC user questions but is not
registered by default (requires a channel sender).

All file paths are resolved inside `ToolContext::workspace_root()`. Attempts to
escape the workspace fail before execution.

`Bash` runs foreground commands by default. Set `run_in_background=true` with a
short `description` to start a background task and receive a `task_id`.
`TaskOutput` returns the current status, exit code when finished, and captured
stdout/stderr. `TaskStop` terminates the background shell process group and
returns the captured output. Foreground timeout/cancellation and background stop
clean up the shell process group on Unix, but commands that daemonize into a new
session or process group are outside this compact cleanup contract. It is not a
PTY and does not support interactive stdin.

Foreground `Bash` content is raw terminal text: stdout followed by stderr. The
structured `details` still carry `exit_code`, capped stdout/stderr, and
truncation flags for runtime consumers, but those fields are not surfaced in
the transcript body.

`Edit` returns concise text content for the model and structured `details` for
consumers that need inspection metadata: the relative path, old/new strings,
`replace_all`, and a stable unified diff.

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
for the current `Read` tool shape, and
[examples/rust/tool_schema.rs](../examples/rust/tool_schema.rs) for the Rust
schema-generation API.
