# Inline skill activation rendering implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix duplicate user-message rendering when `/skill:<name>` is used with paste/image markers, so the prompt renders as a single `SkillActivation` transcript card.

**Architecture:** Move paste/image marker expansion into `activate_skill_directives` so the `SkillActivation` card body is computed from the same expanded content the runtime will later echo. Return both the raw stripped body (for turn submission and skill context) and the display body (for suppression) from `activate_skill_directives`. Update both callers and add an integration test.

**Tech Stack:** Rust, Cargo, neo-agent, neo-tui, cargo-nextest

---

## File structure

| File | Responsibility |
|---|---|
| `crates/neo-agent/src/modes/interactive/slash_commands.rs` | Parses and activates skill directives. Modified to expand markers and return display text alongside raw body. |
| `crates/neo-agent/src/modes/interactive/mod.rs` | Submits user prompts. Modified to use returned display text for `pending_skill_user_message_to_suppress`. |
| `crates/neo-agent/src/modes/interactive/tests.rs` | Integration tests. New test exercises a paste marker inside an inline skill prompt. |

---

### Task 1: Expand markers inside `activate_skill_directives`

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs:205-236`

- [ ] **Step 1: Change signature and body**

Change the return type from `Result<String>` to `Result<(String, String)>` where the first element is the raw stripped body and the second is the expanded display body. Expand markers using the same helper the turn uses, but keep the raw body for skill context.

```rust
pub(super) fn activate_skill_directives(
    &mut self,
    directives: InlineSkillDirectives,
) -> Result<(String, String)> {
    let skill_store = self
        .skill_store
        .as_ref()
        .context("skill store not loaded")?;
    let mut names = Vec::new();
    let mut loaded_blocks = Vec::new();
    for invocation in &directives.invocations {
        let skill = skill_store
            .get(&invocation.name)
            .with_context(|| format!("skill `{}` not found", invocation.name))?;
        let (expanded_skill, _) =
            expand_slash_skill(&invocation.name, &invocation.args, skill)?;
        names.push(invocation.name.clone());
        loaded_blocks.push(render_loaded_skill_block(
            skill,
            invocation.args.as_str(),
            expanded_skill.as_str(),
        ));
    }

    let expanded_content = crate::prompt::parts::expand_prompt_markers(
        &directives.body,
        &self.paste_store,
        &self.image_attachment_store,
    );
    let display_body = content_to_display_text(&expanded_content);

    self.push_skill_invocation_entry(names, &display_body);
    self.pending_skill_context = Some(render_user_slash_skill_context(
        &directives.invocations,
        &loaded_blocks,
        directives.body.as_str(),
    ));
    Ok((directives.body, display_body))
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p neo-agent --lib`
Expected: compilation errors from the two unchanged callers (fixed next).

---

### Task 2: Update callers to use the new return value

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs:1277-1302`
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs:164-177`

- [ ] **Step 1: Update inline skill submission in `submit_current_prompt`**

Replace the inline skill block with the version that captures the display body for suppression:

```rust
if let Some(directives) = parse_inline_skill_directives(&prompt) {
    if directives
        .invocations
        .iter()
        .any(|invocation| invocation.name.is_empty())
    {
        self.push_status("Usage: /skill:<name> [args]");
        return Ok(());
    }
    let (stripped_prompt, display_body) = match self.activate_skill_directives(directives) {
        Ok(pair) => pair,
        Err(err) => {
            self.push_status(format!("Skill error: {err}"));
            return Ok(());
        }
    };
    if stripped_prompt.trim().is_empty() {
        self.clear_submitted_prompt();
        return Ok(());
    }
    self.pending_skill_user_message_to_suppress = Some(display_body);
    let Some(prompt) = self.submit_prompt_text(stripped_prompt) else {
        return Ok(());
    };
    self.start_turn_from_submitted_prompt(prompt)?;
    self.drain_active_turn().await?;
    return self.start_pending_background_question_followups().await;
}
```

- [ ] **Step 2: Update pure slash-command handler**

In `handle_skill_slash_command`, ignore the returned tuple but keep the success/failure logic:

```rust
fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives) {
    if directives
        .invocations
        .iter()
        .any(|invocation| invocation.name.is_empty())
    {
        self.push_status("Usage: /skill:<name> [args]");
    } else {
        match self.activate_skill_directives(directives) {
            Ok(_) => self.clear_submitted_prompt(),
            Err(err) => self.push_status(format!("Skill error: {err}")),
        }
    }
}
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p neo-agent --lib`
Expected: clean compile.

---

### Task 3: Add integration test for paste marker suppression

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs` (after line 2106)

- [ ] **Step 1: Write the test**

```rust
#[tokio::test]
async fn inline_skill_directive_with_paste_marker_renders_one_card() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let seen_requests = std::sync::Arc::clone(&requests);
    let paste_text = "line one\nline two\nline three";
    let expected_display = format!("{paste_text}review this");
    let expanded_for_event = expected_display.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_requests = std::sync::Arc::clone(&seen_requests);
            let expanded_for_event = expanded_for_event.clone();
            async move {
                seen_requests.lock().expect("requests lock").push(request);
                Ok(vec![AgentEvent::MessageAppended {
                    message: AgentMessage::user_text(expanded_for_event),
                }])
            }
        },
    );
    controller.skill_store = Some(skill_store_with_two_prompt_skills());
    controller.paste_store.insert(1, paste_text.to_owned());

    controller.type_text("/skill:skill_one [paste #1 +3 lines]review this");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("skill activation succeeds");

    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    let entries = transcript_entries(&controller);
    let skill_card = entries
        .iter()
        .find(|entry| matches!(entry, TranscriptEntry::SkillActivation { .. }))
        .expect("one skill activation card");
    assert!(matches!(
        skill_card,
        TranscriptEntry::SkillActivation { names, body, .. }
            if names == &vec!["skill_one".to_owned()] && body == &expected_display
    ));

    assert!(
        !entries.iter().any(|entry| matches!(entry, TranscriptEntry::UserMessage(text) if text == &expected_display)),
        "expanded skill activation body should not be rendered again as a user message"
    );

    let requests = requests.lock().expect("requests lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].prompt, vec![Content::text(expected_display)]);
}
```

- [ ] **Step 2: Run the new test and confirm it passes**

Run: `cargo nextest run -p neo-agent --lib inline_skill_directive_with_paste_marker_renders_one_card`
Expected: PASS.

- [ ] **Step 3: Run all inline skill tests**

Run: `cargo nextest run -p neo-agent --lib inline_skill_directive`
Expected: all PASS.

---

### Task 4: Final verification and commit

**Files:**
- Run commands in repo root `/Users/chenyuanhao/Workspace/neo`

- [ ] **Step 1: Format check**

Run: `cargo fmt --all --check`
Expected: no formatting issues.

- [ ] **Step 2: Targeted lint**

Run: `cargo clippy -p neo-agent --lib -- -D clippy::all`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/neo-agent/src/modes/interactive/slash_commands.rs \
        crates/neo-agent/src/modes/interactive/mod.rs \
        crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "fix(tui): expand markers before rendering inline skill activation card"
```

---

## Self-review

**Spec coverage:**
- Single SkillActivation card: Task 1 expands markers for the card body; Task 2 uses expanded body for suppression.
- Inline `/skill:` anywhere in prompt: unchanged parser, so still supported.
- Skill context keeps raw body: Task 1 passes `directives.body.as_str()` to `render_user_slash_skill_context`.
- Test: Task 3.

**Placeholder scan:** All code blocks are complete; no TBD/TODO.

**Type consistency:** `activate_skill_directives` returns `Result<(String, String)>` in all tasks; callers destructure consistently.

## Execution handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-03-inline-skill-activation-rendering-plan.md`.**

Two execution options:

**1. Subagent-Driven (recommended)** — Dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using `executing-plans`, batch execution with checkpoints.

Which approach would you like?
