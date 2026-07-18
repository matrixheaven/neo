# Neo Stream Retry Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound silent model streams, stop permanent quota failures immediately, and make retry terminal state visible and durable in Neo's existing transcript Card.

**Architecture:** `neo-agent-core` remains the only retry owner. `neo-ai` classifies permanent quota errors before the runtime sees them, the runtime converts first-event and between-event silence into the existing retryable `Transport` path, `neo-agent` maps the three global TOML settings into every `AgentConfig`, and `neo-tui` renders/replays one canonical retry Card. Ordinary retries continue to clone the same frozen `ChatRequest`; no provider retry loop, watchdog task, or second lifecycle is added.

**Tech Stack:** Rust 2024 workspace, Tokio, futures streams, serde/TOML, Neo `AgentEvent` JSONL, transcript entries, focused `cargo test` targets.

## Global Constraints

- The only user-facing configuration is:

  ```toml
  [runtime.retry]
  max_retries = 5
  first_event_timeout_secs = 60
  stream_idle_timeout_secs = 120
  ```

- `max_retries` is a `u32`; it counts requests after the initial request, defaults to `5`, accepts `100`, and `0` disables automatic retry.
- Both timeout fields are `u64` seconds. `first_event_timeout_secs` defaults to `60`, `stream_idle_timeout_secs` defaults to `120`, and each field's `0` disables only that deadline.
- The first-event deadline covers the first poll through the first normalized `AiStreamEvent`. The idle deadline restarts after each later normalized event. Provider keepalive comments do not reset either deadline.
- Timeout expiry becomes `AiError::Transport` and follows the existing runtime retry loop. Cancellation wins when an event, deadline, and `Esc` are simultaneously ready.
- Permanent quota exhaustion uses `AiError::QuotaExhausted { message }`, code `provider.quota_exhausted`, and is terminal. It never emits a reconnect lifecycle.
- Permanent structured codes are exactly `insufficient_quota`, `insufficient_balance`, `billing_limit_exceeded`, `usage_limit_exceeded`, and `payment_required`.
- Permanent fallback phrases are exactly `usage limit for this billing cycle`, `purchase extra usage`, `insufficient balance`, `insufficient credits`, `quota exhausted`, and `billing limit exceeded`, matched case-insensitively.
- Generic `quota`, `limit`, and `billing` matches are forbidden. `quota_exceeded` and `resource_exhausted` remain retryable rate-limit signals.
- HTTP `402` is permanent quota exhaustion. HTTP `403` and `429` become permanent quota exhaustion only when a canonical code or phrase is present; ordinary `403` remains `Auth` and ordinary `429` remains `RateLimit`.
- Retry attempts clone the already-frozen `ChatRequest`; no projection, compaction estimation, reminder injection, or tool-schema construction is rerun. Failed attempt deltas remain live-only and never enter canonical context or replay.
- `RetryExhausted` is the only durable retry terminal Card. Replay ignores `RetryScheduled`, `RetryStarted`, `RetryResumed`, and `RetrySucceeded`, but restores `RetryExhausted` and deduplicates its following `Error` and `RunFinished(Error)`.
- Waiting and connecting Cards animate. Exhaustion says `retry disabled`, `after 1 retry`, or `after N retries`; it never calls retries attempts.
- English and Chinese configuration guides must describe identical behavior, not just list the new fields.
- No compatibility aliases, provider/model overrides, new dependencies, new event variants, new background watchdog, or second retry state machine.
- Preserve Windows, Linux, and macOS behavior. Use `Duration`, Tokio, and existing typed interfaces; do not introduce shell or platform-specific timing code.
- Subagents do not perform Git mutation. The coordinator stages only the task's files and commits only after RED/GREEN evidence and a clean task review.

## File Map

| File | Responsibility |
| --- | --- |
| `crates/neo-ai/src/error.rs` | Add terminal quota variant, code, and retryability |
| `crates/neo-ai/src/error_info.rs` | Add user-facing quota metadata without hiding provider detail |
| `crates/neo-ai/src/providers/common/error.rs` | Centralize HTTP and stream hard-quota classification |
| `crates/neo-agent-core/src/runtime/config.rs` | Carry timeout defaults and values in every `AgentConfig` |
| `crates/neo-agent-core/src/runtime/turn_loop.rs` | Pass the existing retry configuration into each model attempt |
| `crates/neo-agent-core/src/runtime/stream_aggregator.rs` | Race stream polling against cancellation and the active silence deadline |
| `crates/neo-agent-core/tests/runtime_turn.rs` | Prove timeout retry, frozen request identity, attempt rollback, and zero-disable cancellation |
| `crates/neo-agent/src/config/mod.rs` | Add public runtime retry values and defaults |
| `crates/neo-agent/src/config/types.rs` | Deserialize the two optional TOML timeout fields |
| `crates/neo-agent/src/config/loader.rs` | Apply defaults and materialize the canonical config table |
| `crates/neo-agent/src/modes/run/runtime/agent.rs` | Copy all three retry values into `AgentConfig` |
| `crates/neo-agent/src/modes/run/mod.rs` | Verify app-to-core config mapping |
| `docs/en/configuration/config-files.md` | Canonical English behavior and examples |
| `docs/zh/configuration/config-files.md` | Canonical Chinese behavior and examples |
| `crates/neo-tui/src/transcript/entry/mod.rs` | Feed animation frames to retry rendering and schedule both live phases |
| `crates/neo-tui/src/transcript/entry/render_status.rs` | Render spinner, retry-count terminal wording, and normalized transport detail |
| `crates/neo-tui/src/transcript/event_handler.rs` | Preserve quota detail and suppress generic run-finished noise |
| `crates/neo-tui/tests/transcript_pane.rs` | Prove Card animation, terminal wording, detail, and deduplication |
| `crates/neo-agent/src/modes/interactive/mod.rs` | Restore only durable retry exhaustion during replay |
| `crates/neo-agent/src/modes/interactive/tests.rs` | Prove replay has exactly one terminal Card |

---

### Task 1: Classify Permanent Quota Exhaustion

**Files:**
- Modify: `crates/neo-ai/src/error.rs`
- Modify: `crates/neo-ai/src/error_info.rs`
- Modify: `crates/neo-ai/src/providers/common/error.rs`
- Test: unit tests in those three modules

**Interfaces:**
- Consumes: `ProviderError::HttpStatus`, `stream_failure`, `AiError::code`, `AiError::is_retryable`, and `error_info`.
- Produces: `AiError::QuotaExhausted { message: String }`, stable code `provider.quota_exhausted`, and a shared terminal classifier used by all provider adapters.

- [ ] **Step 1: Extend the exhaustive error tests before the enum exists**

  Add quota assertions to `error::tests::code_returns_domain_dot_reason` and `error::tests::is_retryable_for_each_variant`:

  ```rust
  assert_eq!(
      AiError::QuotaExhausted {
          message: "buy more credits".into(),
      }
      .code(),
      "provider.quota_exhausted"
  );
  assert!(!AiError::QuotaExhausted {
      message: "buy more credits".into(),
  }
  .is_retryable());
  ```

  Extend `error_info::tests::known_codes_return_specific_info`:

  ```rust
  let quota = error_info("provider.quota_exhausted");
  assert_eq!(quota.title, "Quota Exhausted");
  assert!(!quota.retryable);
  assert_eq!(quota.action, None);
  ```

- [ ] **Step 2: Run the exhaustive tests and record RED**

  Run:

  ```bash
  rtk cargo test --package neo-ai --lib -- error::tests::code_returns_domain_dot_reason --exact --nocapture --include-ignored
  ```

  Expected: compilation fails because `AiError::QuotaExhausted` does not exist. This is the required RED; a syntax or unrelated build failure does not count.

- [ ] **Step 3: Add table-driven shared-classifier tests and record RED**

  Add `providers::common::error::tests::permanent_quota_http_errors_are_terminal`. It must assert:

  ```rust
  for (status, body) in [
      (402, "Payment Required"),
      (403, r#"{"error":{"code":"insufficient_quota"}}"#),
      (429, "Usage limit for this billing cycle"),
  ] {
      let error = ProviderError::HttpStatus {
          status,
          body: Some(body.into()),
          retry_after: None,
      }
      .into_ai_error();
      assert!(matches!(error, AiError::QuotaExhausted { .. }));
      assert!(!error.is_retryable());
  }

  assert!(matches!(
      ProviderError::HttpStatus {
          status: 403,
          body: Some("Forbidden".into()),
          retry_after: None,
      }
      .into_ai_error(),
      AiError::Auth { .. }
  ));
  assert!(matches!(
      ProviderError::HttpStatus {
          status: 429,
          body: Some("Too Many Requests".into()),
          retry_after: None,
      }
      .into_ai_error(),
      AiError::RateLimit { .. }
  ));
  ```

  Add `providers::common::error::tests::permanent_quota_stream_codes_are_terminal`. Iterate over all five permanent codes, then prove both ambiguous codes remain retryable:

  ```rust
  for code in [
      "insufficient_quota",
      "insufficient_balance",
      "billing_limit_exceeded",
      "usage_limit_exceeded",
      "payment_required",
  ] {
      assert!(matches!(
          stream_failure(Some(code), "provider detail").into_ai_error(),
          AiError::QuotaExhausted { .. }
      ));
  }
  for code in ["quota_exceeded", "resource_exhausted"] {
      assert!(matches!(
          stream_failure(Some(code), "try later").into_ai_error(),
          AiError::RateLimit { .. }
      ));
  }
  ```

  Run:

  ```bash
  rtk cargo test --package neo-ai --lib -- providers::common::error::tests::permanent_quota_http_errors_are_terminal --exact --nocapture --include-ignored
  ```

  Expected: RED because `402`, hard-quota `403`, and hard-quota `429` do not yet produce the terminal variant.

- [ ] **Step 4: Implement the minimal canonical taxonomy**

  Add the enum member, code, and terminal retryability:

  ```rust
  #[error("quota exhausted: {message}")]
  QuotaExhausted { message: String },
  ```

  ```rust
  Self::QuotaExhausted { .. } => "provider.quota_exhausted",
  ```

  Place `QuotaExhausted` in the non-retryable branch of `is_retryable`. Add this metadata without an action, because an action would replace the provider's useful detail in the TUI:

  ```rust
  "provider.quota_exhausted" => info("Quota Exhausted", false, None),
  ```

- [ ] **Step 5: Implement one narrow shared quota classifier**

  In `providers/common/error.rs`, add only these canonical markers:

  ```rust
  const PERMANENT_QUOTA_CODES: &[&str] = &[
      "insufficient_quota",
      "insufficient_balance",
      "billing_limit_exceeded",
      "usage_limit_exceeded",
      "payment_required",
  ];
  const PERMANENT_QUOTA_PHRASES: &[&str] = &[
      "usage limit for this billing cycle",
      "purchase extra usage",
      "insufficient balance",
      "insufficient credits",
      "quota exhausted",
      "billing limit exceeded",
  ];
  ```

  Implement code-token and phrase matching with standard string operations. Code matching must compare a complete underscore-delimited token, not use a generic `contains("quota")`/`contains("limit")` check. In `stream_failure`, map an exact permanent code to the existing synthetic HTTP `402`; leave `quota_exceeded` and `resource_exhausted` in the `429` branch.

  Order `ProviderError::into_ai_error` as follows:

  ```rust
  402 => AiError::QuotaExhausted { message: excerpt },
  403 | 429 if is_permanent_quota(&excerpt) => {
      AiError::QuotaExhausted { message: excerpt }
  }
  429 => AiError::RateLimit {
      message: excerpt,
      retry_after,
  },
  401 | 403 => AiError::Auth { message: excerpt },
  ```

  Do not add a `ProviderError` variant or modify individual adapters; every provider already routes HTTP and in-stream failures through this shared module.

- [ ] **Step 6: Run GREEN verification**

  Run all four exact behavior tests:

  ```bash
  rtk cargo test --package neo-ai --lib -- error::tests::code_returns_domain_dot_reason --exact --nocapture --include-ignored
  rtk cargo test --package neo-ai --lib -- error::tests::is_retryable_for_each_variant --exact --nocapture --include-ignored
  rtk cargo test --package neo-ai --lib -- providers::common::error::tests::permanent_quota_http_errors_are_terminal --exact --nocapture --include-ignored
  rtk cargo test --package neo-ai --lib -- providers::common::error::tests::permanent_quota_stream_codes_are_terminal --exact --nocapture --include-ignored
  ```

  Expected: each command reports one passed test and zero failures.

- [ ] **Step 7: Review and commit Task 1**

  A fresh reviewer must independently report both spec compliance and code quality approved. Critical or Important findings return to a fixer and then the same review gate. After approval, the coordinator runs `rtk git diff --check`, stages exactly the three Task 1 files, and commits:

  ```bash
  rtk git add crates/neo-ai/src/error.rs crates/neo-ai/src/error_info.rs crates/neo-ai/src/providers/common/error.rs
  rtk git commit -m "fix(neo-ai): stop retrying exhausted quota"
  ```

---

### Task 2: Bound First-Event and Stream-Idle Silence

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
- Modify: `crates/neo-agent-core/src/runtime/stream_aggregator.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Test: private unit test in `stream_aggregator.rs` and integration tests in `runtime_turn.rs`

**Interfaces:**
- Consumes: the existing frozen `ChatRequest`, `run_model_request_with_retries`, `AiError::Transport`, `CancellationToken`, and `DelayedHarness`.
- Produces: public constants `DEFAULT_FIRST_EVENT_TIMEOUT_SECS: u64 = 60` and `DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 120`, plus matching `AgentConfig` fields used by Task 3.

- [ ] **Step 1: Generalize only the existing delayed test harness**

  Keep `DelayedHarness::new(steps)` for its current callers and add `from_turns` for retry tests. Store turns in `VecDeque<Vec<DelayedStep>>`, pop one sequence per `stream_chat`, and expose cloned captured requests:

  ```rust
  fn from_turns(turns: impl IntoIterator<Item = Vec<DelayedStep>>) -> Self;

  fn requests(&self) -> Vec<ChatRequest> {
      self.client
          .requests
          .lock()
          .expect("request lock poisoned")
          .clone()
  }
  ```

  This is test-only infrastructure; do not add test accessors to production clients.

- [ ] **Step 2: Write first-event timeout and frozen-request tests**

  Add `stream_first_event_timeout_retries_same_request`. Configure `first_event_timeout_secs = 1`, `stream_idle_timeout_secs = 0`, and `max_retries = 1`. The first turn delays longer than one second before any event; the second emits a complete successful message. Assert:

  ```rust
  assert_eq!(harness.requests().len(), 2);
  assert_eq!(
      serde_json::to_value(&harness.requests()[0]).expect("serialize first request"),
      serde_json::to_value(&harness.requests()[1]).expect("serialize retry request")
  );
  assert!(events.iter().any(|event| matches!(
      event,
      AgentEvent::RetryScheduled {
          retry: 1,
          error_code,
          message,
          ..
      } if error_code == "provider.transport_error"
          && message.contains("first model stream event")
  )));
  assert!(events.iter().any(|event| matches!(
      event,
      AgentEvent::RetrySucceeded { retries_used: 1, .. }
  )));
  ```

  Run:

  ```bash
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_first_event_timeout_retries_same_request --exact --nocapture --include-ignored
  ```

  Expected: RED because `AgentConfig` has no timeout fields and a silent stream has no runtime deadline.

- [ ] **Step 3: Write idle rollback and zero-disable cancellation tests**

  Add `stream_idle_timeout_retries_and_discards_partial_attempt`. The first turn emits `MessageStart` and `TextDelta("discarded partial")`, then delays beyond one second. The retry emits `MessageStart`, `TextDelta("winning answer")`, and `MessageEnd`. Set first-event timeout to `0`, idle timeout to `1`, and one retry. Assert the lifecycle identifies a transport idle timeout, exactly one assistant `MessageAppended` contains only `winning answer`, `AgentContext` contains no `discarded partial`, and both captured requests serialize identically.

  Add `stream_timeout_zero_waits_until_cancelled`. Set both timeout fields and `max_retries` to `0`, use a long delayed stream, cancel through the existing active-turn token after `TurnStarted`, and assert cancellation barriers are emitted with no `RetryScheduled`, `RetryStarted`, or `RetryExhausted`.

  Run:

  ```bash
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_idle_timeout_retries_and_discards_partial_attempt --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_timeout_zero_waits_until_cancelled --exact --nocapture --include-ignored
  ```

  Expected: both are RED for missing timeout configuration/behavior.

- [ ] **Step 4: Write the simultaneous cancellation race test**

  In `stream_aggregator.rs`, add `runtime::stream_aggregator::tests::next_model_event_prefers_cancel_over_ready_event`. For multiple iterations, create a ready one-event stream and a pre-cancelled token, call `next_model_event`, and require `AiError::Cancelled` every time. Repetition makes the current unbiased select reliably expose its non-deterministic event win.

  Run:

  ```bash
  rtk cargo test --package neo-agent-core --lib -- runtime::stream_aggregator::tests::next_model_event_prefers_cancel_over_ready_event --exact --nocapture --include-ignored
  ```

  Expected: RED because the current select is not biased toward cancellation and has no deadline input.

- [ ] **Step 5: Add core timeout values and defaults**

  In `runtime/config.rs`, add:

  ```rust
  pub const DEFAULT_FIRST_EVENT_TIMEOUT_SECS: u64 = 60;
  pub const DEFAULT_STREAM_IDLE_TIMEOUT_SECS: u64 = 120;
  ```

  Add these fields immediately after `max_retries` and initialize them in `AgentConfig::for_model`:

  ```rust
  pub first_event_timeout_secs: u64,
  pub stream_idle_timeout_secs: u64,
  ```

  ```rust
  first_event_timeout_secs: DEFAULT_FIRST_EVENT_TIMEOUT_SECS,
  stream_idle_timeout_secs: DEFAULT_STREAM_IDLE_TIMEOUT_SECS,
  ```

  Existing `AgentConfig: Clone` carries the values to child runtimes; do not add a clone round-trip test.

- [ ] **Step 6: Race each stream poll against the active deadline**

  Pass `config` from `run_model_request_with_retries` into `run_model_attempt`. In `run_model_attempt`, use `received_event = false`; choose the first-event seconds before the first successful normalized event, then choose idle seconds for every later poll. A successful event sets `received_event = true`, so the next loop creates a fresh idle deadline.

  Extend `next_model_event` with `timeout_secs` and whether it is waiting for the first event. Use one local future: `tokio::time::sleep(Duration::from_secs(timeout_secs))` when nonzero and `std::future::pending::<()>()` when zero. Pin it, then use this exact priority:

  ```rust
  tokio::select! {
      biased;
      () = cancel_token.cancelled() => Some(Err(neo_ai::AiError::Cancelled)),
      event = stream.next() => event,
      () = &mut timeout => Some(Err(neo_ai::AiError::Transport {
          message: if waiting_for_first_event {
              format!("timed out waiting {timeout_secs}s for the first model stream event")
          } else {
              format!("model stream idle for {timeout_secs}s")
          },
      })),
  }
  ```

  Do not spawn a watchdog or change the retry loop. Returning `Transport` makes existing `RetryScheduled`, backoff, attempt rollback, request clone, and exhaustion behavior apply automatically.

- [ ] **Step 7: Run GREEN verification**

  Run:

  ```bash
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_first_event_timeout_retries_same_request --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_idle_timeout_retries_and_discards_partial_attempt --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --test runtime_turn -- stream_timeout_zero_waits_until_cancelled --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent-core --lib -- runtime::stream_aggregator::tests::next_model_event_prefers_cancel_over_ready_event --exact --nocapture --include-ignored
  ```

  Expected: each exact test passes with zero failures. Inspect captured request equality and event assertions; elapsed time alone is not evidence.

- [ ] **Step 8: Review and commit Task 2**

  Require clean spec and quality verdicts. After fixes and re-review, run `rtk git diff --check`, stage exactly the four Task 2 files, and commit:

  ```bash
  rtk git add crates/neo-agent-core/src/runtime/config.rs crates/neo-agent-core/src/runtime/turn_loop.rs crates/neo-agent-core/src/runtime/stream_aggregator.rs crates/neo-agent-core/tests/runtime_turn.rs
  rtk git commit -m "feat(runtime): retry silent model streams"
  ```

---

### Task 3: Wire TOML Configuration and Document the Contract

**Files:**
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `docs/en/configuration/config-files.md`
- Modify: `docs/zh/configuration/config-files.md`
- Test: existing config and run-mode unit tests

**Interfaces:**
- Consumes: Task 2's `DEFAULT_FIRST_EVENT_TIMEOUT_SECS`, `DEFAULT_STREAM_IDLE_TIMEOUT_SECS`, and `AgentConfig` fields.
- Produces: canonical `RuntimeRetryConfig` and TOML loading for all three values, inherited by interactive, run, RPC, and child runtimes through the existing `AgentConfig` construction path.

- [ ] **Step 1: Extend the existing config loader test and record RED**

  Rename `runtime_retry_defaults_and_loads_explicit_max_retries` to `runtime_retry_defaults_and_loads_explicit_values`. Build a `FileRuntimeRetryConfig` with:

  ```rust
  max_retries: Some(100),
  first_event_timeout_secs: Some(7),
  stream_idle_timeout_secs: Some(11),
  ```

  Assert explicit values and defaults:

  ```rust
  assert_eq!(config.retry.max_retries, 100);
  assert_eq!(config.retry.first_event_timeout_secs, 7);
  assert_eq!(config.retry.stream_idle_timeout_secs, 11);

  let defaults = RuntimeConfig::default().retry;
  assert_eq!(defaults.max_retries, 5);
  assert_eq!(defaults.first_event_timeout_secs, 60);
  assert_eq!(defaults.stream_idle_timeout_secs, 120);
  ```

  Run:

  ```bash
  rtk cargo test --package neo-agent --bin neo -- config::tests::runtime_retry_defaults_and_loads_explicit_values --exact --nocapture --include-ignored
  ```

  Expected: compilation fails because the file and runtime config structs do not contain the timeout fields.

- [ ] **Step 2: Extend the app-to-core mapping test and record RED**

  In `modes::run::tests::agent_config_for_app_applies_runtime_config`, construct:

  ```rust
  retry: RuntimeRetryConfig {
      max_retries: 100,
      first_event_timeout_secs: 7,
      stream_idle_timeout_secs: 11,
  },
  ```

  Assert the returned `AgentConfig` contains `100`, `7`, and `11`. Run:

  ```bash
  rtk cargo test --package neo-agent --bin neo -- modes::run::tests::agent_config_for_app_applies_runtime_config --exact --nocapture --include-ignored
  ```

  Expected: RED because `RuntimeRetryConfig` and the mapping path do not yet expose both timeout values.

- [ ] **Step 3: Add one canonical config path**

  Extend both structs:

  ```rust
  pub struct RuntimeRetryConfig {
      pub max_retries: u32,
      pub first_event_timeout_secs: u64,
      pub stream_idle_timeout_secs: u64,
  }
  ```

  ```rust
  pub(crate) struct FileRuntimeRetryConfig {
      pub(crate) max_retries: Option<u32>,
      pub(crate) first_event_timeout_secs: Option<u64>,
      pub(crate) stream_idle_timeout_secs: Option<u64>,
  }
  ```

  Use Task 2's exported constants in `RuntimeRetryConfig::default`, `runtime_from_file`, and `default_file_runtime_retry`; do not duplicate alternate defaults or accept legacy aliases. In `agent_config_for_app`, assign all three values next to the existing `max_retries` assignment.

- [ ] **Step 4: Write the complete English configuration contract**

  Replace the short `[runtime.retry]` section with the full canonical example and prose that states all of the following:

  - field types and defaults;
  - independent `0` semantics for retry and each deadline;
  - initial request plus retry-count meaning, including that `100` permits up to 101 requests;
  - first normalized event versus silence between later normalized events;
  - keepalive comments do not reset deadlines;
  - timeout expiry is a retryable transport failure;
  - permanent quota exhaustion is terminal and does not show a reconnect Card;
  - ordinary retries resend the same frozen request so prompt/cache identity stays stable;
  - failed attempt deltas are not persisted to canonical context or replay;
  - valid `Retry-After` overrides local backoff and is capped at 24 hours;
  - `Esc` cancels an active stream or retry wait;
  - the inline Card animates while waiting/connecting and only exhausted state is restored on replay.

- [ ] **Step 5: Mirror the same contract in Chinese**

  Update `docs/zh/configuration/config-files.md` with the same example, numeric values, zero semantics, cache/persistence guarantees, hard-quota terminal behavior, Card/replay behavior, `Retry-After`, and `Esc`. Translate prose, but keep TOML keys, error classes, and numeric semantics identical to English.

- [ ] **Step 6: Run GREEN and documentation parity verification**

  Run:

  ```bash
  rtk cargo test --package neo-agent --bin neo -- config::tests::runtime_retry_defaults_and_loads_explicit_values --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent --bin neo -- modes::run::tests::agent_config_for_app_applies_runtime_config --exact --nocapture --include-ignored
  rtk rg -n "first_event_timeout_secs|stream_idle_timeout_secs|QuotaExhausted|Retry-After|Esc|100" docs/en/configuration/config-files.md docs/zh/configuration/config-files.md
  ```

  Expected: both exact tests pass. The search must show both keys and every named contract marker in both language files; manually compare the two retry sections for semantic parity.

- [ ] **Step 7: Review and commit Task 3**

  Require both review verdicts and resolve all Critical/Important findings. Run `rtk git diff --check`, stage exactly the seven Task 3 files, and commit:

  ```bash
  rtk git add crates/neo-agent/src/config/mod.rs crates/neo-agent/src/config/types.rs crates/neo-agent/src/config/loader.rs crates/neo-agent/src/modes/run/runtime/agent.rs crates/neo-agent/src/modes/run/mod.rs docs/en/configuration/config-files.md docs/zh/configuration/config-files.md
  rtk git commit -m "feat(config): expose stream retry timeouts"
  ```

---

### Task 4: Finish Retry Card Terminal and Replay Semantics

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/entry/render_status.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/tests/transcript_pane.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Test: `neo-tui` transcript integration tests and `neo-agent` interactive unit test

**Interfaces:**
- Consumes: existing `RetryStatusData`, lifecycle events, Task 1's `provider.quota_exhausted` metadata, and session replay events.
- Produces: an animated waiting/connecting Card, exact terminal retry wording, one quota detail row, no generic run-finished row, and durable exhaustion replay.

- [ ] **Step 1: Extend the existing retry Card test and record RED**

  In `retry_status_renders_fixed_waiting_connecting_and_exhausted_states`, render at activity frame 0, call `advance_animation_at_ms`, and render again. Assert waiting and connecting each move from `⠋` to `⠙`. Change the transport fixture message to `transport error: error decoding response body` and assert the rendered detail is exactly one `Network · error decoding response body` with no `Network · transport error:`.

  Exercise all terminal wording branches:

  ```text
  Reconnect failed · retry disabled
  Reconnect failed after 1 retry
  Reconnect failed after 5 retries
  ```

  Retain the `99/100` width/count coverage. Run:

  ```bash
  rtk cargo test --package neo-tui --test transcript_pane -- retry_status_renders_fixed_waiting_connecting_and_exhausted_states --exact --nocapture --include-ignored
  ```

  Expected: RED because retry rendering has no Card spinner, connecting is not treated as visible animation, terminal text says attempts, and the transport prefix is duplicated.

- [ ] **Step 2: Extend terminal dedup and quota detail tests and record RED**

  Extend `retry_exhaustion_suppresses_followup_error_card` by applying `RunFinished { stop_reason: Error }` after the existing follow-up `Error`; assert entry count remains unchanged and rendered output has no `runtime error`.

  Add `quota_exhausted_error_preserves_provider_detail`. Apply:

  ```rust
  AgentEvent::Error {
      turn: 1,
      message: "quota exhausted: balance is 0; purchase extra usage".into(),
      code: Some("provider.quota_exhausted".into()),
      retry_after: None,
  }
  ```

  Then apply `RunFinished(Error)`. Assert `Quota Exhausted` and `balance is 0; purchase extra usage` each appear exactly once, while `Check API key`, `quota exhausted:`, `runtime error`, and any `Reconnecting` text are absent.

  Run:

  ```bash
  rtk cargo test --package neo-tui --test transcript_pane -- retry_exhaustion_suppresses_followup_error_card --exact --nocapture --include-ignored
  rtk cargo test --package neo-tui --test transcript_pane -- quota_exhausted_error_preserves_provider_detail --exact --nocapture --include-ignored
  ```

  Expected: RED because `RunFinished(Error)` appends a generic status and quota errors do not yet render title plus provider detail.

- [ ] **Step 3: Write the durable replay test and record RED**

  Rename `replay_session_into_transcript_ignores_retry_lifecycle` to `replay_session_into_transcript_restores_only_retry_exhaustion`. The fixture must contain scheduled, started, resumed, exhausted, follow-up `Error`, `TurnFinished(Error)`, and `RunFinished(Error)` events. Assert there is exactly one `TranscriptEntry::RetryStatus`, it is finalized/exhausted, the render contains `Reconnect failed after 1 retry`, and no active `Reconnecting`, duplicate `Error`, or `runtime error` row exists.

  Run:

  ```bash
  rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::replay_session_into_transcript_restores_only_retry_exhaustion --exact --nocapture --include-ignored
  ```

  Expected: RED because replay currently drops `RetryExhausted` with the transient lifecycle.

- [ ] **Step 4: Implement the minimal Card rendering changes**

  Pass `activity_frame` from `render_message_entry` to `render_retry_status`. In the renderer, use the existing ten-frame braille sequence locally and prefix it only for waiting/connecting:

  ```rust
  const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
  let spinner = SPINNER[activity_frame % SPINNER.len()];
  ```

  Render exhaustion with three branches based on `data.retry`: zero, one, or plural. For `provider.transport_error`, remove one leading `transport error: ` from the display detail after choosing the `Network` title. JSONL/event messages remain unchanged.

  In both `TranscriptEntry::on_render_tick` and `TranscriptEntry::has_visible_animation`, treat `RetryPhase::Waiting | RetryPhase::Connecting` as animated. Keep `RetryPhase::Exhausted` cacheable and finalized.

- [ ] **Step 5: Implement one terminal error surface**

  In the `AgentEvent::Error` rendering branch, handle `provider.quota_exhausted` before the generic metadata-action branch:

  ```rust
  (Some("provider.quota_exhausted"), _) => {
      let detail = message
          .strip_prefix("quota exhausted: ")
          .unwrap_or(message);
      format!("✗ Quota Exhausted — {detail}")
  }
  ```

  Keep error severity. Change only `run_finished_notice(StopReason::Error)` to `None`; runtime/provider errors already have a specific `AgentEvent::Error` or the interactive turn error path, while `RunFinished` still stops footer activity. Preserve the existing MaxTokens and Cancelled notices.

- [ ] **Step 6: Restore only durable exhaustion on replay**

  Keep transient lifecycle events ignored, but route exhaustion through the normal transcript handler:

  ```rust
  AgentEvent::ApprovalRequested { .. }
  | AgentEvent::RetryScheduled { .. }
  | AgentEvent::RetryStarted { .. }
  | AgentEvent::RetryResumed { .. }
  | AgentEvent::RetrySucceeded { .. } => {}
  AgentEvent::RetryExhausted { .. } => transcript.apply_agent_event(event),
  ```

  Existing exhausted-card guards deduplicate the following `Error`; Step 5 prevents `RunFinished(Error)` from adding a generic row. Do not persist or restore transient countdown/connecting state.

- [ ] **Step 7: Run GREEN verification**

  Run:

  ```bash
  rtk cargo test --package neo-tui --test transcript_pane -- retry_status_renders_fixed_waiting_connecting_and_exhausted_states --exact --nocapture --include-ignored
  rtk cargo test --package neo-tui --test transcript_pane -- retry_exhaustion_suppresses_followup_error_card --exact --nocapture --include-ignored
  rtk cargo test --package neo-tui --test transcript_pane -- quota_exhausted_error_preserves_provider_detail --exact --nocapture --include-ignored
  rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::replay_session_into_transcript_restores_only_retry_exhaustion --exact --nocapture --include-ignored
  ```

  Expected: each exact test passes; the animation assertions prove both phases request/render changing frames, and replay contains exactly one terminal retry Card.

- [ ] **Step 8: Review and commit Task 4**

  Require clean spec-compliance and code-quality verdicts, fix all Critical/Important findings, and re-review. Run `rtk git diff --check`, stage exactly the six Task 4 files, and commit:

  ```bash
  rtk git add crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/entry/render_status.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs
  rtk git commit -m "fix(tui): finalize reconnect transcript state"
  ```

---

## Final Verification and Review

- [ ] Generate one review package covering the four implementation tasks and dispatch a fresh whole-feature reviewer against the approved spec. The reviewer must check cache identity, retry ownership, timeout races, quota false positives, config/docs parity, transcript terminal deduplication, and replay.
- [ ] Send all final Critical/Important findings to one fixer, rerun the exact tests covering each fix, and re-review the resulting range. Do not accept open Critical/Important findings.
- [ ] Run every exact test named in Tasks 1 through 4 against the final HEAD. Do not substitute a broad package/workspace test for this evidence.
- [ ] Run final hygiene:

  ```bash
  rtk cargo fmt --all --check
  rtk git diff --check
  rtk git status --short
  ```

- [ ] Confirm the implementation commits contain only the files listed by their tasks and the worktree has no uncommitted task changes. Do not push.
