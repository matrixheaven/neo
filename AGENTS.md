# Neo — Agent Guide

Neo is a Rust-native, local-only AI coding agent (CLI + TUI). Cargo workspace, edition 2024, min Rust 1.88. Vendored dirs (`claude-code`, `codex`, `kimi-code`, `opencode`, `pi`) under `docs/` are reference-only, not part of the workspace.

Read [CX.md](../../.kimi-code/CX.md) and [RTK.md](../../.kimi-code/RTK.md). Use `cx`/`rtk` CLIs to save tokens. Parallelize substantial work across ≥3 subagents when slices are independent.

<important>

**NEVER restore any file to `HEAD` or any prior revision. NEVER use `git` mutations. Use `git` only for `diff`/`log`/`status` (read-only or commit or push).**

</important>

## Critical rules

1. **Stay in scope.** Don't fix unrelated failures or clean up other agents' work. The worktree is shared and concurrent.
2. **Never revert worktree files** to make tests pass. If another agent's in-progress work breaks your build, skip those tests and report it.
3. **Simplify, don't pile on.** Delete obsolete paths. No compatibility branches, fallback paths, or duplicate models to preserve status quo.
4. **No hosted services.** Local binary only. No marketplace, profile sync, or hosted collaboration.

## Work loop: recall → scope → verify

1. Recall: `icm recall-context "<task>" --limit 5`.
2. Scope your own work only.
3. Verify proportionally (tiers below). Use `cargo run -p xtask -- test ...`; never use bare `cargo test` as evidence.

### Verification tiers — err toward less testing

| Tier | When | What to run |
|------|------|-------------|
| **Trivial** | typo, doc edit, rename, config-only | No tests. Build check optional. |
| **Medium** | single function, localized fix, small feature | Focused tests for touched crate only: `cargo run -p xtask -- test -p <crate> <filter>` |
| **Complex** | cross-module refactor, arch change, new subsystem | Focused tests → `cargo run -p xtask -- coverage` → `cargo run -p xtask -- crap`. `ci` only as final gate. |

Never widen scope to "make sure nothing broke" — that's CI's job.

**CRAP policy:** production code under `crates/` must have no function with CRAP > 30 (scored from `target/llvm-cov/lcov.info`). No allowlists or threshold hikes. Simplify, delete obsolete paths, or add real behavior tests.

Artifacts: `target/llvm-cov/lcov.info`, `target/crap/crap-crates.md`, `target/crap/crap-crates.json`.

## Crates

| Crate | Role |
|-------|------|
| `neo-ai` | Provider-neutral `ChatRequest`, `ModelClient`, `AiStreamEvent`, registries, `FakeModelClient`. |
| `neo-agent-core` | `AgentRuntime`, `ToolRegistry`, built-in tools, `PermissionMode`, sessions, MCP/extension adapters, skills, RPC, export. |
| `neo-tui` | Terminal UI components, input, diff rendering, inline image encoding. |
| `neo-agent` | The `neo` binary: CLI parsing, config, dispatch to print/run/resume/TUI modes. |
| `xtask` | Maintenance: check, test, coverage, crap, ci, parity, release-smoke. |

## Build & test commands

```bash
cargo build -p neo-agent                    # build binary
cargo run -p xtask -- check                 # default gate (xtask only)
cargo run -p xtask -- check --workspace     # full fmt/clippy/nextest
cargo run -p xtask -- test -p <crate> [filter]  # focused tests
cargo run -p xtask -- test --workspace --all-features  # all tests
cargo run -p xtask -- coverage              # LCOV
cargo run -p xtask -- crap                  # CRAP gate
cargo run -p xtask -- ci                    # full local CI
cargo run -p xtask -- parity                # docs/examples parity
cargo run -p xtask -- release-smoke         # local release check
```

Test runner is `cargo-nextest` (install if missing; never fall back to `cargo test`). Deterministic model tests: `FakeModelClient` / `FakeHarness`. Resource-sensitive tests must be classified in `.config/nextest.toml`. Tests must not depend on shared cwd, ambient env, fixed ports, or other tests' side effects. Fixture lines in docs/examples: prefix `# xtask-parity: allow <reason>`.

## Code style

- `unsafe_code = "forbid"`; `clippy::pedantic` warned; `missing_errors_doc`, `missing_panics_doc`, `module_name_repetitions` allowed.
- Typed Rust interfaces first; wire protocols (MCP, JSON-RPC, JSONL) at crate boundaries.
- Provider code in `crates/neo-ai/src/providers/`. Tool schemas small and stable.
- Session events are normalized `AgentEvent` values — JSONL must not depend on provider wire formats.

## Runtime architecture (quick reference)

1. Config: CLI → env → `~/.neo/config.toml` (`$NEO_HOME`) → defaults. No project-local config.
2. Sessions: JSONL under `~/.neo/sessions/wd_<slug>_<hash12>/` (workspace-scoped buckets). Global `session_index.jsonl` for cross-workspace resume.
3. Model resolution: `ModelRegistry` (catalog + inline TOML) → `ProviderRegistry` → `ProviderResolver` selects wire client by provider `type`.
4. Streams normalized to `AiStreamEvent` (`TextDelta`, `Thinking*`, `ToolCall*`, `MessageEnd`, `Error`). Reasoning preserved as `ContentPart::Thinking`.
5. Tools authorized against `PermissionMode`, executed by `ToolRegistry`.
6. Skills: project/user/extra/built-in tiers; `<available_skills>` injected into system prompt; activation injects skill body before user message.
7. Goals: autonomous across turns via `update_goal_status`; no turn cap. Stored under `<session_dir>/goals/`.
8. Queue & steer: `Enter` while busy → follow-up (FIFO). `Ctrl+S` → steer at next break point. See `docs/queue-and-steer.md`.

### Built-in tools

`read`, `list`, `grep`, `find`, `glob`, `write`, `edit`, `bash`, `terminal` (PTY), `todo`, `enter_plan_mode`, `exit_plan_mode`. With `GoalManager`: `StartGoal`, `ExitGoalMode`, `UpdateGoalStatus`, `GetGoalStatus`. `ask_user` available but not registered by default.

### Extension & MCP namespacing

- Extensions: `extension__<id>__<tool>` (JSONL RPC).
- MCP: `mcp__<server>__<tool>` via `McpStdioToolAdapter` / `McpHttpToolAdapter`. Resources are runtime state, not model context.

### Key TUI/UX contracts

- **Permission modes**: `ask`, `auto`, `yolo` — control tool approval policy.
- **Development modes**: `normal`, `plan`, `goal` — mutually exclusive. Shift+Tab cycles. Independent from permission modes.
- **Blocking dialogs** (`/resume`, `/model`, `/provider`, approval, `AskUserQuestion`, `ExitPlanMode`, `ExitGoalMode`): hide composer (`prompt_height = 0`), route all input to dialog. Tool batches with any blocking-dialog tool must execute sequentially even in parallel mode (exception: `AskUserQuestion` with `background = true`).
- Slash commands: `/ask`, `/auto`, `/yolo`, `/permissions`, `/plan`, `/model`, `/provider`, `/resume`, `/skill:<name>`, `/goal`.

### Provider types

`openai-responses`, `openai-compatible`, `openai-chat`, `anthropic`, `google`. Wire client selected by provider `type`, not model `api`.

### Config sections

`providers.<id>`, `models.<alias>`, `permission_mode`, `runtime` (temp, max_tokens, reasoning_effort, queue/execution modes, compaction, extra_skill_dirs), `tui` (image_protocol, fetch_remote_images, keybindings), `mcp.servers`. System prompt: `~/.neo/SYSTEM.md`, `~/.neo/APPEND_SYSTEM.md`. Trust: `~/.neo/trust.json` gates `AGENTS.md`/`CLAUDE.md` loading.

## Security

No unsafe code. API keys inline (`api_key`) or env-ref (`api_key_env`); `neo config show` redacts secrets. Write/execute tools workspace-contained; `Read` allows absolute paths outside workspace. Remote image fetch disabled by default. Disabled MCP servers not started. Local-only surface.

## Persistent memory (ICM) — MANDATORY

```bash
icm recall-context "<task>" --limit 5    # before work
icm store -t <topic> -c "<desc>" -i high  # after resolving errors, making decisions, discovering preferences, completing significant work, or every ~20 tool calls
```

Never store trivial details, existing AGENTS.md facts, or transient logs.

## Git mutation policy — STRICT

**No git mutations** unless the user explicitly authorizes that exact command. The safety boundary is the worktree.

**Forbidden** (discard/rewrite worktree): `git reset --hard/--merge/--keep`, `git checkout/restore -- <path>`, `git stash`, `git rebase`, `git clean -fd`, `git rm`, `git commit --amend`, force push, `git filter-branch/repo`, `git gc --prune`, `git reflog expire`.

**Per-command authorization required**: `git add`, `commit`, `push`, `merge`, `cherry-pick`, `checkout/switch <branch>`, `branch -d/-D`, `tag`, `worktree add/remove`.

**Read-only allowed**: `status`, `diff`, `log`, `show`, `branch` (no delete), `stash list`, `reflog`, `blame`, `ls-files`, `fsck`.

Blocked work is never a reason to revert files. Undo your own edits with targeted file edits, never `git checkout`. Subagent prompts must include this ban.
