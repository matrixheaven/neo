# Architecture

Neo is organized as a local agent around a narrow model/provider layer and an
agent-core runtime that can be tested without a terminal UI.

## Crate Boundaries

```text
neo-agent CLI/TUI
  -> neo-agent-core runtime, sessions, permissions, tools, MCP, local extensions,
     skill loading, JSONL RPC, and HTML export
      -> neo-ai provider-neutral model and stream contracts
  -> neo-tui terminal UI primitives (crossterm-based component tree)
xtask maintenance commands
```

## Implemented Today

- `neo-ai` defines provider-neutral request, message, model, capability, tool, and stream event types.
- `neo-ai` defines request options, environment key helpers, model/provider registries, and production provider resolution.
- `neo-ai` includes OpenAI Responses, Anthropic Messages, Google Generative AI,
  OpenAI-compatible, and OpenAI-style image generation network clients.
- `neo-ai::providers::fake::FakeModelClient` records requests and replays stream events for tests.
- `neo-agent-core` contains a runtime turn loop, fake harness, permissions,
  built-in tools (Read, List, Grep, Find, Glob, Write, Edit, Bash, Terminal,
  TodoList, EnterPlanMode, ExitPlanMode, and goal tools when a GoalManager is
  attached), MCP adapters, local extension adapters, skill loading, JSONL RPC
  primitives, HTML export, reasoning event persistence, and JSONL session
  helpers.
- `neo-agent` exposes the local command-line and TUI surface.
- `neo-tui` owns terminal rendering via a component-tree architecture:
  - `terminal/`: single-buffer terminal rendering, input parsing, and low-level UI
    primitives.
  - `neo_tui.rs`: the Neo surface that combines transcript, chrome, prompt,
    overlays, and footer state.
  - `transcript/`: `TranscriptStore`, ordered transcript entries, tool call
    lifecycle rendering, per-tool-type renderers, LCS-based inline diff preview.
  - `widgets/`: `QuestionStateMachine` (multi-question dialog), `TodoPanel`.
  - `image.rs`: Kitty, iTerm2, and Sixel inline image encoding.
- `xtask check` verifies the stable developer tooling slice, and
  `xtask release-smoke` exercises local-only CLI surfaces.

## Intended Runtime Flow

1. `neo-agent` parses CLI arguments and loads configuration.
2. `neo-agent-core` opens or creates a session.
3. The runtime resolves a model provider from config and the production provider registry.
4. The agent loop sends a `neo_ai::ChatRequest` to a `ModelClient`.
5. Stream events are normalized as `AiStreamEvent` values.
6. Tool calls are authorized, executed, and returned as `ChatMessage::ToolResult`.
7. Reasoning events are preserved as thinking content instead of being mixed
   into plain assistant text.
8. Session events are persisted so `resume` can rebuild conversation and tool
   state from local JSONL history.

The current Rust surface implements all major components of this flow. See the
individual crate docs in `docs/` for module-by-module status.

## Design Principles

- Keep provider-specific code behind `ModelClient`.
- Keep model-facing tool schemas small and stable.
- Treat permissions and session persistence as runtime policy, not provider behavior.
- Keep permission modes (`ask`, `auto`, `yolo`) separate from development
  modes (`normal`, `plan`, `goal`).
- Prefer typed Rust interfaces first; add wire protocols such as MCP at the boundary.
- Keep hosted/cloud distribution, profile sync, and managed collaboration out
  of the supported local-agent surface until the product deliberately reopens
  those boundaries.

## Approval Scoping (NEO-30)

"Approve for this session" operates on three layers, each narrower and more
explicit than the last. Together they replace the old tool-name wildcard model
where approving one `Bash` command approved every future `Bash` call.

### Layer 1 — exact session key (in-memory)

`AgentConfig::session_approvals` is a `HashSet<SessionApprovalKey>`, not a set
of tool names. Each key is derived from the tool call's arguments:

- **Bash**: `SessionApprovalKey::Shell { workspace, cwd, command }` where
  `command` is the POSIX-tokenized argv (e.g. `["git", "status"]`). `git status`
  and `git log` produce different keys and never share a grant. Compound
  commands (`a && b`) that cannot be reduced to a single plain-word command are
  stored as an opaque `["__shell_script__", <exact text>]` key, so approving the
  whole line never implicitly approves one sub-command.
- **Write/Edit**: `SessionApprovalKey::FileWrite { workspace, path, operation }`.
  Approving `Write` for one file does not approve `Write` for another, and does
  not approve `Edit` for the same file.

A later request skips prompting only when *every* key in its scope is already
approved. `AllowForSession` with no derived scope degrades to `AllowOnce` — it
never creates a wildcard.

### Layer 2 — persistent prefix rules (on disk)

A separate, user-chosen mechanism: "Approve commands starting with `git`" is
stored as a `PrefixApprovalRule { prefix, label }` in
`~/.neo/approval_rules.json` and survives restarts. This is the correct home for
the `git *` use case — an explicit grant, not an accidental side effect of the
session cache. Loaded at startup via `load_prefix_approval_rules`, saved on
approval via `save_prefix_approval_rules`. A guard refuses empty prefixes that
would approve every command.

### Layer 3 — command safety classification

- `is_known_safe_command`: read-only commands (`cat`, `ls`, `git status`,
  `cargo test`, etc.) skip the prompt entirely in ask mode. For `git`, only read
  subcommands (`status`, `log`, `diff`, …) qualify — `git push`,
  `git reset --hard`, `git clean` still prompt.
- `command_might_be_dangerous`: `rm -rf`, `sudo`, `dd`, `mkfs`, `chmod`,
  `curl | sh`, etc. always force a prompt and are never offered a session or
  prefix scope, so the user re-reviews every time.

### UI labels

The modal and inline transcript show the exact cached target, not a generic
"Approve for this session":

- Session option: "Approve this exact command for this session" / "Approve
  writes to this file for this session".
- Prefix option: "Approve commands starting with git".
- Options that cannot be safely offered (dangerous commands, review transitions,
  generic tools) are omitted entirely so numeric shortcuts stay predictable.
