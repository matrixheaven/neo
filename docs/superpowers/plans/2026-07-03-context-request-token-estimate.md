# Complete Context Request Token Estimate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the TUI footer `ctx a/b` used-token estimate include the full model request shape, especially tool schemas, instead of only `ChatRequest.messages`.

**Architecture:** Add a request-level estimator in `neo-agent-core` that sums estimated tokens for messages and tool specs. Keep provider-reported usage on the existing `TokenUsage` path; this change only affects `ContextWindowUpdated.used_tokens`.

**Tech Stack:** Rust 2024, `neo-ai::ChatRequest`, `neo-ai::ToolSpec`, `cargo nextest`.

---

### Task 1: Request-Level Estimator

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/tokens.rs`
- Modify: `crates/neo-agent-core/src/runtime/turn_loop.rs`
- Test: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Write the failing test**

Add a runtime test that sends a very small user message with a large `ToolSpec` and asserts the first `ContextWindowUpdated.used_tokens` is larger than message-only accounting could produce.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_context_window_estimate_includes_tool_schemas
```

Expected: the new assertion fails because the current footer estimate only calls `estimate_chat_messages_tokens(&request.messages)`.

- [ ] **Step 3: Implement minimal code**

In `tokens.rs`, add:

```rust
pub(crate) fn estimate_chat_request_tokens(request: &ChatRequest) -> usize
```

It must sum:
- every effective request message
- role labels for message overhead
- assistant tool call names and arguments
- every tool `name`, `description`, and serialized `input_schema`
- thinking text when it is present in the request
- ASCII text as 4 chars per token, non-ASCII text as 1 char per token

Then update both `ContextWindowUpdated` emission sites in `turn_loop.rs` to call the request-level estimator.

- [ ] **Step 4: Verify GREEN**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_context_window_estimate_includes_tool_schemas
```

Expected: PASS.

- [ ] **Step 5: Verify adjusted existing exact tests**

Run:

```bash
cargo nextest run -p neo-agent-core --test runtime_turn runtime_streams_one_turn_text_and_updates_context
```

Expected: PASS after updating the exact expected `ContextWindowUpdated` token values to the new role-aware estimate.

No git mutation is part of this plan because the active Neo workspace policy requires explicit per-instance authorization for commits and other git mutations.
