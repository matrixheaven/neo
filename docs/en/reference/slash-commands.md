# Slash Commands Reference

In interactive mode, any input beginning with `/` is parsed by `InteractiveController::handle_slash_command`. This document lists all built-in slash commands.

Source location: [`crates/neo-agent/src/modes/interactive/slash_commands.rs`](../../../crates/neo-agent/src/modes/interactive/slash_commands.rs) and `STATIC_SLASH_COMMANDS` in `prompt_completion.rs`.

## Session Management

| Command | Alias | Description |
| --- | --- | --- |
| `/new` | — | Start a new local session. |
| `/clear` | `/new` | An alias for `/new`. |
| `/resume` | — | Open the session picker to restore a local session. |
| `/compact` | — | Request a manual context compaction; an instruction may be appended as `/compact <instruction>`. |
| `/tasks` | — | View currently active background tasks. |
| `/fork` | — | Create a new branch from the current session and switch to it. |
| `/init [instruction]` | — | Create or refresh the workspace-root `AGENTS.md` only; nested `AGENTS.md` files are user-authored and never generated or modified by `/init`. Extra text is passed to the init workflow as natural-language guidance. |

`/init` is TUI-only. Interactive workflows such as `/init`, `/skill:self-evo`, and `/skill:create-skill` may open a local preflight in Auto mode before starting. Neo does this mechanically from the parsed slash command; the model does not decide to switch permission modes.

## Mode Control

| Command | Alias | Description |
| --- | --- | --- |
| `/plan` | — | Toggle plan mode; arguments: `on` / `off` / `clear`. |
| `/goal` | — | Goal mode entry; arguments such as `replace <obj>`, `next <obj>`. |
| `/ask` | — | Switch to **Ask** permission mode (prompt before every risky action). |
| `/auto` | — | Switch to **Auto** permission mode (non-interactive execution). |
| `/yolo` | — | Switch to **Yolo** permission mode (skip confirmations). |
| `/permissions` | `/permission` | Open the permission mode picker. |

> `/ask`, `/auto`, and `/yolo` take effect immediately even while a turn is running (real-time switching). All other slash commands require the current turn to be interrupted first.

## Information & Status

| Command | Description |
| --- | --- |
| `/help` | Open the help panel, listing all available commands and skills. |
| `/model [alias]` | With no argument, opens the model picker; with an argument, switches to the specified alias. |
| `/provider` | Open the provider picker to view configured providers. |
| `/mcp` | Open the MCP management panel to view / manage MCP servers. |
| `/btw [question]` | Open a temporary side panel for an ad-hoc ("by the way") question. |

## Exit

Neo's interactive mode does **not** have an `/exit` or `/quit` slash command. See [Keyboard Shortcuts · General](keyboard.md) for ways to exit:

| Action | Shortcut |
| --- | --- |
| Exit the application (when the prompt is empty) | `Ctrl+D` (press again within 500 ms to confirm) |
| Clear the editor / interrupt a turn | `Ctrl+C` |
| Suspend to background | `Ctrl+Z` |

## Built-in Skills

| Command | Description |
| --- | --- |
| `/skill:<name> [args]` | Activate the skill named `<name>`, optionally with arguments; multiple `/skill:` directives are supported on the same line. |

Once activated, the skill's content is injected as context and a `SkillActivation` card is shown in the transcript. The list of available skills can be viewed via `/help` or prompt auto-completion.

## Command Palette (non-slash)

Press `Ctrl+P` to open the command palette, which contains commands not exposed as slash commands — for example: `session.exportHtml` (export to HTML), `fork` (fork a session), `copy-prompt`, `select-transcript`, and more. See [Keyboard Shortcuts](keyboard.md).
