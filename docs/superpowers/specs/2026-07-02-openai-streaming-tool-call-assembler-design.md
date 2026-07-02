# OpenAI Streaming Tool-Call Assembly

**Date:** 2026-07-02
**Status:** Implemented and verified on 2026-07-02
**Crates affected:** `neo-ai`, `neo-agent-core`
**Primary files:** `crates/neo-ai/src/providers/openai/compatible.rs`, `crates/neo-ai/src/providers/openai/responses.rs`, `crates/neo-ai/src/types.rs`, `crates/neo-agent-core/src/runtime/stream_aggregator.rs`, `crates/neo-agent-core/src/runtime/tool_dispatch.rs`

## Goal

Make Neo's OpenAI protocol handling robust to streaming tool-call arguments that arrive as partial, interleaved, reordered, duplicated, or provider-mutated chunks. The provider layer must not stop the model turn just because accumulated tool-call arguments are not strict JSON at stream finish. It should preserve the raw argument string, emit stable tool-call events, and let runtime decide whether the final raw arguments are executable, repairable, or should become a model-visible tool error.

This is not a Xiaomi-specific compatibility patch. Xiaomi/MiMo's half argument sample is one regression fixture for a general OpenAI Chat Completions streaming problem.

## Current Problem

`openai/compatible.rs` accumulates `function.arguments` fragments and, in `finish_events`, directly parses the accumulated string with `serde_json::from_str`. If the stream ends with a half JSON object such as:

```json
{"command":"uname -a","description":
```

the provider returns `AiError::Stream`, the runtime reports a stopped run, and the model cannot recover. That is the wrong layer for this failure. The provider has only seen protocol bytes; it does not know tool schemas, required fields, or whether an incomplete optional field can be ignored safely.

The Chat path also lacks a final item event equivalent to Responses `output_item.done`. It currently treats accumulated deltas as final parsed JSON. That makes these cases fragile:

- Empty or duplicate deltas.
- `finish_reason = "tool_calls"` after the last argument delta.
- Tool-call id changes mid-stream.
- Arguments arrive before `function.name`.
- Multiple tool calls interleave by `index`.

## Reference Behavior

Codex separates stream event parsing from tool execution. Responses events treat item completion as authoritative, and raw arguments remain available until the tool handler parses them. If parsing fails, the failure is converted into a model-visible tool result instead of a provider stream error.

Pi and Kimi both have practical tolerance for unstable Chat tool-call chunks: route by stable index, buffer unnamed arguments, and avoid splitting one logical call when a provider mutates the id.

Neo should adopt the same layered shape in Rust terms:

- Provider: assemble stream fragments into raw tool-call records.
- Runtime: parse raw arguments against a known tool and schema.
- Tool dispatch: execute only validated arguments.
- Model feedback: malformed or unrecoverable arguments become tool results with `is_error = true`.

## Approaches Considered

### Approach A: Provider-local Lenient Parse

Keep `AiStreamEvent::ToolCallEnd { arguments: Value }`, make `compatible.rs` try strict JSON first, then repair partial JSON locally.

This is small but keeps the wrong ownership boundary. The provider would need schema knowledge to decide whether a partial object is safe. It would also leave Responses and future OpenAI-compatible providers with similar ad hoc logic.

### Approach B: Shared Assembler With Raw Arguments, Runtime Parse

Add a provider-neutral `StreamingToolCallAssembler` in `neo-ai`. It assembles Chat and Responses stream fragments into stable raw argument strings. Change stream completion events so tool-call end carries raw arguments, not already-parsed JSON. `neo-agent-core` parses or guarded-repairs raw arguments before permission checks and tool execution.

This is the recommended path. It fixes the fatal layer boundary, gives Chat a real finalization step, and lets runtime return model-visible tool errors without pretending invalid JSON is provider transport failure.

### Approach C: Full Tool-Call Model Rewrite

Change `ToolCall`, `AgentToolCall`, chat history serialization, session JSONL, and all providers to store only raw arguments everywhere.

This is conceptually clean but too broad for this task. The implementation plan should only make the minimum canonical change needed at the streaming/runtime boundary, then migrate storage and provider serialization if the implementation reveals that the old parsed-only `ToolCall` model is still blocking model-visible error feedback.

## Design Decision

Use Approach B.

The canonical event boundary becomes:

```rust
AiStreamEvent::ToolCallEnd {
    id: String,
    raw_arguments: String,
}
```

`ToolCallArgsDelta` remains a preview event:

```rust
AiStreamEvent::ToolCallArgsDelta {
    id: String,
    json_fragment: String,
}
```

The preview delta is never executable. The final raw string is the only source runtime may parse for execution.

`neo-agent-core` converts raw arguments into one of three outcomes:

```rust
enum ToolArgumentsOutcome {
    Valid(serde_json::Value),
    Repaired {
        arguments: serde_json::Value,
        warning: String,
    },
    Invalid {
        message: String,
        raw_excerpt: String,
    },
}
```

Only `Valid` and accepted `Repaired` outcomes can reach permission evaluation or tool execution. `Invalid` produces a tool result error addressed to the same tool-call id, so the model can retry in the next turn.

## StreamingToolCallAssembler

Create a small assembler module in `neo-ai`, for example `crates/neo-ai/src/tool_assembly.rs`.

The assembler owns stream-local state only. It does not know tool schemas, permissions, or runtime execution.

### Inputs

The assembler should support these operations:

```rust
struct ToolCallChunk {
    index: Option<u64>,
    id: Option<String>,
    name: Option<String>,
    arguments_fragment: Option<String>,
}

enum ToolCallAssemblyEvent {
    Start { id: String, name: String },
    ArgsDelta { id: String, json_fragment: String },
    End { id: String, raw_arguments: String },
}
```

The implementation can expose whatever Rust API fits the existing parser style, but the behavior should match this contract.

### Keying

Use `index` as the primary key whenever present. This prevents `id` mutation from splitting a single tool call:

- First chunk: `index = 0`, `id = "functions.read:0"`.
- Later chunk: `index = 0`, `id = "chatcmpl-tool-a"`.
- Later chunk: `index = 0`, `id = "chatcmpl-tool-b"`.

These chunks are one logical call. The first non-empty id becomes the stable emitted id. Later id changes may update internal metadata only if needed for diagnostics; they must not change routing or split arguments.

If `index` is absent, use a single active fallback slot for Chat-style providers that omit index. If multiple unindexed tool calls appear in one stream, fail closed with a provider stream error because there is no deterministic way to route fragments without inventing unsafe behavior.

### Name and Header Ordering

Arguments may arrive before `function.name`.

The assembler should buffer argument fragments for an unnamed slot. It must not emit `ToolCallArgsDelta` before `ToolCallStart`; downstream transcript rendering expects a started tool call. When the name arrives, emit:

1. `ToolCallStart`.
2. One `ToolCallArgsDelta` containing the accumulated preview so far, if non-empty.

If the stream finishes a slot that never receives a name, emit a provider stream error for missing tool name. Running an unnamed tool is not meaningful.

### Argument Fragment Semantics

Some providers send true deltas. Others resend a growing prefix snapshot. Keep the current useful behavior from `merge_tool_argument_fragment`:

- Empty fragment: ignore.
- Fragment starts with accumulated string: emit only the new suffix and replace accumulated string with the full fragment.
- Accumulated string starts with fragment: treat as duplicate/older prefix and emit nothing.
- Otherwise append the fragment and emit it.

The accumulated string is preview state. The final `End` event carries the final raw string.

### Chat Completion Finalization

OpenAI Chat Completions has no `output_item.done` event. For Chat, the assembler finalizes all open slots when the parser sees terminal evidence:

- `finish_reason = "tool_calls"` or `"function_call"`.
- `[DONE]` after tool-call chunks.
- End of response body with a terminal finish reason already seen.

The finalized raw arguments are the assembled preview strings. Strict JSON parsing does not happen here.

### Responses Finalization

OpenAI Responses has explicit item completion. The Responses parser should use the same assembler, but treat the done item as authoritative:

- Argument delta events update preview.
- `output_item.done` / function-call done supplies final raw arguments when available.
- The final raw from the done item overrides preview accumulation for the `End` event.

This gives Chat and Responses one shared assembly policy while respecting Responses' stronger protocol event.

## Runtime Parse and Guarded Repair

Move final argument interpretation to `neo-agent-core`, after the runtime knows the tool name and has access to tool specs.

### Strict Parse First

For every `ToolCallEnd { raw_arguments }`, runtime first tries:

```rust
serde_json::from_str::<serde_json::Value>(&raw_arguments)
```

If that succeeds, keep current behavior: emit `AgentEvent::ToolCallFinished`, append the assistant tool call, and pass parsed arguments into permission/tool dispatch.

### Guarded Repair Second

If strict parse fails, try a guarded object-prefix repair. This is intentionally narrower than generic JSON repair.

Repair is allowed only when all of these are true:

- The raw string starts a JSON object.
- The parser can recover complete top-level key/value pairs without reading from an unclosed string, unclosed array, or unclosed object.
- Every required property from the tool's input schema is present in the recovered complete pairs.
- No required property's value came from an incomplete token.
- The incomplete tail is only an optional property or trailing syntax after all required fields.

Repair is rejected when:

- A required property is missing.
- A required property's string/value is incomplete.
- The top-level structure is not an object.
- The recovered object would change the semantic value of an already complete field.
- The tool is unknown, because schema requirements are unavailable.

Example recoverable raw arguments:

```json
{"command":"uname -a","description":
```

If `command` is required and `description` is optional, runtime may execute with:

```json
{"command":"uname -a"}
```

Example non-recoverable raw arguments:

```json
{"command":"uname -
```

The command itself is incomplete, so runtime must not execute.

### Invalid Arguments Become Tool Error

If both strict parse and guarded repair fail, runtime should emit a model-visible tool result error for the same call id and tool name.

The content should be concise and actionable:

```text
Tool arguments were invalid JSON: unexpected end of JSON input. Please retry the tool call with complete JSON arguments.
```

The error result should include structured details when available:

```json
{
  "kind": "invalid_tool_arguments",
  "parse_error": "...",
  "raw_arguments_excerpt": "...",
  "repair_attempted": true,
  "repair_reason": "required field command was incomplete"
}
```

The run should continue into the normal next model turn. It must not become `Run stopped` solely because tool arguments were malformed.

## Agent Message and Replay Contract

The implementation plan must inspect whether the existing `AgentToolCall { arguments: Value }` is sufficient once invalid raw arguments become tool-result errors.

Preferred minimal contract:

- Successful and repaired tool calls continue to store parsed `arguments: Value` in `AgentToolCall`.
- Invalid tool calls store enough assistant/tool-result context to replay the model-visible error in the next request without fabricating executable arguments.

If the current message model cannot represent an invalid assistant tool call plus matching tool result, update `AgentToolCall` canonically rather than adding a compatibility side channel. The likely shape is:

```rust
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
}
```

Executable paths then parse raw arguments into a separate prepared call:

```rust
pub struct PreparedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}
```

Do not keep two long-term execution paths where some code trusts parsed `arguments` and other code trusts `raw_arguments`. If raw storage is needed, raw becomes canonical storage and parsed JSON becomes a short-lived execution artifact.

## Error Handling

Provider stream errors remain appropriate for malformed SSE frames, invalid payload JSON, missing terminal markers, or ambiguous unindexed multi-tool routing.

Provider stream errors are not appropriate for invalid final tool arguments. Invalid final tool arguments are model output, not transport failure.

Runtime invalid-argument handling must happen before:

- Permission checks.
- Approval dialogs.
- Bash/session approval scope calculation.
- Tool execution.

This prevents a partial command string from reaching approval or execution.

## Non-Goals

- Do not add Xiaomi-only provider branches.
- Do not introduce a second OpenAI-compatible parser path.
- Do not execute arguments recovered from incomplete required values.
- Do not add hosted behavior, telemetry, or remote repair.
- Do not broaden this task into provider-specific thinking/reasoning compatibility knobs. Those can be a later OpenAI-compatible model capability design.

## Regression Tests

Keep tests narrow. Use exact test filters; do not run broad `cargo test` as completion evidence.

### `neo-ai` OpenAI Compatible Tests

Target: `crates/neo-ai/tests/openai_compatible_provider.rs`

Add fixture-style tests for:

1. Half JSON arguments do not become `AiError::Stream`.
   - Input raw: `{"command":"uname -a","description": `
   - Expected provider events include `ToolCallEnd { raw_arguments }`.
2. Standard fragmented arguments still produce one start, preview deltas, one end.
3. Empty argument deltas are ignored.
4. `finish_reason = "tool_calls"` finalizes accumulated arguments without `[DONE]` if terminal evidence is already present.
5. Id mutation with stable index merges into one tool call.
6. Arguments before name buffer until name, then emit start plus initial preview delta.
7. Two indexed tool calls interleave and finalize independently.
8. Multiple unindexed concurrent tool calls fail closed.

Example exact command shape:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_half_json_arguments_emit_raw_tool_call_end --exact --nocapture
```

### `neo-ai` Responses Tests

Target: an existing or new narrow OpenAI Responses test target.

Cover:

- Argument deltas update preview.
- Done item final raw arguments override preview.
- Malformed final raw arguments are emitted raw, not parsed in provider.

### `neo-agent-core` Runtime Tests

Target: `crates/neo-agent-core/tests/runtime_turn.rs` or a narrower runtime tool-argument test target if one already exists.

Cover:

1. Malformed raw arguments produce a `ToolResult` error and the next model turn continues.
2. Recoverable optional-tail partial JSON executes repaired arguments.
3. Incomplete required field does not execute and returns tool error.
4. Invalid raw arguments do not trigger permission approval or command execution.
5. Repaired arguments include enough transcript/session detail to make the behavior inspectable.

Example exact command shape:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_invalid_tool_arguments_return_model_visible_error --exact --nocapture
```

## Migration Notes

This is a breaking internal API change for `AiStreamEvent::ToolCallEnd`. Update all providers that emit tool calls:

- OpenAI Chat Completions: emit assembled raw string.
- OpenAI Responses: emit authoritative final raw string from done item where available.
- Anthropic: serialize accumulated `input_json_delta` raw string without parsing in provider.
- Google: if native function args are already a JSON value, serialize to a raw JSON string at the event boundary.
- Fake model/tests: update fixtures to use `raw_arguments` for stream end.

Outbound request serialization can keep using parsed tool-call arguments for successful calls until the implementation proves raw storage is required. If invalid-call replay requires raw storage, make raw `AgentToolCall` storage canonical in the same implementation, not as a compatibility layer.

## Implementation Boundaries for the Plan

The implementation plan should split work in this order:

1. Add focused failing tests for provider half JSON, id mutation, args-before-name, and interleaving.
2. Introduce `StreamingToolCallAssembler` and wire it into OpenAI Chat.
3. Change `AiStreamEvent::ToolCallEnd` to raw arguments and update all emitters/consumers.
4. Move strict parse to runtime and add invalid-argument tool-result feedback.
5. Add guarded repair using tool schema required fields.
6. Wire Responses through the shared assembler.
7. Run exact targeted tests only.

The plan should avoid parallel edits to the same files unless split by clear ownership: one slice for `neo-ai` assembler/provider tests, one for `neo-agent-core` runtime parse/error tests, and one follow-up slice for Responses after the shared event contract lands.

## Acceptance Criteria

- The Xiaomi-style half JSON sample does not produce provider/runtime stream error.
- Valid fragmented Chat tool calls still execute normally.
- Tool id mutation by stable `index` does not split arguments.
- Arguments before name are buffered and emitted after `ToolCallStart`.
- Multiple indexed tool calls interleave safely.
- Invalid or unrecoverable raw arguments become model-visible tool errors.
- Optional-tail guarded repair can execute only when all required fields are complete.
- No provider-specific Xiaomi branch is added.
- No duplicate long-term parsed/raw execution paths remain.
