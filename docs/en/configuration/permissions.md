# Permission Modes

Before executing a tool call, Neo decides whether user approval is required based on the current permission mode. The permission mode is controlled by the `permission_mode` field in `config.toml`, CLI flags (`--auto` / `--yolo`), and the `/ask`, `/auto`, `/yolo`, `/permissions` commands in the interactive TUI.

## The Four Permission Modes

At runtime Neo has three named modes — `Ask`, `Auto`, and `Yolo` — plus **Plan mode** activated by the `EnterPlanMode` tool. The three are defined by the `PermissionMode` enum, with Plan mode layered on top as an additional hard guard.

| Mode | String value | Behavior |
| --- | --- | --- |
| **Ask** | `"ask"` | Default. Read-like tools (`Read`/`List`/`Grep`/`Glob`, etc.) and known-safe commands are auto-approved; writes, shell, and tool calls always pop up an approval dialog |
| **Auto** | `"auto"` | Automatically approves all tool calls (including shell, writes, `ExitPlanMode`, and `ExitGoalMode`). `AskUserQuestion` is hard-denied |
| **Yolo** | `"yolo"` | Approves ordinary tool calls including dangerous commands and skips project trust checks, but still shows Plan/Goal review dialogs. Use only in controlled environments |
| **Plan** | — | Entered when the model calls `EnterPlanMode`; only read-only tools and writes to the current plan file are allowed, and `ExitPlanMode` requires user approval to exit (except in Auto) |

> Precedence of the three modes: the CLI flags `--yolo` / `--auto` override the config file; they cannot be used together. While running, you can switch in real time via slash commands, and the change takes effect immediately on the turn in progress.

```toml
# config.toml
permission_mode = "ask"
```

```shell
# CLI flags
neo --auto
neo --yolo
```

## Dynamic approval options

When Neo needs approval, the dialog is built from a single runtime-owned request. **Options are dynamic**: the list includes only actions the runtime can honor for that call — labels are presentation copy, not a separate semantic source.

### Ordinary Tool / Shell approvals

For ordinary Tool and Shell approvals the offered actions are:

| Option | Description | Storage |
| --- | --- | --- |
| **Approve once** | Approves only this occurrence; the next time still requires approval | Not persisted |
| **Session grant** (when available) | Auto-approves matching operations for the rest of the session | In memory (`session_approvals`) |
| **Prefix grant** (when available) | Future shell commands starting with this prefix are auto-approved | On disk (`~/.neo/approval_rules.json`) |
| **Reject** | Denies, returns `approval denied` to the model | — |

Ordinary Tool/Shell approvals **do not offer revision feedback**. Session and prefix grants appear only when the runtime can derive a safe reusable scope; if neither applies, the dialog shows only Approve once and Reject.

**Background Bash** never offers a reusable grant: session and prefix options are omitted, so only one-shot Approve and Reject are available.

### Plan and Goal review

- **Plan (`ExitPlanMode`)**: Approve (optionally with an alternative approach) exits plan mode and continues; **Reject** and **Revise** keep Plan mode active so Neo can revise the plan.
- **Goal (`ExitGoalMode`)**: The review shows **objective**, **completion criterion**, and **phases**. Approve starts the goal; **Reject** and **Revise create no goal**.
- **Ask** and **Yolo** show Plan/Goal review dialogs. **Auto** skips them and proceeds without a review prompt.

### Session-Level (Layer 1)

Session-level approvals are recorded by **exact normalized key**, never cached by tool name:

- **Shell**: `<workspace> + <cwd> + <argv>`. `git status` and `git log` are two distinct keys; a compound command like `git status && git push` is recorded as a single opaque key, not leaked as a separate `git status`.
- **File write/edit**: `<workspace> + <path> + <operation>`. Write and Edit are two independent keys.
- **Tool**: `<workspace> + <fully-qualified tool name>` (mainly for MCP tools).

> Cross-workspace isolation: all keys carry the workspace root path, so approvals do not leak when a session is reused.

### Prefix-Level (Layer 2)

Prefix-level rules match by token prefix (not substring), are persisted in `~/.neo/approval_rules.json`, and remain in effect across restarts:

```json
{
  "prefix_rules": [
    { "prefix": ["git"], "label": "git" },
    { "prefix": ["cargo", "test"], "label": "cargo test" }
  ]
}
```

- Empty prefixes are rejected (to prevent "approve all commands");
- Compound commands (containing `&&`, `|`, `;`, etc.) do not generate prefix rules, because their prefix is not a stable argv prefix;
- Dangerous commands (`rm -rf`, `sudo`, `curl | sh`, etc.) always force an approval dialog and never generate any reusable authorization.

### Command Safety Classification (Layer 3)

In Ask mode, Neo first classifies the command to decide whether to skip approval:

- **Known safe**: `ls`, `cat`, `git status`, `git log`, `cargo test`, and other read-only subcommands — auto-approved.
- **Dangerous commands**: `rm -rf`, `sudo`, `chmod`, `curl ... | sh`, etc. — force an approval dialog, requiring confirmation even if a prefix rule exists.
- **Others**: normal approval dialog.

## Permission Decision Flow (User's View)

From tool call initiation to execution, Neo short-circuits in the following order (returning as soon as any layer matches):

1. **Plan mode hard guard**: if in Plan mode and the tool is not on the read-only whitelist → denied outright.
2. **Auto / background AskUser**: Auto mode denies `AskUserQuestion` and approves everything else (including Plan/Goal exits); background `AskUserQuestion` never pops a dialog; `EnterPlanMode` is auto-approved in all modes.
3. **Prefix rules (Layer 2)**: matches a persisted prefix → approved.
4. **Session cache (Layer 1)**: matches an exact key already approved this session → approved.
5. **State-transition tools**: `ExitPlanMode` / `ExitGoalMode` pop a review dialog in Ask and Yolo (Auto already approved them in step 2).
6. **Yolo mode**: approves remaining ordinary tool calls (Plan/Goal reviews already handled above).
7. **Safety classification**: safe commands are approved; dangerous commands force a dialog; default-approved tools (`Read`/`List`/`Grep`/`Find`/`Glob`/`TodoList`/`TaskList`/`TaskOutput`/`Skill`/`AskUserQuestion`/`Sleep`) are approved.
8. **Fallback**: pops the approval dialog with only the actions the runtime can honor for that call (Approve once, optional session/prefix grants, Reject — no revision feedback on ordinary tools).

> Real-time: modes switched via `/ask`, `/auto`, `/yolo`, `/permissions` take effect immediately — there is no need to cancel the current turn; the next tool call will be evaluated against the new mode.

## Next Steps

- [Configuration Files](config-files.md) — where the `permission_mode` field lives
- [Provider Configuration](providers.md) — models and endpoints
- [Data Storage Locations](data-locations.md) — where `approval_rules.json` is stored
