# Neo WebUI Composer Completion Implementation Plan

**Goal:** Add `/` command and `@` workspace-file completion to the WebUI composer. Empty transcripts place the list below the composer; non-empty transcripts place it above.

**Architecture:** The existing interactive `prompt_completion` module remains the only completion owner. `WebSessionHost` converts its candidates into typed `neo-webui` replies; the browser only renders and inserts relative values.

**Tech Stack:** Rust, Axum, React, TypeScript, Vitest, Playwright.

**Baseline/Authority Refs:** User-approved design in the 2026-08-11 task; `docs/aegis/specs/2026-08-09-neo-webui-design.md`; current `prompt_completion.rs` behavior.

**Compatibility Boundary:** Preserve current transcript/session behavior, do not expose absolute paths, do not duplicate command or file discovery, and preserve all pre-existing dirty WebUI edits.

**TDD Route:** Mode `off`; decision `skipped`; posture is post-change focused regression. No strict TDD authority was requested.

**Verification:** Exact Rust behavior tests, exact WebUI composer tests, frontend build, and Playwright checks for both popup placements.

## Scope Decisions

- Requirement ready: yes. The user fixed triggers, insertion behavior, and placement.
- Change necessity: a non-code option cannot create missing query, state, keyboard, or rendering paths. Decision: code-change.
- Existing owner: reuse `prompt_completion`; no second catalog or scanner.
- Architecture integrity: the host serves structured relative candidates; the browser never reads workspace paths directly.
- Complexity: `composer.tsx` and `host.rs` are large, so keep them wiring-only and put pure browser token/replacement logic in a small focused module if needed.
- Execution route: inline; the protocol, host, and composer changes are sequential and share types.

## Task 1: Serve Canonical Completion Candidates

**Files:** `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, `crates/neo-agent/src/modes/webui/host.rs`, `crates/neo-webui/src/protocol.rs`, `crates/neo-webui/src/server.rs`, focused Rust tests.

Expose the existing candidate query within `neo-agent`, add one typed WebUI query/reply, validate query length and trigger, and return only candidate value, label, description, and kind. Verify slash and file candidates without absolute paths.

## Task 2: Render and Operate the Composer Popup

**Files:** `crates/neo-webui/web/src/api.ts`, `crates/neo-webui/web/src/protocol.ts`, `crates/neo-webui/web/src/components/composer.tsx`, `crates/neo-webui/web/src/styles.css`, `crates/neo-webui/web/test/unit/composer.test.tsx`.

Detect the active `/` or `@` token at the caret, cancel stale queries, render a listbox below an empty transcript and above a non-empty transcript, and support pointer, ArrowUp/ArrowDown, Enter/Tab, and Escape. Selection replaces only the active token and restores focus.

## Task 3: Verify and Package

Run one exact Rust target/filter per touched boundary, the exact composer test file, the frontend build, and focused Playwright placement checks. Build the fixed embedded assets, stage only task-owned changes, and commit once with a conventional message.

## Risks and Retirement

- Stale asynchronous results are ignored by request identity.
- Empty results close the popup; request failure leaves normal typing intact.
- No old completion path is retired because none existed in WebUI. No compatibility fallback is added.
