# Slash Commands Reference

In interactive mode, any input beginning with `/` is parsed by `InteractiveController::handle_slash_command`. This document lists all built-in slash commands.

Source location: [`crates/neo-agent/src/modes/interactive/slash_commands.rs`](../../../../crates/neo-agent/src/modes/interactive/slash_commands.rs) and `STATIC_SLASH_COMMANDS` in `prompt_completion.rs`.

## Session Management

| Command | Alias | Description |
| --- | --- | --- |
| `/new` | — | Start a new local session. |
| `/clear` | `/new` | An alias for `/new`. |
| `/resume` | — | Open the session picker to restore a local session. |
| `/compact` | — | Request a manual context compaction; an instruction may be appended as `/compact <instruction>`. |
| `/tasks` | — | Open the task browser: background tasks and workflow runs (phase, admission wait, awaiting input, usage). |
| `/workflow` | — | Open the searchable effective-workflow picker. |
| `/workflow <task>` | — | Start a normal model turn with the complete effective workflow catalog so the model can choose an existing workflow. |
| `/workflow:<name> <task>` | — | Start a normal model turn with the selected workflow definition and full input schema. |
| `/skill:create-workflow <request>` | — | Author or change a workflow through the existing skill path. |
| `/fork` | — | Create a new branch from the current session and switch to it. |
| `/init [instruction]` | — | Create or refresh the workspace-root `AGENTS.md` only; nested `AGENTS.md` files are user-authored and never generated or modified by `/init`. Extra text is passed to the init workflow as natural-language guidance. |

`/init` is TUI-only. Interactive flows such as `/init`, `/skill:self-evo`, and `/skill:create-skill` may open a local preflight in Auto mode before starting. Neo does this mechanically from the parsed slash command; the model does not decide to switch permission modes.

### `/workflow` forms

| Form | Behavior |
| --- | --- |
| `/workflow` | Opens a searchable picker. Choosing an item only fills `/workflow:<name> ` in the composer; it does not start a turn. |
| `/workflow <natural-language task>` | Sends the complete effective catalog to one visible model turn. If nothing fits, the assistant asks before authoring or continuing without a workflow. |
| `/workflow:<name> <natural-language task>` | Sends the resolved definition and full input schema to one visible model turn. The model maps the task to workflow inputs. |
| `/skill:create-workflow <authoring request>` | Separately enters workflow authoring. It is not required to use an existing saved workflow. |

Slash matching is exact: `/workflowish` and prose containing `/workflow` are ordinary prompts. Local grammar or registry errors keep the original input in the composer and start no model turn. After the model chooses a workflow, existing Ask / Auto / Yolo permissions and workflow cards apply. Headless `neo workflow` commands remain for humans and scripts. See [Workflows](../guides/workflows.md).

## Mode Control

| Command | Alias | Description |
| --- | --- | --- |
| `/plan` | — | Toggle plan mode; arguments: `on` / `off` / `clear`. |
| `/goal` | — | Goal mode entry; arguments such as `replace <obj>`, `next <obj>`. |
| `/ask` | — | Switch to **Ask** permission mode (prompt before every risky action). |
| `/auto` | — | Switch to **Auto** permission mode (non-interactive execution). |
| `/yolo` | — | Switch to **Yolo** permission mode (skip confirmations). |
| `/permissions` | `/permission` | Open the permission mode picker. |

> `/ask`, `/auto`, and `/yolo` take effect immediately even while a turn is running (real-time switching). `/theme <name-or-id>` may also be applied during a running turn. All other slash commands require the current turn to be interrupted first.

## Theme Management

| Command | Behavior |
| --- | --- |
| `/theme` | Open the theme manager. Requires the main turn to be idle; while busy, Neo keeps the turn active and shows the idle requirement. |
| `/theme <name-or-id>` | Apply the theme to the current session immediately, even during a running turn. Resolution is exact: the logical `ThemeId` first, then a unique exact display name; no fuzzy matching. |
| `/theme reload` | Clear the current session override and re-apply the theme resolved from `[tui].theme`. |
| `/skill:custom-theme` | Explicit-only AI-assisted theme creation; previews before saving and never auto-applies. |

The manager supports list, filter, preview, Apply for session, Set startup default, import (with Overwrite / Save as new conflict choices), copy, delete (the active and startup-default themes are protected), and refresh. `/theme <name-or-id>` affects the current session only — it does not change the startup default. See [Themes](../customization/themes.md).

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
