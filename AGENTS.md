# Neo Agent Workspace — Agent Guide

This file is written for AI coding agents. It assumes you know nothing about the
project. Use it to orient yourself before editing code, running tests, or writing
documentation.

Read @ [CX.md](../../.kimi-code/CX.md)  @[RTK.md](../../.kimi-code/RTK.md) ，Use `cx` and `rtk` clito save tokens

Swarm mode : Parallel at least 3 subagent swarm to finish a job

不接受任何反驳，这个项目将会同时有 N 个 AI Agent 并行开发，禁止一切犯贱行为：动 git 的任何回溯操作来方便你自己的工作，你必须立刻停下

不接受任何反驳，禁止去做不属于你的工作，别的 AI 整出来的报错，不关你任何事，100%专注于管好你自己的工作！

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
- **TUI**: `crossterm`-based custom terminal renderer (`InlineRenderer`) with
  differential rendering, inline image protocols (Kitty, iTerm2, Sixel),
  bracketed-paste handling, and a component-tree architecture.
- **Serialization / schemas**: `serde`, `serde_json`, `schemars`, `toml`.
- **Markdown**: `pulldown-cmark` for rendering and export.
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
├── docs/                      # Markdown documentation
├── xtask/                     # maintenance command runner
└── target/                    # build output
```

### Crate responsibilities

| Crate | Package name | Public API role |
|-------|--------------|-----------------|
| `crates/neo-ai` | `neo-ai` | Provider-neutral `ChatRequest`, `ModelClient`, `AiStreamEvent`, registries, reasoning options, image generation, `FakeModelClient`. |
| `crates/neo-agent-core` | `neo-agent-core` | `AgentRuntime`, `AgentContext`, `ToolRegistry`, built-in tools, `PermissionPolicy`, `FakeHarness`, JSONL session helpers, MCP adapters. |
| `crates/neo-sdk` | `neo-sdk` | JSONL RPC frame types, skill manifest helpers, safe Markdown-to-HTML export. |
| `crates/neo-extensions` | `neo-extensions` | `neo-extension.toml` discovery, installation, lifecycle, stdio JSONL runner. |
| `crates/neo-tui` | `neo-tui` | Reusable terminal UI components, input handling, diff rendering, inline image encoding. |
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
  `crates/neo-ai/src/providers/`.
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
- TUI tests use a simulated terminal shell in `crates/neo-tui/tests/`.
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
   (seeded catalog + inline `[models.*]` TOML + legacy JSON catalogs) and
   `neo_ai::ProviderRegistry` (built-in defaults + config-driven custom
   providers). `ProviderResolver` selects the wire client by the provider's
   declared `type`.
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

### Session storage and workspace scoping

Sessions are stored in a **centralized, workspace-scoped** layout under
`~/.neo/sessions/` (or `$NEO_HOME/sessions/`). Each workspace (project
directory) gets a deterministic bucket directory:

```
~/.neo/sessions/
├── wd_neo_eb208ec56c5c/          ← bucket for /path/to/neo
│   ├── 1718370000000.jsonl       ← session transcript
│   ├── sessions.metadata.json    ← per-bucket metadata index
│   └── ...
├── wd_myproject_a1b2c3d4e5f6/    ← bucket for /path/to/myproject
│   └── ...
└── session_index.jsonl           ← global index (session ID → location)
```

The bucket name is `wd_<slug>_<hash12>` where `<slug>` is derived from the
directory basename and `<hash12>` is the first 12 hex chars of SHA-256 of the
canonicalized absolute path. This ensures:
- `/resume` only shows sessions from the **current workspace**
- Different projects with the same basename get different buckets
- The `NEO_HOME` env var overrides the home directory (`~/.neo` by default)

On startup, `migrate_legacy_sessions()` automatically moves any sessions from
the old `{project_dir}/.neo/sessions/` layout into the new bucket directory.
Migration is idempotent.

The global `session_index.jsonl` enables `neo resume <session_id>` to locate
sessions across workspaces.

### Built-in tools

Registered by `neo_agent_core::ToolRegistry::with_builtin_tools()`:

- `read`, `list`, `grep`, `find`, `glob` — file read/search tools.
- `write`, `edit` — file write tools (`edit` returns a unified diff in details).
- `bash` — shell execution with foreground/background modes and process-group
  cleanup.
- `terminal` — PTY session tool (`start`, `write`, `read`, `resize`, `stop`).
- `todo` — task list management with `pending`/`in_progress`/`done` statuses.
- `enter_plan_mode`, `exit_plan_mode` — read-only planning mode toggle.

The `ask_user` tool is available but not registered by default (requires a
channel sender for reverse-RPC user questions).

### TUI capabilities

The interactive TUI mode (`neo-agent` with no subcommand) provides:

- **Transcript rendering**: differential rendering of assistant text, thinking
  blocks, tool call cards, and images with syntax highlighting.
- **Inline image protocols**: Kitty Graphics, iTerm2 inline images, and Sixel
  encoding for terminal image display.
- **Session picker**: `ctrl+r` or `/resume` opens a local session picker.
- **Model picker**: `/model` opens a model selection dialog.
- **Session fork**: `ctrl+n` in the session picker forks the selected session.
- **Transcript selection copy**: `ctrl+shift+c` copies selected transcript text.
- **Paste buffering**: bracketed-paste handling for multi-line input.
- **Theme support**: customizable color themes via `.neo/themes/*.json` or
  `~/.neo/themes/*.json`.
- **Keybinding customization**: configurable keybindings via config.
- **Approval dialog**: interactive permission approval for tool execution.
- **Question dialog**: multi-question interactive dialogs via `ask_user` tool.

### Image generation

`neo images generate` uses OpenAI-style image endpoints for models that
advertise image-generation capability in the local model catalog:

```bash
neo images generate "a compact terminal workstation" \
  --model openai/gpt-image-1 \
  --output .neo/generated/workstation.png
```

### Prompt templates

Project prompt templates live in `.neo/prompts/*.md` and are invoked by slash
name. User-global templates live in `~/.neo/prompts/*.md`:

```bash
neo prompts list
neo prompts preview review
neo --prompt-template review print src/lib.rs
```

### Trust management

Project context files (`AGENTS.md`, `CLAUDE.md`) are loaded only when the
project is trusted; trust is stored in `~/.neo/trust.json`:

```bash
neo trust status
neo trust approve
neo trust deny
neo trust clear
```

### RPC mode

`neo rpc` accepts JSONL request frames for local session clients:

```bash
neo rpc
```

Supports `get_commands`, `sessions.list`, `sessions.get`, `get_messages`,
`sessions.export_html`, `sessions.export_json`, and `neo.session.export_json`
methods.

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
- `providers.<id>` — full provider definition with `type`, `base_url`,
  `api_key` (inline) or `api_key_env` (environment variable).
  Users can define arbitrary provider ids.
- `models.<alias>` — inline model definitions with `provider`, `model`,
  `max_context_tokens`, `capabilities`, `display_name`.
- `model_catalogs` — legacy JSON model catalog files (still supported,
  loaded in addition to inline `[models.*]` entries).
- `permissions` — `Allow` / `Ask` / `Deny` for `file_read`, `file_write`,
  `shell`, `tool`.
- `runtime` — `temperature`, `max_tokens`, `reasoning_effort`, queue modes,
  tool execution mode, compaction.
- `tui` — `image_protocol`, `fetch_remote_images`, `keybindings`.
- `mcp.servers` — stdio/HTTP/SSE MCP server entries.

Provider types: `openai-responses`, `openai-compatible`, `openai-chat`,
`anthropic`, `google`. The `ProviderResolver` selects the wire-protocol
client based on the provider's declared `type`, not the model's `api` field.

CLI provider/model management: `neo provider add/remove/list`,
`neo provider catalog list/add` (models.dev integration),
`neo models add/remove/list/set`.

TUI slash commands: `/model` (model picker), `/provider` (provider list),
`/resume` (session picker).

CLI session management: `neo sessions list/show/rename/fork/summarize/compact/export-html/export-json`.

CLI extension management: `neo extensions install/update/uninstall/list/status/enable/disable/call`.

CLI MCP management: `neo mcp list`, `neo mcp servers add/remove/enable/disable/health/start/stop`,
`neo mcp tools <server>`, `neo mcp resources <server> list/read/watch`.

CLI image generation: `neo images generate --model <provider/model> --output <path>`.

System prompt resources:

- `.neo/SYSTEM.md` and `~/.neo/SYSTEM.md`.
- `.neo/APPEND_SYSTEM.md` and `~/.neo/APPEND_SYSTEM.md`.

Project context files (`AGENTS.md`, `CLAUDE.md`) are loaded only when the
project is trusted; trust is stored in `~/.neo/trust.json`.

## Security considerations

- **No unsafe code**: workspace lint `unsafe_code = "forbid"`.
- **Secrets**: API keys can be stored inline in config (`api_key = "..."`) or
  referenced via environment variables (`api_key_env = "OPENAI_API_KEY"`).
  `neo config show` redacts `api_key`, MCP `env`, and `headers` values.
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
- The repo uses a generated catalog schema artifact convention. If a generated
  schema exists, `cargo run -p xtask -- catalog check` validates it.
- Example code is in `examples/rust/` as a separate workspace crate.
- Example config and tool schemas are in `examples/config/` and
  `examples/tools/`.

## Git policy — STRICT (READ THIS BEFORE ANY git COMMAND)

**Git 修改操作（mutations）一律禁止，除非用户在当前会话中明确逐次授权。**

### 绝对禁止（无论是否"看起来安全"）

以下命令会**不可逆地丢失未提交的工作**，**永远不得执行**，包括在
subagent / background task / hook 中也不行：

- `git reset`（任何形式：`--hard`、`--soft`、`--mixed`、`HEAD`、`HEAD~N`、`HEAD^`）
- `git checkout -- <file>` / `git checkout HEAD -- <file>` / `git restore <file>`
  （还原工作区文件）
- `git checkout <commit> -- <path>`（把文件回退到历史版本）
- `git stash` / `git stash drop` / `git stash clear`
- `git rebase` / `git rebase -i` / `git rebase --abort`
- `git clean -fd` / `git clean -fdx`（删除未跟踪文件/目录）
- `git rm`（删除已跟踪文件）
- `git gc --prune=now` / `git reflog expire`（清理引用日志）
- `git filter-branch` / `git filter-repo`

### 需要逐次确认（不能假设之前的授权延续）

- `git commit` / `git commit --amend`
- `git push` / `git push --force`
- `git merge` / `git cherry-pick`
- `git branch -d` / `git branch -D`
- `git tag` / `git tag -d`
- `git add`（仅当用户明确要求暂存时）
- `git checkout <branch>` / `git switch <branch>`（切换分支）
- `git worktree add` / `git worktree remove`

### 允许（只读，不需要确认）

`git status`、`git diff`、`git log`、`git show`、`git branch`（不带 -d）、
`git stash list`、`git reflog`、`git blame`、`git ls-files` 等只读命令可自由使用。

### 给 subagent 的规则

**subagent（Agent / AgentSwarm / background bash）不得执行任何 git mutation。**
如果 subagent 认为需要 commit/reset/checkout，它必须返回结果让主 agent
向用户请求授权，而不是自己执行。subagent 的 prompt 里也应包含此约束。

### 违反后果

执行 `git reset`、`git checkout --`、`git stash`、`git clean` 等命令会
**静默丢弃用户未提交的代码**，且可能无法通过 `git fsck` 恢复。
这是最容易发生的灾难性操作，必须零容忍。

## Current workspace health (as of last exploration)

The workspace compiles and all tests in `neo-agent-core` and `neo-agent` pass
as of the workspace-scoped session storage refactor. The `neo-ai` crate has
some pre-existing clippy warnings (missing backticks, collapsible `if`) that
are unrelated to session management.

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

## Git mutation policy — STRICT

**NEVER run any git command that discards, reverts, or rewrites uncommitted
working-tree changes unless the user gives an explicit, message-level
instruction to do so.** This applies to all agents and subagents.

### Banned commands (never run without explicit user approval)

| Command | Why banned |
|---------|------------|
| `git reset --hard` | Discards all uncommitted work |
| `git restore <path>` / `git checkout -- <path>` | Reverts tracked files to HEAD |
| `git checkout .` / `git checkout -- .` | Reverts entire worktree |
| `git stash` / `git stash drop` / `git stash clear` | Hides or destroys uncommitted changes |
| `git clean -fd` / `git clean -fdx` | Deletes untracked files and dirs |
| `git rebase` / `git rebase -i` | Rewrites commit history |
| `git commit --amend` / `git commit --amend --no-edit` | Rewrites the last commit |
| `git push --force` / `git push -f` | Overwrites remote history |

### Allowed git commands

`git status`, `git diff`, `git log`, `git branch`, `git show`, `git add`,
`git stash list` (read-only), `git fsck`, `git reflog` — all fine.

`git commit`, `git push` (non-force), `git merge`, `git checkout -b <new>` are
allowed **only when the user explicitly asks for them** (this restates the
global rule in the system prompt).

### If you need to undo your own edit

Use `Edit` / `Write` to revert the specific lines you changed. Never use a
git command to blow away the file — you will destroy the user's parallel work
in the same file.

### Subagent dispatch

When delegating to subagents, include this rule in the prompt. A subagent
that runs `git restore` or `git stash` can silently destroy hours of the
user's uncommitted work.<!-- git-mutation-ban -->
