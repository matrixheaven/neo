# Permission Modes

Before executing a tool call, Neo decides whether user approval is required based on the current permission mode. The permission mode is controlled by the `permission_mode` field in `config.toml`, CLI flags (`--auto` / `--yolo`), and the `/ask`, `/auto`, `/yolo`, `/permissions` commands in the interactive TUI.

## The Four Permission Modes

At runtime Neo has three named modes — `Ask`, `Auto`, and `Yolo` — plus **Plan mode** activated by the `EnterPlanMode` tool. The three are defined by the `PermissionMode` enum, with Plan mode layered on top as an additional hard guard.

| Mode | String value | Behavior |
| --- | --- | --- |
| **Ask** | `"ask"` | Default. Read-like tools (`Read`/`List`/`Grep`/`Glob`, etc.) and known-safe commands are auto-approved; writes, shell, and tool calls always pop up an approval dialog |
| **Auto** | `"auto"` | Automatically approves all tool calls (including shell and writes). However, `AskUserQuestion` is hard-denied, and `ExitPlanMode` / `ExitGoalMode` still require approval |
| **Yolo** | `"yolo"` | Approves everything, including dangerous commands; also skips project trust checks. Use only in controlled environments |
| **Plan** | — | Entered when the model calls `EnterPlanMode`; only read-only tools and writes to the current plan file are allowed, and `ExitPlanMode` requires user approval to exit |

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

## Approval Granularity

In Ask mode, every call requiring approval offers several granularity options (determined by `PermissionApprovalDecision`):

| Decision | Description | Storage location |
| --- | --- | --- |
| **Allow once** (single) | Approves only this occurrence; the next time still requires approval | Not persisted |
| **Allow for session** (session) | Automatically approves identical operations within the current session | In memory (`session_approvals`) |
| **Allow for prefix** (prefix) | Future shell commands starting with this prefix are auto-approved | On disk (`~/.neo/approval_rules.json`) |
| **Reject** | Denies, returns `approval denied` to the model | — |

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
2. **Auto / background AskUser**: Auto mode denies `AskUserQuestion` and approves everything else; background `AskUserQuestion` never pops a dialog; `EnterPlanMode` is auto-approved in all modes.
3. **Prefix rules (Layer 2)**: matches a persisted prefix → approved.
4. **Session cache (Layer 1)**: matches an exact key already approved this session → approved.
5. **State-transition tools**: `ExitPlanMode` / `ExitGoalMode` require separate approval (even in Auto mode).
6. **Yolo mode**: approves all remaining calls.
7. **Safety classification**: safe commands are approved; dangerous commands force a dialog; default-approved tools (`Read`/`List`/`Grep`/`Find`/`Glob`/`TodoList`/`TaskList`/`TaskOutput`/`Skill`/`AskUserQuestion`/`Sleep`) are approved.
8. **Fallback**: pops the approval dialog, waiting for the user to choose Allow once / Allow for session / Allow for prefix / Reject.

> Real-time: modes switched via `/ask`, `/auto`, `/yolo`, `/permissions` take effect immediately — there is no need to cancel the current turn; the next tool call will be evaluated against the new mode.

## Next Steps

- [Configuration Files](config-files.md) — where the `permission_mode` field lives
- [Provider Configuration](providers.md) — models and endpoints
- [Data Storage Locations](data-locations.md) — where `approval_rules.json` is stored
