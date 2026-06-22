# NEO-35 Neo Extensions Host Plugin API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign Neo extensions from a local JSONL tool skeleton into a pi-style host plugin API with events, commands, TUI primitives, and Neo-native tools, while keeping MCP as the external tool/server protocol boundary.

**Architecture:** Build a first-class extension subsystem in `neo-agent-core` around an `ExtensionManager`, a stable process/JSON-RPC host protocol, explicit capability grants, and a contribution registry. The product API should feel like `docs/pi` host plugins, but the runtime should remain language-agnostic and process-isolated rather than embedding a JS/TS module loader.

**Tech Stack:** Rust 2024, `tokio`, JSONL/JSON-RPC primitives from `neo_agent_core::rpc`, existing Neo TUI dialog infrastructure, `serde`/`schemars` for wire types, `crossterm` input handling, and focused `nextest` runs through `cargo run -p xtask -- test`.

---

## Linear Context

- Linear: [NEO-35](https://linear.app/neo-agent/issue/NEO-35/design-a-useful-neo-extensions-system-distinct-from-mcp)
- Title: Design a useful Neo extensions system distinct from MCP
- Priority: Medium
- Project: Infrastructure
- Team: Neo
- Label: Feature

## Interview Decisions

The user intentionally chose a broad host-plugin direction:

- Neo extensions should reach the level of `docs/pi`'s complete host plugin API.
- Extensions must not remain a second flavor of MCP tools.
- MCP remains the external tool/server protocol boundary.
- Extensions become Neo-native host integration: lifecycle events, commands, TUI UI, status, host-context tools, and workflow customization.
- Product feel can borrow from pi, but runtime should be stable, language-agnostic, and process-isolated.
- Do not directly embed JS/TS extension modules in Phase 1.
- Do not expose debug-console-like CLI surfaces.
- Do not add `inspect`.
- Remove the old `neo extensions call ...` CLI surface; it is skeleton-stage raw RPC residue.
- Keep a single `doctor` command for health checks.
- Do not preserve obsolete compatibility paths when migrating the skeleton.

## Phase 1 Scope

Phase 1 should deliver a useful host plugin MVP:

1. Event hooks for lifecycle observation.
2. Slash commands, shortcuts, and CLI flags contributed by extensions.
3. TUI UI API primitives: select, input, confirm, notify, status/footer/header slots, editor widgets, and autocomplete.
4. Neo-native model-callable tools with bounded host context and normal Neo approval behavior.
5. `/extensions` TUI management UI for discovery, capability visibility, enable/disable, details, contribution lists, and recent errors.
6. Product-level CLI management: `list`, `install`, `update`, `uninstall`, `enable`, `disable`, and `doctor`.

## Deferred Scope

These surfaces are valuable but should not be opened before the core lifecycle and trust model are stable:

- Provider/model/OAuth registration.
- System prompt and context injection.
- Permission policy mutation.
- Security policy plugins.
- Advanced custom transcript renderers beyond simple tool/status contribution display.
- Hosted marketplace, package signing, or remote discovery.

## Extension vs MCP Boundary

Use this distinction everywhere in implementation and docs:

| Surface | MCP | Neo Extensions |
| --- | --- | --- |
| Primary job | External tool/resource protocol | Neo host plugin API |
| Process model | External server speaking MCP | Local extension process speaking Neo host protocol |
| Model tools | Yes, through `mcp__server__tool` | Yes, through `extension__id__tool`, subject to Neo policy |
| UI integration | No direct Neo UI control | Yes, through bounded TUI host APIs |
| Slash commands | No | Yes |
| Keybindings | No | Yes |
| Session lifecycle hooks | No | Yes |
| Provider registration | Not Phase 1 | Phase 2 candidate |
| Trust boundary | Server config plus tool approval | Workspace trust, manifest capabilities, enable confirmation, tool approval |

## Mandatory References

Read these before coding:

- `AGENTS.md`
- `~/.codex/RTK.md`
- `~/.codex/CX.md`
- `docs/pi/packages/coding-agent/src/core/extensions/types.ts`
- `docs/pi/packages/coding-agent/src/core/extensions/loader.ts`
- `docs/pi/packages/coding-agent/src/core/extensions/runner.ts`
- `docs/codex/docs/sandbox.md`
- `docs/codex/SECURITY.md`
- `crates/neo-agent-core/src/tools/extensions/discovery.rs`
- `crates/neo-agent-core/src/tools/extensions/runner.rs`
- `crates/neo-agent-core/src/tools/extensions/bridge.rs`
- `crates/neo-agent-core/src/tools/extensions/installation.rs`
- `crates/neo-agent-core/src/tools/extensions/lifecycle.rs`
- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/extension_commands.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-agent/src/modes/interactive.rs`
- `crates/neo-tui/src/dialogs/provider_manager.rs`
- `crates/neo-tui/src/dialogs/choice_picker.rs`
- `crates/neo-tui/src/dialogs/mod.rs`
- `docs/packages.md`
- `docs/quickstart.md`
- `crates/neo-agent/tests/cli_commands.rs`
- `crates/neo-agent/tests/mock_provider_e2e.rs`
- `crates/neo-agent-core/tests/extension_runner.rs`

Run recall first:

```bash
rtk icm recall-context "NEO-35 Neo extensions host plugin API pi MCP trust TUI" --limit 5
```

If recall fails because the local database is unavailable, continue and mention the failure in the implementation note.

## Current State

Neo currently has a narrow skeleton:

- `ExtensionManifest` lives in `crates/neo-agent-core/src/tools/extensions/discovery.rs`.
- Manifest fields are only `id`, `name`, `version`, optional `description`, and `runner`.
- Only stdio transport exists.
- `ExtensionRunner` in `runner.rs` spawns a child process, sends one request, reads one response, ignores notifications, and rejects extension-originated requests.
- `bridge.rs` discovers enabled extensions by calling `tools.list`.
- Extension tools are advertised as `extension__<extension>__<tool>`.
- Tool execution spawns the extension again and calls the declared method.
- `installation.rs` copies local extension directories into the extension root and records sources.
- `lifecycle.rs` tracks enabled/disabled state.
- `crates/neo-agent/src/extension_commands.rs` exposes list/install/update/uninstall/status/enable/disable/call.
- `extensions call` is raw JSON-RPC invocation and must be removed.
- Current docs still describe extensions mostly as local assets and tool RPC.

This is useful as scaffolding, but it is not a host plugin API.

## Target Product Shape

Neo extensions should feel like local product capabilities:

- A user installs or enables an extension.
- Neo shows what capabilities the extension asks for.
- The user can see what the extension contributes: commands, shortcuts, tools, hooks, UI status items.
- Extensions can participate in the Neo TUI without needing to fake a model tool.
- Extensions can register model-callable tools when the feature really is a tool.
- Extension tools still respect active permission mode and approval prompts.
- Project extensions do not auto-load when the workspace is untrusted.
- User extensions may load globally, but dangerous work still goes through Neo policy.
- Health and installation issues are visible through `/extensions` and `neo extensions doctor`.

## Security Model

The security model is part of the feature, not an afterthought.

1. Project extensions are gated by workspace trust.
2. User extensions may load, but their dangerous actions still go through the active permission mode.
3. Extension manifests must declare capabilities.
4. First enable shows capability summary.
5. Extension tools must not bypass Neo tool approval.
6. Phase 1 must not let extensions mutate permission policy.
7. Transcript, tool arguments, extension output, and extension errors are untrusted data.
8. Host requests from extension processes must be validated against granted capabilities.
9. Extension processes must not get broad host context by default.
10. Extension failures must isolate to the extension and must not crash the agent loop.

Capability names for Phase 1:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCapability {
    Tools,
    Commands,
    Shortcuts,
    CliFlags,
    Ui,
    Events,
    HostContext,
    Filesystem,
    Shell,
    Network,
}
```

Capability behavior:

- `tools`: extension may register model-callable tools.
- `commands`: extension may register slash commands.
- `shortcuts`: extension may register TUI shortcuts.
- `cli_flags`: extension may declare extension-owned CLI flags.
- `ui`: extension may call bounded TUI UI requests.
- `events`: extension may subscribe to host lifecycle events.
- `host_context`: extension may receive bounded context snapshots.
- `filesystem`: extension declares it may touch local files through its own process.
- `shell`: extension declares it may spawn shell commands through its own process.
- `network`: extension declares it may perform network I/O through its own process.

Neo cannot fully sandbox arbitrary child processes in Phase 1, so capabilities are disclosure and host-API gating. Do not claim they are OS-level enforcement unless a real sandbox is added.

## Proposed File Structure

Create a first-class extension module and migrate the old `tools/extensions` skeleton into it. The old location should not remain the source of truth.

| Path | Responsibility |
| --- | --- |
| `crates/neo-agent-core/src/extensions/mod.rs` | Public extension subsystem exports. |
| `crates/neo-agent-core/src/extensions/manifest.rs` | Manifest v2 schema, capability declarations, discovery metadata. |
| `crates/neo-agent-core/src/extensions/protocol.rs` | JSON-RPC method names, request/response structs, host/extension wire contracts. |
| `crates/neo-agent-core/src/extensions/process.rs` | Long-lived child process transport, bidirectional JSONL RPC pump, shutdown. |
| `crates/neo-agent-core/src/extensions/manager.rs` | `ExtensionManager`, lifecycle, contribution registry, reload, status snapshots. |
| `crates/neo-agent-core/src/extensions/contributions.rs` | Tool, command, shortcut, flag, UI, and hook contribution structs. |
| `crates/neo-agent-core/src/extensions/events.rs` | Event names and payload structs. |
| `crates/neo-agent-core/src/extensions/context.rs` | Bounded host context snapshots for extension calls. |
| `crates/neo-agent-core/src/extensions/doctor.rs` | Health checks, diagnostics, manifest validation, startup probe. |
| `crates/neo-agent-core/src/extensions/install.rs` | Local install/update/uninstall logic migrated from `installation.rs`. |
| `crates/neo-agent-core/src/extensions/lifecycle.rs` | Enable/disable state and capability grant state. |
| `crates/neo-agent-core/src/tools/extensions/` | Remove after migration, or leave only a private transition module inside the same task while deleting public exports before completion. |
| `crates/neo-agent/src/extension_commands.rs` | Product-level CLI commands only; remove `call`; add `doctor`. |
| `crates/neo-agent/src/cli.rs` | Update `ExtensionCommand` variants; remove `Call`; remove CLI names that feel like internal debug surfaces. |
| `crates/neo-agent/src/main.rs` | Dispatch new command set; wire manager into runtime/TUI setup. |
| `crates/neo-agent/src/modes/interactive.rs` | Slash command `/extensions`, manager action handling, extension command invocation. |
| `crates/neo-tui/src/dialogs/extension_manager.rs` | `/extensions` overlay state and rendering. |
| `crates/neo-tui/src/dialogs/mod.rs` | Export extension manager dialog. |
| `docs/extensions.md` | New user-facing extension concept and API documentation. |
| `docs/packages.md` | Replace skeleton extension docs with product-level package docs. |
| `docs/quickstart.md` | Update extension commands and remove `extensions call`. |
| `examples/extensions/` | Minimal host-plugin examples. |

## Manifest V2 Shape

Use a clear manifest instead of overloading `runner`.

Example:

```toml
schema_version = 2
id = "release-helper"
name = "Release Helper"
version = "0.1.0"
description = "Adds release workflow commands and status UI."

[process]
transport = "stdio"
command = "python3"
args = ["extension.py"]

[capabilities]
tools = true
commands = true
shortcuts = true
ui = true
events = true
host_context = true
filesystem = true
shell = false
network = false

[permissions.filesystem]
read = ["."]
write = ["CHANGELOG.md", "docs/"]
```

Rust shape:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub process: ExtensionProcessManifest,
    #[serde(default)]
    pub capabilities: ExtensionCapabilities,
    #[serde(default)]
    pub permissions: ExtensionPermissionDeclaration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionProcessManifest {
    pub transport: ExtensionTransportKind,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<ExtensionEnv>,
}
```

Migration rule:

- Do not keep old manifest v1 as an equal path.
- The task may include a one-time parser error with a helpful message:
  `extension manifest schema_version is missing; Neo extensions now require schema_version = 2`.
- Do not silently accept old `runner` manifests after this feature lands.

## Runtime Protocol

The extension process should be long-lived while enabled and healthy. The host and extension communicate over JSONL RPC using the existing `neo_agent_core::rpc` primitives.

### Host To Extension

```text
extension.initialize
extension.shutdown
extension.event
extension.command.execute
extension.shortcut.execute
extension.tool.execute
extension.flag.changed
```

### Extension To Host

```text
host.ui.select
host.ui.confirm
host.ui.input
host.ui.notify
host.ui.set_status
host.ui.set_widget
host.autocomplete.register
host.command.register
host.shortcut.register
host.tool.register
host.log
host.get_context
```

### Initialize Flow

1. Host discovers manifest.
2. Host checks workspace trust and extension enablement.
3. Host computes granted capabilities.
4. Host starts the extension process.
5. Host sends `extension.initialize`.
6. Extension returns static contributions.
7. Host validates contributions against granted capabilities.
8. Host registers accepted contributions.
9. Host updates `/extensions` status row.

Initialize params:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionInitializeParams {
    pub protocol_version: u32,
    pub extension_id: String,
    pub mode: ExtensionHostMode,
    pub cwd: PathBuf,
    pub workspace_trusted: bool,
    pub granted_capabilities: BTreeSet<ExtensionCapability>,
    pub context: ExtensionContextSnapshot,
}
```

Initialize result:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionInitializeResult {
    #[serde(default)]
    pub tools: Vec<ExtensionToolContribution>,
    #[serde(default)]
    pub commands: Vec<ExtensionCommandContribution>,
    #[serde(default)]
    pub shortcuts: Vec<ExtensionShortcutContribution>,
    #[serde(default)]
    pub cli_flags: Vec<ExtensionCliFlagContribution>,
    #[serde(default)]
    pub event_subscriptions: Vec<ExtensionEventSubscription>,
    #[serde(default)]
    pub ui_contributions: Vec<ExtensionUiContribution>,
}
```

Validation rules:

- If manifest lacks `tools`, reject returned tools.
- If manifest lacks `commands`, reject returned commands.
- If manifest lacks `ui`, reject extension-originated `host.ui.*`.
- If manifest lacks `events`, ignore event subscriptions.
- Duplicate contribution names are errors for that extension.
- Cross-extension duplicate slash command names are errors unless the manager has a deterministic conflict policy.
- Contributions from unhealthy extensions are removed from active registries.

## Bounded Host Context

Do not copy pi's entire in-process context into a child process. Provide snapshots.

Phase 1 context:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionContextSnapshot {
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub permission_mode: String,
    pub is_idle: bool,
    pub workspace_trusted: bool,
    pub context_usage: Option<ExtensionContextUsage>,
}
```

Do not include:

- API keys.
- Full config with secrets.
- Provider registry internals.
- Raw system prompt by default.
- Full transcript by default.
- Permission policy mutation handles.

If a future phase adds prompt/context hooks, it must use explicit capabilities and separate review.

## Event Hooks

Start with observational events and command/tool execution events. Avoid veto/mutation hooks in Phase 1.

Recommended Phase 1 event names:

```text
session.start
session.end
session.switch
turn.start
turn.end
message.start
message.delta
message.end
tool.start
tool.update
tool.end
extension.reload
```

Event payload example:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionEvent {
    pub name: String,
    pub timestamp_ms: u64,
    pub context: ExtensionContextSnapshot,
    #[serde(default)]
    pub payload: serde_json::Value,
}
```

Rules:

- Event handlers are best-effort.
- A failed observational event marks the extension degraded but does not fail the Neo turn.
- Events that require direct user interaction must go through `host.ui.*`.
- Do not implement permission-policy hooks in Phase 1.

## Neo-Native Tools

Extension tools are not MCP tools.

They are Neo host tools contributed by local extensions:

- Namespaced as `extension__<extension_id>__<tool_name>`.
- Displayed in `/extensions`.
- Registered in `ToolRegistry`.
- Executed through the extension manager.
- Subject to the same approval flow as other Neo tools.
- Disabled when the extension is disabled or unhealthy.

Contribution shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionToolContribution {
    pub name: String,
    pub label: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    #[serde(default)]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub execution_mode: ExtensionToolExecutionMode,
}
```

Execution shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionToolExecuteParams {
    pub tool_call_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub context: ExtensionContextSnapshot,
}
```

Tool result shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionToolExecuteResult {
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    #[serde(default)]
    pub terminate: bool,
}
```

Do not keep `tools.list` as the primary discovery protocol. It may inspire the new initialize result, but new code should not preserve the old method as an equal runtime path.

## Commands, Shortcuts, And Flags

Extension commands:

- Register slash commands with name, description, optional argument completion, and handler method.
- Commands run with `ExtensionCommandContext` snapshot plus allowed host actions.
- Commands can use `host.ui.*` if `ui` is granted.
- Commands should appear in slash completion.
- Disable commands when the extension is disabled or unhealthy.

Contribution shape:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionCommandContribution {
    pub name: String,
    pub description: String,
    pub handler: String,
    #[serde(default)]
    pub argument_completion: Option<String>,
}
```

Shortcut contribution:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionShortcutContribution {
    pub key: String,
    pub description: String,
    pub handler: String,
}
```

Flag contribution:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionCliFlagContribution {
    pub name: String,
    pub description: String,
    pub value_type: ExtensionFlagType,
    #[serde(default)]
    pub default_value: Option<serde_json::Value>,
}
```

Collision rules:

- Built-in slash commands win over extension commands.
- Extension command conflicts are shown as extension errors.
- The `/extensions` UI should show disabled contributions for conflicted commands.
- Shortcuts that conflict with built-ins should be rejected unless Neo has an existing override policy.

## TUI `/extensions` Design

The TUI should feel like a product management surface, not a debug panel.

### Main View

```text
╭─ Extensions ─────────────────────────────────────────────────────────────╮
│ Search  release_                                                        │
├──────────────────────────────────────┬──────────────────────────────────┤
│ ● release-helper        enabled       │ Release Helper                   │
│   0.1.0  user extension              │ Adds release workflow commands.   │
│                                      │                                  │
│ ● pr-review             enabled       │ Status        enabled            │
│   0.3.2  project extension           │ Trust         user                │
│                                      │ Health        ok                  │
│ ○ jira-workflows        disabled      │ Capabilities  tools commands ui  │
│   0.2.0  project extension           │               events context      │
│                                      │                                  │
│ ! stale-demo            error         │ Contributions                    │
│   0.1.0  user extension              │   Tools       2                  │
│                                      │   Commands    /release /changelog │
│                                      │   Shortcuts   ctrl+alt+r          │
│                                      │   Hooks       turn.start tool.end │
├──────────────────────────────────────┴──────────────────────────────────┤
│ Enter details   e enable/disable   d doctor   r reload   q close        │
╰──────────────────────────────────────────────────────────────────────────╯
```

Notes:

- The right pane uses normal product words: status, health, capabilities, contributions.
- Do not label the action or pane `inspect`.
- Show errors as `error`, `degraded`, or `disabled`.
- Use `doctor`, not raw method calls.

### Details View

```text
╭─ Extension Details: release-helper ──────────────────────────────────────╮
│ Status       enabled                                                     │
│ Version      0.1.0                                                       │
│ Source       ~/.neo/extensions/release-helper                            │
│ Trust        user extension                                              │
│ Health       ok                                                          │
├─ Capabilities ───────────────────────────────────────────────────────────┤
│ [tools] [commands] [shortcuts] [ui] [events] [host_context]              │
├─ Tools ──────────────────────────────────────────────────────────────────┤
│ extension__release_helper__draft_changelog                               │
│   Draft changelog sections from the current branch.                      │
│ extension__release_helper__prepare_release                               │
│   Prepare release checklist and update notes.                            │
├─ Commands ───────────────────────────────────────────────────────────────┤
│ /release       Open release workflow                                     │
│ /changelog     Draft changelog from commits                              │
├─ Hooks ──────────────────────────────────────────────────────────────────┤
│ turn.start      update footer status                                     │
│ tool.end        collect release-relevant edits                           │
├─ Recent Errors ──────────────────────────────────────────────────────────┤
│ No recent errors.                                                        │
├──────────────────────────────────────────────────────────────────────────┤
│ e disable   d doctor   o open source folder   esc back                   │
╰──────────────────────────────────────────────────────────────────────────╯
```

### Enable Confirmation

```text
╭─ Enable Extension ───────────────────────────────────────────────────────╮
│ release-helper wants to add Neo host capabilities.                       │
│                                                                          │
│ Capabilities                                                             │
│   tools          registers model-callable Neo tools                      │
│   commands       adds slash commands                                     │
│   ui             can show dialogs and status UI                          │
│   events         receives lifecycle events                               │
│   host_context   receives bounded session/workspace context              │
│                                                                          │
│ Risk                                                                     │
│   This extension runs as a local process. Its tools still require Neo     │
│   approval when they perform protected actions.                          │
│                                                                          │
│ [Enable]  [Cancel]                                                       │
╰──────────────────────────────────────────────────────────────────────────╯
```

### Doctor View

```text
╭─ Extension Doctor: release-helper ───────────────────────────────────────╮
│ Manifest             ok                                                  │
│ Capabilities          ok                                                  │
│ Process startup       ok  pid 42812                                      │
│ Initialize protocol   ok  version 1                                      │
│ Contributions         ok  2 tools, 2 commands, 2 hooks                   │
│ Conflicts             ok                                                  │
│ Recent stderr         empty                                               │
│ Last error            none                                                │
├──────────────────────────────────────────────────────────────────────────┤
│ r rerun doctor   esc back                                                │
╰──────────────────────────────────────────────────────────────────────────╯
```

## CLI UX

Keep CLI product-level:

```bash
neo extensions list
neo extensions install path/to/extension
neo extensions update release-helper
neo extensions uninstall release-helper
neo extensions enable release-helper
neo extensions disable release-helper
neo extensions doctor
neo extensions doctor release-helper
```

Remove:

```bash
neo extensions call ...
```

Do not add:

```bash
neo extensions inspect ...
```

If the current `status` command remains during migration, fold its useful output into `list` and `doctor`, then remove `status` from the final Phase 1 CLI surface. The TUI details view is the product place for per-extension detail.

## Task 1: Create First-Class Extension Domain Module

**Files:**

- Create: `crates/neo-agent-core/src/extensions/mod.rs`
- Create: `crates/neo-agent-core/src/extensions/manifest.rs`
- Create: `crates/neo-agent-core/src/extensions/contributions.rs`
- Create: `crates/neo-agent-core/src/extensions/context.rs`
- Modify: `crates/neo-agent-core/src/lib.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Remove or migrate: `crates/neo-agent-core/src/tools/extensions/discovery.rs`

- [ ] Step 1: Add manifest parsing tests in `crates/neo-agent-core/src/extensions/manifest.rs`.

```rust
#[test]
fn parses_manifest_v2_with_capabilities() {
    let manifest: ExtensionManifest = toml::from_str(
        r#"
schema_version = 2
id = "release-helper"
name = "Release Helper"
version = "0.1.0"
description = "Release workflow helpers"

[process]
transport = "stdio"
command = "python3"
args = ["extension.py"]

[capabilities]
tools = true
commands = true
ui = true
events = true
host_context = true
"#,
    )
    .expect("manifest parses");

    assert_eq!(manifest.id, "release-helper");
    assert!(manifest.capabilities.tools);
    assert!(manifest.capabilities.commands);
    assert!(manifest.capabilities.ui);
    assert_eq!(manifest.process.command, "python3");
}
```

- [ ] Step 2: Add the v2 manifest structs and reject missing schema version.

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub process: ExtensionProcessManifest,
    #[serde(default)]
    pub capabilities: ExtensionCapabilities,
    #[serde(default)]
    pub permissions: ExtensionPermissionDeclaration,
}

impl ExtensionManifest {
    pub fn validate(&self) -> Result<(), ExtensionManifestError> {
        if self.schema_version != 2 {
            return Err(ExtensionManifestError::UnsupportedSchemaVersion(self.schema_version));
        }
        validate_extension_id(&self.id)?;
        Ok(())
    }
}
```

- [ ] Step 3: Move discovery from `tools/extensions/discovery.rs` into `extensions/manifest.rs` or a focused `extensions/discovery.rs`.

Expected behavior:

- Discover `neo-extension.toml`.
- Parse v2 only.
- Validate duplicate ids.
- Include source tier: `User`, `Project`, or `ExplicitPath`.
- Return helpful parse errors with manifest path.

- [ ] Step 4: Export the new module from `crates/neo-agent-core/src/lib.rs`.

```rust
pub mod extensions;
```

- [ ] Step 5: Remove public `neo_agent_core::tools::extensions` exports after all call sites migrate.

Run:

```bash
rtk rg -n "tools::extensions|tools/extensions" crates docs
```

Expected: no production call site depends on `tools::extensions` as the source of truth.

## Task 2: Build The Bidirectional Process Protocol

**Files:**

- Create: `crates/neo-agent-core/src/extensions/protocol.rs`
- Create: `crates/neo-agent-core/src/extensions/process.rs`
- Modify: `crates/neo-agent-core/src/rpc/mod.rs` if existing JSONL helpers need reuse hooks
- Test: `crates/neo-agent-core/tests/extension_process.rs`

- [ ] Step 1: Write a test fixture extension that receives `extension.initialize` and returns contributions.

Use a temporary Python script in the test. It should read JSONL, respond to initialize, and keep running until shutdown.

Expected result:

```rust
assert_eq!(initialized.tools.len(), 1);
assert_eq!(initialized.commands.len(), 1);
```

- [ ] Step 2: Implement `ExtensionProcess`.

Required behavior:

- Spawn once per enabled extension.
- Keep stdin/stdout open.
- Read responses, notifications, and extension-originated requests.
- Route extension-originated `host.*` requests to a host handler.
- Track process pid.
- Capture recent stderr lines for doctor output.
- Kill process on shutdown.
- Mark process unhealthy on EOF, malformed JSON, response id mismatch, or startup timeout.

Skeleton:

```rust
pub struct ExtensionProcess {
    id: String,
    child: Child,
    stdin: ChildStdin,
    pending: PendingRpcMap,
    recent_stderr: RecentLines,
}

impl ExtensionProcess {
    pub async fn request<P, R>(&self, method: &'static str, params: P) -> Result<R, ExtensionProcessError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        // encode request, write JSONL, await matching response id
    }
}
```

- [ ] Step 3: Implement initialize and shutdown protocol helpers.

```rust
pub async fn initialize(
    &self,
    params: ExtensionInitializeParams,
) -> Result<ExtensionInitializeResult, ExtensionProcessError> {
    self.request("extension.initialize", params).await
}
```

- [ ] Step 4: Add timeouts.

Use:

- Startup timeout default: 5 seconds.
- Request timeout default: 30 seconds.
- Tool execution timeout should respect the existing tool timeout if one exists.

- [ ] Step 5: Replace old spawn-per-request behavior.

Run:

```bash
rtk rg -n "ExtensionRunner::spawn|tools.list" crates/neo-agent-core crates/neo-agent
```

Expected: old runner usage removed from production registration/execution.

## Task 3: Implement ExtensionManager And Contribution Registry

**Files:**

- Create: `crates/neo-agent-core/src/extensions/manager.rs`
- Create: `crates/neo-agent-core/src/extensions/lifecycle.rs`
- Modify: `crates/neo-agent-core/src/extensions/install.rs`
- Test: `crates/neo-agent-core/tests/extension_manager.rs`

- [ ] Step 1: Define status snapshots.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtensionStatusSnapshot {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: ExtensionSourceTier,
    pub enabled: bool,
    pub health: ExtensionHealth,
    pub granted_capabilities: BTreeSet<ExtensionCapability>,
    pub contributions: ExtensionContributionSummary,
    pub last_error: Option<String>,
}
```

- [ ] Step 2: Implement manager lifecycle.

```rust
pub struct ExtensionManager {
    discovery: ExtensionDiscovery,
    lifecycle: ExtensionLifecycleStore,
    processes: BTreeMap<String, ExtensionProcessHandle>,
    registry: ExtensionContributionRegistry,
}
```

Required methods:

```rust
impl ExtensionManager {
    pub async fn load_enabled(&mut self, ctx: ExtensionLoadContext) -> Result<(), ExtensionManagerError>;
    pub async fn reload(&mut self, ctx: ExtensionLoadContext) -> Result<(), ExtensionManagerError>;
    pub async fn enable(&mut self, id: &str, grants: CapabilityGrantDecision) -> Result<ExtensionStatusSnapshot, ExtensionManagerError>;
    pub async fn disable(&mut self, id: &str) -> Result<ExtensionStatusSnapshot, ExtensionManagerError>;
    pub fn statuses(&self) -> Vec<ExtensionStatusSnapshot>;
    pub fn contributions(&self) -> &ExtensionContributionRegistry;
}
```

- [ ] Step 3: Gate project extensions on workspace trust.

Test:

```rust
#[tokio::test]
async fn project_extension_does_not_load_when_workspace_untrusted() {
    let mut manager = fixture_manager_with_project_extension().await;
    manager
        .load_enabled(ExtensionLoadContext {
            workspace_trusted: false,
            ..fixture_context()
        })
        .await
        .expect("load completes");

    assert_eq!(manager.statuses()[0].health, ExtensionHealth::BlockedByTrust);
    assert!(manager.contributions().tools().is_empty());
}
```

- [ ] Step 4: Validate capabilities before accepting contributions.

Test cases:

- Extension returns a tool without `tools`: contribution rejected.
- Extension calls `host.ui.input` without `ui`: request rejected.
- Extension subscribes to events without `events`: subscription rejected.
- Extension asks for `host_context` and receives only bounded context fields.

- [ ] Step 5: Isolate failures.

Expected:

- One extension failure does not prevent other extensions from loading.
- Failed extension status becomes `error`.
- Contributions from failed extension are absent.
- Recent error appears in `/extensions` and `doctor`.

## Task 4: Register Neo-Native Extension Tools

**Files:**

- Modify: `crates/neo-agent-core/src/extensions/manager.rs`
- Modify: `crates/neo-agent-core/src/tools/registry.rs` or the local registry integration point
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Test: `crates/neo-agent/tests/mock_provider_e2e.rs`
- Test: `crates/neo-agent-core/tests/extension_manager.rs`

- [ ] Step 1: Write a focused test proving extension tools are in the model request.

Use the existing mock-provider e2e pattern from `mock_provider_e2e.rs`.

Expected:

```rust
assert!(tool_names.contains(&"extension__release_helper__draft_changelog"));
```

- [ ] Step 2: Implement an `ExtensionToolAdapter`.

```rust
pub struct ExtensionToolAdapter {
    extension_id: String,
    contribution: ExtensionToolContribution,
    manager: ExtensionManagerHandle,
}
```

Execution should call:

```text
extension.tool.execute
```

with `ExtensionToolExecuteParams`.

- [ ] Step 3: Route tool execution through normal `ToolRegistry`.

Do not add a separate tool execution path for extension tools. The model should see regular Neo tool schemas, and the runtime should apply the same permission flow it applies to other registered tools.

- [ ] Step 4: Remove `tools.list` registration path.

Run:

```bash
rtk rg -n "tools.list|discover_extension_tools|ExtensionToolSpec" crates
```

Expected: no production code uses old skeleton discovery.

## Task 5: Add Event Delivery

**Files:**

- Create: `crates/neo-agent-core/src/extensions/events.rs`
- Modify: `crates/neo-agent-core/src/runtime.rs`
- Modify: `crates/neo-agent-core/src/session.rs` if session events need bridge points
- Test: `crates/neo-agent-core/tests/extension_events.rs`

- [ ] Step 1: Define event structs and names.

```rust
pub const EVENT_TURN_START: &str = "turn.start";
pub const EVENT_TURN_END: &str = "turn.end";
pub const EVENT_TOOL_START: &str = "tool.start";
pub const EVENT_TOOL_END: &str = "tool.end";
```

- [ ] Step 2: Add event emission points.

Emit:

- `session.start` when an interactive/run session starts.
- `turn.start` before model request.
- `message.delta` during assistant text streaming.
- `tool.start` before tool execution.
- `tool.end` after tool execution.
- `turn.end` after the agent turn finishes.

- [ ] Step 3: Make event delivery best-effort.

Expected:

- Event failure marks extension degraded.
- Event failure does not fail the user turn.
- Event failure is visible in status and doctor.

- [ ] Step 4: Add tests for best-effort isolation.

Expected:

```rust
assert_eq!(turn_result.assistant_text(), "done");
assert_eq!(manager.status("bad-events").health, ExtensionHealth::Degraded);
```

## Task 6: Add Extension Commands, Shortcuts, And Flags

**Files:**

- Modify: `crates/neo-agent-core/src/extensions/contributions.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: slash command registry source in `crates/neo-agent`
- Modify: keybinding handling in `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`
- Test: relevant TUI tests if present for slash command completion

- [ ] Step 1: Register extension slash commands into the same registry used by built-ins.

Expected:

- `/release` appears in slash completion.
- Invoking `/release stable` calls `extension.command.execute`.
- Disabled extension removes `/release`.

- [ ] Step 2: Reject command name conflicts.

Conflict policy:

- Built-ins win.
- First extension by deterministic id order wins only if no built-in conflict.
- Losers show conflict in `/extensions` details and doctor.

- [ ] Step 3: Wire shortcuts.

Shortcut constraints:

- Reject conflicts with existing core bindings unless Neo already has a user override policy.
- Show accepted shortcuts in `/extensions`.
- Show rejected shortcuts in doctor.

- [ ] Step 4: Wire CLI flags only if the existing CLI parser can expose them cleanly.

If dynamic CLI flags cannot be represented safely through `clap` without awkward global parsing, scope Phase 1 CLI flags to extension command handlers in TUI and document that process-level dynamic flags require a separate CLI parsing design. Do not hack raw args parsing into `main.rs`.

## Task 7: Add Host UI Requests And `/extensions` TUI

**Files:**

- Create: `crates/neo-tui/src/dialogs/extension_manager.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Modify: `crates/neo-agent-core/src/extensions/protocol.rs`
- Test: TUI dialog tests under `crates/neo-tui/tests/` if existing dialog tests fit

- [ ] Step 1: Add `/extensions` slash command.

Expected:

- `/extensions` opens the manager overlay.
- Overlay uses the blocking dialog contract: when focused, main composer does not receive text.
- Escape closes overlay.
- Enter opens details.
- `e` toggles enable/disable with confirmation when enabling new capabilities.
- `d` opens doctor view.
- `r` reloads extension contributions.

- [ ] Step 2: Implement host UI requests.

Host request methods:

```text
host.ui.select
host.ui.confirm
host.ui.input
host.ui.notify
host.ui.set_status
host.ui.set_widget
```

Rules:

- These require `ui` capability.
- Blocking dialogs must use existing TUI dialog routing.
- Requests made outside TUI should degrade gracefully:
  - `notify` logs to stderr or session notification.
  - `select`, `confirm`, and `input` return a structured unavailable error.

- [ ] Step 3: Render main and details views from status snapshots.

Use stable dimensions:

- Left list width: about 34-38 columns.
- Right detail pane fills remaining width.
- Truncate long extension names and paths with ellipsis.
- Do not wrap long JSON in headers.

- [ ] Step 4: Add capability confirmation dialog.

The dialog must show:

- Extension name.
- Source tier.
- Requested capabilities.
- Risk explanation.
- Enable/Cancel choices.

- [ ] Step 5: Add recent errors and doctor integration.

The TUI does not need raw logs. Show concise recent stderr/error lines with truncation.

## Task 8: Clean CLI Surface And Add Doctor

**Files:**

- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/extension_commands.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Modify: `crates/neo-agent/tests/cli_commands.rs`
- Modify: `docs/packages.md`
- Modify: `docs/quickstart.md`

- [ ] Step 1: Remove `ExtensionCommand::Call`.

Delete:

```rust
Call {
    extension_id: String,
    method: String,
    params: String,
    root: std::path::PathBuf,
}
```

- [ ] Step 2: Remove `extension_commands::call`.

Delete the raw RPC invocation function from `crates/neo-agent/src/extension_commands.rs`.

- [ ] Step 3: Add `Doctor`.

CLI shape:

```rust
Doctor {
    extension_id: Option<String>,
    #[arg(long, default_value = ".neo/extensions")]
    root: std::path::PathBuf,
}
```

Output should be human-readable and concise:

```text
release-helper  ok
  manifest       ok
  process        ok
  initialize     ok
  contributions  2 tools, 2 commands, 2 hooks
  conflicts      none
```

- [ ] Step 4: Keep product-level commands only.

Final CLI:

```text
extensions list
extensions install
extensions update
extensions uninstall
extensions enable
extensions disable
extensions doctor
```

If `status` is still present when this task starts, fold it into `list`/`doctor` and remove it from the final command enum.

- [ ] Step 5: Add CLI regression tests.

Required assertions:

```rust
assert_unknown_command(["extensions", "call", "echo", "tool.echo"]);
assert_unknown_command(["extensions", "inspect", "echo"]);
```

Also update existing tests that currently call `extensions call`:

- `extensions_call_round_trips_json_rpc`
- `extensions_lifecycle_commands_persist_status_and_gate_call`

Replace them with manager/tool execution tests and doctor tests.

## Task 9: Install/Enable Flow And Capability Grants

**Files:**

- Modify: `crates/neo-agent-core/src/extensions/install.rs`
- Modify: `crates/neo-agent-core/src/extensions/lifecycle.rs`
- Modify: `crates/neo-agent/src/extension_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent-core/tests/extension_lifecycle.rs`

- [ ] Step 1: Store enabled state and granted capabilities.

State shape:

```toml
[extensions.release-helper]
enabled = true
granted_capabilities = ["tools", "commands", "ui", "events", "host_context"]
last_enabled_at = "2026-06-22T00:00:00Z"
```

- [ ] Step 2: CLI enable should require capability grant confirmation only when interactive.

If CLI runs non-interactively, print a clear error when the extension asks for new capabilities:

```text
extension release-helper requests capabilities: tools, commands, ui
run `neo extensions enable release-helper --accept-capabilities` to enable non-interactively
```

Add an explicit flag:

```text
neo extensions enable release-helper --accept-capabilities
```

This is not a debug flag. It is a deliberate non-interactive consent mechanism.

- [ ] Step 3: TUI enable uses the capability confirmation dialog.

Accepted capabilities are persisted. If manifest adds new capabilities after update, extension becomes `needs_review` until the user accepts the new grant.

- [ ] Step 4: Project extension trust gating.

If source tier is project and workspace is untrusted:

- Do not start process.
- Status: `blocked_by_trust`.
- TUI row explains trust requirement.
- Doctor reports the trust block without treating it as a process failure.

## Task 10: Documentation And Examples

**Files:**

- Create: `docs/extensions.md`
- Modify: `docs/index.md`
- Modify: `docs/packages.md`
- Modify: `docs/quickstart.md`
- Create: `examples/extensions/release-helper/neo-extension.toml`
- Create: `examples/extensions/release-helper/extension.py`

- [ ] Step 1: Write user docs around product concepts.

Docs must explain:

- What extensions are.
- How they differ from MCP.
- How to install, enable, disable, update, uninstall.
- What `/extensions` shows.
- What capabilities mean.
- What trust does.
- Why `doctor` exists.

- [ ] Step 2: Write extension author docs.

Include:

- Manifest v2.
- Initialize protocol.
- Contribution examples.
- Tool execution.
- Command execution.
- UI request examples.
- Event subscription examples.
- Capability declaration.
- Error handling.

- [ ] Step 3: Update old docs.

Remove references to:

```bash
neo extensions call echo tool.echo '{"value":42}'
```

Replace skeleton framing with host-plugin framing.

- [ ] Step 4: Add a minimal example extension.

The example should:

- Register one command.
- Register one tool.
- Subscribe to `turn.start`.
- Use `host.ui.notify`.
- Declare capabilities accurately.

## Task 11: Verification

**Files:**

- Test updates across `crates/neo-agent-core/tests/`
- Test updates across `crates/neo-agent/tests/`
- TUI tests where existing infrastructure supports it

- [ ] Step 1: Focused extension core tests.

Run:

```bash
cargo run -p xtask -- test -p neo-agent-core extension_
```

Expected: extension manifest, process, manager, lifecycle, and event tests pass.

- [ ] Step 2: Focused CLI tests.

Run:

```bash
cargo run -p xtask -- test -p neo-agent cli_commands extensions
```

Expected:

- Product commands pass.
- `extensions call` is unknown.
- `extensions inspect` is unknown.
- `extensions doctor` reports health.

- [ ] Step 3: Focused model tool registration test.

Run:

```bash
cargo run -p xtask -- test -p neo-agent mock_provider_e2e extension
```

Expected:

- Extension tools appear in model request.
- Disabled extension tools do not appear.
- Project extension blocked by trust does not appear.

- [ ] Step 4: Run formatting.

Run:

```bash
cargo fmt --all --check
```

Expected: clean formatting.

- [ ] Step 5: Run only broader checks if implementation touches shared runtime paths.

If runtime changes touch common tool execution, permissions, or interactive input routing, run:

```bash
cargo run -p xtask -- test -p neo-agent-core runtime_turn
cargo run -p xtask -- test -p neo-agent interactive
```

Do not run full CI for this task unless the implementation becomes a cross-workspace refactor beyond the files above.

## Pitfalls

- Do not rebrand the old raw JSONL tool skeleton as the new system.
- Do not keep `extensions call` as a hidden compatibility path.
- Do not add `inspect`.
- Do not let extension tools bypass permission approval.
- Do not copy full pi context into a child process.
- Do not expose API keys or unredacted config to extensions.
- Do not silently load project extensions when workspace trust is denied or undecided.
- Do not treat capability declarations as OS sandboxing.
- Do not let event hook failures fail user turns.
- Do not let stale contributions remain active after disable, reload, or process crash.
- Do not add provider/model/OAuth registration in Phase 1.
- Do not add permission-policy mutation in Phase 1.
- Do not duplicate slash command registries.
- Do not let extension UI requests bypass the blocking dialog contract.
- Do not make `/extensions` a log viewer or debug console.
- Do not preserve old manifest v1 as an equal production path.

## Self-Review Checklist

Before marking NEO-35 complete, verify:

- The implementation creates a first-class extension host API, not just tools.
- MCP docs and extension docs describe distinct roles.
- `neo extensions call` is gone from CLI, tests, and docs.
- No `inspect` command exists.
- `doctor` is the only diagnostic command.
- `/extensions` exists and shows status, capabilities, contributions, and errors.
- Project extensions are trust gated.
- Capability grants are persisted and reviewed when changed.
- Extension tools use normal Neo tool approval.
- Event failures are isolated.
- Disabled or unhealthy extensions do not contribute tools/commands/shortcuts.
- Tests use `cargo run -p xtask -- test`, not bare `cargo test`.
- No git mutation commands are run unless the user explicitly authorizes the exact command.

## Execution Notes

This handoff is intentionally large. A good implementation split is:

1. Core manifest/protocol/manager.
2. Tool registration and event delivery.
3. Commands, shortcuts, and TUI host requests.
4. `/extensions` UI.
5. CLI cleanup and doctor.
6. Docs and examples.

Each split should leave Neo compiling with focused tests. Avoid a long-lived half-migrated state where both `tools/extensions` and `extensions` are independently active.
