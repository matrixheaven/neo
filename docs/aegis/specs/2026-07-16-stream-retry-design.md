# Neo Stream Retry and Reconnect Design

**Date:** 2026-07-16; hardened 2026-07-18  
**Status:** Approved
**Scope:** `neo-ai`, `neo-agent-core`, `neo-agent`, `neo-tui`, session JSONL

## Motivation

Neo currently retries failures while opening an HTTP response, but a transport
failure after headers arrive is treated as a terminal stream error. A typical
failure is:

```text
Stream disconnected before completion: Transport error: network error: error decoding response body
```

The user receives no reconnect progress, cannot see the retry budget, and has
no clear way to interrupt a long provider-requested wait. The implementation
also has two retry owners: provider HTTP helpers and the runtime recovery path.
That makes request counts and session behavior unpredictable.

Neo will use one runtime-owned retry loop with exact request replay, mutable
attempt-local UI state, and a durable retry lifecycle.

The hardening revision also closes two observed gaps. A provider can remain
silent forever without producing either a stream event or an error, leaving
only the generic footer spinner. Providers can also report a permanent billing
quota failure using an HTTP status otherwise associated with authentication or
rate limiting. Neo must bound stream silence and distinguish permanent quota
exhaustion from transient rate limiting before it decides to retry.

## Goals

- Recover transient request and stream transport failures automatically.
- Show `Reconnecting n/max` in the existing inline transcript position.
- Preserve provider prompt-cache identity across ordinary retries.
- Ensure failed attempt output never enters canonical context or session replay.
- Honor provider `Retry-After` values with a cancellable countdown.
- Make the retry budget and stream-silence deadlines configurable from one
  global TOML section.
- Stop immediately on permanent quota exhaustion and preserve the provider's
  actionable error detail.
- Keep interactive, run, RPC, and child-agent behavior consistent.

## Non-goals

- Provider-specific continuation or response-id resume.
- Model fallback.
- Retrying tool execution.
- Automatically resuming an in-flight request after process crash.
- Provider- or model-specific retry overrides.
- Configurable backoff factor, jitter, or delay caps.
- Provider- or model-specific stream timeout overrides.
- A new hosted telemetry or metrics subsystem.

## Architecture and ownership

### `neo-ai`: one attempt only

Provider clients open one response and parse one stream. They do not sleep,
count attempts, or emit retry UI events. The shared HTTP helper's retry loop is
removed, along with `RequestOptions.retries` and the cancellation token that
exists only for that loop.

`AiStreamEvent` represents successful stream lifecycle events only. Provider
failures are returned as `Result<AiStreamEvent, AiError>` and are never encoded
as an in-band stream event.

### `neo-agent-core`: single retry owner

The runtime constructs one canonical `ChatRequest` for a model step. It owns:

- retry budget and attempt numbering;
- retryable-error classification decisions;
- backoff and `Retry-After` waiting;
- first-event and between-event stream deadlines;
- cancellation during waiting, connect, and body streaming;
- attempt transaction boundaries;
- retry lifecycle events.

Every ordinary retry clones and re-sends the frozen request. It does not rerun
context projection, compaction estimation, reminder injection, or tool-schema
construction. System prompt bytes, message order, tool order, model settings,
and `session_id` / prompt-cache identity remain stable.

### Attempt transaction

Text, thinking, and tool-call deltas may be forwarded live to the TUI, but are
provisional until the attempt completes. `MessageAppended` is emitted only for
the winning completed attempt. Tool execution starts only after that complete
message is available.

The session persistence layer buffers stream-detail events by `(turn, attempt)`.
`RetryScheduled` drops the failed buffer. A successful `MessageAppended` flushes
the winning buffer before writing the aggregate message. Thus a failed attempt
cannot poison `AgentContext` or leave duplicate partial output in replay.

## Retry state machine

```text
AttemptRunning(0)
   | retryable error
   v
RetryWaiting(1/max)
   | cancellable delay elapsed
   v
AttemptRunning(1)
   |- retryable error -> RetryWaiting(2/max)
   |- completed -------> Committed
   |- terminal error --> Failed
   `- Esc -------------> Cancelled
```

`attempt = 0` is the initial request. `retry = 1` is the first reconnect.
`max_retries = 5` means at most six requests. `max_retries = 0` ends after the
initial failure. A retry is consumed only when the next request is actually
issued; cancelling while waiting does not consume it.

## Error taxonomy

`AiError` uses the following canonical variants:

```text
Configuration
RateLimit { message, retry_after }
Auth
QuotaExhausted { message }
ContextOverflow
Server { status, message, retry_after }
Transport { message }
Protocol { message }
Cancelled
```

Retryable classes:

- `Transport`: DNS, TCP, TLS, request timeout, body decode failure, connection
  reset, and SSE EOF before the terminal marker.
- `RateLimit`: HTTP 429 or a provider-declared rate-limit code.
- `Server`: HTTP 5xx or a provider-declared overload/unavailable code.
- HTTP 408 maps to retryable `Transport`.

Terminal classes:

- 401 authentication failures;
- permanent quota exhaustion, including HTTP 402, structured
  `insufficient_quota`, `insufficient_balance`, `billing_limit_exceeded`,
  `usage_limit_exceeded`, and `payment_required` errors;
- HTTP 403 or 429 responses whose structured code or sanitized body identifies
  permanent quota exhaustion; otherwise 403 remains `Auth` and 429 remains
  retryable `RateLimit`;
- ordinary 4xx and configuration failures;
- context overflow, which remains owned by compaction recovery;
- user cancellation;
- deterministic protocol failures, including malformed UTF-8/JSON or an
  invalid complete SSE frame.

Body fallback classification is deliberately narrow and case-insensitive. It
matches only `usage limit for this billing cycle`, `purchase extra usage`,
`insufficient balance`, `insufficient credits`, `quota exhausted`, or
`billing limit exceeded`. Adding another phrase requires a provider fixture;
generic words such as `quota`, `limit`, or `billing` alone never make a
403/429 terminal.

A clean stream close without a completion marker is still an incomplete
transport and is retryable. A complete but invalid frame is a protocol error
and is not retried.

Silence is also an incomplete transport. The first-event deadline begins when
the provider stream is first polled and ends when the first normalized
`AiStreamEvent` arrives. After that, the idle deadline restarts after every
normalized event. Provider keepalive comments that do not produce an
`AiStreamEvent` do not reset either deadline. Expiry returns `Transport`
through the same attempt boundary as connection reset or premature EOF, so no
second retry owner is introduced.

## Backoff and configuration

The user-facing settings are:

```toml
[runtime.retry]
max_retries = 5
first_event_timeout_secs = 60
stream_idle_timeout_secs = 120
```

`max_retries` is a `u32`; `100` is valid. Zero disables automatic retry. The
default is five retries after the initial request.

`first_event_timeout_secs` bounds response opening plus the wait for the first
normalized model event. `stream_idle_timeout_secs` bounds silence between later
normalized events. Their defaults are 60 and 120 seconds respectively. Each is
a `u64`; zero disables only that deadline. Expiry is a retryable `Transport`
failure, while `Esc` remains terminal cancellation and wins a simultaneous
deadline race.

Without `Retry-After`, delay is exponential with additive jitter:

```text
retry 1: 500ms + 0..25% jitter
retry 2: 1s    + 0..25% jitter
retry 3: 2s    + 0..25% jitter
...
cap:     30s after jitter
```

Exponentiation uses saturating integer arithmetic. A provider `Retry-After`
delta-seconds or HTTP-date overrides local backoff and receives no jitter.
Past dates become zero delay. Values above 24 hours are capped at 24 hours.
Invalid values use local backoff.

The delay is awaited with `tokio::select!` against the active turn
cancellation token. `Esc` cancels the wait immediately.

## Lifecycle events

The runtime emits these structured events:

```text
RetryScheduled {
    turn, retry, max_retries, delay_ms, error_code, message
}
RetryStarted { turn, retry, max_retries }
RetryResumed { turn, retry }
RetrySucceeded { turn, retries_used }
RetryExhausted { turn, retries_used, error_code, message }
```

`RetryResumed` is emitted before the first delta of a new attempt. It means the
stream has produced a valid event, not that the attempt has completed. A later
failure can therefore schedule another retry.

When the budget is exhausted, the runtime emits `RetryExhausted` followed by
the existing `AgentEvent::Error` as the single terminal error surface. The
message includes the retry count and the final underlying cause.

## TUI behavior

Retry is one mutable live transcript entry at the original assistant position:

```text
⠋ Reconnecting 1/5 · retry in 12s · esc interrupt
  └ Network · error decoding response body
```

While the request is being issued, the header changes to `connecting`. On
`RetryResumed`, the entry is replaced in place by the new attempt's live
thinking/text/tool draft. No failed retry card is appended at the bottom.

The entry owns an animated spinner while waiting or connecting. It uses warning
styling while waiting and error styling after exhaustion. Details are
normalized and width-wrapped; the TUI removes redundant canonical prefixes such
as `transport error:` after it has already rendered the `Network` title. Full
error chains remain in logs and JSONL. Retry counts use stable width constraints
so `9/10` and `99/100` do not shift layout.

The runtime emits only `delay_ms`. The TUI derives the countdown from a local
monotonic `Instant` and schedules redraws at second boundaries. It does not
emit per-second events or write countdown chatter to JSONL.

The footer keeps the existing generic `working · esc interrupt` hint. Retry
attempt, error detail, and countdown have one visual owner: the inline retry
entry.

On success the entry disappears as a retry status and the winning live content
occupies the same position. On exhaustion it becomes the single terminal error
entry. Its header says `retry disabled`, `after 1 retry`, or `after N retries`;
it never calls the retry count an attempt count. The following generic `Error`
and `RunFinished(Error)` events must not append duplicate terminal status rows.
On user cancellation it follows existing interrupted-turn finalization.

`QuotaExhausted` never creates a reconnect entry. It renders one terminal error
entry containing the sanitized provider detail, rather than replacing that
detail with a generic `Check API key` action.

## Persistence, replay, and other surfaces

The current `SessionEventPersistence` is moved to a shared
`neo-agent-core::session` boundary and used by interactive, run, RPC, and child
agent writers. It must support flushing multiple events for one input because
winning attempt detail is released as a batch.

Retry lifecycle events are written immediately. Stream-detail events are held
by attempt and are written only for the winning attempt. Replay ignores
transient `RetryScheduled`, `RetryStarted`, `RetryResumed`, and
`RetrySucceeded` entries, but rehydrates `RetryExhausted` as the single
durable terminal retry card. Its following generic `Error` and
`RunFinished(Error)` events are deduped against that card. Export/debug readers
can inspect every lifecycle record.

If the process exits during backoff or an attempt, the next process does not
automatically resend the ambiguous request. Replay finalizes the open turn as
interrupted; the user must submit a new turn.

Child agents inherit the parent's global retry configuration. Their retry state
is rendered inside the corresponding Delegate/DelegateGroup/DelegateSwarm
transcript and never as a new top-level card.

`neo run --json` forwards all lifecycle events. Non-TTY human output keeps
assistant content on stdout and writes one plain retry line per scheduled retry
to stderr. TTY output uses the animated inline presentation.

## Verification plan

Focused tests should cover:

### `neo-ai`

- body transport decode failure maps to `Transport`;
- EOF before terminal marker maps to retryable `Transport`;
- malformed complete JSON/SSE maps to non-retryable `Protocol`;
- retryability for transport, 408, 429, 5xx, auth, context, and cancellation.
- HTTP 402 and permanent-quota 403/429 bodies map to terminal
  `QuotaExhausted`, while ordinary 403/429 behavior remains unchanged;
- structured `insufficient_quota` and `insufficient_balance` stream failures
  map to terminal `QuotaExhausted`.

### `neo-agent-core`

- first attempt emits partial text then disconnects and second attempt succeeds;
- both outbound `ChatRequest` values are equal;
- failed partial text never reaches `MessageAppended` or context;
- event order matches the state machine;
- zero retries, high retry counts, exhaustion, non-retryable errors, and
  cancellation during backoff;
- first-event silence and between-event silence become retryable transport
  failures, restart the frozen request, and remain cancellable;
- local backoff, `Retry-After`, 24-hour cap, and invalid-header fallback.

### `neo-agent`

- default, zero-disabled, and explicit high values for all three
  `[runtime.retry]` settings;
- JSONL contains lifecycle plus winning attempt only;
- failed attempt detail is absent from JSONL;
- JSON output and non-TTY stderr behavior.

### `neo-tui`

- one entry is mutated across retries;
- countdown formatting and long durations;
- animated waiting and connecting states;
- resume replaces the retry entry in place;
- exhaustion uses retry-count wording and becomes one final error entry without
  a generic run-finished duplicate;
- transport detail has no redundant prefix and quota detail remains visible;
- replay restores only exhausted retry terminal cards.

## Migration and cleanup

Implementation deletes the provider-level retry loop, `RequestOptions.retries`,
the old in-band `AiStreamEvent::Error`, and tests that assert the removed
behavior. Config and user documentation are updated in both English and
Chinese. Both configuration guides document the three settings, defaults,
zero-disable behavior, first-event versus idle semantics, retryable classes,
terminal quota behavior, request-count meaning, cache-stable frozen replay,
`Retry-After`, and `Esc`. No compatibility alias or second retry path is
retained.
