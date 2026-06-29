# Context Compaction Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

## ⚠️ PREREQUISITE — Must be completed BEFORE this plan

**This plan MUST be executed AFTER `docs/superpowers/plans/2026-06-29-structured-errors-retry-fallback.md` (Plan A).**

The overflow recovery logic (Task 8) depends on the new `AiError` variants introduced by Plan A:
- `AiError::ContextOverflow { message }`
- `AiError::Server { status, message }`
- `AiError::Auth { message }`

These variants do **not** exist in the current codebase. Attempting this plan before Plan A will fail to compile at Task 8.

**Verification before starting:**
```bash
grep -n 'ContextOverflow' crates/neo-ai/src/error.rs
```
If this returns no results, Plan A is not yet complete. Stop.

---

**Goal:** Enhance neo's context compaction with multi-round compression, provider overflow recovery, observed-max adaptation, empty-summary retry with prefix shrink, and staleness detection.

**Architecture:** All enhancements target the LIVE compaction path — `maybe_compact()` in `compaction_trigger.rs` — NOT `run_compaction()` in `compaction/mod.rs`. The `run_compaction()` function is the library API used by the `/compact` command and is kept as-is. Shared helpers (`reduce_compact_count`, `is_stale`, `generate_with_retry`) are added to `compaction/mod.rs` and imported by `compaction_trigger.rs`. Overflow recovery goes in `turn_loop.rs` around `run_model_turn`.

**Tech Stack:** Rust, tokio, tokio-util (CancellationToken)

**Spec:** `docs/superpowers/specs/2026-06-29-compaction-enhancement-design.md`

**Key context from codebase exploration:**

### CRITICAL: Two compaction paths exist

1. **`maybe_compact()` in `compaction_trigger.rs`** — the LIVE path, called from `turn_loop.rs:56`. Takes `&mut EventEmitter`. Spawns summary task, runs progress loop, applies result. Does NOT call `context.apply_compaction()` directly — only emits `CompactionApplied` event.

2. **`run_compaction()` in `compaction/mod.rs`** — the library API. Takes `&mut AgentContext` + `&mut Vec<AgentEvent>`. Directly calls `context.apply_compaction()`. Used by the `/compact` command. KEPT AS-IS by this plan.

### `maybe_compact` call chain (the LIVE path we enhance):

```
turn_loop.rs:56 → maybe_compact(model, config, emitter, cancel_token)
  → evaluate_compaction_need(config, emitter) → Option<CompactionTrigger>
  → compute_compacted_count(&trigger)
  → emit_compaction_started(emitter, ...)
  → spawn_summary_task(model, config, messages, instruction, cancel_token)
     → compaction::generate_compaction_summary(...) inside tokio::spawn
  → run_summary_progress_loop(emitter, progress_rx, summary_rx)
  → apply_compaction_result(emitter, config, messages, compacted_count, summary_text, ...)
```

### Important codebase facts:

- **`AgentConfig` has NO `Default` impl.** It derives `Clone, Serialize, Deserialize, JsonSchema`. Constructor is `for_model(model)` at line 159. Contains closure/handler fields. Tests MUST use `for_model` or a test helper.
- **`AgentRuntimeError` is NOT `Clone`.** It wraps `AiError` via `#[from]`. Turn loop recovery must use a two-phase match — never clone the error.
- **`turn_loop.rs:23`** imports `use super::tokens::estimate_chat_messages_tokens;` but NOT `estimate_messages_tokens`. Need to add `use super::tokens::estimate_messages_tokens;`.
- **`turn_loop.rs:56`** calls `maybe_compact(&model, &config, emitter, &cancel_token).await;`
- **`turn_loop.rs:72-80`** calls `run_model_turn(...)?.await?` — the `?` propagates `AgentRuntimeError` straight up. Overflow recovery must go between lines 71-80.
- **`evaluate_compaction_need`** (`compaction_trigger.rs:105-153`) reads `max_context_tokens` at line 130: `config.model.capabilities.max_context_tokens.unwrap_or(0) as usize`.
- **`apply_compaction_result`** (`compaction_trigger.rs:352-392`) builds `CompactionSummary`, emits `CompactionApplied { summary }`. Does NOT call `context.apply_compaction()`.
- **`CompactionSettings`** (`config.rs:428-460`) derives `Clone, Copy`. Has `new(max_estimated_tokens, keep_recent_messages)` constructor.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/neo-agent-core/src/compaction/mod.rs` | `CompactionError` extensions, `reduce_compact_count`, `is_stale`, `generate_with_retry` (shared helpers, imported by `compaction_trigger.rs`). `run_compaction` UNCHANGED. |
| `crates/neo-agent-core/src/runtime/config.rs` | `observed_max_context_tokens` field on `AgentConfig`, `effective_max_context_tokens()`, `observe_context_overflow()` |
| `crates/neo-agent-core/src/runtime/compaction_trigger.rs` | C-1: multi-round `maybe_compact` loop; C-4: empty-summary retry in `run_summary_progress_loop`; C-5: staleness check in `apply_compaction_result`; use `effective_max_context_tokens` |
| `crates/neo-agent-core/src/runtime/turn_loop.rs` | C-2: overflow recovery — catch error from `run_model_turn`, forced compaction, retry (no clone) |
| `crates/neo-agent/src/config/mod.rs` | `RuntimeCompactionConfig` add `max_rounds`, `max_retry_attempts` |
| `crates/neo-agent/src/config/types.rs` | `FileRuntimeCompactionConfig` add fields |

---

## Task 1: Extend `CompactionError` with `Truncated` and `Stale` variants

**Files:**
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to the test module in `crates/neo-agent-core/src/compaction/mod.rs`:

```rust
    #[test]
    fn truncated_error_displays_attempt_count() {
        let err = CompactionError::Truncated(5);
        let msg = err.to_string();
        assert!(msg.contains("5"));
        assert!(msg.contains("truncated"));
    }

    #[test]
    fn stale_error_has_message() {
        let err = CompactionError::Stale;
        let msg = err.to_string();
        assert!(msg.contains("stale"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core truncated_error`
Expected: FAIL — `CompactionError::Truncated` doesn't exist.

- [ ] **Step 3: Add the new variants**

In `crates/neo-agent-core/src/compaction/mod.rs`, find the `CompactionError` enum (around line 36) and add two variants:

```rust
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction LLM call failed: {0}")]
    Llm(String),
    #[error("compaction produced an empty summary")]
    Empty,
    #[error("compaction cancelled")]
    Cancelled,
    #[error("no safe compaction boundary found in the current history")]
    NoBoundary,
    #[error("compaction truncated: model returned empty/truncated after {0} attempts")]
    Truncated(u32),
    #[error("compaction stale: history changed during summarization")]
    Stale,
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-agent-core truncated_error`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/compaction/mod.rs
git commit -m "feat(compaction): add Truncated and Stale error variants"
```

---

## Task 2: Add `reduce_compact_count` + `is_stale` helpers to `compaction/mod.rs`

These are shared helpers placed in `compaction/mod.rs` so both `compaction_trigger.rs` (live path) and `run_compaction` (library path) can use them.

**Files:**
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/neo-agent-core/src/compaction/mod.rs`:

```rust
    #[test]
    fn reduce_compact_count_finds_smaller_safe_boundary() {
        let messages = vec![
            user_msg("task 1"),
            assistant_text("done 1"),
            user_msg("task 2"),
            assistant_text("done 2"),
            user_msg("task 3"),
            assistant_text("done 3"),
        ];
        // Current count = 4 (split after index 3). Should find index 1 as smaller safe split.
        let reduced = reduce_compact_count(&messages, 4);
        assert_eq!(reduced, 2);
    }

    #[test]
    fn reduce_compact_count_returns_zero_when_no_smaller_split() {
        let messages = vec![
            user_msg("only"),
            assistant_text("reply"),
        ];
        // Current count = 1. Can't reduce below 1 safely.
        let reduced = reduce_compact_count(&messages, 1);
        assert_eq!(reduced, 0);
    }

    #[test]
    fn is_stale_detects_shorter_history() {
        let snapshot = vec![
            user_msg("a"),
            assistant_text("b"),
            user_msg("c"),
        ];
        let current = vec![
            user_msg("a"),
        ];
        assert!(is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_detects_modified_message() {
        let snapshot = vec![
            user_msg("original"),
            assistant_text("reply"),
        ];
        let current = vec![
            user_msg("changed"),
            assistant_text("reply"),
        ];
        assert!(is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_allows_append() {
        let snapshot = vec![
            user_msg("a"),
            assistant_text("b"),
        ];
        let current = vec![
            user_msg("a"),
            assistant_text("b"),
            user_msg("follow-up"),
        ];
        assert!(!is_stale(&snapshot, &current));
    }

    #[test]
    fn is_stale_returns_false_for_identical() {
        let messages = vec![
            user_msg("a"),
            assistant_text("b"),
        ];
        assert!(!is_stale(&messages, &messages));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core reduce_compact_count`
Expected: FAIL — `reduce_compact_count` and `is_stale` don't exist.

- [ ] **Step 3: Implement the helpers**

Add to `crates/neo-agent-core/src/compaction/mod.rs`, before the `run_compaction` function (around line 530). Make them `pub(crate)` so `compaction_trigger.rs` can import them:

```rust
/// Find a safe compaction split point smaller than `current_count`.
///
/// Walks backward from `current_count - 1` looking for a valid
/// [`can_split_after`] boundary. Returns 0 if no smaller safe split exists.
#[must_use]
pub(crate) fn reduce_compact_count(messages: &[AgentMessage], current_count: usize) -> usize {
    if current_count <= 1 {
        return 0;
    }
    for index in (0..current_count - 1).rev() {
        if can_split_after(messages, index) {
            return index + 1;
        }
    }
    0
}

/// Whether the current context messages differ from a snapshot taken
/// before compaction began. Used to detect undo/clear during the LLM call.
pub(crate) fn is_stale(snapshot: &[AgentMessage], current: &[AgentMessage]) -> bool {
    if current.len() < snapshot.len() {
        return true;
    }
    snapshot
        .iter()
        .zip(current.iter())
        .any(|(a, b)| a != b)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-agent-core reduce_compact_count`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/compaction/mod.rs
git commit -m "feat(compaction): add pub(crate) reduce_compact_count and is_stale helpers"
```

---

## Task 3: Add `generate_with_retry` with prefix shrink to `compaction/mod.rs`

This is the empty-summary retry + prefix shrink helper. It wraps `generate_compaction_summary` and is imported by `compaction_trigger.rs` for use in the live path's `spawn_summary_task`.

**Files:**
- Modify: `crates/neo-agent-core/src/compaction/mod.rs`

- [ ] **Step 1: Write the failing test**

Add to the test module:

```rust
    #[test]
    fn generate_with_retry_has_correct_signature() {
        // Full async testing requires FakeModelClient + tokio runtime.
        // The integration is verified by the multi-round compaction tests in
        // compaction_trigger.rs (Task 7). This test is a compilation marker:
        // if the function signature changes, this fails to compile.
        fn _assert_signature(
            model: &Arc<dyn ModelClient>,
            config: &AgentConfig,
            messages: &[AgentMessage],
            strategy: &CompactionStrategy,
            max_context_tokens: usize,
            cancel_token: &CancellationToken,
            max_retry_attempts: u32,
        ) -> std::pin::Pin<Box<dyn Future<Output = Result<(String, usize), CompactionError>> + Send + '_>> {
            Box::pin(generate_with_retry(
                model,
                config,
                messages,
                strategy,
                max_context_tokens,
                cancel_token,
                max_retry_attempts,
            ))
        }
        // If this compiles, the signature is correct.
    }
```

Note: If the `Future` import is not already present in the test module, add `use std::future::Future;` at the top of the test module. If it conflicts, use the fully-qualified path `std::future::Future` as shown.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core generate_with_retry`
Expected: FAIL — `generate_with_retry` doesn't exist.

- [ ] **Step 3: Implement `generate_with_retry`**

Add to `crates/neo-agent-core/src/compaction/mod.rs`, after `generate_compaction_summary` (around line 522):

```rust
/// Generate a compaction summary with retry on empty/truncated responses.
///
/// Each retry shrinks the prefix to a smaller safe boundary, giving the
/// model a shorter input that is more likely to produce a valid summary.
///
/// Returns `(summary_text, actual_compacted_count)` so the caller knows
/// exactly which messages were summarized.
pub(crate) async fn generate_with_retry(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages: &[AgentMessage],
    strategy: &CompactionStrategy,
    max_context_tokens: usize,
    cancel_token: &CancellationToken,
    max_retry_attempts: u32,
) -> Result<(String, usize), CompactionError> {
    let mut compacted_count = compute_compact_count(
        messages,
        CompactionSource::Auto,
        strategy,
        max_context_tokens,
    );

    for attempt in 0..max_retry_attempts {
        if compacted_count == 0 {
            return Err(CompactionError::NoBoundary);
        }

        if cancel_token.is_cancelled() {
            return Err(CompactionError::Cancelled);
        }

        let prefix = &messages[..compacted_count];
        match generate_compaction_summary(
            model,
            config,
            prefix,
            None,
            cancel_token,
            |_| {},
        )
        .await
        {
            Ok(summary) if !summary.trim().is_empty() => {
                return Ok((summary, compacted_count));
            }
            Ok(_) => {
                // Empty summary → shrink prefix
                let reduced = reduce_compact_count(messages, compacted_count);
                if reduced == 0 {
                    return Err(CompactionError::Truncated(attempt + 1));
                }
                compacted_count = reduced;
            }
            Err(CompactionError::Llm(msg)) if is_retryable_compaction_error(&msg) => {
                let reduced = reduce_compact_count(messages, compacted_count);
                if reduced == 0 {
                    return Err(CompactionError::Truncated(attempt + 1));
                }
                compacted_count = reduced;
            }
            Err(e) => return Err(e),
        }
    }
    Err(CompactionError::Truncated(max_retry_attempts))
}

/// Whether a compaction LLM error is worth retrying (rate limit, timeout, etc.).
fn is_retryable_compaction_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("rate limit")
        || lower.contains("429")
        || lower.contains("timeout")
        || lower.contains("connection")
}
```

- [ ] **Step 4: Run build and fix compilation**

Run: `cargo build -p neo-agent-core`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/compaction/mod.rs
git commit -m "feat(compaction): add generate_with_retry with prefix shrink on empty summary"
```

---

## Task 4: Add `observed_max_context_tokens` to `AgentConfig` + `effective_max_context_tokens()`

**IMPORTANT:** `AgentConfig` has NO `Default` impl. It derives `Clone, Serialize, Deserialize, JsonSchema` and is constructed via `for_model(model)`. Tests must use a test helper that calls `for_model`.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`

- [ ] **Step 1: Write the failing test with a test helper**

Add to the test module in `config.rs` (or create one if it doesn't exist):

```rust
    use neo_ai::{ModelSpec, ModelCapabilities};

    /// Test helper — constructs a minimal AgentConfig via for_model.
    /// AgentConfig has NO Default impl (closure/handler fields).
    fn test_config() -> AgentConfig {
        let spec = ModelSpec {
            id: "test-model".into(),
            provider: "test".into(),
            api_model_id: Some("test-model".into()),
            capabilities: ModelCapabilities {
                max_context_tokens: Some(200_000),
                ..ModelCapabilities::default()
            },
        };
        let mut config = AgentConfig::for_model(spec);
        config
    }

    #[test]
    fn effective_max_uses_observed_when_smaller() {
        let mut config = test_config();
        config.model.capabilities.max_context_tokens = Some(200_000);
        *config.observed_max_context_tokens.lock().unwrap() = Some(100_000);

        let effective = effective_max_context_tokens(&config);
        // observed (100k * 0.85 = 85k) < configured (200k) → use 85k
        assert_eq!(effective, 85_000);
    }

    #[test]
    fn effective_max_uses_configured_when_no_observation() {
        let config = test_config();
        // observed is None → use configured 200k
        let effective = effective_max_context_tokens(&config);
        assert_eq!(effective, 200_000);
    }

    #[test]
    fn observe_context_overflow_only_updates_smaller() {
        let config = test_config();
        config.model.capabilities.max_context_tokens = Some(200_000);

        observe_context_overflow(&config, 180_000);
        // 180k * 0.85 = 153k
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(153_000)
        );

        // Second overflow at 220k → 220k * 0.85 = 187k > 153k → should NOT update
        observe_context_overflow(&config, 220_000);
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(153_000)
        );

        // Third overflow at 100k → 100k * 0.85 = 85k < 153k → should update
        observe_context_overflow(&config, 100_000);
        assert_eq!(
            *config.observed_max_context_tokens.lock().unwrap(),
            Some(85_000)
        );
    }
```

Note: The exact `ModelSpec` and `ModelCapabilities` field names and construction may differ. Before writing the test, verify the actual struct:

```bash
grep -n 'pub struct ModelSpec' crates/neo-ai/src/model.rs
grep -n 'pub struct ModelCapabilities' crates/neo-ai/src/model.rs
grep -n 'pub fn for_model' crates/neo-agent-core/src/runtime/config.rs
```

Adjust the test helper to match the actual constructor signature. If `for_model` requires additional arguments, provide minimal test values.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core effective_max`
Expected: FAIL — `observed_max_context_tokens` field and functions don't exist.

- [ ] **Step 3: Add the field to `AgentConfig` struct**

In `crates/neo-agent-core/src/runtime/config.rs`, add to the `AgentConfig` struct definition (around line 75, after `compaction`):

```rust
    /// Runtime-observed context overflow point.
    /// Set when provider reports overflow; used to cap effective max.
    #[serde(skip)]
    pub observed_max_context_tokens: std::sync::Mutex<Option<usize>>,
```

- [ ] **Step 4: Initialize the field in `AgentConfig::for_model`**

Find `for_model` (around line 159). Add to the struct construction:

```rust
            observed_max_context_tokens: std::sync::Mutex::new(None),
```

Find ALL other places where `AgentConfig` is constructed (search for `AgentConfig {`):

```bash
grep -rn 'AgentConfig {' crates/neo-agent-core/src/ crates/neo-agent/src/
```

Add `observed_max_context_tokens: std::sync::Mutex::new(None),` to every construction site.

- [ ] **Step 5: Add the functions**

Add after the `AgentConfig` struct definition (or `impl` block):

```rust
/// Safety ratio: observed overflow point × this = safe effective max.
const OVERFLOW_SAFETY_RATIO: f64 = 0.85;

/// Effective max context tokens, considering observed overflow.
///
/// Returns `min(configured, observed × 0.85)`.
#[must_use]
pub fn effective_max_context_tokens(config: &AgentConfig) -> usize {
    let configured = config
        .model
        .capabilities
        .max_context_tokens
        .unwrap_or(0) as usize;
    let observed = config
        .observed_max_context_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|v| ((v as f64) * OVERFLOW_SAFETY_RATIO) as usize);

    match (configured, observed) {
        (0, Some(o)) => o,
        (c, Some(o)) => c.min(o),
        (c, None) => c,
    }
}

/// Record an observed context overflow point.
///
/// Only updates if the new value (× 0.85) is smaller than the current
/// observation — never increases the effective max.
pub fn observe_context_overflow(config: &AgentConfig, estimated_tokens: usize) {
    let safe = ((estimated_tokens as f64) * OVERFLOW_SAFETY_RATIO) as usize;
    let mut guard = config
        .observed_max_context_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match *guard {
        Some(current) if safe < current => *guard = Some(safe),
        None => *guard = Some(safe),
        _ => {}
    }
}
```

- [ ] **Step 6: Run tests and fix compilation**

Run: `cargo run -p xtask -- test -p neo-agent-core effective_max`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-agent-core/src/runtime/config.rs
git commit -m "feat(runtime): add observed_max_context_tokens for adaptive compaction thresholds"
```

---

## Task 5: Wire `effective_max_context_tokens` into `evaluate_compaction_need`

Replace the raw `max_context_tokens` read at `compaction_trigger.rs:130` with the adaptive function.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`

- [ ] **Step 1: Locate the current read**

In `compaction_trigger.rs`, `evaluate_compaction_need` at line 130:

```rust
let max_context_tokens = config.model.capabilities.max_context_tokens.unwrap_or(0) as usize;
```

- [ ] **Step 2: Replace with `effective_max_context_tokens`**

Change line 130 to:

```rust
let max_context_tokens = super::config::effective_max_context_tokens(config);
```

- [ ] **Step 3: Verify the import resolves**

Check that `super::config` is accessible from `compaction_trigger.rs`. The module is `runtime::config`, and `compaction_trigger.rs` is in `runtime/`, so `super::config` should resolve. If not, add at the top:

```rust
use super::config::effective_max_context_tokens;
```

and use `effective_max_context_tokens(config)` directly.

- [ ] **Step 4: Run build**

Run: `cargo build -p neo-agent-core`
Expected: PASS

- [ ] **Step 5: Run tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "refactor(compaction): use effective_max_context_tokens in evaluate_compaction_need"
```

---

## Task 6: Add empty-summary retry + prefix shrink to `run_summary_progress_loop` in `compaction_trigger.rs`

**This is C-4.** The live path's summary generation happens inside `spawn_summary_task` → `run_summary_progress_loop`. We enhance the summary task to use `generate_with_retry` instead of a single `generate_compaction_summary` call.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`

- [ ] **Step 1: Read the current `spawn_summary_task` and `run_summary_progress_loop`**

Read `compaction_trigger.rs` focusing on `spawn_summary_task` (find it via the call chain from `maybe_compact`) and `run_summary_progress_loop`. Understand how the summary result is sent back through `summary_rx`.

```bash
grep -n 'fn spawn_summary_task\|fn run_summary_progress_loop' crates/neo-agent-core/src/runtime/compaction_trigger.rs
```

- [ ] **Step 2: Add the import for `generate_with_retry`**

At the top of `compaction_trigger.rs`, add:

```rust
use crate::compaction::{generate_with_retry, reduce_compact_count};
```

Adjust if the existing imports use a different style (e.g., `use crate::compaction::{...}`).

- [ ] **Step 3: Modify `spawn_summary_task` to use `generate_with_retry`**

Inside `spawn_summary_task`, find where `generate_compaction_summary(...)` is called. Replace it with `generate_with_retry(...)`, passing the config's `max_retry_attempts`:

The current code looks approximately like:

```rust
let summary_result = compaction::generate_compaction_summary(
    &model,
    &config,
    &messages,
    instruction,
    &cancel_token,
    |progress| { let _ = progress_tx.send(progress); },
).await;
```

Replace with:

```rust
let summary_result = generate_with_retry(
    &model,
    &config,
    &messages,
    &strategy,        // may need to build a CompactionStrategy here
    max_context_tokens,
    &cancel_token,
    max_retry_attempts,
).await;
```

Note: `generate_with_retry` returns `(String, usize)` — the summary text AND the actual compacted count (which may differ from the initial count after prefix shrink). The caller (`run_summary_progress_loop` / `apply_compaction_result`) must use this `actual_compacted_count` instead of the pre-computed `compacted_count`.

- [ ] **Step 4: Thread the actual compacted count through the summary channel**

The `spawn_summary_task` function (`compaction_trigger.rs:231-264`) currently returns `(mpsc::UnboundedReceiver<usize>, oneshot::Receiver<Result<String, CompactionError>>)`. The `oneshot` carries just the summary `String`.

Change the `oneshot` payload from `Result<String, CompactionError>` to `Result<(String, usize), CompactionError>` so it also carries the actual compacted count.

In `spawn_summary_task`, the summary task body calls `generate_compaction_summary` — replace that call with `generate_with_retry` (from Step 3) which returns `(String, usize)`:

```rust
// In spawn_summary_task (compaction_trigger.rs ~line 249):
tokio::spawn(async move {
    // OLD:
    // let result = compaction::generate_compaction_summary(...).await;

    // NEW: use generate_with_retry which returns (String, usize)
    let strategy = build_compaction_strategy(&summary_config.compaction.unwrap_or(&CompactionSettings::new(100_000, 4)));
    let max_tokens = summary_config.model.capabilities.max_context_tokens.unwrap_or(0) as usize;
    let max_retry = summary_config.compaction.as_ref().map(|s| s.max_retry_attempts).unwrap_or(5);
    let result = compaction::generate_with_retry(
        &summary_model,
        &summary_config,
        &summary_messages,
        &strategy,
        max_tokens,
        &summary_cancel,
        max_retry,
    ).await;
    let _ = summary_tx.send(result);
});
```

Then update `run_summary_progress_loop` (lines 269-348) which reads from `summary_rx`. Change its return type from `Option<(String, u8)>` to `Option<((String, usize), u8)>`:

```rust
// In run_summary_progress_loop:
// OLD: let Some((summary_text, progress_percent)) = ... else { ... }
// NEW: the oneshot now carries (String, usize)
let Some(((summary_text, actual_count), progress_percent)) = ... else { ... };
// Return both:
Some(((summary_text, actual_count), progress_percent))
```

Then update `maybe_compact` (line 55-64) to unpack the new shape:

```rust
// OLD:
// let Some((summary_text, progress_percent)) = run_summary_progress_loop(...)
// NEW:
let Some(((summary_text, actual_compacted_count), progress_percent)) = run_summary_progress_loop(
    emitter, &mut progress_rx, &mut summary_rx, target_summary_chars,
).await else { return; };

// Pass actual_compacted_count (not the pre-computed compacted_count) to apply_compaction_result:
apply_compaction_result(
    emitter, config, &trigger.messages,
    actual_compacted_count,  // ← was: compacted_count
    summary_text, trigger.used_tokens, progress_percent,
).await;
```

- [ ] **Step 5: Pass `max_retry_attempts` from config**

Read the compaction settings to get `max_retry_attempts`:

```rust
let max_retry_attempts = config
    .compaction
    .as_ref()
    .map(|s| s.max_retry_attempts)
    .unwrap_or(5);
```

(This field is added in Task 9. If Task 9 is not yet done, hardcode `5` for now and switch to config in Task 9.)

- [ ] **Step 6: Run build and fix compilation**

Run: `cargo build -p neo-agent-core`
Expected: PASS — fix any borrow/channel type issues.

- [ ] **Step 7: Run tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "feat(compaction): empty-summary retry with prefix shrink in live compaction path"
```

---

## Task 7: Add staleness check in `apply_compaction_result`

**This is C-5.** Before emitting `CompactionApplied`, check whether the context messages have changed since the snapshot was taken (before the summary task was spawned).

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`

- [ ] **Step 1: Read `apply_compaction_result`**

Read `compaction_trigger.rs:352-392` to see the current signature and body:

```rust
grep -n 'fn apply_compaction_result' crates/neo-agent-core/src/runtime/compaction_trigger.rs
```

- [ ] **Step 2: Add staleness check before emitting `CompactionApplied`**

The staleness check needs a snapshot of the messages taken before the summary task. Thread a `snapshot: &[AgentMessage]` parameter into `apply_compaction_result` (or take the snapshot inside `maybe_compact` before spawning and pass it down).

In `maybe_compact`, capture the snapshot right before `spawn_summary_task`:

```rust
let message_snapshot: Vec<AgentMessage> = emitter.context.messages().to_vec();
```

Pass `&message_snapshot` to `apply_compaction_result`.

In `apply_compaction_result`, before building the `CompactionSummary`, add:

```rust
let current_messages = emitter.context.messages();
if crate::compaction::is_stale(snapshot, current_messages) {
    // History changed during summarization (undo/clear). Don't apply stale summary.
    return; // or return Ok(()) with an appropriate signature
}
```

Add the import at the top of `compaction_trigger.rs`:

```rust
use crate::compaction::is_stale;
```

- [ ] **Step 3: Run build**

Run: `cargo build -p neo-agent-core`
Expected: PASS

- [ ] **Step 4: Run tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "feat(compaction): staleness check in apply_compaction_result"
```

---

## Task 8: Add multi-round loop to `maybe_compact`

**This is C-1.** Wrap the body of `maybe_compact` in a loop that continues compacting until either: the context is small enough, no safe boundary is found, max rounds is reached, or reduction is too small.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs`

- [ ] **Step 1: Read the current `maybe_compact` function**

Read `compaction_trigger.rs` to see the full `maybe_compact` body:

```bash
grep -n 'pub.*async.*fn maybe_compact' crates/neo-agent-core/src/runtime/compaction_trigger.rs
```

- [ ] **Step 2: Wrap the body in a loop**

The current `maybe_compact` does:
1. `evaluate_compaction_need` → `Option<CompactionTrigger>`
2. If `None`, return
3. `compute_compacted_count`
4. `emit_compaction_started`
5. `spawn_summary_task`
6. `run_summary_progress_loop`
7. `apply_compaction_result`

Wrap steps 1-7 in a loop:

```rust
pub async fn maybe_compact(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) {
    let max_rounds = config
        .compaction
        .as_ref()
        .map(|s| s.max_rounds)
        .unwrap_or(5);
    let min_reduction_tokens: usize = 1024;

    for round in 0..max_rounds {
        // Step 1: Evaluate need
        let Some(trigger) = evaluate_compaction_need(config, emitter) else {
            break; // no compaction needed
        };

        // Snapshot for staleness check
        let message_snapshot: Vec<AgentMessage> = emitter.context.messages().to_vec();

        // Steps 3-4: Compute count + emit started
        let compacted_count = compute_compacted_count(&trigger);
        emit_compaction_started(emitter, /* ... */);

        // Step 5: Spawn summary task (now using generate_with_retry from Task 6)
        let (progress_tx, progress_rx, summary_rx) = spawn_summary_task(
            model, config, &trigger.messages, trigger.custom_instruction, cancel_token,
        );

        // Step 6: Run progress loop
        let summary_result = run_summary_progress_loop(emitter, progress_rx, summary_rx).await;

        // Step 7: Apply result (with staleness check from Task 7)
        let Ok((summary_text, actual_compacted_count)) = summary_result else {
            break; // error or cancelled
        };

        let tokens_before = trigger.used_tokens;

        apply_compaction_result(
            emitter,
            config,
            &message_snapshot,
            actual_compacted_count,
            summary_text,
            // ...
        ).await;

        // Check if we should continue to another round
        let tokens_after = estimate_messages_tokens(emitter.context.messages());
        if tokens_before.saturating_sub(tokens_after) < min_reduction_tokens {
            break; // diminishing returns
        }
    }
}
```

**NOTE:** The code above shows the multi-round structure. Before editing, read the full `maybe_compact` body (lines 18-76 of `compaction_trigger.rs`) — the functions `emit_compaction_started`, `spawn_summary_task`, `run_summary_progress_loop`, and `apply_compaction_result` have specific return types that must be threaded through. After Task 6's changes, `run_summary_progress_loop` returns `Option<((String, usize), u8)>` — make sure the multi-round loop unpacks this correctly in each round.

- [ ] **Step 3: Run build and fix compilation**

Run: `cargo build -p neo-agent-core`
Expected: PASS — fix borrow issues (emitter borrowed in multiple places).

The key borrow challenge: `emitter.context.messages()` borrows `emitter`, but `evaluate_compaction_need` also takes `emitter`. Structure the code so borrows don't overlap — clone the message snapshot into a `Vec` before passing it to functions that take `&mut EventEmitter`.

- [ ] **Step 4: Run tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "feat(compaction): multi-round compaction in maybe_compact live path"
```

---

## Task 9: Add overflow recovery to `turn_loop.rs`

**This is C-2.** Catch overflow errors from `run_model_turn`, record the observed overflow, trigger forced compaction, and retry. Uses two-phase match to avoid cloning `AgentRuntimeError`.

**PREREQUISITE:** Plan A must be complete. `AiError::ContextOverflow`, `AiError::Server { status }`, `AiError::Auth` must exist.

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`

- [ ] **Step 1: Add the missing import**

At the top of `turn_loop.rs`, add (alongside the existing `use super::tokens::estimate_chat_messages_tokens;` at line 23):

```rust
use super::tokens::estimate_messages_tokens;
```

- [ ] **Step 2: Write the failing tests**

Add to the test module in `turn_loop.rs`:

```rust
    #[test]
    fn should_recover_from_context_overflow_error() {
        use neo_ai::AiError;
        let err = AgentRuntimeError::Model(AiError::ContextOverflow {
            message: "too long".into(),
        });
        assert!(should_recover_from_overflow(&err));
    }

    #[test]
    fn should_not_recover_from_auth_error() {
        use neo_ai::AiError;
        let err = AgentRuntimeError::Model(AiError::Auth {
            message: "bad key".into(),
        });
        assert!(!should_recover_from_overflow(&err));
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core should_recover_from`
Expected: FAIL — function doesn't exist (or Plan A not done — see prerequisite).

- [ ] **Step 4: Implement `should_recover_from_overflow`**

In `crates/neo-agent-core/src/runtime/turn_loop.rs`, add a simple variant checker (the full token-ratio check for 413 is done inline):

```rust
/// Whether an error represents a context overflow that compaction might fix.
fn should_recover_from_overflow(err: &AgentRuntimeError) -> bool {
    let AgentRuntimeError::Model(ai_err) = err else {
        return false;
    };
    matches!(ai_err, AiError::ContextOverflow { .. })
}
```

- [ ] **Step 5: Wire recovery into `run_agent_turn` — two-phase match (no clone)**

In `run_agent_turn`, the current code at lines 72-80 calls `run_model_turn(...)?.await?`. The `?` propagates the error straight up. Replace it with a two-phase match that catches overflow errors and retries.

Find this block (around lines 72-80):

```rust
        let assistant = run_model_turn(
            Arc::clone(&model),
            &config,
            request,
            turn,
            emitter,
            cancel_token.clone(),
        )
        .await?;
```

Replace with:

```rust
        let assistant = {
            let model_result = run_model_turn(
                Arc::clone(&model),
                &config,
                request,
                turn,
                emitter,
                cancel_token.clone(),
            )
            .await;

            match model_result {
                Ok(result) => result,
                Err(e) => {
                    if !should_recover_from_overflow(&e) {
                        return Err(e);
                    }

                    // Record observed overflow for adaptive threshold
                    let estimated = estimate_messages_tokens(emitter.context.messages());
                    super::config::observe_context_overflow(&config, estimated);

                    // Trigger forced compaction via the live path.
                    // Set the manual_compact_request mutex that evaluate_compaction_need
                    // reads — this is the SAME mechanism /compact uses. It sets
                    // `force = true` inside CompactionTrigger without needing a
                    // separate API.
                    {
                        let mut guard = config
                            .manual_compact_request
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        *guard = Some(String::new()); // empty instruction = force with no custom text
                    }
                    maybe_compact(&model, &config, emitter, &cancel_token).await;

                    // Rebuild request with compacted context and retry once
                    let retry_request = chat_request(&config, &emitter.context).await;
                    match run_model_turn(
                        Arc::clone(&model),
                        &config,
                        retry_request,
                        turn,
                        emitter,
                        cancel_token.clone(),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            // Recovery failed — return a synthetic error (can't clone original)
                            return Err(AgentRuntimeError::Model(AiError::Stream {
                                message: "compaction recovery failed after context overflow".into(),
                            }));
                        }
                    }
                }
            }
        };
```

**NOTE on forced compaction:** The `config.manual_compact_request` mutex is the same mechanism `/compact` uses. `evaluate_compaction_need` reads it at `compaction_trigger.rs:114-123` — when it's `Some(instruction)`, `force` is set to `true` in the `CompactionTrigger`. Setting it to `Some(String::new())` forces compaction with no custom instruction text, which is exactly what we want for overflow recovery. The mutex is consumed (taken) by `evaluate_compaction_need`, so no cleanup is needed.

- [ ] **Step 6: Handle the borrow issue**

`emitter.context.messages()` borrows `emitter`, but `maybe_compact` takes `&mut emitter`. Clone the messages into a `Vec` first:

```rust
let messages_snapshot = emitter.context.messages().to_vec();
let estimated = estimate_messages_tokens(&messages_snapshot);
super::config::observe_context_overflow(&config, estimated);
// now borrow emitter mutably
maybe_compact(&model, &config, emitter, &cancel_token).await;
```

- [ ] **Step 7: Run build and fix compilation**

Run: `cargo build -p neo-agent-core`
Expected: PASS — fix borrow checker issues and ensure `AiError::Stream` variant name matches (check `crates/neo-ai/src/error.rs`).

```bash
grep -n 'Stream' crates/neo-ai/src/error.rs
```

If the variant is named differently (e.g., `AiError::Other` or `AiError::Custom`), use the correct name.

- [ ] **Step 8: Run tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add crates/neo-agent-core/src/runtime/turn_loop.rs crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "feat(runtime): overflow recovery — forced compaction + retry on context overflow"
```

---

## Task 10: Add config fields `max_rounds` / `max_retry_attempts` to `CompactionSettings`

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs` (`CompactionSettings`)
- Modify: `crates/neo-agent/src/config/mod.rs` (`RuntimeCompactionConfig`)
- Modify: `crates/neo-agent/src/config/types.rs` (`FileRuntimeCompactionConfig`)
- Modify: `crates/neo-agent-core/src/runtime/compaction_trigger.rs` (use config values instead of hardcoded)

- [ ] **Step 1: Add fields to `CompactionSettings`**

In `crates/neo-agent-core/src/runtime/config.rs`, find `CompactionSettings` (around line 428) and add:

```rust
pub struct CompactionSettings {
    // ... existing fields ...
    /// Maximum compaction rounds per invocation.
    pub max_rounds: usize,
    /// Maximum retry attempts for empty/truncated summaries.
    pub max_retry_attempts: u32,
}
```

In `CompactionSettings::new()` (around line 460), add the defaults:

```rust
pub fn new(max_estimated_tokens: usize, keep_recent_messages: usize) -> Self {
    Self {
        // ... existing fields ...
        max_rounds: 5,
        max_retry_attempts: 5,
    }
}
```

- [ ] **Step 2: Add fields to `RuntimeCompactionConfig`**

In `crates/neo-agent/src/config/mod.rs`, find `RuntimeCompactionConfig` and add:

```rust
pub struct RuntimeCompactionConfig {
    // ... existing fields ...
    pub max_rounds: usize,
    pub max_retry_attempts: u32,
}
```

In `Default for RuntimeCompactionConfig`:

```rust
max_rounds: 5,
max_retry_attempts: 5,
```

- [ ] **Step 3: Add fields to `FileRuntimeCompactionConfig`**

In `crates/neo-agent/src/config/types.rs`:

```rust
pub(crate) struct FileRuntimeCompactionConfig {
    // ... existing fields ...
    pub(crate) max_rounds: Option<usize>,
    pub(crate) max_retry_attempts: Option<u32>,
}
```

- [ ] **Step 4: Update config mapping**

Find the mapping from `FileRuntimeCompactionConfig` to `CompactionSettings` (or `RuntimeCompactionConfig`). Add:

```rust
max_rounds: file.max_rounds.unwrap_or(5),
max_retry_attempts: file.max_retry_attempts.unwrap_or(5),
```

- [ ] **Step 5: Use config values in `compaction_trigger.rs`**

In the `maybe_compact` loop (Task 8), replace the hardcoded `5` for `max_rounds`:

```rust
let max_rounds = config
    .compaction
    .as_ref()
    .map(|s| s.max_rounds)
    .unwrap_or(5);
```

In the `spawn_summary_task` / `generate_with_retry` call (Task 6), replace the hardcoded `5` for `max_retry_attempts`:

```rust
let max_retry_attempts = config
    .compaction
    .as_ref()
    .map(|s| s.max_retry_attempts)
    .unwrap_or(5);
```

- [ ] **Step 6: Run build and tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

Run: `cargo build -p neo-agent`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-agent/src/config/ crates/neo-agent-core/src/runtime/config.rs crates/neo-agent-core/src/runtime/compaction_trigger.rs
git commit -m "feat(compaction): configurable max_rounds and max_retry_attempts"
```

---

## Task 11: Run full CRAP + coverage gate

- [ ] **Step 1: Run focused tests**

Run: `cargo run -p xtask -- test -p neo-agent-core`
Expected: PASS

- [ ] **Step 2: Run CRAP gate**

Run: `cargo run -p xtask -- crap`
Expected: No function with CRAP > 30. If any new function exceeds, simplify it.

Check the artifacts:
- `target/crap/crap-crates.md` — scan for any `neo-agent-core` functions exceeding 30.
- If `maybe_compact` (now with the multi-round loop) exceeds 30, extract the loop body into a helper function `run_single_compaction_round`.

- [ ] **Step 3: Run coverage**

Run: `cargo run -p xtask -- coverage`
Expected: LCOV file generated at `target/llvm-cov/lcov.info`.

- [ ] **Step 4: Run parity check**

Run: `cargo run -p xtask -- parity`
Expected: PASS — ensure any new config examples in `examples/` match.

- [ ] **Step 5: Final commit if any cleanup**

```bash
git add -u
git commit -m "test(compaction): verify CRAP and coverage thresholds"
```

---

## Summary of BLOCKER fixes applied

| Blocker | Issue | Fix |
|---------|-------|-----|
| **1** | Depends on Plan A's `AiError` variants | Added prerequisite section at top; Task 9 verification step checks for `ContextOverflow` before proceeding |
| **2** | `AgentConfig::default()` doesn't exist | Task 4 uses `test_config()` helper calling `AgentConfig::for_model(test_model_spec)`; all tests use this helper |
| **3** | `AgentRuntimeError` not Clone | Task 9 uses two-phase match: `match model_result { Ok(r) => r, Err(e) => { if !recoverable { return Err(e) } ... } }` — error is moved, never cloned |
| **4** | Enhancements target wrong entrypoint | All C-1 through C-5 applied to `maybe_compact()` / `compaction_trigger.rs` (the LIVE path). `run_compaction()` in `compaction/mod.rs` is UNCHANGED. Helpers are `pub(crate)` in `compaction/mod.rs`, imported by `compaction_trigger.rs`. |
