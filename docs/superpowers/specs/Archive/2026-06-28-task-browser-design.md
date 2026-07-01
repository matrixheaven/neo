# `/tasks` Task Browser — Design Spec

## Overview

Replace the current human-facing `/tasks` behavior in Neo's TUI with a full-screen Task Browser. Today `/tasks` appends the model-oriented `TaskList` text result to the transcript:

```text
active_background_tasks: 0
No background tasks found.
```

That output is appropriate for the `TaskList` tool, but it is not a good user interface. The human `/tasks` command should open an interactive task-management panel, inspired by `docs/kimi-code`, while keeping Neo's own TUI style and data model.

The tool-facing `TaskList` result remains unchanged. This spec only changes the TUI slash-command experience.

## Goals

- `/tasks` opens a blocking, full-screen Task Browser instead of appending raw tool output to the transcript.
- Default filter is `ALL`, so completed, failed, cancelled, waiting, and running tasks remain visible.
- The browser presents a three-pane layout on normal terminal widths:
  - task list
  - selected task detail
  - selected task output preview
- Empty state is product-facing copy, not model/tool protocol text.
- Users can refresh, filter, select tasks, inspect output, and stop non-terminal tasks.
- The panel preserves the previous chat state and returns cleanly to the composer on close.
- Model tools keep their structured `TaskList`, `TaskOutput`, and `TaskStop` semantics.

## Non-Goals

- Do not change the `TaskList` tool result format.
- Do not add slash subcommands such as `/tasks active`, `/tasks stop`, or `/tasks output`.
- Do not expose task internals such as raw JSON snapshots or manager debug fields.
- Do not build a separate task history page outside the TUI shell.
- Do not merge task UI with the model transcript; `/tasks` is a blocking browser overlay.

---

## 1. Product Model

Neo has two task surfaces:

| Surface | Audience | Shape |
| --- | --- | --- |
| `TaskList` tool | model | structured text + details |
| `/tasks` slash command | human | interactive TUI browser |

This separation is intentional. The model needs stable, parseable task state. The user needs a navigable control surface with status, details, output preview, and action shortcuts.

`/tasks` should feel like `/resume`, approval dialogs, and other blocking TUI surfaces: once open, it owns keyboard input until closed. It should not create a conversation turn, write a model-visible transcript entry, or mutate prompt history.

## 2. Entry And Exit

### Entry

When the user submits exactly `/tasks` from the composer:

1. Clear the submitted prompt.
2. Open `TaskBrowserState` as a blocking overlay/screen.
3. Snapshot current background tasks from the shared `BackgroundTaskManager`.
4. Default the filter to `ALL`.
5. Select the first visible task, if any.

If the user is in shell mode and submits exactly `/tasks`, the same Task Browser opens. `/tasks` is not executed as a shell command.

### Exit

`Q` and `Esc` close the browser and restore the previous TUI state. Closing the browser must not clear unrelated composer text, shell-mode state, queued follow-ups, or transcript scroll state.

If a stop confirmation is active, `Esc` cancels the confirmation first. A second `Esc` closes the browser.

---

## 3. Layout

### Normal Width

Normal-width terminals use a three-pane layout. The left `Tasks` pane must consume the full content height from the pane row down to the footer, matching `docs/kimi-code`. Only the right column is split vertically into `Detail` and `Preview Output`.

The Task Browser owns its shortcut footer. While it is open, Neo must not append the normal composer/footer underneath it; otherwise the left pane will not truly fill the available browser height.

```text
 TASK BROWSER  filter=ALL  0 total                                      ...

┌─ Tasks [all] ─────────────────────┐ ┌─ Detail ───────────────────────────────────────────────┐
│ No background tasks in this       │ │ Select a task from the list.                           │
│ session.                          │ │                                                          │
│                                   │ │                                                          │
│                                   │ │                                                          │
│                                   │ └──────────────────────────────────────────────────────────┘
│                                   │ ┌─ Preview Output ─────────────────────────────────────────┐
│                                   │ │ No task selected.                                        │
│                                   │ │                                                          │
│                                   │ │                                                          │
│                                   │ │                                                          │
└───────────────────────────────────┘ └──────────────────────────────────────────────────────────┘

 ↑↓ select   Enter/O output   S stop   R refresh   Tab filter   Q/Esc close
```

With tasks:

```text
 TASK BROWSER  filter=ALL  1 running  2 completed  1 interrupted  4 total       ...

┌─ Tasks [all] ─────────────────────┐ ┌─ Detail ───────────────────────────────────────────────┐
│ › ● bash-72d1  running   cargo... │ │ id:          bash-72d1                                  │
│   ✓ bash-a83f  done      rg ...   │ │ kind:        bash                                       │
│   ✕ bash-ef20  failed    npm ...  │ │ status:      running                                    │
│   ◼ ask-918a   waiting   Pick...  │ │ elapsed:     02:14                                      │
│                                   │ │ detached:    yes                                        │
│                                   │ └──────────────────────────────────────────────────────────┘
│                                   │ ┌─ Preview Output ─────────────────────────────────────────┐
│                                   │ │ Compiling neo-tui v0.1.0                                 │
│                                   │ │ ...                                                      │
│                                   │ │                                                          │
└───────────────────────────────────┘ └──────────────────────────────────────────────────────────┘

 ↑↓ select   Enter/O output   S stop   R refresh   Tab filter   Q/Esc close
```

### Responsive Behavior

The browser must remain usable on small terminals:

- Wide layout: full-height left list plus right detail/output stack.
- Narrow layout: list on top, detail/output below.
- Very low height: keep header and footer visible; shrink preview output before shrinking the task list below a useful minimum.
- If only one content pane can be shown, `Enter`/`O` toggles output focus and `Tab` still cycles filter.

Minimum useful targets:

- list width: 30 columns
- right pane width: 50 columns
- preview height: 3 rows
- footer height: 1 row

If these cannot all fit, render the most useful degraded view rather than overflowing text.

---

## 4. Header

Header format:

```text
 TASK BROWSER  filter=ALL  1 running  2 completed  1 interrupted  4 total       ...
```

Rules:

- Always show `TASK BROWSER`.
- Always show the active filter: `filter=ALL` or `filter=ACTIVE`.
- Count only visible tasks after filtering.
- Show non-zero status counters in stable order:
  1. running
  2. waiting
  3. completed
  4. interrupted
- Always show total visible count.
- Fit/truncate to terminal width.

`interrupted` covers failed, cancelled, timed out, killed, and lost terminal states. The list/detail panes may show the more precise status.

---

## 5. Task List Pane

The list pane is the primary navigation surface.

Title:

```text
Tasks [all]
Tasks [active]
```

Rows use a compact status marker:

| Marker | Meaning |
| --- | --- |
| `●` | running |
| `◼` | waiting/pending question |
| `✓` | completed |
| `✕` | failed/cancelled/timed out/killed/lost |

Row shape:

```text
› ● bash-72d1  running   cargo...
  ✓ bash-a83f  done      rg ...
  ✕ bash-ef20  failed    npm ...
  ◼ ask-918a   waiting   Pick...
```

Selection:

- `›` marks the selected visible task.
- Selection remains on the same task ID across refreshes if it still exists and remains visible.
- If the selected task disappears because of filtering, select the first visible task.
- If no tasks are visible, clear selection.

Empty states:

```text
No background tasks in this session.
```

for `ALL`, and:

```text
No active tasks. Tab = show all.
```

for `ACTIVE`.

---

## 6. Detail Pane

The detail pane shows stable task metadata for the selected task.

Empty state:

```text
Select a task from the list.
```

Selected bash task:

```text
id:          bash-72d1
kind:        bash
status:      running
elapsed:     02:14
detached:    yes
timeout:     10:00
```

Selected question task:

```text
id:          ask-918a
kind:        question
status:      waiting
elapsed:     00:31
prompt:      Pick one
```

Terminal task detail includes:

```text
exit code:   1
finished:    00:02 ago
reason:      timed out
```

Detail labels must be human-facing. Avoid exposing Rust enum names or internal field names where a clearer label exists.

---

## 7. Preview Output Pane

The preview pane shows a bounded output tail for the selected task.

Empty state:

```text
No task selected.
```

No output:

```text
No output yet.
```

Output rules:

- Show a sanitized tail of stdout/stderr or task output, matching the existing shell-output sanitization policy.
- Keep it preview-sized; do not make `/tasks` load an unbounded log into memory.
- Preserve truncation indicators when output is truncated.
- For non-output tasks, show the most useful task-specific preview. For a pending question, show the prompt text.

`Enter` or `O` focuses/toggles output viewing. V1 may keep this inside the preview pane with scroll controls rather than opening a separate full output viewer. The keybinding should be reserved now so a later dedicated output viewer can reuse it without changing user muscle memory.

---

## 8. Filtering

Default filter: `ALL`.

`Tab` toggles:

```text
ALL -> ACTIVE -> ALL
```

`ALL` includes all known session tasks. `ACTIVE` includes non-terminal tasks:

- running
- waiting
- pending

Filtering must not delete or stop tasks. It only changes visibility.

---

## 9. Actions

Footer:

```text
 ↑↓ select   Enter/O output   S stop   R refresh   Tab filter   Q/Esc close
```

Keybindings:

| Key | Behavior |
| --- | --- |
| `Up` / `Down` | move selection |
| `PageUp` / `PageDown` | scroll list or focused output |
| `Home` / `End` | first/last visible task |
| `Enter` / `O` | focus/toggle output preview |
| `S` | stop selected non-terminal task |
| `R` | refresh task snapshots |
| `Tab` | toggle filter |
| `Q` / `Esc` | close browser or cancel confirmation |

### Stop Confirmation

Stopping is destructive enough to require confirmation.

When `S` is pressed on a running/waiting task:

```text
Stop bash-72d1?  Enter confirm   Esc cancel
```

`Enter` calls the same task-stop path used by the model-facing `TaskStop` tool. `Esc` cancels the confirmation. Stopping a terminal task is a no-op and should show a short footer message:

```text
Task already finished.
```

---

## 10. Refresh Model

The browser reads from the shared `BackgroundTaskManager`.

Refresh sources:

- initial snapshot on open
- manual refresh with `R`
- lightweight periodic refresh while open, approximately once per second
- immediate refresh after stop confirmation completes

The periodic refresh must not block rendering. If snapshot collection fails, keep the previous view and show a footer status message:

```text
Could not refresh tasks.
```

The browser should not auto-close when the last active task finishes. It should refresh in place so the completed task remains inspectable under the default `ALL` filter.

---

## 11. Architecture

Add a TUI task-browser module under `neo-tui`, for example:

```text
crates/neo-tui/src/tasks_browser/
  mod.rs
  state.rs
  render.rs
```

Suggested responsibilities:

- `TaskBrowserState`: filter, selection, scroll offsets, focused pane, confirmation state, footer message.
- `TaskBrowserSnapshot`: UI-ready task rows and selected-task detail/preview data.
- `TaskBrowserRenderer`: pure rendering from state + snapshot + theme + terminal size.
- `TaskBrowserAction`: input actions emitted by key handling.

`neo-agent` owns runtime integration:

- opening the browser from `/tasks`
- pulling snapshots from `BackgroundTaskManager`
- dispatching stop actions
- refreshing snapshots on timer/manual refresh
- closing the browser

Keep render logic pure and testable in `neo-tui`. Keep async task management in `neo-agent`/`neo-agent-core`.

### Data Shape

The UI should depend on a stable view model rather than raw manager internals:

```rust
pub struct TaskBrowserItem {
    pub id: String,
    pub kind: TaskBrowserKind,
    pub status: TaskBrowserStatus,
    pub title: String,
    pub description: String,
    pub elapsed: String,
    pub detail_lines: Vec<String>,
    pub preview_lines: Vec<String>,
    pub can_stop: bool,
}
```

The adapter from `BackgroundTaskSnapshot` to `TaskBrowserItem` lives outside the renderer. This keeps task rendering independent from background task storage details.

---

## 12. Integration With Existing Features

### Shell Mode

Shell mode already treats exact `/tasks` specially. That behavior should open the same Task Browser and should not execute `/tasks` as a shell command.

When a shell command is moved to the background, the transcript hint remains:

```text
Moved to background. Use /tasks to view.
```

### Background Questions

Pending background questions appear as waiting tasks. If later Neo adds direct question answering from the Task Browser, it should extend this surface. V1 only lists them and shows their prompt/detail.

### Transcript

Opening and closing `/tasks` should not append a status message to the transcript. Errors that prevent the browser from opening may use the existing status mechanism, but normal empty state belongs inside the browser.

### Permissions

Opening `/tasks` is read-only and needs no approval. Stopping a task is user-initiated from the TUI and should not need model/tool approval, but it should require the in-browser confirmation described above.

---

## 13. Testing

Render tests in `neo-tui`:

- empty `ALL` state shows `TASK BROWSER`, `filter=ALL`, `0 total`, and `No background tasks in this session.`
- empty `ACTIVE` state shows `No active tasks. Tab = show all.`
- populated state shows status counters, selected row marker, detail lines, preview output, and footer shortcuts
- narrow terminal renders a usable stacked layout
- long task IDs/descriptions/output lines are truncated without overflowing
- stop confirmation renders and `Esc` returns to normal browser state

Controller tests in `neo-agent`:

- exact `/tasks` opens the browser rather than appending `active_background_tasks`
- exact `/tasks` in shell mode opens the browser rather than executing a shell command
- default filter is `ALL`
- `Tab` toggles filter and preserves selection when possible
- `R` refreshes from the shared manager
- `S` + `Enter` stops a non-terminal task and refreshes state
- `Q`/`Esc` closes the browser without mutating prompt history

Core tests remain focused on the model tools:

- `TaskList` empty output still returns `active_background_tasks: 0` and `No background tasks found.`
- `TaskList` active/all filtering still returns structured content/details for model use.

## Open Decisions

All v1 product decisions are resolved:

- Use full-screen Kimi-style Task Browser.
- Default filter is `ALL`.
- Keep `TaskList` text output unchanged for models.
- Use in-browser stop confirmation.
- Reserve `Enter/O` for output viewing, even if v1 keeps output inside the preview pane.
