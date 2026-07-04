# Interaction mode

Running `neo` with no arguments drops you into the interactive TUI. This page covers input methods, permission modes, approval dialogs, streaming output, and advanced interactions like queue & steer.

## Input modes

| Mode | Trigger | Use case |
| --- | --- | --- |
| Single-line | Type and press `Enter` | Short questions and instructions |
| Multi-line | `Alt+Enter` or `Ctrl+J` for a newline | Multi-paragraph prompts, code snippets |
| History recall | `↑` / `↓` on an empty prompt | Reuse a previous input |
| Shell mode | Enter the shell command editing path | Edit a queued shell command |

### File / path completion

Typing a relative or absolute path prefix triggers local file completion; `/` triggers slash-command completion; `@` triggers model-alias completion. Press `Tab` to accept a completion suggestion.

### `@` file references

Using `@path/to/file` in a prompt splices the file contents into the message on submit, so you don't have to copy-paste manually:

```
Review whether error handling in @src/parser.rs is complete
```

## Image paste

Neo supports sending clipboard images as multimodal attachments alongside the prompt.

| Platform | Paste shortcut |
| --- | --- |
| macOS / Linux | `Ctrl+V` |
| Windows | `Alt+V` |

Pasting inserts an image attachment preview into the prompt, sent with the next message. Requires a vision-capable model (e.g. `gpt-4.1`).

## Slash commands

Typing `/` at the start of the prompt triggers command completion. Common commands:

| Command | Effect |
| --- | --- |
| `/new` `/clear` | Start a fresh local session |
| `/resume` | Open the session picker |
| `/model [alias]` | Switch model (no arg opens the model picker) |
| `/provider` | Show configured providers |
| `/mcp` | View/manage MCP servers |
| `/tasks` | View background tasks |
| `/plan [on\|off\|clear]` | Toggle plan mode |
| `/goal <objective>` | Start/manage goal mode |
| `/compact [instruction]` | Request a manual context compaction |
| `/permissions` `/ask` `/auto` `/yolo` | Switch permission mode |
| `/btw <question>` | Open the side-question panel |
| `/skill:<name>` | Activate a skill |
| `/help` | Open the help panel |

See the [slash commands reference](../reference/slash-commands.md) for the full list.

## Permission modes

The permission mode decides whether Neo asks you to confirm each tool call. Switch with `/permissions` or a launch flag.

| Mode | Flag | Behavior |
| --- | --- | --- |
| **Ask** (default) | — | Commands, edits, and other risky actions ask first; read-only/search tools run directly |
| **Auto** | `--auto` | Fully non-interactive: tool actions are auto-approved and `AskUserQuestion` is skipped |
| **YOLO** | `--yolo` | Auto-approves tool actions and plan transitions, but Neo may still actively ask you questions when needed |

> `--auto` and `--yolo` are mutually exclusive. Auto is suited to scripted/headless flows; YOLO fits workflows where you want to let go but keep the option for interactive questions.

Inside the TUI you can switch at any time with `/ask` `/auto` `/yolo`, or cycle development modes (Normal → Plan → Goal) with `Shift+Tab`.

## Approval dialog

When Neo needs to execute a tool call that requires approval, an approval dialog appears. Options:

| Option | Meaning |
| --- | --- |
| **Approve** | Allow this one time |
| **Always Approve** (exact) | Auto-approve this exact command for the rest of the session |
| **Always Approve** (prefix) | Auto-approve any command with the same prefix this session |
| **Reject** | Deny this call; Neo receives the rejection signal and adapts |
| **Revise** | Reject with revision feedback; Neo reads your note and retries |

Keys:

| Key | Action |
| --- | --- |
| `↑` `↓` / `PageUp` `PageDown` | Navigate between options |
| `Enter` | Confirm the selected option |
| `Esc` / `Ctrl+C` | Reject and cancel (also interrupts any running turn) |

## Streaming output

Neo renders model output as a stream by default: assistant text is printed to the transcript as it is generated, and tool calls and results appear as collapsible cards.

- `Ctrl+T`: expand/collapse the Todo panel
- Tool-output cards can be collapsed/expanded
- Use the transcript selection shortcut to select and copy a region of output

## /btw side question

`/btw` opens a temporary **side-question panel** to ask a tangential question outside the main session. It inherits the current session's context, but its answers are not written back to the main transcript:

```
/btw What's the complexity of the fizzbuzz here?
```

| Key | Action |
| --- | --- |
| `/btw <text>` | Open the panel and ask immediately |
| Empty prompt + `↑` / `↓` | Scroll through panel history |
| `Esc` | Close the panel |

The main turn is unaffected while a side question runs; only one `/btw` can run at a time.

## Queue & steer

Neo lets you queue follow-up instructions while the agent is busy, or inject a steer message at the next breakpoint.

### Queueing a follow-up

While Neo is executing a turn, just type a new prompt and submit — it joins the **follow-up queue** and runs automatically once the current turn ends.

| Key | Action |
| --- | --- |
| `Ctrl+S` | Inject the current input as a **steer** message at the next breakpoint |
| `Alt+↑` | Pull the head follow-up back into the editor to revise before resubmitting |

### Steer injection

`Ctrl+S` (`PromptSteer`) injects the current prompt as a **steer instruction** into the running turn. It is seen by the agent at the next tool-call breakpoint, letting you correct course without waiting for the turn to end. If no turn is running, it behaves like a queued follow-up.

## Quitting

| Key | Behavior |
| --- | --- |
| `Ctrl+C` twice | Quit after confirmation (or after the prompt is cleared) |
| `Ctrl+D` twice | Same as above |
| `Ctrl+Z` | Suspend to background |

A single `Ctrl+C` first tries: interrupt the running turn / reject a pending approval / close an overlay / clear the prompt — only then does it prompt for quit confirmation.

## Next steps

- [Session management](sessions.md) — Resume, fork, compact, and export conversations
- [Plan mode](plan-mode.md) — Let Neo produce a plan before touching code
- [Goal mode](goals.md) — Autonomously drive a verifiable objective
- [Slash commands reference](../reference/slash-commands.md)
