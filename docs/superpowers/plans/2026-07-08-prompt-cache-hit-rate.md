# Neo Prompt Cache Hit Rate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Neo's Anthropic-compatible prompt-cache behavior measurable and more stable by fixing usage parsing, improving cache-control anchors, and surfacing conservative cache hit rate in the TUI.

**Architecture:** Keep the provider/public runtime API narrow. Implement Anthropic-specific usage accumulation and cache-anchor planning inside `anthropic.rs`, add provider golden tests for real SSE/request shapes, and add TUI/runtime guardrails that make cache behavior observable without changing durable session history or compaction policy.

**Tech Stack:** Rust 2024, `neo-ai`, `neo-agent-core`, `neo-tui`, `serde_json`, existing provider mock server tests, exact `cargo test` commands.

---

## Policy Notes

- This plan is written for the shared Neo worktree. Do not run git mutation commands unless the user explicitly authorizes that specific command.
- Do not use broad `cargo test` or package-wide `cargo nextest run` as evidence. Use the exact commands listed under each task.
- Do not modify Google context caching or full compaction behavior in this plan.
- Do not add compatibility branches or duplicate old Anthropic cache-control paths. Replace the old injector.

## File Map

- Modify: `crates/neo-ai/src/providers/anthropic.rs`
  - Add Anthropic usage accumulator.
  - Replace last-message-only cache-control injection with a two-message-anchor planner.
  - Honor existing `CacheRetention` if the implementation chooses to include that small follow-up in Task 4.
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs`
  - Add realistic Anthropic usage parser tests.
  - Add request-body cache-control anchor tests.
- Modify: `crates/neo-tui/src/shell/context.rs`
  - Add conservative cache hit-rate formatting.
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
  - Update footer token usage tests.
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
  - Document `ContextAppendTransform` cache-prefix constraints.
- Modify: `crates/neo-agent-core/src/runtime/chat_request.rs`
  - Add or reuse test helpers for prefix-stability tests if needed.
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
  - Add runtime prefix-stability tests.
- Optional follow-up modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
  - Move subagent role profile out of system prompt.
- Optional follow-up modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
  - Cover subagent system prompt stability.

## Task 1: Parse Anthropic Usage From Real SSE Shape

**Files:**
- Modify: `crates/neo-ai/src/providers/anthropic.rs:510-705`
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs:1529-1612`

- [ ] **Step 1: Add a failing realistic usage test**

In `crates/neo-ai/tests/real_provider_adapters.rs`, update `anthropic_messages_client_posts_messages_payload_and_streams_events` so `message_start.message.usage` carries input/cache tokens and `message_delta.usage` carries output tokens.

Use this event shape in the existing test:

```rust
json!({
    "type": "message_start",
    "message": {
        "id": "msg-1",
        "usage": {
            "input_tokens": 11,
            "cache_read_input_tokens": 8,
            "cache_creation_input_tokens": 2
        }
    }
}),
```

Replace the current `message_delta` usage object with:

```rust
json!({
    "type": "message_delta",
    "delta": { "stop_reason": "tool_use" },
    "usage": { "output_tokens": 3 }
}),
```

Keep the expected final event:

```rust
AiStreamEvent::MessageEnd {
    stop_reason: StopReason::ToolUse,
    usage: Some(neo_ai::TokenUsage {
        input_tokens: 11,
        output_tokens: 3,
        input_cache_read_tokens: 8,
        input_cache_write_tokens: 2,
    })
}
```

- [ ] **Step 2: Run the exact test and confirm it fails**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_posts_messages_payload_and_streams_events --exact --nocapture
```

Expected: FAIL because current parser ignores `message_start.message.usage` and `token_usage_from` cannot build full usage from delta-only `output_tokens`.

- [ ] **Step 3: Implement a local Anthropic usage accumulator**

In `crates/neo-ai/src/providers/anthropic.rs`, replace `ParseState.usage: Option<TokenUsage>` with an accumulator.

Add near `ThinkingBlock`:

```rust
#[derive(Default)]
struct AnthropicUsageAccumulator {
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    input_cache_read_tokens: u32,
    input_cache_write_tokens: u32,
}

impl AnthropicUsageAccumulator {
    fn merge_start_usage(&mut self, usage: &Value) {
        if let Some(input) = token_u32(usage.get("input_tokens")) {
            self.input_tokens = Some(input);
        }
        self.input_cache_read_tokens = token_u32(usage.get("cache_read_input_tokens"))
            .or_else(|| token_u32(usage.get("input_cache_read_tokens")))
            .unwrap_or(self.input_cache_read_tokens);
        self.input_cache_write_tokens = token_u32(usage.get("cache_creation_input_tokens"))
            .or_else(|| token_u32(usage.get("input_cache_write_tokens")))
            .unwrap_or(self.input_cache_write_tokens);
    }

    fn merge_delta_usage(&mut self, usage: &Value) {
        if let Some(output) = token_u32(usage.get("output_tokens")) {
            self.output_tokens = Some(output);
        }
    }

    fn finish(&self) -> Option<TokenUsage> {
        let input_tokens = self.input_tokens?;
        Some(TokenUsage {
            input_tokens,
            output_tokens: self.output_tokens.unwrap_or(0),
            input_cache_read_tokens: self.input_cache_read_tokens,
            input_cache_write_tokens: self.input_cache_write_tokens,
        })
    }
}

fn token_u32(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()
}
```

Change `ParseState`:

```rust
usage: AnthropicUsageAccumulator,
```

Initialize it with:

```rust
usage: AnthropicUsageAccumulator::default(),
```

In the `message_start` branch, read `message` once and merge usage:

```rust
Some("message_start") => {
    let message = value.get("message").unwrap_or(&Value::Null);
    let id = message
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_owned();
    self.ensure_started(id);
    if let Some(usage) = message.get("usage") {
        self.usage.merge_start_usage(usage);
    }
}
```

In `ingest_message_delta`, replace the `token_usage_from` assignment with:

```rust
if let Some(usage) = value.get("usage") {
    self.usage.merge_delta_usage(usage);
}
```

In `finish_events`, emit:

```rust
usage: self.usage.finish(),
```

- [ ] **Step 4: Run the exact test and confirm it passes**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_posts_messages_payload_and_streams_events --exact --nocapture
```

Expected: PASS.

## Task 2: Replace Anthropic Last-Message Cache Injection With Anchor Planning

**Files:**
- Modify: `crates/neo-ai/src/providers/anthropic.rs:214-277`
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs:1631-1674`

- [ ] **Step 1: Add a failing cache-anchor test for tool loops**

Add this test to `crates/neo-ai/tests/real_provider_adapters.rs` near the existing Anthropic cache test:

```rust
#[tokio::test]
async fn anthropic_messages_client_marks_latest_real_user_and_tail_for_prompt_cache() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-cache-loop" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.messages = vec![
        ChatMessage::System {
            content: vec![ContentPart::Text { text: "stable system".to_owned() }],
        },
        ChatMessage::User {
            content: vec![ContentPart::Text { text: "analyze repo".to_owned() }],
        },
        ChatMessage::Assistant {
            content: Vec::new(),
            tool_calls: vec![neo_ai::ToolCall {
                id: "toolu-1".to_owned(),
                name: "read_file".to_owned(),
                arguments: json!({"path":"Cargo.toml"}),
            }],
        },
        ChatMessage::ToolResult {
            tool_call_id: "toolu-1".to_owned(),
            content: vec![ContentPart::Text { text: "workspace".to_owned() }],
            is_error: false,
        },
    ];

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    let messages = sent.body["messages"].as_array().expect("messages array");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "analyze repo");
    assert!(messages[0]["content"][0].get("cache_control").is_some());
    assert!(messages.last().unwrap()["content"][0].get("cache_control").is_some());
    assert!(count_cache_control(&sent.body) <= 4);
}

fn count_cache_control(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            usize::from(map.contains_key("cache_control"))
                + map.values().map(count_cache_control).sum::<usize>()
        }
        serde_json::Value::Array(items) => items.iter().map(count_cache_control).sum(),
        _ => 0,
    }
}
```

If `neo_ai::ToolCall` field names differ, use the existing helper/type construction pattern already present in this test file instead of changing the public type.

- [ ] **Step 2: Run the exact test and confirm it fails**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_marks_latest_real_user_and_tail_for_prompt_cache --exact --nocapture
```

Expected: FAIL because current code only marks the last Anthropic message body, not the latest real user message.

- [ ] **Step 3: Add Anthropic message body origin tracking**

In `crates/neo-ai/src/providers/anthropic.rs`, add private types near `message_bodies`:

```rust
struct AnthropicMessageBody {
    value: Value,
    origin: AnthropicBodyOrigin,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AnthropicBodyOrigin {
    RealUser,
    Assistant,
    ToolResult,
}
```

Change `message_bodies` to build `Vec<AnthropicMessageBody>` internally. For consecutive `ChatMessage::ToolResult`, push one body with `origin: AnthropicBodyOrigin::ToolResult`. For `ChatMessage::User`, use `RealUser`. For `ChatMessage::Assistant`, use `Assistant`.

- [ ] **Step 4: Replace the last-message injector**

Delete `inject_cache_control_on_last_message` and add:

```rust
fn inject_cache_control_on_message_anchors(messages: &mut [AnthropicMessageBody]) {
    let latest_real_user = messages
        .iter()
        .rposition(|message| message.origin == AnthropicBodyOrigin::RealUser);
    let tail = messages.len().checked_sub(1);

    let mut indexes = Vec::new();
    if let Some(index) = latest_real_user {
        indexes.push(index);
    }
    if let Some(index) = tail
        && !indexes.contains(&index)
    {
        indexes.push(index);
    }

    for index in indexes {
        inject_cache_control_on_message(&mut messages[index].value);
    }
}

fn inject_cache_control_on_message(message: &mut Value) {
    let Some(content) = message.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(last_block) = content.last_mut() else {
        return;
    };
    let Some(block_type) = last_block.get("type").and_then(Value::as_str) else {
        return;
    };
    if !matches!(block_type, "text" | "image" | "tool_use" | "tool_result") {
        return;
    }
    if let Some(object) = last_block.as_object_mut() {
        object.insert("cache_control".to_owned(), cache_control());
    }
}
```

At the end of `message_bodies`, call `inject_cache_control_on_message_anchors(&mut bodies)` and then return:

```rust
Ok(bodies.into_iter().map(|body| body.value).collect())
```

- [ ] **Step 5: Run both Anthropic cache tests**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_marks_system_tools_and_last_message_for_prompt_cache --exact --nocapture
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_marks_latest_real_user_and_tail_for_prompt_cache --exact --nocapture
```

Expected: both PASS.

## Task 3: Add Conservative Cache Hit-Rate Display

**Files:**
- Modify: `crates/neo-tui/src/shell/context.rs:77-140`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs:5760-5795`

- [ ] **Step 1: Update the footer replay test expectation**

In `rebuild_transcript_from_session_restores_footer_token_usage`, keep the existing assertions and add:

```rust
assert!(footer.contains("hit"), "{footer}");
```

If the test uses exact footer text, update it to expect:

```text
cache 169.2k read · hit 83%
```

Use the actual percentage produced by the fixture values in the test.

- [ ] **Step 2: Run the exact test and confirm it fails**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::rebuild_transcript_from_session_restores_footer_token_usage --exact --nocapture --include-ignored
```

Expected: FAIL because footer currently shows raw cache read/write but not hit rate.

- [ ] **Step 3: Implement hit-rate formatting**

In `crates/neo-tui/src/shell/context.rs`, add:

```rust
fn format_cache_hit_rate(input: u64, read: u64, write: u64) -> Option<String> {
    let denominator = input.max(read.saturating_add(write));
    if denominator == 0 || read == 0 {
        return None;
    }
    let percent = read.saturating_mul(100) / denominator;
    Some(format!("hit {percent}%"))
}
```

In `MainAgentTokenUsage::label`, after `format_cache_usage(...)`, append the hit-rate label:

```rust
if let Some(hit_rate) = format_cache_hit_rate(
    self.input_tokens,
    self.input_cache_read_tokens,
    self.input_cache_write_tokens,
) {
    parts.push(hit_rate);
}
```

Keep `format_cache_usage` unchanged so raw values remain visible.

- [ ] **Step 4: Run the exact footer test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::rebuild_transcript_from_session_restores_footer_token_usage --exact --nocapture --include-ignored
```

Expected: PASS.

## Task 4: Honor CacheRetention In Anthropic Cache Control

**Files:**
- Modify: `crates/neo-ai/src/providers/anthropic.rs:111-165`
- Modify: `crates/neo-ai/tests/real_provider_adapters.rs`

- [ ] **Step 1: Add a failing test for `CacheRetention::None`**

Add a test near the Anthropic cache tests:

```rust
#[tokio::test]
async fn anthropic_messages_client_omits_cache_control_when_cache_retention_is_none() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "message_start", "message": { "id": "msg-no-cache" } }),
        json!({ "type": "message_stop" }),
    ])]);
    let client = AnthropicMessagesClient::new(server.url.clone(), "test-key");
    let mut request = request(ApiKind::AnthropicMessages);
    request.options.cache = CacheRetention::None;
    request.messages = vec![ChatMessage::User {
        content: vec![ContentPart::Text { text: "hello".to_owned() }],
    }];

    client
        .stream_chat(request)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    let sent = server.requests().pop().unwrap();
    assert_eq!(count_cache_control(&sent.body), 0);
}
```

- [ ] **Step 2: Run the exact test and confirm it fails**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_omits_cache_control_when_cache_retention_is_none --exact --nocapture
```

Expected: FAIL because Anthropic cache control is currently hard-coded.

- [ ] **Step 3: Thread `CacheRetention` through Anthropic cache helpers**

Change `cache_control()` to accept retention:

```rust
fn cache_control(cache: CacheRetention) -> Option<Value> {
    match cache {
        CacheRetention::None => None,
        CacheRetention::Short | CacheRetention::Long => Some(json!({
            "type": "ephemeral",
            "ttl": "1h",
        })),
    }
}
```

Important: map `Long` to `1h` unless official provider support in this codebase proves a longer Anthropic TTL is accepted. Do not invent `24h`.

Update system/tool/message insertion sites to skip insertion when `cache_control(request.options.cache)` returns `None`.

Change `message_bodies` signature:

```rust
fn message_bodies(
    messages: &[ChatMessage],
    replay_reasoning: bool,
    cache: CacheRetention,
) -> Result<Vec<Value>, ProviderError>
```

Then call it from `request_body` with `request.options.cache`.

- [ ] **Step 4: Run cache tests**

Run:

```bash
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_omits_cache_control_when_cache_retention_is_none --exact --nocapture
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_marks_system_tools_and_last_message_for_prompt_cache --exact --nocapture
cargo test --package neo-ai --test real_provider_adapters -- anthropic_messages_client_marks_latest_real_user_and_tail_for_prompt_cache --exact --nocapture
```

Expected: all PASS.

## Task 5: Add Runtime Prefix Stability Guardrails

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs:23-24`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Document `ContextAppendTransform` constraints**

In `crates/neo-agent-core/src/runtime/config.rs`, replace the current one-line type alias area with a comment:

```rust
/// Request-local append transform for model-visible context.
///
/// This transform must append only new request-local messages. It must not
/// mutate, delete, reorder, or replace messages from durable `AgentContext`
/// history. Providers rely on byte-stable historical prefixes for prompt-cache
/// hits, and compaction/projection code owns the only intentional request-time
/// rewriting path.
pub type ContextAppendTransform = Arc<dyn Fn(&[AgentMessage]) -> Vec<AgentMessage> + Send + Sync>;
```

- [ ] **Step 2: Add a prefix stability test**

In `crates/neo-agent-core/tests/runtime_turn.rs`, add a test using existing harness helpers that builds two requests from the same durable context while changing live permission mode or plan mode between them. Assert that the serialized prefix of `request.messages` before the newest append is identical.

Use this assertion style:

```rust
let before = serde_json::to_value(&first_request.messages[..prefix_len]).unwrap();
let after = serde_json::to_value(&second_request.messages[..prefix_len]).unwrap();
assert_eq!(before, after);
```

If `chat_request` is not accessible from this integration test, add the test to `crates/neo-agent-core/src/runtime/chat_request.rs` under its existing `#[cfg(test)]` module instead.

- [ ] **Step 3: Run the exact prefix test**

Run the exact command matching the final test location. If placed in `chat_request.rs`, run:

```bash
cargo test --package neo-agent-core --lib runtime::chat_request::tests::chat_request_keeps_existing_prefix_stable_across_live_mode_changes --exact --nocapture
```

Expected: PASS.

## Task 6: Optional Follow-up: Stabilize Multi-Agent Child System Prompts

**Files:**
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs:1809-1818`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs:2268-2273`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`

- [ ] **Step 1: Add a failing multi-agent system prompt stability test**

Add a test that creates child configs for two roles and asserts their system prompt base is identical when the parent config is identical. The role-specific profile should be visible in the child user prompt, not in `config.system_prompt`.

If private functions make this awkward, test through the fake model request captured by the multi-agent runtime rather than widening public API.

- [ ] **Step 2: Move role profile from system prompt to child prompt**

In `child_config`, stop formatting `<subagent_profile>` into `config.system_prompt`. Keep the filtered tool list and `before_tool_call` guard unchanged.

In `child_prompt`, append:

```rust
let profile = super::profile::AgentProfile::for_role(role);
format!(
    "You are a bounded Neo subagent.\n\nRole: {role:?}\nTask: {task}\nContext mode: {}\n\n<subagent_profile>\n{}\n\nDo not repeat or acknowledge this profile text in your final answer. Return only the requested findings or summary.\n</subagent_profile>\n\nReturn a concise result for the parent agent. Do not perform git mutations. Do not run git add, git commit, git reset, git checkout, git restore, git stash, git clean, git rebase, git push, git rm, git branch, git switch, git merge, git cherry-pick, git tag, or git worktree.",
    context.as_str(),
    profile.prompt_addendum
)
```

- [ ] **Step 3: Run the exact multi-agent test**

Run:

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- child_role_profile_does_not_change_child_system_prompt --exact --nocapture
```

Expected: PASS.

## Task 7: Final Narrow Verification

**Files:**
- No new files.

- [ ] **Step 1: Run touched exact tests only**

Run the exact tests from Tasks 1-6. Do not broaden to package-wide tests.

- [ ] **Step 2: Run whitespace check on touched files**

Run:

```bash
git diff --check -- crates/neo-ai/src/providers/anthropic.rs crates/neo-ai/tests/real_provider_adapters.rs crates/neo-tui/src/shell/context.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/src/runtime/config.rs crates/neo-agent-core/src/runtime/chat_request.rs crates/neo-agent-core/tests/runtime_turn.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/tests/multi_agent_runtime.rs
```

Expected: no output.

- [ ] **Step 3: Record manual dashboard validation notes**

After implementation, repeat one DeepSeek Anthropic-compatible long analysis run and record these observations in the final response, not in source code:

```text
start hit/miss:
after warmup hit/miss:
late-run hit/miss:
local Neo footer usage:
provider dashboard usage:
```

Expected: local Neo cache usage is non-empty and the provider dashboard still trends toward high cache-hit share. The exact 1M warmup plateau is provider behavior and should not be treated as failure by itself.
