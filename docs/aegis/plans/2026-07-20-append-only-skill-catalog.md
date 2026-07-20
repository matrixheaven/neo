# Append-Only Skill Catalog Implementation Plan

**Goal:** Implement the approved append-only skill catalog contract.

**Architecture:** `SkillStore` owns deterministic rendering. `AgentRuntime`
compares the current rendering with the latest same-origin replayed reminder
and emits `MessageAppended` only when it changed.

**Tech Stack:** Rust 2024, Neo session JSONL, `AgentMessage`/`MessageOrigin`.

**Baseline/Authority Refs:**
`docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md`,
`docs/aegis/specs/2026-07-08-prompt-cache-hit-rate-design.md`, and
`docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`.

**Compatibility Boundary:** Resume remains silent and non-blocking. Existing
history is never rewritten. No compatibility owner or new event shape is added.

**TDD Route:** Mode `off`; decision `skipped`; use post-change regressions.

**Verification:** One exact `neo-agent-core` runtime test, one exact
`neo-agent` resource test, touched-file rustfmt, and `git diff --check`.

## Task 1: Move catalog ownership to `SkillStore`

**Files:** `crates/neo-agent-core/src/skills/mod.rs`,
`crates/neo-agent/src/resources.rs`.

**Change Necessity:** Configuration cannot make `HashMap` iteration
deterministic or persist a replayable delta. Add
`SkillStore::available_skills_prompt() -> String`, sort auto-invokable skills,
and delete the old system-prompt formatter/call path.

**Verification:**
`cargo test --package neo-agent --bin neo -- resources::tests::system_prompt_excludes_available_skills --exact --nocapture --include-ignored`.

## Task 2: Persist only changed snapshots

**Files:** `crates/neo-agent-core/src/runtime/agent.rs`,
`crates/neo-agent-core/tests/runtime_turn.rs`.

**Change Necessity:** The runtime is the existing owner that has both the live
`SkillStoreHandle` and replayed `AgentContext`. Before the user message, build
`system_reminder_with_origin(catalog, "available_skills")`, compare it with
the latest same-origin message, and emit only on change.

**Verification:**
`cargo test --package neo-agent-core --test runtime_turn -- runtime_appends_available_skills_snapshot_only_when_changed --exact --nocapture --include-ignored`.

## Retirement

Delete skill rendering from `load_system_prompt`; retain no fallback or dual
owner. The first resume of a pre-feature session is the only unavoidable cache
reset because its original randomized catalog bytes were not stored.
