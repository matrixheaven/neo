# Interactive Preflight Skills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a generic TUI interactive preflight contract, migrate `/init` onto it, and use it for `/skill:self-evo` and the new `/skill:create-skill` workflow.

**Architecture:** Add a small `interactive_preflight` module under interactive mode and make slash dispatch query it before starting local workflows. Runtime, not the model, mechanically triggers preflight from parsed workflow facts plus current `PermissionMode`. Skill workflows use the existing skill-context path, with a small generated injection turn only when a skill-only invocation needs the model to start immediately.

**Tech Stack:** Rust 2024, `neo-agent`, `neo-agent-core`, `neo-tui` choice picker overlays, existing `PermissionMode`, existing built-in skill Markdown loader, focused `cargo test --package ... --bin/--lib -- <exact test> --exact` verification.

---

## Scope Check

This spec is one coherent change set: one reusable preflight abstraction, three workflow users (`/init`, `self-evo`, `create-skill`), and docs. It should stay in one implementation plan because the skill behavior depends on the generic preflight contract. Keep commits gated by Neo policy: **do not run `git add`, `git commit`, branch, reset, checkout, stash, push, or any git mutation unless the user explicitly authorizes that exact command. Subagents must never run git mutations.**

## File Structure

- Create `crates/neo-agent/src/modes/interactive/interactive_preflight.rs`
  - Owns `WorkflowId`, `WorkflowInvocation`, `InteractivePreflightSpec`, `PreflightChoice`, `PreflightAction`, `PendingInteractiveWorkflow`, and helper functions that decide whether a parsed workflow needs preflight.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`
  - Registers the new module and replaces `pending_init_instruction` with `pending_interactive_workflow`.
  - Adds a helper to start a skill-only generated injection turn after skill context has been activated.
- Modify `crates/neo-agent/src/modes/interactive/slash_commands.rs`
  - Opens generic preflight instead of init-specific preflight.
  - Routes `/skill:` directives through preflight detection before activation.
- Modify `crates/neo-agent/src/modes/interactive/dialog_results.rs`
  - Resolves generic preflight choices and starts the stored pending continuation.
- Modify `crates/neo-agent/src/modes/interactive/init_command.rs`
  - Deletes init-specific preflight types and keeps only init prompt/validator behavior.
- Modify `crates/neo-agent/src/modes/interactive/tests.rs`
  - Updates `/init` preflight assertions and adds skill preflight behavior tests.
- Modify `crates/neo-agent-core/src/skills/builtin/mod.rs`
  - Includes the new built-in `create-skill`.
- Modify `crates/neo-agent-core/src/skills/builtin/self-evo.md`
  - Requires concrete scope for no-argument use and requires verification in generated skills.
- Create `crates/neo-agent-core/src/skills/builtin/create-skill.md`
  - Defines the new built-in prompt skill.
- Modify `crates/neo-agent-core/src/tools/skills_manager.rs`
  - Adds narrow tests that built-ins include `create-skill` and skill instructions mention verification.
- Modify docs:
  - `docs/en/customization/skills.md`
  - `docs/zh/customization/skills.md`
  - `docs/en/guides/interaction.md`
  - `docs/zh/guides/interaction.md`
  - `docs/en/reference/slash-commands.md`
  - `docs/zh/reference/slash-commands.md`

## Task 1: Add Failing Generic Preflight Tests

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Replace init-specific cancellation assertion in the existing test**

Find `init_preflight_dialog_cancel_starts_no_workflow` and replace the last assertion:

```rust
assert_eq!(controller.pending_init_instruction, None);
```

with:

```rust
assert!(controller.pending_interactive_workflow.is_none());
```

This should fail before implementation because `pending_interactive_workflow` does not exist.

- [ ] **Step 2: Rename the init auto preflight test to assert generic behavior**

Rename:

```rust
async fn slash_init_in_auto_opens_preflight_without_starting_turn()
```

to:

```rust
async fn slash_init_in_auto_opens_generic_preflight_without_starting_turn()
```

Keep the existing body. The test should continue to assert that no turn starts and a choice picker opens.

- [ ] **Step 3: Add failing test for no-argument `self-evo` in Auto**

Append this test near the `/init` preflight tests:

```rust
#[tokio::test]
async fn slash_self_evo_without_args_in_auto_opens_required_preflight() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller
        .chrome()
        .focused_overlay()
        .expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}
```

- [ ] **Step 4: Add failing test for selecting the `self-evo` recommended choice**

Append:

```rust
#[tokio::test]
async fn self_evo_preflight_switch_to_ask_starts_skill_workflow() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:self-evo");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("self-evo"), "{prompt}");
    assert!(prompt.contains("Ask me which session scope to distill"), "{prompt}");
}
```

- [ ] **Step 5: Add failing test that `self-evo` with args skips preflight in Auto**

Append:

```rust
#[tokio::test]
async fn slash_self_evo_with_scope_in_auto_skips_preflight() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:self-evo 7");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("self-evo scope starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("7"), "{prompt}");
}
```

- [ ] **Step 6: Add failing tests for `create-skill` preflight**

Append:

```rust
#[tokio::test]
async fn slash_create_skill_without_instruction_in_auto_opens_required_preflight() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill preflight opens");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_some());
}

#[tokio::test]
async fn slash_create_skill_with_instruction_in_auto_skips_preflight() {
    let seen_prompt = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen_prompt_clone = std::sync::Arc::clone(&seen_prompt);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let seen_prompt = std::sync::Arc::clone(&seen_prompt_clone);
            async move {
                *seen_prompt.lock().expect("prompt lock") = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:create-skill make a rust panic review skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("create-skill instruction starts");
    controller
        .wait_for_active_turn()
        .await
        .expect("turn completes");

    assert!(controller.chrome().focused_overlay().is_none());
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("make a rust panic review skill"), "{prompt}");
}
```

- [ ] **Step 7: Add failing test for multiple required preflight skills**

Append:

```rust
#[tokio::test]
async fn multiple_required_preflight_skills_return_status_without_turn() {
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |_request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            async move {
                turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );
    controller.set_permission_mode(PermissionMode::Auto);

    controller.type_text("/skill:self-evo /skill:create-skill");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("slash handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(controller.chrome().focused_overlay().is_none());
    assert!(transcript_has_status(
        &controller,
        "Run one interactive skill workflow at a time"
    ));
}
```

- [ ] **Step 8: Run one failing test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_without_args_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: FAIL because `create-skill` is not built in yet or because generic preflight state does not exist.

## Task 2: Add Generic Interactive Preflight Module

**Files:**
- Create: `crates/neo-agent/src/modes/interactive/interactive_preflight.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/init_command.rs`

- [ ] **Step 1: Create `interactive_preflight.rs`**

Add:

```rust
use neo_agent_core::PermissionMode;
use neo_tui::dialogs::ChoiceItem;

use super::InlineSkillDirectives;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkflowId {
    Init,
    Skill(String),
}

impl WorkflowId {
    pub(super) fn key(&self) -> String {
        match self {
            Self::Init => "init".to_owned(),
            Self::Skill(name) => format!("skill:{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PendingInteractiveWorkflow {
    Init {
        instruction: String,
        auto_mode_best_effort: bool,
    },
    Skill {
        directives: InlineSkillDirectives,
        generated_prompt: Option<String>,
        auto_mode_best_effort: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AutoModePolicy {
    None,
    OptionalClarification {
        best_effort_note: String,
    },
    RequiredClarification {
        missing_input: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreflightAction {
    SwitchPermissionMode(PermissionMode),
    ContinueAutoBestEffort,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreflightChoice {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) description: String,
    pub(super) action: PreflightAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InteractivePreflightSpec {
    pub(super) workflow_id: WorkflowId,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) recommended: PreflightChoice,
    pub(super) alternate: Option<PreflightChoice>,
    pub(super) cancel: PreflightChoice,
    pub(super) auto_mode_policy: AutoModePolicy,
}

impl InteractivePreflightSpec {
    pub(super) fn initial_id(&self) -> String {
        self.recommended.id.clone()
    }

    pub(super) fn choices(&self) -> Vec<PreflightChoice> {
        let mut choices = vec![self.recommended.clone()];
        if let Some(alternate) = &self.alternate {
            choices.push(alternate.clone());
        }
        choices.push(self.cancel.clone());
        choices
    }

    pub(super) fn choice_items(&self) -> Vec<ChoiceItem> {
        self.choices()
            .into_iter()
            .map(|choice| ChoiceItem::new(choice.id, choice.label).with_description(choice.description))
            .collect()
    }

    pub(super) fn action_for_choice(&self, id: &str) -> Option<PreflightAction> {
        self.choices()
            .into_iter()
            .find_map(|choice| (choice.id == id).then_some(choice.action))
    }
}

pub(super) fn init_preflight() -> InteractivePreflightSpec {
    optional_preflight(
        WorkflowId::Init,
        "Switch to Ask mode?",
        "Generating a strong AGENTS.md usually requires asking about reference locations, project preferences, and durable workflow rules.",
        "Switch to Ask and start",
        "Ask mode lets the workflow clarify missing project guidance before writing.",
        "Stay Auto and generate best effort",
        "Start /init without user questions. The agent will proceed with explicit best-effort assumptions.",
        "Do not start /init.",
    )
}

pub(super) fn preflight_for_skill_directives(
    directives: &InlineSkillDirectives,
) -> Result<Option<(InteractivePreflightSpec, Option<String>)>, String> {
    let required = directives
        .invocations
        .iter()
        .filter_map(|invocation| required_skill_preflight(&invocation.name, &invocation.args))
        .collect::<Vec<_>>();
    if required.len() > 1 {
        return Err("Run one interactive skill workflow at a time".to_owned());
    }
    Ok(required.into_iter().next())
}

pub(super) fn auto_best_effort_note() -> &'static str {
    "Auto permission mode remained active. User questions are unavailable during this workflow. Proceed with explicit best-effort assumptions and report any assumption that materially affects the result."
}

fn required_skill_preflight(
    name: &str,
    args: &str,
) -> Option<(InteractivePreflightSpec, Option<String>)> {
    match (name, args.trim().is_empty()) {
        ("self-evo", true) => Some((
            required_preflight(
                WorkflowId::Skill("self-evo".to_owned()),
                "Choose self-evo scope?",
                "self-evo writes reusable skills. It needs a scope before it can safely distill recent work.",
                "Switch to Ask and choose scope",
                "Ask mode lets self-evo ask whether to use the current session, recent sessions, or a specific session/topic.",
                "Do not start self-evo.",
                "scope is required before distillation",
            ),
            Some("Ask me which session scope to distill before creating any skill.".to_owned()),
        )),
        ("create-skill", true) => Some((
            required_preflight(
                WorkflowId::Skill("create-skill".to_owned()),
                "Describe the skill to create?",
                "create-skill writes a persistent skill. It needs your requirement before it can create one safely.",
                "Switch to Ask and describe it",
                "Ask mode lets create-skill ask what capability, inputs, outputs, and verification the skill needs.",
                "Do not start create-skill.",
                "skill requirement is required before authoring",
            ),
            Some("Ask me what skill to create before drafting or calling CreateSkill.".to_owned()),
        )),
        _ => None,
    }
}

fn optional_preflight(
    workflow_id: WorkflowId,
    title: &str,
    body: &str,
    recommended_label: &str,
    recommended_description: &str,
    alternate_label: &str,
    alternate_description: &str,
    cancel_description: &str,
) -> InteractivePreflightSpec {
    let key = workflow_id.key();
    InteractivePreflightSpec {
        workflow_id,
        title: title.to_owned(),
        body: body.to_owned(),
        recommended: PreflightChoice {
            id: format!("preflight:{key}:switch-ask"),
            label: recommended_label.to_owned(),
            description: recommended_description.to_owned(),
            action: PreflightAction::SwitchPermissionMode(PermissionMode::Ask),
        },
        alternate: Some(PreflightChoice {
            id: format!("preflight:{key}:continue-auto"),
            label: alternate_label.to_owned(),
            description: alternate_description.to_owned(),
            action: PreflightAction::ContinueAutoBestEffort,
        }),
        cancel: PreflightChoice {
            id: format!("preflight:{key}:cancel"),
            label: "Cancel".to_owned(),
            description: cancel_description.to_owned(),
            action: PreflightAction::Cancel,
        },
        auto_mode_policy: AutoModePolicy::OptionalClarification {
            best_effort_note: auto_best_effort_note().to_owned(),
        },
    }
}

fn required_preflight(
    workflow_id: WorkflowId,
    title: &str,
    body: &str,
    recommended_label: &str,
    recommended_description: &str,
    cancel_description: &str,
    missing_input: &str,
) -> InteractivePreflightSpec {
    let key = workflow_id.key();
    InteractivePreflightSpec {
        workflow_id,
        title: title.to_owned(),
        body: body.to_owned(),
        recommended: PreflightChoice {
            id: format!("preflight:{key}:switch-ask"),
            label: recommended_label.to_owned(),
            description: recommended_description.to_owned(),
            action: PreflightAction::SwitchPermissionMode(PermissionMode::Ask),
        },
        alternate: None,
        cancel: PreflightChoice {
            id: format!("preflight:{key}:cancel"),
            label: "Cancel".to_owned(),
            description: cancel_description.to_owned(),
            action: PreflightAction::Cancel,
        },
        auto_mode_policy: AutoModePolicy::RequiredClarification {
            missing_input: missing_input.to_owned(),
        },
    }
}
```

- [ ] **Step 2: Register the module and state**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add after `mod init_command;`:

```rust
mod interactive_preflight;
use interactive_preflight::{InteractivePreflightSpec, PendingInteractiveWorkflow, PreflightAction};
```

Find the `InteractiveController` field:

```rust
pending_init_instruction: Option<String>,
```

Replace it with:

```rust
pending_interactive_workflow: Option<PendingInteractiveWorkflow>,
pending_preflight: Option<InteractivePreflightSpec>,
```

In the constructor field initialization, replace:

```rust
pending_init_instruction: None,
```

with:

```rust
pending_interactive_workflow: None,
pending_preflight: None,
```

Use the existing `transcript_has_status` test helper for status assertions; no
new status inspection helper is needed.

- [ ] **Step 3: Delete init-specific preflight from `init_command.rs`**

Remove these items from `crates/neo-agent/src/modes/interactive/init_command.rs`:

```rust
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InitPreflight {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) recommended_id: String,
    pub(super) alternate_id: String,
    pub(super) cancel_id: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreflightDecision {
    SwitchPermissionMode(PermissionMode),
    Continue,
    Cancel,
}

#[allow(dead_code)]
pub(super) fn init_preflight() -> InitPreflight {
    InitPreflight {
        title: "Switch to Ask mode?".to_owned(),
        body: "Generating a strong AGENTS.md usually requires asking about reference locations, project preferences, and durable workflow rules.".to_owned(),
        recommended_id: "preflight:init:switch-ask".to_owned(),
        alternate_id: "preflight:init:continue-auto".to_owned(),
        cancel_id: "preflight:init:cancel".to_owned(),
    }
}

#[allow(dead_code)]
pub(super) fn preflight_decision(id: &str) -> Option<PreflightDecision> {
    match id {
        "preflight:init:switch-ask" => {
            Some(PreflightDecision::SwitchPermissionMode(PermissionMode::Ask))
        }
        "preflight:init:continue-auto" => Some(PreflightDecision::Continue),
        "preflight:init:cancel" => Some(PreflightDecision::Cancel),
        _ => None,
    }
}
```

Then remove the now-unused import:

```rust
use neo_agent_core::PermissionMode;
```

- [ ] **Step 4: Run the expected failing test again**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_without_args_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: FAIL because the module exists but slash dispatch is not wired yet.

## Task 3: Migrate `/init` to Generic Preflight

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Replace `open_init_preflight` with `open_interactive_preflight`**

In `slash_commands.rs`, replace the whole `open_init_preflight` function with:

```rust
fn open_interactive_preflight(
    &mut self,
    spec: InteractivePreflightSpec,
    pending: PendingInteractiveWorkflow,
) {
    self.pending_preflight = Some(spec.clone());
    self.pending_interactive_workflow = Some(pending);
    let theme = self.tui.chrome().theme();
    self.tui
        .chrome_mut()
        .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
            title: spec.title,
            items: spec.choice_items(),
            initial_id: Some(spec.initial_id()),
            theme,
            page_size: 3,
            current_id: None,
        });
}
```

At the top of `slash_commands.rs`, add:

```rust
use super::interactive_preflight::{self, PendingInteractiveWorkflow};
```

- [ ] **Step 2: Gate `/init` through generic preflight**

In `handle_slash_command`, replace:

```rust
if self.permission_mode == super::PermissionMode::Auto {
    self.open_init_preflight(&instruction);
    return true;
}
```

with:

```rust
if self.permission_mode == super::PermissionMode::Auto {
    self.open_interactive_preflight(
        interactive_preflight::init_preflight(),
        PendingInteractiveWorkflow::Init {
            instruction,
            auto_mode_best_effort: false,
        },
    );
    return true;
}
```

Keep the non-auto `run_init_workflow(&instruction, false)` path unchanged.

- [ ] **Step 3: Resolve generic preflight choices**

In `dialog_results.rs`, replace `handle_preflight_choice_item` with:

```rust
pub(super) async fn handle_preflight_choice_item(&mut self, id: &str) -> bool {
    let Some(spec) = self.pending_preflight.clone() else {
        return false;
    };
    let Some(action) = spec.action_for_choice(id) else {
        return false;
    };
    let pending = self.pending_interactive_workflow.take();
    self.pending_preflight = None;

    match action {
        PreflightAction::SwitchPermissionMode(mode) => {
            self.set_permission_mode(mode);
            if let Some(workflow) = pending {
                self.start_pending_interactive_workflow(workflow, false).await;
            }
        }
        PreflightAction::ContinueAutoBestEffort => {
            if let Some(workflow) = pending {
                self.start_pending_interactive_workflow(workflow, true).await;
            }
        }
        PreflightAction::Cancel => {
            self.push_status("Interactive workflow cancelled");
        }
    }
    true
}
```

At the top of `dialog_results.rs`, import:

```rust
use super::interactive_preflight::{PendingInteractiveWorkflow, PreflightAction};
```

- [ ] **Step 4: Add pending workflow starter**

In `slash_commands.rs` or a small new impl block in `dialog_results.rs`, add:

```rust
pub(super) async fn start_pending_interactive_workflow(
    &mut self,
    workflow: PendingInteractiveWorkflow,
    auto_mode_best_effort: bool,
) {
    match workflow {
        PendingInteractiveWorkflow::Init { instruction, .. } => {
            if let Err(error) = self.run_init_workflow(&instruction, auto_mode_best_effort).await {
                self.push_status(format!("Failed to start /init: {error}"));
            }
        }
        PendingInteractiveWorkflow::Skill {
            directives,
            generated_prompt,
            ..
        } => {
            if let Err(error) = self.start_skill_workflow_from_directives(
                directives,
                generated_prompt,
                auto_mode_best_effort,
            ).await {
                self.push_status(format!("Failed to start skill workflow: {error}"));
            }
        }
    }
}
```

The `start_skill_workflow_from_directives` helper is implemented in Task 4. To make this compile during Task 3, add this temporary body now:

```rust
async fn start_skill_workflow_from_directives(
    &mut self,
    _directives: InlineSkillDirectives,
    _generated_prompt: Option<String>,
    _auto_mode_best_effort: bool,
) -> Result<()> {
    anyhow::bail!("skill workflow preflight is not wired yet")
}
```

Task 4 replaces this temporary body. Do not leave the temporary error in the final implementation.

- [ ] **Step 5: Clear generic pending state when choice picker is cancelled**

In `handle_choice_picker_result`, replace:

```rust
neo_tui::dialogs::ChoiceResult::Cancelled => {
    self.pending_init_instruction = None;
}
```

with:

```rust
neo_tui::dialogs::ChoiceResult::Cancelled => {
    self.pending_interactive_workflow = None;
    self.pending_preflight = None;
}
```

- [ ] **Step 6: Run `/init` generic preflight test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_init_in_auto_opens_generic_preflight_without_starting_turn --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 7: Run `/init` continue-auto test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::init_preflight_continue_keeps_auto_and_starts_best_effort --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 8: Gated commit**

Only if the user explicitly authorizes this exact git mutation, run:

```bash
git add crates/neo-agent/src/modes/interactive/interactive_preflight.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/init_command.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "refactor: generalize interactive preflight"
```

Expected: commit succeeds. If authorization is not present, skip this step and continue.

## Task 4: Wire Skill Workflow Preflight and Skill-Only Turns

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Route skill directives through preflight in slash dispatch**

Change `handle_skill_slash_command` signature from:

```rust
fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives)
```

to:

```rust
fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives) -> bool
```

Replace the body with:

```rust
fn handle_skill_slash_command(&mut self, directives: InlineSkillDirectives) -> bool {
    if directives
        .invocations
        .iter()
        .any(|invocation| invocation.name.is_empty())
    {
        self.push_status("Usage: /skill:<name> [args]");
        return true;
    }
    if self.permission_mode == super::PermissionMode::Auto {
        match interactive_preflight::preflight_for_skill_directives(&directives) {
            Ok(Some((spec, generated_prompt))) => {
                self.open_interactive_preflight(
                    spec,
                    PendingInteractiveWorkflow::Skill {
                        directives,
                        generated_prompt,
                        auto_mode_best_effort: false,
                    },
                );
                return true;
            }
            Ok(None) => {}
            Err(message) => {
                self.clear_submitted_prompt();
                self.push_status(message);
                return true;
            }
        }
    }
    match self.activate_skill_directives(directives) {
        Ok(_) => self.clear_submitted_prompt(),
        Err(err) => self.push_status(format!("Skill error: {err}")),
    }
    true
}
```

Then update `handle_model_or_skill_slash_command`:

```rust
if let Some(directives) = parse_inline_skill_directives(prompt) {
    return self.handle_skill_slash_command(directives);
}
```

- [ ] **Step 2: Route skill directives through preflight in prompt submission path**

In `mod.rs`, inside the existing block:

```rust
if let Some(directives) = parse_inline_skill_directives(&prompt) {
```

insert after the empty-name guard:

```rust
if self.permission_mode == PermissionMode::Auto {
    match interactive_preflight::preflight_for_skill_directives(&directives) {
        Ok(Some((spec, generated_prompt))) => {
            self.clear_submitted_prompt();
            self.open_interactive_preflight(
                spec,
                PendingInteractiveWorkflow::Skill {
                    directives,
                    generated_prompt,
                    auto_mode_best_effort: false,
                },
            );
            return Ok(());
        }
        Ok(None) => {}
        Err(message) => {
            self.clear_submitted_prompt();
            self.push_status(message);
            return Ok(());
        }
    }
}
```

This catches `/skill:self-evo` before the slash-command fallback because inline skill parsing currently runs first.

- [ ] **Step 3: Implement `start_skill_workflow_from_directives`**

Replace the temporary helper from Task 3 with:

```rust
async fn start_skill_workflow_from_directives(
    &mut self,
    directives: InlineSkillDirectives,
    generated_prompt: Option<String>,
    auto_mode_best_effort: bool,
) -> Result<()> {
    let (stripped_prompt, display_body) = self.activate_skill_directives(directives)?;
    let prompt = generated_prompt
        .or_else(|| (!stripped_prompt.trim().is_empty()).then_some(stripped_prompt))
        .unwrap_or_else(|| "Run the activated skill workflow.".to_owned());
    let prompt = if auto_mode_best_effort {
        format!(
            "{}\n\n{}",
            interactive_preflight::auto_best_effort_note(),
            prompt
        )
    } else {
        prompt
    };
    self.pending_skill_user_message_to_suppress = Some(display_body);
    self.start_generated_injection_turn_from_text(prompt, "skill", "/skill workflow")?;
    self.wait_for_active_turn().await?;
    self.start_pending_background_question_followups().await
}
```

This uses injection origin so a generated no-argument skill prompt does not pollute prompt history. It still consumes `pending_skill_context` because `start_turn_with_prompt_origin` takes it from controller state.

- [ ] **Step 4: Preserve direct skill invocations with instruction**

In the normal inline skill block in `mod.rs`, keep the existing behavior for non-preflight skills. The code after the new preflight guard should remain:

```rust
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
self.start_turn_from_submitted_prompt(prompt, false)?;
self.drain_active_turn().await?;
return self.start_pending_background_question_followups().await;
```

- [ ] **Step 5: Run no-arg `self-evo` Auto preflight test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_without_args_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 6: Run `self-evo` recommended-choice test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::self_evo_preflight_switch_to_ask_starts_skill_workflow --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 7: Run `self-evo` scoped Auto test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_with_scope_in_auto_skips_preflight --exact --nocapture --include-ignored
```

Expected: PASS. If it fails with `skill not found` for `self-evo`, inspect test setup and ensure built-ins are loaded for `new_for_test`.

- [ ] **Step 8: Run multiple-required-skills status test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::multiple_required_preflight_skills_return_status_without_turn --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 9: Gated commit**

Only if the user explicitly authorizes this exact git mutation, run:

```bash
git add crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat: preflight interactive skill workflows"
```

Expected: commit succeeds. If authorization is not present, skip this step and continue.

## Task 5: Add `create-skill` Built-In and Improve `self-evo`

**Files:**
- Create: `crates/neo-agent-core/src/skills/builtin/create-skill.md`
- Modify: `crates/neo-agent-core/src/skills/builtin/mod.rs`
- Modify: `crates/neo-agent-core/src/skills/builtin/self-evo.md`
- Modify: `crates/neo-agent-core/src/tools/skills_manager.rs`

- [ ] **Step 1: Add failing built-in skill tests**

In `crates/neo-agent-core/src/tools/skills_manager.rs`, append inside the existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn builtin_skills_include_create_skill() {
    let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
    let names = skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"create-skill"), "built-ins: {names:?}");
}

#[test]
fn self_evo_builtin_requires_scope_and_verify_section() {
    let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
    let skill = skills
        .iter()
        .find(|skill| skill.name == "self-evo")
        .expect("self-evo built-in");
    assert!(
        skill.body.contains("No-argument invocation is not a scope"),
        "{}",
        skill.body
    );
    assert!(
        skill.body.contains("## Verify"),
        "{}",
        skill.body
    );
}

#[test]
fn create_skill_builtin_requires_verify_and_create_skill_tool() {
    let skills = crate::skills::builtin::builtin_skills().expect("built-ins load");
    let skill = skills
        .iter()
        .find(|skill| skill.name == "create-skill")
        .expect("create-skill built-in");
    assert!(
        skill.body.contains("## Verify"),
        "{}",
        skill.body
    );
    assert!(
        skill.body.contains("CreateSkill"),
        "{}",
        skill.body
    );
    assert!(
        skill.manifest.disable_model_invocation,
        "create-skill must require explicit user invocation"
    );
}
```

- [ ] **Step 2: Run one failing built-in test**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::builtin_skills_include_create_skill --exact --nocapture
```

Expected: FAIL because `create-skill` is not included.

- [ ] **Step 3: Add the `create-skill` built-in Markdown**

Create `crates/neo-agent-core/src/skills/builtin/create-skill.md` with:

```markdown
---
name: create-skill
description: Create a Neo skill from the user's requirements, including verification guidance.
type: prompt
disableModelInvocation: true
---

You are a Neo skill author. Create one focused Neo skill from the user's requirement.

No-argument invocation is not a requirement. If the user invoked `/skill:create-skill` without describing the desired capability, call `AskUserQuestion` before drafting. Ask what the skill should help with, what inputs it should accept, and how success should be verified.

## Steps

1. Restate the requested skill capability in one sentence.
2. Decide whether this is one focused workflow. If the request combines unrelated workflows, ask the user to split it before creating a skill.
3. Choose a portable skill name:
   - lowercase ASCII letters and digits;
   - `-`, `_`, and `.` are allowed;
   - no slashes, spaces, trailing dots, or Windows device names.
4. Draft a concise description that says when to use the skill.
5. Draft the skill body in current Neo format. Do not include YAML frontmatter in the `body` argument because `CreateSkill` generates it.
6. Include a `## Verify` section in the skill body with concrete checks the future agent can run or inspect.
7. Call `CreateSkill` with `name`, `description`, `skill_type: "prompt"`, and `body`.
8. Call `ListSkills` and verify the created skill name is visible in the active skill store.
9. Report the created path, whether a backup was made, and the verification result.

## Rules

- Prefer one small skill over one broad skill.
- Do not create vague skills.
- Do not duplicate guidance that belongs in `AGENTS.md`.
- Do not use obsolete skill formats or compatibility aliases.
- Do not write skill files directly; use `CreateSkill`.
- If `CreateSkill` reports a reload failure, tell the user the file was written but the active session cannot use it yet.
```

- [ ] **Step 4: Include the new built-in**

In `crates/neo-agent-core/src/skills/builtin/mod.rs`, add:

```rust
const CREATE_SKILL: &str = include_str!("create-skill.md");
```

Replace:

```rust
const BUILTIN_SOURCES: &[&str] = &[SUB_SKILL, SELF_EVO, MCP_CONFIG];
```

with:

```rust
const BUILTIN_SOURCES: &[&str] = &[SUB_SKILL, SELF_EVO, MCP_CONFIG, CREATE_SKILL];
```

- [ ] **Step 5: Replace `self-evo.md` instructions**

Replace `crates/neo-agent-core/src/skills/builtin/self-evo.md` with:

```markdown
---
name: self-evo
description: Summarize the current session or a concrete recent scope into reusable Neo skills saved under ~/.neo/skills/.
type: prompt
disableModelInvocation: true
---

You are a skill author. Turn recent work into reusable Neo skills.

Usage examples from the user:
- `/skill:self-evo current` — summarize the current session.
- `/skill:self-evo 7` — summarize all sessions from the last 7 days.
- `/skill:self-evo session_abc123` — summarize a specific session by id.
- `/skill:self-evo 019c6e27-e55b-73d1-87d8-4e01f1f75043` — summarize a specific session by UUID.
- `/skill:self-evo topic:prompt-cache` — summarize work about a concrete topic.

No-argument invocation is not a scope. If the user invoked `/skill:self-evo` without an argument, call `AskUserQuestion` before summarizing. Ask whether to distill the current session, recent sessions by day count, or a specific session id or topic. Do not proceed until the scope is concrete.

## Steps

1. Determine the concrete scope:
   - `current` means the current session.
   - A number means recent sessions from the last N days.
   - A value starting with `session_` or a UUID means that specific session.
   - A value starting with `topic:` means sessions or memories about that topic.
2. Summarize only the selected scope.
3. Identify reusable patterns, decision rules, recovery workflows, or repeated procedures.
4. Skip trivial facts, one-off context, and guidance that belongs in `AGENTS.md`.
5. For each distinct pattern, draft one focused skill with:
   - `name`: short, lowercase, portable, and unique.
   - `description`: one sentence explaining when to use it.
   - `type`: `prompt` unless the user explicitly requested another supported type.
   - `body`: Markdown without YAML frontmatter.
6. Include a `## Verify` section in every generated skill body. The section must explain how a future agent can check that the skill was applied correctly.
7. Call `CreateSkill` to save each skill under `~/.neo/skills/<name>/SKILL.md`.
8. Call `ListSkills` and verify every created skill is visible in the active skill store.
9. If an existing skill was backed up and overwritten, report the overwritten skill and backup path.

## Rules

- Do not create vague skills.
- Do not create a skill when the selected scope contains no concrete repeatable workflow.
- Do not include YAML frontmatter in the `CreateSkill.body` argument.
- Do not write skill files directly; use `CreateSkill`.
- Keep each skill concise but complete enough for future reuse.
- If skill store reload fails, tell the user the file was written but the active session cannot use it yet.

## Verify

A successful self-evo run creates only focused skills, each generated skill body includes its own `## Verify` section, and `ListSkills` shows the created names without restarting Neo.
```

- [ ] **Step 6: Run built-in tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::builtin_skills_include_create_skill --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::self_evo_builtin_requires_scope_and_verify_section --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_builtin_requires_verify_and_create_skill_tool --exact --nocapture
```

Expected: PASS.

- [ ] **Step 7: Re-run skill workflow tests that depended on the built-in**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_create_skill_without_instruction_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_create_skill_with_instruction_in_auto_skips_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 8: Gated commit**

Only if the user explicitly authorizes this exact git mutation, run:

```bash
git add crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/src/skills/builtin/self-evo.md crates/neo-agent-core/src/tools/skills_manager.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat: add create-skill workflow"
```

Expected: commit succeeds. If authorization is not present, skip this step and continue.

## Task 6: Update User Documentation

**Files:**
- Modify: `docs/en/customization/skills.md`
- Modify: `docs/zh/customization/skills.md`
- Modify: `docs/en/guides/interaction.md`
- Modify: `docs/zh/guides/interaction.md`
- Modify: `docs/en/reference/slash-commands.md`
- Modify: `docs/zh/reference/slash-commands.md`

- [ ] **Step 1: Update English skill guide built-in table**

In `docs/en/customization/skills.md`, add this row to the built-in skill table:

```markdown
| `create-skill` | prompt | Create a Neo skill from the user's requirements, including verification guidance |
```

Replace the `self-evo` row description with:

```markdown
| `self-evo` | prompt | Summarize a concrete current, recent, session, or topic scope into reusable skills |
```

- [ ] **Step 2: Update English skill guide behavior text**

After the built-in skill table in `docs/en/customization/skills.md`, add:

```markdown
`/skill:self-evo` without arguments asks for a distillation scope before creating skills. In Auto permission mode, Neo opens an interactive preflight before the model turn so the workflow does not block unattended execution later.

`/skill:create-skill` creates one focused skill through the `CreateSkill` tool. If no requirement is provided, it asks for the desired capability before drafting. Created skills include verification guidance and are reloaded into the active skill store when `CreateSkill` succeeds.
```

- [ ] **Step 3: Update Chinese skill guide table and behavior text**

In `docs/zh/customization/skills.md`, add this row:

```markdown
| `create-skill` | prompt | 按用户需求创建 Neo skill，并包含验证说明 |
```

Replace the `self-evo` row description with:

```markdown
| `self-evo` | prompt | 把明确的当前、近期、会话或主题范围总结成可复用技能 |
```

After the table, add:

```markdown
`/skill:self-evo` 不带参数时会先询问蒸馏范围，再创建技能。在 Auto 权限模式下，Neo 会在模型回合开始前打开交互预检，避免无人值守运行中途才停下来等待用户回答。

`/skill:create-skill` 通过 `CreateSkill` 工具创建一个聚焦的 skill。如果没有提供需求，它会先询问要创建的能力再起草。创建出的 skill 会包含验证说明；`CreateSkill` 成功后会重新加载当前会话的 skill store。
```

- [ ] **Step 4: Update English interaction guide Auto mode language**

In `docs/en/guides/interaction.md`, find the Auto permission mode description and make sure it says:

```markdown
Auto mode is intended for unattended execution: tool actions are auto-approved and `AskUserQuestion` is unavailable during the running turn. For workflows that may need clarification, Neo can show an interactive preflight before the turn starts. Optional workflows may continue with best-effort assumptions; workflows missing required input ask you to switch to Ask mode or cancel before any work starts.
```

- [ ] **Step 5: Update Chinese interaction guide Auto mode language**

In `docs/zh/guides/interaction.md`, add the matching text:

```markdown
Auto 模式面向无人值守执行：工具动作自动批准，运行中的模型回合不能使用 `AskUserQuestion` 等待用户回答。对于可能需要澄清的工作流，Neo 可以在回合开始前显示交互预检。可选澄清的工作流可以按 best-effort 假设继续；缺少必需输入的工作流会要求你先切到 Ask 或取消，避免跑到一半才停住。
```

- [ ] **Step 6: Update slash-command references**

In `docs/en/reference/slash-commands.md`, add or update the preflight sentence:

```markdown
Interactive workflows such as `/init`, `/skill:self-evo`, and `/skill:create-skill` may open a local preflight in Auto mode before starting. Neo does this mechanically from the parsed slash command; the model does not decide to switch permission modes.
```

In `docs/zh/reference/slash-commands.md`, add:

```markdown
`/init`、`/skill:self-evo`、`/skill:create-skill` 等交互型工作流在 Auto 模式下可能会在开始前打开本地预检。Neo 会根据已解析的 slash command 机械触发该预检；模型不能自行决定切换权限模式。
```

- [ ] **Step 7: Verify docs whitespace**

Run:

```bash
git diff --check -- docs/en/customization/skills.md docs/zh/customization/skills.md docs/en/guides/interaction.md docs/zh/guides/interaction.md docs/en/reference/slash-commands.md docs/zh/reference/slash-commands.md
```

Expected: no output.

- [ ] **Step 8: Gated commit**

Only if the user explicitly authorizes this exact git mutation, run:

```bash
git add docs/en/customization/skills.md docs/zh/customization/skills.md docs/en/guides/interaction.md docs/zh/guides/interaction.md docs/en/reference/slash-commands.md docs/zh/reference/slash-commands.md
git commit -m "docs: describe interactive skill preflight"
```

Expected: commit succeeds. If authorization is not present, skip this step and continue.

## Task 7: Final Focused Verification and Cleanup

**Files:**
- Modify only files touched by earlier tasks if verification exposes a targeted issue.

- [ ] **Step 1: Run exact `/init` regression test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_init_submits_generated_workflow_prompt --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 2: Run exact generic preflight regression test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_init_in_auto_opens_generic_preflight_without_starting_turn --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 3: Run exact required skill preflight tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_without_args_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_create_skill_without_instruction_in_auto_opens_required_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 4: Run exact no-preflight skill tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_self_evo_with_scope_in_auto_skips_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_create_skill_with_instruction_in_auto_skips_preflight --exact --nocapture --include-ignored
```

Expected: PASS.

- [ ] **Step 5: Run exact built-in skill tests**

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::builtin_skills_include_create_skill --exact --nocapture
```

Expected: PASS.

Run:

```bash
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::create_skill_builtin_requires_verify_and_create_skill_tool --exact --nocapture
```

Expected: PASS.

- [ ] **Step 6: Check formatting narrowly**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS. If it fails only on touched Rust files, run `cargo fmt --all`, then rerun the check. Do not reformat unrelated generated files manually.

- [ ] **Step 7: Check whitespace on touched files**

Run:

```bash
git diff --check -- crates/neo-agent/src/modes/interactive/interactive_preflight.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/init_command.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/src/skills/builtin/self-evo.md crates/neo-agent-core/src/tools/skills_manager.rs docs/en/customization/skills.md docs/zh/customization/skills.md docs/en/guides/interaction.md docs/zh/guides/interaction.md docs/en/reference/slash-commands.md docs/zh/reference/slash-commands.md
```

Expected: no output.

- [ ] **Step 8: Remove obsolete init preflight identifiers**

Run:

```bash
rg -n "pending_init_instruction|InitPreflight|preflight_decision|init_preflight\\(" crates/neo-agent/src/modes/interactive
```

Expected: no matches for `pending_init_instruction`, `InitPreflight`, or `preflight_decision`. A match for `interactive_preflight::init_preflight()` is acceptable.

- [ ] **Step 9: Store ICM completion note**

Run:

```bash
icm store -t context-neo -c "Implemented generic interactive preflight for /init and interaction-sensitive skill workflows. /skill:self-evo and /skill:create-skill now preflight required missing input in Auto mode, create-skill is a built-in skill, self-evo requires concrete scope, and docs/tests were updated." -i high -k "interactive-preflight,self-evo,create-skill,auto,ask"
```

Expected: `Stored: ...`

- [ ] **Step 10: Gated final commit**

Only if the user explicitly authorizes this exact git mutation and previous task commits were skipped, run:

```bash
git add crates/neo-agent/src/modes/interactive/interactive_preflight.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/init_command.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/src/skills/builtin/create-skill.md crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/src/skills/builtin/self-evo.md crates/neo-agent-core/src/tools/skills_manager.rs docs/en/customization/skills.md docs/zh/customization/skills.md docs/en/guides/interaction.md docs/zh/guides/interaction.md docs/en/reference/slash-commands.md docs/zh/reference/slash-commands.md
git commit -m "feat: add interactive skill preflight"
```

Expected: commit succeeds. If authorization is not present, skip this step and report uncommitted changes.

## Self-Review

Spec coverage: PASS. The plan covers generic runtime-triggered preflight, `/init` migration, `self-evo` no-argument scope handling, new `create-skill` built-in, skill store reload expectations, docs, focused tests, and deletion of init-specific compatibility paths.

Placeholder scan: PASS. The plan contains concrete file paths, commands, snippets, and expected results. There are no unresolved marker words or empty implementation sections.

Type consistency: PASS. `InteractivePreflightSpec`, `PreflightChoice`, `PreflightAction`, `PendingInteractiveWorkflow`, and `auto_best_effort_note()` are introduced before later tasks reference them. The plan uses `pending_interactive_workflow` and `pending_preflight` consistently after deleting `pending_init_instruction`.
