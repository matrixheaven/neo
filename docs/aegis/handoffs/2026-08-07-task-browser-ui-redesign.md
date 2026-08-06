# Handoff: Implement the Approved Task Browser UI Redesign

Give everything below the divider to the implementation AI unchanged.

---

You are the coordinating implementation agent for Neo's approved `/tasks` and
Workflow UI redesign. Working directory:

```text
/Users/chenyuanhao/Workspace/neo
```

The user approved the design on `2026-08-07`. Implement the following plan
exactly, verify each task, and create the scoped local commits described there:

```text
docs/aegis/plans/2026-08-07-task-browser-ui-redesign.md
```

Do not reopen product design. Do not repeat broad repository exploration. The
owner map, breakpoints, interaction path, retirement boundary, tests, and
implementation order are already fixed.

## 1. Read Before Editing

Read in this order:

1. `AGENTS.md`
2. `~/.codex/RTK.md`
3. `~/.codex/CX.md`
4. `docs/aegis/specs/2026-08-07-task-browser-ui-design.md`
5. `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`, sections
   11-18 and 24-32
6. `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`
7. `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
8. `docs/aegis/baseline/2026-08-04-fullscreen-transcript-document.md`
9. `docs/aegis/plans/2026-08-07-task-browser-ui-redesign.md`
10. this handoff

Then run:

```bash
icm recall-context "task browser tasks workflow UI responsive preview pointer" --limit 5
git status --short --branch
git log -5 --oneline
```

Planning snapshot, for history only:

```text
branch: main
HEAD: 4c722d9558a7a553f22907d49669f3fef5b4cbce
remote relation: ahead 6
pre-existing unrelated modification:
  crates/neo-tui/tests/transcript_selection.rs
```

Use the actual execution-time HEAD and worktree. Preserve every user/other-agent
change. The pre-existing transcript-selection modification is not part of this
work and must never be staged, rewritten, or reverted.

## 2. Frozen Design

General Task Browser:

- `>= 100`: task list plus persistent inspector; list width `30-42` columns.
- `70-99`: one full-width page at a time: list, Details, or Latest output.
- `< 70`: same route as medium; task rows may use at most two stable lines.
- selected row uses the full existing selected foreground/background style;
  status retains marker plus semantic color; elapsed stays geometrically stable.
- output is explicitly labelled Preview; commands wrap/scroll and are never
  silently cut.

Workflow workspace:

- `>= 100`: Steps left, Agents right, selected-agent preview below.
- `70-99`: summary, Steps, Agents, and selected-agent preview stacked.
- `< 70`: stable header and `[Steps | Agents]`; `Tab` switches; `Enter` opens
  full-width Agent Details; `Esc` returns through Details, Workflow, tasks.
- the old sequential small route is retired. Do not keep a fallback, setting,
  alias, feature flag, or second renderer.

All widths use one outer screen structure, plain dividers, fixed header/footer,
existing theme roles, stable dimensions, and contextual actions. No card mosaic,
animation, second spinner, new palette, or new theme configuration.

## 3. Confirmed Owners

- visual composition and responsive geometry:
  `crates/neo-tui/src/tasks_browser/render.rs`
- keyed selection, focus, scroll, dialogs, and local actions:
  `crates/neo-tui/src/tasks_browser/state.rs`
- immutable TUI view rows:
  `crates/neo-tui/src/tasks_browser/view.rs`
- host-to-TUI mapping only:
  `crates/neo-agent/src/modes/task_browser.rs`
- last rendered dimensions only:
  `crates/neo-tui/src/app.rs`
- key/pointer delivery only:
  `crates/neo-agent/src/modes/interactive/input.rs`
- focused rendering/state tests:
  `crates/neo-tui/tests/task_browser.rs`
- controller input tests:
  `crates/neo-agent/src/modes/interactive/tests.rs`

Use one private responsive geometry calculation in `render.rs` for both drawing
and pointer hit testing. No other file may calculate the breakpoints or pane
rectangles independently.

## 4. Existing Code Facts; Do Not Rediscover Them

1. `TaskBrowserFocus` already has Tasks, Output, Steps, and Agents.
2. `TaskBrowserAction` and `handle_action` already own open/back, output focus,
   Steps/Agents switching, Agent Details, pause/resume, stop, save, answer, and
   child paging.
3. The current Workflow renderer already branches at `100` and `70`; the small
   branch already switches Steps/Agents by focus but incorrectly keeps a lower
   Details pane.
4. General `render_browser` currently splits only after details are opened and
   at `>= 70`; this must be replaced.
5. `snapshot_to_item` already supplies `detail_lines` and `preview_lines`.
6. `CommandOutput` already exposes stdout/stderr truncation booleans, and
   `append_stream_lines` already emits truncation markers. Do not add another
   output completeness model.
7. `WorkflowChildRow` already owns `generated_files`; the TUI adapter currently
   drops it. Copy it through; do not modify runtime or journal types.
8. `NeoTui::render_frame` knows the exact full-screen width/height but its
   fullscreen-overlay early return does not save them. Store only this last
   frame size for pointer hit testing; do not replace transcript `last_layout`.
9. Task Browser is a full-screen overlay and already consumes pointer input
   before prompt/transcript routing.
10. Existing exact tests are listed in the plan and were confirmed by
    `cargo nextest list`. New filters must be listed after they are added; zero
    tests is failure.

## 5. Prohibited Changes

- Do not modify Workflow runtime, registry, launch, journal, scheduling,
  recovery, retention, task control, or durable record types.
- Do not modify model context, system prompts, provider requests, compaction,
  cache prefix, session JSONL, or event order.
- Do not modify Delegate, DelegateGroup, DelegateSwarm, Bash, Terminal, or
  Workflow transcript card bodies, layout, grouping, expansion, or placement.
- Do not add a second task model, renderer, layout engine, state machine,
  viewport, output store, theme field, dependency, setting, or compatibility
  branch.
- Do not implement search, sort editing, bulk actions, metrics, manual refresh,
  jump-to-transcript, predictive progress, or decorative animation.
- Do not refactor `state.rs`, `app.rs`, or interactive input beyond the exact
  local wiring in the plan.
- Do not fix unrelated failures or formatting.
- Do not use `reset`, `checkout --`, `restore`, `stash`, `clean`, `rebase`,
  `git rm`, amend, force push, branch switching, or worktree add/remove.
- Do not push, publish, release, create a branch, or create/remove a worktree
  without separate user authority.

## 6. Fixed Execution Order

Run serially because Tasks 1-3 share `render.rs`:

1. General Task Browser composition.
2. Workflow workspace and Agent Details projection.
3. Pointer hit testing and input routing.
4. Combined verification and active record amendment.

Do not parallelize source writes. The approved execution route is inline via
`aegis:executing-plans`. If the environment mandates subagents, use only one
implementation agent at a time and keep the coordinator as the only Git writer.

## 7. Per-Task Protocol

Before each task, record:

```text
TaskStartSnapshot
- current branch and HEAD
- staged, unstaged, and untracked paths
- exact files this task may modify
- baseline commit for the task
- how overlapping user changes will be preserved
```

Then:

```text
apply only the current task
-> compare the diff against every task step and frozen boundary
-> review correctness, Unicode width, cross-platform input, and complexity
-> run cargo nextest list for any new filter
-> run only the task's exact tests
-> run git diff --check
-> stage only current-task files
-> commit with the plan's message
-> git show --stat --oneline HEAD
-> git status --short --branch
-> update the long-task checkpoint
```

An open review issue blocks the next task. A test failure outside the task is
reported and left untouched. If the plan is wrong, return `BLOCKED` with exact
evidence; do not invent a new architecture.

## 8. Test Obligations

Run the exact commands in each task. Final guards must include:

```bash
cargo nextest run -p neo-tui --test task_browser browser_keeps_keyed_task_selection_and_filters_active_tasks
cargo nextest run -p neo-tui --test task_browser workflow_pause_stop_and_save_actions_only_emit_valid_intents
cargo nextest run -p neo-tui --test workflow_transcript non_workflow_delegate_family_cards_remain_unchanged
rustfmt --check --edition 2024 crates/neo-tui/src/tasks_browser/render.rs crates/neo-tui/src/tasks_browser/state.rs crates/neo-tui/src/tasks_browser/view.rs crates/neo-tui/src/app.rs crates/neo-agent/src/modes/task_browser.rs crates/neo-agent/src/modes/interactive/input.rs
git diff --check
```

The responsive matrix is widths `32`, `69`, `70`, `99`, `100`, `120`, `180`
and heights `12`, `20`, `40`. Assert frame height, maximum visible width,
content hierarchy, page route, full command recoverability, Preview labels,
and no overlap.

Report evidence separately:

- macOS focused automation;
- macOS real graphical terminal;
- Fedora native automation/PTY;
- Windows native automation;
- Windows Terminal logged-in desktop pointer behavior;
- skipped platform checks and reasons;
- final residual risk.

Synthetic mouse events and macOS tests are not Windows-native evidence. SSH or
PTY is not a Windows Terminal graphical pointer check.

## 9. Git Boundary

One coherent task equals one commit. The coordinator may `git add` and
`git commit` under current project rules, but must stage exact paths only. The
pre-existing `crates/neo-tui/tests/transcript_selection.rs` modification is
outside this work. Do not include it even if it becomes relevant to a broad
format or test command.

Expected commits:

```text
feat(tui): redesign task browser layout
feat(tui): redesign workflow task workspace
feat(tui): add task browser pointer navigation
docs: record task browser redesign
```

## 10. Completion Condition

Use `aegis:verification-before-completion` before claiming completion. Completion
requires every Final Acceptance item in the plan, four scoped commits, no open
review issue, fresh exact tests, clean task-owned diffs, honest native evidence,
and no change outside the approved boundary.

This handoff does not authorize push, release, branch/worktree changes, or any
product redesign.
