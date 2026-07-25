# Delegate Tool Activity Summary And Theme Implementation Plan

## Goal

Implement the approved one-line collapsed DelegateSwarm file hint and restore
semantic theme styling across Delegate-family child tool activity.

## Architecture

Keep `child_activity.rs` as the only semantic activity presentation owner. It
will expose one shared span builder for tool status and optional inline file
metadata. Detailed cards call it without an inline file hint and continue
rendering every file below; collapsed Swarm calls it with the typed file list.
`swarm_card.rs` remains responsible only for choosing the latest child activity
and composing the existing child-row prefix.

## Tech Stack

- Rust 2024
- `neo-tui` `Line`, `Span`, `Style`, `TuiTheme`, Unicode width helpers
- Existing `AgentToolFileChange` projection
- `cargo nextest`, standalone `rustfmt`, `git diff --check`

## Baseline/Authority Refs

- `AGENTS.md`
- `docs/aegis/specs/2026-07-24-delegate-edit-write-file-activity-brief.md`
- `docs/aegis/specs/2026-07-25-delegate-tool-activity-summary-theme-brief.md`
- Current `child_activity.rs` and `swarm_card.rs`

## Compatibility Boundary

- No core/runtime/schema/event/persistence changes.
- Preserve card hierarchy, height, ordering, progress, and expansion behavior.
- Preserve non-Edit/Write summary wording and bounded shell-command behavior.
- Preserve the complete expanded file list and long-path wrapping.

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: post-change regression
- Reason: strict TDD was not requested; one existing integration target can
  exercise text, styles, and all three Delegate-family consumers.
- Verification: one exact TUI test, targeted formatting, scoped diff checks,
  and `cargo check` for the Neo binary.

## Verification

```bash
cargo nextest run -p neo-tui --test multi_agent_transcript delegate_family_tool_activity_uses_theme_and_collapsed_file_hint
rustfmt --check --edition 2024 crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/tests/multi_agent_transcript.rs
git diff --check -- crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-tui/tests/multi_agent_transcript.rs
cargo check -p neo-agent --bin neo
```

## Planning Readback

Requirement Ready Check:

- Source: approved 2026-07-25 Spec Brief behavior and layouts.
- Open blockers: none.
- Decision: ready.

Change Necessity:

- Need: collapsed Swarm discards typed file rows and both presentation paths
  flatten tool status into unstyled text.
- No-code option: theme/config changes cannot assign different styles to
  semantic segments inside one raw span.
- Minimum boundary: shared child activity renderer, collapsed Swarm composition,
  and one focused integration test.
- Decision: code-change.

Existence Check:

- Proposed surface: shared semantic span helper inside the existing renderer.
- Reuse candidate: `child_activity.rs`, `ChildToolRow`, and existing typed file
  fields.
- Decision: reuse-existing; no new module, type owner, or fallback.

Architecture Integrity Lens:

- Invariant: one owner selects representatives, computes totals, and maps theme
  colors.
- Caller responsibility: Swarm supplies existing layout prefix and selected
  typed activity only.
- Verdict: aligned.

Plan-Time Complexity Check:

- Target files: two established renderer files and one integration test.
- Pressure: `swarm_card.rs` already owns collapsed child selection;
  `child_activity.rs` already owns detailed semantic rendering.
- Recommendation: edit in place with small private helpers and one shared
  `pub(super)` span function.
- Budget: within-budget.

Plan Pressure Test: owner, compatibility, representative priority, style map,
and exact verification are explicit; result `proceed`.

## Execution Readiness View

- Intent Lock: one-line collapsed file identity plus theme restoration.
- Scope Fence: `child_activity.rs`, `swarm_card.rs`, one TUI test.
- Baseline Lock: approved 2026-07-25 Spec Brief and parent file-activity spec.
- Approved Behavior: deterministic risk-first representative and complete
  aggregate totals labeled `total`.
- Compatibility Boundary: no card-layout or core contract changes.
- Retirement Boundary: replace raw status spans; do not retain a second styled
  formatting path.
- Test Obligation: inspect text and custom-theme span styles through Delegate
  and collapsed/expanded Swarm.
- Drift Rule: stop if implementation requires new theme fields, core events, or
  a card-specific Edit/Write parser.
- Evidence: exact nextest result, rustfmt, scoped diff check, binary check.
- Advisory Boundary: planning guidance only, not completion authority.

## Task 1: Build Shared Semantic Tool Activity Spans

Files:

- `crates/neo-tui/src/transcript/child_activity.rs`

Why: current `Span::raw(status)` loses phase, tool-name, file-marker, and diff
styles even though the renderer already has the typed fields and theme.

Steps:

1. Add a `pub(super)` tool-status span builder accepting `name`, `summary`,
   `phase`, optional typed files, `now_ms`, maximum width, and `theme`.
2. Preserve existing verbs, summaries, queue/elapsed suffixes, and bounded
   Bash/Terminal behavior without parsing rendered strings.
3. When inline files are requested for Edit/Write, choose the representative
   by `Failed`, `CommittedUnsynced`, `Pending`, then first canonical row.
4. For multiple files, show the count and sum `added`/`removed` only when every
   row supplies both values and no row is pending. Prefix the aggregate with
   `total`; single-file stats remain file-local.
5. Split detailed file rows into semantic spans for marker, path, positive and
   negative stats, line count, and diagnostic. Keep existing wrapping and row
   order.
6. Make `render_child_tool_row` use the shared builder without inline files,
   so the detailed path list is not duplicated.

Impact/Compatibility: only styles and the approved status composition change;
the number and order of detailed rows remain stable.

Repair Track: replace the raw-span root cause in the shared renderer.

Retirement Track: remove the raw full-status span; retain plain bounded helpers
only where non-rendering text calculations still require them.

## Task 2: Use The Shared Builder In Collapsed Swarm

Files:

- `crates/neo-tui/src/transcript/swarm_card.rs`

Why: the collapsed row currently selects only `(name, summary, phase)` and
styles the complete activity as `text_primary`, losing both typed file data and
semantic styles.

Steps:

1. Include `files` when selecting ongoing or latest tool activity.
2. Make the collapsed activity summary return semantic spans and delegate tool
   composition to the shared builder with inline files enabled.
3. Keep queued/task/final-text fallbacks as current styled primary text.
4. Append the existing activity prefix and returned spans without changing the
   child row's one-line truncation or width budget.

Impact/Compatibility: collapsed child rows remain one line with identical
prefix/progress content; only the tool activity suffix changes.

Repair Track: remove the caller-side plain-string formatting path.

Retirement Track: `bounded_phase_tool_status` becomes unnecessary for collapsed
rendering and must be deleted if no caller remains.

## Task 3: Focused Regression And Commit

Files:

- `crates/neo-tui/tests/multi_agent_transcript.rs`

Steps:

1. Add `delegate_family_tool_activity_uses_theme_and_collapsed_file_hint` using
   a custom theme whose relevant colors are distinct.
2. Assert Delegate tool verb/name styles and detailed marker/path/diff styles.
3. Assert collapsed Swarm remains one row per child and renders single-file,
   multi-file `total`, pending, and failure-priority representative text.
4. Assert expanded Swarm retains the complete ordered file list without an
   inline duplicate in the detailed tool heading.
5. Run the exact Verification commands and inspect the scoped diff.
6. Stage only the planned docs/index and three Rust/test files, then commit with
   `fix(tui): improve delegate tool activity summaries`.

## Risks

- Styling can be lost if the collapsed caller concatenates ANSI/plain strings;
  append spans directly and truncate the final `Line`.
- Aggregate stats can mislead on partial data; omit them unless structurally
  complete.
- Inline file hints can duplicate expanded rows; detailed callers must disable
  the inline hint.
- Long paths can consume the row tail; retain deterministic truncation and the
  full expanded list rather than adding basename heuristics.

## Retirement

No compatibility formatter is retained. The plain collapsed status composition
is replaced by the shared semantic span builder. Existing bounded shell text
helpers remain only if they still serve non-rendering width calculations.

ADR signal: none; no durable architecture boundary changes.
