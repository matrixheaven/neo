# Neo Extension Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Hard-delete Neo's extension product surface while keeping generic JSONL RPC infrastructure.

**Architecture:** Remove extension at every Neo-owned product boundary: CLI commands, runtime tool registration, TUI completion, tests, and docs. Keep MCP as the only external tool integration path and keep generic RPC where it is independently used by `neo rpc` and RPC codec tests. Do not read, migrate, clean, or document old extension data as supported behavior.

**Tech Stack:** Rust 2024, `clap`, `tokio`, `cargo nextest`, `rg`, existing Neo CLI/TUI/core crates.

---

## Preflight

- Read `AGENTS.md`, `docs/superpowers/specs/2026-07-01-neo-extension-removal-design.md`, and this plan.
- Run `git status --short` and identify unrelated dirty files before editing.
- Do not use `git reset`, `git checkout --`, `git restore`, `git stash`, `git clean`, `git rm`, or any branch mutation.
- Delete files with normal file edits or patch deletion, then stage with `git add`.
- Do not touch vendored references under `docs/pi`, `docs/codex`, or `docs/kimi-code`.

## File Map

- `crates/neo-agent/src/cli.rs`: remove `Command::Extensions` and `ExtensionCommand`.
- `crates/neo-agent/src/main.rs`: remove extension imports, dispatch arm, path helpers, and `dispatch_extensions`.
- `crates/neo-agent/src/extension_commands.rs`: delete the extension CLI command implementation file.
- `crates/neo-agent-core/src/tools/mod.rs`: remove `pub mod extensions`.
- `crates/neo-agent-core/src/tools/extensions/`: delete extension-specific bridge, discovery, installation, lifecycle, and runner modules.
- `crates/neo-agent/src/modes/run/runtime/agent.rs`: stop registering extension tools in the model tool registry.
- `crates/neo-agent/src/modes/interactive/prompt_completion.rs`: remove extension command completion discovery and source metadata.
- `crates/neo-agent/tests/cli_commands.rs`: delete extension lifecycle tests and add one unknown-subcommand test.
- `crates/neo-agent/tests/mock_provider_e2e.rs`: turn extension tool registration coverage into a negative assertion, then remove extension fixtures.
- `crates/neo-agent-core/tests/extension_runner.rs`: delete the extension runner test file.
- `crates/neo-agent-core/tests/rpc_jsonl.rs`: keep RPC tests but rename extension-flavored fixture method names.
- `crates/neo-agent/src/modes/interactive/tests.rs`: remove extension completion fixture/source assertions.
- `README.md`, `AGENTS.md`, `docs/index.md`, `docs/packages.md`, `docs/quickstart.md`, `docs/architecture.md`: remove Neo-owned extension documentation.
- `docs/superpowers/plans/2026-06-22-neo-35-extensions-host-plugin-api-handoff.md`: delete the obsolete extension plan.

### Task 1: Remove The CLI Extension Product Surface

**Files:**
- Modify: `crates/neo-agent/tests/cli_commands.rs`
- Modify: `crates/neo-agent/src/cli.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Delete: `crates/neo-agent/src/extension_commands.rs`

- [ ] **Step 1: Add the failing unknown-subcommand test**

In `crates/neo-agent/tests/cli_commands.rs`, add this test near `removed_remote_cli_surfaces_fail_parser`:

```rust
#[test]
fn extensions_subcommand_is_unknown() {
    let output = neo()
        .args(["extensions", "list"])
        .output()
        .expect("neo command should run");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized subcommand"));
    assert!(stderr.contains("extensions"));
}
```

- [ ] **Step 2: Run the new test and verify it fails**

Run:

```bash
cargo nextest run -p neo-agent --test cli_commands extensions_subcommand_is_unknown
```

Expected: FAIL because `neo extensions list` is still accepted and exits successfully.

- [ ] **Step 3: Remove extension CLI parsing**

In `crates/neo-agent/src/cli.rs`, remove the `Extensions` variant from `Command`:

```rust
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 在标准输入/文件上运行一次代理任务
    Run {
        #[arg(long, value_enum)]
        output: Option<RunOutput>,
        prompt: Vec<String>,
    },
    /// 恢复指定会话并进入交互模式
    Resume { session_id: Option<String> },
    /// 会话管理
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// 模型提供商管理
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    /// 模型管理
    Models {
        #[command(subcommand)]
        command: ModelCommand,
    },
    /// MCP 服务器管理
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// JSONL RPC 服务端模式
    Rpc,
    /// 工作区信任管理
    Trust {
        #[command(subcommand)]
        command: TrustCommand,
    },
}
```

Delete the entire `ExtensionCommand` enum from `crates/neo-agent/src/cli.rs`.

- [ ] **Step 4: Remove extension CLI dispatch**

In `crates/neo-agent/src/main.rs`, delete `mod extension_commands;`.

Update the `use crate::cli` import so it no longer imports `ExtensionCommand`:

```rust
use crate::{
    cli::{CatalogCommand, Cli, Command, McpCommand, ModelCommand, ProviderCommand, SessionCommand},
    config::{AppConfig, ConfigOverrides},
};
```

Remove this match arm from `dispatch_command`:

```rust
Some(Command::Extensions { command }) => dispatch_extensions(config, command).await,
```

Delete these items from `crates/neo-agent/src/main.rs`:

```rust
async fn dispatch_extensions(
    config: &AppConfig,
    command: ExtensionCommand,
) -> anyhow::Result<String> {
    match command {
        ExtensionCommand::List { root } => {
            let paths = extension_paths(config, root);
            extension_commands::list(&paths.root, &paths.state_path, &paths.registry_path)
        }
        ExtensionCommand::Install { source, root } => {
            let paths = extension_paths(config, root);
            extension_commands::install(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &source,
            )
        }
        ExtensionCommand::Update { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::update(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &extension_id,
            )
        }
        ExtensionCommand::Uninstall { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::uninstall(
                &paths.root,
                &paths.state_path,
                &paths.registry_path,
                &extension_id,
            )
        }
        ExtensionCommand::Status { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::status(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Enable { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::enable(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Disable { extension_id, root } => {
            let paths = extension_paths(config, root);
            extension_commands::disable(&paths.root, &paths.state_path, &extension_id)
        }
        ExtensionCommand::Call {
            extension_id,
            method,
            params,
            root,
        } => {
            let paths = extension_paths(config, root);
            extension_commands::call(
                &paths.root,
                &paths.state_path,
                &extension_id,
                &method,
                &params,
            )
            .await
        }
    }
}

struct ExtensionPaths {
    root: PathBuf,
    state_path: PathBuf,
    registry_path: PathBuf,
}

fn extension_paths(config: &AppConfig, root: PathBuf) -> ExtensionPaths {
    let neo_home = crate::config::neo_home().unwrap_or_else(|| config.project_dir.join(".neo"));
    ExtensionPaths {
        root: resolve_default_extension_root(&neo_home, root),
        state_path: neo_home.join("extensions-state.toml"),
        registry_path: neo_home.join("extensions-sources.toml"),
    }
}

fn resolve_default_extension_root(neo_home: &Path, root: PathBuf) -> PathBuf {
    if root == Path::new("extensions") || root == Path::new(".neo/extensions") {
        neo_home.join("extensions")
    } else {
        root
    }
}
```

After this edit, if the compiler reports `unused import: Path` or `unused import: PathBuf` in `crates/neo-agent/src/main.rs`, remove that name from the `std::path::{...}` import and rerun the same CLI test.

- [ ] **Step 5: Delete extension command implementation and positive CLI tests**

Delete `crates/neo-agent/src/extension_commands.rs`.

In `crates/neo-agent/tests/cli_commands.rs`, delete these extension-positive tests:

```rust
fn extensions_list_discovers_manifests()
fn extensions_install_update_and_list_sources_from_local_directory()
fn extensions_defaults_use_project_config_directory_when_invoked_from_another_cwd()
fn extensions_uninstall_removes_install_dir_and_source_entry()
fn extensions_call_round_trips_json_rpc()
fn extensions_lifecycle_commands_persist_status_and_gate_call()
```

Also remove this extension-only helper function if no remaining CLI test calls it:

```rust
fn write_extension_manifest(root: &std::path::Path, id: &str, name: &str, version: &str)
```

If `removed_remote_cli_surfaces_fail_parser` still contains extension subcommand cases, remove only these two cases because `extensions` itself is now fully unknown and covered by `extensions_subcommand_is_unknown`:

```rust
vec!["extensions", "search", "echo"],
vec!["extensions", "install", "echo", "--from", "marketplace"],
```

- [ ] **Step 6: Run the CLI test**

Run:

```bash
cargo nextest run -p neo-agent --test cli_commands extensions_subcommand_is_unknown
```

Expected: PASS.

- [ ] **Step 7: Commit CLI removal**

Run:

```bash
git add -A crates/neo-agent/src/cli.rs crates/neo-agent/src/main.rs crates/neo-agent/src/extension_commands.rs crates/neo-agent/tests/cli_commands.rs
git commit -m "refactor(agent): remove extension cli surface"
```

### Task 2: Remove Runtime Extension Tool Registration

**Files:**
- Modify: `crates/neo-agent/tests/mock_provider_e2e.rs`
- Modify: `crates/neo-agent/src/modes/run/runtime/agent.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/bridge.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/discovery.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/installation.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/lifecycle.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/mod.rs`
- Delete: `crates/neo-agent-core/src/tools/extensions/runner.rs`
- Delete: `crates/neo-agent-core/tests/extension_runner.rs`

- [ ] **Step 1: Rewrite the model-tool E2E as a negative test**

In `crates/neo-agent/tests/mock_provider_e2e.rs`, rename `run_text_registers_enabled_extension_tool_in_model_request` to `run_text_does_not_register_extension_tools_in_model_request` and replace the assertions with:

```rust
#[test]
fn run_text_does_not_register_extension_tools_in_model_request() {
    let temp = TempDir::new().expect("tempdir");
    write_echo_extension_at(&isolated_home_path().join("extensions/echo"));
    let server = MockSseServer::start(vec![openai_response_sse("resp-no-ext", "extension ignored")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "list tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "extension ignored\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let tool_names = model_tool_names(&requests[0].body);
    for name in &tool_names {
        assert_model_function_name_safe(name);
        assert!(
            !name.starts_with("extension__"),
            "unexpected extension tool registered: {name}"
        );
    }
    assert!(tool_names.contains(&"CreateSkill"));
    assert!(tool_names.contains(&"ListSkills"));
    assert!(tool_names.contains(&"MoveSkill"));
    assert!(tool_names.contains(&"SummarizeSessions"));
}
```

- [ ] **Step 2: Run the negative runtime test and verify it fails**

Run:

```bash
cargo nextest run -p neo-agent --test mock_provider_e2e run_text_does_not_register_extension_tools_in_model_request
```

Expected: FAIL because the old runtime still registers `extension__echo__echo`.

- [ ] **Step 3: Stop runtime extension registration**

In `crates/neo-agent/src/modes/run/runtime/agent.rs`, remove this block from `tool_registry_for_config`:

```rust
let extension_home =
    crate::config::neo_home().unwrap_or_else(|| config.project_dir.join(".neo"));
neo_agent_core::tools::extensions::register_enabled_extension_tools(
    &mut registry,
    &neo_agent_core::tools::extensions::default_extension_root(&extension_home),
    &neo_agent_core::tools::extensions::default_extension_state_path(&extension_home),
)
.await?;
```

The function should proceed from built-in registry creation directly to the existing MCP manager setup. The start of `tool_registry_for_config` should look like this:

```rust
pub(crate) async fn tool_registry_for_config(
    config: &AppConfig,
    todos: std::sync::Arc<std::sync::Mutex<Vec<neo_agent_core::TodoEventData>>>,
    mcp_manager: Option<&McpConnectionManager>,
) -> anyhow::Result<ToolRegistry> {
    let mut registry = ToolRegistry::with_builtin_tools_and_todos(todos);
    let manager;
    let manager_ref = if let Some(manager) = mcp_manager {
        manager
    } else {
        manager = McpConnectionManager::new(ProcessSupervisor::default());
        &manager
    };
    if let Err(error) = crate::mcp_ops::reload_mcp_manager_from_config(config, manager_ref).await {
```

- [ ] **Step 4: Remove the core extension module export**

In `crates/neo-agent-core/src/tools/mod.rs`, delete:

```rust
pub mod extensions;
```

- [ ] **Step 5: Delete extension core implementation and runner tests**

Delete these files:

```text
crates/neo-agent-core/src/tools/extensions/bridge.rs
crates/neo-agent-core/src/tools/extensions/discovery.rs
crates/neo-agent-core/src/tools/extensions/installation.rs
crates/neo-agent-core/src/tools/extensions/lifecycle.rs
crates/neo-agent-core/src/tools/extensions/mod.rs
crates/neo-agent-core/src/tools/extensions/runner.rs
crates/neo-agent-core/tests/extension_runner.rs
```

After deleting the files, delete the now-empty directory `crates/neo-agent-core/src/tools/extensions`.

- [ ] **Step 6: Remove extension fixtures from the E2E test**

After the runtime deletion makes the negative test pass, simplify `crates/neo-agent/tests/mock_provider_e2e.rs` by deleting these helper functions if `rg -n "write_echo_extension_at|write_named_echo_extension_at" crates/neo-agent/tests/mock_provider_e2e.rs` shows no remaining call sites other than the definitions:

```rust
fn write_echo_extension_at(extension: &std::path::Path) -> std::path::PathBuf
fn write_named_echo_extension_at(
    extension: &std::path::Path,
    extension_id: &str,
) -> std::path::PathBuf
```

Then update the negative test to create inert leftover data without any executable extension runner:

```rust
#[test]
fn run_text_does_not_register_extension_tools_in_model_request() {
    let temp = TempDir::new().expect("tempdir");
    let leftover = isolated_home_path().join("extensions/echo");
    std::fs::create_dir_all(&leftover).expect("create leftover extension dir");
    std::fs::write(
        leftover.join("neo-extension.toml"),
        r#"
id = "echo"
name = "Echo"
version = "0.1.0"

[runner]
command = "python3"
"#,
    )
    .expect("write leftover extension manifest");
    let server = MockSseServer::start(vec![openai_response_sse("resp-no-ext", "extension ignored")]);
    write_mock_responses_config(&temp, &server.url);

    let mut command = neo();
    command
        .current_dir(temp.path())
        .env("OPENAI_API_KEY", "test-key")
        .args(["run", "--output", "text", "list tools"]);

    let stdout = run(command);

    assert_eq!(stdout, "extension ignored\n");
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let tool_names = model_tool_names(&requests[0].body);
    for name in &tool_names {
        assert_model_function_name_safe(name);
        assert!(
            !name.starts_with("extension__"),
            "unexpected extension tool registered: {name}"
        );
    }
    assert!(tool_names.contains(&"CreateSkill"));
    assert!(tool_names.contains(&"ListSkills"));
    assert!(tool_names.contains(&"MoveSkill"));
    assert!(tool_names.contains(&"SummarizeSessions"));
}
```

- [ ] **Step 7: Run the runtime test**

Run:

```bash
cargo nextest run -p neo-agent --test mock_provider_e2e run_text_does_not_register_extension_tools_in_model_request
```

Expected: PASS.

- [ ] **Step 8: Commit runtime removal**

Run:

```bash
git add -A crates/neo-agent/tests/mock_provider_e2e.rs crates/neo-agent/src/modes/run/runtime/agent.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/tools/extensions crates/neo-agent-core/tests/extension_runner.rs
git commit -m "refactor(core): remove extension tool runtime"
```

### Task 3: Remove Extension Completion Plumbing

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Rewrite the completion test to exclude extensions**

In `crates/neo-agent/src/modes/interactive/tests.rs`, rename `autocomplete_source_model_merges_local_commands_and_provider_models_with_metadata` to `completion_catalog_excludes_extension_commands`.

Replace the `CompletionCatalog` setup and slash assertions with:

```rust
let catalog = CompletionCatalog {
    slash_prompts: vec![PickerItem::new(
        "/review",
        "/review",
        Some("Review project changes"),
    )],
    prompt_packages: vec![PickerItem::new(
        "/review-package",
        "/review-package",
        Some("Packaged review prompt"),
    )],
    session_commands: vec![PickerItem::new(
        "/review-session",
        "/review-session",
        Some("Session command"),
    )],
    model_items: vec![PickerItem::new(
        "anthropic/claude-sonnet",
        "anthropic/claude-sonnet",
        Some("Messages"),
    )],
};
```

Replace the slash-source assertions with:

```rust
let slash =
    completion_source_candidates(temp.path(), "/rev", &catalog).expect("slash completions");
let slash_sources = slash
    .iter()
    .map(|candidate| candidate.source)
    .collect::<Vec<_>>();
assert!(slash_sources.contains(&CompletionSource::SlashPrompt));
assert!(slash_sources.contains(&CompletionSource::PromptPackage));
assert!(slash_sources.contains(&CompletionSource::SessionCommand));
assert!(slash.iter().all(|candidate| {
    candidate
        .to_picker_item()
        .description
        .as_deref()
        .is_none_or(|description| !description.contains("extension command"))
}));
```

- [ ] **Step 2: Run the completion test and verify it fails**

Run:

```bash
cargo nextest run -p neo-agent --lib completion_catalog_excludes_extension_commands
```

Expected: FAIL to compile because `CompletionCatalog` still requires `extension_commands`, or fail at assertions if the code still exposes extension completion sources.

- [ ] **Step 3: Remove extension completion discovery**

In `crates/neo-agent/src/modes/interactive/prompt_completion.rs`, remove the `Context` import if it is only used by extension completion:

```rust
use anyhow::Result;
```

Update `prompt_completions` so the catalog no longer requests extension commands:

```rust
let catalog = CompletionCatalog {
    slash_prompts: slash_prompt_template_completion_items(root, prefix, project_trusted)
        .unwrap_or_default(),
    prompt_packages: prompt_package_completion_items(root, project_trusted)?,
    session_commands: session_completion_items(skill_store),
    model_items: model_items.to_vec(),
};
```

Delete the complete definitions of these functions:

```rust
fn extension_command_completion_items(root: &Path, project_trusted: bool) -> Result<Vec<PickerItem>>
fn discover_extension_commands(extension_root: &Path) -> Result<Vec<PickerItem>>
```

Update `CompletionCatalog`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CompletionCatalog {
    pub(super) slash_prompts: Vec<PickerItem>,
    pub(super) prompt_packages: Vec<PickerItem>,
    pub(super) session_commands: Vec<PickerItem>,
    pub(super) model_items: Vec<PickerItem>,
}
```

Update `CompletionSource`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(super) enum CompletionSource {
    LocalFile,
    SlashPrompt,
    PromptPackage,
    SessionCommand,
    ProviderModel,
}
```

Update `CompletionSource::label`:

```rust
impl CompletionSource {
    const fn label(self) -> &'static str {
        match self {
            Self::LocalFile => "local file",
            Self::SlashPrompt => "slash prompt",
            Self::PromptPackage => "prompt package",
            Self::SessionCommand => "session command",
            Self::ProviderModel => "provider model",
        }
    }
}
```

Update `slash_source_candidates`:

```rust
fn slash_source_candidates(prefix: &str, catalog: &CompletionCatalog) -> Vec<CompletionCandidate> {
    let sources = [
        (&catalog.slash_prompts, CompletionSource::SlashPrompt),
        (&catalog.prompt_packages, CompletionSource::PromptPackage),
        (&catalog.session_commands, CompletionSource::SessionCommand),
    ];
    sources
        .into_iter()
        .flat_map(|(items, source)| {
            items
                .iter()
                .filter(move |item| item.value.starts_with(prefix))
                .cloned()
                .map(move |item| CompletionCandidate::from_picker(item, source))
        })
        .collect()
}
```

After deleting `discover_extension_commands`, `PathBuf` should no longer be used in this file. Replace the path import with:

```rust
use std::path::Path;
```

- [ ] **Step 4: Run the completion test**

Run:

```bash
cargo nextest run -p neo-agent --lib completion_catalog_excludes_extension_commands
```

Expected: PASS.

- [ ] **Step 5: Commit completion removal**

Run:

```bash
git add crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "refactor(tui): remove extension completions"
```

### Task 4: Keep RPC Generic And Remove Extension-Flavored Fixtures

**Files:**
- Modify: `crates/neo-agent-core/tests/rpc_jsonl.rs`

- [ ] **Step 1: Rewrite the generic RPC fixture name**

In `crates/neo-agent-core/tests/rpc_jsonl.rs`, update `jsonl_codec_encodes_and_decodes_rpc_messages` so the sample request method is `rpc.describe`, not `extension.describe`:

```rust
let request = RpcMessage::Request(RpcRequest::new(
    "req-1",
    "rpc.describe",
    json!({ "name": "alpha" }),
));
```

- [ ] **Step 2: Run the RPC test**

Run:

```bash
cargo nextest run -p neo-agent-core --test rpc_jsonl jsonl_codec_encodes_and_decodes_rpc_messages
```

Expected: PASS.

- [ ] **Step 3: Commit RPC fixture cleanup**

Run:

```bash
git add crates/neo-agent-core/tests/rpc_jsonl.rs
git commit -m "test(core): keep rpc fixtures extension-neutral"
```

### Task 5: Remove Neo-Owned Extension Documentation And Old Plan

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`
- Modify: `docs/index.md`
- Modify: `docs/packages.md`
- Modify: `docs/quickstart.md`
- Modify: `docs/architecture.md`
- Delete: `docs/superpowers/plans/2026-06-22-neo-35-extensions-host-plugin-api-handoff.md`

- [ ] **Step 1: Scan Neo-owned docs for extension references**

Run:

```bash
rg -n "extensions?|Extension|extension__|neo-extension|NEO-35|host plugin" README.md AGENTS.md docs/*.md docs/superpowers
```

Expected before editing: hits in Neo-owned docs and old plans. Hits under `docs/pi`, `docs/codex`, and `docs/kimi-code` are not part of this scan.

- [ ] **Step 2: Update README crate description**

In `README.md`, replace the `neo-agent-core` bullet that currently mentions local extensions with:

```md
- `neo-agent-core`: agent loop, tools, permissions, sessions, MCP, skill loading, JSONL RPC, and HTML export
```

- [ ] **Step 3: Update AGENTS.md crate and runtime references**

In `AGENTS.md`, update the `neo-agent-core` crate row to remove extension adapters:

```md
| `neo-agent-core` | `AgentRuntime`, `ToolRegistry`, built-in tools, `PermissionMode`, sessions, MCP adapters, skills, RPC, export. |
```

Delete the "Extension & MCP namespacing" subsection and replace it with:

```md
### MCP namespacing

- MCP: `mcp__<server>__<tool>` via `McpStdioToolAdapter` / `McpHttpToolAdapter`. Resources are runtime state, not model context.
```

- [ ] **Step 4: Update top-level docs index and architecture docs**

In `docs/index.md`, remove local extensions from the local assets and crate descriptions. Use this wording where a local-assets description is still needed:

```md
- [Local Assets](packages.md) - prompt templates, themes, and unsupported local asset notes.
```

In `docs/architecture.md`, remove local extensions from the architecture summary. Keep MCP, skills, JSONL RPC, sessions, and export references intact.

- [ ] **Step 5: Rewrite packages and quickstart docs**

In `docs/packages.md`, remove the "Extensions" section entirely. The document should describe prompt templates and themes only. If the title mentions extensions, rename it to:

```md
# Local Prompt And Theme Assets
```

In `docs/quickstart.md`, remove extension CLI examples and extension feature paragraphs. Keep local skills and MCP setup examples intact.

- [ ] **Step 6: Delete old extension planning doc**

Delete this file:

```text
docs/superpowers/plans/2026-06-22-neo-35-extensions-host-plugin-api-handoff.md
```

Do not archive it and do not replace it with a superseded note.

- [ ] **Step 7: Run the Neo-owned docs scan**

Run:

```bash
rg -n "neo extensions|extension__|neo-extension\\.toml|tools::extensions|ExtensionCommand|ExtensionRunner|ExtensionLifecycle|NEO-35|host plugin" README.md AGENTS.md docs/*.md docs/superpowers
```

Expected: no output from Neo-owned docs/plans.

- [ ] **Step 8: Commit docs cleanup**

Run:

```bash
git add -A README.md AGENTS.md docs/index.md docs/packages.md docs/quickstart.md docs/architecture.md docs/superpowers/plans/2026-06-22-neo-35-extensions-host-plugin-api-handoff.md
git commit -m "docs: remove neo extension product docs"
```

### Task 6: Final Residue Scan And Focused Verification

**Files:**
- Verify only; no planned source edits unless the scans find missed extension residue.

- [ ] **Step 1: Run the final source residue scan**

Run:

```bash
rg -n "neo extensions|extension__|neo-extension\\.toml|tools::extensions|ExtensionCommand|ExtensionRunner|ExtensionLifecycle" crates README.md AGENTS.md docs/*.md docs/superpowers
```

Expected: no output.

- [ ] **Step 2: Run a broader Neo-owned extension scan**

Run:

```bash
rg -n "extensions?|Extension" crates README.md AGENTS.md docs/*.md docs/superpowers
```

Expected: any remaining hits must be ordinary English/Rust terms not naming the removed Neo extension product. Examples that may remain if they are not product references: file extension handling in markdown or image code, Rust extension traits, or browser/OS extension wording. Do not touch vendored references.

- [ ] **Step 3: Run focused tests**

Run:

```bash
cargo nextest run -p neo-agent --test cli_commands extensions_subcommand_is_unknown
cargo nextest run -p neo-agent --test mock_provider_e2e run_text_does_not_register_extension_tools_in_model_request
cargo nextest run -p neo-agent --lib completion_catalog_excludes_extension_commands
cargo nextest run -p neo-agent-core --test rpc_jsonl jsonl_codec_encodes_and_decodes_rpc_messages
```

Expected: all PASS.

- [ ] **Step 4: Run formatting**

Run:

```bash
cargo fmt --all --check
```

Expected: PASS.

- [ ] **Step 5: Run clippy if compile feedback touched both agent and core library surfaces**

Run these if the previous tasks changed public imports or module exports in ways that could leave warnings outside the focused tests:

```bash
cargo clippy -p neo-agent --lib -- -D warnings
cargo clippy -p neo-agent-core --lib -- -D warnings
```

Expected: PASS. If clippy fails due unrelated dirty worktree changes, do not fix unrelated files; record the exact failing target and error.

- [ ] **Step 6: Store ICM completion context**

Run:

```bash
icm store -t context-neo -c "Completed hard removal of Neo extension product surface: CLI, runtime tool registration, TUI completion, extension-specific tests, Neo-owned docs, and old NEO-35 plan removed; generic JSONL RPC and MCP retained." -i high -k "neo,extensions,removal,mcp,rpc"
```

Expected: ICM reports a stored record. If embedding prints a warning but returns a line beginning with `Stored:`, treat it as success.

- [ ] **Step 7: Commit final verification fixes if any**

If Task 6 required any cleanup edits, commit them:

```bash
git add -A crates README.md AGENTS.md docs
git commit -m "chore: remove remaining extension residue"
```

If Task 6 found no edits, do not create an empty commit.
