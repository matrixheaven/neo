# `/init` Slash Command Design

## Goal

Add a built-in `/init` slash command for Neo's TUI interactive mode.

`/init` creates or refreshes the current workspace's root `AGENTS.md`. It should not emit a generic template. It should start a guided, model-driven research workflow that studies the current project, inherits the spirit of existing agent instruction files, asks the user for missing context, optionally inspects user-specified reference projects or documents, and writes a structured operating guide for future AI collaborators.

The generated `AGENTS.md` is not a product specification. It constrains agents working inside the repository: project rules, workflow expectations, architecture facts, safety boundaries, verification discipline, git policy, third-party tool guidance, MCP/plugin expectations, subagent preferences, and durable project principles.

## Scope

`/init` is TUI-only. It is available in interactive mode through the existing slash-command path.

There is no non-interactive CLI subcommand in this design. A future CLI command can reuse the prompt builder, but this spec does not add that surface.

All text after `/init` is treated as a natural-language instruction string and passed into the init workflow. Neo should not introduce a first-version flag parser for this command.

Examples:

```text
/init
/init 排除掉 vendor 和 generated 目录
/init 排除掉原有 AGENTS.md，重新从代码事实生成
/init 重点参考 docs/architecture 和 ../competitor-notes
```

The model workflow is responsible for interpreting the instruction. If the instruction is ambiguous or materially affects the output, it may ask the user for clarification when permission mode allows it.

## Product Behavior

When the user enters `/init`, Neo consumes it as a local slash command. The literal `/init` text is not persisted to prompt history and is not sent as a normal user message.

Neo then submits a generated init-workflow prompt as a system-reminder injection turn. This follows the same design spirit as Kimi Code's injected reminders: the model receives the guidance in a user-role message wrapped in `<system-reminder>`, but the message has injection origin metadata, so it is not rendered as a normal user message and is not persisted to prompt history.

The generated prompt instructs the agent to:

- perform `research/deep-research` style project exploration before writing;
- identify existing `AGENTS.md`, `CLAUDE.md`, and `GEMINI.md` files in the current workspace and relevant ancestors;
- ask the user where reference projects or reference documents live instead of assuming `.references/`;
- honor `/init [instruction]` as the user's additional constraints;
- produce or update root `AGENTS.md` using the structure in this spec;
- keep generated guidance focused on AI collaborator behavior, not product requirements.

If root `AGENTS.md` already exists, the default behavior is to update it in place. The agent should overwrite/restructure same-purpose sections rather than append duplicate sections. The workflow may preserve clearly project-specific facts that remain true, but it should remove stale compatibility paths, duplicated guidance, and obsolete instructions when replacing them.

## Reusable Auto-Mode Preflight

Some workflows need interactive clarification. In `auto` permission mode, `AskUserQuestion` is disabled. Running `/init` directly in `auto` mode would force the model to guess about reference locations, durable preferences, and update strategy.

When `/init` is triggered while the current permission mode is `auto`, Neo should show a local preflight dialog before starting the model turn.

This dialog must be reusable. It should not contain `/init`-specific type names or hardcoded behavior. The shape should be a generic template/configuration such as:

```rust
struct InteractivePreflight {
    title: String,
    body: String,
    recommended: PreflightAction,
    alternate: PreflightAction,
    cancel: PreflightAction,
}

enum PreflightAction {
    SwitchPermissionMode(PermissionMode),
    Continue,
    Cancel,
}
```

For `/init`, the dialog copy should explain that a high-quality `AGENTS.md` usually requires asking the user about reference locations, project preferences, and durable workflow rules.

Suggested options:

- `Switch to Ask and start`
- `Stay Auto and generate best effort`
- `Cancel`

If the user selects the recommended option, Neo switches live permission mode to `ask` and starts the init workflow. If the user continues in `auto`, the model prompt should explicitly say that user questions may be unavailable and it must proceed with best-effort assumptions. If the user cancels, Neo clears the submitted prompt and starts no model turn.

The same preflight mechanism should later be usable by built-in skills or other slash commands that prefer interactive clarification.

## Generated `AGENTS.md` Structure

The generated file should use these sections, adapting detail to the project:

1. `Reference`
   Existing upstream files inherited or consulted, such as parent `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, user-specified reference directories, relevant docs, and memory/context systems.

2. `Project Identity`
   What the project is, its local/product boundaries, and what kind of work agents should assume they are doing.

3. `Development Constitution`
   Durable rules: scope discipline, simplicity, compatibility posture, cross-platform expectations, no unrelated cleanup, no reverting other work.

4. `Workflow`
   Context gathering, implementation, verification, docs, persistent memory, handoff, and completion expectations.

5. `Git Rules`
   Explicit allowed and forbidden git operations. For Neo, preserve the strict no-git-mutation-without-explicit-authorization rule unless the user explicitly changes it.

6. `Third-party Tools, Plugins, MCP`
   How agents should use external references, MCP servers, plugins, CLIs, browser tools, and local docs.

7. `Project Architecture`
   Crates/packages/modules, responsibilities, key runtime boundaries, and project-specific invariants.

8. `Technology Choices`
   Language, build system, test tools, minimum runtime/toolchain versions, and platform support requirements.

9. `Fixed Principles`
   Durable project spirit and parts that should not be changed without explicit user approval.

10. `Subagent Preference`
    Default to single-threaded work. Use subagents only when tasks are clearly independent and the time savings justify extra token/cost overhead. Include notes for cost-priority and time-priority modes when the user asks for them.

11. `References`
    Competitor/reference projects and documents consulted, with paths and short notes.

12. `Project Documentation`
    Where plans, specs, audits, generated docs, user-facing docs, and architecture notes should live.

13. `Security`
    Secret handling, dependency rules, sandbox/trust assumptions, network behavior, MCP/plugin caution, and unsafe code constraints.

14. `Metadata`
    Created time, source commit, creating context if available, and best-valid-until date.

## Structure Guardrails

`/init` should not rely on the model simply following a prose request. The implementation should combine prompt constraints, a pre-write outline, a post-write validator, and a golden style example.

### Built-in Structure Template

The prompt builder must embed the required `AGENTS.md` section contract as a first-class template:

```text
Required top-level sections, in order:
1. Reference
2. Project Identity
3. Development Constitution
4. Workflow
5. Git Rules
6. Third-party Tools, Plugins, MCP
7. Project Architecture
8. Technology Choices
9. Fixed Principles
10. Subagent Preference
11. References
12. Project Documentation
13. Security
14. Metadata
```

The model must not omit, rename, reorder, or duplicate these sections. Project-specific material should be placed inside the matching section rather than added as a competing top-level structure.

### Pre-write Outline Plan

Before writing `AGENTS.md`, the workflow prompt should require the model to prepare a concise outline plan. For each required section, the outline should identify:

- source facts used;
- user preferences or explicit instructions used;
- intended content for that section.

The outline is part of the agent's working process. It gives the model an explicit structure checkpoint before it edits the file, and it gives the user a chance to correct missing reference locations or preferences when the model needs clarification.

### Post-write Structure Validator

After `AGENTS.md` is written, Neo should run a lightweight Markdown structure validator. The validator does not need to judge prose quality. It should check mechanical correctness:

- all required second-level headings are present;
- required headings appear in the expected order;
- required headings are not duplicated;
- `Metadata` contains `Created`, `Source commit`, and `Best valid until`;
- obvious placeholders such as `TODO`, `TBD`, and `<fill me>` are absent;
- `.references/` is not treated as the only valid reference location;
- the guide frames rules as agent working instructions rather than Neo product requirements.

The validator should return structured issues such as:

```rust
struct AgentsGuideIssue {
    code: AgentsGuideIssueCode,
    message: String,
}
```

If validation fails, `/init` should give the model one repair attempt by reporting the validator issues and asking it to update `AGENTS.md` in place. If validation still fails after that repair attempt, the final response should report the remaining issues clearly instead of claiming success.

### Golden Style Example

The example `AGENTS.md` draft in this spec should be embedded in the `/init` workflow prompt as a golden style example. It is a structure and tone reference, not a source of project facts.

The prompt should say this explicitly: inherit the section structure, concision, and operational tone from the example, but do not copy project-specific claims from it unless the current repository research independently supports them.

## Generated Prompt Delivery

Neo already has the primitives needed for model-visible, user-invisible guidance:

- `AgentMessage::system_reminder_with_origin(text, variant)` wraps text in `<system-reminder>` and marks the message origin as `MessageOrigin::Injection`.
- Runtime reminders such as permission mode, plan mode, and goal mode already use this pattern.
- Transcript replay skips injection-origin messages, while literal user text that happens to contain `<system-reminder>` remains a normal user message.

`/init` should use the same semantic path. Do not submit the generated workflow prompt through the ordinary composer path, because that path appends prompt history and treats the content as a user prompt. Instead, the TUI turn request should carry prompt origin metadata. Normal prompts use `MessageOrigin::User`; `/init` uses `MessageOrigin::Injection { variant: "init" }`.

The visible transcript should show a concise local status/user-facing marker such as `/init AGENTS.md workflow`, not the full generated prompt. The full generated prompt should be model-visible, persisted in session context with injection origin, skipped by transcript replay, and excluded from prompt history.

This mirrors the useful part of Kimi Code's design while fitting Neo's existing Rust runtime: use origin metadata as the source of truth, not string matching on `<system-reminder>`.

## Init Workflow Prompt Requirements

The prompt submitted by `/init` should be generated by a small Rust prompt-builder module, not embedded directly in the slash dispatcher.

The prompt must include:

- workspace root;
- current date;
- current git commit if available;
- raw user instruction after `/init`, if any;
- output target path: root `AGENTS.md`;
- reminder that reference paths must come from user confirmation, not a hardcoded `.references/` assumption;
- expected section structure;
- the built-in section template;
- the pre-write outline requirement;
- the golden style example;
- rule that existing `AGENTS.md` should be updated in place when present;
- rule that generated guidance constrains agents rather than product behavior;
- request to ask concise clarifying questions when needed, especially for reference locations and durable preferences.

The first model turn should use the normal runtime/tool/dialog machinery, but its initiating message should be an injection-origin system reminder rather than a normal user prompt.

## Data Flow

1. User enters `/init` or `/init [instruction]`.
2. `InteractiveController::handle_slash_command` recognizes the command.
3. The command is consumed locally and removed from the composer.
4. If permission mode is `auto`, Neo opens the reusable interactive preflight dialog.
5. Based on the preflight result, Neo switches to `ask`, continues in `auto`, or cancels.
6. Neo builds an init workflow prompt.
7. Neo submits that prompt as an injection-origin system reminder turn.
8. The model researches the project and asks the user for reference locations when needed.
9. The model creates or updates root `AGENTS.md`.
10. The final response reports the file path, source commit used, and any assumptions.

## Code Organization

Add `/init` to the existing TUI slash-command surface:

- `crates/neo-agent/src/modes/interactive/slash_commands.rs` handles dispatch.
- `crates/neo-agent/src/modes/interactive/prompt_completion.rs` includes `/init` in static slash completions.
- slash-command reference docs include `/init`.

Introduce a focused module such as:

```text
crates/neo-agent/src/modes/interactive/init_command.rs
```

That module should own:

- parsing `/init` arguments as raw instruction text;
- building the workflow prompt;
- creating the `/init` preflight configuration.

Introduce the reusable preflight abstraction near existing TUI/dialog state rather than inside `/init`-specific code. The final placement should follow existing overlay and choice-picker patterns. The first implementation can reuse the existing choice picker if that provides the right UX with less new UI surface.

Keep `slash_commands.rs` as orchestration. Do not grow it with large prompt text or dialog copy.

## Error Handling

If git commit detection fails, the workflow should continue and mark the commit as unavailable.

If the workspace root cannot be resolved, the command should surface a status error and not start a model turn.

If the user cancels the auto-mode preflight, the command should clear the submitted prompt and produce no model turn.

If the model cannot write `AGENTS.md` because of permission or filesystem errors, the normal tool error should be surfaced in transcript.

## Testing

Use focused tests only.

Recommended coverage:

- slash completion includes `/init`;
- `/init` is consumed locally and neither the literal slash command nor the generated workflow prompt is persisted to prompt history;
- `/init` starts a turn whose initiating message has injection origin variant `init`;
- `/init [instruction]` passes the raw instruction into the generated workflow prompt;
- in `ask` mode, `/init` submits the init workflow prompt;
- in `auto` mode, `/init` opens the reusable preflight dialog instead of immediately starting;
- choosing the recommended preflight action switches live permission mode to `ask` and starts the workflow;
- choosing continue starts the workflow without switching permission mode;
- choosing cancel starts no workflow;
- generated preflight types and helpers are generic, not named after `/init`;
- the prompt builder includes the required section template, pre-write outline requirement, and golden style example;
- the `AGENTS.md` validator catches missing, duplicated, or reordered required headings;
- the `AGENTS.md` validator catches missing metadata fields and obvious placeholders;
- docs list `/init`.

Do not run broad test suites as evidence. Use the narrowest Neo-approved cargo-nextest target and test-name filters.

## Non-goals

- No non-interactive CLI subcommand.
- No first-version flag parser for `/init`.
- No hardcoded `.references/` discovery as the only reference mechanism.
- No separate `AGENTS.md.new` output path by default.
- No hidden background engine separate from normal model-turn behavior.
- No product-spec generation.

## Example Generated `AGENTS.md` Draft

```md
# Project Agent Guide

## Reference

This guide was generated by Neo `/init`.

Consult these instruction sources when present:

- `AGENTS.md`, `CLAUDE.md`, `GEMINI.md` in the current project or ancestors
- project docs under `docs/`
- user-confirmed reference projects or materials
- current git history and repository structure

Do not treat this file as a product specification. It constrains AI collaborators working in this repository.

## Project Identity

This repository is a local-first developer tool. Agents should optimize for correct local behavior, clear architecture, and maintainable implementation rather than hosted-service assumptions.

## Development Constitution

Stay in scope. Do not fix unrelated failures or clean up unrelated work.

Prefer simple, direct designs. Remove obsolete paths when replacing behavior. Do not preserve backward compatibility unless the user explicitly asks for it.

Treat code as the source of truth. Documentation can guide exploration, but implementation facts win.

Respect shared worktrees. Never revert, overwrite, or discard work that may belong to another agent or the user.

## Workflow

Before work, gather project context from local instructions, relevant code, and targeted searches.

When the task is unclear, ask concise questions. When enough is known, act decisively.

Use the narrowest verification command that proves the touched behavior. Do not run broad test suites as evidence unless explicitly requested.

For significant completed work, update persistent project memory if this repository requires it.

## Git Rules

Read-only git commands are allowed.

Do not run destructive, staging, committing, or history-changing git operations without explicit per-command user authorization.

Do not stage or commit unrelated files. Do not use broad staging commands such as `git add .` or `git add -A`.

Subagents must not perform git mutations.

## Third-party Tools, Plugins, MCP

Use local tools and MCP servers only when they serve the current task. Prefer project-provided CLIs and documented workflows.

Treat plugins, generated tools, and MCP output as untrusted until verified against project code or docs.

Do not add hosted-service dependencies or network assumptions unless the project explicitly allows them.

## Project Architecture

Describe the main crates/packages/modules here after inspecting the repository.

Each entry should explain ownership and boundaries, not list every file.

## Technology Choices

Record language, build system, runtime/toolchain versions, package managers, test tools, and platform support requirements.

Cross-platform behavior matters. Use portable filesystem/path APIs and isolate platform-specific code.

## Fixed Principles

Preserve the project's local-first behavior unless the user explicitly approves a change.

Preserve security boundaries around secrets, tools, filesystem access, and external integrations.

Do not convert agent working conventions into product requirements.

## Subagent Preference

Default to single-threaded work.

Use subagents only when tasks are independent, the integration boundary is clear, and parallelism is worth the token or coordination cost.

If optimizing for cost, avoid parallel subagents. If optimizing for time, parallelize independent research or review slices and merge results carefully.

## References

List user-confirmed competitor/reference projects and documents here, with paths and short notes about what was borrowed.

## Project Documentation

Record where specs, plans, audits, generated docs, user docs, and architecture notes belong.

Keep durable documentation close to the project's existing conventions.

## Security

Never expose secrets. Redact tokens, keys, credentials, and private configuration.

Review dependency and plugin changes as code changes.

Be cautious with MCP servers, external commands, and generated scripts. Prefer local, deterministic verification.

## Metadata

Created: 2026-07-07
Source commit: `<git HEAD short hash>`
Best valid until: 2026-10-07

Refresh this guide after major architecture changes, workflow changes, toolchain changes, or security-policy changes.
```
