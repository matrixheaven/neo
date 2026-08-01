# Session Title Generation Fix — Implementation Plan

## Goal

Make Neo's automatic session-title generation reliable for reasoning models:
produce an LLM title instead of silently degrading to a 40-char prompt
truncation, stop re-invoking a failing title call at every turn end, and log
when the fallback is used.

## Architecture

- `crates/neo-ai` owns the provider-neutral request options and the
  Anthropic-compatible wire serialization. A new explicit "force reasoning
  off" flag on `RequestOptions` maps to `thinking: {"type": "disabled"}` on
  the anthropic wire.
- `crates/neo-agent` owns session metadata and the title-generation flow
  (`modes/run/session_mgmt.rs`): the title request is built by a pure helper
  with adequate `max_tokens` and the reasoning-off flag; the one-shot guard
  stops after any recorded title; failures are logged.
- No probe, TUI, config, or persistence-schema changes.

## Tech Stack

Rust workspace (edition 2024), `cargo`/`cargo-nextest`, `serde_json`,
`tracing` (already used in `modes/run`), `FakeModelClient` where needed.

## Baseline / Authority Refs

- Spec: `docs/aegis/specs/2026-08-02-session-title-generation-fix-design.md`
  (this plan implements it).
- Evidence: probe run `target/cache-probe/20260801-162932-d0877b` request 59
  (HTTP 200, `assistant_text_bytes: 0`, `thinking_bytes: 129`);
  `~/.neo/sessions/wd_neo_eb208ec56c5c/sessions.metadata.json` shows the
  stored fallback title with `title_model: null`.
- No ADR or baseline update triggered: internal crate API field addition with
  a default, no durable architecture change.

## Compatibility Boundary

- `RequestOptions` gains `#[serde(default)] disable_reasoning: bool`; every
  existing struct literal uses `..RequestOptions::default()` (verified), so
  compilation and serialization round-trips are preserved.
- Requests with `reasoning: Off` and `disable_reasoning: false` stay
  byte-identical on the wire (no `thinking` key).
- Only the session-title request sets the flag; no other Neo request changes.
- A provider rejecting `thinking: disabled` degrades to today's fallback (no
  worse than current behavior), now with a warn log.

## TDD Route

```text
TDD Route:
- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression unit tests for the touched behavior
- Reason: user requested spec + plan + fix without a strict TDD mandate
- Verification: exact cargo test targets per task, fmt, clippy
```

## Verification (final gate)

```bash
cargo test -p neo-ai --lib providers::anthropic::tests::request_body_reasoning_off_disable_mapping --exact
cargo test -p neo-agent --bin neo -- modes::run::session_mgmt::tests::title_request_is_fast_and_deterministic --exact
cargo fmt --all --check
cargo clippy -p neo-ai --lib -- -D clippy::all
cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

---

## Task 1: `disable_reasoning` flag in neo-ai + anthropic wire mapping

**Files**

- modify `crates/neo-ai/src/options.rs`
- modify `crates/neo-ai/src/providers/anthropic.rs`

**Why**: the title bug's wire-level root cause is that `reasoning: Off`
omits the `thinking` field and the model defaults to thinking. A scoped,
explicit "force reasoning off" must exist without changing behavior for every
`Off` request.

**Change Necessity**: no non-code option exists (wire serialization lives in
code); minimum boundary is the `RequestOptions` field + the anthropic `Off`
branch. Decision: `code-change`.

**Impact/Compatibility**: additive defaulted field; `false` keeps today's wire
bytes. Other providers (`openai`, `openai-compatible`, `google`) ignore the
flag — no changes there.

### Step 1.1 — add the field

In `crates/neo-ai/src/options.rs`, in `pub struct RequestOptions`, directly
after `pub replay_reasoning: bool,` (line 352) insert:

```rust
    /// Explicitly disable reasoning even when the provider would otherwise
    /// default to emitting a reasoning/thinking block. Anthropic-compatible
    /// providers serialize this as `thinking: {"type": "disabled"}`; other
    /// providers ignore it. Used by background requests (e.g. session titles)
    /// that must stay fast and deterministic.
    #[serde(default)]
    pub disable_reasoning: bool,
```

In `impl Default for RequestOptions` (line 362), after
`replay_reasoning: true,` insert:

```rust
            disable_reasoning: false,
```

### Step 1.2 — map it in the anthropic provider

In `crates/neo-ai/src/providers/anthropic.rs`, replace the
`ReasoningSelection::Off` arm (lines 145-150) with:

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

### Step 1.3 — regression unit test

In `crates/neo-ai/src/providers/anthropic.rs`, inside `mod tests` (starts at
line 838, has `use super::*;`), append:

```rust
    #[test]
    fn request_body_reasoning_off_disable_mapping() {
        let base = ChatRequest {
            model: ModelSpec {
                provider: ProviderId("anthropic".to_owned()),
                model: "claude-test".to_owned(),
                api: ApiKind::AnthropicMessages,
                capabilities: ModelCapabilities::tool_chat(),
            },
            messages: vec![ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "hello".to_owned(),
                }],
            }],
            tools: Vec::new(),
            options: RequestOptions::default(),
        };

        let plain = request_body(&base).unwrap();
        assert!(
            plain.pointer("/thinking").is_none(),
            "reasoning Off without the flag must keep omitting the thinking field"
        );

        let disabled = request_body(&ChatRequest {
            options: RequestOptions {
                disable_reasoning: true,
                ..RequestOptions::default()
            },
            ..base
        })
        .unwrap();
        assert_eq!(
            disabled.pointer("/thinking"),
            Some(&json!({ "type": "disabled" })),
            "explicit disable must serialize to thinking disabled"
        );
    }
```

If the test module lacks `ProviderId`/`ApiKind`/`ModelSpec` imports, add them
to the `use super::*;` line's crate imports (they exist at crate root:
`crate::{ModelSpec, ProviderId}` and `crate::types::ApiKind`).

### Step 1.4 — verify

```bash
cargo test -p neo-ai --lib providers::anthropic::tests::request_body_reasoning_off_disable_mapping --exact
cargo fmt --all --check
cargo clippy -p neo-ai --lib -- -D clippy::all
```

### Step 1.5 — commit

```bash
git add crates/neo-ai/src/options.rs crates/neo-ai/src/providers/anthropic.rs
git commit -m "feat(ai): support explicit reasoning disable for background requests"
```

---

## Task 2: reliable session title generation in neo-agent

**Files**

- modify `crates/neo-agent/src/modes/run/session_mgmt.rs`

**Why**: the title request must stop failing on reasoning models
(`max_tokens: 32` + no thinking control), the session must not re-invoke a
failing title call every turn, and fallbacks must be visible in logs.

**Change Necessity**: the fix requires code in the canonical owner
(`generate_session_title`/`record_initial_session_title`); no non-code option.
Minimum boundary: `title_request` helper + guard condition + two `tracing::warn!`
calls + a unit test. Decision: `code-change`.

**Impact/Compatibility**: only the session-title request changes shape; the
fallback behavior is preserved as last resort.

### Step 2.1 — imports

Replace the import at the top of `session_mgmt.rs` (line 6):

```rust
use neo_ai::{ChatMessage, ContentPart, RequestOptions};
```

with:

```rust
use neo_ai::{ChatMessage, ChatRequest, ContentPart, ModelSpec, RequestOptions};
```

### Step 2.2 — pure `title_request` helper

Replace the body of `generate_session_title` (lines 151-193) so the request
construction moves into a testable pure helper. The full replacement:

```rust
fn title_request(
    model: ModelSpec,
    prompt: &str,
    assistant_text: &str,
) -> ChatRequest {
    ChatRequest {
        model,
        messages: vec![
            ChatMessage::System {
                content: vec![ContentPart::Text {
                    text: "Generate a concise session title. Return only the title, no quotes."
                        .to_owned(),
                }],
            },
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: format!(
                        "User prompt:\n{}\n\nAssistant response:\n{}",
                        one_line(prompt, 500),
                        one_line(assistant_text, 500)
                    ),
                }],
            },
        ],
        tools: Vec::new(),
        options: RequestOptions {
            max_tokens: Some(512),
            temperature: Some(0.2),
            disable_reasoning: true,
            ..RequestOptions::default()
        },
    }
}

async fn generate_session_title(
    config: &AppConfig,
    prompt: &str,
    assistant_text: &str,
) -> anyhow::Result<(String, String)> {
    let model = super::runtime::resolve_model(config)?;
    let client = super::runtime::resolve_model_client(config, &model)?;
    let model_label = format!("{}/{}", model.provider.0, model.model);
    let request = title_request(model, prompt, assistant_text);
    let events = client.stream_chat(request).collect::<Vec<_>>().await;
    let mut title = String::new();
    for event in events {
        if let neo_ai::AiStreamEvent::TextDelta { text } = event? {
            title.push_str(&text);
        }
    }
    Ok((clean_session_title(&title), model_label))
}
```

### Step 2.3 — one-shot guard

In `record_initial_session_title`, replace the guard (line 133):

```rust
    if record.name.is_some() || record.title_model.is_some() {
```

with:

```rust
    if record.name.is_some() || record.title_updated_at.is_some() {
```

Rationale: `title_updated_at` is set by `record_title` on both the LLM-success
and the fallback path, so a session never re-runs title generation after any
title was recorded. `record.title` is the computed display chain (always
`Some`) and cannot be used here.

### Step 2.4 — failure visibility

Replace the match in `record_initial_session_title` (lines 138-142):

```rust
    let (title, model_label) =
        match generate_session_title(config, prompt, &turn.assistant_text).await {
            Ok((title, model_label)) if !title.is_empty() => (title, Some(model_label)),
            Ok((_, _)) => {
                tracing::warn!(
                    "session {}: title generation returned an empty title; \
                     using a prompt truncation fallback",
                    turn.session_id
                );
                (fallback, None)
            }
            Err(error) => {
                tracing::warn!(
                    error = ?error,
                    "session {}: title generation failed; using a prompt truncation fallback",
                    turn.session_id
                );
                (fallback, None)
            }
        };
```

### Step 2.5 — unit test for the helper

Append a `#[cfg(test)] mod tests` at the end of `session_mgmt.rs` (the file has
no test module today):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::types::ApiKind;
    use neo_ai::{ModelCapabilities, ProviderId};

    fn spec() -> ModelSpec {
        ModelSpec {
            provider: ProviderId("deepseek".to_owned()),
            model: "deepseek-test".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::tool_chat(),
        }
    }

    #[test]
    fn title_request_is_fast_and_deterministic() {
        let request = title_request(
            spec(),
            "read the handoff and complete it",
            "I completed the handoff.",
        );

        assert_eq!(request.options.max_tokens, Some(512));
        assert_eq!(request.options.temperature, Some(0.2));
        assert!(
            request.options.disable_reasoning,
            "title requests must not trigger provider reasoning"
        );
        assert!(request.tools.is_empty(), "title requests carry no tools");

        let mut system = String::new();
        let mut user = String::new();
        for message in &request.messages {
            match message {
                ChatMessage::System { content } => {
                    system = content_text(content, "system").unwrap();
                }
                ChatMessage::User { content } => {
                    user = content_text(content, "user").unwrap();
                }
                _ => {}
            }
        }
        assert_eq!(
            system,
            "Generate a concise session title. Return only the title, no quotes."
        );
        assert_eq!(
            user,
            "User prompt:\nread the handoff and complete it\n\nAssistant response:\nI completed the handoff."
        );
    }
}
```

Note: `content_text` is a private helper already used by `generate_session_title`
via the anthropic provider; in this crate it is `super::output`-free — verify
the call compiles by checking whether `content_text` is in scope in
`session_mgmt.rs`. If it is not exported from the same module, replace the
`content_text(content, ...)` calls with a local extraction:

```rust
                ChatMessage::System { content } => {
                    system = content.iter().filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.clone()),
                        _ => None,
                    }).collect::<Vec<_>>().join("");
                }
```

Use whichever variant compiles; keep the assertions unchanged.

### Step 2.6 — verify

```bash
cargo test -p neo-agent --bin neo -- modes::run::session_mgmt::tests::title_request_is_fast_and_deterministic --exact
cargo fmt --all --check
cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

### Step 2.7 — commit

```bash
git add crates/neo-agent/src/modes/run/session_mgmt.rs
git commit -m "fix(agent): make session title generation reliable for reasoning models"
```

---

## Task 3: docs index, final gate, closeout

**Files**

- append `docs/aegis/INDEX.md`
- (already created) `docs/aegis/specs/2026-08-02-session-title-generation-fix-design.md`
- (this file) `docs/aegis/plans/2026-08-02-session-title-generation-fix.md`

**Why**: workspace index hygiene; final verification gate before closeout.

### Step 3.1 — append INDEX rows

Add these two rows to the top of the `docs/aegis/INDEX.md` table (after the
header), matching the existing format:

```text
| 2026-08-02 | spec | docs/aegis/specs/2026-08-02-session-title-generation-fix-design.md | Session Title Generation Fix Design |
| 2026-08-02 | plan | docs/aegis/plans/2026-08-02-session-title-generation-fix.md | Session Title Generation Fix Implementation Plan |
```

### Step 3.2 — final gate

```bash
cargo test -p neo-ai --lib providers::anthropic::tests::request_body_reasoning_off_disable_mapping --exact
cargo test -p neo-agent --bin neo -- modes::run::session_mgmt::tests::title_request_is_fast_and_deterministic --exact
cargo fmt --all --check
cargo clippy -p neo-ai --lib -- -D clippy::all
cargo clippy -p neo-agent --bin neo -- -D clippy::all
git diff --check
```

### Step 3.3 — commit

```bash
git add docs/aegis/INDEX.md docs/aegis/specs/2026-08-02-session-title-generation-fix-design.md docs/aegis/plans/2026-08-02-session-title-generation-fix.md
git commit -m "docs(dev): spec and plan for session title generation fix"
```

## Risks

- A provider that rejects or ignores `thinking: {"type": "disabled"}` degrades
  to today's fallback (warn-logged); never worse than current behavior.
- `content_text` availability in `session_mgmt.rs` — Step 2.5 provides a
  fallback extraction so the test compiles either way.
- The plan does not change `crates/` beyond the three named files, does not
  touch `.gitignore` or other concurrent files, and makes no push/branch/worktree
  operations.

## Rollback

Revert the three code files (Task 1 and Task 2 commits) to restore the previous
behavior; no persisted schema or config migration exists.

## Repair / Retirement

- Repair: title-request configuration (canonical owner `generate_session_title`)
  and the anthropic `Off` wire mapping.
- Retired: the silent per-turn retry guard keyed on `title_model`; replaced by
  a one-shot guard keyed on `title_updated_at`. The prompt-truncation fallback
  remains as last resort, now logged.
