# Use-case recipes

This page offers a set of ready-to-reuse prompt templates with typical expected outcomes. Each recipe is organized as "scenario → template → expected result".

## General tips

- Be specific: name file paths, function names, and constraints.
- Use `@file` references to feed code into context so Neo doesn't guess wrong.
- For complex changes, run `/plan` first so Neo produces a plan you can approve.
- For multi-turn autonomous tasks, use `/goal`.

---

## Code review

> Goal: Have Neo assess the quality, risks, and improvements of a piece of code.

```
Review the security and error handling of @src/auth/session.rs.
Focus on:
1. Whether the auth flow has privilege-escalation or timing vulnerabilities
2. Whether error branches leak sensitive information
3. Whether there's input parsing that could be fuzzed
List issues sorted by severity, each with file:line and a fix suggestion.
```

**Expected**: Neo reads through the relevant files with Read/Grep, outputs a severity-ranked issue list (with line numbers and suggestions), and offers patch drafts when useful. You can then push it with "fix them one by one, highest severity first".

---

## Implement a feature

> Goal: Implement a full new feature. Plan mode is recommended first.

```
/plan
Add a `neo foo <name>` subcommand to the CLI:
- Read a specific section from ~/.neo/config.toml
- Output as JSON or plain text (--output text|json)
- Reuse the existing clap subcommand style
Produce a plan first, including the files touched and the order of changes.
```

**Expected**: Neo enters plan mode, investigates `cli.rs` and existing subcommand implementations, writes a step-by-step plan to the plan file, and calls `ExitPlanMode` to pop the approval. After approval, it implements and self-tests per the plan.

Variant (autonomous multi-turn implementation):

```
/goal Implement the neo foo subcommand and add unit tests. Completion criterion: cargo nextest run -p neo-agent is fully green
```

---

## Fix a bug

> Goal: Locate and fix a bug from its symptoms.

```
Symptom: Running `neo sessions list` panics when a session's summary field is null.
Stack bottom is at crates/neo-agent/src/modes/sessions.rs list().
@crates/neo-agent/src/modes/sessions.rs
Locate the root cause, give a minimal fix, and add a regression test.
```

**Expected**: Neo pins down the issue with Grep/Read, proposes a fix, and either writes it (per permission mode) or pops an approval; then adds a test case verifying "a null summary still lists cleanly".

Including a reproduction command is even better:

```
Repro: NEO_HOME=/tmp/neo-empty neo sessions list
```

---

## Refactor

> Goal: Improve structure without changing external behavior. Make sure test guardrails exist first.

```
/plan
Split goal_continuation_messages() in crates/neo-agent-core/src/runtime/turn_loop.rs
into a standalone module runtime/goal_continuation.rs, keeping the behavior of every
call site unchanged.
Precondition: confirm cargo nextest run -p neo-agent-core --lib is fully green as the baseline first.
```

**Expected**: Neo first runs the existing tests to confirm a green baseline, then produces a refactor plan noting which symbols move and how call sites adjust; after approval, it executes and re-runs the tests.

---

## Investigate a codebase

> Goal: Understand the structure, dependencies, and critical paths of an unfamiliar area. Investigation only — no code changes.

```
I'm taking over the input subsystem of crates/neo-tui.
Give me a tour:
1. Entry module and public API
2. How events flow from keypress to KeybindingAction
3. The boundary with InteractiveController
4. A recommended reading order
Do not modify any files.
```

**Expected**: Neo produces a structured tour with file:line references using read-only tools; you can follow up with "where's the best place to add a custom keybinding".

Quick-scan variant:

```
In one sentence each, summarize the responsibility of every tool under @crates/neo-agent-core/src/tools/, output as a table.
```

---

## Write tests

> Goal: Backfill unit/integration tests for a module.

```
Add a table-driven set of unit tests for prevalidate_exit_plan_mode
in crates/neo-agent-core/src/tools/plan_mode.rs, covering:
- valid input
- reserved labels (approve/reject/revise)
- duplicate labels
- more than 3 options
- more than 5 suggestions
Keep it consistent with the existing #[cfg(test)] style.
```

**Expected**: Neo reads the existing test style, adds new `#[test]` cases, and runs `cargo nextest run -p neo-agent-core --lib plan_mode` to confirm green.

---

## Prompt cheat sheet

| Goal | Recommended entry |
| --- | --- |
| Small-scope change | Direct `neo run "..."` or interactive input |
| Medium/large implementation | `/plan` for a plan → approve → execute |
| Autonomous long task | `/goal <objective>` with a completion criterion |
| Investigation | Explicitly write "do not modify any files" |
| Adding tests | Give the function name, the branches to cover, and the existing test style |

## Next steps

- [Interaction mode](interaction.md) — `/plan`, `/goal`, approvals, and permission modes in depth
- [Plan mode](plan-mode.md) — The plan approval flow
- [Goal mode](goals.md) — Autonomously driving a verifiable objective
- [Quickstart](../quickstart.md) — Command and flag cheat sheet
