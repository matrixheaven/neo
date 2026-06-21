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
| `Bash` | `{ "command": "cargo run -p xtask -- test -p xtask", "cwd": ".", "timeout": 300, "max_output_bytes": 65536 }` or `{ "command": "cargo run -p xtask -- test -p xtask", "run_in_background": true, "description": "test xtask" }` | shell |
| `TaskList` | `{ "active_only": true, "limit": 20 }` | tool |
| `TaskOutput` | `{ "task_id": "bash-...", "block": false, "timeout": 30 }` or `{ "task_id": "question-..." }` | tool |
| `TaskStop` | `{ "task_id": "bash-...", "reason": "no longer needed" }` or `{ "task_id": "question-..." }` | tool |
| `Terminal` | `{ "mode": "start", "command": "bash" }` then `{ "mode": "write", "handle": "...", "input": "ls\n" }` / `{ "mode": "read", "handle": "..." }` / `{ "mode": "resize", "handle": "...", "cols": 120, "rows": 40 }` / `{ "mode": "stop", "handle": "..." }` | shell |
| `TodoList` | `{}` to read, `{ "todos": [{ "title": "Fix bug", "status": "in_progress" }] }` to replace, `{ "todos": [] }` to clear | tool |
| `EnterPlanMode` | `{}` | tool |
| `ExitPlanMode` | `{ "plan_summary": "..." }` | tool |

Additionally, `AskUserQuestion` is available for reverse-RPC user questions but is not
registered by default (requires a channel sender).

All file paths are resolved inside `ToolContext::workspace_root()`. Attempts to
escape the workspace fail before execution.

`Bash` runs foreground commands by default. Set `run_in_background=true` with a
short `description` to start a background task and receive a `bash-*` task id.
`AskUserQuestion` may also set `background=true`, which returns a `question-*`
task id while the TUI keeps the question visible for the user.

Background tasks are managed by the shared Background Task System. `TaskList`
lists active or historical tasks, `TaskOutput` returns the current status and
captured output/answers, and `TaskStop` terminates a background shell process
group or cancels a pending question. Status values are `running`,
`waiting_for_user`, `completed`, `failed`, `stopped`, and `timed_out`.
Foreground timeout/cancellation and background stop clean up the shell process
group on Unix, but commands that daemonize into a new session or process group
are outside this compact cleanup contract. It is not a PTY and does not support
interactive stdin.

Foreground `Bash` content is raw terminal text: stdout followed by stderr. The
structured `details` still carry `exit_code`, capped stdout/stderr, and
truncation flags for runtime consumers, but those fields are not surfaced in
the transcript body.

`Edit` returns concise text content for the model and structured `details` for
consumers that need inspection metadata: the relative path, old/new strings,
`replace_all`, and a stable unified diff.

`TodoList` maintains the model-visible task list for multi-step work. Successful
writes return structured `details.todos`, which the runtime persists as
`TodoUpdated` and the TUI renders in the dedicated Todo panel above the prompt.
Successful `TodoList` result text is still returned to the model, but the TUI
tool transcript hides that duplicate body; failed `TodoList` results remain
visible in the tool card.

## Runtime Boundary

The `neo-agent-core` tool layer separates:

- `ToolRegistry`: lists available tools and their schemas.
- `PermissionMode`: the active `manual`, `auto`, or `yolo` mode that decides
  whether risky tool calls require user approval. Plan mode adds a hard guard
  that cannot be bypassed by `auto` or `yolo`.
- `Tool`: owns schema generation and execution.
- `ToolResult`: returns text content, error state, and optional details.

Recoverable `ToolError` values from tool lookup, input parsing, or execution
are converted into `ToolResult::error` records and appended as model-visible
tool-result messages. Runtime setup failures, cancellation, and max-turn
boundaries still remain runtime errors or terminal lifecycle events.

In `manual` mode, the runtime emits `AgentEvent::ApprovalRequested` before
risky operations and executes only if the configured approval handler returns
allow. Without a handler, the operation returns an approval-required tool
result instead of executing silently. `auto` mode approves tool actions
automatically after hard safety policies, but it denies `AskUserQuestion` so
the model must continue without user input. `yolo` mode approves tool actions
automatically while still allowing explicit `AskUserQuestion` prompts.

Tool calls that can open focused blocking dialogs are serialized within their
model response batch, even when parallel tool execution is enabled. This covers
non-background `AskUserQuestion`, approval-backed operations such as shell or
write approvals, and plan review via `ExitPlanMode`. Later tools in the same
batch do not start until the active dialog-producing call resolves, so the TUI
never has to stack multiple user-choice dialogs at once. `AskUserQuestion` with
`background=true` is intentionally non-blocking and may return a task id
immediately.

## Example

See [examples/tools/read-file-schema.json](../examples/tools/read-file-schema.json)
for the current `Read` tool shape, and
[examples/rust/tool_schema.rs](../examples/rust/tool_schema.rs) for the Rust
schema-generation API.
