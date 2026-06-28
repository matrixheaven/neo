# Neo Crates Architecture Restructure Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the four "god files" and pervasive code duplication across all four Neo crates by splitting monolithic `.rs` files into focused subdirectories, extracting shared infrastructure, and enforcing single-file-single-responsibility — without changing any runtime behavior.

**Architecture:** Six independent phases, each scoped to one crate or one cross-cutting concern. Each phase produces working, testable software on its own. Phases are ordered by risk (lowest first) and can be executed in parallel if they touch different crates.

**Tech Stack:** Rust edition 2024, min 1.88. Cargo workspace. Test runner: `cargo-nextest` via `cargo run -p xtask -- test -p <crate>`.

---

## Phase Overview

| Phase | Crate | Target | Lines Affected | Risk | Prerequisites |
|-------|-------|--------|---------------|------|---------------|
| **1** | neo-ai | Provider dedup: extract shared SSE/error/HTTP/helpers | ~800 deleted | 🟢 Low | None |
| **2** | neo-agent-core | Split `runtime.rs` (4,623 lines) → `runtime/` subdirectory | ~4,600 moved | 🟡 Medium | None |
| **3** | neo-agent | Split `interactive.rs` (16,639 lines) → `interactive/` subdirectory | ~16,600 moved | 🟠 Medium-High | None |
| **4** | neo-agent | Split `run.rs` (3,222 lines) + `config.rs` (1,268 lines) into domain modules | ~4,500 moved | 🟡 Medium | Phase 3 (shared imports) |
| **5** | neo-tui | Split `shell/mod.rs`, `pane.rs`, `entry.rs`, `frame_differ.rs` | ~6,650 moved | 🟡 Medium | None |
| **6** | Cross-cutting | Token dedup, plan consolidation, dead code, theme relocation | ~300 deleted + ~500 moved | 🟢 Low | Phases 2+4 ideally |

**Verification commands (all phases):**
- Build: `cargo build -p <crate>`
- Test: `cargo run -p xtask -- test -p <crate>`
- Full check: `cargo run -p xtask -- check`
- Full CI: `cargo run -p xtask -- ci`

---

## Execution Strategy

Each phase is independently shippable. The recommended execution order is **1 → 2 → 5 → 6 → 4 → 3** (lowest risk to highest risk), but phases touching different crates can overlap. **Commit after every task within a phase.**

For each file-splitting task, the mechanical pattern is:
1. Create the new target file(s) with `mod.rs`
2. Move code blocks (using exact line ranges from the mapping tables)
3. Fix import paths (`use` statements)
4. Build + test
5. Commit

**Critical rule for all file moves:** Do NOT change any logic. Move code verbatim. The only edits allowed are: (a) `use` path updates, (b) visibility adjustments (`fn` → `pub(crate) fn` if the function is now in a sibling module), (c) `super::` → `crate::` path fixes.

---

<!-- ==================================================================== -->
<!-- PHASE 1: neo-ai Provider Dedup -->
<!-- ==================================================================== -->

# Phase 1: neo-ai Provider Deduplication

**Goal:** Eliminate ~800 lines of byte-for-byte duplicated code across the four provider wire clients by extracting shared SSE infrastructure, error types, HTTP retry logic, and common helpers.

**Crate:** `crates/neo-ai`

**Target structure:**
```
crates/neo-ai/src/providers/
├── mod.rs                    # existing + `mod common;` `mod openai;`
├── common/
│   ├── mod.rs                # re-exports
│   ├── error.rs              # unified ProviderError
│   ├── helpers.rs            # rounded_f64, token_usage_from, reject_images
│   ├── http.rs               # open_response retry loop + inject_extra_headers
│   └── sse.rs                # IncrementalSse<P>, find_frame_end, parse_sse_frame, stream_response
├── openai/
│   ├── mod.rs                # shared headers, image_url
│   ├── responses.rs          # moved from openai_responses.rs
│   ├── compatible.rs         # moved from openai_compatible.rs
│   └── images.rs             # moved from openai_images.rs
├── anthropic.rs              # stays, slimmed
├── google.rs                 # stays, slimmed
└── fake.rs                   # unchanged
```

---

### Task 1.1: Create `common/error.rs` — unified ProviderError

**Files:**
- Create: `crates/neo-ai/src/providers/common/mod.rs`
- Create: `crates/neo-ai/src/providers/common/error.rs`
- Modify: `crates/neo-ai/src/providers/mod.rs`

- [ ] **Step 1: Create `common/mod.rs`**

```rust
//! Shared infrastructure for provider wire clients.
pub(crate) mod error;
```

- [ ] **Step 2: Create `common/error.rs`** with unified `ProviderError` enum

The unified enum must cover all variants from the four providers:
- responses/compatible: `Header(String)`, `HttpStatus(u16)`, `Transport(reqwest::Error)`, `Stream(String)`
- anthropic: same but `HttpStatus { status: u16, body: String }`
- google: adds `Url(String)`, `Unsupported(String)`

Unify to `HttpStatus { status: u16, body: Option<String> }` — anthropic passes `Some(body)`, others pass `None`.

Include: `MAX_HTTP_ERROR_BODY_CHARS` const, `error_body_excerpt()`, `format_http_status_error()`, `is_retryable()`, `into_ai_error()`.

Read `openai_responses.rs:94-118` and `anthropic.rs:98-148` to get the exact `is_retryable` and `into_ai_error` logic. Combine them into one impl.

```rust
use crate::error::AiError;

const MAX_HTTP_ERROR_BODY_CHARS: usize = 4096;

fn error_body_excerpt(body: &str) -> String { /* truncate at MAX */ }

pub(crate) fn format_http_status_error(status: u16, body: &Option<String>) -> String { /* ... */ }

#[derive(Debug)]
pub(crate) enum ProviderError {
    Header(String),
    HttpStatus { status: u16, body: Option<String> },
    Transport(reqwest::Error),
    Stream(String),
    Url(String),
    Unsupported(String),
}

impl ProviderError {
    pub(crate) const fn is_retryable(&self) -> bool { /* 429 || >=500 || Transport */ }
    pub(crate) fn into_ai_error(self) -> AiError { /* map each variant */ }
}
```

- [ ] **Step 3: Add `mod common;` to `providers/mod.rs`**

Add after existing `pub mod` lines, before `use crate::types::ContentPart;`.

- [ ] **Step 4: Build** — `cargo build -p neo-ai`

- [ ] **Step 5: Commit**

```bash
git add crates/neo-ai/src/providers/common/ crates/neo-ai/src/providers/mod.rs
git commit -m "refactor(neo-ai): add unified ProviderError in providers/common/error.rs"
```

---

### Task 1.2: Create `common/helpers.rs` — `rounded_f64`, `token_usage_from`, `reject_images`

**Files:**
- Create: `crates/neo-ai/src/providers/common/helpers.rs`
- Modify: `crates/neo-ai/src/providers/common/mod.rs`

- [ ] **Step 1: Create `helpers.rs`**

Three functions, all parameterized to handle per-provider differences:

```rust
use crate::types::{ContentPart, TokenUsage};
use serde_json::Value;
use super::error::ProviderError;

pub(crate) fn rounded_f64(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

pub(crate) fn token_usage_from(value: &Value, input_key: &str, output_key: &str) -> Option<TokenUsage> {
    let usage = value.get("usage")?;
    Some(TokenUsage {
        input_tokens: u32::try_from(usage.get(input_key)?.as_u64()?).ok()?,
        output_tokens: u32::try_from(usage.get(output_key)?.as_u64()?).ok()?,
    })
}

pub(crate) fn reject_images(content: &[ContentPart], role: &str, provider_label: &str) -> Result<(), ProviderError> {
    if content.iter().any(|c| matches!(c, ContentPart::Image { .. })) {
        Err(ProviderError::Unsupported(format!("{provider_label} does not support image content (role: {role})")))
    } else {
        Ok(())
    }
}
```

- [ ] **Step 2: Add `pub(crate) mod helpers;` to `common/mod.rs`**

- [ ] **Step 3: Build + commit**

```bash
git add crates/neo-ai/src/providers/common/
git commit -m "refactor(neo-ai): add common helpers (rounded_f64, token_usage_from, reject_images)"
```

---

### Task 1.3: Wire all four providers to common error + helpers

**Files:** `openai_responses.rs`, `anthropic.rs`, `openai_compatible.rs`, `google.rs`

- [ ] **Step 1: For each provider, add imports**

```rust
use super::common::error::ProviderError;
use super::common::helpers::{reject_images, rounded_f64, token_usage_from};
```

- [ ] **Step 2: For each provider, delete local `ProviderError` enum + impl**

Delete the local `enum ProviderError` and its `impl` block. Replace all `ProviderError::HttpStatus(status)` with `ProviderError::HttpStatus { status, body: None }`. For anthropic, change `HttpStatus { status, body }` to `HttpStatus { status, body: Some(body) }`.

- [ ] **Step 3: Replace `token_usage` calls**

Delete local `fn token_usage`. Replace call sites:
- responses/anthropic: `token_usage_from(value, "input_tokens", "output_tokens")`
- compatible: `token_usage_from(value, "prompt_tokens", "completion_tokens")`
- google: `token_usage_from(value, "promptTokenCount", "candidatesTokenCount")`

- [ ] **Step 4: Replace `reject_images` and `rounded_f64`**

Delete local copies. Replace call sites with the imported versions, passing the provider label string.

- [ ] **Step 5: Build + test**

```bash
cargo run -p xtask -- test -p neo-ai
```

- [ ] **Step 6: Commit**

```bash
git add crates/neo-ai/src/providers/
git commit -m "refactor(neo-ai): wire all providers to common ProviderError and helpers"
```

---

### Task 1.4: Extract `common/http.rs` — shared retry loop + header injection

**Files:**
- Create: `crates/neo-ai/src/providers/common/http.rs`
- Modify: all four provider files

- [ ] **Step 1: Create `http.rs`**

Read `openai_responses.rs:29-47` to get the exact retry loop. Extract two functions:

```rust
pub(crate) fn inject_extra_headers(headers: &mut HeaderMap, extra: &BTreeMap<String, String>) -> Result<(), ProviderError> { /* ... */ }

pub(crate) async fn open_response<F, Fut>(request: &ChatRequest, once: F) -> Result<reqwest::Response, AiError>
where F: Fn(&ChatRequest) -> Fut, Fut: Future<Output = Result<Response, ProviderError>> { /* retry loop */ }
```

- [ ] **Step 2: Add `pub(crate) mod http;` to `common/mod.rs`**

- [ ] **Step 3: For each provider, delete local `open_response` and call shared version**

Each provider's `open_response` becomes:
```rust
async fn open_response(&self, request: &ChatRequest) -> Result<Response, AiError> {
    super::common::http::open_response(request, |req| self.open_response_once(req)).await
}
```

Replace extra-headers injection loops in each `fn headers` with `inject_extra_headers(&mut headers, extra)?;`.

- [ ] **Step 4: Build + test + commit**

```bash
cargo run -p xtask -- test -p neo-ai
git commit -am "refactor(neo-ai): extract shared HTTP retry loop and header injection"
```

---

### Task 1.5: Extract `common/sse.rs` — SSE infrastructure with `SsePayloadParser` trait

**Files:**
- Create: `crates/neo-ai/src/providers/common/sse.rs`
- Modify: all four provider files

This is the highest-risk task. Extract `StreamChunk`, `find_frame_end`, `parse_sse_frame`, `IncrementalSse`, and `stream_response` — all byte-identical across providers.

- [ ] **Step 1: Create `sse.rs` with trait + generic struct**

Define `SsePayloadParser` trait that each provider's `ParseState` will implement:
- `fn ingest(&mut self, value: &Value) -> Result<(), AiError>`
- `fn drain_events(&mut self) -> Vec<Result<AiStreamEvent, AiError>>`
- `fn saw_terminal(&self) -> bool`
- `fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>>`
- `fn handles_done_marker(&self) -> bool` (default `true`; google overrides to `false`)

Define `IncrementalSse<P: SsePayloadParser>` with `new`, `push_chunk`, `finish`.
Define `stream_response(response, parser)` free function.

Read `openai_responses.rs:379-504, 404-479` for exact implementations. Copy verbatim, parameterize with `P`.

- [ ] **Step 2: Implement `SsePayloadParser` for each provider's `ParseState`**

Each provider deletes local `StreamChunk`/`stream_response`/`IncrementalSse`/`find_frame_end`/`parse_sse_frame` and adds:

```rust
impl SsePayloadParser for ParseState {
    fn ingest(&mut self, value: &Value) -> Result<(), AiError> { self.ingest(value); Ok(()) }
    fn drain_events(&mut self) -> Vec<Result<AiStreamEvent, AiError>> { self.drain_events().into_iter().map(Ok).collect() }
    fn saw_terminal(&self) -> bool { self.saw_terminal() }
    fn finish(&mut self) -> Vec<Result<AiStreamEvent, AiError>> { /* call finish_events, map errors */ }
}
```

**Critical per-provider differences:**
- **compatible**: `finish` must preserve `drain_trailing_payload_events` logic (read `openai_compatible.rs:353-451`). Tests at lines 731/755 cover this.
- **google**: override `handles_done_marker() -> false`. `ingest` already returns `Result`. `finish_events` returns `Vec` not `Result`.

- [ ] **Step 3: Build + test + commit**

```bash
cargo run -p xtask -- test -p neo-ai
git commit -am "refactor(neo-ai): extract shared SSE infrastructure with SsePayloadParser trait"
```

---

### Task 1.6: Create `providers/openai/` subdirectory

**Files:** Move three OpenAI files + update imports

- [ ] **Step 1: Move files**

```bash
mkdir -p crates/neo-ai/src/providers/openai
git mv crates/neo-ai/src/providers/openai_responses.rs crates/neo-ai/src/providers/openai/responses.rs
git mv crates/neo-ai/src/providers/openai_compatible.rs crates/neo-ai/src/providers/openai/compatible.rs
git mv crates/neo-ai/src/providers/openai_images.rs crates/neo-ai/src/providers/openai/images.rs
```

- [ ] **Step 2: Create `openai/mod.rs`**

```rust
pub mod compatible;
pub mod images;
pub mod responses;
```

- [ ] **Step 3: Update `providers/mod.rs`** — replace three `pub mod openai_*` with `pub mod openai;`

- [ ] **Step 4: Update `registry.rs`** imports — `openai::responses::OpenAiResponsesClient` etc.

- [ ] **Step 5: Fix `super::` references** in moved files → `crate::providers::common::*` and `crate::providers::collect_text_content`

- [ ] **Step 6: Build + test + commit**

---

### Task 1.7: Extract OpenAI-shared `headers` + `image_url` into `openai/mod.rs`

**Files:** `openai/mod.rs`, `openai/responses.rs`, `openai/compatible.rs`

- [ ] **Step 1: Add shared functions to `openai/mod.rs`**

`headers(api_key, extra_headers, session_id)` — byte-identical between responses and compatible (Authorization: Bearer + extra + x-client-request-id).
`image_url(base64_data, mime_type)` — byte-identical between responses and compatible.

- [ ] **Step 2: Delete local copies in responses.rs and compatible.rs, use `super::headers()` / `super::image_url()`**

- [ ] **Step 3: Build + test + commit**

```bash
cargo run -p xtask -- test -p neo-ai
git commit -am "refactor(neo-ai): extract shared OpenAI headers and image_url"
```

---

<!-- ==================================================================== -->
<!-- PHASE 2: neo-agent-core runtime.rs split -->
<!-- ==================================================================== -->

# Phase 2: Split `runtime.rs` (4,623 lines) → `runtime/` subdirectory

**Goal:** Break the god-file `runtime.rs` into focused modules, each < 500 lines, with clear single responsibilities.

**Crate:** `crates/neo-agent-core`

**Current state:** `runtime.rs` (4,623 lines) contains 16 responsibility domains: config, context, entry point, turn loop, stream aggregation, tool dispatch, permission pipeline (920 lines!), compaction trigger, plan orchestration, queue management, skill dispatch, image blobs, chat request building, events, token estimation, tests.

### Target structure

```
crates/neo-agent-core/src/runtime/
├── mod.rs                (~200) AgentRuntime entry + run_turn + run_agent_turn + re-exports
├── config.rs             (~480) AgentConfig, CompactionSettings, type aliases, builders
├── context.rs            (~220) AgentContext + replay logic
├── chat_request.rs       (~170) chat_request + capability validation + reasoning filter
├── stream_aggregator.rs  (~230) ModelTurnState + run_model_turn
├── tool_dispatch.rs      (~480) execute_tool_calls + scheduling + prepare_tool_call
├── permission.rs         (~920) permission pipeline + resolve_approval + shell tokenize
├── compaction_trigger.rs (~383) maybe_compact + evaluate + spawn_summary + progress
├── plan_orchestration.rs (~290) plan enter/exit/guard/inject
├── queue.rs              (~120) steering/follow-up/live-steer drain
├── skill_dispatch.rs     (~94)  invoke_skill logic
├── image_blobs.rs        (~98)  resolve_image/content_blobs
├── events.rs             (~400) EventEmitter + EventSink + side-effect emitters
└── tokens.rs             (~76)  estimate_* token functions
```

Tests (lines 4312–4623, ~312 lines) stay in their respective domain modules as `#[cfg(test)]`.

### Code mapping table

| Current lines | Domain | Target file | Key items |
|--------------|--------|------------|-----------|
| 1–36 | (imports) | distributed | Each module gets its own `use` block |
| 38–54 | type aliases | `config.rs` | `ContextTransform`, `BeforeToolCallHook`, etc. |
| 56–82 | permission types | `permission.rs` | `ApprovalRequest`, `PermissionPreparation` |
| 84–99 | dispatch types | `tool_dispatch.rs` | `ToolSchedulingClass`, `PreparedToolCall`, `PreparedToolCallResult` |
| 101–111 | config enums | `config.rs` | `QueueMode`, `ToolExecutionMode` |
| 113–522 | config | `config.rs` | `AgentConfig` struct (31 fields) + `impl` + `CompactionSettings` |
| 524–737 | context | `context.rs` | `AgentContext` + impl + replay logic |
| 739–753 | entry types | `mod.rs` | `AgentRuntimeError`, `AgentEventStream` |
| 761–810 | queue types | `queue.rs` | `ActiveTurnInput`, `SteerInputHandle` |
| 812–1063 | entry | `mod.rs` | `AgentRuntime`, `SpawnedRun`, `run_turn*`, `run_compaction_turn*` |
| 1065–1162 | image_blob | `image_blobs.rs` | `resolve_image_blobs`, `resolve_content_blobs`, `read_blob_bytes` |
| 1164–1170 | plan | `plan_orchestration.rs` | `plan_mode_plans_dir` |
| 1172–1330 | chat_request | `chat_request.rs` | `chat_request`, `workspace_context_message`, `validate_model_capabilities` |
| 1326–1432 | events | `events.rs` | `EventEmitter`, `EventPublisher`, `EventSink` |
| 1434–1570 | turn_loop | `mod.rs` | `run_agent_turn`, `next_pending_after_assistant`, `append_tool_result_messages` |
| 1572–1741 | plan | `plan_orchestration.rs` | `attach_exit_plan_details`, `emit_plan_*`, `enter_plan_mode_state` |
| 1633–1644, 1743–1907 | events | `events.rs` | `emit_tool_side_effect_events`, `emit_todo_event`, `emit_goal_event_*`, `emit_queue_drained`, etc. |
| 1768–1975 | turn_loop | `mod.rs` | `goal_continuation_messages`, `terminal_pre_model_stop`, `append_queued_messages` |
| 1848–1907 | queue | `queue.rs` | `drain_next_pending_queue`, `drain_steering_queue`, `drain_follow_up_queue`, `drain_live_steer_input` |
| 1977–2359 | compaction | `compaction_trigger.rs` | `maybe_compact`, `CompactionTrigger`, `evaluate_compaction_need`, `spawn_summary_task`, `run_summary_progress_loop`, `apply_compaction_result` |
| 2361–2436 | tokens | `tokens.rs` | `estimate_messages_tokens`, `estimate_chat_message_tokens`, etc. |
| 2438–2448 | queue | `queue.rs` | `take_messages` |
| 2468–2779 | tool_dispatch | `tool_dispatch.rs` | `execute_tool_calls`, `execute_tool_calls_sequential`, `execute_tool_calls_parallel`, `before_tool_result`, `after_tool_result` |
| 2781–3010 | stream | `stream_aggregator.rs` | `run_model_turn`, `ModelTurnState` + impl |
| 3012–3055 | tool_dispatch | `tool_dispatch.rs` | `prepare_and_run_tool` |
| 3057–3928 | permission | `permission.rs` | `prepare_tool_call`, `permission_preparation_for_mode`, `check_plan_guard`, `check_mode_early_returns`, `check_cached_approvals`, `check_transition_tools`, `check_plan_file_write`, `check_safe_or_prompt`, `resolve_approval`, `tokenize_shell_command`, `bash_approval_scope`, etc. |
| 3930–4139 | events | `events.rs` | `emit_shell_started`, `emit_shell_finished`, `emit_terminal_events`, `make_tool_update_callback` |
| 4141–4310 | tool_dispatch | `tool_dispatch.rs` | `run_tool_with_cancel`, `cancelled_tool_result`, `run_model_bash_with_cancel`, `default_tool_context` |
| 4181–4274 | skill | `skill_dispatch.rs` | `invoke_skill_tool_spec`, `execute_invoke_skill`, `SkillToolRequest` |
| 4312–4623 | test | distributed | skill tests → `skill_dispatch.rs`, compaction tests → `compaction_trigger.rs` |

---

### Task 2.1: Create `runtime/` directory scaffold

- [ ] **Step 1: Create directory + empty `mod.rs`**

```bash
mkdir -p crates/neo-agent-core/src/runtime
```

Create `crates/neo-agent-core/src/runtime/mod.rs` with all submodule declarations:

```rust
mod config;
mod context;
mod chat_request;
mod stream_aggregator;
mod tool_dispatch;
mod permission;
mod compaction_trigger;
mod plan_orchestration;
mod queue;
mod skill_dispatch;
mod image_blobs;
mod events;
mod tokens;
```

- [ ] **Step 2: Rename `runtime.rs` to `runtime/legacy.rs` temporarily**

```bash
git mv crates/neo-agent-core/src/runtime.rs crates/neo-agent-core/src/runtime/legacy.rs
```

Add `mod legacy;` to `runtime/mod.rs`.

- [ ] **Step 3: Update `lib.rs`** — `pub mod runtime;` stays the same (it already points to the directory).

- [ ] **Step 4: Build** — must still compile with everything in `legacy.rs`.

- [ ] **Step 5: Commit**

---

### Task 2.2: Extract `runtime/tokens.rs`

Lowest-risk extraction — token estimation functions are pure, stateless, and only called within runtime.

- [ ] **Step 1: Move lines 2361–2436 from `legacy.rs` to `tokens.rs`**

Move: `estimate_messages_tokens`, `estimate_chat_messages_tokens`, `estimate_chat_message_tokens`, `estimate_message_tokens`, `estimate_chat_content_chars`, `estimate_content_chars`.

- [ ] **Step 2: Add necessary `use` imports in `tokens.rs`** (for `ChatMessage`, `ContentPart`, `Content` types).

- [ ] **Step 3: Make functions `pub(super)` so other runtime modules can call them**

- [ ] **Step 4: Update call sites in `legacy.rs`** — prefix with `super::tokens::` or add `use super::tokens::*;`

- [ ] **Step 5: Build + test**

```bash
cargo run -p xtask -- test -p neo-agent-core
```

- [ ] **Step 6: Commit**

```bash
git commit -am "refactor(neo-agent-core): extract token estimation into runtime/tokens.rs"
```

---

### Task 2.3: Extract `runtime/image_blobs.rs`

- [ ] **Step 1: Move lines 1065–1162** (`resolve_image_blobs`, `resolve_content_blobs`, `read_blob_bytes`) to `image_blobs.rs`

- [ ] **Step 2: Make functions `pub(super)`, add `use` imports, update call sites**

- [ ] **Step 3: Build + test + commit**

---

### Task 2.4: Extract `runtime/queue.rs`

- [ ] **Step 1: Move queue types and functions**

Move lines 761–810 (`ActiveTurnInput`, `SteerInputHandle` + impl) and lines 1848–1907 (`drain_next_pending_queue`, `drain_steering_queue`, `drain_follow_up_queue`, `drain_live_steer_input`, `emit_queue_drained`) and 2438–2448 (`take_messages`).

- [ ] **Step 2: Make items `pub(super)` where needed, fix imports**

- [ ] **Step 3: Build + test + commit**

---

### Task 2.5: Extract `runtime/events.rs`

- [ ] **Step 1: Move event infrastructure**

Move lines 1326–1432 (`EventEmitter`, `EventPublisher` trait, `EventSink`) and all `emit_*` side-effect functions:
- 1633–1644 (`emit_tool_side_effect_events`)
- 1743–1766 (`emit_todo_event`)
- 1789–1907 (`emit_goal_event_from_result`, `emit_queue_drained`, `emit_run_finished`, `emit_effective_context_window`, `emit_context_window_update`)
- 3930–4139 (`emit_shell_started`, `emit_shell_finished`, `shell_command_outcome_from_details`, `emit_terminal_events`, `make_tool_update_callback`)
- 1932–1953 (`chat_request_for_context_estimate` if event-related, or to `chat_request.rs`)

- [ ] **Step 2: Make items `pub(super)`, fix imports**

- [ ] **Step 3: Build + test + commit**

---

### Task 2.6: Extract `runtime/config.rs`

- [ ] **Step 1: Move config types**

Move lines 38–54 (type aliases: `ContextTransform`, hooks, `ApprovalHandler`), 101–111 (`QueueMode`, `ToolExecutionMode`), 113–522 (`AgentConfig` + impl + `CompactionSettings`).

- [ ] **Step 2: Update `lib.rs` re-exports**

Check `lib.rs` line 32 — it does `pub use runtime::*;` (or similar). Ensure the re-exports still work. The items are now `runtime::config::AgentConfig` — add `pub use config::*;` in `runtime/mod.rs`.

- [ ] **Step 3: Build + test**

This is a critical extraction — `AgentConfig` is used everywhere. If build fails, check that all `use crate::runtime::AgentConfig` paths resolve.

- [ ] **Step 4: Commit**

---

### Task 2.7: Extract `runtime/context.rs`

- [ ] **Step 1: Move lines 524–737** (`AgentContext` struct + impl + replay methods) to `context.rs`

- [ ] **Step 2: Make items `pub(super)` or `pub`, fix imports**

- [ ] **Step 3: Build + test + commit**

---

### Task 2.8: Extract `runtime/stream_aggregator.rs`

- [ ] **Step 1: Move lines 2781–3010** (`run_model_turn`, `ModelTurnState` struct + impl) to `stream_aggregator.rs`

- [ ] **Step 2: Make items `pub(super)`, fix imports**

- [ ] **Step 3: Build + test + commit**

---

### Task 2.9: Extract `runtime/permission.rs`

This is the largest domain extraction (~920 lines). The permission pipeline is self-contained: it receives an `AgentConfig` reference and returns decisions.

- [ ] **Step 1: Move lines 56–82** (`ApprovalRequest`, `PermissionPreparation`) and **3057–3928** (all permission functions from `prepare_tool_call` through `tool_approval_scope`).

Functions to move: `prepare_tool_call`, `permission_preparation_for_mode`, `check_plan_guard`, `check_mode_early_returns`, `check_cached_approvals`, `check_transition_tools`, `check_plan_file_write`, `check_safe_or_prompt`, `current_permission_mode`, `is_default_approved_tool`, `access_for_tool`, `resolve_approval`, `permission_error`, `permission_operation_for_tool`, `path_subject`, `workspace_key_root`, `resolve_bash_cwd`, `tokenize_shell_command`, `is_compound_or_opaque_command`, `shell_argv`, `shell_argv_for_prefix_check`, `approval_scope_for_tool_call`, `bash_approval_scope`, `file_write_approval_scope`, `tool_approval_scope`.

- [ ] **Step 2: Make items `pub(super)`, fix imports**

These functions take `&AgentConfig` — import `super::config::AgentConfig`.

- [ ] **Step 3: Build + test + commit**

---

### Task 2.10: Extract remaining domains

Extract in this order (each is a separate commit):
1. `runtime/compaction_trigger.rs` — lines 1977–2359
2. `runtime/plan_orchestration.rs` — lines 1164–1170, 1572–1741, 2577–2588
3. `runtime/tool_dispatch.rs` — lines 84–99, 2468–2779, 3012–3055, 4141–4310
4. `runtime/skill_dispatch.rs` — lines 4181–4274
5. `runtime/chat_request.rs` — lines 1172–1330

For each: move lines, make `pub(super)`, fix imports, build + test + commit.

---

### Task 2.11: Finalize `runtime/mod.rs`

- [ ] **Step 1: Verify `legacy.rs` is now empty** (or only contains the entry point + turn loop)

The only items remaining should be: `AgentRuntime`, `AgentRuntimeError`, `AgentEventStream`, `SpawnedRun`, `run_agent_turn`, `next_pending_after_assistant`, `append_tool_result_messages`, `goal_continuation_messages`, `terminal_pre_model_stop`, `append_queued_messages`, `terminates_tool_batch`, `continues_after_terminating_batch`.

- [ ] **Step 2: Move these into `mod.rs` directly**, delete `legacy.rs`

- [ ] **Step 3: Add `pub use` re-exports in `mod.rs`**

```rust
pub use self::config::*;
pub use self::context::AgentContext;
pub use self::context::ActiveTurnInput;  // if not already in queue
// etc. — ensure all items that were `pub` in the old runtime.rs are re-exported
```

- [ ] **Step 4: Full build + test + clippy**

```bash
cargo run -p xtask -- test -p neo-agent-core
cargo run -p xtask -- check
```

- [ ] **Step 5: Commit**

```bash
git commit -am "refactor(neo-agent-core): finalize runtime/ module split"
```

---

<!-- ==================================================================== -->
<!-- PHASE 3: neo-agent interactive.rs split -->
<!-- ==================================================================== -->

# Phase 3: Split `interactive.rs` (16,639 lines) → `interactive/` subdirectory

**Goal:** Break the 16,639-line monolith into focused modules, each < 650 lines.

**Crate:** `crates/neo-agent`

**Current state:** `modes/interactive.rs` contains the entire TUI application layer — 16+ responsibility domains including git status parsing, prompt completion engine, clipboard management, slash command dispatch, MCP manager UI, model picker catalog, turn lifecycle, approval routing, and ~8,000 lines of tests (55% of the file).

### Target structure

```
crates/neo-agent/src/modes/interactive/
├── mod.rs                (~250) Entry: execute_with_startup, controller_for_config, StartupAction
├── controller.rs         (~600) InteractiveController struct + terminal main loop
├── turn.rs               (~400) Turn lifecycle: TurnRequest, TurnChannels, drain/cancel
├── input.rs              (~550) Input event routing + keybinding dispatch
├── keybinding_priority.rs(~100) 4 ACTION_PRIORITY const arrays
├── prompt_edit.rs        (~350) Prompt editing + paste/marker handling
├── prompt_completion.rs  (~650) CompletionCatalog/Candidate/Source + completion engine
├── slash_commands.rs     (~200) Slash command dispatch
├── command_palette.rs    (~250) Command palette specs + run_*_command
├── approval.rs           (~250) Approval decision routing
├── questions.rs          (~100) AskUser reverse RPC
├── mode_state.rs         (~450) Permission/plan/goal mode state machine
├── git_status.rs         (~180) GitStatusBadge + parse_git_* functions
├── sessions.rs           (~400) Session load/fork/rebuild
├── btw_sidecar.rs        (~250) /btw sidecar management
├── mcp_manager.rs        (~450) MCP manager interaction
├── catalog_fetch.rs      (~400) Provider catalog add/remove
├── model_picker.rs       (~400) Picker catalog + model entries
├── clipboard.rs          (~120) System clipboard adapter
├── terminal_io.rs        (~200) NeoTerminal + RawStdinEvents
├── snapshot.rs           (~80)  Snapshot rendering for tests
├── prompt_history.rs     (~40)  Prompt history persistence
├── log_events.rs         (~40)  Log event drain
├── startup.rs            (~150) Startup action + trust dialog
└── tests/                (~8000) All 206 tests split by domain
```

### Strategy

This is the largest mechanical refactor. The approach:

1. **First pass:** Convert `interactive.rs` to `interactive/mod.rs` (no code changes, just file move). Build to verify.
2. **Second pass:** Extract leaf modules (no dependencies on other interactive code): `git_status`, `clipboard`, `keybinding_priority`, `terminal_io`, `snapshot`, `prompt_history`, `log_events`. Each is a separate commit.
3. **Third pass:** Extract mid-level modules: `prompt_completion`, `prompt_edit`, `mode_state`, `approval`, `questions`.
4. **Fourth pass:** Extract high-level modules: `turn`, `input`, `slash_commands`, `command_palette`, `mcp_manager`, `catalog_fetch`, `model_picker`, `btw_sidecar`, `sessions`.
5. **Fifth pass:** Extract tests into `tests/` submodules.

---

### Task 3.1: Convert `interactive.rs` to `interactive/mod.rs`

- [ ] **Step 1: Move the file**

```bash
mkdir -p crates/neo-agent/src/modes/interactive
git mv crates/neo-agent/src/modes/interactive.rs crates/neo-agent/src/modes/interactive/mod.rs
```

- [ ] **Step 2: Update `modes/mod.rs`** — should already have `pub mod interactive;` (was `pub mod interactive;` when it was a file). No change needed if it was `pub mod interactive;`.

- [ ] **Step 3: Build** — `cargo build -p neo-agent`

- [ ] **Step 4: Commit**

---

### Task 3.2: Extract `interactive/git_status.rs` (leaf module)

**Source:** Lines 143–298 in `mod.rs` — `GitStatusBadge` struct + `format()` + `git_status_label` + `git_status_label_with_program` + `parse_git_status_porcelain` + `parse_git_branch_header` + `parse_git_sync_count` + `parse_git_numstat` + `parse_git_numstat_count` + `event_should_refresh_git_status` + `refresh_git_status_now` + `refresh_git_status_if_due`.

- [ ] **Step 1: Read lines 143–298, move to `git_status.rs`**

- [ ] **Step 2: Make items `pub(super)` or `pub(crate)`, add `use` imports**

- [ ] **Step 3: Update `mod.rs`** — add `mod git_status;`, remove moved code, update call sites with `git_status::GitStatusBadge` etc.

- [ ] **Step 4: Build + test + commit**

---

### Task 3.3: Extract remaining leaf modules

Extract each as a separate commit, following the same pattern as Task 3.2:

- [ ] **`interactive/keybinding_priority.rs`** — 4 `const &[KeybindingAction]` arrays (~98 lines). Pure static data.

- [ ] **`interactive/clipboard.rs`** — `write_system_clipboard`, `clipboard_commands`, `spawn_clipboard_command`, etc. (~70 lines). Pure OS adapter.

- [ ] **`interactive/terminal_io.rs`** — `NeoTerminal` + `TerminalEvents` trait + `RawStdinEvents` (~200 lines).

- [ ] **`interactive/snapshot.rs`** — `compose_tui_frame`, `render_transcript_snapshot`, `render_overlay_snapshot`, `render_picker_snapshot` (~64 lines).

- [ ] **`interactive/prompt_history.rs`** — `load_prompt_history`, `append_prompt_history` (~40 lines).

- [ ] **`interactive/log_events.rs`** — `set_log_event_receiver`, `drain_log_events` (~40 lines).

- [ ] **`interactive/startup.rs`** — `StartupAction`, `InteractiveOptions`, `apply_startup_action`, trust dialog logic (~150 lines).

After each extraction: build + test + commit.

---

### Task 3.4: Extract `interactive/prompt_completion.rs`

**Source:** The completion engine (~650 lines) — `CompletionCatalog`, `CompletionCandidate`, `CompletionSource`, `FilesystemCompletionRequest`, `prompt_completions`, `filesystem_completion_candidates`, `model_completion_candidates`, `slash_prompt_template_completion_items`, `prompt_package_completion_items`, `extension_command_completion_items`, `discover_extension_commands`, `STATIC_SLASH_COMMANDS`.

- [ ] **Step 1: Read the relevant line ranges, move to `prompt_completion.rs`**

- [ ] **Step 2: Make items `pub(super)`, fix imports**

- [ ] **Step 3: Build + test + commit**

---

### Task 3.5: Extract `interactive/prompt_edit.rs`

**Source:** `apply_prompt_edit`, `handle_prompt_edit_event`, `prompt_edit_for_action` series, `clean_pasted_text`, `handle_paste_text`, `expand_marker_at_cursor`, `handle_paste_image`, `PromptSubmission` + impl (~350 lines).

- [ ] Move, fix imports, build + test + commit.

---

### Task 3.6: Extract `interactive/mode_state.rs`

**Source:** `set_permission_mode`, `open_permission_picker`, `permission_mode_items`, `set_plan_mode_from_user`, `sync_runtime_plan_mode`, `cycle_development_mode`, `handle_goal_command` full family (~15 functions), `request_manual_compaction` (~450 lines).

- [ ] Move, fix imports, build + test + commit.

---

### Task 3.7: Extract `interactive/approval.rs` + `interactive/questions.rs`

**approval.rs source:** `PendingApprovalResponse`, `resolve_approval`, `approval_decision`, `dispatch_approval_response`, `reject_all_pending_approvals`, etc. (~250 lines).

**questions.rs source:** `register_pending_question`, `resolve_question`, `background_question_followup_prompt` (~100 lines).

- [ ] Extract both (separate commits), fix imports, build + test + commit.

---

### Task 3.8: Extract `interactive/turn.rs`

**Source:** `TurnRequest`, `TurnOutcome`, `TurnChannels`, `RunningTurn`, `start_turn_with_prompt`, `drain_active_turn`, `cancel_active_turn`, `wait_for_active_turn` (~400 lines).

- [ ] Move, fix imports, build + test + commit.

---

### Task 3.9: Extract `interactive/input.rs`

**Source:** `handle_input_event`, `handle_keybinding_key`, `handle_keybinding_action`, `handle_basic/overlay/prompt/transcript_keybinding_action`, `dialog_input_event`, `handle_cancel/interrupt_input`, `ExitGesture`, `ExitConfirmation` (~550 lines).

- [ ] Move, fix imports, build + test + commit.

---

### Task 3.10: Extract remaining high-level modules

Each as a separate commit:
- `interactive/slash_commands.rs` (~200 lines)
- `interactive/command_palette.rs` (~250 lines)
- `interactive/mcp_manager.rs` (~450 lines)
- `interactive/catalog_fetch.rs` (~400 lines)
- `interactive/model_picker.rs` (~400 lines)
- `interactive/btw_sidecar.rs` (~250 lines)
- `interactive/sessions.rs` (~400 lines)
- `interactive/image_capabilities.rs` (~50 lines)

---

### Task 3.11: Extract tests into `interactive/tests/`

- [ ] **Step 1: Move the `#[cfg(test)] mod tests` block** (lines ~6702–14799, ~8,098 lines) into a `tests/` subdirectory or inline `#[cfg(test)]` modules in each domain file.

Split tests by domain:
- `tests/git_status.rs` — git status parsing tests
- `tests/prompt_completion.rs` — completion engine tests
- `tests/approval.rs` — approval routing tests
- `tests/slash_commands.rs` — slash command tests
- `tests/turn.rs` — turn lifecycle tests
- etc.

- [ ] **Step 2: Update `mod.rs`** — remove inline test module, add `#[cfg(test)] mod tests;` pointing to the directory.

- [ ] **Step 3: Build + test + commit**

```bash
cargo run -p xtask -- test -p neo-agent
git commit -am "refactor(neo-agent): extract interactive tests into domain test modules"
```

---

### Task 3.12: Finalize `interactive/mod.rs`

- [ ] **Step 1: Verify `mod.rs`** contains only: `execute_with_startup`, `execute_tty_with_startup`, `controller_for_config`, `StartupAction`, `InteractiveOptions`, module declarations, and `pub use` re-exports.

- [ ] **Step 2: Verify `interactive/controller.rs`** contains `InteractiveController` struct + main loop only (~600 lines).

- [ ] **Step 3: Full build + test + clippy**

```bash
cargo run -p xtask -- test -p neo-agent
cargo run -p xtask -- check
```

- [ ] **Step 4: Commit**

---

<!-- ==================================================================== -->
<!-- PHASE 4: neo-agent run.rs + config.rs split -->
<!-- ==================================================================== -->

# Phase 4: Split `run.rs` (3,222 lines) + `config.rs` (1,268 lines)

**Goal:** Break `run.rs` into domain modules and consolidate `config.rs`/`config_ops.rs` into a `config/` directory.

**Crate:** `crates/neo-agent`

### Target structure for `run.rs` split

```
crates/neo-agent/src/
├── modes/
│   └── run.rs             (~500) Only execute engine: execute/run_prompt*/streaming
├── runtime/
│   ├── mod.rs             (~10)  Re-exports
│   ├── agent.rs           (~300) agent_config_for_app, tool_registry_for_config, build_mcp_client
│   ├── model.rs           (~350) resolve_model, model_registry_for_config, select_config_model, resolve_model_client
│   └── session.rs         (~200) create_session_path, session_id_from_path, latest_session_id, generate_session_title
├── output/
│   ├── mod.rs             (~10)
│   ├── json.rs            (~500) StableJsonState + stable_* functions
│   └── models_cli.rs      (~130) list_configured_models + helpers
├── mcp/
│   ├── mod.rs             (~10)
│   ├── cli.rs             (~200) list_mcp, add_mcp_server, auth_mcp_server, probe_mcp_server
│   └── (mcp_ops.rs moves here too)
```

### Target structure for `config/` directory

```
crates/neo-agent/src/
├── config/
│   ├── mod.rs             (~400) Type definitions: AppConfig, FileConfig, RuntimeConfig, etc.
│   ├── types.rs           (~250) ProviderConfig, ModelConfig, McpServerConfig, McpTransport
│   ├── loader.rs          (~300) AppConfig::load, read/write_file_config, validate_*
│   ├── paths.rs           (~60)  neo_home, expand_user_path, user_home, workspace_sessions_dir
│   ├── match.rs           (~120) scoped_models, wildcard_*, fuzzy_match
│   └── mutations.rs       (~680) config_ops.rs content + mcp writes from config.rs
```

---

### Task 4.1: Convert `config.rs` to `config/` directory

- [ ] **Step 1: Create directory, move file**

```bash
mkdir -p crates/neo-agent/src/config
git mv crates/neo-agent/src/config.rs crates/neo-agent/src/config/mod.rs
```

- [ ] **Step 2: Build** — verify it still compiles (Rust resolves `config/mod.rs` the same as `config.rs`)

- [ ] **Step 3: Commit**

---

### Task 4.2: Extract `config/paths.rs`

- [ ] **Step 1: Move path utility functions** (lines 917–968: `neo_home`, `default_config_path`, `global_prompts_dir`, `workspace_sessions_dir`, `expand_user_path`, `expand_user_path_with_home`, `user_home`) to `paths.rs`

- [ ] **Step 2: Make functions `pub(crate)`, add `mod paths;` to `config/mod.rs`, update call sites**

Call sites: search for `config::neo_home`, `config::expand_user_path`, etc. These should still work via re-export.

Add to `config/mod.rs`: `pub(crate) use paths::*;` or have callers use `config::paths::neo_home`.

- [ ] **Step 3: Build + test + commit**

---

### Task 4.3: Extract `config/match.rs`

- [ ] **Step 1: Move matching functions** (lines 85–197: `scoped_models`, `model_matches_scope_pattern`, `strip_thinking_suffix`, `has_glob_meta`, `wildcard_match`, `wildcard_initial_row`, `wildcard_advance_row`, `wildcard_cell_matches`, `fuzzy_match`) to `match.rs`

Note: `match` is a reserved word in Rust. Use `config/matching.rs` instead.

- [ ] **Step 2: Make functions `pub(crate)`, add `mod matching;`, update call sites**

- [ ] **Step 3: Build + test + commit**

---

### Task 4.4: Extract `config/loader.rs`

- [ ] **Step 1: Move load + validate functions** (lines 503–895: `AppConfig::load`, `default_model_label`, `runtime_from_file`, `tui_from_file`, `validate_runtime_config`, `validate_tui_config`, `validate_tui_context_conflicts`, `context_actions_for_key`, TUI action constants, `TuiConfig::keybinding_overrides`, `read_file_config`, `write_file_config`, `resolve_project_trust_state`) to `loader.rs`

- [ ] **Step 2: These reference config types** — import from `super::*` or specific paths

- [ ] **Step 3: Build + test + commit**

---

### Task 4.5: Merge `config_ops.rs` + MCP writes into `config/mutations.rs`

- [ ] **Step 1: Move `config_ops.rs` content to `config/mutations.rs`**

- [ ] **Step 2: Move MCP write functions** from `config/mod.rs` (lines 662–711: `upsert_mcp_server`, `remove_mcp_server`, `set_mcp_server_enabled`) to `mutations.rs`

- [ ] **Step 3: Delete `config_ops.rs`**, update `main.rs`/`cli.rs` references from `crate::config_ops` to `crate::config::mutations`

- [ ] **Step 4: Build + test + commit**

---

### Task 4.6: Extract `run.rs` — `output/json.rs`

- [ ] **Step 1: Create `output/` directory**

- [ ] **Step 2: Move JSON serialization code** (lines 72–586: `stable_json_output`, `write_json_line`, `StableJsonState` + impl + all `stable_*` functions, `AssistantContentState`) to `output/json.rs`

- [ ] **Step 3: Make items `pub(crate)`, fix imports in `run.rs`**

- [ ] **Step 4: Build + test + commit**

---

### Task 4.7: Extract `run.rs` — `output/models_cli.rs`

- [ ] **Step 1: Move model listing functions** (lines 592–715: `list_configured_models`, `ConfiguredModelEntry`, all `configured_model*` functions) to `output/models_cli.rs`

- [ ] **Step 2: Build + test + commit**

---

### Task 4.8: Extract `run.rs` — `runtime/agent.rs` + `runtime/model.rs`

- [ ] **Step 1: Create `runtime/` directory**

- [ ] **Step 2: Move `agent_build` functions** (lines 1649–1938: `agent_config_for_app`, `attach_async_approval_handler`, `tool_registry_for_config`, `wait_for_mcp_manager_probe`, `build_mcp_client`, `effective_compaction_max_estimated_tokens`, compaction constants) to `runtime/agent.rs`

**Important:** These are `pub(crate)` and used by `btw.rs` and `interactive.rs`. The import path changes from `crate::modes::run::agent_config_for_app` to `crate::runtime::agent::agent_config_for_app`. Update all call sites:
- `btw.rs:23` — `use crate::modes::run::{agent_config_for_app, tool_registry_for_config}`
- `interactive.rs` — multiple references via `crate::modes::run::`

- [ ] **Step 3: Move `model_resolve` functions** (lines 717–794, 2040–2302: `provider_registry_for_config`, `resolve_provider_credential*`, `apply_configured_provider_overrides`, `resolve_model`, `model_registry_for_config`, `select_config_model`, `find_default_model`, `model_spec_matches_default`, `model_config_to_spec`, `parse_model_capabilities`, `resolve_model_client`) to `runtime/model.rs`

**Important:** Same import path change for callers. `interactive.rs` references: `resolve_model`, `resolve_model_client`, `model_registry_for_config`, `select_config_model`.

- [ ] **Step 4: Build + test + commit**

---

### Task 4.9: Extract `run.rs` — `runtime/session.rs`

- [ ] **Step 1: Move session management functions** (lines 1940–2143: `create_session_path`, `session_id_from_path`, `latest_session_id`, `record_session_activity`, `record_initial_session_title`, `generate_session_title`, `clean_session_title`, `one_line`) to `runtime/session.rs`

**Important:** `latest_session_id` is `pub(crate)` — update import in `interactive.rs`.

- [ ] **Step 2: Build + test + commit**

---

### Task 4.10: Extract `run.rs` — `mcp/cli.rs`

- [ ] **Step 1: Create `mcp/` directory**

- [ ] **Step 2: Move MCP CLI functions** (lines 796–978: `list_mcp`, `list_mcp_tools_for_server`, `add_mcp_server`, `auth_mcp_server`, `probe_mcp_server`, `apply_tool_filter`, `key_value_pairs`) to `mcp/cli.rs`

**Important:** Delete the duplicate `key_value_pairs` in `run.rs:1938` and use the one from `mcp_ops.rs:96` instead (eliminating the duplicate identified in the audit).

- [ ] **Step 3: Move `mcp_ops.rs` to `mcp/ops.rs`** for consolidation

- [ ] **Step 4: Build + test + commit**

---

### Task 4.11: Finalize `run.rs`

- [ ] **Step 1: Verify `run.rs`** now contains only: `execute`, `run_prompt*`, `run_prompt_streaming*`, `prepare_*_streaming_turn`, `finish_prompt_turn*`, `SessionEventWriter`, `StreamingTurnIO`, `PreparedStreamingTurn`, `StreamingEventEffect`, stream utils (~500 lines)

- [ ] **Step 2: Full build + test**

```bash
cargo run -p xtask -- test -p neo-agent
cargo run -p xtask -- check
```

- [ ] **Step 3: Commit**

---

<!-- ==================================================================== -->
<!-- PHASE 5: neo-tui large file splits -->
<!-- ==================================================================== -->

# Phase 5: Split neo-tui large files

**Goal:** Break four neo-tui god-files into focused modules and fix the theme dependency inversion.

**Crate:** `crates/neo-tui`

### Target structure

```
crates/neo-tui/src/
├── shell/
│   ├── mod.rs              (~100) Module declarations + pub use + NeoChromeState struct
│   ├── state.rs            (~250) NeoChromeState impl: accessors + new
│   ├── event_router.rs     (~220) apply_agent_event + apply_stream_update
│   ├── overlay.rs           → rename from existing overlay.rs (stays)
│   ├── dialog_factory.rs   (~240) open_* dialog creation methods
│   ├── input_dispatch.rs   (~350) handle_focused_dialog_input + translate_key + result handlers
│   └── approval.rs          → existing (stays, may absorb pane.rs approval methods)
├── transcript/
│   ├── mod.rs              (stays)
│   ├── pane.rs             (~600) TranscriptPane: viewport + event append only
│   ├── chrome_render.rs    (~520) render_chrome_lines, render_prompt_lines, etc.
│   ├── approval_data.rs    (~280) approval_prompt, shell_approval_details, etc.
│   ├── entry/
│   │   ├── mod.rs          (~300) TranscriptEntry enum + constructors + render dispatch
│   │   ├── copy.rs         (~210) copy_parts functions
│   │   ├── render_banner.rs(~85)
│   │   ├── render_thinking.rs (~80)
│   │   ├── render_goal.rs  (~130)
│   │   ├── render_skill.rs (~40)
│   │   └── render_status.rs(~140)
│   └── (other files stay)
├── screen_output/
│   ├── mod.rs              (stays)
│   ├── frame_differ.rs     (~800) Core diff algorithm only
│   ├── kitty_image.rs      (~160) Kitty image lifecycle management
│   ├── raw_mode.rs         (~110) Enable/disable raw mode + Drop
│   └── debug_log.rs        (~200) Debug/crash log file I/O
└── primitive/
    ├── theme.rs            (~300) MOVED from shell/theme.rs — TuiTheme + ChromeMode + DevelopmentMode
    └── (other files stay)
```

---

### Task 5.1: Move `shell/theme.rs` → `primitive/theme.rs`

This must happen first because it fixes the dependency inversion (transcript/widgets currently depend on `shell::TuiTheme`).

- [ ] **Step 1: Read `shell/theme.rs` (302 lines)** — contains `TuiTheme`, `ChromeMode`, `DevelopmentMode`, `GoalModeStatus`

- [ ] **Step 2: Move to `primitive/theme.rs`**

```bash
git mv crates/neo-tui/src/shell/theme.rs crates/neo-tui/src/primitive/theme.rs
```

- [ ] **Step 3: Update `shell/mod.rs`** — change `mod theme;` to `pub use crate::primitive::theme::*;` (or specific re-exports to maintain backward compat)

- [ ] **Step 4: Update `primitive/mod.rs`** — add `pub mod theme;`

- [ ] **Step 5: Fix all `use crate::shell::theme::*` / `use crate::shell::TuiTheme`** references → `use crate::primitive::theme::*`

Search: `grep -rn "shell::.*TuiTheme\|shell::.*ChromeMode\|shell::.*DevelopmentMode\|shell::.*GoalModeStatus\|shell::theme" crates/neo-tui/src/`

- [ ] **Step 6: Build + test + commit**

---

### Task 5.2: Split `shell/mod.rs` — extract `state.rs` + `event_router.rs`

- [ ] **Step 1: Extract `shell/state.rs`**

Move `NeoChromeState` struct definition (lines 50–89) + `new()` (91–130) + all pure accessor methods (132–474: `title`, `session_label`, `model_label`, `workspace_root`, `context_window`, `working_label`, `permission_mode`, `set_*`, `theme`, `prompt`, `copy_buffer`, etc.).

Add `mod state;` to `shell/mod.rs`. Keep `pub use state::NeoChromeState;`.

- [ ] **Step 2: Extract `shell/event_router.rs`**

Move `apply_stream_update` (476–503) + `apply_agent_event` (505–695) — ~220 lines of event routing logic.

These are methods on `NeoChromeState`. In the new file, implement as `impl NeoChromeState { ... }` (Rust allows impl blocks in multiple files for the same type, as long as they're in the same crate).

- [ ] **Step 3: Build + test + commit**

---

### Task 5.3: Split `shell/mod.rs` — extract `dialog_factory.rs` + `input_dispatch.rs`

- [ ] **Step 1: Extract `shell/dialog_factory.rs`**

Move all `open_*` methods (772–1010, 1370–1380): `request_approval`, `open_command_palette`, `open_session_picker`, `open_model_picker`, `open_prompt_completion_picker`, `open_tabbed_model_selector`, `open_provider_manager`, `open_mcp_manager`, `open_choice_picker`, `open_api_key_input`, `open_custom_registry_import`, `open_mcp_add_form`, `open_trust_dialog`, `push_task_browser_overlay`, `push_question_overlay`.

- [ ] **Step 2: Extract `shell/input_dispatch.rs`**

Move input dispatch methods (1012–1454, 1456–1597): `handle_focused_dialog_input`, `translate_key_event_for_dialog`, `model_selector_result`, `tabbed_model_selector_result`, `provider_manager_action`, `mcp_manager_action`, `choice_picker_result`, `api_key_input_result`, `text_input_result`, `custom_registry_import_result`, `mcp_add_form_result`, `take_question_result`, `handle_question_dialog_key`, `confirm_question`, `cancel_question`, `move_overlay_selection_*`, `task_browser_state*`.

- [ ] **Step 3: Build + test + commit**

---

### Task 5.4: Split `transcript/entry.rs` → `entry/` subdirectory

- [ ] **Step 1: Convert `entry.rs` to `entry/mod.rs`**

```bash
mkdir -p crates/neo-tui/src/transcript/entry
git mv crates/neo-tui/src/transcript/entry.rs crates/neo-tui/src/transcript/entry/mod.rs
```

- [ ] **Step 2: Extract `entry/copy.rs`**

Move all `copy_*` functions (399–532, 1079–1135): `copy_parts`, `complex_copy_parts`, `utility_copy_parts`, `card_copy_parts`, `simple_copy_parts`, `text_copy_parts`, `status_copy_parts`, `media_copy_parts`, `copy_banner`, `copy_tool`, `copy_compaction`, `copy_goal`, `copy_skill`.

- [ ] **Step 3: Extract render functions by variant**

Each render function group moves to its own file:
- `entry/render_banner.rs` — `render_welcome_banner` (834–918)
- `entry/render_thinking.rs` — `render_thinking`, `render_thinking_block`, `thinking_spinner`, `thinking_style` (600–655, 958–976, 1315–1317)
- `entry/render_goal.rs` — `render_goal_card`, `GoalCardChrome`, `goal_card_*` (1137–1268)
- `entry/render_skill.rs` — `render_skill_used`, `skill_body` (1270–1309)
- `entry/render_status.rs` — `render_status`, `render_compaction`, `compaction_pulse_char`, `severity_color`, `status_style` (568–574, 937–948, 978–1077, 1311–1313)
- Remaining render helpers stay in `entry/mod.rs` or `entry/render_common.rs`

- [ ] **Step 4: Build + test + commit** after each sub-extraction

---

### Task 5.5: Split `transcript/pane.rs` — extract `chrome_render.rs` + `approval_data.rs`

- [ ] **Step 1: Extract `transcript/chrome_render.rs`**

Move chrome rendering functions (lines 40–50, 1095–1150, 1223–1709): `apply_gutter`, `render_ordered_tools`, `is_groupable`, `ChromeRender` struct, `render_chrome_lines`, `render_chrome_lines_mut`, `render_footer_only_lines`, `frame_content_width`, `render_prompt_completion_dropdown`, `render_prompt_lines`, `build_prompt_logical_lines`, `scroll_indicator_*_border`, `expand_prompt_tabs`, `find_cursor`, `render_footer_lines`, `development_mode_badge`, `render_git_status_label`, `render_git_status_part`.

These are free functions (not methods on `TranscriptPane`), so they move cleanly.

- [ ] **Step 2: Extract `transcript/approval_data.rs`**

Move approval data extraction functions (lines 594–770, 877–1069): `select_approval`, `resolve_approval`, `resolve_unresolved_approvals`, `upsert_approval`, `active_approval_mut`, `update_active_approval_queue_count`, `advance_queued_approval`, `ApprovalPromptSummary`, `approval_prompt`, `shell_approval_details`, `terminal_approval_title`, `terminal_approval_details`, `labeled_argument`, `compact_details`, `non_empty_details`.

Note: some of these are methods on `TranscriptPane`. Use multi-file `impl` blocks.

- [ ] **Step 3: `pane.rs`** now contains only viewport + event append logic (~600 lines)

- [ ] **Step 4: Build + test + commit**

---

### Task 5.6: Split `screen_output/frame_differ.rs` — extract debug_log + kitty_image + raw_mode

- [ ] **Step 1: Extract `screen_output/debug_log.rs`**

Move debug I/O functions (lines 51–187, 632–686): `debug_log_enabled`, `write_output_log`, `create_debug_log_file`, `debug_log_path`, `write_rendered_lines`, `write_debug_log`, `write_debug_log_lines`, `write_debug_log_header`, `write_optional_debug_text`, `write_width_crash_log`, `write_width_crash_body`, `write_width_crash_header`, `log_render_start`, `log_diff_render`.

These are free functions or methods on `TuiRenderer`. Free functions move cleanly; methods use multi-file `impl`.

- [ ] **Step 2: Extract `screen_output/kitty_image.rs`**

Move kitty image functions (lines 1034–1117, 1196–1266): `is_image_line`, `collect_kitty_image_ids`, `extract_kitty_image_ids`, `extract_kitty_image_rows`, `get_kitty_image_reserved_rows`, `delete_kitty_images`, `reserved_render_rows`, `image_block_fits`, `push_image_block`, `expand_changed_range_for_kitty_images`, `delete_changed_kitty_images`.

- [ ] **Step 3: Extract `screen_output/raw_mode.rs`**

Move raw mode functions (lines 55–68, 376–465, 1269–1273): `is_termux_session`, `hardware_cursor_enabled_from_env_value`, `TuiRenderer::enter`, `TuiRenderer::leave`, `TuiRenderer::write_leave_output`, `TuiRenderer::suspend_prepare`, `TuiRenderer::suspend_resume`, `impl Drop for TuiRenderer`.

- [ ] **Step 4: `frame_differ.rs`** now contains only the core diff algorithm (~800 lines)

- [ ] **Step 5: Build + test + commit**

---

### Task 5.7: Move generic widgets from `shell/` to `widgets/`

- [ ] **Step 1: Move `shell/select_list.rs` → `widgets/select_list.rs`**

- [ ] **Step 2: Move `shell/pickers.rs` → `widgets/pickers.rs`**

- [ ] **Step 3: Move `shell/command_palette.rs` → `widgets/command_palette.rs`**

- [ ] **Step 4: Update `widgets/mod.rs`** + update `shell/mod.rs` re-exports

- [ ] **Step 5: Build + test + commit**

---

<!-- ==================================================================== -->
<!-- PHASE 6: Cross-cutting cleanup -->
<!-- ==================================================================== -->

# Phase 6: Cross-cutting cleanup

**Goal:** Eliminate remaining code duplication and naming inconsistencies across crates.

---

### Task 6.1: Unify token estimation functions

**Problem:** `estimate_messages_tokens` / `estimate_message_tokens` / `estimate_content_chars` are duplicated in:
- `neo-agent-core/src/runtime.rs` (lines 2361–2436) — already extracted in Phase 2 to `runtime/tokens.rs`
- `neo-agent-core/src/compaction/mod.rs` (lines 506–540)
- `neo-agent-core/src/session/mod.rs` (lines 758–762)

- [ ] **Step 1: After Phase 2 is complete, `runtime/tokens.rs` has the canonical implementation**

- [ ] **Step 2: Replace `compaction/mod.rs` local token estimation** with import from `crate::runtime::tokens`

- [ ] **Step 3: Replace `session/mod.rs` local token estimation** with import from `crate::runtime::tokens`

- [ ] **Step 4: Build + test**

```bash
cargo run -p xtask -- test -p neo-agent-core
```

- [ ] **Step 5: Commit**

---

### Task 6.2: Consolidate plan-related code

**Problem:** Plan mode logic is scattered across 4 locations in neo-agent-core:
1. `tools/plan_mode.rs` (364 lines) — `EnterPlanMode`/`ExitPlanMode` tools
2. `mode/plan.rs` (266 lines) — `PlanMode` state machine
3. `mode/plan_mode_guard.rs` (156 lines) — write-protection guard
4. `injection/plan_mode.rs` (77 lines) — thin wrapper injector

Plus ~200 lines of plan orchestration in `runtime.rs` (extracted to `runtime/plan_orchestration.rs` in Phase 2).

- [ ] **Step 1: Merge `injection/plan_mode.rs` into `mode/plan.rs`**

The injector (77 lines) is a thin wrapper around `PlanMode` logic. Move its content into `mode/plan.rs` and delete the `injection/` directory.

- [ ] **Step 2: Unify naming** — rename `mode/plan.rs` to `mode/plan_mode.rs` for consistency with `tools/plan_mode.rs` and `mode/plan_mode_guard.rs`

- [ ] **Step 3: Delete `injection/mod.rs`** and remove `pub mod injection;` from `lib.rs`

- [ ] **Step 4: Build + test + commit**

---

### Task 6.3: Delete dead code — `session/jsonl.rs`

**Problem:** `session/jsonl.rs` (28 lines) provides `encode_event`/`decode_event`/`append_event` that are a subset of what `session/mod.rs`'s `JsonlSessionWriter`/`JsonlSessionReader` already do.

- [ ] **Step 1: Grep for usages of `jsonl::`** in the crate

```bash
grep -rn "jsonl::" crates/neo-agent-core/src/ crates/neo-agent/src/
```

- [ ] **Step 2: If no external usages, delete `session/jsonl.rs`**

- [ ] **Step 3: Remove `mod jsonl;` from `session/mod.rs`**

- [ ] **Step 4: Build + test + commit**

---

### Task 6.4: Consolidate `oauth.rs` into `oauth/mod.rs`

**Problem:** `oauth.rs` (55 lines, top-level) is a forward file that defines `OAuthTokenSet`/`OAuthError` then declares `pub mod callback_server; pub mod store;`. This is inconsistent with other subdirectories (`session/`, `compaction/`) which all use `mod.rs`.

- [ ] **Step 1: Move content of `oauth.rs` into `oauth/mod.rs`**

```bash
cat crates/neo-agent-core/src/oauth.rs > crates/neo-agent-core/src/oauth/mod.rs
git rm crates/neo-agent-core/src/oauth.rs
```

- [ ] **Step 2: Build** — Rust resolves `oauth/mod.rs` identically to `oauth.rs`. No import changes needed.

- [ ] **Step 3: Commit**

---

### Task 6.5: Eliminate duplicate constants and functions

**Problem:** Two duplicates identified in audit:

1. `CONTEXT_FILE_CANDIDATES` — defined in both `trust.rs:11` and `resources.rs:24`
2. `key_value_pairs` — defined in both `mcp_ops.rs:96` and `run.rs:1938` (deleted in Phase 4 Task 4.10)

- [ ] **Step 1: For `CONTEXT_FILE_CANDIDATES`** — make it `pub(crate) const` in `trust.rs`, update `resources.rs` to import from `trust::`

- [ ] **Step 2: For `key_value_pairs`** — already handled in Phase 4 (Task 4.10 deletes the `run.rs` copy)

- [ ] **Step 3: Build + test + commit**

---

### Task 6.6: Rename `rpc_mode.rs` → `rpc/server.rs`

**Problem:** `rpc_mode.rs` is misleadingly named — it's a JSONL RPC server/handler, not a "mode".

- [ ] **Step 1: Create `rpc/` directory**

```bash
mkdir -p crates/neo-agent/src/rpc
git mv crates/neo-agent/src/rpc_mode.rs crates/neo-agent/src/rpc/server.rs
git mv crates/neo-agent/src/rpc_types.rs crates/neo-agent/src/rpc/types.rs
```

- [ ] **Step 2: Create `rpc/mod.rs`**

```rust
pub mod server;
pub mod types;
```

- [ ] **Step 3: Update all references** from `crate::rpc_mode` / `crate::rpc_types` to `crate::rpc::server` / `crate::rpc::types`

- [ ] **Step 4: Build + test + commit**

---

### Task 6.7: Group prompt files into `prompt/` directory

**Problem:** `prompt_templates.rs`, `prompt_history.rs`, `prompt_parts.rs` are flat files that should be grouped.

- [ ] **Step 1: Create directory and move files**

```bash
mkdir -p crates/neo-agent/src/prompt
git mv crates/neo-agent/src/prompt_templates.rs crates/neo-agent/src/prompt/templates.rs
git mv crates/neo-agent/src/prompt_history.rs crates/neo-agent/src/prompt/history.rs
git mv crates/neo-agent/src/prompt_parts.rs crates/neo-agent/src/prompt/parts.rs
```

- [ ] **Step 2: Create `prompt/mod.rs`** with module declarations

- [ ] **Step 3: Update all references** from `crate::prompt_templates` etc. to `crate::prompt::templates` etc.

- [ ] **Step 4: Build + test + commit**

---

### Task 6.8: Final full verification

- [ ] **Step 1: Full workspace build**

```bash
cargo build -p neo-agent
```

- [ ] **Step 2: Full workspace test**

```bash
cargo run -p xtask -- test --workspace
```

- [ ] **Step 3: Full CI check**

```bash
cargo run -p xtask -- check
```

- [ ] **Step 4: Verify line counts improved**

```bash
find crates -path '*/src/*.rs' -not -path '*/target/*' | xargs wc -l | sort -rn | head -20
```

Expected: No file > 1000 lines (except maybe `controller.rs` in interactive). The top 5 files should each be < 800 lines.

- [ ] **Step 5: Commit any final fixups**

---

## Self-Review

### Spec coverage

The audit identified these problem categories. Each maps to a phase:

| Problem | Phase | Status |
|---------|-------|--------|
| neo-ai provider duplication (~800 lines) | Phase 1 | ✅ 7 tasks |
| runtime.rs god-file (4,623 lines) | Phase 2 | ✅ 11 tasks |
| interactive.rs god-file (16,639 lines) | Phase 3 | ✅ 12 tasks |
| run.rs god-file (3,222 lines) | Phase 4 | ✅ 6 tasks |
| config.rs / config_ops.rs boundary | Phase 4 | ✅ 5 tasks |
| shell/mod.rs god-object (1,667 lines) | Phase 5 | ✅ 3 tasks |
| transcript/pane.rs mixed (1,782 lines) | Phase 5 | ✅ 1 task |
| transcript/entry.rs mixed (1,528 lines) | Phase 5 | ✅ 1 task |
| frame_differ.rs mixed (1,675 lines) | Phase 5 | ✅ 1 task |
| theme.rs dependency inversion | Phase 5 | ✅ Task 5.1 |
| token estimation 3x duplication | Phase 6 | ✅ Task 6.1 |
| plan logic scattered (4 locations) | Phase 6 | ✅ Task 6.2 |
| session/jsonl.rs dead code | Phase 6 | ✅ Task 6.3 |
| oauth.rs layout inconsistency | Phase 6 | ✅ Task 6.4 |
| CONTEXT_FILE_CANDIDATES / key_value_pairs duplication | Phase 6 | ✅ Task 6.5 |
| rpc_mode.rs naming | Phase 6 | ✅ Task 6.6 |
| prompt_* files flat | Phase 6 | ✅ Task 6.7 |
| shell/ generic widgets in wrong dir | Phase 5 | ✅ Task 5.7 |

### Placeholder scan

No `TODO`, `TBD`, or "implement later" markers in task steps. Code blocks show actual signatures and logic patterns. Where existing code must be read (e.g., "Read `openai_responses.rs:29-47`"), exact line ranges are given.

### Type consistency

- `ProviderError` — defined in Phase 1 Task 1.1, used in Tasks 1.2–1.5 with consistent variant names
- `SsePayloadParser` — defined in Phase 1 Task 1.5, implemented with consistent method signatures across all 4 providers
- `AgentConfig` — defined in Phase 2 Task 2.6, re-exported in `mod.rs`, used by permission/compaction/tool_dispatch modules with `super::config::AgentConfig`
- `NeoChromeState` — defined in Phase 5 Task 5.2 in `state.rs`, impl blocks extended in `event_router.rs`, `dialog_factory.rs`, `input_dispatch.rs` via multi-file `impl`
- `TranscriptEntry` — defined in Phase 5 Task 5.4 in `entry/mod.rs`, render functions in submodules reference it via `super::TranscriptEntry`
- `TuiTheme` — moved to `primitive/theme.rs` in Phase 5 Task 5.1, re-exported from `shell/mod.rs` for backward compat

### Execution order recommendation

1. **Phase 1** (neo-ai) — zero dependencies, lowest risk
2. **Phase 6 Tasks 6.3–6.5** (dead code, duplicates) — can run immediately, lowest risk
3. **Phase 2** (runtime.rs) — self-contained within neo-agent-core
4. **Phase 5** (neo-tui) — self-contained within neo-tui
5. **Phase 4** (run.rs + config.rs) — depends on Phase 3 for import path stability
6. **Phase 3** (interactive.rs) — largest scope, highest risk, do last
7. **Phase 6 remaining tasks** — cleanup after major splits
