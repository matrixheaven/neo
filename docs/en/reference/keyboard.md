# Keyboard Shortcuts Reference

Shortcuts in the Neo TUI are managed centrally by `KeybindingsManager` and support user overrides via configuration. This document lists the default keybindings.

Source location: [`crates/neo-tui/src/input/keybinding.rs`](../../../crates/neo-tui/src/input/keybinding.rs) (`default_keybinding_definitions`) and [`crates/neo-agent/src/modes/interactive/input.rs`](../../../crates/neo-agent/src/modes/interactive/input.rs) (event dispatch).

Each action maps to a stable configuration ID (e.g. `tui.editor.cursorUp`) that can be used to rebind keys in user configuration.

## General

| Shortcut | Action | Description |
| --- | --- | --- |
| `Enter` | `InputSubmit` | Submit the current prompt. |
| `Tab` | `InputTab` | Trigger auto-completion. |
| `Ctrl+C` | `AppClear` / `InputCopy` | Copy when the editor has a selection; otherwise clear the editor / interrupt the turn / reject an approval. |
| `Ctrl+D` | `AppExit` | Exit when the prompt is empty; press again within 500 ms to confirm the exit. |
| `Ctrl+Z` | `AppSuspend` | Suspend Neo to the shell background (resume with `fg`). |
| `Esc` | `SelectCancel` | Close a popup / cancel a selection. |

## Mode Switching

| Shortcut | Action | Description |
| --- | --- | --- |
| `Shift+Tab` | `CycleDevelopmentMode` | Cycle through normal → plan → goal modes. |
| `Ctrl+P` | `PromptCompletionToggle` | Open the `/` command completion list; close it when already open. |
| `Ctrl+R` | `SessionPickerOpen` | Open the session picker. |
| `Ctrl+N` | `SessionFork` | Fork the selected session when the picker is open; otherwise fork the current session. |
| `Ctrl+A` | `SessionPickerToggleScope` | Toggle current workspace / all sessions in the session picker. |

> Plan / Goal / Model picker also expose `TogglePlanMode` / `ModelPickerOpen` actions, but they have no default key binding and are triggered via the command palette or slash commands.

## Input Editing (Emacs Style)

| Shortcut | Action |
| --- | --- |
| `←` / `Ctrl+B` | Move cursor left |
| `→` / `Ctrl+F` | Move cursor right |
| `Alt+←` / `Ctrl+←` / `Alt+B` | Move one word left |
| `Alt+→` / `Ctrl+→` / `Alt+F` | Move one word right |
| `Home` / `Ctrl+A` | Move to start of line |
| `End` / `Ctrl+E` | Move to end of line |
| `PageUp` / `PageDown` | Scroll up / down a page |
| `Backspace` | Delete one character backward |
| `Delete` / `Ctrl+D` | Delete one character forward |
| `Ctrl+W` / `Alt+Backspace` | Delete one word backward |
| `Alt+D` / `Alt+Delete` | Delete one word forward |
| `Ctrl+U` | Delete to start of line |
| `Ctrl+K` | Delete to end of line |
| `Ctrl+Y` | Yank (paste deleted content) |
| `Ctrl+-` / `Ctrl+_` | Undo |
| `Alt+Enter` / `Ctrl+J` | Insert a newline |
| `Ctrl+V` (Windows: `Alt+V`) | Paste image from clipboard |

## Streaming Control

| Shortcut | Action | Description |
| --- | --- | --- |
| `Ctrl+S` | `PromptSteer` | Steer the running turn with the current editor text at the next natural breakpoint; if no turn is running, queues it as the next message. Requires `stty -ixon`. |
| `Alt+Up` | `EditNextQueuedMessage` | Pull the next queued message back into the editor for editing. |
| `Ctrl+C` | `Cancel` / `Interrupt` | Cancel / interrupt the current turn; rejects all pending approvals if any are waiting. |
| `Ctrl+T` | `TodoPanelToggle` | Expand / collapse the Todo panel. |

## Tool Output & Transcript

| Shortcut | Action | Description |
| --- | --- | --- |
| `Ctrl+O` | `ToolOutputToggle` | Expand / collapse tool call output. |
| `Ctrl+Space` | `TranscriptSelectionStart` | Enter transcript entry selection. |
| `Ctrl+Shift+Space` | `TranscriptSelectionClear` | Clear the transcript selection. |
| `Shift+Up` / `Shift+Down` | Extend selection up / down | |
| `Shift+PageUp` / `Shift+PageDown` | Extend selection up / down a page | |
| `Ctrl+C` (with a selection) | `TranscriptCopySelection` | Copy the transcript selection. |

## Approval Modal

When the approval modal is open, input is handled by `handle_pending_approval_input`:

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move between Approve / Always approve / Reject / Revise options. |
| `1` – `4` | Directly select the Nth option. |
| `Enter` | Confirm the current option; when Revise is selected, the first Enter enters feedback input, and a second Enter submits. |
| `Esc` / `Ctrl+C` | Reject (equivalent to Reject) / close. |
| `Backspace` / `Delete` | Delete within the Revise feedback input. |
| Other characters | Append to the Revise feedback input. |

## Pickers / Popups (General)

| Shortcut | Action |
| --- | --- |
| `↑` / `↓` | `SelectUp` / `SelectDown` |
| `PageUp` / `PageDown` | `SelectPageUp` / `SelectPageDown` |
| `Enter` | `SelectConfirm` |
| `Esc` / `Ctrl+C` | `SelectCancel` |

## Customization

All actions expose a stable ID (see the configuration ID corresponding to the "Action" column in each table, e.g. `tui.input.submit`, `app.exit`). Bind keys under the key paths shown in the table below in `~/.neo/config.*` to override the defaults; conflicts are detected and reported by `KeybindingsManager`.
