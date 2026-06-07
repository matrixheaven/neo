# Tools

Tools are model-visible capabilities that the runtime can authorize and execute.

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

## Intended Runtime Boundary

The future `neo-agent-core` tool layer should separate:

- `ToolRegistry`: lists available tools and their schemas.
- `ToolAuthorizer`: approves, denies, or asks before execution.
- `ToolExecutor`: performs the operation and returns structured output.
- `ToolAuditLog`: records the request, decision, and result for session replay.

This split lets CLI, TUI, and MCP entrypoints share the same policy.

## Example

See [examples/tools/read-file-schema.json](../examples/tools/read-file-schema.json) for the intended shape of a small file-reading tool schema.
