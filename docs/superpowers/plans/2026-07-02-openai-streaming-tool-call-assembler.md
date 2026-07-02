# OpenAI Streaming Tool-Call Assembler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OpenAI-style streaming tool calls preserve raw final arguments, survive partial JSON, route chunks by stable index, and turn malformed final arguments into model-visible tool errors instead of provider stream failures.

**Architecture:** Add a provider-neutral `StreamingToolCallAssembler` in `neo-ai` and make streaming providers emit `ToolCallEnd { raw_arguments }`. Store raw tool arguments canonically in `AgentToolCall`; parse into `PreparedToolCall` only inside runtime dispatch before permission and execution. Invalid or unrecoverable raw arguments produce normal tool result errors so the next model turn can retry.

**Completion Status:** Implemented and verified on 2026-07-02. Final verification evidence is recorded in Task 12.

**Tech Stack:** Rust 2024, `neo-ai` providers and stream event types, `neo-agent-core` runtime/tool dispatch, `serde_json`, existing `cargo test --package ... --test ... -- <exact> --exact --nocapture` verification style.

---

## Execution Policy

- Work in `/Users/chenyuanhao/Workspace/neo`.
- Before implementation, run `icm recall-context "openai streaming tool-call assembler raw_arguments runtime guarded repair" --limit 5`.
- Do not modify vendored reference code under `docs/codex`, `docs/kimi-code`, or `docs/pi`.
- Do not add Xiaomi-specific branches. Xiaomi/MiMo is a regression fixture only.
- Do not keep dual long-term parsed/raw execution paths. Raw arguments become canonical storage; parsed JSON is a short-lived execution artifact.
- Do not run broad `cargo test`, package-wide `cargo nextest run`, or vague substring filters as evidence.
- Git mutation is forbidden unless the user explicitly authorizes that exact command in the execution session. That means no `git add`, `git commit`, `git stash`, `git reset`, `git checkout --`, `git clean`, `git rebase`, `git push`, or branch deletion. Use read-only `git status`, `git diff`, and `git log` freely.
- If a task says "authorization checkpoint", stop and ask the user before staging or committing. Subagents must never perform git mutations.

## Source Spec

Read this first:

- `docs/superpowers/specs/2026-07-02-openai-streaming-tool-call-assembler-design.md`

The spec has the acceptance criteria. This plan is the execution recipe.

## File Structure

- Create `crates/neo-ai/src/tool_assembly.rs`
  - Own stream-local tool-call assembly.
  - Key by `index` when present.
  - Buffer arguments before name.
  - Emit `Start`, `ArgsDelta`, and `End` events.
  - Never parse tool arguments as JSON.

- Modify `crates/neo-ai/src/lib.rs`
  - Add `pub mod tool_assembly;`.

- Modify `crates/neo-ai/src/types.rs`
  - Change `ToolCall.arguments` from `serde_json::Value` to `String`.
  - Change `AiStreamEvent::ToolCallEnd` from `arguments: serde_json::Value` to `raw_arguments: String`.

- Modify `crates/neo-ai/src/stream.rs`
  - Make `collect_tool_arguments` return raw strings or parse raw final arguments at the helper boundary.
  - Do not reassemble preview deltas if an `End` event exists.

- Modify `crates/neo-ai/src/providers/openai/compatible.rs`
  - Replace local `tool_args`, `tool_index_ids`, and `merge_tool_argument_fragment` with `StreamingToolCallAssembler`.
  - Finalize assembled calls on `finish_events`.

- Modify `crates/neo-ai/src/providers/openai/responses.rs`
  - Use the shared assembler for function call argument deltas.
  - Let `response.output_item.done` supply authoritative final raw arguments for function calls.

- Modify `crates/neo-ai/src/providers/anthropic.rs`, `crates/neo-ai/src/providers/google.rs`, and `crates/neo-ai/src/providers/fake.rs`
  - Emit raw argument strings at `ToolCallEnd`.
  - Do not parse accumulated tool arguments in providers.

- Modify `crates/neo-agent-core/src/messages.rs`
  - Store `AgentToolCall.raw_arguments: String` canonically.
  - Remove `AgentToolCall.arguments` as durable storage.

- Create `crates/neo-agent-core/src/runtime/tool_arguments.rs`
  - Parse raw arguments for execution.
  - Implement guarded object-prefix repair.
  - Build invalid-argument `ToolResult` errors.

- Modify `crates/neo-agent-core/src/runtime/mod.rs`
  - Add `mod tool_arguments;`.

- Modify `crates/neo-agent-core/src/runtime/stream_aggregator.rs`
  - Convert `ToolCallEnd { raw_arguments }` into `AgentToolCall { raw_arguments }`.
  - Emit `ToolCallFinished` with raw canonical storage.

- Modify `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
  - Prepare parsed arguments before permission checks, approval dialogs, scheduling, and execution.
  - Pass parsed arguments through `PreparedToolCall`.
  - Return invalid raw arguments as model-visible tool errors.

- Modify `crates/neo-agent-core/src/runtime/permission.rs`, `skill_dispatch.rs`, `tokens.rs`, `events.rs`, and any compile-failing consumers
  - Use parsed prepared arguments in execution-time code.
  - Use raw arguments for durable transcript/chat serialization.

- Modify tests:
  - `crates/neo-ai/tests/openai_compatible_provider.rs`
  - Existing OpenAI Responses provider tests, or add `crates/neo-ai/tests/openai_responses_provider.rs` if no narrow target exists.
  - `crates/neo-agent-core/tests/runtime_turn.rs`
  - Any fixture tests that construct `ToolCallEnd` or `AgentToolCall`.

## Dependency Waves

- Wave 1, sequential: `neo-ai` event contract and `StreamingToolCallAssembler`.
- Wave 2, mostly parallel after Wave 1 compiles: OpenAI Chat provider tests/wiring and non-OpenAI provider raw-event migration.
- Wave 3, sequential: `AgentToolCall` raw canonical storage and runtime prepared-argument parsing.
- Wave 4, sequential: guarded repair plus invalid-argument tool result continuation.
- Wave 5, sequential: Responses authoritative final item wiring.
- Wave 6, final verification: exact tests and diff review.

Do not dispatch two subagents that edit the same file in the same wave.

---

### Task 1: Add `neo-ai` Tool Assembly Unit Tests

**Files:**
- Create: `crates/neo-ai/src/tool_assembly.rs`
- Modify: `crates/neo-ai/src/lib.rs`

- [ ] **Step 1: Create the new module with tests first**

Create `crates/neo-ai/src/tool_assembly.rs` with this failing test scaffold and minimal public types. The tests intentionally refer to behavior the implementation does not yet satisfy.

```rust
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallChunk {
    pub index: Option<u64>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments_fragment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallAssemblyEvent {
    Start { id: String, name: String },
    ArgsDelta { id: String, json_fragment: String },
    End { id: String, raw_arguments: String },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolCallAssemblyError {
    #[error("multiple unindexed tool calls cannot be assembled deterministically")]
    AmbiguousUnindexedToolCalls,
    #[error("tool call {id} finished without a function name")]
    MissingName { id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ToolCallKey {
    Indexed(u64),
    Unindexed,
}

#[derive(Debug, Clone, Default)]
struct ToolCallSlot {
    stable_id: Option<String>,
    name: Option<String>,
    raw_arguments: String,
    started: bool,
    finished: bool,
}

#[derive(Debug, Default)]
pub struct StreamingToolCallAssembler {
    slots: BTreeMap<ToolCallKey, ToolCallSlot>,
    saw_unindexed: bool,
}

impl StreamingToolCallAssembler {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ingest(
        &mut self,
        chunk: ToolCallChunk,
    ) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let key = self.key_for(&chunk)?;
        let slot = self.slots.entry(key).or_default();
        Ok(update_slot(slot, chunk))
    }

    pub fn finish_all(&mut self) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let mut out = Vec::new();
        for slot in self.slots.values_mut() {
            if slot.finished {
                continue;
            }
            let Some(id) = slot.stable_id.clone() else {
                continue;
            };
            if slot.name.is_none() {
                return Err(ToolCallAssemblyError::MissingName { id });
            }
            slot.finished = true;
            out.push(ToolCallAssemblyEvent::End {
                id,
                raw_arguments: slot.raw_arguments.clone(),
            });
        }
        Ok(out)
    }

    pub fn finish_with_final_arguments(
        &mut self,
        index: Option<u64>,
        id: String,
        name: String,
        raw_arguments: String,
    ) -> Result<Vec<ToolCallAssemblyEvent>, ToolCallAssemblyError> {
        let key = index.map_or(ToolCallKey::Unindexed, ToolCallKey::Indexed);
        let slot = self.slots.entry(key).or_default();
        let mut out = Vec::new();
        if slot.stable_id.is_none() {
            slot.stable_id = Some(id.clone());
        }
        if slot.name.is_none() {
            slot.name = Some(name.clone());
        }
        if !slot.started {
            slot.started = true;
            out.push(ToolCallAssemblyEvent::Start {
                id: slot.stable_id.clone().unwrap_or(id.clone()),
                name,
            });
        }
        slot.raw_arguments = raw_arguments.clone();
        if !slot.finished {
            slot.finished = true;
            out.push(ToolCallAssemblyEvent::End {
                id: slot.stable_id.clone().unwrap_or(id),
                raw_arguments,
            });
        }
        Ok(out)
    }

    fn key_for(&mut self, chunk: &ToolCallChunk) -> Result<ToolCallKey, ToolCallAssemblyError> {
        if let Some(index) = chunk.index {
            return Ok(ToolCallKey::Indexed(index));
        }
        if self.saw_unindexed && chunk.id.is_some() {
            let existing = self.slots.get(&ToolCallKey::Unindexed);
            let same_id = existing
                .and_then(|slot| slot.stable_id.as_deref())
                .is_some_and(|id| chunk.id.as_deref() == Some(id));
            if !same_id {
                return Err(ToolCallAssemblyError::AmbiguousUnindexedToolCalls);
            }
        }
        self.saw_unindexed = true;
        Ok(ToolCallKey::Unindexed)
    }
}

fn update_slot(slot: &mut ToolCallSlot, chunk: ToolCallChunk) -> Vec<ToolCallAssemblyEvent> {
    let mut out = Vec::new();
    if slot.stable_id.is_none() {
        slot.stable_id = chunk.id.or_else(|| Some("tool-0".to_owned()));
    }
    if slot.name.is_none() {
        slot.name = chunk.name;
        if let (Some(id), Some(name)) = (slot.stable_id.clone(), slot.name.clone()) {
            slot.started = true;
            out.push(ToolCallAssemblyEvent::Start { id: id.clone(), name });
            if !slot.raw_arguments.is_empty() {
                out.push(ToolCallAssemblyEvent::ArgsDelta {
                    id,
                    json_fragment: slot.raw_arguments.clone(),
                });
            }
        }
    }
    if let Some(fragment) = chunk.arguments_fragment {
        if let Some(delta) = merge_argument_fragment(&mut slot.raw_arguments, &fragment) {
            if slot.started {
                out.push(ToolCallAssemblyEvent::ArgsDelta {
                    id: slot.stable_id.clone().unwrap_or_else(|| "tool-0".to_owned()),
                    json_fragment: delta,
                });
            }
        }
    }
    out
}

fn merge_argument_fragment(arguments: &mut String, fragment: &str) -> Option<String> {
    if fragment.is_empty() {
        return None;
    }
    if arguments.is_empty() {
        arguments.push_str(fragment);
        return Some(fragment.to_owned());
    }
    if fragment.starts_with(arguments.as_str()) {
        let delta = fragment[arguments.len()..].to_owned();
        arguments.clear();
        arguments.push_str(fragment);
        return (!delta.is_empty()).then_some(delta);
    }
    if arguments.starts_with(fragment) {
        return None;
    }
    arguments.push_str(fragment);
    Some(fragment.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(
        index: Option<u64>,
        id: Option<&str>,
        name: Option<&str>,
        args: Option<&str>,
    ) -> ToolCallChunk {
        ToolCallChunk {
            index,
            id: id.map(str::to_owned),
            name: name.map(str::to_owned),
            arguments_fragment: args.map(str::to_owned),
        }
    }

    #[test]
    fn stable_index_survives_id_mutation() {
        let mut assembler = StreamingToolCallAssembler::new();
        let first = assembler
            .ingest(chunk(Some(0), Some("functions.read:0"), Some("read"), Some("{\"path\":")))
            .unwrap();
        let second = assembler
            .ingest(chunk(Some(0), Some("chatcmpl-tool-a"), None, Some("\"Cargo.toml\"}")))
            .unwrap();
        let end = assembler.finish_all().unwrap();

        assert_eq!(
            [first, second, end].concat(),
            vec![
                ToolCallAssemblyEvent::Start {
                    id: "functions.read:0".to_owned(),
                    name: "read".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "functions.read:0".to_owned(),
                    json_fragment: "{\"path\":".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "functions.read:0".to_owned(),
                    json_fragment: "\"Cargo.toml\"}".to_owned(),
                },
                ToolCallAssemblyEvent::End {
                    id: "functions.read:0".to_owned(),
                    raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn arguments_before_name_are_buffered_until_start() {
        let mut assembler = StreamingToolCallAssembler::new();
        assert_eq!(
            assembler
                .ingest(chunk(Some(0), Some("call-1"), None, Some("{\"path\":\"Cargo")))
                .unwrap(),
            Vec::<ToolCallAssemblyEvent>::new()
        );
        let events = assembler
            .ingest(chunk(Some(0), None, Some("read"), Some(".toml\"}")))
            .unwrap();

        assert_eq!(
            events,
            vec![
                ToolCallAssemblyEvent::Start {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "call-1".to_owned(),
                    json_fragment: "{\"path\":\"Cargo".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "call-1".to_owned(),
                    json_fragment: ".toml\"}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn interleaved_indexed_calls_finish_independently() {
        let mut assembler = StreamingToolCallAssembler::new();
        let mut events = Vec::new();
        events.extend(
            assembler
                .ingest(chunk(Some(0), Some("call-a"), Some("read"), Some("{\"path\":")))
                .unwrap(),
        );
        events.extend(
            assembler
                .ingest(chunk(Some(1), Some("call-b"), Some("grep"), Some("{\"pattern\":")))
                .unwrap(),
        );
        events.extend(
            assembler
                .ingest(chunk(Some(0), None, None, Some("\"Cargo.toml\"}")))
                .unwrap(),
        );
        events.extend(
            assembler
                .ingest(chunk(Some(1), None, None, Some("\"neo\"}")))
                .unwrap(),
        );
        events.extend(assembler.finish_all().unwrap());

        assert!(events.contains(&ToolCallAssemblyEvent::End {
            id: "call-a".to_owned(),
            raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
        }));
        assert!(events.contains(&ToolCallAssemblyEvent::End {
            id: "call-b".to_owned(),
            raw_arguments: "{\"pattern\":\"neo\"}".to_owned(),
        }));
    }

    #[test]
    fn duplicate_prefix_fragments_emit_only_new_suffix() {
        let mut assembler = StreamingToolCallAssembler::new();
        let first = assembler
            .ingest(chunk(Some(0), Some("call-1"), Some("read"), Some("{\"path\":")))
            .unwrap();
        let second = assembler
            .ingest(chunk(Some(0), None, None, Some("{\"path\":\"Cargo.toml\"}")))
            .unwrap();

        assert_eq!(
            first.into_iter().chain(second).collect::<Vec<_>>(),
            vec![
                ToolCallAssemblyEvent::Start {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "call-1".to_owned(),
                    json_fragment: "{\"path\":".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "call-1".to_owned(),
                    json_fragment: "\"Cargo.toml\"}".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn final_arguments_override_preview_without_duplicate_delta() {
        let mut assembler = StreamingToolCallAssembler::new();
        let preview = assembler
            .ingest(chunk(
                Some(0),
                Some("call-1"),
                Some("read"),
                Some("{\"path\":\"Car"),
            ))
            .unwrap();
        let done = assembler
            .finish_with_final_arguments(
                Some(0),
                "call-1".to_owned(),
                "read".to_owned(),
                "{\"path\":\"Cargo.toml\"}".to_owned(),
            )
            .unwrap();

        assert_eq!(
            preview.into_iter().chain(done).collect::<Vec<_>>(),
            vec![
                ToolCallAssemblyEvent::Start {
                    id: "call-1".to_owned(),
                    name: "read".to_owned(),
                },
                ToolCallAssemblyEvent::ArgsDelta {
                    id: "call-1".to_owned(),
                    json_fragment: "{\"path\":\"Car".to_owned(),
                },
                ToolCallAssemblyEvent::End {
                    id: "call-1".to_owned(),
                    raw_arguments: "{\"path\":\"Cargo.toml\"}".to_owned(),
                },
            ]
        );
    }
}
```

- [ ] **Step 2: Export the module**

In `crates/neo-ai/src/lib.rs`, add the module near the other public modules:

```rust
pub mod tool_assembly;
```

- [ ] **Step 3: Run the assembler unit tests**

Run:

```bash
cargo test --package neo-ai --lib tool_assembly -- --nocapture
```

Expected: PASS. If this fails, fix only `crates/neo-ai/src/tool_assembly.rs` until these unit tests pass.

---

### Task 2: Change Stream Event Contract to Raw Arguments

**Files:**
- Modify: `crates/neo-ai/src/types.rs`
- Modify: `crates/neo-ai/src/stream.rs`
- Modify: compile-failing `AiStreamEvent::ToolCallEnd` constructors in tests and providers

- [ ] **Step 1: Change the event and tool-call data types**

In `crates/neo-ai/src/types.rs`, change `ToolCall` to raw canonical storage:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
}
```

In the same file, change `AiStreamEvent::ToolCallEnd` to:

```rust
    ToolCallEnd {
        id: String,
        raw_arguments: String,
    },
```

- [ ] **Step 2: Update outbound OpenAI Chat serialization**

In `crates/neo-ai/src/providers/openai/compatible.rs`, update `tool_call_body` to use raw arguments:

```rust
fn tool_call_body(tool_call: &crate::ToolCall) -> Value {
    json!({
        "id": tool_call.id,
        "type": "function",
        "function": {
            "name": tool_call.name,
            "arguments": tool_call.raw_arguments,
        }
    })
}
```

Update the unit test `message_body_serializes_assistant_tool_calls` in the same file so the fixture uses:

```rust
ToolCall {
    id: "call_1".to_owned(),
    name: "lookup".to_owned(),
    raw_arguments: r#"{"query":"neo"}"#.to_owned(),
}
```

and the assertion still expects the function argument string:

```rust
assert_eq!(
    body["tool_calls"][0]["function"]["arguments"],
    r#"{"query":"neo"}"#
);
```

- [ ] **Step 3: Update `collect_tool_arguments`**

Replace `crates/neo-ai/src/stream.rs` with:

```rust
use crate::{AiError, AiStreamEvent};

pub fn collect_tool_arguments(
    events: &[AiStreamEvent],
    tool_call_id: &str,
) -> Result<serde_json::Value, AiError> {
    let mut preview = String::new();
    let mut saw_delta = false;

    for event in events {
        match event {
            AiStreamEvent::ToolCallArgsDelta { id, json_fragment } if id == tool_call_id => {
                saw_delta = true;
                preview.push_str(json_fragment);
            }
            AiStreamEvent::ToolCallEnd { id, raw_arguments } if id == tool_call_id => {
                return parse_tool_arguments(raw_arguments);
            }
            _ => {}
        }
    }

    if !saw_delta {
        return Err(AiError::Stream {
            message: format!("missing tool arguments for tool call {tool_call_id}"),
        });
    }

    parse_tool_arguments(&preview)
}

fn parse_tool_arguments(raw: &str) -> Result<serde_json::Value, AiError> {
    serde_json::from_str(raw).map_err(|err| AiError::Stream {
        message: format!("invalid tool arguments: {err}"),
    })
}
```

- [ ] **Step 4: Run the narrow serialization test**

Run:

```bash
cargo test --package neo-ai --lib providers::openai::compatible::tests::message_body_serializes_assistant_tool_calls --exact --nocapture
```

Expected: PASS after updating the test fixture and `tool_call_body`.

- [ ] **Step 5: Use compile errors as a worklist**

Run:

```bash
cargo test --package neo-ai --lib tool_assembly -- --nocapture
```

Expected: it may fail to compile because other providers/tests still construct `ToolCall { arguments: ... }` or `ToolCallEnd { arguments: ... }`. Fix only the compile errors by replacing:

```rust
arguments: json!({ "path": "README.md" })
```

with:

```rust
raw_arguments: r#"{"path":"README.md"}"#.to_owned()
```

and replacing:

```rust
AiStreamEvent::ToolCallEnd {
    id,
    arguments,
}
```

with:

```rust
AiStreamEvent::ToolCallEnd {
    id,
    raw_arguments,
}
```

Do not parse raw arguments in providers while doing this migration.

---

### Task 3: Wire the Assembler Into OpenAI Chat Completions

**Files:**
- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`
- Test: `crates/neo-ai/tests/openai_compatible_provider.rs`

- [ ] **Step 1: Add the half-JSON provider regression test**

In `crates/neo-ai/tests/openai_compatible_provider.rs`, add this test near `openai_compatible_client_posts_typed_options_and_normalizes_sse_events`:

```rust
#[tokio::test]
async fn openai_compatible_half_json_arguments_emit_raw_tool_call_end() {
    let raw = r#"{"command":"uname -a","description": "#;
    let server = MockServer::start(vec![sse_response(&[json!({
        "id": "chatcmpl-half-json",
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call-1",
                    "function": {
                        "name": "Bash",
                        "arguments": raw
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    })])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions {
            retries: Some(0),
            ..RequestOptions::default()
        }))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: raw.to_owned(),
    }));
}
```

- [ ] **Step 2: Run the failing half-JSON test**

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_half_json_arguments_emit_raw_tool_call_end --exact --nocapture
```

Expected before implementation: FAIL or compile failure because Chat still parses final arguments in provider.

- [ ] **Step 3: Replace local Chat tool-call state with the assembler**

In `crates/neo-ai/src/providers/openai/compatible.rs`, update imports:

```rust
use crate::tool_assembly::{
    StreamingToolCallAssembler, ToolCallAssemblyEvent, ToolCallChunk,
};
```

Change `ParseState` fields from:

```rust
    tool_args: BTreeMap<String, String>,
    tool_index_ids: BTreeMap<u64, String>,
```

to:

```rust
    tool_calls: StreamingToolCallAssembler,
```

Change `Default for ParseState` accordingly:

```rust
            tool_calls: StreamingToolCallAssembler::new(),
```

Replace `ingest_tool_call` with:

```rust
    fn ingest_tool_call(&mut self, tool_call: &Value) {
        let function = tool_call.get("function").unwrap_or(&Value::Null);
        let chunk = ToolCallChunk {
            index: tool_call.get("index").and_then(Value::as_u64),
            id: tool_call.get("id").and_then(Value::as_str).map(str::to_owned),
            name: function.get("name").and_then(Value::as_str).map(str::to_owned),
            arguments_fragment: function
                .get("arguments")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        match self.tool_calls.ingest(chunk) {
            Ok(events) => self.push_tool_events(events),
            Err(err) => {
                self.last_stop_reason = StopReason::Error;
                self.saw_finish_reason = true;
                self.events.push(AiStreamEvent::Error {
                    message: err.to_string(),
                });
            }
        }
    }

    fn push_tool_events(&mut self, events: Vec<ToolCallAssemblyEvent>) {
        self.events.extend(events.into_iter().map(|event| match event {
            ToolCallAssemblyEvent::Start { id, name } => AiStreamEvent::ToolCallStart { id, name },
            ToolCallAssemblyEvent::ArgsDelta { id, json_fragment } => {
                AiStreamEvent::ToolCallArgsDelta { id, json_fragment }
            }
            ToolCallAssemblyEvent::End { id, raw_arguments } => {
                AiStreamEvent::ToolCallEnd { id, raw_arguments }
            }
        }));
    }
```

Replace the final tool parsing loop in `finish_events`:

```rust
        let tool_events = self
            .tool_calls
            .finish_all()
            .map_err(|err| ProviderError::Stream(err.to_string()))?;
        self.push_tool_events(tool_events);
```

Delete `merge_tool_argument_fragment` from `compatible.rs` after the compiler no longer needs it.

- [ ] **Step 4: Run the half-JSON test again**

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_half_json_arguments_emit_raw_tool_call_end --exact --nocapture
```

Expected: PASS.

- [ ] **Step 5: Add and run id-mutation and args-before-name tests**

Add these tests to `crates/neo-ai/tests/openai_compatible_provider.rs`:

```rust
#[tokio::test]
async fn openai_compatible_stable_index_survives_tool_id_mutation() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-id-mutation",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "functions.read:0",
                        "function": { "name": "read_file", "arguments": "{\"path\":" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "chatcmpl-tool-b",
                        "function": { "arguments": "\"Cargo.toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::ToolCallStart { .. }))
            .count(),
        1
    );
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "functions.read:0".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_buffers_arguments_until_tool_name_arrives() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-args-first",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "arguments": "{\"path\":\"Cargo" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "name": "read_file", "arguments": ".toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let start_pos = events
        .iter()
        .position(|event| matches!(event, AiStreamEvent::ToolCallStart { .. }))
        .expect("missing start");
    let delta_pos = events
        .iter()
        .position(|event| matches!(event, AiStreamEvent::ToolCallArgsDelta { .. }))
        .expect("missing delta");
    assert!(start_pos < delta_pos);
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}
```

Run each exactly:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_stable_index_survives_tool_id_mutation --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_buffers_arguments_until_tool_name_arrives --exact --nocapture
```

Expected: PASS.

---

### Task 4: Add OpenAI Chat Interleaving and Empty-Delta Coverage

**Files:**
- Modify: `crates/neo-ai/tests/openai_compatible_provider.rs`
- Modify when the new tests fail: `crates/neo-ai/src/tool_assembly.rs`, `crates/neo-ai/src/providers/openai/compatible.rs`

- [ ] **Step 1: Add interleaving and empty-delta tests**

Add:

```rust
#[tokio::test]
async fn openai_compatible_interleaves_two_indexed_tool_calls() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-interleave",
            "choices": [{
                "delta": {
                    "tool_calls": [
                        { "index": 0, "id": "call-a", "function": { "name": "read_file", "arguments": "{\"path\":" } },
                        { "index": 1, "id": "call-b", "function": { "name": "read_file", "arguments": "{\"path\":" } }
                    ]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [
                        { "index": 1, "function": { "arguments": "\"B.md\"}" } },
                        { "index": 0, "function": { "arguments": "\"A.md\"}" } }
                    ]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-a".to_owned(),
        raw_arguments: r#"{"path":"A.md"}"#.to_owned(),
    }));
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-b".to_owned(),
        raw_arguments: r#"{"path":"B.md"}"#.to_owned(),
    }));
}

#[tokio::test]
async fn openai_compatible_ignores_empty_tool_argument_deltas() {
    let server = MockServer::start(vec![sse_response(&[
        json!({
            "id": "chatcmpl-empty-delta",
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call-1",
                        "function": { "name": "read_file", "arguments": "" }
                    }]
                }
            }]
        }),
        json!({
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "function": { "arguments": "{\"path\":\"Cargo.toml\"}" }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }),
    ])]);
    let client = OpenAiCompatibleClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(RequestOptions::default()))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AiStreamEvent::ToolCallArgsDelta { json_fragment, .. } if json_fragment.is_empty()))
            .count(),
        0
    );
    assert!(events.contains(&AiStreamEvent::ToolCallEnd {
        id: "call-1".to_owned(),
        raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
    }));
}
```

- [ ] **Step 2: Run both exact tests**

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_interleaves_two_indexed_tool_calls --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_ignores_empty_tool_argument_deltas --exact --nocapture
```

Expected: PASS.

---

### Task 5: Migrate `AgentToolCall` to Raw Canonical Storage

**Files:**
- Modify: `crates/neo-agent-core/src/messages.rs`
- Modify compile-failing fixtures in `crates/neo-agent-core/tests/*.rs`
- Modify compile-failing runtime consumers later tasks will refine

- [ ] **Step 1: Change `AgentToolCall`**

In `crates/neo-agent-core/src/messages.rs`, replace `AgentToolCall` and conversions with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
}

impl From<ToolCall> for AgentToolCall {
    fn from(value: ToolCall) -> Self {
        Self {
            id: value.id,
            name: value.name,
            raw_arguments: value.raw_arguments,
        }
    }
}

impl From<AgentToolCall> for ToolCall {
    fn from(value: AgentToolCall) -> Self {
        Self {
            id: value.id,
            name: value.name,
            raw_arguments: value.raw_arguments,
        }
    }
}
```

- [ ] **Step 2: Update `stream_aggregator` finish call**

In `crates/neo-agent-core/src/runtime/stream_aggregator.rs`, change the match arm:

```rust
            AiStreamEvent::ToolCallEnd { id, raw_arguments } => {
                self.finish_tool_call(turn, id, raw_arguments, emitter);
            }
```

Change `finish_tool_call` signature and body:

```rust
    fn finish_tool_call(
        &mut self,
        turn: u32,
        id: String,
        raw_arguments: String,
        emitter: &mut EventEmitter,
    ) {
        let tool_call = AgentToolCall {
            name: self.tool_names.remove(&id).unwrap_or_default(),
            id,
            raw_arguments,
        };
        emitter.emit(AgentEvent::ToolCallFinished {
            turn,
            tool_call: tool_call.clone(),
        });
        self.tool_calls.push(tool_call);
    }
```

- [ ] **Step 3: Update tests and fixtures mechanically**

For each compile error that constructs an `AgentToolCall`, replace:

```rust
arguments: json!({ "text": "neo" }),
```

with compact raw JSON:

```rust
raw_arguments: r#"{"text":"neo"}"#.to_owned(),
```

For empty objects, use:

```rust
raw_arguments: "{}".to_owned(),
```

For dynamic values in tests, use:

```rust
raw_arguments: serde_json::to_string(&json!({ "path": file_path })).unwrap(),
```

Also update helper functions in `crates/neo-agent-core/tests/runtime_turn.rs` that accept parsed JSON and create stream events. For example, change:

```rust
fn bash_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
```

to:

```rust
fn bash_tool_turn(
    turn_index: usize,
    tool_id: &str,
    arguments: serde_json::Value,
) -> Vec<AiStreamEvent> {
    let raw_arguments = arguments.to_string();
```

and change the `ToolCallEnd` inside that helper from:

```rust
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            arguments,
        },
```

to:

```rust
        AiStreamEvent::ToolCallEnd {
            id: tool_id.to_owned(),
            raw_arguments,
        },
```

Apply the same pattern to `terminal_tool_turn` and any other local helper that receives `serde_json::Value` only to build a `ToolCallEnd`.

- [ ] **Step 4: Run a compile-oriented narrow test**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_records_tool_calls_and_sends_tool_specs_to_model --exact --nocapture
```

Expected at this point: likely compile failures in runtime code that still reads `tool_call.arguments`. Continue to Task 6 instead of broadening the test.

---

### Task 6: Add Runtime Tool Argument Parser and Prepared Calls

**Files:**
- Create: `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/runtime/permission.rs`
- Modify: `crates/neo-agent-core/src/events.rs`

- [ ] **Step 1: Add runtime argument parsing module**

Create `crates/neo-agent-core/src/runtime/tool_arguments.rs`:

```rust
use crate::{AgentToolCall, ToolResult, ToolSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedToolCall {
    pub id: String,
    pub name: String,
    pub raw_arguments: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolArgumentsOutcome {
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

pub fn prepare_tool_arguments(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> Result<PreparedToolCall, ToolResult> {
    match parse_tool_arguments(tool_call, tool_specs) {
        ToolArgumentsOutcome::Valid(arguments) | ToolArgumentsOutcome::Repaired { arguments, .. } => {
            Ok(PreparedToolCall {
                id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                raw_arguments: tool_call.raw_arguments.clone(),
                arguments,
            })
        }
        ToolArgumentsOutcome::Invalid {
            message,
            raw_excerpt,
        } => Err(ToolResult::error(message).with_details(serde_json::json!({
            "kind": "invalid_tool_arguments",
            "raw_arguments_excerpt": raw_excerpt,
            "repair_attempted": false
        }))),
    }
}

pub fn parse_tool_arguments(
    tool_call: &AgentToolCall,
    _tool_specs: &[ToolSpec],
) -> ToolArgumentsOutcome {
    match serde_json::from_str::<serde_json::Value>(&tool_call.raw_arguments) {
        Ok(arguments) => ToolArgumentsOutcome::Valid(arguments),
        Err(err) => ToolArgumentsOutcome::Invalid {
            message: format!(
                "Tool arguments were invalid JSON: {err}. Please retry the tool call with complete JSON arguments."
            ),
            raw_excerpt: raw_excerpt(&tool_call.raw_arguments),
        },
    }
}

fn raw_excerpt(raw: &str) -> String {
    const MAX: usize = 512;
    raw.chars().take(MAX).collect()
}
```

This is intentionally strict-only. Guarded repair lands in Task 8.

- [ ] **Step 2: Register the module**

In `crates/neo-agent-core/src/runtime/mod.rs`, add:

```rust
mod tool_arguments;
```

- [ ] **Step 3: Change `AgentEvent::ToolExecutionStarted`**

In `crates/neo-agent-core/src/events.rs`, keep execution-start arguments parsed because execution events describe what will run:

```rust
    ToolExecutionStarted {
        turn: u32,
        id: String,
        name: String,
        arguments: serde_json::Value,
    },
```

No schema change is needed here. Later steps must pass `prepared.arguments.clone()`, not raw storage.

- [ ] **Step 4: Thread prepared calls through tool dispatch**

In `crates/neo-agent-core/src/runtime/tool_dispatch.rs`, import:

```rust
use super::tool_arguments::{prepare_tool_arguments, PreparedToolCall};
```

Add this helper near `execute_tool_calls`:

```rust
fn prepare_tool_calls_for_execution(
    registry: &ToolRegistry,
    tool_calls: &[AgentToolCall],
) -> Vec<(AgentToolCall, Result<PreparedToolCall, ToolResult>)> {
    let specs = registry.specs();
    tool_calls
        .iter()
        .cloned()
        .map(|call| {
            let prepared = prepare_tool_arguments(&call, &specs);
            (call, prepared)
        })
        .collect()
}
```

Change `execute_tool_calls` so the first operation is:

```rust
    let prepared_calls = prepare_tool_calls_for_execution(registry.as_ref(), tool_calls);
```

Then pass `&prepared_calls` to sequential/parallel helpers instead of `tool_calls`. The helper signatures should become:

```rust
async fn execute_tool_calls_sequential(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStore>,
    turn: u32,
    prepared_calls: &[(AgentToolCall, Result<PreparedToolCall, ToolResult>)],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError>
```

and:

```rust
async fn execute_tool_calls_parallel(
    config: &AgentConfig,
    model: Arc<dyn ModelClient>,
    registry: Arc<ToolRegistry>,
    skills: Option<&SkillStore>,
    turn: u32,
    prepared_calls: &[(AgentToolCall, Result<PreparedToolCall, ToolResult>)],
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
    process_supervisor: &ProcessSupervisor,
) -> Result<Vec<(AgentToolCall, ToolResult)>, AgentRuntimeError>
```

- [ ] **Step 5: Ensure invalid arguments skip permission and execution**

In sequential execution, replace the loop head with this shape:

```rust
    for (tool_call, prepared_result) in prepared_calls {
        let prepared = match prepared_result {
            Ok(prepared) => prepared,
            Err(result) => {
                emitter.emit(AgentEvent::ToolExecutionFinished {
                    turn,
                    id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: result.clone(),
                });
                results.push((tool_call.clone(), result.clone()));
                continue;
            }
        };
        emitter.emit(AgentEvent::ToolExecutionStarted {
            turn,
            id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            arguments: prepared.arguments.clone(),
        });
        // Existing before_tool_result / prepare_and_run_tool flow continues here,
        // but pass `prepared` wherever parsed arguments are needed.
    }
```

In parallel execution, do the same before scheduling a future. Invalid calls must be pushed into `completed` immediately and must not call `permission_preparation_for_mode`, `before_tool_result`, or `registry.run`.

- [ ] **Step 6: Update permission functions to accept prepared arguments**

In `crates/neo-agent-core/src/runtime/permission.rs`, change execution-time functions that read `tool_call.arguments` so they receive parsed arguments explicitly. For example, update:

```rust
pub(super) fn ask_user_runs_in_background(tool_call: &AgentToolCall) -> bool {
    tool_call
        .arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
```

to:

```rust
pub(super) fn ask_user_runs_in_background(arguments: &serde_json::Value) -> bool {
    arguments
        .get("background")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}
```

Update callers in `tool_dispatch.rs` to pass `&prepared.arguments`.

Apply the same pattern to approval scope helpers such as `path_subject`, `resolve_bash_cwd`, `bash_approval_scope`, and write/edit path scope helpers: execution-time logic takes `&serde_json::Value`.

- [ ] **Step 7: Run the strict-invalid compile test**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_records_tool_calls_and_sends_tool_specs_to_model --exact --nocapture
```

Expected: PASS after all compile errors from `tool_call.arguments` are resolved for this runtime path.

---

### Task 7: Runtime Invalid Arguments Become Tool Results

**Files:**
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify when the new test fails: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify when the new test fails: `crates/neo-agent-core/src/runtime/tool_arguments.rs`

- [ ] **Step 1: Add invalid raw arguments continuation test**

In `crates/neo-agent-core/tests/runtime_turn.rs`, add this test near `runtime_executes_tool_call_and_continues_until_end_turn`. It uses existing `FakeHarness`, `EchoTool`, and `ToolRegistry` definitions from the same file.

```rust
#[tokio::test]
async fn runtime_invalid_tool_arguments_return_model_visible_error() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_1".to_owned(),
                name: "echo".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_1".to_owned(),
                raw_arguments: r#"{"text":"neo"#.to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "retrying".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model()).with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        tools,
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("call echo"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("invalid tool arguments should be reported to the model, not stop the run");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionFinished { result, .. }
                if result.is_error
                    && result.content.contains("Tool arguments were invalid JSON")
        )),
        "invalid arguments should be returned as a model-visible tool error"
    );
    assert!(
        !events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolExecutionStarted { name, .. } if name == "echo"
        )),
        "invalid raw arguments must not reach execution start"
    );
    assert_eq!(harness.requests().len(), 2);
    assert!(matches!(
        harness.requests()[1].messages.last(),
        Some(neo_ai::ChatMessage::ToolResult { tool_call_id, content, .. })
            if tool_call_id == "tool_1"
                && content.iter().any(|part| matches!(
                    part,
                    neo_ai::ContentPart::Text { text }
                        if text.contains("Tool arguments were invalid JSON")
                ))
    ));
}
```

- [ ] **Step 2: Run the invalid-arguments test**

Run the exact test name you added:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_invalid_tool_arguments_return_model_visible_error --exact --nocapture
```

Expected before final wiring: FAIL if invalid arguments still stop the run, trigger approval, or start execution. PASS once Task 6 wiring is complete.

- [ ] **Step 3: Ensure tool result messages are appended**

If the test fails because no next model turn sees the error, inspect `append_tool_result_messages` in `crates/neo-agent-core/src/runtime/turn_loop.rs`. Do not add a special side channel. The invalid-argument path must return `Vec<(AgentToolCall, ToolResult)>` from `execute_tool_calls`, and existing `append_tool_result_messages` should append the result like any other tool result.

The invalid call's `AgentToolCall` must keep:

```rust
AgentToolCall {
    id: "tool_1".to_owned(),
    name: "echo".to_owned(),
    raw_arguments: r#"{"text":"neo"#.to_owned(),
}
```

Expected: after this, the test passes without broad runtime changes.

---

### Task 8: Add Guarded Object-Prefix Repair

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Add unit tests inside `tool_arguments.rs`**

Append this test module to `crates/neo-agent-core/src/runtime/tool_arguments.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolSpec;
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
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            raw_arguments: raw_arguments.to_owned(),
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
    fn rejects_incomplete_required_field() {
        let outcome = parse_tool_arguments(&call(r#"{"command":"uname -"#), &[bash_spec()]);
        assert!(matches!(outcome, ToolArgumentsOutcome::Invalid { .. }));
    }

    #[test]
    fn rejects_unknown_tool_partial_json() {
        let outcome = parse_tool_arguments(
            &AgentToolCall {
                id: "call-1".to_owned(),
                name: "Unknown".to_owned(),
                raw_arguments: r#"{"command":"uname -a","description": "#.to_owned(),
            },
            &[bash_spec()],
        );
        assert!(matches!(outcome, ToolArgumentsOutcome::Invalid { .. }));
    }
}
```

- [ ] **Step 2: Run the failing repair unit test**

Run:

```bash
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::repairs_optional_tail_when_required_field_is_complete --exact --nocapture
```

Expected: FAIL, because Task 6 only has strict parse.

- [ ] **Step 3: Implement guarded repair**

In `crates/neo-agent-core/src/runtime/tool_arguments.rs`, replace `parse_tool_arguments` with:

```rust
pub fn parse_tool_arguments(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> ToolArgumentsOutcome {
    match serde_json::from_str::<serde_json::Value>(&tool_call.raw_arguments) {
        Ok(arguments) => return ToolArgumentsOutcome::Valid(arguments),
        Err(strict_err) => {
            if let Some(repaired) = repair_partial_object(tool_call, tool_specs) {
                return ToolArgumentsOutcome::Repaired {
                    arguments: repaired,
                    warning: "recovered complete required fields from partial JSON object".to_owned(),
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
```

Add these helpers:

```rust
fn repair_partial_object(
    tool_call: &AgentToolCall,
    tool_specs: &[ToolSpec],
) -> Option<serde_json::Value> {
    let required = required_fields(tool_call, tool_specs)?;
    let object = complete_top_level_pairs(&tool_call.raw_arguments)?;
    if required
        .iter()
        .all(|field| object.get(field).is_some())
    {
        Some(serde_json::Value::Object(object))
    } else {
        None
    }
}

fn required_fields(tool_call: &AgentToolCall, tool_specs: &[ToolSpec]) -> Option<Vec<String>> {
    let spec = tool_specs.iter().find(|spec| spec.name == tool_call.name)?;
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

fn complete_value_end(raw: &str, start: usize) -> Option<usize> {
    let mut in_string = false;
    let mut escaped = false;
    let mut depth = 0_i32;
    let mut saw_value = false;
    for (offset, ch) in raw[start..].char_indices() {
        let pos = start + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                saw_value = true;
            }
            '{' | '[' => {
                depth += 1;
                saw_value = true;
            }
            '}' | ']' => {
                if depth == 0 {
                    return saw_value.then_some(pos);
                }
                depth -= 1;
            }
            ',' if depth == 0 => return saw_value.then_some(pos),
            c if c.is_ascii_whitespace() => {}
            _ => saw_value = true,
        }
    }
    None
}
```

- [ ] **Step 4: Run repair tests**

Run:

```bash
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::repairs_optional_tail_when_required_field_is_complete --exact --nocapture
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::rejects_incomplete_required_field --exact --nocapture
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::rejects_unknown_tool_partial_json --exact --nocapture
```

Expected: PASS.

---

### Task 9: Apply Prepared Arguments to Skill, Permission, Tokens, and Chat Replay

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/skill_dispatch.rs`
- Modify: `crates/neo-agent-core/src/runtime/tokens.rs`
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
- Modify: `crates/neo-agent-core/src/runtime/events.rs`
- Modify: compile-failing call sites

- [ ] **Step 1: Skill dispatch takes prepared parsed arguments**

In `crates/neo-agent-core/src/runtime/skill_dispatch.rs`, change `execute_invoke_skill` so it accepts parsed arguments:

```rust
pub(super) fn execute_invoke_skill(
    skills: Option<&SkillStore>,
    tool_call: &AgentToolCall,
    arguments: &serde_json::Value,
) -> ToolResult {
    let request = match skill_tool_request(arguments) {
        Ok(request) => request,
        Err(err) => return ToolResult::error(err),
    };
    // Keep the existing body after this point, replacing reads of
    // `tool_call.arguments` with `arguments`.
}
```

Update `run_tool_with_cancel` to call:

```rust
return execute_invoke_skill(skills, tool_call, &prepared.arguments);
```

- [ ] **Step 2: Token estimation uses raw arguments**

In `crates/neo-agent-core/src/runtime/tokens.rs`, replace argument string length calls:

```rust
tool_call.arguments.to_string().len()
```

with:

```rust
tool_call.raw_arguments.len()
```

- [ ] **Step 3: Chat request serialization uses raw arguments**

In `crates/neo-agent-core/src/runtime/chat_request.rs`, when converting `AgentToolCall` to `neo_ai::ToolCall`, use:

```rust
neo_ai::ToolCall {
    id: tool_call.id.clone(),
    name: tool_call.name.clone(),
    raw_arguments: tool_call.raw_arguments.clone(),
}
```

- [ ] **Step 4: Event/session serialization compiles with raw storage**

In `crates/neo-agent-core/src/runtime/events.rs`, replace reads of `tool_call.arguments` with `tool_call.raw_arguments` where serializing assistant tool calls or transcript details. If a JSON value is required for an execution event, use the parsed value already carried by `ToolExecutionStarted`.

- [ ] **Step 5: Run targeted runtime tests that cover replay and tool records**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- runtime_records_tool_calls_and_sends_tool_specs_to_model --exact --nocapture
cargo test --package neo-agent-core --test session_jsonl -- session_jsonl_records_tool_calls --exact --nocapture
```

Expected: PASS. If `session_jsonl_records_tool_calls` does not exist under that exact name, run the exact nearest test discovered with:

```bash
rg -n "records_tool_calls|ToolCallFinished|tool_calls" crates/neo-agent-core/tests/session_jsonl.rs
```

Then run only that exact test.

---

### Task 10: Migrate Anthropic, Google, and Fake Providers

**Files:**
- Modify: `crates/neo-ai/src/providers/anthropic.rs`
- Modify: `crates/neo-ai/src/providers/google.rs`
- Modify: `crates/neo-ai/src/providers/fake.rs`
- Modify: provider tests that construct `ToolCallEnd`

- [ ] **Step 1: Anthropic emits raw arguments**

In `crates/neo-ai/src/providers/anthropic.rs`, replace the parse loop in `finish_events`:

```rust
        for (id, arguments) in &self.tool_args {
            self.events.push(AiStreamEvent::ToolCallEnd {
                id: id.clone(),
                raw_arguments: arguments.clone(),
            });
        }
```

Do not call `serde_json::from_str` here.

- [ ] **Step 2: Google emits raw JSON strings**

In `crates/neo-ai/src/providers/google.rs`, where it currently has a parsed `serde_json::Value` for function args, emit:

```rust
self.events.push(AiStreamEvent::ToolCallEnd {
    id: id.clone(),
    raw_arguments: arguments.to_string(),
});
```

If Google already stores a raw string in local state, clone that raw string instead.

- [ ] **Step 3: Fake provider emits raw arguments**

In `crates/neo-ai/src/providers/fake.rs`, update test event helpers and fake stream fixtures to construct:

```rust
AiStreamEvent::ToolCallEnd {
    id: "call-1".to_owned(),
    raw_arguments: r#"{"path":"README.md"}"#.to_owned(),
}
```

- [ ] **Step 4: Run one narrow provider test per changed provider**

Use `rg` to find exact tests:

```bash
rg -n "ToolCallEnd|tool arguments|function_call|google|anthropic" crates/neo-ai/tests crates/neo-ai/src/providers -g '*.rs'
```

Run exact tests only. Example command shapes:

```bash
cargo test --package neo-ai --lib providers::anthropic::tests::<exact_test_name> --exact --nocapture
cargo test --package neo-ai --lib providers::google::tests::<exact_test_name> --exact --nocapture
```

Expected: PASS for the exact tests selected.

---

### Task 11: Wire OpenAI Responses Through the Shared Assembler

**Files:**
- Modify: `crates/neo-ai/src/providers/openai/responses.rs`
- Test: existing OpenAI Responses test target, or create `crates/neo-ai/tests/openai_responses_provider.rs`

- [ ] **Step 1: Add Responses tests**

If `crates/neo-ai/tests/openai_responses_provider.rs` does not exist, create it using the same `MockServer` shape from `openai_compatible_provider.rs`. Add a test that sends:

```json
{ "type": "response.created", "response": { "id": "resp-1" } }
{ "type": "response.output_item.added", "item": { "id": "item-1", "type": "function_call", "call_id": "call-1", "name": "read_file" } }
{ "type": "response.function_call_arguments.delta", "item_id": "item-1", "delta": "{\"path\":\"Car" }
{ "type": "response.output_item.done", "item": { "id": "item-1", "type": "function_call", "call_id": "call-1", "name": "read_file", "arguments": "{\"path\":\"Cargo.toml\"}" } }
{ "type": "response.completed", "response": { "usage": { "input_tokens": 1, "output_tokens": 1 } } }
```

Assert:

```rust
assert!(events.contains(&AiStreamEvent::ToolCallEnd {
    id: "call-1".to_owned(),
    raw_arguments: r#"{"path":"Cargo.toml"}"#.to_owned(),
}));
```

- [ ] **Step 2: Run the failing Responses test**

Run the exact test name:

```bash
cargo test --package neo-ai --test openai_responses_provider -- openai_responses_output_item_done_overrides_argument_preview --exact --nocapture
```

Expected before wiring: FAIL or compile failure if Responses still parses final arguments.

- [ ] **Step 3: Add assembler fields to Responses `ParseState`**

In `crates/neo-ai/src/providers/openai/responses.rs`, import:

```rust
use crate::tool_assembly::{
    StreamingToolCallAssembler, ToolCallAssemblyEvent, ToolCallChunk,
};
```

Change `ParseState` tool fields from:

```rust
    tool_args: BTreeMap<String, String>,
    item_call_ids: BTreeMap<String, String>,
```

to:

```rust
    tool_calls: StreamingToolCallAssembler,
    item_call_ids: BTreeMap<String, String>,
    item_names: BTreeMap<String, String>,
    item_indexes: BTreeMap<String, u64>,
    next_tool_index: u64,
```

Initialize both maps, `next_tool_index: 0`, and the assembler in `Default`.

Add this helper on `ParseState`:

```rust
    fn tool_index_for_item(&mut self, item_id: &str) -> u64 {
        if let Some(index) = self.item_indexes.get(item_id) {
            return *index;
        }
        let index = self.next_tool_index;
        self.next_tool_index += 1;
        self.item_indexes.insert(item_id.to_owned(), index);
        index
    }
```

- [ ] **Step 4: Convert Responses events into assembler chunks**

In `ingest_item_added`, after extracting `item_id`, `call_id`, and `name`, store:

```rust
self.item_call_ids.insert(item_id.clone(), call_id.clone());
let index = self.tool_index_for_item(&item_id);
if let Some(name) = item.get("name").and_then(Value::as_str) {
    self.item_names.insert(item_id.clone(), name.to_owned());
    let events = self
        .tool_calls
        .ingest(ToolCallChunk {
            index: Some(index),
            id: Some(call_id),
            name: Some(name.to_owned()),
            arguments_fragment: None,
        })
        .map_err(|err| ProviderError::Stream(err.to_string()));
    match events {
        Ok(events) => self.push_tool_events(events),
        Err(err) => self.events.push(AiStreamEvent::Error {
            message: err.to_string(),
        }),
    }
}
```

Add the same `push_tool_events` helper shape used in `compatible.rs`.

In `ingest_tool_delta`, call:

```rust
let index = self.tool_index_for_item(item_id);
let events = self
    .tool_calls
    .ingest(ToolCallChunk {
        index: Some(index),
        id: Some(id),
        name: self.item_names.get(item_id).cloned(),
        arguments_fragment: value.get("delta").and_then(Value::as_str).map(str::to_owned),
    })
    .map_err(|err| ProviderError::Stream(err.to_string()));
match events {
    Ok(events) => self.push_tool_events(events),
    Err(err) => self.events.push(AiStreamEvent::Error {
        message: err.to_string(),
    }),
}
```

- [ ] **Step 5: Treat function-call `output_item.done` as authoritative final raw**

Update `ingest_output_item_done` to handle function calls before the existing reasoning branch:

```rust
        if item.get("type").and_then(Value::as_str) == Some("function_call") {
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("function-call");
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .or_else(|| self.item_call_ids.get(item_id).map(String::as_str))
                .unwrap_or(item_id)
                .to_owned();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| self.item_names.get(item_id).map(String::as_str))
                .unwrap_or("function_call")
                .to_owned();
            let index = self.tool_index_for_item(item_id);
            let raw_arguments = item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let events = self
                .tool_calls
                .finish_with_final_arguments(Some(index), call_id.clone(), name, raw_arguments)
                .map_err(|err| ProviderError::Stream(err.to_string()));
            match events {
                Ok(events) => self.push_tool_events(events),
                Err(err) => self.events.push(AiStreamEvent::Error {
                    message: err.to_string(),
                }),
            }
            return;
        }
```

- [ ] **Step 6: Finish Responses without parsing**

In `finish_events`, replace the tool parse loop with:

```rust
        let tool_events = self
            .tool_calls
            .finish_all()
            .map_err(|err| ProviderError::Stream(err.to_string()))?;
        self.push_tool_events(tool_events);
```

- [ ] **Step 7: Run the Responses exact test**

Run:

```bash
cargo test --package neo-ai --test openai_responses_provider -- openai_responses_output_item_done_overrides_argument_preview --exact --nocapture
```

Expected: PASS.

---

### Task 12: Final Focused Verification and Cleanup

**Files:**
- Review all files touched in previous tasks.

- [x] **Step 1: Search for obsolete parsed provider arguments**

Run:

```bash
rg -n "invalid tool arguments|serde_json::from_str\\(arguments\\)|ToolCallEnd \\{[^\\n]*arguments|\\.arguments\\b" crates/neo-ai/src crates/neo-agent-core/src crates/neo-ai/tests crates/neo-agent-core/tests -g '*.rs'
```

Expected:

- No provider `finish_events` parses accumulated tool arguments.
- No `AiStreamEvent::ToolCallEnd { arguments: ... }` remains.
- Remaining `.arguments` hits in runtime refer to `PreparedToolCall.arguments`, `ToolExecutionStarted.arguments`, schema fields, or local parsed values.

- [x] **Step 2: Run exact provider regressions**

Run:

```bash
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_half_json_arguments_emit_raw_tool_call_end --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_stable_index_survives_tool_id_mutation --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_buffers_arguments_until_tool_name_arrives --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_interleaves_two_indexed_tool_calls --exact --nocapture
cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_ignores_empty_tool_argument_deltas --exact --nocapture
```

Expected: PASS.

- [x] **Step 3: Run exact runtime regressions**

Run:

```bash
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::repairs_optional_tail_when_required_field_is_complete --exact --nocapture
cargo test --package neo-agent-core --lib runtime::tool_arguments::tests::rejects_incomplete_required_field --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_invalid_tool_arguments_return_model_visible_error --exact --nocapture
cargo test --package neo-agent-core --test runtime_turn -- runtime_records_tool_calls_and_sends_tool_specs_to_model --exact --nocapture
```

Expected: PASS.

- [x] **Step 4: Run `git diff --check`**

Run:

```bash
git diff --check
```

Expected: no output.

- [x] **Step 5: Review diff for forbidden patterns**

Run:

```bash
git diff -- crates/neo-ai/src crates/neo-ai/tests crates/neo-agent-core/src crates/neo-agent-core/tests
```

Expected:

- No Xiaomi-specific `if provider == "xiaomi"` style branches.
- No provider-layer parse failure for final tool arguments.
- No duplicate parsed/raw execution path.
- Invalid raw arguments return `ToolResult::error`.
- Permission checks and approval scopes use parsed prepared arguments only.

- [x] **Step 6: Authorization checkpoint for git mutation**

Stop here and report the exact tests run and their results. Ask the user whether to stage and commit. Do not run `git add` or `git commit` unless the user explicitly authorizes those exact commands in the current execution session.

Suggested commit message if authorized:

```bash
git add crates/neo-ai/src crates/neo-ai/tests crates/neo-agent-core/src crates/neo-agent-core/tests docs/superpowers/specs/2026-07-02-openai-streaming-tool-call-assembler-design.md docs/superpowers/plans/2026-07-02-openai-streaming-tool-call-assembler.md
git commit -m "fix(ai): preserve raw streaming tool arguments"
```

Completion evidence from 2026-07-02:

- `rg -n "invalid tool arguments|serde_json::from_str\\(arguments\\)|ToolCallEnd \\{[^\\n]*arguments|\\.arguments\\b" crates/neo-ai/src crates/neo-agent-core/src crates/neo-ai/tests crates/neo-agent-core/tests -g '*.rs'` showed only raw event, prepared runtime argument, schema/skill, and assertion hits.
- `cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_half_json_arguments_emit_raw_tool_call_end --exact --nocapture` passed.
- `cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_stable_index_survives_tool_id_mutation --exact --nocapture` passed.
- `cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_buffers_arguments_until_tool_name_arrives --exact --nocapture` passed.
- `cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_interleaves_two_indexed_tool_calls --exact --nocapture` passed.
- `cargo test --package neo-ai --test openai_compatible_provider -- openai_compatible_ignores_empty_tool_argument_deltas --exact --nocapture` passed.
- `cargo test --package neo-ai --test real_provider_adapters -- openai_compatible_client_finishes_tool_call_on_tool_calls_finish_reason_without_done --exact --nocapture` passed.
- `cargo test --package neo-ai --test real_provider_adapters -- openai_responses_output_item_done_overrides_argument_preview --exact --nocapture` passed.
- `cargo test --package neo-agent-core --test runtime_turn -- runtime_invalid_tool_arguments_return_model_visible_error --exact --nocapture` passed.
- `cargo test --package neo-agent-core --test runtime_turn -- runtime_records_tool_calls_and_sends_tool_specs_to_model --exact --nocapture` passed.
- `cargo test --package neo-agent-core --lib runtime::tool_arguments -- --nocapture` passed with 5 tests.
- `cargo test --package neo-ai --lib tool_assembly -- --nocapture` passed with 9 tests.
- `cargo test --package neo-ai --lib providers::anthropic::tests::assistant_replay_rejects_invalid_raw_tool_arguments -- --exact --nocapture` passed.
- `cargo test --package neo-ai --lib providers::google::tests::assistant_replay_rejects_invalid_raw_tool_arguments -- --exact --nocapture` passed.
- `cargo fmt --package neo-ai --package neo-agent-core --check` passed.
- `git diff --check` passed.

---

## Handoff Prompts

Use these prompts if handing tasks to other AI workers. Include the execution policy from this plan every time.

### Wave 1 Prompt: `neo-ai` Assembler and Chat Provider

```text
在 /Users/chenyuanhao/Workspace/neo 执行 docs/superpowers/plans/2026-07-02-openai-streaming-tool-call-assembler.md 的 Task 1-4。

必须先运行:
icm recall-context "openai streaming tool-call assembler raw_arguments runtime guarded repair" --limit 5

范围:
- 只改 neo-ai 的 tool_assembly、types、stream、openai compatible provider 和 openai_compatible_provider tests。
- 不改 neo-agent-core runtime。
- 不添加 Xiaomi-specific 分支。
- provider 层不要 parse final tool arguments。
- 不执行任何 git mutation。

完成后只汇报:
- 改了哪些文件。
- 每个 exact cargo test 命令和 PASS/FAIL。
- 是否还有 compile errors 阻塞后续 Task。
```

### Wave 2 Prompt: Runtime Raw Storage and Invalid Argument Errors

```text
在 /Users/chenyuanhao/Workspace/neo 执行 docs/superpowers/plans/2026-07-02-openai-streaming-tool-call-assembler.md 的 Task 5-9。

前提:
- Wave 1 已完成 AiStreamEvent::ToolCallEnd { raw_arguments }。

范围:
- AgentToolCall raw_arguments 成为 canonical storage。
- parsed serde_json::Value 只能存在于 PreparedToolCall / execution-time event。
- invalid raw arguments 必须返回 ToolResult::error，不能触发 permission 或 execution。
- guarded repair 只能恢复 required fields 都完整的 object-prefix。
- 不执行任何 git mutation。

完成后只汇报:
- runtime 里还剩哪些 .arguments 是合法 prepared/event/schema 用法。
- 每个 exact cargo test 命令和 PASS/FAIL。
- invalid raw arguments 是否进入下一轮 model-visible tool result。
```

### Wave 3 Prompt: Responses and Other Providers

```text
在 /Users/chenyuanhao/Workspace/neo 执行 docs/superpowers/plans/2026-07-02-openai-streaming-tool-call-assembler.md 的 Task 10-12。

前提:
- Wave 1/2 已完成 raw ToolCallEnd contract 和 runtime prepared parsing。

范围:
- Anthropic/Google/Fake provider emit raw_arguments。
- OpenAI Responses 使用 shared assembler。
- output_item.done 的 function-call final arguments 是 authoritative raw。
- 搜索并删除 provider finish_events 里的 final arguments serde_json::from_str。
- 不执行任何 git mutation。

完成后只汇报:
- exact tests 和结果。
- rg 检查输出里是否仍有危险命中。
- git diff --check 结果。
```
