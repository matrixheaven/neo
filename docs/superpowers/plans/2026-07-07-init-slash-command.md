# Init Slash Command Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a TUI-only `/init [instruction]` slash command that launches a guided AGENTS.md generation workflow with reusable Auto-mode preflight, strict structure guardrails, injection-origin prompt delivery, and focused validation.

**Architecture:** Keep slash dispatch thin by adding a focused `init_command` module for prompt construction, preflight configuration, and AGENTS.md validation. Reuse the existing choice picker for the first reusable preflight implementation. Start `/init` through the normal turn machinery, but mark the initiating message as injection-origin system-reminder content so it is model-visible, hidden from transcript replay, and excluded from prompt history.

**Tech Stack:** Rust 2024, Neo TUI interactive controller, ratatui-backed existing dialogs, `cargo nextest` with narrow filters, Markdown string parsing with small local Rust helpers.

---

## Scope Notes

This plan implements the spec in `docs/superpowers/specs/2026-07-07-init-slash-command-design.md`.

`/init` is TUI-only. Do not add a CLI subcommand.

Do not add a first-version flag parser. Everything after `/init` is raw natural-language instruction text.

Do not hardcode `.references/` as the reference location. The generated workflow prompt must ask the user where reference projects or documents live.

Use Neo's existing injection-origin semantics for generated workflow prompts. This mirrors the useful part of Kimi Code's `<system-reminder>` design: model-visible guidance is tagged with injection origin metadata, and the UI/history layers treat origin metadata as the source of truth.

Git mutation is restricted in this repository. Subagents must not run `git add`, `git commit`, or other git mutations. When a task includes a checkpoint command, treat it as documentation for a main agent after explicit per-command user authorization.

## File Structure

Create:

- `crates/neo-agent/src/modes/interactive/init_command.rs`
  Owns `/init` raw instruction parsing, prompt construction, reusable preflight configuration, and AGENTS.md structure validation.

Modify:

- `crates/neo-agent/src/modes/interactive/mod.rs`
  Adds `mod init_command;`, prompt-origin support on `TurnRequest`, a generated injection-turn helper, and small controller fields for pending preflight continuation.

- `crates/neo-agent/src/modes/interactive/turn.rs`
  Adds an origin-aware turn starter while keeping the existing normal-user wrapper.

- `crates/neo-agent/src/modes/interactive/controller_factory.rs`
  Forwards `TurnRequest.prompt_origin` into the production runtime driver.

- `crates/neo-agent/src/modes/run/mod.rs`
  Preserves prompt origin when appending the initial user-role message to JSONL and runtime context.

- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
  Routes `/init` and `/init [instruction]` to `init_command` helpers.

- `crates/neo-agent/src/modes/interactive/dialog_results.rs`
  Handles generic preflight choice ids produced by `/init`.

- `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
  Adds `/init` to static slash completions.

- `crates/neo-agent/src/modes/interactive/tests.rs`
  Adds focused tests for completions, dispatch, raw instruction propagation, preflight behavior, and validator behavior.

- `crates/neo-agent/Cargo.toml`
  Adds the existing workspace `chrono` dependency to the `neo-agent` crate for cross-platform date labels in the generated workflow prompt.

- `docs/en/reference/slash-commands.md`
  Documents `/init [instruction]`.

- `docs/zh/reference/slash-commands.md`
  Documents `/init [instruction]`.

Do not move existing slash-command code. Keep new logic in `init_command.rs` and make existing files delegate into it.

## Task 1: Add the Init Command Prompt Builder and Validator

**Files:**

- Create: `crates/neo-agent/src/modes/interactive/init_command.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing unit tests for init prompt and validator helpers**

Append these tests near the slash-command tests in `crates/neo-agent/src/modes/interactive/tests.rs`:

```rust
#[test]
fn init_command_prompt_includes_instruction_and_structure_guardrails() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: Some("abc1234"),
        instruction: Some("排除掉 generated 目录"),
        auto_mode_best_effort: false,
    });

    assert!(prompt.contains("Target file: /workspace/neo/AGENTS.md"));
    assert!(prompt.contains("Current date: 2026-07-07"));
    assert!(prompt.contains("Source commit: abc1234"));
    assert!(prompt.contains("User instruction after /init: 排除掉 generated 目录"));
    assert!(prompt.contains("Required top-level sections, in order:"));
    assert!(prompt.contains("1. Reference"));
    assert!(prompt.contains("14. Metadata"));
    assert!(prompt.contains("Before writing AGENTS.md, prepare a concise outline plan"));
    assert!(prompt.contains("Golden style example"));
    assert!(prompt.contains("ask the user where reference projects or reference documents live"));
    assert!(prompt.contains("Do not treat this guide as a Neo product specification"));
}

#[test]
fn init_command_prompt_marks_auto_mode_best_effort() {
    let prompt = init_command::build_init_workflow_prompt(init_command::InitPromptRequest {
        workspace_root: Path::new("/workspace/neo"),
        current_date: "2026-07-07",
        source_commit: None,
        instruction: None,
        auto_mode_best_effort: true,
    });

    assert!(prompt.contains("Source commit: unavailable"));
    assert!(prompt.contains("Auto permission mode remained active"));
    assert!(prompt.contains("proceed with best-effort assumptions"));
}

#[test]
fn agents_guide_validator_accepts_required_structure() {
    let guide = init_command::example_agents_guide_for_tests("abc1234");
    assert_eq!(init_command::validate_agents_guide(&guide), Vec::new());
}

#[test]
fn agents_guide_validator_reports_missing_duplicate_and_reordered_headings() {
    let guide = r#"# Project Agent Guide

## Reference
text

## Workflow
text

## Project Identity
text

## Workflow
text

## Metadata
Created: 2026-07-07
Source commit: abc1234
Best valid until: 2026-10-07
"#;

    let issues = init_command::validate_agents_guide(guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::DuplicateHeading));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::HeadingOrder));
}

#[test]
fn agents_guide_validator_reports_missing_metadata_and_placeholders() {
    let guide = init_command::REQUIRED_AGENTS_HEADINGS
        .iter()
        .map(|heading| format!("## {heading}\nplaceholder content\n"))
        .collect::<Vec<_>>()
        .join("\n");

    let issues = init_command::validate_agents_guide(&guide);
    let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();

    assert!(codes.contains(&init_command::AgentsGuideIssueCode::MissingMetadataField));
    assert!(codes.contains(&init_command::AgentsGuideIssueCode::PlaceholderText));
}
```

- [ ] **Step 2: Run the focused test filter and confirm it fails**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_command_prompt_includes_instruction_and_structure_guardrails
```

Expected: fails to compile because `init_command` does not exist.

- [ ] **Step 3: Register the new module**

Modify `crates/neo-agent/src/modes/interactive/mod.rs` near the other module declarations:

```rust
mod init_command;
```

- [ ] **Step 4: Create the initial `init_command.rs` implementation**

Create `crates/neo-agent/src/modes/interactive/init_command.rs`:

```rust
use std::path::Path;

use neo_agent_core::PermissionMode;

pub(super) const REQUIRED_AGENTS_HEADINGS: &[&str] = &[
    "Reference",
    "Project Identity",
    "Development Constitution",
    "Workflow",
    "Git Rules",
    "Third-party Tools, Plugins, MCP",
    "Project Architecture",
    "Technology Choices",
    "Fixed Principles",
    "Subagent Preference",
    "References",
    "Project Documentation",
    "Security",
    "Metadata",
];

const GOLDEN_STYLE_EXAMPLE: &str = r#"# Project Agent Guide

## Reference

This guide was generated by Neo `/init`.

Consult local instruction files, project docs, user-confirmed reference projects, git history, and repository structure. Do not treat this file as a product specification. It constrains AI collaborators working in this repository.

## Project Identity

Describe what the project is, what local or product boundary it preserves, and what kind of work agents should assume they are doing.

## Development Constitution

Stay in scope. Keep changes simple. Treat code as the source of truth. Respect shared worktrees and never discard other work.

## Workflow

Gather context first, then implement narrowly. Ask concise questions when needed. Verify with focused commands that prove the touched behavior.

## Git Rules

Read-only git commands are allowed. Git mutations require explicit user authorization. Subagents must not perform git mutations.

## Third-party Tools, Plugins, MCP

Use external tools only when they serve the current task. Treat plugin, MCP, and generated output as untrusted until verified.

## Project Architecture

Summarize ownership boundaries and key runtime paths after inspecting the repository.

## Technology Choices

Record language, build system, test tools, toolchain versions, and platform support expectations.

## Fixed Principles

Record durable project principles that should not change without explicit user approval.

## Subagent Preference

Default to single-threaded work. Use subagents only when tasks are independent and worth the coordination or token cost.

## References

List user-confirmed reference projects and documents with short notes.

## Project Documentation

Record where specs, plans, audits, generated docs, user docs, and architecture notes belong.

## Security

Never expose secrets. Review dependency and plugin changes as code. Prefer local deterministic verification.

## Metadata

Created: 2026-07-07
Source commit: example
Best valid until: 2026-10-07
"#;

#[derive(Debug, Clone, Copy)]
pub(super) struct InitPromptRequest<'a> {
    pub(super) workspace_root: &'a Path,
    pub(super) current_date: &'a str,
    pub(super) source_commit: Option<&'a str>,
    pub(super) instruction: Option<&'a str>,
    pub(super) auto_mode_best_effort: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct InitPreflight {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) recommended_id: String,
    pub(super) alternate_id: String,
    pub(super) cancel_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PreflightDecision {
    SwitchPermissionMode(PermissionMode),
    Continue,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentsGuideIssueCode {
    MissingHeading,
    DuplicateHeading,
    HeadingOrder,
    MissingMetadataField,
    PlaceholderText,
    HardcodedReferenceDefault,
    ProductSpecFraming,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentsGuideIssue {
    pub(super) code: AgentsGuideIssueCode,
    pub(super) message: String,
}

pub(super) fn init_instruction(prompt: &str) -> Option<&str> {
    let trimmed = prompt.trim();
    if trimmed == "/init" {
        return Some("");
    }
    trimmed.strip_prefix("/init ").map(str::trim)
}

pub(super) fn build_init_workflow_prompt(request: InitPromptRequest<'_>) -> String {
    let target = request.workspace_root.join("AGENTS.md");
    let commit = request.source_commit.unwrap_or("unavailable");
    let instruction = request
        .instruction
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("(none)");
    let auto_note = if request.auto_mode_best_effort {
        "Auto permission mode remained active. AskUserQuestion may be unavailable; proceed with best-effort assumptions when the user cannot be asked."
    } else {
        "Interactive clarification is allowed when information is missing."
    };

    format!(
        r#"You are running Neo's /init workflow.

Target file: {}
Workspace root: {}
Current date: {}
Source commit: {}
User instruction after /init: {}

Mission:
- Research the current repository before writing.
- Inherit the spirit of existing AGENTS.md, CLAUDE.md, and GEMINI.md files when present.
- Ask the user where reference projects or reference documents live. Do not assume .references/ is the reference location.
- Create or update the root AGENTS.md in place.
- Do not treat this guide as a Neo product specification. It constrains AI collaborators.
- {auto_note}

Required top-level sections, in order:
{}

Before writing AGENTS.md, prepare a concise outline plan. For each required section, list source facts used, user preferences used, and intended content.

After writing AGENTS.md, ensure all required sections are present in order, Metadata contains Created, Source commit, and Best valid until, and no placeholder text remains.

Golden style example. Use this for structure, concision, and operational tone only. Do not copy project-specific facts unless current repository research supports them:

{}
"#,
        target.display(),
        request.workspace_root.display(),
        request.current_date,
        commit,
        instruction,
        section_contract(),
        GOLDEN_STYLE_EXAMPLE
    )
}

pub(super) fn wrap_init_system_reminder(prompt: &str) -> String {
    format!("<system-reminder>\n{}\n</system-reminder>", prompt.trim())
}

pub(super) fn init_preflight() -> InitPreflight {
    InitPreflight {
        title: "Switch to Ask mode?".to_owned(),
        body: "Generating a strong AGENTS.md usually requires asking about reference locations, project preferences, and durable workflow rules.".to_owned(),
        recommended_id: "preflight:init:switch-ask".to_owned(),
        alternate_id: "preflight:init:continue-auto".to_owned(),
        cancel_id: "preflight:init:cancel".to_owned(),
    }
}

pub(super) fn preflight_decision(id: &str) -> Option<PreflightDecision> {
    match id {
        "preflight:init:switch-ask" => Some(PreflightDecision::SwitchPermissionMode(
            PermissionMode::Ask,
        )),
        "preflight:init:continue-auto" => Some(PreflightDecision::Continue),
        "preflight:init:cancel" => Some(PreflightDecision::Cancel),
        _ => None,
    }
}

pub(super) fn validate_agents_guide(markdown: &str) -> Vec<AgentsGuideIssue> {
    let headings = second_level_headings(markdown);
    let mut issues = Vec::new();

    for required in REQUIRED_AGENTS_HEADINGS {
        let count = headings.iter().filter(|heading| heading == required).count();
        if count == 0 {
            issues.push(issue(
                AgentsGuideIssueCode::MissingHeading,
                format!("Missing required heading: {required}"),
            ));
        } else if count > 1 {
            issues.push(issue(
                AgentsGuideIssueCode::DuplicateHeading,
                format!("Duplicate required heading: {required}"),
            ));
        }
    }

    let required_positions = REQUIRED_AGENTS_HEADINGS
        .iter()
        .filter_map(|required| headings.iter().position(|heading| heading == required))
        .collect::<Vec<_>>();
    if !required_positions.windows(2).all(|pair| pair[0] < pair[1]) {
        issues.push(issue(
            AgentsGuideIssueCode::HeadingOrder,
            "Required headings are not in the expected order",
        ));
    }

    for field in ["Created:", "Source commit:", "Best valid until:"] {
        if !markdown.contains(field) {
            issues.push(issue(
                AgentsGuideIssueCode::MissingMetadataField,
                format!("Missing Metadata field: {field}"),
            ));
        }
    }

    for marker in ["TODO", "TBD", "<fill me>", "placeholder content"] {
        if markdown.contains(marker) {
            issues.push(issue(
                AgentsGuideIssueCode::PlaceholderText,
                format!("Placeholder text remains: {marker}"),
            ));
        }
    }

    if markdown.contains("always use .references/") {
        issues.push(issue(
            AgentsGuideIssueCode::HardcodedReferenceDefault,
            ".references/ is framed as the only reference location",
        ));
    }

    if markdown.contains("Neo must implement") || markdown.contains("product requirement") {
        issues.push(issue(
            AgentsGuideIssueCode::ProductSpecFraming,
            "Guide appears to frame agent rules as product requirements",
        ));
    }

    issues
}

pub(super) fn example_agents_guide_for_tests(commit: &str) -> String {
    REQUIRED_AGENTS_HEADINGS
        .iter()
        .map(|heading| {
            if *heading == "Metadata" {
                format!(
                    "## Metadata\nCreated: 2026-07-07\nSource commit: {commit}\nBest valid until: 2026-10-07\n"
                )
            } else {
                format!("## {heading}\nConcrete project guidance.\n")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn section_contract() -> String {
    REQUIRED_AGENTS_HEADINGS
        .iter()
        .enumerate()
        .map(|(index, heading)| format!("{}. {heading}", index + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn second_level_headings(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(str::trim)
        .map(str::to_owned)
        .collect()
}

fn issue(code: AgentsGuideIssueCode, message: impl Into<String>) -> AgentsGuideIssue {
    AgentsGuideIssue {
        code,
        message: message.into(),
    }
}
```

- [ ] **Step 5: Run the focused helper tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_command
```

Expected: PASS for all tests whose names contain `init_command`.

## Task 2: Wire `/init` Dispatch Without Auto Preflight

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent/src/modes/interactive/controller_factory.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify: `crates/neo-agent/Cargo.toml`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`
- Test: `crates/neo-agent/src/modes/run/mod.rs`

- [ ] **Step 1: Add failing tests for ask-mode `/init` submission**

Append these tests near the slash-command tests:

```rust
#[tokio::test]
async fn slash_init_submits_generated_workflow_prompt() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<TurnRequest>::new()));
    let requests_clone = std::sync::Arc::clone(&requests);
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        test_workspace_root(),
        move |request| {
            let requests = std::sync::Arc::clone(&requests_clone);
            async move {
                requests.lock().expect("request lock").push(request);
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("/init handled");

    let requests = requests.lock().expect("request lock");
    let request = requests.first().expect("one init request");
    let prompt = request
        .prompt
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("");
    assert!(prompt.contains("<system-reminder>"));
    assert!(prompt.contains("You are running Neo's /init workflow."));
    assert!(prompt.contains("User instruction after /init: 排除掉 generated 目录"));
    assert!(prompt.contains("Target file:"));
    assert!(prompt.contains("AGENTS.md"));
    assert_eq!(
        request.prompt_origin,
        neo_agent_core::MessageOrigin::injection("init")
    );
}

#[tokio::test]
async fn slash_init_is_not_persisted_to_prompt_history() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompt-history.jsonl");
    let store = crate::prompt::history::PromptHistoryStore::for_dir(PathBuf::from(dir.path()));
    let mut controller = controller_with_history_store(store);

    controller.type_text("/init 排除掉 generated 目录");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::InputSubmit))
        .await
        .expect("/init command handled");

    let persisted = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(persisted.is_empty(), "generated /init prompt must not be persisted: {persisted}");
}
```

Add this focused runtime-origin unit test inside the existing `#[cfg(test)] mod tests` in `crates/neo-agent/src/modes/run/mod.rs`:

```rust
#[tokio::test]
async fn append_user_event_preserves_injection_origin() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("session.jsonl");
    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create jsonl writer");
    let mut event_writer = SessionEventWriter::jsonl(&mut writer);
    let origin = MessageOrigin::injection("init");

    let (message, events) = append_user_event(
        vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
        origin.clone(),
        &mut event_writer,
    )
    .await
    .expect("append user event");

    assert!(message.is_injection());
    assert_eq!(
        events,
        vec![AgentEvent::MessageAppended {
            message: AgentMessage::User {
                content: vec![Content::text("<system-reminder>\ninit\n</system-reminder>")],
                origin,
            },
        }]
    );
}
```

- [ ] **Step 2: Run the focused test and confirm it fails**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init_submits_generated_workflow_prompt
```

Expected: FAIL because `/init` is not handled yet and the captured prompt either stays empty or contains the literal user text.

Also run:

```bash
cargo nextest run -p neo-agent --bin neo append_user_event_preserves_injection_origin
```

Expected: FAIL because `append_user_event` still constructs `AgentMessage::user_content`, which always uses `MessageOrigin::User`.

- [ ] **Step 3: Add `chrono` to `neo-agent`**

In `crates/neo-agent/Cargo.toml`, add this dependency in `[dependencies]`:

```toml
chrono.workspace = true
```

- [ ] **Step 4: Add prompt-origin support from TUI request to runtime JSONL**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add a field to `TurnRequest`:

```rust
pub prompt_origin: neo_agent_core::MessageOrigin,
```

Initialize it in `TurnRequest::new`:

```rust
prompt_origin: neo_agent_core::MessageOrigin::User,
```

In `crates/neo-agent/src/modes/interactive/turn.rs`, keep the existing method as the normal-user wrapper:

```rust
pub(super) fn start_turn_with_prompt(
    &mut self,
    prompt: Vec<Content>,
    model_override: Option<super::SelectedModel>,
) {
    self.start_turn_with_prompt_origin(
        prompt,
        model_override,
        neo_agent_core::MessageOrigin::User,
    );
}
```

Move the current body of `start_turn_with_prompt` into a new origin-aware method:

```rust
pub(super) fn start_turn_with_prompt_origin(
    &mut self,
    prompt: Vec<Content>,
    model_override: Option<super::SelectedModel>,
    prompt_origin: neo_agent_core::MessageOrigin,
) {
    if self.active_turn.is_some() {
        self.push_status("A turn is already running");
        return;
    }

    // Keep the existing channel/cancel-token setup unchanged.
    // After constructing `request`, preserve the origin before applying skill context.
    let mut request = TurnRequest::new(
        prompt,
        self.active_session_id.clone(),
        model_override.or_else(|| self.active_model.clone()),
        if self.current_thinking {
            Some(neo_ai::ReasoningEffort::High)
        } else {
            None
        },
    );
    request.prompt_origin = prompt_origin;

    // Leave the rest of the existing request population and spawn logic unchanged.
}
```

The implementation should copy the existing method body exactly and only add the `prompt_origin` assignment. Do not create a separate execution path.

In `crates/neo-agent/src/modes/run/mod.rs`, import `MessageOrigin` with the existing core imports. Then change the append helpers to preserve origin:

```rust
async fn append_user_event(
    content: Vec<Content>,
    origin: MessageOrigin,
    writer: &mut SessionEventWriter<'_>,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let user_message = AgentMessage::User { content, origin };
    let user_event = AgentEvent::MessageAppended {
        message: user_message.clone(),
    };
    writer.append_event(&user_event).await?;
    writer.flush().await?;
    Ok((user_message, vec![user_event]))
}

async fn append_user_event_jsonl(
    content: Vec<Content>,
    origin: MessageOrigin,
    writer: &mut JsonlSessionWriter,
) -> anyhow::Result<(AgentMessage, Vec<AgentEvent>)> {
    let mut writer = SessionEventWriter::jsonl(writer);
    append_user_event(content, origin, &mut writer).await
}
```

Update existing non-streaming call sites in `run/mod.rs` to pass `MessageOrigin::User`.

Update `prepare_new_streaming_turn` and `prepare_existing_streaming_turn` to accept `prompt_origin: MessageOrigin` and pass it to `append_user_event_jsonl`:

```rust
async fn prepare_new_streaming_turn(
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    // Keep existing setup unchanged.
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), prompt_origin, &mut writer).await?;
    // Keep existing return unchanged.
}

async fn prepare_existing_streaming_turn(
    session_id: &str,
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    skill_context: Option<String>,
) -> anyhow::Result<PreparedStreamingTurn> {
    // Keep existing replay/setup unchanged.
    let (user_message, initial_events) =
        append_user_event_jsonl(prompt.to_vec(), prompt_origin, &mut writer).await?;
    // Keep existing return unchanged.
}
```

Update the public streaming functions to accept origin:

```rust
pub async fn run_prompt_streaming(
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared =
        prepare_new_streaming_turn(prompt, prompt_origin, config, session_id_tx, skill_context)
            .await?;
    // Keep existing runtime setup unchanged.
}

pub async fn run_prompt_in_session_streaming(
    session_id: &str,
    prompt: &[Content],
    prompt_origin: MessageOrigin,
    config: &AppConfig,
    event_tx: mpsc::UnboundedSender<anyhow::Result<AgentEvent>>,
    approval_tx: mpsc::UnboundedSender<PromptApprovalRequest>,
    session_id_tx: Option<mpsc::UnboundedSender<String>>,
    cancel_token: CancellationToken,
    question_tx: Option<mpsc::UnboundedSender<PendingQuestion>>,
    skill_context: Option<String>,
    plan_review_feedback: Option<std::collections::BTreeMap<String, String>>,
    plan_mode: Option<Arc<RwLock<PlanMode>>>,
    goal_mode_authoring: bool,
    steer_input: SteerInputHandle,
    mcp_manager: Option<McpConnectionManager>,
    manual_compact_request: Arc<Mutex<Option<String>>>,
    compaction_only: bool,
) -> anyhow::Result<PromptTurn> {
    let prepared = prepare_existing_streaming_turn(
        session_id,
        prompt,
        prompt_origin,
        config,
        session_id_tx,
        skill_context,
    )
    .await?;
    // Keep existing runtime setup unchanged.
}
```

In `crates/neo-agent/src/modes/interactive/controller_factory.rs`, pass `request.prompt_origin.clone()` into both streaming runtime calls:

```rust
let turn = crate::modes::run::run_prompt_in_session_streaming(
    &session_id,
    &request.prompt,
    request.prompt_origin.clone(),
    &effective_config,
    channels.events,
    channels.approvals,
    Some(channels.session_ids),
    channels.cancel_token,
    Some(channels.questions),
    request.skill_context.clone(),
    Some(request.plan_review_feedback.clone()),
    Some(Arc::clone(&request.plan_mode)),
    request.goal_mode_authoring,
    channels.steer_input,
    request.mcp_manager.clone(),
    Arc::clone(&request.manual_compact_request),
    request.compaction_only,
)
.await?;
```

And for new sessions:

```rust
let turn = crate::modes::run::run_prompt_streaming(
    &request.prompt,
    request.prompt_origin.clone(),
    &effective_config,
    channels.events,
    channels.approvals,
    Some(channels.session_ids),
    channels.cancel_token,
    Some(channels.questions),
    request.skill_context.clone(),
    Some(request.plan_review_feedback.clone()),
    Some(Arc::clone(&request.plan_mode)),
    request.goal_mode_authoring,
    channels.steer_input,
    request.mcp_manager.clone(),
    Arc::clone(&request.manual_compact_request),
    request.compaction_only,
)
.await?;
```

This is the important Kimi-inspired invariant: the `<system-reminder>` wrapper gives the model directive semantics, while `MessageOrigin::Injection { variant: "init" }` is the source of truth for transcript replay, prompt history exclusion, and future filtering. Do not hide generated workflow prompts by string matching on `<system-reminder>`.

- [ ] **Step 5: Add a helper that starts the init turn without prompt history**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add this helper near `start_turn_from_submitted_prompt`:

```rust
fn start_generated_injection_turn_from_text(
    &mut self,
    prompt: String,
    variant: &'static str,
    local_message: impl Into<String>,
) -> Result<()> {
    let PromptSubmission {
        prompt,
        model_override,
    } = PromptSubmission::from_text(
        prompt,
        &self.model_items,
        self.local_config.as_ref(),
        &self.completion_root,
    )?;
    let content = crate::prompt::parts::expand_prompt_markers(
        &prompt,
        &self.paste_store,
        &self.image_attachment_store,
    );
    let display_text = content_to_display_text(&content);
    self.tui
        .transcript_mut()
        .push_user_message(local_message.into());
    self.pending_local_user_message_to_suppress = Some(display_text);
    self.start_turn_with_prompt_origin(
        content,
        model_override,
        neo_agent_core::MessageOrigin::injection(variant),
    );
    Ok(())
}
```

This helper intentionally does not call `submit_prompt_text`, `chrome.submit_prompt`, or `append_prompt_history`. `/init` is a local slash workflow, and its generated workflow prompt should not appear in user prompt history. It also starts the runtime turn with injection origin so replay/transcript layers hide the generated prompt by metadata, the same way existing system reminders are hidden.

In `crates/neo-agent/src/modes/interactive/slash_commands.rs`, add `init_command` to the imports:

```rust
use super::init_command::{self, InitPromptRequest};
```

Add this method inside `impl InteractiveController`:

```rust
pub(super) fn start_init_workflow(
    &mut self,
    instruction: &str,
    auto_mode_best_effort: bool,
) -> Result<()> {
    let workspace_root = self.tui.chrome().workspace_root().to_path_buf();
    let current_date = current_date_label();
    let source_commit = current_git_commit();
    let prompt = init_command::build_init_workflow_prompt(InitPromptRequest {
        workspace_root: workspace_root.as_path(),
        current_date: current_date.as_str(),
        source_commit: source_commit.as_deref(),
        instruction: (!instruction.is_empty()).then_some(instruction),
        auto_mode_best_effort,
    });
    let reminder = init_command::wrap_init_system_reminder(&prompt);
    self.start_generated_injection_turn_from_text(reminder, "init", "/init AGENTS.md workflow")
}

pub(super) async fn run_init_workflow(
    &mut self,
    instruction: &str,
    auto_mode_best_effort: bool,
) -> Result<()> {
    self.start_init_workflow(instruction, auto_mode_best_effort)?;
    self.drain_active_turn().await?;
    self.repair_agents_guide_once_if_needed().await?;
    self.start_pending_background_question_followups().await
}
```

Add this helper near the bottom of `slash_commands.rs`:

```rust
fn current_date_label() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}
```

Use `chrono` because it is already a workspace dependency and avoids shelling out to platform-specific date commands.

Add this helper near the bottom of `slash_commands.rs`:

```rust
fn current_git_commit() -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}
```

Use the existing project convention if there is already a git-status helper for this exact value. Do not add a dependency only for the date.

- [ ] **Step 6: Wire `/init` into slash dispatch**

In `handle_slash_command`, before `handle_simple_slash_command`, add:

```rust
if let Some(instruction) = init_command::init_instruction(prompt) {
    self.clear_submitted_prompt();
    if let Err(error) = self.run_init_workflow(instruction, false).await {
        self.push_status(format!("Failed to start /init: {error}"));
    }
    return true;
}
```

- [ ] **Step 7: Run the ask-mode `/init` test**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init_submits_generated_workflow_prompt
```

Expected: PASS.

- [ ] **Step 8: Run prompt-history and origin-preservation tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init_is_not_persisted_to_prompt_history
cargo nextest run -p neo-agent --bin neo append_user_event_preserves_injection_origin
```

Expected: PASS.

## Task 3: Add Reusable Auto-Mode Preflight

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive/init_command.rs`
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing tests for Auto-mode preflight**

Append these tests near existing permission slash tests:

```rust
#[tokio::test]
async fn slash_init_in_auto_opens_preflight_without_starting_turn() {
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

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("/init opens preflight");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    let overlay = controller.chrome().focused_overlay().expect("preflight overlay");
    assert!(matches!(overlay.kind, OverlayKind::ChoicePicker(_)));
    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
}

#[tokio::test]
async fn init_preflight_switch_to_ask_starts_workflow() {
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

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm recommended option");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Ask);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("User instruction after /init: 参考 docs"));
    assert!(prompt.contains("Interactive clarification is allowed"));
}

#[tokio::test]
async fn init_preflight_continue_keeps_auto_and_starts_best_effort() {
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

    controller.type_text("/init 参考 docs");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("preflight opens");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectDown))
        .await
        .expect("select continue");
    controller
        .handle_input_event(InputEvent::Action(KeybindingAction::SelectConfirm))
        .await
        .expect("confirm continue");

    assert_eq!(controller.chrome().permission_mode(), PermissionMode::Auto);
    let prompt = seen_prompt.lock().expect("prompt lock").clone();
    assert!(prompt.contains("Auto permission mode remained active"));
}
```

- [ ] **Step 2: Run one Auto-mode preflight test and confirm it fails**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init_in_auto_opens_preflight_without_starting_turn
```

Expected: FAIL because `/init` currently starts immediately in Auto mode.

- [ ] **Step 3: Add pending init instruction state to the controller**

In `crates/neo-agent/src/modes/interactive/mod.rs`, add this field to `InteractiveController`:

```rust
pending_init_instruction: Option<String>,
```

Initialize it to `None` in all `InteractiveController` constructors, including `new_for_test`.

- [ ] **Step 4: Add a generic preflight choice picker opener**

In `slash_commands.rs`, add:

```rust
fn open_init_preflight(&mut self, instruction: &str) {
    let preflight = init_command::init_preflight();
    self.pending_init_instruction = Some(instruction.to_owned());
    let theme = self.tui.chrome().theme();
    self.tui
        .chrome_mut()
        .open_choice_picker(neo_tui::dialogs::ChoicePickerOptions {
            title: preflight.title,
            items: vec![
                neo_tui::dialogs::ChoiceItem::new(
                    preflight.recommended_id,
                    "Switch to Ask and start",
                )
                .with_description(preflight.body.clone()),
                neo_tui::dialogs::ChoiceItem::new(
                    preflight.alternate_id,
                    "Stay Auto and generate best effort",
                )
                .with_description("Start /init without user questions. The agent will proceed with best-effort assumptions."),
                neo_tui::dialogs::ChoiceItem::new(preflight.cancel_id, "Cancel")
                    .with_description("Do not start /init."),
            ],
            initial_id: Some("preflight:init:switch-ask".to_owned()),
            theme,
            page_size: 3,
            current_id: None,
        });
}
```

This uses init-specific ids but a generic choice-picker mechanism. Keep the type names `InitPreflight` and `PreflightDecision` generic enough to move later.

- [ ] **Step 5: Gate `/init` in Auto mode**

Replace the `/init` branch from Task 2 with:

```rust
if let Some(instruction) = init_command::init_instruction(prompt) {
    self.clear_submitted_prompt();
    if self.permission_mode == PermissionMode::Auto {
        self.open_init_preflight(instruction);
        return true;
    }
    if let Err(error) = self.run_init_workflow(instruction, false).await {
        self.push_status(format!("Failed to start /init: {error}"));
    }
    return true;
}
```

- [ ] **Step 6: Handle preflight choice ids**

In `crates/neo-agent/src/modes/interactive/dialog_results.rs`, make choice-result handling async so a selected preflight option can start and drain the generated `/init` turn.

Update `process_provider_dialog_result`:

```rust
} else if self.tui.chrome_mut().choice_picker_result().is_some() {
    self.handle_choice_picker_result().await;
```

Change `handle_choice_picker_result`:

```rust
pub(super) async fn handle_choice_picker_result(&mut self) {
    let Some(result) = self.tui.chrome_mut().choice_picker_result().cloned() else {
        return;
    };
    self.tui.chrome_mut().close_focused_overlay();
    if let neo_tui::dialogs::ChoiceResult::Selected(item) = result {
        self.handle_selected_choice_item(&item.id).await;
    }
}
```

Change `handle_selected_choice_item` to async and check preflight before existing permission/catalog handling:

```rust
pub(super) async fn handle_selected_choice_item(&mut self, id: &str) {
    if self.handle_preflight_choice_item(id).await {
        return;
    }
    if self.handle_permission_choice_item(id) {
        return;
    }
    if self.handle_catalog_choice_item(id) {
        return;
    }
    self.handle_builtin_choice_item(id);
}

pub(super) async fn handle_preflight_choice_item(&mut self, id: &str) -> bool {
    let Some(decision) = init_command::preflight_decision(id) else {
        return false;
    };
    let instruction = self.pending_init_instruction.take().unwrap_or_default();
    match decision {
        init_command::PreflightDecision::SwitchPermissionMode(mode) => {
            self.set_permission_mode(mode);
            if let Err(error) = self.run_init_workflow(&instruction, false).await {
                self.push_status(format!("Failed to start /init: {error}"));
            }
        }
        init_command::PreflightDecision::Continue => {
            if let Err(error) = self.run_init_workflow(&instruction, true).await {
                self.push_status(format!("Failed to start /init: {error}"));
            }
        }
        init_command::PreflightDecision::Cancel => {
            self.push_status("/init cancelled");
        }
    }
    true
}
```

Import `init_command` at the top of `dialog_results.rs`:

```rust
use super::init_command;
```

`start_init_workflow` must be `pub(super)` because `dialog_results.rs` calls it after the preflight picker returns a choice id.

- [ ] **Step 7: Run the preflight tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init_in_auto_opens_preflight_without_starting_turn
cargo nextest run -p neo-agent --bin neo init_preflight_switch_to_ask_starts_workflow
cargo nextest run -p neo-agent --bin neo init_preflight_continue_keeps_auto_and_starts_best_effort
```

Expected: all three tests pass.

## Task 4: Add Slash Completion and Docs

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `docs/en/reference/slash-commands.md`
- Modify: `docs/zh/reference/slash-commands.md`

- [ ] **Step 1: Add a failing completion test**

Add this test near the existing slash completion tests:

```rust
#[test]
fn slash_completions_include_init_command() {
    let completions = prompt_completions(&test_workspace_root(), "/", &[], None, true)
        .expect("slash completions");
    let values: Vec<_> = completions.iter().map(|item| item.value.as_str()).collect();

    assert!(values.contains(&"/init"), "missing /init: {values:?}");
}
```

- [ ] **Step 2: Run the completion test and confirm it fails**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_completions_include_init_command
```

Expected: FAIL because `/init` is not in `STATIC_SLASH_COMMANDS`.

- [ ] **Step 3: Add `/init` to static completions**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, add this entry to `STATIC_SLASH_COMMANDS`:

```rust
("/init", "Create or refresh AGENTS.md"),
```

Place it near other session/project workflow commands such as `/compact` and `/plan`.

- [ ] **Step 4: Run the completion test**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_completions_include_init_command
```

Expected: PASS.

- [ ] **Step 5: Update English slash docs**

In `docs/en/reference/slash-commands.md`, add `/init [instruction]` to the appropriate section:

```md
| `/init [instruction]` | — | Create or refresh the workspace `AGENTS.md`. Extra text is passed to the init workflow as natural-language guidance. |
```

Also add a short note:

```md
`/init` is TUI-only. In Auto permission mode it first opens a preflight dialog so the user can switch to Ask mode before the workflow asks for reference locations or durable project preferences.
```

- [ ] **Step 6: Update Chinese slash docs**

In `docs/zh/reference/slash-commands.md`, add:

```md
| `/init [instruction]` | — | 创建或刷新工作区 `AGENTS.md`；后续文本会作为自然语言指导传入 init 工作流。 |
```

Add:

```md
`/init` 仅支持 TUI 交互模式。在 Auto 权限模式下会先打开预检对话框，方便用户切换到 Ask 模式，以便工作流询问 reference 位置和长期项目偏好。
```

- [ ] **Step 7: No docs tests required**

No test command is required for the Markdown-only documentation edits beyond the completion test already run.

## Task 5: Add Post-write Validation and One Repair Attempt

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive/init_command.rs`
- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add failing test for repair prompt**

Add:

```rust
#[test]
fn init_repair_prompt_lists_validator_issues() {
    let issues = vec![
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingHeading,
            message: "Missing required heading: Security".to_owned(),
        },
        init_command::AgentsGuideIssue {
            code: init_command::AgentsGuideIssueCode::MissingMetadataField,
            message: "Missing Metadata field: Best valid until:".to_owned(),
        },
    ];

    let prompt = init_command::build_agents_guide_repair_prompt(&issues);

    assert!(prompt.contains("AGENTS.md structure validation failed"));
    assert!(prompt.contains("Missing required heading: Security"));
    assert!(prompt.contains("Missing Metadata field: Best valid until:"));
    assert!(prompt.contains("Update AGENTS.md in place"));
    assert!(prompt.contains("Do not claim success until the issues are fixed"));
}
```

Also add this integration test near the `/init` slash tests:

```rust
#[tokio::test]
async fn slash_init_runs_one_repair_turn_when_agents_guide_validation_fails() {
    let dir = tempfile::tempdir().expect("temp dir");
    let agents_path = dir.path().join("AGENTS.md");
    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = std::sync::Arc::clone(&turn_count);
    let agents_path_clone = agents_path.clone();
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        dir.path().to_path_buf(),
        move |request| {
            let turn_count = std::sync::Arc::clone(&turn_count_clone);
            let agents_path = agents_path_clone.clone();
            async move {
                let prompt = request
                    .prompt
                    .iter()
                    .filter_map(Content::as_text)
                    .collect::<Vec<_>>()
                    .join("");
                let turn = turn_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if prompt.contains("AGENTS.md structure validation failed") {
                    std::fs::write(
                        agents_path,
                        init_command::example_agents_guide_for_tests("abc1234"),
                    )
                    .expect("write repaired AGENTS");
                } else {
                    std::fs::write(
                        agents_path,
                        "## Reference\ntext\n\n## Metadata\nCreated: 2026-07-07\n",
                    )
                    .expect("write invalid AGENTS");
                }
                assert!(turn < 2, "only initial and repair turns should run");
                Ok(Vec::<AgentEvent>::new())
            }
        },
    );

    controller.type_text("/init");
    controller
        .handle_input_event(InputEvent::Submit)
        .await
        .expect("/init handled");

    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert!(transcript_has_status(
        &controller,
        "AGENTS.md structure validation passed after repair"
    ));
}
```

- [ ] **Step 2: Run the repair tests and confirm they fail**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_repair_prompt_lists_validator_issues
cargo nextest run -p neo-agent --bin neo slash_init_runs_one_repair_turn_when_agents_guide_validation_fails
```

Expected: FAIL because `build_agents_guide_repair_prompt` and the post-write repair flow do not exist.

- [ ] **Step 3: Implement repair prompt builder**

Add to `init_command.rs`:

```rust
pub(super) fn build_agents_guide_repair_prompt(issues: &[AgentsGuideIssue]) -> String {
    let issue_lines = issues
        .iter()
        .enumerate()
        .map(|(index, issue)| format!("{}. {:?}: {}", index + 1, issue.code, issue.message))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "AGENTS.md structure validation failed.\n\nIssues:\n{issue_lines}\n\nUpdate AGENTS.md in place to fix these issues. Preserve correct project-specific content. Do not claim success until the issues are fixed."
    )
}
```

- [ ] **Step 4: Run repair prompt test**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_repair_prompt_lists_validator_issues
```

Expected: PASS.

- [ ] **Step 5: Wire one automatic post-write validation and repair attempt**

Add this method inside `impl InteractiveController` in `slash_commands.rs`:

```rust
async fn repair_agents_guide_once_if_needed(&mut self) -> Result<()> {
    let path = self.tui.chrome().workspace_root().join("AGENTS.md");
    let Ok(markdown) = tokio::fs::read_to_string(&path).await else {
        return Ok(());
    };
    let issues = init_command::validate_agents_guide(&markdown);
    if issues.is_empty() {
        self.push_status("AGENTS.md structure validation passed");
        return Ok(());
    }

    let repair_prompt = init_command::build_agents_guide_repair_prompt(&issues);
    let reminder = init_command::wrap_init_system_reminder(&repair_prompt);
    self.start_generated_injection_turn_from_text(reminder, "init", "/init AGENTS.md repair")?;
    self.drain_active_turn().await?;

    let Ok(repaired_markdown) = tokio::fs::read_to_string(&path).await else {
        self.push_status("AGENTS.md repair finished, but file could not be re-read");
        return Ok(());
    };
    let remaining = init_command::validate_agents_guide(&repaired_markdown);
    if remaining.is_empty() {
        self.push_status("AGENTS.md structure validation passed after repair");
    } else {
        self.push_status(format!(
            "AGENTS.md still has {} structure validation issue(s)",
            remaining.len()
        ));
    }
    Ok(())
}
```

This is intentionally one repair attempt. Do not loop until success.

- [ ] **Step 6: Run focused repair and prompt tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_repair_prompt_lists_validator_issues
```

Expected: PASS.

## Task 6: Narrow Integration Verification

**Files:**

- Modify: none unless earlier tasks reveal compile issues.

- [ ] **Step 1: Run all focused `/init` tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_init
```

Expected: PASS for all tests whose names contain `slash_init`.

- [ ] **Step 2: Run focused preflight tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_preflight
```

Expected: PASS.

- [ ] **Step 3: Run focused validator and prompt tests**

Run:

```bash
cargo nextest run -p neo-agent --bin neo init_command
```

Expected: PASS.

- [ ] **Step 4: Run focused completion test**

Run:

```bash
cargo nextest run -p neo-agent --bin neo slash_completions_include_init_command
```

Expected: PASS.

- [ ] **Step 5: Format check only if Rust files changed**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

If formatting fails, run `cargo fmt --all`, then rerun:

```bash
cargo fmt --all --check
```

Expected: PASS.

## Task 7: Main-Agent Git Checkpoint After Explicit Authorization

**Files:**

- All files changed by earlier tasks.

- [ ] **Step 1: Inspect status**

Run:

```bash
git status --short
```

Expected: shows only files intentionally changed for `/init` plus the spec and plan docs if they are still untracked.

- [ ] **Step 2: Ask for git mutation authorization**

Ask the user for explicit authorization before any `git add` or `git commit`.

Use this wording:

```text
I have verified the /init implementation. May I stage the explicit changed paths and create one conventional commit?
```

- [ ] **Step 3: If authorized, stage explicit paths only**

Run only after the user authorizes this specific mutation:

```bash
git add \
  crates/neo-agent/src/modes/interactive/init_command.rs \
  crates/neo-agent/src/modes/interactive/mod.rs \
  crates/neo-agent/src/modes/interactive/slash_commands.rs \
  crates/neo-agent/src/modes/interactive/dialog_results.rs \
  crates/neo-agent/src/modes/interactive/prompt_completion.rs \
  crates/neo-agent/src/modes/interactive/tests.rs \
  crates/neo-agent/Cargo.toml \
  docs/en/reference/slash-commands.md \
  docs/zh/reference/slash-commands.md \
  docs/superpowers/specs/2026-07-07-init-slash-command-design.md \
  docs/superpowers/plans/2026-07-07-init-slash-command.md
```

Expected: command exits successfully.

- [ ] **Step 4: If authorized, commit**

Run only after the user authorizes this specific mutation:

```bash
git commit -m "feat(slash): add init AGENTS workflow"
```

Expected: commit succeeds.

## Self-Review

Spec coverage:

- TUI-only `/init`: Task 2.
- `/init [instruction]` raw instruction: Task 1 and Task 2.
- Existing `AGENTS.md` in-place update instruction: Task 1 prompt builder.
- User-confirmed reference paths: Task 1 prompt builder.
- Reusable Auto-mode preflight: Task 3.
- Built-in structure template: Task 1.
- Pre-write outline: Task 1.
- Golden style example: Task 1.
- Post-write validator: Task 1.
- Generated prompt delivery with injection origin instead of `submit_prompt_text`: Task 2.
- Runtime JSONL origin preservation for `/init` and repair prompts: Task 2 and Task 5.
- Repair prompt support: Task 5.
- Completion and docs: Task 4.
- Narrow verification: Task 6.
- Git policy: Task 7.

Placeholder scan:

- The plan intentionally includes forbidden-marker strings only inside validator test/implementation examples because the validator must detect those literals. Do not copy those strings into generated docs.

Type consistency:

- `InitPromptRequest`, `InitPreflight`, `PreflightDecision`, `AgentsGuideIssueCode`, `AgentsGuideIssue`, `build_init_workflow_prompt`, `wrap_init_system_reminder`, `build_agents_guide_repair_prompt`, `validate_agents_guide`, and `init_instruction` are introduced in Task 1 and reused consistently in later tasks.
- `TurnRequest.prompt_origin`, `start_turn_with_prompt_origin`, `start_generated_injection_turn_from_text`, `run_prompt_streaming(..., prompt_origin, ...)`, `run_prompt_in_session_streaming(..., prompt_origin, ...)`, `append_user_event(..., origin, ...)`, and `append_user_event_jsonl(..., origin, ...)` use the same `neo_agent_core::MessageOrigin` type throughout the request-to-runtime path.
