# Task Browser UI Redesign Implementation Plan

## Goal

Implement the approved `/tasks` redesign as one quiet, dense task console:

- a general task list with a persistent wide inspector and single-page medium
  and small layouts;
- a dedicated Workflow workspace with split, stacked, and tabbed responsive
  layouts;
- readable complete commands, honestly labelled output previews, stable focus,
  selection, refresh, and pointer behavior;
- no change to Workflow execution, persistence, task control, model context, or
  Delegate-family transcript cards.

## Architecture

Keep the existing Task Browser as the only UI owner. `render.rs` owns visual
composition and one shared responsive geometry calculation; `state.rs` keeps
the existing keyed selection, focus, scroll, dialog, and action state;
`view.rs` carries only immutable projection fields already available from the
host; the existing interactive input route maps keys and pointer events into
Task Browser actions. Reuse `TuiTheme`, `wrap_text`, `truncate_width`, the
fullscreen overlay, task snapshots, periodic refresh, and current control
methods. Do not add a second layout engine, task model, output store, or UI
framework.

## Tech Stack

Rust 2024, `crossterm 0.29`, Neo's `neo-tui` primitives and fullscreen overlay,
the existing background-task/Workflow projections, `cargo nextest`, and exact
binary tests.

## Baseline/Authority Refs

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `docs/aegis/specs/2026-08-07-task-browser-ui-design.md`
- `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`
- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
- User approval on `2026-08-07`

## Compatibility Boundary

- `/tasks` remains the only background-task and Workflow operator surface.
- `BackgroundTaskManager`, `WorkflowRuntime`, and
  `WorkflowDefinitionRegistry` retain their current ownership.
- The TUI remains an immutable projection with ephemeral selection, focus,
  scrolling, answer drafts, and responsive geometry only.
- Existing filter, refresh, keyed selection, task control, save, answer,
  paging, and back-navigation behavior remains intact.
- Task Browser remains a fullscreen overlay; alternate-screen and mouse-capture
  lifecycle remain unchanged.
- Delegate, DelegateGroup, DelegateSwarm, Bash, Terminal, and Workflow
  transcript cards retain their logical content, grouping, expansion, and
  placement.
- Session JSONL, provider requests, compaction input, cache-prefix bytes,
  Workflow journal, scheduling, recovery, and results remain unchanged.
- The former `< 70` Workflow sequence `summary -> Steps -> Agents -> details`
  is retired. The only small-width path is a stable header with `[Steps |
  Agents]`, `Tab` to switch, and `Enter` to open Agent Details. No compatibility
  setting or fallback branch is allowed.

## TDD Route

- Mode: off
- Decision: skipped
- Strict authority: not applicable
- Test posture: minimum implementation followed by focused regression
- Reason: the project and user did not request strict test-first development;
  the approved behavior and existing owners are already explicit.
- Verification: one package, one explicit target, and one named filter per
  focused test command; `cargo nextest list` must show each filter after new
  tests are added.

## Verification

Each task has its own exact regression. Final verification repeats every
focused command, checks formatting and diffs, then records native platform
evidence separately from macOS-local evidence. A zero-test result is failure,
not evidence.

## Aegis Visibility

Planning is required because responsive layout, pointer hit testing, and the
retirement of the old narrow Workflow route must share one owner and one
verification path without drifting into runtime or transcript-card changes.

## Plan Basis

- Planning snapshot: `main` at `4c722d9558a7a553f22907d49669f3fef5b4cbce`.
- The worktree already contained an unrelated modification to
  `crates/neo-tui/tests/transcript_selection.rs`; it is not part of this plan
  and must never be staged, rewritten, or reverted by this work.
- Current code facts:
  - `crates/neo-tui/src/tasks_browser/render.rs` is the visual owner.
  - `state.rs` already exposes Tasks, Output, Steps, and Agents focus and all
    required open/back/control actions.
  - `view.rs` already carries task rows and Workflow rows; the host projection
    already has `generated_files`, while the TUI adapter currently drops it.
  - `crates/neo-agent/src/modes/task_browser.rs` already produces
    `detail_lines`, `preview_lines`, and explicit stdout/stderr truncation
    markers.
  - the current Workflow renderer already uses the `100` and `70` breakpoints;
    the small branch already switches Steps and Agents by focus but still
    renders an extra details region.
  - the fullscreen overlay occupies the complete rendered frame.

## Requirement Ready Check

- Requirement source refs: approved `2026-08-07` design and user approval.
- Goals and scope refs: Goal, Compatibility Boundary, and approved acceptance
  criteria.
- User/scenario refs: general task scanning, Workflow monitoring, complete
  command review, small-terminal navigation, live refresh, and pointer use.
- Requirement item refs: Tasks 1-4 below.
- Acceptance/verification refs: each task's Verification and Final Acceptance.
- Open blocker questions: none.
- Decision: ready.

## BaselineUsageDraft

- Required baseline refs: all records under Baseline/Authority Refs.
- Acknowledged before plan refs: all required records were viewed before this
  plan was written.
- Cited in plan refs: all required records are listed above and carried into
  the compatibility, retirement, and final verification sections.
- Missing refs: none.
- Decision: continue.

## Change Necessity

- User-visible need: the current general browser hides the inspector until
  opened, splits medium widths into narrow columns, uses pointer-only row
  selection treatment, silently cuts rendered lines, and does not provide the
  approved Workflow hierarchy at all breakpoints.
- No-change/non-code option: documentation cannot change terminal composition,
  focus, wrapping, or pointer routing.
- Why code change is necessary: the behavior is owned by production rendering,
  local view projection, and input routing.
- Minimum change boundary: current Task Browser render/state/view modules, the
  existing adapter and input path, one last-frame size value in `NeoTui`, and
  focused tests.
- Decision: code-change.

## Existence Check

- Proposed new surface: none.
- Existing owner/reuse candidate: Task Browser renderer/state/view, `NeoTui`
  fullscreen overlay, current adapter, current input route, `TuiTheme`, and
  `wrap_text`.
- Why existing surface is insufficient: the owners are correct; their current
  composition and pointer wiring do not implement the approved design.
- Creation proof: only a private geometry/hit-test helper inside `render.rs`
  and local action variants are justified so rendering and pointer input do not
  calculate breakpoints separately.
- Entropy/retirement impact: remove the old pane composition and small-width
  extra-details behavior; do not keep fallback rendering.
- Decision: reuse-existing.

## Architecture Integrity Lens

- Invariant: runtime snapshots remain authoritative; the TUI only presents and
  navigates them.
- Canonical owner: `render.rs` owns breakpoints and rectangles; `state.rs` owns
  keyed selection/focus; interactive input owns key/pointer delivery.
- Responsibility overlap: the adapter may copy existing `generated_files`, but
  it must not format layouts or parse Workflow journals.
- Higher-level simplification: one private geometry calculation feeds both
  rendering and hit testing; no separate pointer coordinate model.
- Retirement/falsifier: a second layout module, new runtime field, new task
  store, compatibility route, or transcript-card edit means the implementation
  has drifted.
- Verdict: proceed.

## Files

Primary implementation files:

- `crates/neo-tui/src/tasks_browser/render.rs`
- `crates/neo-tui/src/tasks_browser/state.rs`
- `crates/neo-tui/src/tasks_browser/view.rs`
- `crates/neo-tui/src/app.rs`
- `crates/neo-agent/src/modes/task_browser.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`

Focused test files:

- `crates/neo-tui/tests/task_browser.rs`
- `crates/neo-agent/src/modes/interactive/tests.rs`

Completion records after implementation evidence passes:

- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md` only if
  overlay behavior materially changes; otherwise leave it untouched.

Do not touch any other file unless current source evidence proves one of these
owners cannot satisfy an approved criterion. Stop and revise the plan before
crossing a runtime, persistence, transcript-card, context, or provider boundary.

## Complexity Budget

- Artifact class: source and maintained test complexity.
- Target files/artifacts: `render.rs` (451 lines), `state.rs` (1429 lines),
  `tests/task_browser.rs` (794 lines) at planning time; interactive tests are
  already a large shared owner.
- Current pressure: `state.rs` is above the strong threshold and the Task
  Browser integration test is at the soft threshold.
- Projected post-change pressure: at risk if layout logic enters `state.rs` or
  tests duplicate every cosmetic line.
- Budget result: within-budget only with render-first composition, local state
  wiring, and a few behavior-level assertions.
- Planned governance: keep geometry in `render.rs`; add no responsibility to
  `state.rs`; extend existing fixture builders and tests; create no generic UI
  abstraction or new dependency.

## Plan-Time Complexity Check

- Target files: the Files list above.
- Existing size/shape signals: `render.rs` is cohesive; `state.rs` is large but
  already owns selection; `tests/task_browser.rs` already owns all Task Browser
  behavior.
- Owner fit: responsive geometry and styling fit `render.rs`; stable-key
  selection fits `state.rs`; last rendered dimensions fit `NeoTui`.
- Add-in-place risk: only `state.rs` and the interactive test owner are high
  pressure.
- Better file boundary: none. A new layout file would create another owner for
  one screen; a new input test target would duplicate the controller harness.
- Recommendation: edit `render.rs` in place, use wiring-only/local-selection
  edits elsewhere, and avoid broad refactors.

## Plan Pressure Test

- Owner/retirement: one renderer and one small-width route; old composition is
  replaced rather than retained.
- Architecture integrity/higher-level path: current owners are sufficient; no
  runtime or task-model change is needed.
- Verification scope: boundary widths/heights, complete commands, preview
  labels, keyed refresh, controls, pointer hit testing, and frozen transcript
  cards all have named checks.
- Task executability: Tasks 1-3 share `render.rs` and therefore run serially;
  Task 4 follows fresh combined evidence.
- Pressure result: proceed.

## Execution Readiness View

- Intent Lock: implement exactly the approved character-art hierarchy; do not
  reopen product design.
- Scope Fence: Task Browser presentation, existing projection fields, local
  input wiring, focused tests, and final baseline amendment only.
- Baseline Lock: the five Task Browser/Workflow/fullscreen records listed above
  must be re-read before the first source edit.
- Approved Behavior: wide general inspector; medium/small full pages; wide
  split Workflow; medium stacked Workflow; small Steps/Agents tabs; complete
  commands; visible preview labels; stable refresh and controls.
- Owner Constraints: layout geometry lives once in `render.rs`; runtime and
  transcript cards are not owners.
- Compatibility Boundary: see the section above.
- Retirement Boundary: delete the old pane composition and do not retain the
  old small-width sequence or a setting for it.
- Task Batches: Tasks 1, 2, 3, then 4; strictly serial because the first three
  share files.
- Test Obligations: exact per-task regressions, final combined regressions,
  formatting, diff checks, and separately labelled native evidence.
- Review Gates: after each task, compare the diff against this plan, review
  correctness/cross-platform/complexity, rerun the exact filter, then commit
  only that task.
- Drift/Rewind Rules: stop before editing runtime, persistence, transcript-card,
  context, provider, theme-schema, or unrelated files; return to this plan if
  a second geometry owner or fallback appears.
- Evidence Required Before Completion: all exact tests discover and pass,
  width/height assertions pass, no frozen-card change, formatted diff, scoped
  commits, and native evidence reported honestly.
- Advisory Boundary: this view guides execution; it does not itself grant
  completion.

## Task 1: Rebuild the general Task Browser composition

### Files

- Modify: `crates/neo-tui/src/tasks_browser/render.rs`
- Modify tests: `crates/neo-tui/tests/task_browser.rs`

### Why

The general browser must become readable at all approved widths before the
Workflow screen can reuse the same visual grammar and geometry.

### Change Necessity

The current `render_browser` only shows details after opening and splits at
`>= 70`; the approved behavior requires an always-visible wide inspector and
full-width medium/small pages. The minimum owner is `render.rs` plus its
existing integration test.

### Impact/Compatibility

Do not change `TaskBrowserSnapshot`, filtering, selection, output scroll, stop,
or back actions. This task is rendering-only.

### Steps

1. In `render.rs`, import the existing `pad_to_width` and `wrap_text` primitives
   and define private `WIDE_MIN_COLUMNS = 100`, `MEDIUM_MIN_COLUMNS = 70`,
   `MIN_TASK_LIST_COLUMNS = 30`, and `MAX_TASK_LIST_COLUMNS = 42` constants.
2. Add one private layout value in `render.rs` that calculates header, content,
   footer, task list, inspector, Details, and Latest output rectangles for the
   supplied width/height. This same value will be reused by Task 3 hit testing;
   do not duplicate breakpoint arithmetic elsewhere.
3. Replace the current browser header with `TASKS`, visible `ALL`, `ACTIVE`, and
   `WORKFLOWS` choices, and the active filter's `snapshot.total_matched` count.
   Fall back to `visible_items().len()` only when `total_matched` is absent.
4. Rework task rows so status marker, human handle/task ID, title, and elapsed
   time have fixed priority. Use the existing status marker and matching theme
   status color. Paint the complete selected row with `selected_fg` and
   `selected_bg`; pad before painting so highlight width is stable.
5. At `< 70`, render a selected task in at most two stable lines and drop lower
   priority fields before Unicode-width truncation. At `>= 70`, keep one row and
   right-align elapsed time when it fits.
6. At `>= 100`, always render the `30-42` column list and the remaining-width
   inspector separated by one divider. The inspector follows selection even
   when `task_details_open` is false. Opening details or output only changes
   focus and scrolling, not geometry.
7. At `70-99` and `< 70`, render exactly one full-width page: list when details
   are closed, Details when open with Tasks focus, Latest output when focus is
   Output. `Esc` continues to return to list before closing.
8. Wrap Details and Latest output body lines with existing `wrap_text`; never
   silently replace an unseen command suffix with ellipsis. Keep viewport
   slicing after wrapping. Title the bounded output region `Latest output ·
   Preview`; when existing stdout/stderr truncation markers are present, keep
   them visible as continuation evidence.
9. Make focused section titles/dividers use `theme.brand`; ordinary structure
   uses `overlay_border`. Use only existing theme roles.
10. Extend `browser_opens_non_workflow_details_and_scrolls_its_output` to prove
    wide inspector visibility, medium Details/Output pages, `Esc` layering,
    wrapped long commands, and visible Preview text.
11. Add one boundary rendering test named
    `browser_renderer_matches_general_layout_at_supported_sizes` over widths
    `[32, 69, 70, 99, 100, 120, 180]` and heights `[12, 20, 40]`. Assert exact
    frame height, no line wider than the terminal, wide inspector presence,
    medium/small single-page behavior, and no overlap.

### Verification

```bash
cargo nextest list -p neo-tui --test task_browser browser_renderer_matches_general_layout_at_supported_sizes
cargo nextest run -p neo-tui --test task_browser browser_renderer_matches_general_layout_at_supported_sizes
cargo nextest run -p neo-tui --test task_browser browser_opens_non_workflow_details_and_scrolls_its_output
git diff --check
```

Commit after review and fresh verification:

```text
feat(tui): redesign task browser layout
```

## Task 2: Rebuild the dedicated Workflow workspace and Agent Details

### Files

- Modify: `crates/neo-tui/src/tasks_browser/render.rs`
- Modify locally: `crates/neo-tui/src/tasks_browser/state.rs`
- Modify: `crates/neo-tui/src/tasks_browser/view.rs`
- Modify: `crates/neo-agent/src/modes/task_browser.rs`
- Modify tests: `crates/neo-tui/tests/task_browser.rs`
- Modify adapter tests in: `crates/neo-agent/src/modes/task_browser.rs`

### Why

Workflow needs its approved Steps/Agents/Details hierarchy at each breakpoint,
with complete activity commands and useful agent facts, while keeping runtime
and durable projections unchanged.

### Change Necessity

The current renderer leaves a small Details pane under the narrow tab page,
uses boxed regions with no selected-row theme treatment, and drops existing
`generated_files` from the host row. The minimum change is the current renderer
and adapter/view wiring.

### Impact/Compatibility

Do not change `WorkflowChildRow`, `WorkflowRuntime`, journal data, live merge,
child order, paging, answer/save/control actions, or token accounting. Copy only
the already-existing `generated_files` field into the TUI view. Actual usage
remains absent from roster rows and visible only in Agent Details.

### Steps

1. Add `generated_files: Vec<String>` to `TaskBrowserWorkflowChild`; populate it
   from `WorkflowChildRow::generated_files` in `workflow_child_row`. Update the
   existing fixture constructors and adapter regression; add no runtime field.
2. Rework `workflow_header` into a stable identity/summary area containing
   display name, purpose, status label/marker, elapsed time, observed child
   counts, and Needs input when present. Continue hiding run IDs, revisions,
   hashes, scope, provider details, and predictions.
3. Render Steps in declared order and Agents in durable page order. Apply
   semantic status colors and full-row selected styling. The focused Steps or
   Agents title/divider uses `theme.brand`; the other uses `overlay_border`.
4. At `>= 100`, use the shared geometry to render Steps on the left, Agents on
   the right, and a lower selected-agent preview across the full width. Keep the
   left width clamped to `30-42` columns.
5. At `70-99`, stack summary, Steps, Agents, and compact selected-agent preview
   vertically with a fixed footer. Do not let content height overwrite the
   footer.
6. At `< 70`, render only the stable Workflow header, `[Steps | Agents]` tab
   selector, the active navigation page, and the contextual footer. `Tab`
   switches the visible page. Do not render the old summary page or a lower
   Details preview.
7. When `child_details_open` is true at any width, render the existing
   full-width Agent Details page and let `Esc` return to the responsive
   Workflow workspace. Include title, role, state, elapsed, wrapped current
   activity/command, terminal result/failure, generated files, and actual usage
   when present.
8. Wrap activity and command text with `wrap_text`. Reuse the existing
   `output_scroll` field for Agent Details while `child_details_open` is true:
   reset it on open/close, make PageUp/PageDown move it, slice only after
   wrapping, and render a visible continuation line when more rows remain.
   Do not add a second scroll field or silently ellipsize a command suffix.
9. Keep answer, save, replacement, stop confirmation, paging, and Needs input
   states on their existing state/action paths. Only update their visual
   placement to fit the responsive workspace.
10. Expand `workflow_renderer_fits_supported_widths_and_places_usage_in_details`
    to include width `69`, assert the exact split/stack/tab route at `69/70/99/100`,
    assert full-height Agent Details after Enter, assert usage/files stay out of
    the roster, and assert PageDown can reveal every wrapped segment of a long
    Bash/Terminal activity command.
11. Keep the existing keyed-selection, open/back, and control tests unchanged
    unless an assertion names retired visual text. Do not add cosmetic snapshot
    duplication.

### Verification

```bash
cargo nextest run -p neo-agent --bin neo workflow_child_row_displays_projected_live_and_durable_terminal_facts
cargo nextest run -p neo-tui --test task_browser workflow_renderer_fits_supported_widths_and_places_usage_in_details
cargo nextest run -p neo-tui --test task_browser workflow_opens_in_place_and_esc_returns_to_the_selected_task
cargo nextest run -p neo-tui --test task_browser workflow_steps_and_agents_keep_their_keyed_selection_after_a_refresh
git diff --check
```

Commit after review and fresh verification:

```text
feat(tui): redesign workflow task workspace
```

## Task 3: Route pointer input through the shared layout

### Files

- Modify: `crates/neo-tui/src/tasks_browser/render.rs`
- Modify: `crates/neo-tui/src/tasks_browser/state.rs`
- Modify: `crates/neo-tui/src/app.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Modify tests: `crates/neo-tui/tests/task_browser.rs`
- Modify tests: `crates/neo-agent/src/modes/interactive/tests.rs`

### Why

The approved UI requires clicks to select the row under the pointer and wheel
events to move the pane under the pointer. Current Task Browser input consumes
pointer events but only maps wheel direction to the already-focused selection.

### Change Necessity

Rendering alone cannot map physical rows to stable task/step/agent keys. The
minimum wiring is one last-rendered frame size, shared render geometry hit
testing, and local state actions.

### Impact/Compatibility

Task Browser remains the highest-priority pointer owner while open; pointer
events must not reach prompt history or transcript selection. Keyboard actions
and all external control methods remain unchanged.

### Steps

1. Add a `last_frame_size: Option<(usize, usize)>` field and read-only getter to
   `NeoTui`. Set it at the start of every `render_frame` call before the
   fullscreen-overlay early return. Do not alter `last_layout`, transcript
   region routing, or terminal lifecycle.
2. In `render.rs`, expose a small Task Browser pointer-to-action function that
   consumes the same private geometry used by rendering. It may return only
   existing/local Task Browser actions; do not expose rectangle math to
   `neo-agent`.
3. Add local action variants for selecting a visible task row, Workflow step,
   and Workflow agent by index, plus moving the pointed Workflow pane for wheel
   input. `state.rs` must immediately translate indices to existing stable IDs
   or keys and reuse current reconciliation/child-refresh logic. Do not store
   row numbers across refreshes.
4. Map a primary-button press to the row under the pointer. Selecting a Workflow
   step sets Steps focus and requests the existing child-page refresh; selecting
   an agent sets Agents focus. Presses on headers, dividers, blank rows, or the
   footer are no-ops.
5. Map wheel events by pointer region: task list moves task selection; Steps
   moves step selection; Agents moves agent selection; general Output changes
   the existing output scroll. Pointer column chooses the pane at wide widths;
   pointer row chooses the stacked pane at medium widths.
6. Keep drag/release events consumed by Task Browser without changing selection
   or reaching the transcript. Keep answer/save dialogs' current keyboard and
   wheel ownership.
7. Add a pure renderer/state test named
   `browser_pointer_hit_testing_uses_rendered_regions_and_stable_keys` covering
   wide columns, medium stacked rows, small tabs, blank/footer no-op, and keyed
   selection after refresh.
8. Add a controller regression named
   `task_browser_mouse_click_selects_rows_and_wheel_uses_pointed_pane`. Render a
   known `120x24` frame first, send typed mouse events at rows found from that
   frame, then assert the selected task/step/agent changes while prompt history
   and transcript selection do not.

### Verification

```bash
cargo nextest list -p neo-tui --test task_browser browser_pointer_hit_testing_uses_rendered_regions_and_stable_keys
cargo nextest run -p neo-tui --test task_browser browser_pointer_hit_testing_uses_rendered_regions_and_stable_keys
cargo nextest list -p neo-agent --bin neo task_browser_mouse_click_selects_rows_and_wheel_uses_pointed_pane
cargo nextest run -p neo-agent --bin neo task_browser_mouse_click_selects_rows_and_wheel_uses_pointed_pane
cargo nextest run -p neo-agent --bin neo task_browser_mouse_wheel_moves_selection_without_prompt_history
git diff --check
```

Commit after review and fresh verification:

```text
feat(tui): add task browser pointer navigation
```

## Task 4: Run combined verification and synchronize active records

### Files

- Test-only changes only when a real uncovered regression requires them.
- Amend after all evidence passes:
  `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- Amend after all evidence passes:
  `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
- Amend `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
  only if the implementation changed fullscreen overlay behavior beyond
  recording the last rendered size.

### Why

The three implementation tasks share one screen and must be verified together
before active Workflow records stop pointing at the retired narrow sequence.

### Change Necessity

Code changes need combined evidence; the active ADR/baseline text must then
name the current tabbed route. No new ADR is needed because no runtime or
ownership decision changes.

### Impact/Compatibility

Record only verified visual/responsive behavior. Do not rewrite historical
designs or claim native evidence that was not executed.

### Steps

1. Run every exact Task 1-3 command again from the final source state. Confirm
   each filter appears in `cargo nextest list` and each run executes at least
   one test.
2. Run the unchanged behavioral guards:

```bash
cargo nextest run -p neo-tui --test task_browser browser_keeps_keyed_task_selection_and_filters_active_tasks
cargo nextest run -p neo-tui --test task_browser workflow_pause_stop_and_save_actions_only_emit_valid_intents
cargo nextest run -p neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged
```

3. Run source formatting and diff checks:

```bash
rustfmt --check --edition 2024 crates/neo-tui/src/tasks_browser/render.rs crates/neo-tui/src/tasks_browser/state.rs crates/neo-tui/src/tasks_browser/view.rs crates/neo-tui/src/app.rs crates/neo-agent/src/modes/task_browser.rs crates/neo-agent/src/modes/interactive/input.rs
git diff --check
```

4. On macOS, render/operate real terminal sizes `32x12`, `69x20`, `70x20`,
   `99x20`, `100x20`, `120x40`, and `180x40`. Verify no overlap, stable
   selection, correct page transitions, pointer selection, wheel ownership,
   wrapped long commands, visible Preview labels, and usable Needs input/save/
   stop states.
5. Native evidence is a separate slice. Before using Parallels, check host
   memory, boot at most one VM, run the same focused Cargo commands in a clean
   checkout of the exact implementation commit, record the commit and output,
   and shut down that VM before starting another. Fedora and Windows results
   must be labelled native; skipped graphical-terminal pointer checks remain
   explicit residual risk.
6. Append a narrow UI/responsive amendment to ADR-0008 and the 2026-07-28
   baseline: wide split, medium stack, small Steps/Agents tabs, complete command
   access, visible previews, unchanged runtime/card ownership, and retirement
   of the sequential small route. Do not rewrite history.
7. Run `git diff --check`, review only this task's docs, and commit the verified
   record update.

Commit after review and fresh verification:

```text
docs: record task browser redesign
```

## Execution Route

- Decision: inline
- Evidence: Tasks 1-3 share `render.rs` and are dependency ordered; independent
  implementation agents would create overlapping writes and re-read the same
  owner.
- Fallback: if an executing environment requires delegation, use one fresh
  implementation agent at a time and keep the coordinator as the only Git
  writer; never parallelize source writes.
- User confirmation required: no. The user already approved the design, but
  this plan/handoff turn stops before execution as explicitly requested.

## Risks

- ANSI selected-row styling must preserve display width and reset styles before
  dividers.
- Unicode wrapping must not split wide graphemes or let a long word overwrite
  the footer; reuse `wrap_text` and existing width utilities.
- Last-frame hit testing can drift if rendering and pointer geometry diverge;
  both must call the same private layout calculation.
- Short heights can starve stacked Workflow details; fixed header/footer and
  active navigation remain higher priority than optional preview rows.
- Adapter changes must copy only existing immutable fields and never reach into
  runtime/journal logic.
- Native pointer and clipboard behavior cannot be inferred from macOS unit
  tests or synthetic events.

## Retirement

- Replace the current `>= 70` general split with `>= 100` persistent inspector
  and medium/small full pages.
- Remove the small Workflow branch's always-present lower Details pane.
- Retire the sequential small Workflow route from active ADR/baseline text.
- Do not preserve old rendering behind a setting, alias, feature flag, fallback,
  or duplicate helper.
- Do not add search, sorting, metrics, manual refresh, new theme fields, a new
  task model, or a new output store.

## Final Acceptance

Completion requires all of the following:

- general rendering matches the approved hierarchy at widths `32`, `69`, `70`,
  `99`, `100`, `120`, `180` and heights `12`, `20`, `40`;
- Workflow uses split wide, stacked medium, and tabbed small layouts with no
  sequential compatibility path;
- selected/focused/status states are visible with every theme and without color
  as the only signal;
- complete Bash/Terminal commands remain reachable through wrapping/scrolling,
  and bounded output is visibly a Preview;
- key and pointer input, filter, refresh, stable selection, output scroll,
  pause/resume, stop, save, answer, paging, and back navigation retain their
  existing semantics;
- frozen Delegate-family card regression remains unchanged;
- no runtime, journal, session-context, provider, compaction, cache-prefix, or
  transcript-card file is modified;
- every task has one scoped commit, no unrelated file is staged, and no push,
  release, branch, or worktree operation occurs without separate authority;
- macOS-local, Fedora-native, Windows-native, graphical-terminal, and skipped
  evidence are reported as distinct categories with residual risk.
