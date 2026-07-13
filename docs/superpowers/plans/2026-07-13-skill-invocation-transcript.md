# Skill Invocation Transcript Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make automatic and manual skill invocations visible through one transcript component with muted source markers and explicit failure state.

**Architecture:** Replace the success-only `SkillActivated` event with a source-aware, outcome-aware `SkillInvocation` event. Emit automatic invocation events after the common tool execution result is available, feed manual slash activation through the same event shape, and render both with one transcript entry renderer.

**Tech Stack:** Rust 2024, serde, ratatui, cargo-nextest.

---

### Task 1: Normalize Skill Invocation Events

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Test: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] **Step 1: Extend the existing runtime skill test to require the semantic event**

In `ask_mode_skill_tool_runs_without_approval`, assert that the default parallel configuration emits:

```rust
assert!(events.iter().any(|event| matches!(
    event,
    AgentEvent::SkillInvocation {
        names,
        source: SkillInvocationSource::Auto,
        outcome: SkillInvocationOutcome::Activated,
        body,
    } if names == &["review".to_owned()] && body.is_empty()
)));
```

- [ ] **Step 2: Run the exact test and verify RED**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- ask_mode_skill_tool_runs_without_approval --exact --nocapture
```

Expected: compilation or assertion failure because `SkillInvocation` and its metadata types do not exist.

- [ ] **Step 3: Replace the old event and centralize runtime emission**

Define public serializable `SkillInvocationSource` and `SkillInvocationOutcome` enums in `events.rs`, replace `SkillActivated` with `SkillInvocation`, and re-export the types from `lib.rs` if the crate's existing event exports require it.

Remove the `SkillActivated` emission from `prepare_and_run_tool`. Introduce one `emit_tool_execution_finished` helper and call it at every sequential and parallel completion boundary, including invalid arguments and hook short-circuits. The helper emits exactly one semantic event immediately before the ordinary finished event for every `Skill` result:

```rust
if tool_call.name.as_ref() == "Skill" {
    emitter.emit(AgentEvent::SkillInvocation {
        names: vec![skill_name(arguments).to_owned()],
        source: SkillInvocationSource::Auto,
        outcome: if result.is_error {
            SkillInvocationOutcome::Failed
        } else {
            SkillInvocationOutcome::Activated
        },
        body: if result.is_error {
            result.content.clone()
        } else {
            format_skill_tool_arguments(arguments)
        },
    });
}
```

Use a focused helper for name recovery and event construction so sequential and parallel modes cannot diverge again. Do not retain `SkillActivated` or a compatibility emission branch.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the command from Step 2. Expected: one test passes.

- [ ] **Step 5: Add a failed-invocation assertion at the same runtime boundary**

Add one focused test using `FakeHarness` and an empty `SkillStore`, invoking `Skill` with `{"skill":"missing"}`. Assert one `SkillInvocation` event has `source: Auto`, `outcome: Failed`, name `missing`, and an unavailable-skill body.

- [ ] **Step 6: Run the exact failed-invocation test**

Run:

```bash
cargo test --package neo-agent-core --test runtime_turn -- automatic_missing_skill_emits_failed_skill_invocation --exact --nocapture
```

Expected: one test passes.

### Task 2: Render One Source-Aware Skill Component

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/entry/copy.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`
- Modify: `crates/neo-tui/src/shell/event_router.rs`
- Test: `crates/neo-tui/tests/transcript_pane.rs`

- [ ] **Step 1: Replace the current renderer test with the desired compact auto header**

Construct an automatic activated event with no body and assert the rendered transcript contains exactly the semantic header and no divider:

```text
✦ Skill activated: brainstorming · auto
```

Also assert no `TranscriptEntry::ToolRun` is created from the surrounding generic tool lifecycle.

- [ ] **Step 2: Run the exact TUI test and verify RED**

Run:

```bash
cargo test --package neo-tui --test transcript_pane -- skill_tool_call_renders_as_skill_activation_card_not_tool_card --exact --nocapture
```

Expected: compilation or assertion failure because the source-aware event and compact renderer are not implemented.

- [ ] **Step 3: Update the transcript entry and event handler**

Store `source: SkillInvocationSource` and `outcome: SkillInvocationOutcome` in `TranscriptEntry::SkillActivation`. Replace the obsolete constructor with `skill_invocation`, then update copying, equality/matching code, and expansion logic accordingly. Replace `apply_skill_goal_event` handling with the new event and pass all fields into the single entry constructor.

Render `· auto` or `· manual` in muted style. Use `✦ Skill activated:` with `status_warn` for activated entries and `✕ Skill failed:` with `status_error` for failed entries. Only append the divider and body rows when `body.trim()` is non-empty.

- [ ] **Step 4: Run the exact TUI test and verify GREEN**

Run the command from Step 2. Expected: one test passes.

- [ ] **Step 5: Add one focused failure rendering test**

Apply `SkillInvocation { source: Auto, outcome: Failed }`, render the pane, and assert the `✕ Skill failed` header, muted `auto` marker, and error body are visible without a generic `ToolRun`.

- [ ] **Step 6: Run the exact failure rendering test**

Run:

```bash
cargo test --package neo-tui --test transcript_pane -- failed_skill_tool_renders_semantic_failure_card --exact --nocapture
```

Expected: one test passes.

### Task 3: Route Manual Activation and Replay Through the Semantic Event

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Update the existing inline skill test to assert `Manual`**

Extend `inline_multi_skill_directives_activate_one_card_and_submit_stripped_prompt` so the single activation entry contains both skill names and `SkillInvocationSource::Manual`.

- [ ] **Step 2: Run the exact interactive test and verify RED**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::inline_multi_skill_directives_activate_one_card_and_submit_stripped_prompt --exact --nocapture --include-ignored
```

Expected: compilation or assertion failure because manual source is absent.

- [ ] **Step 3: Route slash activation through `AgentEvent::SkillInvocation`**

Replace `push_skill_invocation_entry`'s direct entry construction with one call to `TranscriptPane::apply_agent_event` using `source: Manual`, `outcome: Activated`, the aggregated names, and display body. Do not retain the direct constructor path.

Update session replay to apply recorded `SkillInvocation` events in the same match arm used for other semantic transcript events.

- [ ] **Step 4: Run the exact interactive test and verify GREEN**

Run the command from Step 2. Expected: one test passes.

- [ ] **Step 5: Add an end-to-end automatic interactive regression test**

Use `FakeHarness` with a real loaded test skill and the controller's normal default parallel runtime. Submit a prompt that produces a `Skill` tool call, wait for the active turn, then assert one automatic `SkillActivation` entry and zero `ToolRun` entries named `Skill`.

- [ ] **Step 6: Run the exact end-to-end test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::automatic_skill_invocation_renders_one_semantic_card --exact --nocapture --include-ignored
```

Expected: one test passes.

### Task 4: Focused Verification

**Files:**
- Verify only touched targets.

- [ ] **Step 1: Check formatting**

Run `cargo fmt --all --check`. Expected: exit 0.

- [ ] **Step 2: Run the three exact behavior tests**

Run the exact runtime, TUI, and interactive commands from Tasks 1-3. Expected: all exit 0.

- [ ] **Step 3: Run target-specific clippy**

Run:

```bash
cargo clippy -p neo-agent-core --lib -- -D clippy::all
cargo clippy -p neo-tui --lib -- -D clippy::all
cargo clippy -p neo-agent --bin neo -- -D clippy::all
```

Expected: all commands exit 0 without warnings.

- [ ] **Step 4: Review the scoped diff**

Run `git diff --check` and `git diff --` for the files named above. Confirm there is no obsolete `SkillActivated` variant, no duplicate manual renderer, and no unrelated worktree change.
