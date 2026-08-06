# Neo Task Browser UI Design

Status: `draft-user-review`
Date: `2026-08-07`
Approved direction: the user approved the task-browser and Workflow character
mockups on `2026-08-07`; this document fixes the exact behavior and boundaries
for written review before implementation planning.

## 1. Summary

Redesign `/tasks` as a quiet, dense task console:

- the general browser uses a stable task list plus a readable task inspector;
- selected rows, status, elapsed time, details, and output form one clear visual
  hierarchy;
- Workflow keeps its dedicated Steps, Agents, and Details information model;
- narrow terminals use one column without overlapping or hiding controls;
- the existing theme, task snapshots, refresh path, controls, and runtime owners
  remain authoritative.

This is a presentation redesign. It does not create another dashboard, task
model, Workflow projection, output store, or transcript card implementation.

## 2. Design Thesis

### Visual thesis

An operational console with restrained borders, one focus accent, semantic
status colors, stable columns, and no card mosaic.

### Content plan

1. Header: page identity, current filter, accurate task count.
2. Primary workspace: task list and selected-task inspector.
3. Workflow workspace: summary, Steps, Agents, selected-agent details.
4. Footer: only actions valid in the current focus and state.

### Interaction thesis

- selection changes color and content, never geometry;
- live refresh updates rows in place and preserves selection and scroll;
- output follows while unlocked, while manual scrolling keeps the current view
  and exposes newer activity without jumping.

There is no ornamental animation, duplicate spinner, pulsing border, or moving
progress decoration.

## 3. Goals

1. Let a user identify running, waiting, failed, paused, and completed work in
   one scan.
2. Make the selected task's purpose, state, elapsed time, details, and latest
   output readable without decorative nesting.
3. Make Workflow steps and agents visibly related while keeping their existing
   navigation and control semantics.
4. Keep issued shell commands viewable. A bounded preview must identify itself
   as a preview; it must never imply that silently cut content is complete.
5. Keep the interface usable at widths `32`, `69`, `70`, `99`, `100`, `120`,
   and `180`, and heights `12`, `20`, and `40`.

## 4. Non-goals

- no Workflow runtime, journal, scheduling, recovery, or task-control changes;
- no new fields in durable task or Workflow records solely for decoration;
- no redesign of Delegate, DelegateGroup, DelegateSwarm, Bash, Terminal, or
  Workflow transcript cards;
- no new theme settings or user configuration;
- no search, sorting editor, bulk actions, metrics dashboard, or manual refresh;
- no compatibility layout or setting that preserves both old and new narrow
  Workflow navigation.

## 5. Baseline Alignment

The design preserves these current boundaries:

- `/tasks` remains the only background-task and Workflow operator surface;
- `BackgroundTaskManager` remains the task lookup and control adapter;
- `WorkflowRuntime` remains the durable Workflow lifecycle owner;
- the TUI renders immutable projections and owns selection, focus, scrolling,
  answer drafts, and responsive layout only;
- Task Browser remains an overlay inside the fullscreen session;
- alternate-screen mouse capture and input priority remain unchanged;
- transcript cards remain unchanged.

This design replaces only the visual examples and narrow-layout requirement in
`docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`. Its former
small-width sequential route:

```text
Workflow summary -> Steps -> Agents -> Agent details
```

is retired. The single current small-width route is:

```text
Workflow header -> [Steps | Agents] -> Agent details
```

`Tab` switches Steps and Agents. `Enter` opens the selected agent details.
There is no compatibility branch for the retired sequential route.

## 6. Shared Visual Language

### 6.1 Theme roles

Reuse the existing `TuiTheme` roles:

- `selected_fg` and `selected_bg` for the selected row;
- `brand` for the focused divider or title;
- `text_primary` for task identity and active content;
- `text_muted` for secondary metadata;
- existing `status_ok`, `status_error`, `status_warn`, `status_pending`, and
  `status_cancelled` roles for states;
- `overlay_border` for ordinary structure.

No literal palette values and no new theme fields are introduced.

### 6.2 Status communication

Reuse the existing status markers and labels. Color is never the only signal.
Every status remains distinguishable in monochrome output through its marker
and plain-language label.

### 6.3 Structure

- one outer screen frame;
- plain columns and horizontal dividers inside it;
- no nested cards;
- no border around every row;
- headers and footers have fixed height;
- labels, counters, and dynamic output cannot resize neighboring regions.

## 7. General Task Browser

### 7.1 Header

The header contains:

- `TASKS`;
- the existing `ALL`, `ACTIVE`, and `WORKFLOWS` filter choices;
- the accurate `total_matched` count for the active filter.

Do not show counts for inactive filters unless the current projection already
provides them. Do not calculate misleading counts from one paged result.

### 7.2 Task rows

Each row contains, in priority order:

1. selection treatment;
2. status marker;
3. human handle, falling back to task ID;
4. title;
5. elapsed time aligned to the right when width permits.

The selected row uses `selected_fg` and `selected_bg` across the full available
row. It does not depend on a `>` character alone. Live status or elapsed-time
changes do not move the selected row or change pane width.

### 7.3 Wide layout: `>= 100` columns

The list occupies roughly one third of the screen, clamped to `30-42` columns.
The inspector owns the remaining width. A single divider separates them.

The inspector is always visible and follows selection. It contains:

- status, task handle, title, kind, and elapsed time;
- existing `detail_lines` under a Details heading;
- existing `preview_lines` under a Latest output heading;
- an explicit preview or continuation indicator whenever the projection knows
  that more output exists.

`Enter` on a Workflow opens its dedicated operator. `Enter` on another task
moves into the existing detail-reading state. `O` focuses output. `Esc` first
returns focus to the list, then closes the browser.

```text
+- TASKS ------------------------------------------------------------------------+
|  [ALL]   ACTIVE   WORKFLOWS                                  12 tasks          |
+-----------------------------+--------------------------------------------------+
| TASKS                   AGE | RUNNING  #wf-27                           03:18   |
|                             | Release readiness                                 |
|  ● #wf-27 Release       3m  |                                                   |
|  ! #q-08  Choose target 42s | DETAILS                                           |
|  ○ #b-31  Build Windows 18s | Validate release candidates on every platform.    |
|  ✓ #d-19  Review parser 8m  |                                                   |
|  × #b-17  Package Linux 11m +-- LATEST OUTPUT ---------------------------------+
|                             | $ cargo nextest run -p neo-agent --bin neo ...    |
|                             | PASS task_browser_navigation          Preview 4/18|
+-----------------------------+--------------------------------------------------+
| Up/Down select  Tab filter  Enter open  O output  X stop  Esc close            |
+--------------------------------------------------------------------------------+
```

### 7.4 Medium layout: `70-99` columns

Use one full-width page at a time:

- the list is the default page;
- `Enter` opens full-width details;
- `O` opens full-width output;
- `Esc` returns to the list before closing the browser.

This avoids two narrow columns and keeps long task names readable.

### 7.5 Small layout: `< 70` columns

Keep the same one-page route as the medium layout. A task row may use two
stable lines:

```text
selected marker + status + handle
title + elapsed
```

The longest word is Unicode-width truncated only after the lower-priority
fields have been removed. Footer actions are contextual; hidden actions remain
reachable from their owning page rather than being silently unavailable.

## 8. Workflow Operator

### 8.1 Header

The header retains the existing information model:

- display name and plain-language purpose;
- macro state and elapsed time;
- completed, working, and queued child counts;
- a prominent Needs input state when applicable.

It does not expose run IDs, revisions, hashes, journal sequence, provider wire
details, or predicted completion percentages.

### 8.2 Wide layout: `>= 100` columns

The top workspace keeps Steps on the left and Agents on the right. The lower
workspace shows the selected agent's current activity, recent tools, and latest
output. These are divided regions, not independent cards.

```text
+- WORKFLOW / RELEASE READINESS --------------------------------------------------+
| RUNNING · 03:18       2 done · 1 working · 2 queued       NEEDS INPUT           |
+-----------------------------+--------------------------------------------------+
| STEPS                       | AGENTS                                            |
| ✓ 1 Prepare      2 · 0 · 0 | ✓ planner      lead     00:38  Plan completed     |
| ● 2 Verify       1 · 1 · 1 | ● linux-check  worker   01:11  Running tests      |
| ○ 3 Publish      0 · 0 · 2 | ○ windows      worker      --  Waiting            |
+- SELECTED AGENT / LINUX-CHECK --------------------------------------------------+
| CURRENT ACTIVITY                                                                |
| Using Bash · running · 01:11                                                    |
| $ cargo nextest run -p neo-agent --bin neo                                     |
|   task_browser_periodic_refresh_updates_open_browser                            |
| LATEST OUTPUT                                                Preview 6/18       |
+--------------------------------------------------------------------------------+
| Tab switch  Enter details  P pause  X stop  S save  Esc back                   |
+--------------------------------------------------------------------------------+
```

Steps remain in declared order. Agents remain in durable creation order. Live
refresh never reorders rows under the cursor.

### 8.3 Medium layout: `70-99` columns

Keep the current stacked hierarchy:

1. Workflow summary;
2. Steps;
3. Agents for the selected step;
4. compact selected-agent preview;
5. contextual footer.

`Tab` switches focus between Steps and Agents. Each region scrolls without
moving the footer.

### 8.4 Small layout: `< 70` columns

Show one navigation region under a stable Workflow header:

```text
+- WORKFLOW --------------------------+
| Release readiness             03:18 |
| RUNNING · NEEDS INPUT                |
| [STEPS]   AGENTS                     |
+--------------------------------------+
| ✓ 1 Prepare               2 · 0 · 0 |
| ● 2 Verify                1 · 1 · 1 |
| ○ 3 Publish               0 · 0 · 2 |
+--------------------------------------+
| Tab switch  Enter open  Esc back     |
+--------------------------------------+
```

`Tab` switches the visible Steps or Agents page. `Enter` on an agent opens the
existing full-width Agent Details page. `Esc` returns through details, Workflow,
and the general task list in that order.

### 8.5 Needs input

Existing answer-dialog behavior remains unchanged:

- a newly selected waiting Workflow opens its answer dialog automatically;
- dismissing it leaves a persistent Needs input banner;
- the same dismissed request does not reopen on every refresh;
- a new request, explicit answer action, or reopening the Workflow may open it;
- unsupported schema shapes continue through the existing structured editor;
- no secret input is accepted.

### 8.6 Commands and output

Agent Details continues to use the existing compact child-activity projection.
For an issued Bash or Terminal command:

- wrap the full command across terminal rows when shown in Details;
- do not replace the unseen suffix with an unlabeled ellipsis;
- if height prevents showing all activity, expose scrolling and a visible
  continuation indicator;
- label bounded output as a preview and retain the existing route to complete
  task output;
- do not embed or recreate transcript Delegate-family cards.

## 9. Input and Refresh Behavior

- `Up/Down` moves selection in the focused list or pane.
- `Tab` changes the general filter, or switches Workflow Steps and Agents.
- `Enter`, `O`, `P`, `X`, `S`, and `Esc` retain their existing contextual
  meanings unless the responsive rules above narrow their visible page.
- Mouse click selects the row under the pointer.
- Mouse wheel scrolls the pane under the pointer and never reaches prompt
  history while Task Browser is open.
- Periodic refresh keeps the selected task, selected step, selected agent,
  manual step pin, and scroll position whenever the referenced item still
  exists.
- If the selected item disappears, selection moves through the existing
  reconciliation path rather than leaving an empty active focus.

## 10. Empty, Loading, and Failure States

- Empty list: show `No tasks.` in the list workspace; do not render an empty
  inspector as another box.
- No selected item: show `No task selected.` in the inspector.
- No Workflow steps or agents: retain the existing plain empty-state messages.
- Refresh failure: keep the last valid snapshot and show the existing footer
  message; do not blank the screen.
- Stop/save/answer confirmation: keep the existing blocking focus and replace
  only the contextual footer content.

## 11. Ownership and Implementation Boundary

The existing owners remain:

- `crates/neo-tui/src/tasks_browser/render.rs`: composition and visual styling;
- `crates/neo-tui/src/tasks_browser/state.rs`: existing selection, focus,
  scrolling, filters, and responsive state;
- `crates/neo-tui/src/tasks_browser/view.rs`: current task and Workflow view
  structures;
- existing interactive input routing: key and mouse delivery;
- existing shell overlay: available rectangle and alternate-screen lifecycle.

Implementation should be render-first. `state.rs` is already a high-pressure
file; it may receive only local reuse or wiring needed by the approved layout.
No new presentation responsibility belongs in runtime or background-task code.

## 12. Alternatives Considered

### Full-width list with modal details

Rejected. It preserves maximum row width but makes repeated comparison slow and
hides live context while navigating.

### Dense fixed-column table

Rejected. It scans well at one width but degrades long handles, descriptions,
Workflow activity, and narrow terminals.

### Selected design: list plus inspector, dedicated Workflow workspace

Chosen. It reuses the current information and owners, improves scanning, keeps
Workflow depth, and has a simple single-column fallback.

## 13. Acceptance Criteria

1. General task rendering matches the documented hierarchy at widths `32`,
   `69`, `70`, `99`, `100`, `120`, and `180`.
2. Workflow rendering matches the tabbed small layout, stacked medium layout,
   and split wide layout at the same boundary widths.
3. At heights `12`, `20`, and `40`, no header, row, output, dialog, or footer
   overlaps another region.
4. Every status is identifiable without color; selected and focused states are
   visually distinct under every built-in and custom theme.
5. Long Unicode task handles, titles, purposes, activities, and output do not
   resize stable regions or overwrite controls.
6. Issued Bash and Terminal commands are fully viewable through wrapping or
   scrolling; truncation is never silent.
7. A bounded output region is visibly a preview and does not imply completeness.
8. Filter, selection, scrolling, mouse-wheel ownership, periodic refresh, stop,
   pause/resume, save, answer, and back navigation retain current behavior.
9. Workflow selection and Agent Details retain durable order and existing live
   activity enrichment.
10. Delegate, DelegateGroup, DelegateSwarm, Bash, Terminal, and Workflow
    transcript cards retain byte-for-byte-equivalent logical content and
    unchanged expansion behavior.
11. No Workflow journal, runtime, scheduler, task-control, session context,
    provider request, compaction, or cache-prefix behavior changes.
12. Focused rendering and interaction tests use one package, one explicit target,
    and named filters; local evidence is not reported as native cross-platform
    evidence.

## 14. Decision and Complexity Record

### TaskStartSnapshot

- Objective: beautify the general `/tasks` browser and its dedicated Workflow
  operator using the approved character-art direction.
- Success evidence: responsive render assertions, focused input behavior, and
  unchanged transcript-card regressions.
- Stop condition: written design reviewed, then a separate approved plan and
  verified implementation.
- Scope: Task Browser presentation and directly required local interaction.
- Existing worktree: unrelated modifications are preserved and excluded from
  this task's commit.

### TaskIntentDraft

- Outcome: one coherent, readable task console across general and Workflow
  tasks.
- Non-goals: runtime changes, new data owners, new settings, or card redesign.
- Main risk: changing responsive navigation while calling the work visual-only.
- Control: document the single small-width route and verify every boundary.

### BaselineUsageDraft

- Required baseline refs:
  `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`,
  `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`, and
  `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`.
- Cited in design refs: the three records above plus
  `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`.
- Missing refs: none.
- Decision: continue.

### Requirement Ready Check

- Requirement source: user request and approved character mockups on
  `2026-08-07`.
- Goals, scenarios, visual hierarchy, responsive behavior, invariants, and
  acceptance checks: specified above.
- Open blocker questions: none for design review.
- Decision: ready.

### ImpactStatementDraft

- Affected layer: Task Browser rendering and directly required view state.
- Preserved layers: runtime, persistence, task projection, session context,
  transcript cards, and provider behavior.
- Compatibility: one new layout only; the former sequential small-width
  Workflow requirement is retired.

### Existence Check

- Proposed new runtime or UI owner: none.
- Existing reuse candidate: current Task Browser renderer, state, view models,
  theme, overlay, and input path.
- Decision: reuse existing.

### Product Risk Lens

- Value: faster scanning, clearer focus, readable Workflow activity.
- Trade-off: the wide browser spends about one third of its width on the task
  list, while medium terminals move details to another page.
- Decision: accepted by the approved mockups.

### Architecture Integrity Lens

- Invariant: the TUI is a projection and never becomes a task or Workflow
  lifecycle owner.
- Responsibility overlap: none added.
- Retirement: old small-width sequential design text is replaced, not retained
  as a fallback.
- Verdict: aligned.

### Complexity Budget

- Artifact class: source and test complexity.
- Likely targets: `render.rs` (`451` lines), `state.rs` (`1429` lines), and
  `tests/task_browser.rs` (`794` lines) before implementation.
- Current pressure: `state.rs` is over the strong pressure threshold; the test
  target is near the soft threshold.
- Projected result: within budget only if work is render-first and state changes
  remain local reuse or wiring.
- Governance: do not add a second layout owner or generic abstraction; add only
  focused behavior tests that would fail on a real visual or navigation
  regression.

## 15. ADR and Baseline Signal

No new architecture owner or runtime decision is introduced, so no new ADR is
needed at design time. After implementation and verification, the existing
Workflow product-surface ADR and landed baseline should receive a narrow visual
and responsive-layout amendment so they no longer point at the retired
small-width sequence.
