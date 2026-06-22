# NEO-17 MCP Runtime Reliability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn Neo's MCP support from per-turn static discovery into a reliable runtime service with server lifecycle state, isolated startup failures, reconnect/health handling, resource APIs, config hot reload, and diagnostics that NEO-32 can present in the `/mcp` TUI manager.

**Architecture:** Keep protocol adapters in `neo-agent-core`, add a manager layer that owns configured MCP server state and exposes snapshots, resource operations, and model-visible MCP tools. Keep config parsing and persistence in `neo-agent`, with a small `mcp_ops` service bridge shared by CLI and TUI. NEO-32 should consume the NEO-17 manager/service APIs instead of building a second probe/discovery path.

**Tech Stack:** Rust 2024, `tokio`, `reqwest`, Neo `ToolRegistry`, Neo MCP adapters, global `~/.neo/config.toml` / `$NEO_HOME/config.toml`, focused `xtask`/nextest verification.

---

## Linear Context

- Linear: [NEO-17](https://linear.app/neo-agent/issue/NEO-17/improve-mcp-feature)
- Title: Improve MCP feature
- Priority: High
- Project: Infrastructure
- Team: Neo
- Related UI task: [NEO-32](https://linear.app/neo-agent/issue/NEO-32/implement-mcp-slash-command-and-interactive-mcp-manager)

NEO-17 existing description asks for:

- Connection lifecycle management: reconnect, health check, graceful shutdown.
- Dynamic tool discovery updates when MCP server tools change.
- Resource support: `resources/list`, `resources/read`, and resource update notifications, without silently injecting resources into model context.
- Error isolation: connection failure must not block the main agent.
- Config hot reload after `neo mcp add/del`.
- Better MCP diagnostics with server id and repair hints.

Important correction from code reading: Neo already has partial resource protocol support in the adapter layer. `McpToolAdapter` already includes `list_resources`, `read_resource`, `subscribe_resource`, `unsubscribe_resource`, and `next_resource_update`; both stdio and HTTP/SSE adapters have implementations and tests. NEO-17 should not rebuild those adapters from scratch. The missing work is the runtime manager, status surface, resource access surface, hot reload, resilient registration, and diagnostics.

## Relationship Between NEO-17 And NEO-32

### Ownership Boundary

NEO-17 owns the MCP runtime foundation:

- Server lifecycle state: disabled, pending, connected, failed, reconnecting.
- Connection startup and shutdown.
- Reconnect and health behavior.
- Dynamic `tools/list` refresh and tool registry synchronization.
- Resource list/read/update state.
- Config hot reload hooks and service API.
- Diagnostic error shape and status snapshots.

NEO-32 owns the TUI product surface:

- `/mcp` slash command.
- `McpManagerState` overlay modeled after `/provider`.
- Add/edit/delete/enable/disable/test flows in the TUI.
- Keyboard routing and blocking dialog behavior.
- TUI rendering of status snapshots produced by NEO-17.

### Dependency Recommendation

Set NEO-17 as blocking NEO-32 for the final integrated experience.

NEO-32 can prototype the overlay against static config rows, but the finished `/mcp` manager should consume NEO-17 APIs for:

- live server status,
- tool counts,
- connection errors,
- refresh/reconnect actions,
- resource counts,
- config changes taking effect without restart.

If NEO-32 is implemented first, it must not duplicate connection management. It should expose temporary "not connected yet" states and replace them with NEO-17 manager snapshots later. Duplicating discovery/probe logic in NEO-32 is the main thing to avoid.

### Sequencing

Recommended sequence:

1. Implement the NEO-17 `McpConnectionManager` and status snapshots.
2. Make `tool_registry_for_config` failure-isolated and manager-backed.
3. Add explicit resource access surfaces.
4. Add config hot reload service hooks.
5. Update docs and focused tests.
6. Revisit NEO-32 handoff so its `/mcp` overlay uses the manager snapshots and actions.

## Current Neo Code Map

### Adapter Layer Already Exists

Read:

- `crates/neo-agent-core/src/tools/mcp.rs`
- `crates/neo-agent-core/tests/tool_mcp.rs`
- `docs/mcp.md`

Important existing types:

```rust
pub trait McpToolAdapter: Send + Sync {
    async fn list_tools(&self) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolResponse, McpError>;
    async fn list_resources(&self) -> Result<Vec<McpResourceDefinition>, McpError>;
    async fn read_resource(&self, uri: &str) -> Result<McpResourceRead, McpError>;
    async fn subscribe_resource(&self, uri: &str) -> Result<(), McpError>;
    async fn unsubscribe_resource(&self, uri: &str) -> Result<(), McpError>;
    async fn next_resource_update(&self) -> Result<McpResourceUpdate, McpError>;
}
```

Existing adapters:

- `McpStdioToolAdapter`
- `McpHttpToolAdapter`
- `McpToolProvider`

Existing behavior:

- Stdio starts a configured command and reuses an initialized session inside one adapter.
- HTTP/SSE sends JSON-RPC requests and handles SSE response bodies.
- Resource update notifications are queued by the adapters.
- `McpToolProvider::discover_dyn(server_id, adapter)` calls `tools/list`.
- `McpToolProvider::register_into(registry)` registers one Neo tool per remote MCP tool.

The manager must build on these.

### Current Registration Is Static And Brittle

Read:

- `crates/neo-agent/src/modes/run.rs`

Current registration path:

```rust
pub(crate) async fn tool_registry_for_config(
    config: &AppConfig,
    todos: std::sync::Arc<std::sync::Mutex<Vec<neo_agent_core::TodoEventData>>>,
) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_builtin_tools_and_todos(todos);
    for server in config.mcp.servers.iter().filter(|server| server.enabled) {
        register_mcp_server(&mut registry, server).await?;
    }
    Ok(registry)
}
```

Problem:

- One broken MCP server fails `tool_registry_for_config`, which can prevent the agent from starting.
- Tool discovery happens once when the runtime is built.
- Tool specs are copied into `AgentConfig.tools` by `AgentRuntime::with_tools_and_skills`.
- `chat_request` sends `config.tools.clone()` to the model, so dynamic updates need an explicit sync point before each model request or at least before each new turn.

### Current CLI Config Surface

Read:

- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-agent/src/config.rs`
- `crates/neo-agent/src/modes/run.rs`

Current CLI:

- `neo mcp list`
- `neo mcp add <name> -t studio|remote-http|remote-sse ...`
- `neo mcp del <name>`
- `neo mcp enable <name>`
- `neo mcp disable <name>`

Persisted config:

```rust
pub struct McpServerConfig {
    pub id: String,
    pub enabled: bool,
    pub transport: String,
    pub command: Option<String>,
    pub url: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
}
```

Config mutations already exist:

- `upsert_mcp_server`
- `remove_mcp_server`
- `set_mcp_server_enabled`

NEO-17 should not move global config into project-local config. MCP config remains global Neo config.

### Current TUI Refresh Hooks

Read:

- `crates/neo-agent/src/modes/interactive.rs`

Existing useful hooks:

- `InteractiveController::refresh_config()`
- `InteractiveController::local_config`
- `TurnRequest::base_config`
- active turns receive the current `local_config` snapshot when launched.

NEO-17 should provide a service that NEO-32 can call after config edits:

- `refresh_config()`
- `mcp_service.apply_config(&new_config.mcp.servers)`
- update the overlay rows from `mcp_service.snapshots()`

### Current Docs

Read:

- `docs/mcp.md`
- `docs/quickstart.md`

Docs already say resources are not silently injected. Preserve that boundary.

## Reference Implementation Notes

### Kimi Code

Read:

- `docs/kimi-code/packages/agent-core/src/mcp/connection-manager.ts`
- `docs/kimi-code/packages/agent-core/src/agent/tool/index.ts`
- `docs/kimi-code/packages/agent-core/src/session/index.ts`
- `docs/kimi-code/packages/agent-core/src/mcp/types.ts`
- `docs/kimi-code/packages/agent-core/src/mcp/tool-naming.ts`

Useful patterns:

- `McpConnectionManager` owns server entries, clients, tools, status, and listeners.
- Status values include `pending`, `connected`, `failed`, `disabled`, `needs-auth`.
- `connectAll` starts configured servers in parallel.
- `connect`, `remove`, `reconnect`, and `shutdown` are manager-level operations.
- Per-server failures are isolated; failed servers emit status and do not block the session.
- `onStatusChange` lets the session/tool manager refresh UI and tool registration.
- `ToolManager` registers and unregisters MCP tools when status changes.
- Unexpected close is surfaced as a failed status with stderr tail when available.
- Tool name collisions are detected and emitted as errors instead of silently overwriting.

Do not copy Kimi's OAuth-specific `needs-auth` flow into NEO-17 unless a later Neo issue asks for OAuth. For this plan, map unauthorized remote servers to `Failed` with a diagnostic hint.

### Codex

Read:

- `docs/codex/codex-rs/codex-mcp/src/connection_manager.rs`
- `docs/codex/codex-rs/codex-mcp/src/connection_manager_tests.rs`
- `docs/codex/codex-rs/core/src/tools/handlers/mcp_resource/list_mcp_resources.rs`
- `docs/codex/codex-rs/core/src/tools/handlers/mcp_resource/read_mcp_resource.rs`

Useful patterns:

- Manager owns async clients keyed by server name.
- Startup emits per-server updates and a final summary.
- Failed startup is represented as server status, not total agent failure.
- `list_all_tools` aggregates tools across connected servers.
- Cached tool snapshots can keep startup from blocking forever.
- Resources are accessed explicitly through tools such as `list_mcp_resources` and `read_mcp_resource`, not injected into model context.
- Resource list/read failures include server and uri context.

Do not import Codex's hosted connectors, OAuth, plugin provenance, or app-server concepts. Neo remains local-only.

## Product And Runtime Decisions

- Disabled MCP servers are not started.
- Config remains global: `~/.neo/config.toml` or `$NEO_HOME/config.toml`.
- Connection failure for one MCP server must never prevent built-in tools or other MCP servers from working.
- Tool discovery changes are applied at a model-request boundary. The model cannot use a tool it has not seen in the current request, so mid-response tool list changes take effect before the next model call, not in the middle of streaming one response.
- MCP resources are not silently appended to context. If exposed to the model, they must be explicit tools with visible call results.
- Secrets in env and headers are never logged, rendered, or persisted into session events.
- Stdio stderr can be shown only as a capped diagnostic tail.
- Tool name collisions must not silently overwrite existing tools.
- The `studio` CLI label remains a CLI alias for local `stdio`; runtime config should use `stdio`.
- Do not introduce hosted MCP registry, OAuth onboarding, or server marketplace behavior.
- Do not create duplicate legacy paths for config parsing, probing, or tool discovery. NEO-32 must consume the same service functions.

## UX Contract For NEO-32 Consumers

NEO-17 should produce snapshots that make NEO-32's `/mcp` overlay straightforward.

### Main Manager Rows

NEO-32 can render a NEO-17 snapshot like this:

```text
+ MCP Servers -------------------------------------------------------------+
| 4 configured, 2 connected, 1 failed, 1 disabled                           |
| Up/Down select  Enter details  R refresh  E toggle  D delete  A add       |
|                                                                          |
| > ok  filesystem      stdio        connected     tools 12    resources 3  |
|      npx -y @modelcontextprotocol/server-filesystem /repo                |
|                                                                          |
|   !!  linear          http         failed        tools 0     resources 0  |
|      HTTP 401 from https://mcp.linear.app/mcp                            |
|      hint: check Authorization header or disable this server              |
|                                                                          |
|   ..  docs-sse        sse          reconnecting  attempt 3    next 8s     |
|      https://example.invalid/mcp/sse                                      |
|                                                                          |
|   --  old-tools       stdio        disabled      not started              |
|                                                                          |
|   + Add MCP server                                                        |
+--------------------------------------------------------------------------+
```

### Detail View

```text
+ MCP: filesystem ---------------------------------------------------------+
| Status        connected                                                   |
| Transport     stdio                                                       |
| Command       npx -y @modelcontextprotocol/server-filesystem /repo        |
| CWD           /Users/chenyuanhao/Workspace/neo                            |
| Env           NODE_OPTIONS, MCP_LOG_LEVEL                                 |
| Tools         12 discovered                                               |
| Resources     3 listed, 1 subscribed                                      |
| Last check    2026-06-22 16:20:12                                         |
|                                                                          |
| Tools                                                                    |
|   read_file              Read a file                                      |
|   list_directory         List a directory                                 |
|   search_files           Search files                                     |
|                                                                          |
| Resources                                                                |
|   file://docs/readme.md  README                    text/markdown          |
|                                                                          |
| T test  R refresh tools  L list resources  E disable  Esc back            |
+--------------------------------------------------------------------------+
```

### Failure Detail

```text
+ MCP: linear -------------------------------------------------------------+
| Status        failed                                                      |
| Transport     http                                                        |
| URL           https://mcp.linear.app/mcp                                  |
| Last error    HTTP 401 from https://mcp.linear.app/mcp                    |
| Hint          Check the Authorization header or disable this server.       |
| Attempts      4                                                           |
| Next retry    paused after max attempts                                   |
|                                                                          |
| R reconnect now  E disable  Esc back                                      |
+--------------------------------------------------------------------------+
```

NEO-17 should provide data for these rows; NEO-32 owns rendering and keyboard flow.

## Proposed File Structure

Create:

- `crates/neo-agent-core/src/tools/mcp_manager.rs`
  - Manager state, snapshots, reconnect policy, tool registration helpers, resource access.
- `crates/neo-agent/src/mcp_ops.rs`
  - Conversion between `McpServerConfig` and core runtime manager config, CLI/TUI shared parsing, redacted summaries, config hot reload helpers.

Modify:

- `crates/neo-agent-core/src/tools/mod.rs`
  - Export the manager and explicit resource tools if implemented in core.
- `crates/neo-agent-core/src/runtime.rs`
  - Refresh tool specs from current registry before each model request if dynamic registry support is implemented.
  - Add MCP status/resource events only if needed for TUI transcript/status surfaces.
- `crates/neo-agent/src/modes/run.rs`
  - Replace direct static MCP discovery with manager-backed discovery.
  - Keep existing CLI behavior stable through `mcp_ops`.
- `crates/neo-agent/src/main.rs`
  - Route `neo mcp` commands through `mcp_ops`.
- `crates/neo-agent/src/modes/interactive.rs`
  - Store a shared MCP service/manager handle for NEO-32.
  - Refresh the manager when config changes.
- `crates/neo-agent/src/config.rs`
  - Only add fields if required for reconnect policy or diagnostics. Prefer no persisted schema churn in the first patch.
- `docs/mcp.md`
  - Update current status and reliability behavior.
- `docs/quickstart.md`
  - Mention that MCP config changes no longer require full restart where supported.

## Data Model Design

### Core Runtime Config

`neo-agent-core` should not depend on `neo-agent::config::McpServerConfig`. Add a runtime shape in core:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedMcpServerConfig {
    pub id: String,
    pub enabled: bool,
    pub transport: ManagedMcpTransport,
    pub enabled_tools: Vec<String>,
    pub disabled_tools: Vec<String>,
    pub startup_timeout_ms: Option<u64>,
    pub tool_timeout_ms: Option<u64>,
    pub reconnect: McpReconnectPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedMcpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: std::collections::BTreeMap<String, String>,
        cwd: Option<std::path::PathBuf>,
    },
    Http {
        url: String,
        headers: std::collections::BTreeMap<String, String>,
    },
    Sse {
        url: String,
        headers: std::collections::BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpReconnectPolicy {
    pub enabled: bool,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub max_attempts: Option<u32>,
}

impl Default for McpReconnectPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay_ms: 500,
            max_delay_ms: 30_000,
            max_attempts: Some(5),
        }
    }
}
```

### Status Snapshots

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStatus {
    Disabled,
    Pending,
    Connected,
    Failed,
    Reconnecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerSnapshot {
    pub id: String,
    pub transport: String,
    pub status: McpServerStatus,
    pub tool_count: usize,
    pub resource_count: Option<usize>,
    pub error: Option<McpDiagnostic>,
    pub reconnect_attempt: u32,
    pub next_retry_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiagnostic {
    pub server_id: String,
    pub transport: String,
    pub message: String,
    pub hint: Option<String>,
    pub stderr_tail: Option<String>,
}
```

### Manager API

```rust
pub struct McpConnectionManager {
    inner: std::sync::Arc<tokio::sync::RwLock<McpConnectionManagerState>>,
}

impl McpConnectionManager {
    pub fn new(process_supervisor: ProcessSupervisor) -> Self;

    pub async fn apply_config(
        &self,
        servers: Vec<ManagedMcpServerConfig>,
    ) -> Vec<McpServerSnapshot>;

    pub async fn upsert_server(
        &self,
        server: ManagedMcpServerConfig,
    ) -> McpServerSnapshot;

    pub async fn remove_server(&self, id: &str) -> bool;

    pub async fn reconnect_now(&self, id: &str) -> anyhow::Result<McpServerSnapshot>;

    pub async fn refresh_tools(&self, id: &str) -> anyhow::Result<McpServerSnapshot>;

    pub async fn snapshots(&self) -> Vec<McpServerSnapshot>;

    pub async fn register_connected_tools_into(
        &self,
        registry: &mut ToolRegistry,
    ) -> Vec<McpDiagnostic>;

    pub async fn list_resources(
        &self,
        server_id: Option<&str>,
    ) -> anyhow::Result<Vec<McpResourceListEntry>>;

    pub async fn read_resource(
        &self,
        server_id: &str,
        uri: &str,
    ) -> anyhow::Result<McpResourceRead>;

    pub async fn shutdown(&self);
}
```

Do not hold a write lock while awaiting adapter network/process operations. Clone the minimal entry data, drop locks, await, then reacquire the lock to commit the result if the entry attempt id is still current.

## Tool Registry Strategy

### Minimal Acceptance

At minimum, `tool_registry_for_config` should:

- build built-in tools,
- build extension tools,
- ask the manager to connect/discover enabled MCP servers,
- register only connected MCP tools,
- collect diagnostics for failed servers,
- return `Ok(registry)` even when one MCP server fails.

This alone fixes the "broken MCP server blocks main agent" problem.

### Dynamic Tool List Acceptance

The stronger acceptance target is: when a connected server's tool list changes, the new list is visible before the next model request boundary.

Current obstacle:

- `AgentRuntime::with_tools_and_skills` copies `tools.specs()` into `AgentConfig.tools`.
- `chat_request` uses `config.tools.clone()`.

Recommended implementation:

1. Make the runtime compute tool specs from the current registry before each model request.
2. Keep the `Skill` tool appended after current registry specs.
3. Do not change tool specs while a model response is streaming.
4. If a model calls a tool that was removed after the request was sent, return a clear `UnknownTool` or MCP diagnostic result rather than panicking.

The smallest safe core change is:

```rust
async fn current_tool_specs(
    registry: Option<&ToolRegistry>,
    skills: Option<&SkillStore>,
) -> Vec<ToolSpec> {
    let mut specs = registry.map_or_else(Vec::new, ToolRegistry::specs);
    if skills.is_some() {
        specs.push(invoke_skill_tool_spec());
    }
    specs
}
```

Then pass those specs into `chat_request`:

```rust
let request = chat_request(
    &config,
    &emitter.context,
    current_tool_specs(tools.as_deref(), skills.as_deref()).await,
).await;
```

If the manager mutates registry contents between request boundaries, the next request sees the new specs.

Avoid holding a registry lock across `Tool::execute`. If you introduce a shared mutable registry, first change `ToolRegistry` to store `Arc<dyn Tool>` so `run` can clone the selected tool handle and drop the lock before awaiting:

```rust
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub async fn run(
        &self,
        name: &str,
        ctx: &ToolContext,
        input: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let tool = Arc::clone(self.tools.get(name).ok_or_else(|| ToolError::UnknownTool {
            name: name.to_owned(),
        })?);
        tool.execute(ctx, input).await
    }
}
```

Only do this if dynamic in-turn updates are in scope for the patch. Otherwise, keep manager-backed discovery at new-turn/runtime creation and document that external config changes apply to the next launched turn.

## Explicit Resource Tools

NEO-17 should add explicit resource tools if the implementation wants model access to MCP resources.

Recommended tool names following Neo built-in naming style:

- `ListMcpResources`
- `ReadMcpResource`

`ListMcpResources` input:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ListMcpResourcesInput {
    pub server_id: Option<String>,
}
```

`ReadMcpResource` input:

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadMcpResourceInput {
    pub server_id: String,
    pub uri: String,
}
```

Rules:

- These tools read from `McpConnectionManager`.
- They must not start disabled servers.
- They must not inject resources without a model tool call.
- Output must include server id and uri.
- Binary/blob resources should be summarized or capped, not dumped unbounded.
- If no MCP resources exist, return a normal successful empty list.

Example model-visible output:

```text
MCP resources:
- server: docs
  uri: file://docs/readme.md
  name: README
  mime_type: text/markdown
```

## Config Hot Reload Strategy

There are two levels.

### Internal Hot Reload

When Neo itself changes config through CLI helper functions or NEO-32's TUI add/delete/toggle flow:

1. Persist the config mutation.
2. Reload `AppConfig`.
3. Convert `AppConfig.mcp.servers` to `ManagedMcpServerConfig`.
4. Call `McpConnectionManager::apply_config`.
5. Refresh local UI snapshots.

This is mandatory.

### External Hot Reload

When another process runs `neo mcp add/del` while a TUI is open:

- Do not add a new file-watcher dependency for the first implementation.
- Use a lightweight config mtime check with `std::fs::metadata(config_path).modified()`.
- Poll at a modest interval such as 2 seconds while the TUI event loop is active.
- If mtime changes, call the same reload path as internal hot reload.
- If reload fails, keep the old manager state and surface a status notice.

This gives the requested "no restart" behavior without growing dependency surface.

## Task 1: Extract Shared MCP Operations

**Files:**

- Create: `crates/neo-agent/src/mcp_ops.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`
- Test: unit tests in `crates/neo-agent/src/mcp_ops.rs`

- [ ] Move shared CLI helpers from `run.rs` into `mcp_ops.rs`:

```rust
pub fn parse_mcp_kind(type_arg: &str) -> anyhow::Result<&'static str> {
    match type_arg {
        "studio" | "stdio" => Ok("stdio"),
        "remote-http" | "http" => Ok("http"),
        "remote-sse" | "sse" => Ok("sse"),
        other => anyhow::bail!("unsupported MCP type: {other}"),
    }
}
```

- [ ] Move or recreate these helpers in `mcp_ops.rs`:

```rust
pub fn display_mcp_kind(transport: &str) -> &str;
pub fn parse_command_string(cmd: &str) -> anyhow::Result<(String, Vec<String>)>;
pub fn key_value_pairs(values: Vec<String>, flag: &str) -> anyhow::Result<BTreeMap<String, String>>;
pub fn build_mcp_server_config(input: AddMcpServerInput) -> anyhow::Result<McpServerConfig>;
pub fn to_managed_config(server: &McpServerConfig) -> anyhow::Result<ManagedMcpServerConfig>;
pub fn to_managed_configs(servers: &[McpServerConfig]) -> anyhow::Result<Vec<ManagedMcpServerConfig>>;
```

- [ ] Define input shape:

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

- [ ] Update `main.rs` MCP dispatch and `run.rs` to call `mcp_ops`.

- [ ] Add focused tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent mcp_ops
```

Required assertions:

- `studio` and `stdio` map to stored `stdio`.
- `remote-http` maps to stored `http`.
- `remote-sse` maps to stored `sse`.
- stdio requires `command`.
- remote requires `url`.
- stdio rejects `url` and headers.
- remote rejects `command` and `cwd`.
- conversion to `ManagedMcpServerConfig` preserves tool filters and timeouts.
- summaries expose env/header keys but never values.

## Task 2: Add Core Manager Types

**Files:**

- Create: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp_manager.rs`

- [ ] Add `ManagedMcpServerConfig`, `ManagedMcpTransport`, `McpReconnectPolicy`, `McpServerStatus`, `McpServerSnapshot`, and `McpDiagnostic`.

- [ ] Export them:

```rust
mod mcp_manager;
pub use mcp_manager::*;
```

- [ ] Implement transport to adapter conversion inside core:

```rust
fn adapter_for_config(
    config: &ManagedMcpServerConfig,
    supervisor: ProcessSupervisor,
) -> Result<Arc<dyn McpToolAdapter>, McpDiagnostic>;
```

- [ ] Include a diagnostic hint function:

```rust
fn diagnostic_hint(error: &McpError, config: &ManagedMcpServerConfig) -> Option<String> {
    let message = error.message().to_ascii_lowercase();
    if message.contains("401") || message.contains("unauthorized") {
        return Some("Check remote MCP authorization headers or disable this server.".to_owned());
    }
    if matches!(config.transport, ManagedMcpTransport::Stdio { .. })
        && message.contains("failed to start")
    {
        return Some("Check that the command exists and that cwd is valid.".to_owned());
    }
    if message.contains("timed out") {
        return Some("Increase startup_timeout_ms or check that the MCP server starts quickly.".to_owned());
    }
    None
}
```

- [ ] Add tests:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core tool_mcp_manager
```

Expected:

- disabled config produces `Disabled` snapshot and no adapter start.
- bad stdio command produces `Failed` with server id and hint.
- HTTP 401 diagnostic includes auth hint.

## Task 3: Implement Connection Lifecycle Manager

**Files:**

- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp_manager.rs`

- [ ] Implement internal entry state with attempt ids:

```rust
struct ManagedMcpEntry {
    config: ManagedMcpServerConfig,
    attempt_id: u64,
    status: McpServerStatus,
    adapter: Option<Arc<dyn McpToolAdapter>>,
    tools: Vec<McpToolDefinition>,
    resources: Vec<McpResourceDefinition>,
    error: Option<McpDiagnostic>,
    reconnect_attempt: u32,
    next_retry_ms: Option<u64>,
}
```

- [ ] Implement `apply_config`:

Behavior:

- Remove entries no longer present.
- Shut down removed stdio sessions through adapter/session drop and `ProcessSupervisor`.
- Add new entries.
- If an existing entry's config changed, close and reconnect it.
- If only filters changed, reuse the adapter and recompute allowed tools.
- Disabled entries become `Disabled` and are not started.
- Enabled entries connect in parallel.

- [ ] Implement `connect_one`:

```rust
async fn connect_one(
    entry_config: ManagedMcpServerConfig,
    attempt_id: u64,
    supervisor: ProcessSupervisor,
) -> ConnectOutcome
```

`ConnectOutcome` should include:

- connected adapter,
- filtered tools,
- initial resource list attempt result if cheap,
- diagnostic on failure.

- [ ] Ensure one failed server does not fail the whole manager.

- [ ] Implement `shutdown`.

- [ ] Add tests:

Expected:

- `apply_config` connects two good servers and one bad server, returns connected snapshots for good servers and failed snapshot for the bad one.
- Removing a server removes its tools from later registration.
- Updating command/url increments attempt id and ignores stale connect results.
- Shutdown clears entries and calls supervised cleanup.

## Task 4: Reconnect And Health Behavior

**Files:**

- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp_manager.rs`

- [ ] Implement `reconnect_now(server_id)`.

- [ ] Implement passive failure handling for managed MCP tool calls:

When a manager-routed tool call gets an `McpError`, mark the server `Failed`, store diagnostic, remove its currently registered tools at the next sync, and schedule reconnect if policy allows.

- [ ] Implement bounded exponential backoff:

```rust
fn reconnect_delay_ms(policy: McpReconnectPolicy, attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(16);
    let raw = policy.initial_delay_ms.saturating_mul(1_u64 << shift);
    raw.min(policy.max_delay_ms)
}
```

- [ ] Avoid tight retry loops. If tests need deterministic behavior, configure:

```rust
McpReconnectPolicy {
    enabled: true,
    initial_delay_ms: 0,
    max_delay_ms: 0,
    max_attempts: Some(1),
}
```

- [ ] Implement `refresh_tools(server_id)`:

Behavior:

- Calls `tools/list` on the connected adapter.
- Reapplies enabled/disabled filters.
- Updates snapshot.
- Does not recreate the process unless the adapter reports a connection/protocol error.

- [ ] Implement `health_check(server_id)`:

Behavior:

- If connected, use `tools/list` as the minimal protocol check because Neo's adapter layer does not expose MCP ping.
- If check succeeds, update tools if the list changed.
- If check fails, mark failed and schedule reconnect.

- [ ] Add tests:

Expected:

- failed tool call changes status from `Connected` to `Failed`.
- `reconnect_now` restores `Connected` and tools.
- `refresh_tools` adds a newly advertised tool and removes a disappeared tool.
- backoff delay is capped at `max_delay_ms`.

## Task 5: Register MCP Tools Through The Manager

**Files:**

- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp_manager.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`

- [ ] Add a manager-routed MCP tool wrapper:

```rust
struct ManagedMcpTool {
    server_id: String,
    exposed_name: String,
    remote_name: String,
    description: String,
    input_schema: serde_json::Value,
    manager: McpConnectionManager,
}
```

- [ ] `ManagedMcpTool::execute` should call:

```rust
self.manager
    .call_tool(&self.server_id, &self.remote_name, input)
    .await
```

Then map errors to `ToolError::Mcp`.

- [ ] Register only tools from `Connected` servers.

- [ ] Detect collisions before registering:

If two remote tools sanitize to the same exposed name, register the first stable winner and store a diagnostic for the dropped one. Do not silently overwrite.

- [ ] Update `tool_registry_for_config` so broken MCP servers do not fail the whole registry:

```rust
let diagnostics = mcp_manager.register_connected_tools_into(&mut registry).await;
for diagnostic in diagnostics {
    tracing::warn!(server = %diagnostic.server_id, "MCP diagnostic: {}", diagnostic.message);
}
```

- [ ] Add tests:

Expected:

- one bad MCP server does not prevent built-in `Read`/`Bash` tool specs from existing.
- connected MCP server registers `mcp__server__tool`.
- failed MCP server registers no tools and produces a diagnostic.
- collision does not overwrite existing tool.

## Task 6: Sync Tool Specs At Request Boundaries

**Files:**

- Modify: `crates/neo-agent-core/src/runtime.rs`
- Test: `crates/neo-agent-core/tests/runtime_turn.rs`

- [ ] Change `chat_request` to accept current tool specs rather than always cloning `config.tools`.

Suggested signature:

```rust
async fn chat_request(
    config: &AgentConfig,
    context: &AgentContext,
    tools: Vec<ToolSpec>,
) -> ChatRequest
```

- [ ] In `run_agent_turn`, compute current specs immediately before each model call.

Suggested helper:

```rust
fn current_tool_specs(
    registry: Option<&ToolRegistry>,
    skills: Option<&SkillStore>,
) -> Vec<ToolSpec> {
    let mut specs = registry.map_or_else(Vec::new, ToolRegistry::specs);
    if skills.is_some() {
        specs.push(invoke_skill_tool_spec());
    }
    specs
}
```

- [ ] Preserve `AgentConfig.tools` for serialized/debug snapshots if needed, but do not treat it as the source of truth for every model request.

- [ ] Add runtime test:

Scenario:

1. First model request sees MCP tool `mcp__docs__search`.
2. Between tool result and next model request, test code refreshes manager/registry so `mcp__docs__lookup` replaces it.
3. Second model request includes `mcp__docs__lookup`.

If the production implementation does not support mutating the registry inside an active runtime, replace the test with:

1. First turn uses old tool list.
2. Config/manager refresh runs.
3. Second turn uses new tool list.

The handoff preference is request-boundary updates because it better matches NEO-17.

## Task 7: Add Explicit MCP Resource Access

**Files:**

- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Optionally create: `crates/neo-agent-core/src/tools/mcp_resources.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp.rs`
- Test: `crates/neo-agent-core/tests/tool_mcp_manager.rs`

- [ ] Add manager methods:

```rust
pub async fn list_resources(
    &self,
    server_id: Option<&str>,
) -> anyhow::Result<Vec<McpResourceListEntry>>;

pub async fn read_resource(
    &self,
    server_id: &str,
    uri: &str,
) -> anyhow::Result<McpResourceRead>;
```

- [ ] Add list/read tool wrappers if model access is included:

```rust
pub struct ListMcpResourcesTool {
    manager: McpConnectionManager,
}

pub struct ReadMcpResourceTool {
    manager: McpConnectionManager,
}
```

- [ ] Register them only when a manager is attached. Do not add no-op fake tools when there is no MCP manager.

- [ ] Add output caps:

Rules:

- maximum resources listed per call: 200,
- maximum text returned from one resource: 64 KiB,
- blob content summarized as `<blob base64 bytes=N mime=...>`.

- [ ] Add tests:

Expected:

- list all resources across connected servers.
- list resources for one server.
- read one text resource.
- unknown server returns clear error.
- disabled server is not started.
- resource updates can be consumed by `next_resource_update` and reflected in snapshot state.

## Task 8: Config Hot Reload Service

**Files:**

- Modify: `crates/neo-agent/src/mcp_ops.rs`
- Modify: `crates/neo-agent/src/modes/interactive.rs`
- Test: `crates/neo-agent/src/modes/interactive.rs`

- [ ] Add reload helper:

```rust
pub async fn reload_mcp_manager_from_config(
    config: &AppConfig,
    manager: &McpConnectionManager,
) -> anyhow::Result<Vec<McpServerSnapshot>> {
    let managed = to_managed_configs(&config.mcp.servers)?;
    Ok(manager.apply_config(managed).await)
}
```

- [ ] After `neo mcp add/del/enable/disable`, ensure CLI paths use the same conversion/probe/manager code where possible. CLI commands are short-lived, so they do not need to keep a manager alive after command exit.

- [ ] In interactive controller, add fields:

```rust
mcp_manager: Option<McpConnectionManager>,
config_last_modified: Option<std::time::SystemTime>,
config_reload_interval: std::time::Duration,
last_config_reload_check: Option<std::time::Instant>,
```

- [ ] On startup:

1. Build manager from `local_config.mcp.servers`.
2. Apply config.
3. Store snapshots for NEO-32.

- [ ] On config mutation from NEO-32:

1. Persist mutation.
2. Call existing `refresh_config()`.
3. Call `reload_mcp_manager_from_config`.
4. Refresh overlay rows.

- [ ] On external config mtime change:

1. Reload config.
2. Apply only MCP diffs to manager.
3. Do not interrupt active turns.
4. New tools become available at the next request boundary or next launched turn.

- [ ] Add tests:

Expected:

- changing config file mtime triggers reload.
- invalid edited config preserves previous manager state and emits status.
- add/delete/toggle calls apply manager updates immediately.

## Task 9: CLI Diagnostics

**Files:**

- Modify: `crates/neo-agent/src/mcp_ops.rs`
- Modify: `crates/neo-agent/src/modes/run.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`

- [ ] Keep `neo mcp list` stable enough for existing docs/tests.

- [ ] Improve text output with status:

```text
[1]<filesystem>(studio) connected tools=12 resources=3
{"1":"read_file","2":"list_directory"}

[2]<linear>(remote-http) failed
error: HTTP 401 from https://mcp.linear.app/mcp
hint: Check remote MCP authorization headers or disable this server.
{}
```

- [ ] Consider adding `--json` only if the CLI test surface already has a pattern for it. If added:

```json
{
  "servers": [
    {
      "id": "filesystem",
      "transport": "stdio",
      "enabled": true,
      "status": "connected",
      "tool_count": 12,
      "resource_count": 3
    }
  ]
}
```

- [ ] Add tests:

Expected:

- failed server text includes server id and hint.
- disabled server says disabled and does not attempt startup.
- JSON, if added, redacts env/header values.

## Task 10: Documentation

**Files:**

- Modify: `docs/mcp.md`
- Modify: `docs/quickstart.md`
- Modify: `crates/neo-agent-core/src/skills/builtin/mcp-config.md`

- [ ] Update `docs/mcp.md` Current Status:

```markdown
Neo maintains MCP servers through a runtime connection manager. Enabled servers
are connected independently, failed servers are reported as unavailable, and
other tools remain usable. MCP resources can be listed/read explicitly and are
not injected into model context without a tool call.
```

- [ ] Document statuses:

```markdown
- disabled: configured but not started
- pending: startup/discovery is in progress
- connected: tools are available
- failed: startup or health check failed
- reconnecting: retry is scheduled or in progress
```

- [ ] Document hot reload:

```markdown
Changes made through `neo mcp add/del/enable/disable` are persisted to the
global config. Interactive sessions refresh their MCP manager from the config
and apply changes without a full TUI restart; active model requests see changed
tools at the next request boundary.
```

- [ ] Update built-in `mcp-config` skill so it no longer says changes only take effect in new sessions.

## Edge Cases And Pitfalls

- Do not hold locks across awaits. Clone state, drop lock, await adapter operation, reacquire, then compare attempt id.
- Do not silently overwrite tools when sanitized names collide.
- Do not start disabled servers during list, resources, or diagnostics.
- Do not let one bad server abort `tool_registry_for_config`.
- Do not show env/header values in TUI, CLI, logs, JSON, diagnostics, or session events.
- Do not claim resources were missing at adapter level; they already exist.
- Do not duplicate probe/discovery logic in NEO-32.
- Do not introduce OAuth or hosted registry work.
- Do not use a file watcher dependency unless a later issue approves it.
- Do not refresh model tools in the middle of a streaming model response.
- Do not remove an MCP tool from the registry while its execution future is running; removal should affect subsequent calls.
- Do not run broad workspace tests for this task unless the implementation changes shared runtime behavior substantially enough to warrant it.
- Do not run git mutations without explicit user authorization.

## Verification Plan

Focused commands:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core tool_mcp
rtk cargo run -p xtask -- test -p neo-agent-core tool_mcp_manager
rtk cargo run -p xtask -- test -p neo-agent-core runtime_turn
rtk cargo run -p xtask -- test -p neo-agent mcp_ops
rtk cargo run -p xtask -- test -p neo-agent --test cli_commands mcp
```

If Task 6 changes runtime request construction, include:

```bash
rtk cargo run -p xtask -- test -p neo-agent-core runtime_turn
```

If NEO-32 is implemented in the same branch, include:

```bash
rtk cargo run -p xtask -- test -p neo-tui mcp_manager
rtk cargo run -p xtask -- test -p neo-agent interactive mcp
```

Do not use bare `cargo test` as completion evidence.

## Self Review Checklist

- [ ] NEO-17 is clearly the runtime/service foundation and NEO-32 is clearly the TUI manager.
- [ ] Broken MCP server startup cannot block the main agent.
- [ ] Disabled servers are never started.
- [ ] Tool discovery failure includes server id and a useful hint.
- [ ] Resource APIs are explicit and not silently injected into context.
- [ ] Existing adapter-level resource support is reused.
- [ ] Config hot reload uses one shared path for CLI/TUI config changes.
- [ ] NEO-32 has a stable snapshot API to render status rows.
- [ ] No duplicate probe/discovery path is added in the TUI.
- [ ] No secrets are displayed or logged.
- [ ] Tool name collisions are detected instead of overwritten.
- [ ] Tool specs refresh at request boundaries if dynamic in-turn updates are implemented.
- [ ] Tests cover manager status transitions and failure isolation.

## Suggested Implementation Order

1. `mcp_ops.rs` extraction so CLI, runtime, and future TUI share parsing and conversion.
2. Core manager data types and snapshots.
3. Manager `apply_config`, connect, remove, shutdown.
4. Failure-isolated registration into `ToolRegistry`.
5. Reconnect, refresh, and health behavior.
6. Explicit resource list/read surfaces.
7. Runtime request-boundary tool spec sync if the patch includes dynamic tool updates.
8. Interactive config hot reload bridge for NEO-32.
9. CLI diagnostics.
10. Docs and focused verification.

## Handoff Notes For NEO-32

When NEO-32 is implemented after this:

- Replace static `McpServerRow` construction with `McpServerSnapshot` mapping.
- `R` should call `manager.refresh_tools(server_id)` or `manager.reconnect_now(server_id)` depending on status.
- `Enter` detail view should use `manager.snapshots()` plus `manager.list_resources(Some(server_id))`.
- Add/delete/toggle should persist config through `mcp_ops`, then call `reload_mcp_manager_from_config`.
- The overlay should never call `McpStdioToolAdapter` or `McpHttpToolAdapter` directly.
- The overlay should show env/header keys only.
- Mutating MCP config while an agent turn is active is allowed if it only affects subsequent model request boundaries; it must not cancel the active turn.

