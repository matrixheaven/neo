# Neo Agent Workspace — Agent Guide

This file is written for AI coding agents. It assumes you know nothing about the
project. Use it to orient yourself before editing code, running tests, or writing
documentation.

Read [CX.md](../../.kimi-code/CX.md) and [RTK.md](../../.kimi-code/RTK.md). Use the `cx` and `rtk` CLIs to save tokens when they fit the task.

Swarm mode: parallelize substantial work across at least three subagents when the task has independent slices.

This repository is developed by many AI agents at the same time. Stay inside your assigned scope. Do not fix unrelated failures, do not clean up other agents' work, and do not run git commands that discard, revert, hide, or rewrite worktree changes for your own convenience.

Do not preserve the status quo by piling on compatibility branches, fallback paths, or duplicate models. Simplify the model, delete obsolete paths when possible, and avoid code that only appears safe because tests still pass.

## Work loop: recall → scope → verify proportionally

Every task follows this loop, but verification effort scales with task size.
Do not run more tests than the task warrants, and never run tests outside the
current task's scope.

1. Recall project memory before work: `icm recall-context "<task>" --limit 5`.
2. Scope your own work. Do not fix unrelated failures from other agents.
3. Verify proportionally to the task (see tiers below). When you do run tests,
   use `cargo run -p xtask -- test ...`; never use bare `cargo test` as
   completion evidence.

### Verification tiers

Pick the tier that matches the task. Err toward less testing, not more.

- **Trivial / small tasks** (typo, text fix, doc edit, rename, one-line tweak,
  config-only change): **no tests required.** A build check is optional. Do not
  run LCOV, CRAP, or CI for these.
- **Medium tasks** (a single function, a localized bug fix, a small feature in
  one module): run **focused** tests for the touched crate/target only, e.g.
  `cargo run -p xtask -- test -p neo-agent-core runtime_turn`. Do not run the
  whole workspace suite. LCOV/CRAP only if production behavior changed.
- **Complex tasks** (cross-module refactor, architectural change, new
  subsystem, behavior-affecting runtime change): run focused tests for the
  affected crates, then generate LCOV with `cargo run -p xtask -- coverage`
  and score production code with `cargo run -p xtask -- crap`. Use
  `cargo run -p xtask -- ci` only as a final gate for large changes.

Never widen the test scope to "make sure nothing broke" — that is CI's job, not
the task loop's. If a change is outside your scope, its tests are too.

CRAP policy is strict: production code under `crates/` must have no function
with CRAP > 30 when scored from `target/llvm-cov/lcov.info`. Do not use
allowlists, fallback branches, or threshold increases to hide high scores. If
CRAP is high, simplify the function, delete obsolete paths, or add real behavior
tests and rerun LCOV + CRAP.

Report artifacts:

- LCOV: `target/llvm-cov/lcov.info`
- Production CRAP Markdown: `target/crap/crap-crates.md`
- Production CRAP JSON: `target/crap/crap-crates.json`
- Workspace CRAP observation report: `target/crap/crap-workspace.md`

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
- **TUI**: `crossterm`-based terminal UI with single-buffer rendering primitives,
  inline image protocols (Kitty, iTerm2, Sixel), bracketed-paste handling, and
  a component-tree architecture.
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
│   ├── agent-core             # neo-agent-core: runtime, tools, permissions, sessions, MCP, extensions, skills, RPC, export
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
| `crates/neo-agent-core` | `neo-agent-core` | `AgentRuntime`, `AgentContext`, `ToolRegistry`, built-in tools, `PermissionMode`, `FakeHarness`, JSONL session helpers, MCP adapters, local extension adapters, skill loading, JSONL RPC primitives, HTML export. |
| `crates/neo-tui` | `neo-tui` | Reusable terminal UI components, input handling, diff rendering, inline image encoding. |
| `crates/neo-agent` | `neo-agent` | The `neo` binary. Parses args, loads config, dispatches to `print`/`run`/`resume`/sessions/extensions/MCP/RPC/TUI modes. |
| `xtask` | `xtask` | Maintenance commands: check, test, coverage, crap, ci, parity, release-smoke, catalog check. |

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

# Full workspace fmt/clippy/nextest gate.
cargo run -p xtask -- check --workspace

# Run repository tests through cargo-nextest.
cargo run -p xtask -- test
cargo run -p xtask -- test -p neo-agent-core runtime_turn
cargo run -p xtask -- test --workspace --all-features
cargo run -p xtask -- test --no-run --workspace --all-features
cargo run -p xtask -- test --list --workspace --all-features

# Generate LCOV, run the production CRAP gate, or run the full local CI gate.
cargo run -p xtask -- coverage
cargo run -p xtask -- crap
cargo run -p xtask -- ci

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
cargo run -p xtask -- test --workspace --all-features

# Individual crate or target
cargo run -p xtask -- test -p neo-ai
cargo run -p xtask -- test -p neo-agent-core --lib todo
cargo run -p xtask -- test -p neo-agent-core runtime_turn
cargo run -p xtask -- test -p neo-tui --test tool_cards
cargo run -p xtask -- test -p neo-agent interactive
cargo run -p xtask -- test -p xtask
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
- **When you run tests, use `xtask` as the entrypoint.** Do not claim
  verification from direct `cargo test` commands. Use
  `cargo run -p xtask -- test ...`, which runs `cargo nextest` with the
  repository configuration.
- **Run tests only at the scope the task needs** (see "Verification tiers"
  above). Small tasks need no tests; medium tasks need focused crate/target
  tests; only complex refactors warrant LCOV/CRAP/CI. Do not run the workspace
  suite or `ci` for routine changes.
- `cargo nextest` is the test runner for unit and integration tests. If
  `cargo nextest` is not installed, fail closed and install it; do not silently
  fall back to `cargo test`.
- Local focused tests should be run through `xtask test`, for example
  `cargo run -p xtask -- test -p neo-agent-core runtime_turn` or
  `cargo run -p xtask -- test -p neo-tui --test tool_cards`.
- Use `cargo run -p xtask -- test --no-run --workspace --all-features` only
  when you genuinely need a workspace compile-only check, and
  `cargo run -p xtask -- test --list` for test inventory.
- Use `cargo run -p xtask -- coverage` to generate
  `target/llvm-cov/lcov.info` with `cargo-llvm-cov` and `cargo-nextest` — only
  for complex tasks that changed production behavior.
- Use `cargo run -p xtask -- crap` to generate
  `target/crap/crap-crates.md` and `target/crap/crap-crates.json`, then fail if
  production code in `crates/` has CRAP > 30 — again, only for complex tasks
  touching production code.
- Use `cargo run -p xtask -- crap --workspace` only as an observation report for
  xtask/examples/tests; it is not the first-stage hard gate.
- Use `cargo run -p xtask -- ci` for the full local CI workflow: workspace
  check, LCOV, production CRAP gate, parity, and catalog check — reserve this
  for large/complex changes, not every task.
- New slow, PTY, MCP, provider-wire, real-process, fixed-port, shared-home, or
  resource-sensitive tests must be classified in `.config/nextest.toml` instead
  of reducing global parallelism or relying on accidental execution order.
- Do not add tests that depend on shared current directory, ambient environment,
  fixed `NEO_HOME`, fixed network ports, or another test's side effects.
- Doctests are not part of the normal `nextest` path. Add or run doctests only
  through an explicit xtask entrypoint if one exists; do not reintroduce direct
  `cargo test` as the routine validation path.
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
   (seeded catalog + inline `[models.*]` TOML) and
   `neo_ai::ProviderRegistry` (built-in defaults + config-driven custom
   providers). `ProviderResolver` selects the wire client by the provider's
   declared `type`.
4. `AgentRuntime` sends a `neo_ai::ChatRequest` to the selected `ModelClient`.
5. Provider-native streams are normalized into `AiStreamEvent` values
   (`MessageStart`, `ThinkingStart/Delta/End`, `TextDelta`, `ToolCallStart`,
   `ToolCallArgsDelta`, `ToolCallEnd`, `MessageEnd`, `Error`).
6. Tool calls are authorized against the active `PermissionMode`, executed by
   the `ToolRegistry`, and returned as `ChatMessage::ToolResult`.
7. Reasoning events are preserved as `ContentPart::Thinking` blocks, not mixed
   into plain assistant text.
8. Skills are loaded from project, user, extra, and built-in tiers; an
   `<available_skills>` block is injected into the system prompt, and the
   internal `Skill` tool is offered to the model. When activated, the skill body
   is injected as a context message before the user's message; nested skill
   invocations within a single turn are rejected.
9. Goals, when active, continue autonomously across turns using
   `get_goal_status` / `update_goal_status` until complete, blocked, or paused.
   There is no turn budget.
10. Session events are appended to local JSONL so `resume` can reconstruct
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

When a `GoalManager` is attached, Neo also registers `StartGoal`,
`ExitGoalMode`, `UpdateGoalStatus`, and `GetGoalStatus`.

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
- **Development modes**: Shift+Tab cycles normal → plan → goal → normal.
  Development modes are independent from permission modes; Shift+Enter inserts
  a newline, and permissions are changed through `/permissions`, `/ask`,
  `/auto`, or `/yolo`.
- **Approval dialog**: interactive permission approval for tool execution.
- **Question dialog**: multi-question interactive dialogs via `ask_user` tool.

### Blocking dialog contract

Neo treats focused overlays as blocking dialogs when they require direct user
choice or text entry outside the main composer. This includes `/resume`,
`/model`, `/provider`, session/model/provider pickers, API key or registry input,
approval prompts, non-background `AskUserQuestion`, plan review dialogs such as
`ExitPlanMode`, and goal review dialogs such as `ExitGoalMode`.

When a blocking dialog is focused:

- Hide the main prompt/composer completely (`prompt_height = 0`).
- Route all insert, paste, delete, backspace, arrow, enter, and escape input to
  the dialog before `PromptState`.
- Keep any required free-form text inside the dialog itself, for example
  `Other` text in Ask User or `Reject with feedback` in Approval.
- Do not reintroduce paths where Approval or Ask User borrow the bottom
  composer for text input.
- Add or update regression tests that prove the composer is hidden and typed
  input does not leak into `PromptState` while the dialog is focused.

Tool execution must respect the same contract. If a model response contains any
tool call that can open a blocking dialog, the runtime must execute that tool
batch sequentially in source order, even when `ToolExecutionMode::Parallel` is
configured. This prevents a later Bash approval, Ask User question, or other
blocking prompt from being displayed while an earlier dialog is still waiting.
`AskUserQuestion` with `background = true` is the exception: it may return
immediately and should not force the batch into blocking-dialog serialization.

### Development modes

Neo keeps permission modes and development modes separate. Permission modes are
only `[manual]`, `[auto]`, and `[yolo]`; they control approval policy for risky
tools. Development modes are mutually exclusive: `normal`, `plan`, and `goal`.

- Shift+Tab cycles development mode only: normal → plan → goal → normal.
- Do not bind Shift+Tab or Shift+Enter to permission cycling.
- Shift+Enter, Alt+Enter, and Ctrl+J all insert a newline.
- Footer badges render permission first, then development state, for example
  `[manual] [plan] ...`, `[manual] [goal] ...`, `[manual] [goal•] ...`,
  `[manual] [goal◌] ...`, or `[manual] [goal✗] ...`.
- Plan mode must synchronize with runtime `PlanMode`; it is not just a footer
  flag. The model writes the workspace-scoped plan file and exits through
  `ExitPlanMode`, which opens a blocking review dialog for Accept / Reject /
  Revise when reviewable.
- Goal mode is the AI-assisted goal authoring workflow. The model drafts a
  structured goal, exits through `ExitGoalMode`, and the review dialog decides
  whether a durable goal starts. `/goal <objective>` remains the direct
  user-authored goal path and must not require an AI draft.

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

### Skills

Skills are reusable prompt fragments with YAML frontmatter. They are discovered
from four tiers: project `.neo/skills/**/SKILL.md`, user `~/.neo/skills/**/SKILL.md`,
`extra_skill_dirs` in config, and built-in skills shipped with Neo. When a skill
is activated manually via `/skill:<name>` or automatically by the `Skill` tool,
its expanded body is injected into the conversation as a context message before
the user's original message.

Key manifest fields: `name`, `description`, `type` (`prompt`/`inline`/`flow`),
`whenToUse`, `disableModelInvocation`, `arguments`, `slashCommands`. Placeholders
`$<name>`, `$0`, `$ARGUMENTS`, and `${NEO_SKILL_DIR}` are expanded at invocation
time.

Built-in skills: `sub-skill`, `self-evo`. They are extracted into
`~/.neo/skills/.builtin/` on startup so users can inspect and override them.
Removed built-ins such as `define-goal` are pruned/ignored so stale extracted
copies do not keep loading.

See `docs/skills.md` for the full skill specification.

### Goals

Goals let Neo work autonomously across turns. The TUI supports direct
user-authored goals with `/goal <objective>`, plus `/goal pause`, `/goal
resume`, `/goal cancel`, `/goal replace <objective>`, and `/goal next
<objective>`. Goal mode is separate: it drafts a structured goal and submits it
through `ExitGoalMode` for blocking review before creating the durable goal.
Active goals are stored in `~/.neo/goals/`; structured runs also write
`~/.neo/goals/runs/<goal-id>/GOAL.md`, `ROADMAP.md`, `STATE.md`,
`THINKING.md`, `PROTOCOL.md`, and `phases/phase-N.md`. The runtime continues
turns automatically while the goal is active; the model uses
`update_goal_status` to mark completion or blockers. There is no turn cap.

See `docs/goals.md` for details.

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
- MCP resources are not silently injected into model context.

## Configuration model

Config precedence:

1. CLI flags.
2. Project `.neo/config.toml` (or path from `--config` / `NEO_CONFIG`).
3. User-global `~/.neo/config.toml`.
4. Built-in defaults (`openai/gpt-4.1`).

Project config merges over user-global config field by field. Provider maps are
merged by provider id; MCP servers are merged by server id. Important sections:

- `default_provider`, `default_model`, `api_key_env`.
- `providers.<id>` — full provider definition with `type`, `base_url`,
  `api_key` (inline) or `api_key_env` (environment variable).
  Users can define arbitrary provider ids.
- `models.<alias>` — inline model definitions with `provider`, `model`,
  `max_context_tokens`, `capabilities`, `display_name`.
- `permission_mode` — `manual` / `auto` / `yolo`. Controls how risky tool
  actions are approved; defaults to `manual`.
- `runtime` — `temperature`, `max_tokens`, `reasoning_effort`, queue modes,
  tool execution mode, compaction, `extra_skill_dirs`.
- `tui` — `image_protocol`, `fetch_remote_images`, `keybindings`.
- `mcp.servers` — stdio/HTTP/SSE MCP server entries.

Provider types: `openai-responses`, `openai-compatible`, `openai-chat`,
`anthropic`, `google`. The `ProviderResolver` selects the wire-protocol
client based on the provider's declared `type`, not the model's `api` field.

CLI provider/model management: `neo provider add/remove/list`,
`neo provider catalog list/add` (models.dev integration),
`neo models add/remove/list/set`.

TUI slash commands: `/ask` (manual permission mode), `/auto` (auto permission
mode), `/yolo` (yolo permission mode), `/permissions` (permission mode
selector), `/plan` (toggle plan mode), `/model` (model picker), `/provider`
(provider list), `/resume` (session picker), `/skill:<name>` (activate a skill
and expand it into the prompt), `/goal` (start, pause, resume, cancel, or
replace a goal). Shift+Tab cycles the separate development mode; it must not
change the permission mode.

CLI session management: `neo sessions list/show/rename/fork/summarize/compact/export-html/export-json`.

CLI extension management: `neo extensions install/update/uninstall/list/status/enable/disable/call`.

CLI MCP management: `neo mcp list`,
`neo mcp add <name> -t studio|remote-http|remote-sse ...`,
`neo mcp del <name>`, `neo mcp enable/disable <name>`.

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
- **Shell tool**: follows the active `permission_mode`. In `manual` mode it
  asks for approval; in `auto` and `yolo` it runs after hard safety policies.
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

## Persistent memory (ICM) — MANDATORY

This project uses [ICM](https://github.com/rtk-ai/icm) for persistent memory across sessions. You MUST use it actively.

### Recall before starting work

```bash
icm recall "query"
icm recall "query" -t "topic-name"
icm recall-context "query" --limit 5
```

### Store when required

Call `icm store` BEFORE responding when any of these happens:

1. Error resolved: `icm store -t errors-resolved -c "description" -i high -k "keyword1,keyword2"`
2. Architecture or design decision: `icm store -t decisions-neo -c "description" -i high`
3. User preference discovered: `icm store -t preferences -c "description" -i critical`
4. Significant task completed: `icm store -t context-neo -c "summary of work done" -i high`
5. More than about 20 tool calls since the last store: save a progress summary.

Do not store trivial details, existing AGENTS.md facts, ephemeral build logs, or transient git status.

Other useful commands:

```bash
icm update <id> -c "updated content"
icm health
icm topics
```

## Git mutation policy — STRICT

Git mutations are forbidden unless the user gives explicit, message-level authorization for that exact operation. This applies to the main agent, subagents, background tasks, hooks, and scripts.

### Never run these without explicit approval

These commands can lose, hide, or rewrite work and are not allowed as convenience operations:

- `git reset` in any form.
- `git checkout -- <path>`, `git checkout HEAD -- <path>`, `git restore <path>`, `git checkout .`, or any equivalent worktree revert.
- `git checkout <commit> -- <path>`.
- `git stash`, `git stash drop`, or `git stash clear`.
- `git rebase`, `git rebase -i`, or `git rebase --abort`.
- `git clean -fd` or `git clean -fdx`.
- `git rm`.
- `git gc --prune=now`, `git reflog expire`, `git filter-branch`, or `git filter-repo`.
- `git commit --amend` or any force push.

### Require per-command user authorization

- `git add`
- `git commit`
- `git push`
- `git merge` or `git cherry-pick`
- `git checkout <branch>` or `git switch <branch>`
- `git checkout -b <branch>` or `git switch -c <branch>`
- `git branch -d` / `git branch -D`
- `git tag` / `git tag -d`
- `git worktree add` / `git worktree remove`

### Allowed read-only git commands

`git status`, `git diff`, `git log`, `git show`, `git branch` without deletion, `git stash list`, `git reflog`, `git blame`, `git ls-files`, and `git fsck` are allowed.

### If you need to undo your own edit

Use targeted file edits to undo only the lines you changed. Never use a git command to blow away a file, because that can silently destroy another agent's work in the same file.

### Subagent rule

Every subagent prompt must include the git mutation ban. If a subagent thinks a mutation is needed, it must report that need to the main agent instead of running the command.
<!-- git-mutation-ban -->
