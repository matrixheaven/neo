# Session Title Generation Fix Design

Date: `2026-08-02`

Status: `approved design`

## 1. Purpose

Fix Neo's automatic session-title generation so it reliably produces an
LLM-generated title for reasoning models, stops silently degrading to a prompt
truncation, and stops re-paying a failing LLM call at the end of every turn.

## 2. Problem And Evidence

Observed on `deepseek-v4-flash` through the local cache probe
(`target/cache-probe/20260801-162932-d0877b`):

- request 59 (the session-title call, `system: "Generate a concise session
  title. Return only the title, no quotes."`, `max_tokens: 32`,
  `temperature: 0.2`, no `metadata`, no `tools`) returned HTTP 200 with
  `assistant_text_bytes: 0` and `thinking_bytes: 129`;
- the model emitted a `thinking` block and **no text**, so the collected title
  was empty;
- `generate_session_title` fell back to `one_line(prompt, 40)`, storing
  `title: "阅读并完成：[cache-probe-dashboard-usability.…"` with
  `title_model: null` in `sessions.metadata.json`;
- the terminal title therefore shows a 40-character prompt truncation, and the
  user never sees an LLM title.

### Root cause

1. The title request sets `max_tokens: Some(32)` while sending no `thinking`
   control (`reasoning: Off`). On the Anthropic-compatible wire, Neo maps
   `ReasoningSelection::Off` to **omitting** the `thinking` field
   (`crates/neo-ai/src/providers/anthropic.rs:145-150`). DeepSeek reasoner
   models default to emitting a `thinking` block when the field is absent, so
   the 32-token output budget is consumed by thinking and the title text is
   never produced. The main agent requests avoid this by sending
   `thinking: {"type": "enabled", "budget_tokens": 32768, ...}`.
2. After a fallback, `title_model` stays `null`, so the one-shot guard
   (`record.name.is_some() || record.title_model.is_some()`) does not stop the
   next turn from re-invoking the failing LLM call
   (`crates/neo-agent/src/modes/run/session_mgmt.rs:133`).
3. The failure is silent: `match ... { _ => (fallback, None) }` swallows the
   error with no log.

## 3. Confirmed Scope

### In scope

- `crates/neo-ai/src/options.rs`: add `RequestOptions.disable_reasoning: bool`
  (default `false`) — an explicit "force reasoning off even when the provider
  would default it on" switch for background requests.
- `crates/neo-ai/src/providers/anthropic.rs`: in the
  `ReasoningSelection::Off` branch, when `disable_reasoning` is set, emit
  `thinking: {"type": "disabled"}` on the wire; otherwise keep the current
  omit behavior. Other providers ignore the flag.
- `crates/neo-agent/src/modes/run/session_mgmt.rs`:
  - build the title request through a pure `title_request()` helper with
    `max_tokens: Some(512)` and `disable_reasoning: true`;
  - make title generation one-shot: the guard stops after any recorded title
    (`name` or `title_updated_at` set), including the fallback case;
  - log a `tracing::warn!` when title generation errors or returns empty and
    the fallback is used.

### Out of scope

- Asynchronous (non-blocking) title generation: `record_initial_session_title`
  stays awaited at the three turn-end call sites. Disabling reasoning removes
  the dominant latency (a 1.3 s observed call becomes a fast non-reasoning
  call); full async is a follow-up product decision.
- Choosing a dedicated title model: the title keeps using the currently
  resolved model.
- Changing `ReasoningSelection::Off` to always emit `thinking: disabled` for
  every request: that would alter existing provider behavior for all
  reasoning-off traffic; the explicit flag keeps the change scoped to
  background requests.
- Cache probe changes: the title request remains an unattributable
  out-of-band request and correctly stays `first-req` in the dashboard.
- Title quality, `clean_session_title`, `/rename`, session-list display.

## 4. Design

### 4.1 `RequestOptions.disable_reasoning`

```rust
// crates/neo-ai/src/options.rs
#[serde(default)]
pub disable_reasoning: bool,
```

Default `false`. Documented as: anthropic-compatible providers serialize it as
`thinking: {"type": "disabled"}`; other providers ignore it. Used by
background requests (e.g. session titles) that must stay fast and
deterministic. All existing `RequestOptions { .. }` literals use
`..RequestOptions::default()` (verified for every construction site), so
adding the field is non-breaking.

### 4.2 Anthropic wire mapping

```rust
ReasoningSelection::Off => {
    if request.options.disable_reasoning {
        body["thinking"] = json!({ "type": "disabled" });
    }
    if let Some(temperature) = request.options.temperature {
        body["temperature"] = json!(rounded_f64(temperature));
    }
}
```

`Enabled`/`Effort`/`BudgetTokens` branches are unchanged. The flag is a no-op
for `openai`, `openai-compatible`, and `google` providers (their `Off`
handling already omits reasoning parameters).

### 4.3 Title request construction

Extract a pure helper and call it from `generate_session_title`:

```rust
fn title_request(
    model: neo_ai::ModelSpec,
    prompt: &str,
    assistant_text: &str,
) -> neo_ai::ChatRequest
```

- messages/system text unchanged (same prompt format as today);
- `tools: Vec::new()`;
- `options.max_tokens: Some(512)` (room for a short title plus a bounded
  thinking block if a provider ignores the disable flag);
- `options.temperature: Some(0.2)`;
- `options.disable_reasoning: true`;
- rest `..RequestOptions::default()`.

### 4.4 One-shot guard

```rust
if record.name.is_some() || record.title_updated_at.is_some() {
    return;
}
```

`title_updated_at` is set by `record_title` in both the LLM-success and the
fallback path, so a session never re-runs title generation once a title
(LLM or fallback) has been recorded. `SessionRecord.title` is not usable here
because it is the computed display chain (always `Some`).

### 4.5 Failure visibility

- empty title → `tracing::warn!("session {}: title generation returned an
  empty title; using a prompt truncation fallback", turn.session_id)`;
- error → `tracing::warn!(error = ?error, "session {}: title generation
  failed; using a prompt truncation fallback", turn.session_id)`.

## 5. Files

| File | Change |
| --- | --- |
| `crates/neo-ai/src/options.rs` | add `disable_reasoning` field + default |
| `crates/neo-ai/src/providers/anthropic.rs` | `Off` branch emits `thinking: disabled` when flagged; new unit test |
| `crates/neo-agent/src/modes/run/session_mgmt.rs` | `title_request()` helper, one-shot guard, warn logs; new unit test |

## 6. Compatibility Boundary

- `RequestOptions` gains a defaulted field: all existing literals use
  `..Default` and keep compiling; serialized config round-trips with
  `#[serde(default)]`.
- Requests with `reasoning: Off` and `disable_reasoning: false` are byte-identical
  on the wire (no `thinking` key, same as today).
- Only the session-title request flips the flag; no other Neo request shape
  changes.
- A provider that rejects `thinking: {"type": "disabled"}` surfaces as a title
  error, which the existing fallback already swallows into a prompt
  truncation — the same degraded behavior as today, never worse.

## 7. Test Obligations

- `crates/neo-ai` (lib): anthropic `request_body` test asserting
  `thinking == {"type": "disabled"}` when `disable_reasoning: true`, and no
  `thinking` key when `false` (regression for the current omit behavior).
- `crates/neo-agent` (bin `neo`): `title_request()` test asserting
  `max_tokens == Some(512)`, `temperature == Some(0.2)`,
  `disable_reasoning == true`, `tools.is_empty()`, system text, and the
  `User prompt:/Assistant response:` message format.
- No strict TDD; post-change regression tests only.

## 8. Verification

```bash
cargo test -p neo-ai --lib providers::anthropic::tests::request_body_reasoning_off_disable_mapping --exact
cargo test -p neo-agent --bin neo -- modes::run::session_mgmt::tests::title_request_is_fast_and_deterministic --exact
cargo fmt --all --check
cargo clippy -p neo-ai --lib -- -D clippy::all
cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

## 9. Risk And Rollback

- **Residual**: if a provider ignores or rejects `thinking: disabled`, the
  empty-title fallback path still applies (same UX as today, plus a warn log).
  The flag is additive and reversible by reverting the `session_mgmt.rs`
  one-liner; no persisted schema changes.
- **Rollback**: revert the three files; the previous behavior (32-token title
  request, per-turn retry, silent fallback) returns.

## 10. Repair And Retirement

- **Repair track**: root cause is the title request configuration
  (`max_tokens: 32` + no thinking control) interacting with reasoning models.
  Canonical owner is `generate_session_title` in `session_mgmt.rs`, with the
  wire capability owned by the anthropic provider.
- **Retirement track**: the silent per-turn retry behavior (guard keyed on
  `title_model`) is retired in favor of a one-shot guard keyed on
  `title_updated_at`. No fallback path is deleted; the prompt-truncation
  fallback remains as the last-resort degraded behavior with a log line.
