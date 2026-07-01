# NEO-46 MCP Add Single-Page Form

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current multi-step `PendingMcpAdd` wizard in the TUI `/mcp` add flow with a single-page form that collects all server fields at once, modeled on the existing `CustomRegistryImportState` two-field form.

**Architecture:** Add a new `McpAddFormState` dialog component in `neo-tui`. Keep transport selection as the existing `ChoicePicker` step. After the user selects a transport, `interactive.rs` opens one `McpAddFormState` overlay preconfigured for that transport. Form submission is handled in `interactive.rs` by calling the existing MCP config helpers in `crates/neo-agent/src/mcp_ops.rs` and `config.rs`.

**Tech Stack:** Rust 2024, `crossterm`-style Neo TUI overlays, `neo-tui` dialog components, existing MCP config helpers, `nextest` through `cargo nextest run`.

---

## Linear Context

- Linear: NEO-46
- Title: Replace MCP add wizard with a single-page form
- Priority: Medium
- Project: CLI Commands / TUI
- Team: Neo
- Label: Feature / UX

## Relationship To Other Plans

- Builds on `docs/superpowers/plans/2026-06-22-neo-32-mcp-slash-command-manager-handoff.md` which implemented the original `/mcp` manager and multi-step add wizard.
- OAuth for MCP remote servers is intentionally **out of scope** here; see `docs/superpowers/plans/2026-06-24-neo-47-oauth-authenticator.md`.

## User Request

The user wants the MCP add flow to stop paginating. After selecting stdio / HTTP / SSE, all remaining fields should be collected on one screen, similar to the existing `Import Custom Registry` dialog which collects URL and Bearer Token together.

User feedback also clarifies:

- Field switching uses `Tab` and `↑`/`↓`. Do **not** use `Shift+Tab` (bound to development-mode cycling).
- The first field should be labeled **Name**, not "Server id", even though the serialized config key remains `id`.
- Optional fields: `Env` for stdio; `Bearer Token` and `Headers` for HTTP/SSE.

## Current State

`/mcp` → `A` (Add) opens a transport choice picker. Selecting a transport currently runs a three-step wizard via `PendingMcpAdd` and repeated `TextInput` overlays:

1. Server id
2. Command (stdio) or URL (http/sse)
3. save

The wizard works but feels paginated and lacks context. The code lives in `crates/neo-agent/src/modes/interactive.rs` (`PendingMcpAdd`, `handle_mcp_choice_item`, `continue_mcp_add`, `open_mcp_input`).

## Desired State

`/mcp` → `A` (Add) still opens the transport choice picker. After selecting a transport, a single form overlay appears with all fields for that transport:

| Transport | Fields |
|-----------|--------|
| Local stdio | Name · Command · Env (optional) |
| Remote HTTP | Name · URL · Bearer Token (optional) · Headers (optional) |
| Remote SSE  | Name · URL · Bearer Token (optional) · Headers (optional) |

Navigation:

- `Tab` / `↑` / `↓` switches the active field.
- `Enter` submits the form.
- `Esc` cancels.
- Optional fields display a muted placeholder such as `(optional)` when empty.
- `Env` and `Headers` are key/value strings in the same format used by the CLI: `KEY=value`, one per line or comma-separated. Reuse existing parsing in `mcp_ops::key_value_pairs`.

## Tasks

### Task 1: Create `McpAddFormState` in `neo-tui`

- [ ] Create `crates/neo-tui/src/dialogs/mcp_add_form.rs`.
- [ ] Define:
  - `McpAddFormOptions { title, transport, theme }`
  - `McpAddFormResult { name, command, url, bearer_token, headers, env }`
  - `McpAddFormState` with per-field buffers and an `active_field` index.
- [ ] Implement rendering similar to `CustomRegistryImportState`:
  - Box border with title.
  - One label + value line per field.
  - Active field marked with `▸`.
  - Hint line: `Tab · ↑↓ switch · Enter submit · Esc cancel`.
- [ ] Implement input handling:
  - `Tab`, `SelectDown`, `SelectUp` switch fields.
  - `Insert(ch)` / `Paste` / `Backspace` edit the active field.
  - `Submit` returns `McpAddFormResult::Submitted`.
  - `Cancel` returns `McpAddFormResult::Cancelled`.
- [ ] Mask `Bearer Token` with `•` on screen, but keep the real value in state (same pattern as `CustomRegistryImportState`).
- [ ] Export types from `crates/neo-tui/src/dialogs/mod.rs`.

### Task 2: Wire form into `chrome.rs`

- [ ] Add `OverlayKind::McpAddForm(McpAddFormState)` to `crates/neo-tui/src/chrome.rs`.
- [ ] Add it to `focused_overlay_is_rich_dialog`, `focused_overlay_blocks_prompt`, and `overlay_mode`.
- [ ] Add `open_mcp_add_form(&mut self, opts: &McpAddFormOptions) -> OverlayId` helper.
- [ ] Add accessors for reading the form result (`mcp_add_form_result`).
- [ ] Add rendering/height dispatch.

### Task 3: Replace wizard in `interactive.rs`

- [ ] Remove `PendingMcpAdd`, `McpAddStep`, `continue_mcp_add`, and `open_mcp_input` usage for MCP add.
- [ ] In `handle_mcp_choice_item`, keep transport selection but open `McpAddFormState` instead of `TextInput`.
- [ ] Add `handle_mcp_add_form_result` (or extend `handle_choice_picker_result` path):
  - On submit, build `AddMcpServerInput` from form result + selected transport.
  - Call existing `mcp_ops::add_mcp_server_to_config` and `save_local_config`.
  - Trigger MCP manager config sync.
  - Refresh the MCP manager overlay or show a status message.
- [ ] On cancel, return to the MCP manager overlay or close.

### Task 4: Tests

- [ ] Add unit/rendering tests for `McpAddFormState` in `crates/neo-tui/src/dialogs/mcp_add_form.rs`:
  - Field switching via Tab and arrow keys.
  - Masked token rendering.
  - Submit returns correct result.
- [ ] Add integration test in `crates/neo-agent/src/modes/interactive.rs`:
  - `/mcp` → Add → select stdio → fill Name/Command/Env → submit → config contains new server.
  - Similar smoke tests for HTTP and SSE.
  - Verify the form is rendered in a single composed frame.

### Task 5: Docs

- [ ] Update `docs/mcp.md` TUI section to describe the single-page add form.
- [ ] Keep OAuth out of scope and reference the OAuth plan.

## Testing

- Ensure no regressions: `cargo nextest run -p neo-tui --lib`

## Notes / Constraints

- Do not change the serialized config schema (`id`, `transport`, `command`, `url`, `env`, `headers`, etc.). Only the user-facing label changes from "Server id" to "Name".
- Keep `Env` and `Headers` as optional key/value inputs; reuse `mcp_ops::key_value_pairs` for parsing.
- Do not introduce OAuth or hosted registry behavior in this plan.
- Keep `Shift+Tab` free for development-mode cycling; use only `Tab` and arrow keys for field navigation.
