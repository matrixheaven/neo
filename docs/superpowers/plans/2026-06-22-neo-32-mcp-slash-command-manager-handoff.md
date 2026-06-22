# NEO-32 /mcp Slash Command and MCP Manager Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local TUI `/mcp` slash command that opens an interactive MCP manager. The manager must show configured MCP servers and let the user add, test, enable/disable, delete, and refresh MCP entries without leaving the TUI.

**Architecture:** Keep MCP configuration and connection behavior in `neo-agent`, not `neo-tui`. Extract the existing CLI MCP logic from `crates/neo-agent/src/modes/run.rs` into a reusable MCP service module, then build a `neo-tui` overlay state machine modeled after `ProviderManagerState`. `interactive.rs` should become the coordinator: slash command -> open overlay -> process actions -> call MCP service/config functions -> refresh local config -> update the overlay.

**Tech Stack:** Rust 2024, `crossterm`-style Neo TUI overlays, `neo-tui` dialog components, `neo-agent` config helpers, existing MCP adapters from `neo-agent-core`, `tokio` for async tool discovery/probe, `nextest` through `cargo run -p xtask -- test`.

---

## Linear Context

- Linear: [NEO-32](https://linear.app/neo-agent/issue/NEO-32/implement-mcp-slash-command-and-interactive-mcp-manager)
- Title: Implement `/mcp` slash command and interactive MCP manager
- Priority: Medium
- Project: CLI Commands
- Team: Neo
- Label: Feature
- Blocked by: [NEO-17](https://linear.app/neo-agent/issue/NEO-17/improve-mcp-feature) for final live-status/runtime-manager integration.

## Relationship To NEO-17

NEO-32 is the TUI/product layer. NEO-17 is the runtime/service layer.

NEO-32 may prototype the overlay using static `AppConfig.mcp.servers` rows, but the finished implementation should consume the NEO-17 MCP manager APIs for live status, tool counts, reconnect/refresh actions, resource counts, diagnostics, and config hot reload. Do not duplicate connection lifecycle, retry, resource access, or dynamic tool discovery inside the TUI overlay.

Use this companion plan before implementing the final integration:

- `docs/superpowers/plans/2026-06-22-neo-17-mcp-runtime-reliability-handoff.md`

## User Request

The user wants a new task and a handoff plan for:

> 希望有一个 `/mcp` slash 命令，既可以看现在连了什么 mcp，又可以通过交互去添加 mcp，使用的交互页面可以模仿我们现有的 `/provider` 来写。需要完善的设计，包含字符画设计稿，并添加到 Linear 里。

The user explicitly wants a complete handoff document for another AI to implement. Do not treat this as a vague feature idea; it is an implementation-ready plan.

## Current State

Neo already has a CLI MCP surface:

- `neo mcp list`
- `neo mcp add <name> -t studio|remote-http|remote-sse ...`
- `neo mcp del <name>`
- `neo mcp enable <name>`
- `neo mcp disable <name>`

Neo also already has MCP runtime plumbing:

- Configured MCP servers live in global Neo config under `mcp.servers`.
- Enabled MCP servers are registered into `ToolRegistry`.
- MCP tools are advertised to the model as `mcp__<server_id>__<tool_name>`.
- Disabled MCP servers are not started.
- MCP resources are runtime state, not silently injected into model context.

What is missing:

- TUI `/mcp` slash command.
- TUI command palette entry for MCP.
- Interactive overlay to inspect/add/manage MCP servers.
- Shared service layer so CLI and TUI do not duplicate MCP parsing, persistence, probe, and list logic.

## Mandatory References

Read these before coding:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `docs/mcp.md`
- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/config.rs`
- `crates/neo-agent/src/modes/run.rs`
- `crates/neo-agent/src/modes/interactive.rs`
- `crates/neo-agent/src/config_ops.rs`
- `crates/neo-tui/src/dialogs/provider_manager.rs`
- `crates/neo-tui/src/dialogs/choice_picker.rs`
- `crates/neo-tui/src/chrome.rs`
- `crates/neo-tui/src/dialogs/mod.rs`

Run project recall first:

```bash
rtk icm recall-context "Neo /mcp slash command MCP manager provider overlay config mcp.servers" --limit 5
```

If ICM fails because the sandbox cannot open its database, continue and mention the failure in the final note. Do not block implementation on memory recall when the tool is unavailable.

## Non-Negotiable Project Rules

- Use `rtk` for shell commands.
- Prefer `cx` for symbol navigation before broad reads.
- Do not run bare `cargo test`; use `rtk cargo run -p xtask -- test ...`.
- Do not run broad workspace tests for this task unless the implementation becomes cross-cutting enough to warrant them.
- Do not perform git mutations unless the user gives explicit per-command authorization. This includes `git add`, `git commit`, `git push`, `git switch`, `git checkout`, `git reset`, `git stash`, `git clean`, `git rm`, `git merge`, and `git rebase`.
- Preserve unrelated worktree changes. This repository is shared by multiple AI agents.
- Config is global Neo config: `~/.neo/config.toml` or `$NEO_HOME/config.toml`. Do not invent project-local MCP config.
- Follow the blocking dialog contract: when a rich overlay is focused, hide the main prompt/composer and route text input to the overlay.

## Product Decisions

These decisions are part of this plan:

- `/mcp` is a local TUI command. It must not be submitted as a chat prompt.
- The first implementation supports configured endpoints only:
  - local stdio MCP, called `studio` in the CLI for compatibility
  - remote HTTP MCP
  - remote SSE MCP
- Hosted MCP registries, OAuth onboarding, hosted server lifecycle management, and provider-specific discovery remain out of scope. This matches `docs/mcp.md`.
- The manager should show configured servers immediately. Tool discovery may run lazily or on refresh/test, because discovering tools may start local processes or make network calls.
- Disabled servers must never be started for discovery.
- Secret values must not be displayed. Show env/header key names only, or a redacted count.
- Add flow may save without a successful test if the user chooses to save disabled, but the preferred path is test/probe before saving enabled.
- After saving/removing/toggling, refresh `local_config` so the next agent turn sees the changed MCP set.
- It is acceptable for a newly added MCP server to become available on the next turn rather than mutating an in-flight runtime's tool registry mid-turn.
- If a turn is actively running, `/mcp` should either be blocked with a status message or opened as view-only. The recommended first implementation is to block mutation while `active_turn.is_some()` and keep the behavior simple.

## Existing Code Map

### CLI and Config

- `crates/neo-agent/src/cli.rs`
  - `McpCommand` already models `List`, `Add`, `Del`, `Disable`, and `Enable`.
  - The add command already accepts transport, command, url, env, headers, cwd, enabled/disabled, enabled tools, disabled tools, startup timeout, and tool timeout.

- `crates/neo-agent/src/config.rs`
  - `McpConfig` and `McpServerConfig` are the persisted shapes.
  - `upsert_mcp_server(server, config_path)` writes or replaces a server.
  - `remove_mcp_server(server_id, config_path)` deletes a server.
  - `set_mcp_server_enabled(server_id, enabled, config_path)` toggles enablement.
  - `validate_mcp_server(server)` enforces non-empty ids, no `/` in ids, stdio command, and remote url.

- `crates/neo-agent/src/modes/run.rs`
  - `parse_mcp_kind` maps `studio` -> `stdio`, `remote-http` -> `http`, `remote-sse` -> `sse`.
  - `display_mcp_kind` maps stored transport back to CLI labels.
  - `parse_command_string` uses `shell_words::split`.
  - `list_mcp(config)` returns CLI-oriented text.
  - `add_mcp_server(...)` builds `McpServerConfig`, persists it, and probes if enabled.
  - `probe_mcp_server` and `list_mcp_tools_for_server` are currently private but useful for the TUI.

### TUI Provider Pattern

- `crates/neo-agent/src/modes/interactive.rs`
  - `handle_simple_slash_command` handles `/provider`.
  - `run_open_picker_command` handles command palette id `providers`.
  - `open_provider_picker` builds `ProviderManagerOptions`.
  - `process_provider_dialog_result` routes rich overlay results.
  - `handle_provider_manager_action` handles Add/Delete/Close.
  - Add provider flow uses `ChoicePicker`, API key input, catalog fetch, and config refresh.

- `crates/neo-tui/src/dialogs/provider_manager.rs`
  - Good model for `McpManagerState`.
  - It owns rows, selection, delete confirmation, action enum, render, and input handling.
  - It does not know how to edit config.

- `crates/neo-tui/src/chrome.rs`
  - Add an `OverlayKind` variant for any new rich dialog.
  - Wire open method, input routing, result accessor, render lines, and height.
  - `focused_overlay_is_rich_dialog` controls whether input is routed to dialog and prompt is hidden.

## UX Model

The interaction should feel like `/provider`: compact, keyboard-first, clear rows, and no explanatory prose inside the app beyond labels/hints. The user is trying to manage a tool surface, not read docs.

### Main `/mcp` Manager

Desktop-ish terminal at 92 columns:

```text
╭─ MCP Servers ─────────────────────────────────────────────────────────────╮
│ 3 configured · 2 enabled                                                   │
│ ↑↓ navigate · Enter details/test · A add · E enable/disable · D delete     │
│ R refresh tools · Esc close                                                │
│                                                                            │
│ ❯ ● filesystem        studio       enabled        tools: 12 discovered     │
│     command: npx -y @modelcontextprotocol/server-filesystem /repo          │
│     cwd: /Users/chenyuanhao/Workspace/neo                                  │
│                                                                            │
│   ● linear            remote-http  enabled        tools: 18 discovered     │
│     url: https://mcp.linear.app/mcp                                         │
│     headers: Authorization, X-Client-Version                                │
│                                                                            │
│   ◌ docs              remote-sse   disabled       tools: not discovered    │
│     url: https://example.invalid/mcp/sse                                    │
│                                                                            │
│   + Add MCP server                                                          │
╰────────────────────────────────────────────────────────────────────────────╯
```

Narrow terminal at around 60 columns:

```text
╭─ MCP Servers ─────────────────────────────────────╮
│ 3 configured · 2 enabled                           │
│ ↑↓ · Enter test · A add · E toggle · D delete      │
│ R refresh · Esc close                              │
│                                                    │
│ ❯ ● filesystem    studio       enabled             │
│     tools: 12 discovered                           │
│     npx -y @modelcontextprotocol/server-filesy…    │
│                                                    │
│   ● linear        remote-http  enabled             │
│     tools: 18 discovered                           │
│     https://mcp.linear.app/mcp                     │
│                                                    │
│   ◌ docs          remote-sse   disabled            │
│     tools: not discovered                          │
│                                                    │
│   + Add MCP server                                 │
╰────────────────────────────────────────────────────╯
```

Empty state:

```text
╭─ MCP Servers ─────────────────────────────────────╮
│ No MCP servers configured.                         │
│                                                    │
│ Add a server to expose external tools to Neo.      │
│                                                    │
│ ❯ + Add MCP server                                 │
│                                                    │
│ Enter add · Esc close                              │
╰────────────────────────────────────────────────────╯
```

### Detail/Test View

Enter on a configured server should open details or start a probe. The simplest implementation can make Enter run a probe and update the same row. A richer implementation can show this detail view:

```text
╭─ MCP: filesystem ──────────────────────────────────────────────────────────╮
│ Status: enabled · transport: studio                                        │
│ Command: npx -y @modelcontextprotocol/server-filesystem /repo              │
│ CWD: /Users/chenyuanhao/Workspace/neo                                      │
│ Env: NODE_OPTIONS, MCP_LOG_LEVEL                                           │
│ Tools: read_file, write_file, list_directory, search_files, ...            │
│                                                                            │
│ [T] Test connection   [E] Disable   [D] Delete   [Esc] Back                │
╰────────────────────────────────────────────────────────────────────────────╯
```

If the test is in progress:

```text
╭─ MCP: filesystem ──────────────────────────────────────────────────────────╮
│ Testing connection...                                                      │
│                                                                            │
│ Spinner should use the existing working label/footer pattern if available. │
╰────────────────────────────────────────────────────────────────────────────╯
```

If the test fails:

```text
╭─ MCP: filesystem ──────────────────────────────────────────────────────────╮
│ Status: connect failed                                                     │
│ Error: timeout connecting to MCP server filesystem                         │
│                                                                            │
│ [T] Retry   [E] Disable   [Esc] Back                                       │
╰────────────────────────────────────────────────────────────────────────────╯
```

### Add Flow: Transport Picker

Use existing `ChoicePicker` first:

```text
╭─ Add MCP Server ─────────────────────────────────────╮
│ ▸ Local stdio (studio) — run a command on this machine │
│   Remote HTTP          — JSON-RPC HTTP endpoint         │
│   Remote SSE           — JSON-RPC endpoint over SSE     │
╰──────────────────────────────────────────────────────╯
```

Choice ids:

- `mcp:add:stdio`
- `mcp:add:http`
- `mcp:add:sse`

### Add Flow: Stdio Form

Recommended form layout:

```text
╭─ Add MCP Server: Local stdio ──────────────────────────────────────────────╮
│ Server id                                                                 │
│ ❯ filesystem                                                              │
│                                                                            │
│ Command                                                                    │
│   npx -y @modelcontextprotocol/server-filesystem /repo                     │
│                                                                            │
│ Working directory                                                          │
│   /Users/chenyuanhao/Workspace/neo                                         │
│                                                                            │
│ Env (KEY=value, comma-separated)                                           │
│   NODE_OPTIONS=--max-old-space-size=4096,MCP_LOG_LEVEL=warn                │
│                                                                            │
│ Enabled: yes        Startup timeout: 5000 ms        Tool timeout: default  │
│                                                                            │
│ Ctrl+S test & save · Enter next field · Esc cancel                         │
╰────────────────────────────────────────────────────────────────────────────╯
```

If a full multi-field form is too much for the first patch, implement the add flow as a sequence of existing text-input dialogs. However, the preferred implementation is a dedicated form state because it is easier to validate and test as one component.

### Add Flow: Remote Form

```text
╭─ Add MCP Server: Remote HTTP ──────────────────────────────────────────────╮
│ Server id                                                                 │
│ ❯ linear                                                                  │
│                                                                            │
│ URL                                                                        │
│   https://mcp.linear.app/mcp                                               │
│                                                                            │
│ Headers (KEY=value, comma-separated; values hidden after submit)           │
│   Authorization=Bearer sk-...                                              │
│                                                                            │
│ Enabled: yes        Startup timeout: 5000 ms        Tool timeout: default  │
│                                                                            │
│ Ctrl+S test & save · Enter next field · Esc cancel                         │
╰────────────────────────────────────────────────────────────────────────────╯
```

After save, the manager row should render header values only as keys:

```text
headers: Authorization
```

Never render header/env values in the persisted manager list.

### Add Flow: Test Result / Save Confirm

Successful probe:

```text
╭─ MCP Connection Test ──────────────────────────────────────────────────────╮
│ linear connected successfully.                                             │
│                                                                            │
│ Discovered tools                                                           │
│  1. list_issues                                                            │
│  2. get_issue                                                              │
│  3. save_issue                                                             │
│  4. list_projects                                                          │
│  ...                                                                       │
│                                                                            │
│ [S] Save enabled   [D] Save disabled   [Esc] Cancel                        │
╰────────────────────────────────────────────────────────────────────────────╯
```

Failed probe:

```text
╭─ MCP Connection Test ──────────────────────────────────────────────────────╮
│ linear connect failed.                                                     │
│ Error: HTTP 401 from https://mcp.linear.app/mcp                            │
│                                                                            │
│ [B] Back edit   [D] Save disabled   [Esc] Cancel                           │
╰────────────────────────────────────────────────────────────────────────────╯
```

Do not force users to pass a probe before saving disabled. That lets users create entries they intend to fix later.

## Data Model Design

Add view-model types in `neo-tui` for rendering, not config persistence:

```rust
pub struct McpServerRow {
    pub id: String,
    pub transport_label: String,
    pub enabled: bool,
    pub endpoint_summary: String,
    pub cwd_summary: Option<String>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub tool_status: McpToolStatus,
}

pub enum McpToolStatus {
    NotDiscovered,
    Discovering,
    Discovered(Vec<String>),
    Failed(String),
}
```

Keep `McpServerConfig` in `neo-agent`. Convert config to `McpServerRow` inside `interactive.rs` or a new `mcp_ops` module, then pass rows to `neo_tui::dialogs::McpManagerOptions`.

Recommended action enum:

```rust
pub enum McpManagerAction {
    Add,
    Test(String),
    Refresh(String),
    ToggleEnabled(String),
    Delete(String),
    Close,
}
```

If detail view is included:

```rust
pub enum McpManagerAction {
    Add,
    OpenDetails(String),
    Test(String),
    Refresh(String),
    ToggleEnabled(String),
    Delete(String),
    Close,
}
```

## File Structure

Create:

- `crates/neo-agent/src/mcp_ops.rs`
  - Shared MCP command/service helpers for CLI and TUI.
- `crates/neo-tui/src/dialogs/mcp_manager.rs`
  - MCP manager overlay state and render/input tests.

Modify:

- `crates/neo-agent/src/main.rs` or module declarations if needed
  - Export/use `mcp_ops`.
- `crates/neo-agent/src/modes/run.rs`
  - Replace private MCP helper implementations with calls into `mcp_ops`.
- `crates/neo-agent/src/modes/interactive.rs`
  - Add `/mcp` slash command, command palette command, open/process handlers, pending async probe state, and config refresh behavior.
- `crates/neo-agent/src/cli.rs`
  - Ideally no shape changes; only use extracted functions.
- `crates/neo-agent/src/config.rs`
  - Only modify if validation needs to be shared or made public.
- `crates/neo-tui/src/dialogs/mod.rs`
  - Export `mcp_manager`.
- `crates/neo-tui/src/chrome.rs`
  - Add `OverlayKind::McpManager`, open method, action accessor, input routing, render, and height.
- `docs/mcp.md`
  - Document `/mcp`.
- `docs/quickstart.md` or `docs/index.md`
  - Add a short mention only if those docs already list interactive commands.

## Task 1: Extract Reusable MCP Operations

**Files:**

- Create: `crates/neo-agent/src/mcp_ops.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`
- Modify: module declaration file as needed.

- [ ] Move or copy these helpers from `run.rs` into `mcp_ops.rs`, then update `run.rs` to call the shared functions:
  - `parse_mcp_kind`
  - `display_mcp_kind`
  - `parse_command_string`
  - `list_mcp_tools_for_server`
  - `probe_mcp_server`
  - `apply_tool_filter`
  - `add_mcp_server`

- [ ] Keep the existing public CLI function names stable if other code calls them. If moving public functions would cause churn, have `run.rs` re-export thin wrappers.

- [ ] Add a structured summary function for the TUI:

```rust
pub async fn summarize_mcp_servers(config: &AppConfig) -> Vec<McpServerSummary>
```

Suggested shape:

```rust
pub struct McpServerSummary {
    pub id: String,
    pub transport: String,
    pub transport_label: String,
    pub enabled: bool,
    pub endpoint_summary: String,
    pub cwd: Option<PathBuf>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub tools: McpToolDiscovery,
}

pub enum McpToolDiscovery {
    SkippedDisabled,
    NotRequested,
    Success(Vec<String>),
    Failed(String),
}
```

- [ ] Make discovery optional. The manager needs an instant list view, and tool discovery can be slow or side-effectful. Prefer:

```rust
pub fn summarize_mcp_servers_without_discovery(config: &AppConfig) -> Vec<McpServerSummary>
pub async fn discover_mcp_tools(server: &McpServerConfig) -> anyhow::Result<Vec<String>>
```

- [ ] Add add/update input type so CLI and TUI share validation:

```rust
pub struct AddMcpServerInput {
    pub id: String,
    pub cli_type: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub env: Vec<String>,
    pub headers: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub enabled: bool,
}
```

- [ ] Have `add_mcp_server` accept `AddMcpServerInput` or have both CLI/TUI call a lower-level `build_mcp_server_config(input)` function. Avoid maintaining two argument-parsing paths.

- [ ] Add focused tests for parsing and redaction:

```bash
rtk cargo run -p xtask -- test -p neo-agent mcp_ops
```

Use unit tests in `mcp_ops.rs` where possible:

- `studio` maps to stored `stdio`.
- `remote-http` maps to stored `http`.
- `remote-sse` maps to stored `sse`.
- stdio requires command.
- remote requires url.
- remote rejects command.
- stdio rejects headers.
- summary exposes only env/header keys, never values.

## Task 2: Build `McpManagerState`

**Files:**

- Create: `crates/neo-tui/src/dialogs/mcp_manager.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`

- [ ] Use `provider_manager.rs` as the direct style guide. Keep state local and deterministic.

- [ ] Define options:

```rust
pub struct McpManagerOptions {
    pub servers: Vec<McpServerRow>,
    pub theme: TuiTheme,
}
```

- [ ] Define rows:

```rust
enum Row {
    Server { row: McpServerRow },
    Add,
}
```

- [ ] Define actions:

```rust
pub enum McpManagerAction {
    Add,
    Test(String),
    Refresh(String),
    ToggleEnabled(String),
    Delete(String),
    Close,
}
```

- [ ] Support keyboard input:
  - Up/down/page up/page down moves selection.
  - `Enter` on server -> `Test(id)` or details view.
  - `Enter` on add row -> `Add`.
  - `a`/`A` -> `Add`.
  - `e`/`E` on server -> `ToggleEnabled(id)`.
  - `d`/`D` arms delete confirmation for selected server.
  - `y`/`Y` confirms delete.
  - `n`/`N` or Esc cancels delete confirmation.
  - `r`/`R` on server -> `Refresh(id)`.
  - Esc without confirmation -> `Close`.

- [ ] Render rows with stable widths and truncation:
  - Marker column: selected pointer.
  - Status glyph/text: enabled/disabled.
  - Id.
  - Transport label.
  - Enabled state.
  - Tool status.
  - Endpoint summary line.
  - Optional `cwd`, `env`, `headers` detail line.

- [ ] Keep render readable at widths 48, 60, and 90+. Use `truncate_width` and `visible_width`.

- [ ] Avoid UI cards inside cards. This overlay itself is the framed dialog; rows should not be nested boxes.

- [ ] Export the types from `dialogs/mod.rs`.

- [ ] Add unit tests similar to `provider_manager.rs`:

```bash
rtk cargo run -p xtask -- test -p neo-tui mcp_manager
```

Required test cases:

- Renders title, server rows, and add row.
- Renders empty state.
- Renders enabled and disabled states.
- Redacts env/header values by construction.
- `A` returns Add action.
- Enter on add returns Add action.
- Enter on server returns Test action.
- `E` returns ToggleEnabled action.
- `R` returns Refresh action.
- `D` arms delete confirmation.
- `Y` returns Delete action.
- `N` cancels delete confirmation.
- Esc returns Close.
- `set_options` or equivalent preserves selection by server id after refresh.

## Task 3: Wire the Overlay Into `NeoChromeState`

**Files:**

- Modify: `crates/neo-tui/src/chrome.rs`

- [ ] Import/export the new dialog types.

- [ ] Add open method:

```rust
pub fn open_mcp_manager(&mut self, opts: &crate::dialogs::McpManagerOptions) -> OverlayId
```

- [ ] Add `OverlayKind::McpManager(crate::dialogs::McpManagerState)`.

- [ ] Include `McpManager` in:
  - `focused_overlay_is_rich_dialog`
  - `handle_focused_dialog_input`
  - `rich_dialog_lines`
  - `input_dialog_height` or a specific height function
  - selection/input fallback helpers near `handle_provider_choice_dialog_selection`

- [ ] Add result accessor:

```rust
pub fn mcp_manager_action(&self) -> Option<crate::dialogs::McpManagerAction>
```

- [ ] Use a height of 16 for parity with provider manager unless the rendered form requires a bigger height. Do not let the overlay push composer into a half-visible state.

- [ ] Add or extend chrome-level tests if there are existing overlay tests. If none exist, rely on dialog tests plus interactive tests.

## Task 4: Add `/mcp` Slash Command and Command Palette Entry

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Add `mcp` to command palette specs:

```rust
CommandSpec::new("mcp", "Open MCP servers", Some("Manage MCP servers"))
```

- [ ] Add prompt completion slash entry:

```rust
PickerItem::new(
    "/mcp",
    "/mcp",
    Some(prompt_source_description(
        Some("View and manage MCP servers"),
        Some("MCP manager"),
        Some("local"),
    )),
)
```

- [ ] Add command dispatch:

```rust
match command_id {
    "sessions" => self.open_session_picker(),
    "models" => self.open_model_picker(),
    "providers" => self.open_provider_picker(),
    "mcp" => self.open_mcp_manager(),
    _ => return false,
}
```

- [ ] Add slash handling:

```rust
match prompt {
    "/mcp" => self.open_mcp_manager(),
    ...
}
```

- [ ] Clear submitted prompt after `/mcp`, exactly like `/provider`.

- [ ] Do not accept arguments for first patch. `/mcp add` can be a later extension. If user types `/mcp something`, prefer showing `Unknown command` or treating only exact `/mcp` as local command.

- [ ] Add `open_mcp_manager`:

```rust
fn open_mcp_manager(&mut self) {
    let Some(config) = &self.local_config else {
        self.push_status("No config available");
        return;
    };
    let rows = mcp_rows_from_config(config);
    let theme = self.tui.chrome().theme();
    self.tui.chrome_mut().open_mcp_manager(&McpManagerOptions { servers: rows, theme });
}
```

- [ ] If `active_turn.is_some()`, block mutating actions. Recommended behavior:
  - Opening `/mcp` is allowed for view-only, or fully blocked.
  - Add/delete/toggle/test while active turn is running should be blocked with status:

```text
Cannot modify MCP servers while a turn is running. Press Esc to interrupt first.
```

The simpler first implementation is to block `/mcp` entirely during active turns.

## Task 5: Process MCP Manager Actions

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Add a new result routing function. Do not overload provider flow in a way that conflates choice picker ids.

Recommended structure:

```rust
async fn process_rich_dialog_result(&mut self, result: InputResult) -> Result<()> {
    if !dialog_result_may_close(result) {
        return Ok(());
    }
    if self.process_model_dialog_result() {
        return Ok(());
    }
    if self.process_provider_dialog_result().await {
        return Ok(());
    }
    if self.process_mcp_dialog_result().await {
        return Ok(());
    }
    self.process_question_dialog_result().await
}
```

If provider and MCP both use `ChoicePicker`, guard choice ids with prefixes:

- Provider: existing `known`, `custom`, `catalog:*`, `custom-catalog:*`.
- MCP: `mcp:add:stdio`, `mcp:add:http`, `mcp:add:sse`, `mcp:save:*`, etc.

- [ ] Add `handle_mcp_manager_action`:
  - `Close`: close overlay.
  - `Add`: close overlay and open transport picker.
  - `Test(id)`: start async probe/discovery for that server.
  - `Refresh(id)`: same as test, then update row tool status.
  - `ToggleEnabled(id)`: call `config::set_mcp_server_enabled`, refresh config, reopen/update manager.
  - `Delete(id)`: call `config::remove_mcp_server`, refresh config, reopen/update manager.

- [ ] After any config write, call existing `refresh_config()` so `local_config` matches disk.

- [ ] Reopen the MCP manager after refresh with preserved selected id if practical. If not practical in first patch, reset selection to the same row index.

- [ ] For async probe/discovery:
  - Use `tokio::spawn`.
  - Set `custom_working_label` like provider catalog fetch does.
  - Store a pending handle in `InteractiveApp`, e.g. `pending_mcp_probe: Option<PendingMcpProbe>`.
  - Poll it in the same loop area where `poll_pending_catalog_fetch` is called.

Suggested type:

```rust
struct PendingMcpProbe {
    server_id: String,
    handle: tokio::task::JoinHandle<anyhow::Result<Vec<String>>>,
}
```

- [ ] On success, update row status to `Discovered(tools)` and push status:

```text
MCP filesystem connected (12 tools)
```

- [ ] On failure, update row status to `Failed(error)` and push status:

```text
MCP filesystem connect failed: timeout connecting to MCP server filesystem
```

- [ ] Do not write probe errors into chat transcript as model context.

## Task 6: Implement Add MCP Flow

**Files:**

- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Create or modify TUI input dialog files if a dedicated form is chosen.

### Recommended Option A: Dedicated Form

Create:

- `crates/neo-tui/src/dialogs/mcp_server_form.rs`

Then wire it like `ApiKeyInput`/`CustomRegistryImport`.

Pros:

- Cleanest UX.
- Best for validation.
- Easier to test field navigation and redaction.

Cons:

- More code.

### Acceptable Option B: Sequence of Existing Dialogs

Use `ChoicePicker` plus one or more existing text input dialog primitives if available. If there is no generic text input dialog, do not abuse `ApiKeyInput` labels for non-key inputs; create a generic single-line input component first.

Pros:

- Smaller first patch.

Cons:

- More state in `interactive.rs`.
- Harder to edit previous fields.

### Required Fields

For all transports:

- Server id
- Enabled: yes/no
- Enabled tools allowlist, optional comma-separated
- Disabled tools blocklist, optional comma-separated
- Startup timeout ms, optional
- Tool timeout ms, optional

For stdio/studio:

- Command string, parsed with the existing `shell_words` behavior.
- Optional cwd.
- Optional env pairs as `KEY=value`.

For remote HTTP/SSE:

- URL.
- Optional headers as `KEY=value`.

### Validation Rules

Reuse existing config and CLI validation:

- id cannot be empty.
- id cannot contain `/`.
- transport must be `stdio`, `http`, or `sse`.
- stdio requires command.
- remote HTTP/SSE requires url.
- stdio rejects headers.
- remote HTTP/SSE rejects command/cwd.
- env/header input must parse as key-value pairs.
- timeout values must parse as positive integers if provided.

### Save Behavior

- Build `AddMcpServerInput`.
- Convert to `McpServerConfig` through `mcp_ops`.
- Call `config::upsert_mcp_server`.
- If saved enabled, run probe and report result.
- Refresh config and reopen manager.

## Task 7: Keep CLI Behavior Stable

**Files:**

- Modify: `crates/neo-agent/src/modes/run.rs`

- [ ] Ensure `neo mcp list` output stays unchanged unless deliberately improved with test updates.

- [ ] Ensure CLI add still returns:
  - `<name> successfully connected!` on probe success.
  - `<name> connect failed` on probe failure.
  - `<name> added (disabled)` when disabled.

- [ ] Ensure existing CLI flags still map exactly:
  - `-t studio` -> stored `transport = "stdio"`.
  - `-t remote-http` -> stored `transport = "http"`.
  - `-t remote-sse` -> stored `transport = "sse"`.

- [ ] Add focused CLI regression tests if current tests exist for `neo mcp`; otherwise test shared parser/build functions.

## Task 8: Documentation

**Files:**

- Modify: `docs/mcp.md`
- Optionally modify: `docs/quickstart.md`, `docs/index.md`

- [ ] Add a short section:

```markdown
## TUI MCP Manager

In interactive TUI mode, use `/mcp` to open the MCP manager. The manager lists configured servers, shows enabled/disabled state, lets you test discovery, and can add local stdio or remote HTTP/SSE servers interactively.
```

- [ ] Mention that `/mcp` writes to the same global config as `neo mcp`.

- [ ] Mention secret redaction:

```markdown
Environment variables and headers can be configured, but the TUI list shows only key names, not values.
```

- [ ] Keep hosted registries/OAuth out of scope exactly as `docs/mcp.md` currently states.

## Task 9: Tests

Run only focused tests unless the implementation touches broader runtime code.

Recommended commands:

```bash
rtk cargo run -p xtask -- test -p neo-tui mcp_manager
rtk cargo run -p xtask -- test -p neo-agent mcp
rtk cargo fmt --all --check
```

If you create a new generic form component:

```bash
rtk cargo run -p xtask -- test -p neo-tui mcp_server_form
```

If you touch runtime MCP registration:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core mcp
```

Do not run full CI for this feature unless you significantly refactor MCP runtime or config loading.

## Edge Cases and Pitfalls

- **Do not auto-start disabled MCP servers.** Even discovery/listing must skip disabled rows.
- **Do not discover tools for every server on every render.** Rendering must be pure and fast.
- **Do not leak secrets.** Env/header values must not render in rows, details, status messages, debug-like strings, test snapshots, or docs examples.
- **Do not duplicate CLI parsing.** If TUI splits command strings differently from CLI, users will get inconsistent behavior.
- **Do not mutate an active turn's tool registry mid-turn.** Save config and let the next turn pick it up.
- **Do not write project-local config.** MCP config is global Neo config.
- **Do not treat `studio` as persisted transport.** Persisted value is `stdio`; `studio` is a CLI/user-facing label.
- **Do not turn MCP resources into transcript context.** `docs/mcp.md` says resources are runtime state, not model context.
- **Do not swallow probe errors completely.** The UI should show a concise failure reason, while avoiding sensitive header/env values.
- **Do not leave focused overlay input leaking into the main prompt.** Add tests if there is an existing pattern for prompt hiding/input routing.
- **Do not use a wide-only layout.** Check rendering at narrow widths.
- **Do not make `/mcp` a text output command only.** The user explicitly asked for an interactive page modeled on `/provider`.
- **Do not implement hosted registry/OAuth now.** This would expand the product surface and fight the local-only architecture.

## Self Review Checklist

Before final handoff/PR, verify:

- [ ] `/mcp` exact prompt opens the manager and clears the prompt.
- [ ] `/mcp` does not start a model turn.
- [ ] Command palette can open MCP manager.
- [ ] Empty state renders well.
- [ ] Existing servers render with id, transport, enabled state, and endpoint summary.
- [ ] Disabled servers are visually distinct and are not probed.
- [ ] Env/header values are redacted everywhere in UI and tests.
- [ ] Add stdio flow saves a config entry with `transport = "stdio"`.
- [ ] Add HTTP flow saves `transport = "http"` with url and headers.
- [ ] Add SSE flow saves `transport = "sse"` with url and headers.
- [ ] Enable/disable writes to config and refreshes the manager.
- [ ] Delete requires confirmation and writes to config.
- [ ] Refresh/test handles success and failure.
- [ ] Config refresh updates `local_config`.
- [ ] Next agent turn can see newly saved enabled MCP servers.
- [ ] CLI `neo mcp` behavior remains compatible.
- [ ] Focused tests pass through `xtask`.
- [ ] Formatting check passes.
- [ ] No unrelated files were edited.

## Suggested Implementation Order

1. Extract MCP shared operations into `mcp_ops.rs`.
2. Add parser/summary tests.
3. Build and test `McpManagerState` in `neo-tui`.
4. Wire `McpManagerState` into `NeoChromeState`.
5. Add `/mcp` slash and command palette entry.
6. Handle basic manager actions: close, add transport picker, delete, enable/disable.
7. Add probe/refresh async flow.
8. Add add-server form or input sequence.
9. Update docs.
10. Run focused tests and self review.

## Implementation Notes for the Next AI

The fastest safe path is to keep the first UI iteration close to `ProviderManagerState`:

- One overlay for listing and actions.
- One `ChoicePicker` for choosing transport.
- One dedicated form for add inputs if time allows.
- Async probe state in `interactive.rs`, copied from pending catalog fetch style.

Do not overbuild a full MCP marketplace. The value here is immediate local ergonomics: users can inspect and add explicit MCP endpoints without leaving Neo.
