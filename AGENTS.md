# Neo Agent Workspace — Agent Guide

This file is written for AI coding agents. It assumes you know nothing about the
project. Use it to orient yourself before editing code, running tests, or writing
documentation.



@ [CX.md](../../.kimi-code/CX.md)  @[RTK.md](../../.kimi-code/RTK.md) 

## Project overview

Neo is a Rust-native, local AI coding agent monorepo inspired by `pi`. It is
intentionally local-only: the supported surface is a command-line / terminal UI
application that talks to configured model providers, runs tools inside a
project workspace, persists sessions as local JSONL files, and loads local
extensions, prompt templates, and MCP servers. It does **not** start hosted
services, profile sync, marketplace, or collaboration features.

The repository is a Cargo workspace. The root also contains several unrelated
vendored/reference directories (`claude-code`, `codex`, `kimi-code`, `opencode`,
`pi`) that are ignored by `.gitignore` and are **not** part of the Neo Rust
workspace.

Key files at the root:

- `Cargo.toml` — workspace manifest, members, shared dependencies, lints.
- `rust-toolchain.toml` — stable Rust with `rustfmt` and `clippy`.
- `Cargo.lock` — pinned dependency graph.
- `README.md` — short project summary and common commands.
- `xtask/` — repository maintenance automation (fmt/clippy/test/docs parity).

## Technology stack

- **Language**: Rust, edition 2024, minimum `rust-version = "1.88"`.
- **Toolchain**: stable Rust, `rustfmt`, `clippy`.
- **Async runtime**: `tokio` (`rt-multi-thread`, macros, process, fs, signal,
  time, sync, net).
- **Networking**: `reqwest` with `rustls-tls`, streaming, JSON.
- **CLI**: `clap` derive-based parser in `crates/neo-agent/src/cli.rs`.
- **TUI**: `ratatui` + `crossterm`, with inline image protocols
  (Kitty, iTerm2, Sixel) and bracketed-paste handling.
- **Serialization / schemas**: `serde`, `serde_json`, `schemars`, `toml`.
- **Markdown**: `pulldown-cmark` for rendering and export.
- **Database**: `rusqlite` (bundled) where persistent structured storage is
  needed.
- **Tracing**: `tracing` / `tracing-subscriber`.
- **Provider protocols**: OpenAI Responses, OpenAI-compatible Chat Completions,
  Anthropic Messages, Google Generative AI, OpenAI-style image generation,
  local fake provider for tests.
- **Extension protocols**: stdio/HTTP/SSE JSONL RPC for local extensions;
  stdio/HTTP/SSE MCP (Model Context Protocol) JSON-RPC adapters.

## Workspace layout

```text
.
├── Cargo.toml                 # workspace root
├── rust-toolchain.toml        # stable + rustfmt + clippy
├── README.md
├── crates/
│   ├── ai                     # neo-ai: provider-neutral chat/types/options/registry
│   ├── agent-core             # neo-agent-core: runtime, tools, permissions, sessions, MCP
│   ├── sdk                    # neo-sdk: JSONL RPC, skill loading, HTML export
│   ├── extensions             # neo-extensions: local extension discovery/runner/lifecycle
│   ├── tui                    # neo-tui: reusable terminal UI primitives
│   └── neo-agent              # neo-agent: CLI/TUI binary crate (binary name: neo)
├── examples/
│   ├── rust/                  # Cargo crate with runnable Rust examples
│   ├── config/                # example TOML config files
│   └── tools/                 # example tool schemas
├── docs/                      # Markdown documentation and gap map
│   └── gap/                   # module-by-module pi parity status
├── xtask/                     # maintenance command runner
└── target/                    # build output
```

### Crate responsibilities

| Crate | Package name | Public API role |
|-------|--------------|-----------------|
| `crates/ai` | `neo-ai` | Provider-neutral `ChatRequest`, `ModelClient`, `AiStreamEvent`, registries, reasoning options, image generation, `FakeModelClient`. |
| `crates/agent-core` | `neo-agent-core` | `AgentRuntime`, `AgentContext`, `ToolRegistry`, built-in tools, `PermissionPolicy`, `FakeHarness`, JSONL session helpers, MCP adapters. |
| `crates/sdk` | `neo-sdk` | JSONL RPC frame types, skill manifest helpers, safe Markdown-to-HTML export. |
| `crates/extensions` | `neo-extensions` | `neo-extension.toml` discovery, installation, lifecycle, stdio JSONL runner. |
| `crates/tui` | `neo-tui` | Reusable terminal UI components, input handling, diff rendering, inline image encoding. |
| `crates/neo-agent` | `neo-agent` | The `neo` binary. Parses args, loads config, dispatches to `print`/`run`/`resume`/sessions/extensions/MCP/RPC/TUI modes. |
| `xtask` | `xtask` | Maintenance commands: check, parity, release-smoke, catalog check. |

## Build and test commands

The entry-point binary is built from the `neo-agent` crate. The binary name is
`neo`.

```bash
# Build the CLI binary
cargo build -p neo-agent

# Run the CLI from source
cargo run -p neo-agent -- --help
cargo run -p neo-agent -- models list
cargo run -p neo-agent -- print "hello from neo"
```

Maintenance gates live in `xtask`:

```bash
# Default stable gate: only checks xtask itself. Use this while other crates
# are under active construction.
cargo run -p xtask -- check

# Docs/examples parity gate plus the stable gate.
cargo run -p xtask -- check --docs

# Full workspace fmt/clippy/test gate.
cargo run -p xtask -- check --workspace

# Just the docs/examples parity gate.
cargo run -p xtask -- parity

# Validate generated model-catalog schema artifacts if they exist.
cargo run -p xtask -- catalog check

# Local-only release smoke: help, models, sessions, extensions, MCP fixture,
# catalog check, docs parity.
cargo run -p xtask -- release-smoke
```

Unit and integration tests:

```bash
# All workspace tests
cargo test --workspace --all-features

# Individual crate
cargo test -p neo-ai
cargo test -p neo-agent-core
cargo test -p neo-extensions
cargo test -p neo-sdk
cargo test -p neo-tui
cargo test -p neo-agent
cargo test -p xtask
```

## Code style guidelines

- Rust edition 2024; use `async fn` in traits where appropriate.
- Workspace lints in `Cargo.toml`:
  - `unsafe_code = "forbid"`
  - `clippy::pedantic` is warned by default.
  - `missing_errors_doc`, `missing_panics_doc`, and `module_name_repetitions`
    are explicitly allowed.
- Run `cargo fmt --all --check` before claiming a change is clean.
- Run `cargo clippy --workspace --all-targets --all-features -- -D warnings`
  for full checks.
- Prefer typed Rust interfaces first; add wire protocols (MCP, JSON-RPC, JSONL)
  at crate boundaries.
- Keep provider-specific code behind `ModelClient` implementations in
  `crates/ai/src/providers/`.
- Keep model-facing tool schemas small and stable; avoid leaking runtime
  internals into schemas.
- Session events are normalized `AgentEvent` values; JSONL persistence should not
  depend on provider-native wire formats.

## Testing instructions

- Tests live next to source in `src/` (`#[cfg(test)]`) and in `tests/` per
crate.
- Use `neo_ai::providers::fake::FakeModelClient` and
  `neo_agent_core::harness::FakeHarness` for deterministic model-driven tests.
- Integration tests for CLI surfaces are in `crates/neo-agent/tests/`.
- TUI tests use a simulated terminal shell in `crates/tui/tests/`.
- The `xtask` parity gate validates that docs, examples, and source stay
  consistent:
  - Local Markdown link checks.
  - Scans for production fake/local/placeholder guidance.
  - Auth-token leak scans in docs/export examples.
  - Example TOML/JSON validation and Rust example compilation.
  - Stale gap-claim scans driven by source symbols.
- If a doc/example line is intentionally a fixture, prefix it with:
  `# xtask-parity: allow <reason>`.

## Runtime architecture

1. `neo-agent` parses CLI args (with some `pi`-style short aliases normalised in
   `main.rs`) and loads merged config from CLI → environment → project
   `.neo/config.toml` → `~/.neo/config.toml` → built-in defaults.
2. The runtime opens or creates a local JSONL session via
   `neo_agent_core::session`.
3. The configured model is resolved through `neo_ai::ModelRegistry`
   (seeded catalog + strict local JSON catalogs) and
   `neo_ai::ProviderRegistry::production()`.
4. `AgentRuntime` sends a `neo_ai::ChatRequest` to the selected `ModelClient`.
5. Provider-native streams are normalized into `AiStreamEvent` values
   (`MessageStart`, `ThinkingStart/Delta/End`, `TextDelta`, `ToolCallStart`,
   `ToolCallArgsDelta`, `ToolCallEnd`, `MessageEnd`, `Error`).
6. Tool calls are authorized against `PermissionPolicy`, executed by the
   `ToolRegistry`, and returned as `ChatMessage::ToolResult`.
7. Reasoning events are preserved as `ContentPart::Thinking` blocks, not mixed
   into plain assistant text.
8. Session events are appended to local JSONL so `resume` can reconstruct
   conversation and tool state.

### Built-in tools

Registered by `neo_agent_core::ToolRegistry::with_builtin_tools()`:

- `read`, `list`, `grep`, `find` — file read tools.
- `write`, `edit` — file write tools (`edit` returns a unified diff in details).
- `bash` — shell execution with foreground/background modes and process-group
  cleanup.
- `terminal` — PTY session tool (`start`, `write`, `read`, `resize`, `stop`).

All file paths are resolved inside `ToolContext::workspace_root()`; escaping the
workspace is rejected before execution.

### Extension and MCP boundaries

- Local extensions expose tools through JSONL RPC `tools.list` and are
  advertised to the model as `extension__<id>__<tool>`.
- Configured MCP servers expose tools as `mcp__<server>__<tool>` and execute via
  `McpStdioToolAdapter` or `McpHttpToolAdapter`.
- MCP resources are fetched only through explicit `neo mcp resources` commands;
  they are not silently injected into model context.

## Configuration model

Config precedence:

1. CLI flags.
2. `NEO_*` environment variables.
3. Project `.neo/config.toml` (or path from `--config` / `NEO_CONFIG`).
4. User-global `~/.neo/config.toml`.
5. Built-in defaults (`openai/gpt-4.1`).

Project config merges over user-global config field by field. Provider maps are
merged by provider id; MCP servers are merged by server id. Important sections:

- `default_provider`, `default_model`, `api_key_env`, `api_base`.
- `providers.<id>.api_base` / `api_key_env`.
- `model_catalogs` — strict local JSON model catalogs.
- `permissions` — `Allow` / `Ask` / `Deny` for `file_read`, `file_write`,
  `shell`, `tool`.
- `runtime` — `temperature`, `max_tokens`, `reasoning_effort`, queue modes,
  tool execution mode, compaction.
- `tui` — `image_protocol`, `fetch_remote_images`, `keybindings`.
- `mcp.servers` — stdio/HTTP/SSE MCP server entries.

System prompt resources:

- `.neo/SYSTEM.md` and `~/.neo/SYSTEM.md`.
- `.neo/APPEND_SYSTEM.md` and `~/.neo/APPEND_SYSTEM.md`.

Project context files (`AGENTS.md`, `CLAUDE.md`) are loaded only when the
project is trusted; trust is stored in `~/.neo/trust.json`.

## Security considerations

- **No unsafe code**: workspace lint `unsafe_code = "forbid"`.
- **Secrets**: API keys are read from environment variables, never from config
  files. `neo config show` redacts MCP `env` and `headers` values.
- **Workspace containment**: built-in file tools resolve paths inside the
  workspace and reject parent-dir escapes.
- **Shell tool**: requires explicit `permissions.shell` and can be set to
  `Ask` / `Deny`.
- **Trust**: project `AGENTS.md` / `CLAUDE.md` are gated by `neo trust
  approve|deny|status`. User-global context files are always loaded.
- **Remote images**: fetching remote image URLs is disabled by default
  (`tui.fetch_remote_images = false`). When enabled, fetches require HTTP(S),
  an image content type, and a size guard.
- **MCP**: tool names are namespaced by server id; disabled servers are not
  started; resource updates are runtime state, not model context.
- **No hosted distribution**: the documented surface is local-only. Marketplace,
  package publisher identity, root trust chains, profile sync, and hosted
  collaboration are not supported.

## Deployment process

There is no hosted deployment target. The deliverable is a local binary:

```bash
cargo build --release -p neo-agent
# Binary at target/release/neo
```

Release smoke (`cargo run -p xtask -- release-smoke`) exercises local-only CLI
flows against temporary fixtures; it does not start cloud services or
marketplace fixtures.

## Development conventions and docs

- Documentation lives in `docs/`; start with `docs/index.md` and
  `docs/quickstart.md`.
- The gap map in `docs/gap/` tracks pi-parity status per module; check it
  before assuming a missing capability is a docs omission versus a code gap.
- The repo uses a generated catalog schema artifact convention. If a generated
  schema exists, `cargo run -p xtask -- catalog check` validates it.
- Example code is in `examples/rust/` as a separate workspace crate.
- Example config and tool schemas are in `examples/config/` and
  `examples/tools/`.

## Current workspace health (as of last exploration)

The repository had uncommitted modifications across several crates when this
file was written. A full workspace check currently fails:

- `crates/agent-core/src/session/mod.rs` has a borrow-check error (`id` moved
  and then borrowed).
- `crates/neo-agent/src/modes/interactive.rs` references symbols
  (`tree_order_sessions`, `SessionTreeRecord`) that do not exist in
  `crates/neo-agent/src/session_commands.rs`.

Consequently, `cargo check --workspace` and `cargo run -p xtask -- check
--workspace` do not pass. The narrower `cargo run -p xtask -- check` (xtask
only) succeeds. Fix the compile errors before running the full workspace gate or
the release smoke tests.

<!-- icm:start -->

## Persistent memory (ICM) — MANDATORY

This project uses [ICM](https://github.com/rtk-ai/icm) for persistent memory across sessions.
You MUST use it actively. Not optional.

### Recall (before starting work)

```bash
icm recall "query"                        # search memories
icm recall "query" -t "topic-name"        # filter by topic
icm recall-context "query" --limit 5      # formatted for prompt injection
```

### Store — MANDATORY triggers

You MUST call `icm store` when ANY of the following happens:

1. **Error resolved** → `icm store -t errors-resolved -c "description" -i high -k "keyword1,keyword2"`
2. **Architecture/design decision** → `icm store -t decisions-{project} -c "description" -i high`
3. **User preference discovered** → `icm store -t preferences -c "description" -i critical`
4. **Significant task completed** → `icm store -t context-{project} -c "summary of work done" -i high`
5. **Conversation exceeds ~20 tool calls without a store** → store a progress summary

Do this BEFORE responding to the user. Not after. Not later. Immediately.

Do NOT store: trivial details, info already in CLAUDE.md, ephemeral state (build logs, git status).

### Other commands

```bash
icm update <id> -c "updated content"     # edit memory in-place
icm health                                # topic hygiene audit
icm topics                                # list all topics
```

<!-- icm:end -->
