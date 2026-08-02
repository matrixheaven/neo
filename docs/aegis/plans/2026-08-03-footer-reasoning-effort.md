# Neo Footer Reasoning Effort Display

Date: `2026-08-03`

## Goal

Replace the footer's static `thinking` label with the current reasoning
selection value while preserving the existing single dynamic working spinner.
The footer must show `max`, `high`, or another effort directly after the model;
show `on` for plain enabled reasoning; and omit the label when reasoning is off.
Budget-token selections use a compact unambiguous `budget:<count>` label.

## Architecture

- `InteractiveController.current_reasoning` remains the canonical runtime
  selection and continues to receive the complete `neo_ai::ReasoningSelection`.
- `NeoChromeState` stores that same provider-neutral selection for the footer
  projection instead of reducing it to `thinking_enabled: bool`.
- `chrome_render::render_footer_lines` renders only the static selection label.
  `NeoChromeState::working_label` remains the only dynamic status owner and its
  existing spinner/working behavior is unchanged.
- No new enum, runtime status owner, footer configuration, or compatibility path
  is introduced.

## Tech Stack

Rust edition 2024 workspace; `neo-ai` owns `ReasoningSelection`, `neo-tui`
owns chrome state and footer rendering, and `neo-agent` propagates the current
selection from the interactive controller.

## Baseline / Authority Refs

- Approved user design: direct labels after the model (`max`, `high`, `on`),
  no `reason:` prefix, `Off` omitted, and no second thinking spinner.
- `crates/neo-ai/src/options.rs:129-156`: existing `ReasoningSelection` and
  `ReasoningEffort` contract.
- `crates/neo-tui/src/shell/state.rs:22-243`: current chrome state stores a
  boolean thinking indicator and owns `working_label`.
- `crates/neo-tui/src/transcript/chrome_render.rs:756-821`: current footer
  layout and single spinner rendering.
- `crates/neo-agent/src/modes/interactive/mod.rs:981-985`: current propagation
  point that discards selection detail.
- `docs/aegis/specs/2026-08-03-thinking-and-message-presentation-design.md` and
  its plan are unrelated transcript-presentation work and are not expanded by
  this slice.

## Compatibility Boundary

- Preserve footer ordering, permission/development/shell/model/cwd/git labels,
  right-side context/token usage, width truncation, and colors except for the
  static reasoning label.
- Preserve `working_label`, its spinner, shell status, MCP status, and streaming
  status exactly; do not add a second `thinking` activity indicator.
- Preserve session events, context cache prefixes, provider requests,
  historical records, and model-selection behavior.
- `ReasoningSelection::Off` remains the default and now produces no static
  reasoning label, matching the existing default visual state.
- Existing arbitrary non-empty `ReasoningEffort` strings remain displayable;
  the formatter must not hardcode only the built-in effort names.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: not applicable; strict test-first execution was not
  requested.
- Test posture: focused post-change regression tests for label mapping and
  footer rendering.
- Reason: the user approved a small, bounded UI behavior change rather than
  strict TDD.
- Verification: exact `neo-tui` library filters, package checks, formatting,
  and diff checks listed below.

## Requirement Ready Check

- Requirement source: approved conversation design and the current footer
  behavior.
- Goal: expose the selected reasoning value without confusing it with runtime
  activity.
- Acceptance:
  - Off has no reasoning token in the footer.
  - On renders `on`.
  - Effort renders its raw value, such as `high` or `max`.
  - Budget tokens render as `budget:<compact-count>`.
  - Streaming retains exactly one spinner/working status and never adds a
    separate static or dynamic `thinking` label.
- Decision: `ready`.

## Change Necessity

- User-visible need: the current boolean projection renders `thinking` for every
  enabled effort and is ambiguous beside the existing `working` spinner.
- No-change option: none; the current state has already discarded effort detail
  before the footer renderer receives it.
- Why code change is necessary: the complete `ReasoningSelection` must cross the
  controller-to-TUI state boundary and be formatted in the existing footer owner.
- Minimum change boundary: `neo-tui` chrome state/renderer and the existing
  `InteractiveController::set_current_reasoning` propagation method.
- Decision: `code-change`.

## Existence / Ownership Check

- Proposed new surface: none.
- Existing owner / reuse candidate: `NeoChromeState`, `render_footer_lines`,
  and `ReasoningSelection`.
- Why existing surface is sufficient: it already owns footer projection and the
  controller already owns the canonical selection.
- Decision: `reuse-existing`.

## Architecture Integrity Lens

- Invariant: static reasoning configuration and dynamic runtime activity remain
  separate footer concepts.
- Canonical owner: `ReasoningSelection` remains canonical; TUI derives only a
  display label.
- Responsibility overlap to avoid: do not make `render_footer_lines` infer
  effort from a model alias or create a second activity state.
- Higher-level simplification: replace the lossy boolean with the existing
  selection type and delete the obsolete `thinking_enabled` path.
- Retirement: remove the boolean field, getter, setter, and renderer branch;
  no compatibility alias is retained because this is an internal owner.
- Verdict: proceed with the existing owners.

## Plan-Time Complexity Check

- Target files: `crates/neo-tui/src/shell/state.rs`,
  `crates/neo-tui/src/transcript/chrome_render.rs`, and
  `crates/neo-agent/src/modes/interactive/mod.rs`.
- Existing pressure: `NeoChromeState` is already a large state container and
  `chrome_render` already owns footer composition.
- Add-in-place risk: adding a new footer-specific enum or status field would
  duplicate the reasoning contract and increase state entropy.
- Recommendation: edit-in-place with one existing type replacement and one
  focused formatter; do not extract or refactor unrelated chrome state.

## Tasks

### Task 1: Preserve the full reasoning selection in chrome state

Files:

- Modify `crates/neo-tui/src/shell/state.rs`.
- Modify `crates/neo-agent/src/modes/interactive/mod.rs`.

Steps:

1. Replace `NeoChromeState.thinking_enabled: bool` with a
   `neo_ai::ReasoningSelection`, initialized to `Off`.
2. Replace the boolean getter/setter with a selection setter and a read-only
   selection or display-label accessor using the existing state API style.
3. Update `InteractiveController::set_current_reasoning` to store the complete
   selection in the controller and pass a clone/moved value to the chrome state;
   remove the `is_enabled()` reduction.
4. Keep all callers and selection resolution unchanged.

Verification:

- `rtk cargo check -p neo-agent`
- The `neo-tui` library tests in Task 2 compile against the new state API.

### Task 2: Render direct effort labels and prove the single-status contract

Files:

- Modify `crates/neo-tui/src/transcript/chrome_render.rs`.
- Add focused unit tests in the existing owner file or its existing test owner;
  do not create a new test subsystem.

Steps:

1. Replace the static `thinking` branch with a conditional label derived from
   the stored `ReasoningSelection`:
   - `Off` -> no label;
   - `On` -> `on`;
   - `Effort { effort }` -> `effort.as_str()`;
   - `BudgetTokens { budget_tokens }` -> `budget:<count>` using the existing
     compact token-count formatter.
2. Render the label with the existing muted model/footer styling rather than
   the working-status style.
3. Leave the existing `working_label` spinner branch unchanged.
4. Add focused assertions for Off, On, an effort such as `max`, and a budget
   value. Add a streaming footer assertion that sees the effort label and one
   `working` spinner while not containing the obsolete static `thinking` token.
5. Preserve width truncation and right-side context usage behavior.

Verification:

- `rtk cargo nextest run -p neo-tui --lib footer_reasoning_selection`
- `rtk cargo nextest run -p neo-tui --lib footer_reasoning_does_not_duplicate_working_status`
- `rtk cargo fmt -p neo-tui -- --check`
- `rtk git diff --check -- crates/neo-tui/src/shell/state.rs crates/neo-tui/src/transcript/chrome_render.rs crates/neo-agent/src/modes/interactive/mod.rs`

## Risks

- The only meaningful risk is a stale caller of the removed boolean setter;
  `cargo check -p neo-agent` covers the controller-to-TUI propagation path.
- A longer effort label can reduce left-footer room, but the existing
  `truncate_to_width` path remains authoritative and the right-side context
  label is unchanged.
- Budget-token formatting must remain readable without introducing a second
  token-count owner; reuse the current compact formatter.

## Retirement

The obsolete `thinking_enabled` field, accessor, setter, and renderer branch
are deleted in Task 1/2. No compatibility carrier or fallback remains because
all callers are internal and the canonical `ReasoningSelection` is available at
the existing propagation boundary.

## Completion Evidence

Before completion, inspect the final diff and confirm only the three task-owned
source files and focused tests changed. Run the exact tests and checks above,
then commit one conventional change with a scoped message. Preserve all
pre-existing unrelated worktree modifications.
