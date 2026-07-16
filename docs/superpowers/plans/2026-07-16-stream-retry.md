# Neo Stream Retry and Reconnect Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (- [ ]) syntax for tracking.

**Goal:** Add one runtime-owned, exact-replay retry loop for transient model request/stream failures with configurable retry count, inline reconnect UX, and transactional session persistence.

**Architecture:** neo-ai performs one provider attempt and returns typed transport/protocol errors. neo-agent-core freezes one ChatRequest per model step and owns retry state, backoff, cancellation, lifecycle events, and attempt commit boundaries. neo-agent persists only the winning attempt while neo-tui renders a mutable reconnect entry at the original transcript position.

**Tech Stack:** Rust 2024 workspace, tokio and tokio-util::CancellationToken, futures::Stream, reqwest, serde/toml, JSONL sessions, Neo transcript components, FakeHarness/FakeModelClient.

## Global Constraints

- Runtime retry is the only retry owner; provider HTTP helpers must not retain a second retry loop.
- max_retries counts requests after the initial request; default 5, 0 disables retry, and 100 is valid.
- Ordinary retries clone and resend the frozen ChatRequest; do not rerun projection, compaction estimation, reminders, or tool-schema construction.
- Retryable errors are transport interruption, HTTP 408, HTTP 429, HTTP 5xx, and explicit provider overload/unavailable errors; deterministic protocol errors, auth, ordinary 4xx, context overflow, and cancellation are terminal.
- Local backoff is 500ms * 2^(retry - 1) plus 0..25% jitter, capped at 30s after jitter; valid Retry-After overrides it and is capped at 24h.
- Failed attempt deltas are live-only and never enter AgentContext or replayable canonical session content.
- Retry status is one mutable inline transcript entry; successful recovery removes the status, exhaustion converts it to one terminal error.
- Retry waiting and streaming are cancellable by the existing active-turn CancellationToken and Esc path.
- No provider/model retry overrides, provider-specific continuation, model fallback, tool execution retry, or post-crash automatic resend.
- Preserve cross-platform behavior and keep unrelated dirty worktree changes untouched.
- Update English and Chinese configuration documentation together.

## File Map

| File | Responsibility |
| --- | --- |
| crates/neo-ai/src/error.rs | Canonical AiError variants, codes, retryability, and tests |
| crates/neo-ai/src/options.rs | Remove provider-level retry fields |
| crates/neo-ai/src/providers/common/http.rs | One-shot response opening |
| crates/neo-ai/src/providers/common/error.rs | Provider error mapping and Retry-After metadata |
| crates/neo-ai/src/providers/{anthropic,google,openai/compatible,openai/responses}.rs | Stream body/EOF/protocol classification |
| crates/neo-ai/src/types.rs | Remove in-band AiStreamEvent::Error |
| crates/neo-agent/src/config/{mod.rs,types.rs,loader.rs} | File/default/loading model for runtime.retry |
| crates/neo-agent/src/config/mutations.rs | Materialize retry defaults when writing a new config |
| crates/neo-agent/src/modes/run/runtime/agent.rs | Copy app retry config into AgentConfig |
| crates/neo-agent-core/src/runtime/config.rs | Core retry value carried by AgentConfig |
| crates/neo-agent-core/src/events.rs | Retry lifecycle event schema |
| crates/neo-agent-core/src/runtime/retry.rs | Delay, Retry-After, counters, cancellable wait |
| crates/neo-agent-core/src/runtime/{mod.rs,turn_loop.rs,stream_aggregator.rs} | One-attempt aggregation and retry loop |
| crates/neo-agent-core/src/session/event_persistence.rs | Shared attempt buffer and durable-event projection |
| crates/neo-agent-core/src/session/mod.rs | Export shared persistence |
| crates/neo-agent/src/modes/run/mod.rs | Use shared persistence |
| crates/neo-agent-core/src/multi_agent/runtime.rs | Child persistence and retry activity |
| crates/neo-tui/src/transcript/entry/{mod.rs,render_status.rs} | Retry data and rendering |
| crates/neo-tui/src/transcript/{store.rs,pane.rs,event_handler.rs} | In-place entry lifecycle and live reset |
| crates/neo-tui/src/shell/event_router.rs | Keep streaming mode during retry |
| crates/neo-agent/src/modes/run/output/json.rs | Stable JSON lifecycle mapping |
| docs/en/configuration/config-files.md | English retry config docs |
| docs/zh/configuration/config-files.md | Chinese retry config docs |

---

### Task 1: Make Provider Attempts One-Shot and Classify Stream Failures

**Files:**
- Modify: crates/neo-ai/src/error.rs
- Modify: crates/neo-ai/src/options.rs
- Modify: crates/neo-ai/src/providers/common/http.rs
- Modify: crates/neo-ai/src/providers/common/error.rs
- Modify: crates/neo-ai/src/providers/anthropic.rs
- Modify: crates/neo-ai/src/providers/google.rs
- Modify: crates/neo-ai/src/providers/openai/compatible.rs
- Modify: crates/neo-ai/src/providers/openai/responses.rs
- Modify: crates/neo-ai/src/types.rs
- Test: unit tests in the same neo-ai modules

**Interfaces:**
- Consumes: existing ProviderError, ChatRequest, ModelClient, and provider stream_response functions.
- Produces: AiError::Transport, AiError::Protocol, retryable status mappings, and a one-shot provider stream for Task 3.

- [ ] Step 1: Write failing error taxonomy tests.

Add assertions for the new variants and stable codes:

~~~
assert!(AiError::Transport { message: "eof".into() }.is_retryable());
assert!(AiError::Server {
    status: 503,
    message: "busy".into(),
    retry_after: None,
}.is_retryable());
assert!(!AiError::Protocol { message: "invalid json".into() }.is_retryable());
assert_eq!(
    AiError::Transport { message: "x".into() }.code(),
    "provider.transport_error"
);
~~~

- [ ] Step 2: Run the focused test and verify it fails.

Run:

~~~
cargo nextest run -p neo-ai --lib code_returns_domain_dot_reason
~~~

Expected: FAIL because Transport/Protocol and Server.retry_after do not exist.

- [ ] Step 3: Replace the old stream/network split with canonical variants.

Implement:

~~~
pub enum AiError {
    Configuration { message: String },
    RateLimit { message: String, retry_after: Option<Duration> },
    Auth { message: String },
    ContextOverflow { message: String },
    Server { status: u16, message: String, retry_after: Option<Duration> },
    Transport { message: String },
    Protocol { message: String },
    Cancelled,
}
~~~

Update code() and is_retryable() exhaustively. Map transport and server errors to retryable codes. Map protocol errors to a non-retryable code. Update all constructors and exact-match tests; do not retain Stream or Network aliases.

- [ ] Step 4: Remove provider-level retry inputs and make response opening one-shot.

Delete RequestOptions.retries and RequestOptions.cancel_token. Replace the shared open_response loop with a direct provider call:

~~~
self.open_response_once(&request)
    .await
    .map_err(ProviderError::into_ai_error)
~~~

Remove DEFAULT_MAX_ATTEMPTS, local backoff constants, sleep_cancellable, and the loop tests from common/http.rs. Keep response status and Retry-After parsing in ProviderError.

- [ ] Step 5: Classify body errors, incomplete EOF, and complete protocol errors in all four providers.

Use these branches in every stream_response implementation:

~~~
StreamChunk::Data(Err(err)) => vec![Err(AiError::Transport {
    message: format!("transport error: {err}"),
})],
StreamChunk::End => state.finish(),
~~~

Make finish() return Transport when the stream ends before its terminal marker. Make malformed UTF-8/JSON or invalid complete frame structure return Protocol. Remove AiStreamEvent::Error; provider failures must be Err in the stream result.

- [ ] Step 6: Run provider-focused tests and commit.

Run:

~~~
cargo nextest run -p neo-ai --lib is_retryable_for_each_variant
cargo fmt --all --check
~~~

Expected: selected tests pass and old provider retry-loop tests are gone.

Commit:

~~~
git add crates/neo-ai/src/error.rs crates/neo-ai/src/options.rs crates/neo-ai/src/types.rs crates/neo-ai/src/providers
git commit -m "refactor(neo-ai): make provider streams one-shot"
~~~

### Task 2: Add Global Retry Configuration and Core Wiring

**Files:**
- Modify: crates/neo-agent/src/config/mod.rs
- Modify: crates/neo-agent/src/config/types.rs
- Modify: crates/neo-agent/src/config/loader.rs
- Modify: crates/neo-agent/src/config/mutations.rs
- Modify: crates/neo-agent/src/modes/run/runtime/agent.rs
- Modify: crates/neo-agent-core/src/runtime/config.rs
- Test: loader and agent_config_for_app tests

**Interfaces:**
- Consumes: FileRuntimeConfig, RuntimeConfig, and agent_config_for_app.
- Produces: RuntimeRetryConfig { max_retries: u32 } and AgentConfig.max_retries.

- [ ] Step 1: Add failing config loader tests.

Use runtime_from_file_for_tests with an explicit 100:

~~~
let config = runtime_from_file_for_tests(Some(FileRuntimeConfig {
    retry: Some(FileRuntimeRetryConfig { max_retries: Some(100) }),
    ..FileRuntimeConfig::default()
}));
assert_eq!(config.retry.max_retries, 100);
assert_eq!(RuntimeConfig::default().retry.max_retries, 5);
~~~

- [ ] Step 2: Run the focused test and verify it fails.

Run:

~~~
cargo nextest run -p neo-agent --bin neo agent_config_for_app_applies_runtime_config
~~~

Expected: FAIL because retry fields are not defined.

- [ ] Step 3: Add one canonical runtime/file config path.

Add:

~~~
pub struct RuntimeRetryConfig {
    pub max_retries: u32,
}

impl Default for RuntimeRetryConfig {
    fn default() -> Self { Self { max_retries: 5 } }
}

pub(crate) struct FileRuntimeRetryConfig {
    pub(crate) max_retries: Option<u32>,
}
~~~

Add retry: RuntimeRetryConfig to RuntimeConfig and retry: Option<FileRuntimeRetryConfig> to FileRuntimeConfig. Merge it in runtime_from_file() with unwrap_or(5). Do not add provider/model fallback fields or legacy aliases.

Update the config materialization path in config/mutations.rs so a first-written
config serializes the default [runtime.retry] table with max_retries = 5.

- [ ] Step 4: Carry the value into AgentConfig.

Add pub max_retries: u32 to AgentConfig, initialize it to 5 in AgentConfig::for_model, and assign it in modes/run/runtime/agent.rs:

~~~
agent_config.max_retries = config.runtime.retry.max_retries;
~~~

- [ ] Step 5: Run config tests and commit.

Run:

~~~
cargo nextest run -p neo-agent --bin neo agent_config_for_app_applies_runtime_config
cargo fmt --all --check
~~~

Expected: default is 5, explicit 100 round-trips, and selected tests pass.

Commit:

~~~
git add crates/neo-agent/src/config/mod.rs crates/neo-agent/src/config/types.rs crates/neo-agent/src/config/loader.rs crates/neo-agent/src/config/mutations.rs crates/neo-agent/src/modes/run/runtime/agent.rs crates/neo-agent-core/src/runtime/config.rs
git commit -m "feat(config): add global model retry budget"
~~~

### Task 3: Define Retry Lifecycle Events and Implement the Runtime Loop

**Files:**
- Modify: crates/neo-agent-core/src/events.rs
- Modify: crates/neo-agent-core/src/runtime/mod.rs
- Create: crates/neo-agent-core/src/runtime/retry.rs
- Modify: crates/neo-agent-core/src/runtime/turn_loop.rs
- Modify: crates/neo-agent-core/src/runtime/stream_aggregator.rs
- Modify: crates/neo-agent-core/src/runtime/error.rs
- Test: crates/neo-agent-core/tests/runtime_turn.rs and retry.rs tests

**Interfaces:**
- Consumes: AgentConfig.max_retries, one-shot ModelClient::stream_chat, AiError::is_retryable(), and CancellationToken.
- Produces: Retry lifecycle events and successful-only MessageAppended.

- [ ] Step 1: Add event serialization tests before implementation.

Use:

~~~
let event = AgentEvent::RetryScheduled {
    turn: 1,
    retry: 1,
    max_retries: 5,
    delay_ms: 500,
    error_code: "provider.transport_error".into(),
    message: "body closed".into(),
};
let json = serde_json::to_string(&event).expect("serialize retry event");
let decoded: AgentEvent = serde_json::from_str(&json).expect("deserialize retry event");
assert_eq!(decoded, event);
~~~

- [ ] Step 2: Run the event test and verify it fails.

Run:

~~~
cargo nextest run -p neo-agent-core --lib error_with_code_serializes
~~~

Expected: FAIL because retry lifecycle variants do not exist.

- [ ] Step 3: Add exact lifecycle event variants.

Use:

~~~
RetryScheduled {
    turn: u32,
    retry: u32,
    max_retries: u32,
    delay_ms: u64,
    error_code: String,
    message: String,
},
RetryStarted { turn: u32, retry: u32, max_retries: u32 },
RetryResumed { turn: u32, retry: u32 },
RetrySucceeded { turn: u32, retries_used: u32 },
RetryExhausted {
    turn: u32,
    retries_used: u32,
    error_code: String,
    message: String,
},
~~~

Mark them as non-context-mutating in EventEmitter::apply_to_context.

- [ ] Step 4: Implement deterministic scheduling in runtime/retry.rs.

Expose:

~~~
pub(super) fn retry_delay(error: &AiError, retry: u32) -> Duration;
pub(super) async fn wait_for_retry(
    delay: Duration,
    cancel_token: &CancellationToken,
) -> Result<(), AgentRuntimeError>;
~~~

retry_delay parses Retry-After from RateLimit/Server, clamps it to 24 hours, otherwise computes 500ms * 2^(retry - 1) with random 0..25% additive jitter and a final 30-second cap. Use saturating arithmetic. Test jitter by range and Retry-After by exact values.

- [ ] Step 5: Refactor stream aggregation into one attempt.

Change run_model_turn in stream_aggregator.rs into an attempt function that does not emit TurnStarted, TurnFinished, or MessageAppended on an error. Keep ModelTurnState deltas live, but only emit MessageAppended after MessageEnd has completed the attempt. Convert the AiStreamEvent::Error match arm into direct Err(AiError) propagation and let cancellation return AiError::Cancelled without appending a partial assistant message.

- [ ] Step 6: Wrap attempts in one runtime retry loop.

In turn_loop.rs, emit TurnStarted once and use this control shape inside run_model_turn_with_recovery:

~~~
let mut retries_used = 0;
loop {
    if retries_used > 0 {
        emitter.emit(AgentEvent::RetryStarted {
            turn, retry: retries_used, max_retries: config.max_retries,
        });
    }
    match run_model_attempt(...).await {
        Ok(message) => {
            if retries_used > 0 {
                emitter.emit(AgentEvent::RetrySucceeded { turn, retries_used });
            }
            return Ok(message);
        }
        Err(AgentRuntimeError::Model(error))
            if error.is_retryable() && retries_used < config.max_retries =>
        {
            retries_used += 1;
            let delay = retry_delay(&error, retries_used);
            emitter.emit(AgentEvent::RetryScheduled {
                turn,
                retry: retries_used,
                max_retries: config.max_retries,
                delay_ms: delay.as_millis().try_into().unwrap_or(u64::MAX),
                error_code: error.code().to_owned(),
                message: error.to_string(),
            });
            wait_for_retry(delay, cancel_token).await?;
        }
        Err(error) => return Err(error),
    }
}
~~~

Emit RetryResumed immediately before the first event of a restarted stream. On exhaustion, emit RetryExhausted and return the final model error through the existing AgentEvent::Error path. Keep context overflow on compaction recovery; the compacted request must re-enter the same network retry helper with overflow recovery disabled for that nested request.

- [ ] Step 7: Add runtime regression tests and commit.

Use FakeHarness::from_result_turns in runtime_turn.rs:

~~~
let harness = FakeHarness::from_result_turns([
    vec![
        Ok(AiStreamEvent::MessageStart { id: "a".into() }),
        Ok(AiStreamEvent::TextDelta { text: "partial".into() }),
        Err(AiError::Transport { message: "eof".into() }),
    ],
    vec![
        Ok(AiStreamEvent::MessageStart { id: "b".into() }),
        Ok(AiStreamEvent::TextDelta { text: "complete".into() }),
        Ok(AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        }),
    ],
]);
~~~

Assert equal outbound requests, lifecycle order, no failed MessageAppended/context text, zero budget, exhaustion, protocol failure, and cancellation during backoff.

Run:

~~~
cargo nextest run -p neo-agent-core --test runtime_turn stream_retries_transport_error
cargo nextest run -p neo-agent-core --test runtime_turn retry_does_not_append_failed_attempt
cargo fmt --all --check
~~~

Expected: all named tests pass.

Commit:

~~~
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/runtime crates/neo-agent-core/tests/runtime_turn.rs
git commit -m "feat(runtime): retry transient model stream failures"
~~~

### Task 4: Make Session Persistence Attempt-Transactional

**Files:**
- Create: crates/neo-agent-core/src/session/event_persistence.rs
- Modify: crates/neo-agent-core/src/session/mod.rs
- Modify: crates/neo-agent/src/modes/run/mod.rs
- Modify: crates/neo-agent-core/src/multi_agent/runtime.rs
- Test: event_persistence.rs and existing run persistence tests

**Interfaces:**
- Consumes: AgentEvent stream and existing delegate/swarm progress throttling.
- Produces: SessionEventPersistence::persisted_events(&mut self, event: &AgentEvent) -> Vec<AgentEvent>.

- [ ] Step 1: Move the existing persistence gate without changing behavior.

Move SessionEventPersistence, PersistedAgentProgress, and delegate/swarm throttling from modes/run/mod.rs to session/event_persistence.rs. Export it from session/mod.rs and update run-mode imports. Preserve current progress gates.

- [ ] Step 2: Add failing attempt-buffer tests.

Use the exact projection contract:

~~~
let mut persistence = SessionEventPersistence::default();
assert!(persistence.persisted_events(&text_delta("failed")).is_empty());
assert_eq!(persistence.persisted_events(&retry_scheduled()).len(), 1);
assert!(persistence.persisted_events(&message_appended("winning")).len() >= 1);
~~~

The JSONL projection must contain RetryScheduled, winning detail, and MessageAppended, but never the failed delta.

- [ ] Step 3: Implement the shared attempt buffer.

Buffer model stream-detail events (message/thinking/tool-call lifecycle and token usage) until a successful assistant MessageAppended. RetryScheduled discards the failed buffer. Winning MessageAppended returns the buffer followed by the aggregate event. Retry lifecycle events return immediately. Keep user, tool-result, shell, delegate, and unrelated events on their existing direct path.

- [ ] Step 4: Update all writers to append the returned vector.

Change append_streaming_event and the non-streaming collection path:

~~~
for persisted in persistence.persisted_events(event) {
    writer.append_event(&persisted).await?;
}
~~~

Wrap child-agent JSONL writes in multi_agent/runtime.rs with the same gate; do not create a child-specific implementation.

- [ ] Step 5: Verify and commit.

Run:

~~~
cargo nextest run -p neo-agent --bin neo append_streaming_event_suppresses_duplicate_user_message_externally
cargo nextest run -p neo-agent-core --lib session_event_persistence_discards_failed_attempt
cargo nextest run -p neo-agent --bin neo replay_session_into_transcript_does_not_duplicate_text_delta_aggregate_without_finish
~~~

Expected: failed deltas are absent from JSONL; winning detail replays once; delegate/swarm persistence remains green.

Commit:

~~~
git add crates/neo-agent-core/src/session crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent/src/modes/run/mod.rs
git commit -m "feat(session): persist only winning model attempts"
~~~

### Task 5: Add the Inline Mutable Retry Entry to Neo TUI

**Files:**
- Modify: crates/neo-tui/src/transcript/entry/mod.rs
- Modify: crates/neo-tui/src/transcript/entry/render_status.rs
- Modify: crates/neo-tui/src/transcript/store.rs
- Modify: crates/neo-tui/src/transcript/pane.rs
- Modify: crates/neo-tui/src/transcript/event_handler.rs
- Modify: crates/neo-tui/src/shell/event_router.rs
- Test: crates/neo-tui/tests/transcript_pane.rs, transcript_store.rs, app_shell.rs

**Interfaces:**
- Consumes: retry lifecycle AgentEvent variants and existing live-entry mutation/cache APIs.
- Produces: RetryStatusData, TranscriptEntry::RetryStatus, and pane methods that update/remove one retry entry by turn.

- [ ] Step 1: Add render/data tests for waiting, connecting, and exhaustion.

Use fixed values and assert stripped output contains:

~~~
Reconnecting 1/5 · retry in 12s · esc interrupt
Network · error decoding response body
~~~

Also assert 99/100 is stable and a one-hour delay renders 1h 04m 38s.

- [ ] Step 2: Add RetryStatusData and TranscriptEntry::RetryStatus.

Define a render-only payload with turn, retry, max_retries, phase, delay_ms, started_at_ms, error_code, and message. Add Waiting, Connecting, and Exhausted phases. Reuse existing status colors and wrapping helpers; do not add a second card system.

- [ ] Step 3: Add one-entry upsert/remove/reset operations.

Implement:

~~~
pub fn upsert_retry_status(&mut self, data: RetryStatusData) -> bool;
pub fn clear_retry_status(&mut self, turn: u32) -> bool;
pub fn reset_live_model_attempt(&mut self, turn: u32) -> bool;
~~~

upsert_retry_status mutates an existing non-finalized entry at the same index. reset_live_model_attempt removes only the current turn's provisional assistant/thinking/tool draft and never moves completed transcript entries.

- [ ] Step 4: Route lifecycle events and derive countdown redraws locally.

Handle lifecycle events in TranscriptPane::apply_agent_event before ordinary message events. RetryScheduled resets the live attempt and upserts Waiting; RetryStarted upserts Connecting; RetryResumed clears the status and resets the new live attempt; RetryExhausted upserts Exhausted; RetrySucceeded clears the status.

Use existing live-entry tick/render scheduling to derive remaining seconds from Instant; do not emit one event per second. Keep event_router.rs in streaming mode and leave the footer's generic working hint unchanged.

- [ ] Step 5: Test position stability and commit.

Run:

~~~
cargo nextest run -p neo-tui --test transcript_pane retry_status_mutates_original_position
cargo nextest run -p neo-tui --test transcript_store retry_status_countdown_formats_long_delay
cargo nextest run -p neo-tui --test app_shell retry_keeps_working_mode_until_turn_finishes
~~~

Expected: one entry is mutated in place, resumed output keeps that position, and replay creates no historical reconnect entry.

Commit:

~~~
git add crates/neo-tui/src/transcript crates/neo-tui/src/shell/event_router.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-tui/tests/transcript_store.rs crates/neo-tui/tests/app_shell.rs
git commit -m "feat(tui): show inline model reconnect status"
~~~

### Task 6: Expose Retry Lifecycle in JSON, Human Output, and Child Progress

**Files:**
- Modify: crates/neo-agent/src/modes/run/output/json.rs
- Modify: crates/neo-agent/src/modes/run/mod.rs
- Modify: crates/neo-agent-core/src/multi_agent/runtime.rs
- Test: JSON output tests and multi_agent_runtime.rs

**Interfaces:**
- Consumes: retry lifecycle events and child runtime event forwarding.
- Produces: stable JSON records, stderr retry notices for non-TTY output, and child-card retry activity.

- [ ] Step 1: Add stable JSON mapping tests.

Assert map_event produces:

~~~
{"type":"retry_scheduled","turn":1,"retry":1,"maxRetries":5,"delayMs":500,"errorCode":"provider.transport_error"}
{"type":"retry_started","turn":1,"retry":1,"maxRetries":5}
{"type":"retry_resumed","turn":1,"retry":1}
{"type":"retry_succeeded","turn":1,"retriesUsed":1}
~~~

retry_exhausted includes the final errorCode and message.

- [ ] Step 2: Implement JSON mapping without changing assistant stdout semantics.

Add lifecycle arms to StableJsonState::map_lifecycle_event. Keep assistant delta/message aggregation unchanged; lifecycle records are additive and do not turn failed provisional text into canonical messages.

- [ ] Step 3: Add non-TTY stderr notices.

At the existing run-mode event forwarding boundary, print one plain line for RetryScheduled:

~~~
Reconnecting 1/5 in 500ms: Network error: body closed
~~~

Do not write notices to stdout or emit ANSI redraw control sequences for non-TTY output.

- [ ] Step 4: Surface retry activity inside child cards.

In multi_agent/runtime.rs, handle RetryScheduled and RetryStarted by updating existing latest activity text to bounded Reconnecting n/max. Do not create a root-level transcript entry. Keep Bayesian aggregate and child ordering untouched.

- [ ] Step 5: Run output and child tests and commit.

Run:

~~~
  cargo nextest run -p neo-agent --bin neo retry_events_are_stable_json_records
cargo nextest run -p neo-agent-core --test multi_agent_runtime retry_activity_stays_inside_child_snapshot
~~~

Expected: JSON consumers see lifecycle events, stdout remains assistant-only, stderr receives plain notices, and child retry state remains inside the Delegate/Swarm transcript.

Commit:

~~~
git add crates/neo-agent/src/modes/run/output/json.rs crates/neo-agent/src/modes/run/mod.rs crates/neo-agent-core/src/multi_agent
git commit -m "feat(output): expose model retry lifecycle"
~~~

### Task 7: Document the Canonical Configuration and Remove Dead Paths

**Files:**
- Modify: docs/en/configuration/config-files.md
- Modify: docs/zh/configuration/config-files.md
- Test: config parsing tests from Task 2

**Interfaces:**
- Consumes: final runtime config and typed error/event names from Tasks 1-6.
- Produces: one documented runtime.retry.max_retries path with no compatibility alias.

- [ ] Step 1: Add English configuration docs.

Document runtime.retry.max_retries, default 5, zero-disable behavior, count excluding initial request, Retry-After up to 24h, and Esc interruption. Include:

~~~
[runtime.retry]
max_retries = 5
~~~

- [ ] Step 2: Add matching Chinese docs.

Use the same field name, default, count semantics, cap, and cancellation behavior in docs/zh/configuration/config-files.md.

- [ ] Step 3: Delete stale retry paths and update exhaustive matches.

Run:

~~~
rg -n "request\\.options\\.retries|cancel_token|AiStreamEvent::Error|AiError::Stream|AiError::Network|DEFAULT_MAX_ATTEMPTS" crates
~~~

Every hit must be removed or migrated to the canonical runtime path. Do not add aliases or fallback branches for removed fields.

- [ ] Step 4: Run narrow cross-boundary verification.

Run:

~~~
cargo fmt --all --check
cargo clippy -p neo-ai --lib -- -D clippy::all
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-tui --lib -- -D clippy::all
cargo nextest run -p neo-ai --lib transport_error_is_retryable
cargo nextest run -p neo-agent-core --test runtime_turn stream_retries_transport_error
cargo nextest run -p neo-tui --test transcript_pane retry_status_mutates_original_position
git diff --check
~~~

Expected: all named targets pass and no stale provider retry path or removed error variant remains.

- [ ] Step 5: Commit docs and cleanup.

~~~
git add docs/en/configuration/config-files.md docs/zh/configuration/config-files.md
git commit -m "docs: document model stream retry configuration"
~~~

## Plan Self-Review

- Spec coverage: provider one-shot behavior is Task 1; global config is Task 2; runtime state/events/backoff are Task 3; transactional JSONL is Task 4; TUI position/countdown/cancel behavior is Task 5; JSON, stderr, and child agents are Task 6; bilingual docs and canonical cleanup are Task 7.
- No provider-specific continuation, model fallback, tool retry, post-crash resend, or timing knobs appear in any task.
- Interfaces used by later tasks are named in earlier task outputs: AiError precedes runtime retry, AgentConfig.max_retries precedes the loop, lifecycle events precede persistence/TUI/output, and SessionEventPersistence::persisted_events precedes writer migration.
- No placeholder task language remains; every implementation task includes exact paths, interfaces, code shape, focused commands, expected results, and commit boundaries.
