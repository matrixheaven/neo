# Neo — Agent Guide

Neo is a Rust-native, local-only AI coding agent (CLI + TUI). Cargo workspace, edition 2024, min Rust 1.96.1. Vendored dirs (`claude-code`, `codex`, `kimi-code`, `opencode`, `pi`) under `.references/` are reference-only, not part of the workspace.

Read [CX.md](./CX.md) and [RTK.md](./RTK.md). Use `cx`/`rtk` CLIs to save tokens. Parallelize substantial work across ≥3 subagents when slices are independent.

## Critical rules

> **This guide constrains _you_ (the AI collaborator), not Neo User.** Nothing here is a product specification or feature requirement. Rules describe working conventions for agents operating in this codebase — do not mistake them for Neo's design, architecture decisions, or user-facing behavior. If a rule says "don't do X", that limits what _you_ do while coding, not what Neo as a tool must support.

1. **Stay in scope.** Don't fix unrelated failures or clean up other agents' work. The worktree is shared and concurrent.
2. **Never revert worktree files** to make tests pass. If another agent's in-progress work breaks your build, skip those tests and report it.
3. **Simplify, don't pile on.** Delete obsolete paths. No compatibility branches, fallback paths, or duplicate models to preserve status quo.
4. **No hosted services.** Local binary only. No marketplace, profile sync, or hosted collaboration.
5. **Tests must earn their place.** No redundant tests that duplicate another test's coverage with only cosmetic differences (e.g., a different output flag). No tests asserting trivially true properties (struct field round-trips, derived trait behavior, library correctness). When writing or reviewing tests, apply the same "simplify, don't pile on" principle — a test that catches nothing you wouldn't catch by deleting it is dead weight.
6. **Cross-platform is non-negotiable.** Every feature must work on Windows, Linux, and macOS. No hardcoded path separators (use `Path`/`PathBuf`), shell invocations (no bare `sh -c`), Unix signals, or file-permission assumptions without `#[cfg]` guards and cross-platform fallbacks. Platform-specific code must be isolated behind `cfg(unix)` / `cfg(windows)` with a portable default — never `panic!` or `todo!` on unsupported platforms.

## Context integrity

Context preservation is a hard invariant for every Neo feature and code change:

1. **Never modify the context cache prefix.** Existing prefix bytes, message ordering, and request-visible prefix content must remain stable. Do not introduce rewriting, normalization, truncation, snipping, deduplication, summarization, or other transformations that change an existing prefix.
2. **Never rewrite system prompts or historical conversation.** Canonical system instructions, user messages, assistant messages, tool calls, tool results, reasoning, and session events are append-only records. Corrections, updates, and new instructions must be represented by a new event appended after the existing records.
3. **Derived views must not replace the source.** Compaction, token-budget views, redaction, export, replay, and provider-specific projections may create a separate derived representation only when the canonical records and their order remain intact; they must never mutate, delete, reorder, or silently omit existing canonical content.
4. **Verify prefix stability when touching context code.** Tests or other focused evidence must prove that an unchanged session keeps the same cache prefix and that every new piece of context is appended. A change that cannot preserve this invariant must be rejected or redesigned before implementation.

## Work loop: recall → scope → verify

1. Recall: `icm recall-context "<task>" --limit 5`.
2. Scope your own work only.
3. Verify proportionally (tiers below). Use the narrowest exact command that proves the touched behavior; never use broad `cargo test` as evidence.
4. Commit: after verification passes, commit the changes with a conventional commit message (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:` prefix). One logical task = one commit. Don't batch unrelated changes.

### Verification tiers — err toward less testing

| Tier | When | What to run |
|------|------|-------------|
| **Trivial** | typo, doc edit, rename, config-only | No tests. Build check optional. |
| **Medium** | single function, localized fix, small feature | One exact function-level test when possible; otherwise one explicit target with a narrow filter. |
| **Complex** | cross-module refactor, arch change, new subsystem | Start with the smallest explicit targets for each touched boundary. Add more explicit targets only when evidence points there. |

Never widen scope to "make sure nothing broke" — that's CI's job. Test evidence must name exactly one package, exactly one target selector (`--lib`, `--bin <bin>`, or `--test <target>`), and at least one test-name filter.

## Crates

| Crate | Role |
|-------|------|
| `neo-ai` | Provider-neutral `ChatRequest`, `ModelClient`, `AiStreamEvent`, registries, `FakeModelClient`. |
| `neo-agent-core` | `AgentRuntime`, `ToolRegistry`, built-in tools, `PermissionMode`, sessions, MCP adapters, skills, RPC, export. |
| `neo-tui` | Terminal UI components, input, diff rendering, inline image encoding. |
| `neo-agent` | The `neo` binary: CLI parsing, config, dispatch to print/run/resume/TUI modes. |
## Build & test commands

```bash
cargo build -p neo-agent                    # build binary
cargo fmt --all --check                     # formatting
cargo clippy -p <crate> --lib -- -D clippy::all           # library lint
cargo clippy -p <crate> --test <target> -- -D clippy::all # integration-test lint
cargo nextest run -p <crate> --test <target> <filter>  # integration test
cargo nextest run -p <crate> --lib <filter>            # library unit test
cargo nextest run -p <crate> --bin <bin> <filter>      # binary target test
cargo test --package <crate> --bin <bin> -- <full::test::path> --exact --nocapture --include-ignored # exact binary test
```

Prefer `cargo-nextest` for normal verification. For fast local iteration on a known single test function, exact `cargo test` is acceptable when it names the package, target, full test path, and `--exact`, for example:

```bash
cargo test --package neo-agent --bin neo -- modes::task_browser::tests::task_browser_adapter_shows_waiting_question_prompt --exact --nocapture --include-ignored
```

Do not use broad `cargo test`, package-wide `cargo nextest run`, or vague substring filters as evidence. Deterministic model tests: `FakeModelClient` / `FakeHarness`. Resource-sensitive tests must be classified in `.config/nextest.toml`. Tests must not depend on shared cwd, ambient env, fixed ports, or other tests' side effects.

## Test suite governance

The four crates share one set of test rules. Canonical top-level targets live in §5.8 of `docs/aegis/specs/2026-08-07-test-suite-governance-design.md`. The completed over-limit migration baseline in §5.9 is historical evidence, not pending work.

### Structure

- **Unit tests** verify private pure logic, parsing, or local state transitions only. Small tests stay inline as `#[cfg(test)] mod tests`; an inline block targets ≤300 lines, hard cap 600 lines or 12 tests — exceeding either cap means split out by behavior.
- Extracted unit-test files use explicit names (`permission_mode_tests.rs`). When private test files must group, the production module declares them with explicit `#[path = "test_cases/<behavior>.rs"]`; never create test-only `mod.rs` or `tests.rs` aggregates.
- **Crate behavior tests** go in `crates/<crate>/tests/` only when they check cross-module behavior through public interfaces. Each top-level target is one domain (`provider_stream_behavior.rs`); the top-level file declares only same-name behavior submodules, with test bodies in `tests/<domain>/<behavior>.rs`. This avoids giant files and avoids each small file becoming its own test binary.
- Shared fixtures live in purpose-named files (`http_server.rs`, `isolated_home.rs`) inside the domain directory, pulled in via explicit `#[path]`.
- **Forbidden file names**: test-only `mod.rs`, `tests.rs`, `test.rs`, `misc.rs`, `common.rs`, `integration.rs`, and numeric shards (`part1.rs`, `_1.rs`).
- Test body files target 300–800 lines, hard cap 1200 lines or 30 tests; top-level domain entries hold only module declarations and minimal domain fixtures, target ≤100 lines. A top-level file with 1–2 tests merges into its domain unless platform- or resource-specific. Line counts trigger split review only — never deletion justification.
- Test names are condition-plus-observable-result (`closed_input_routes_enter_to_next_turn`): no `test_` prefix, no ticket numbers, no version suffixes. Platform files end `_windows`/`_unix`/`_macos` with real conditional compilation; resource-pressure files end `_resource` and cannot use the suffix to escape normal CI.

### What earns a new test

Answer all three before adding one:

1. What observable behavior or fault does it guard?
2. What is the cheapest layer that captures that fault?
3. Does an existing primary guard already fail on the same fault?

No forms, tags, coverage gates, layout checkers, or long-term ledgers. New features cover committed behavior and critical failure branches only. Names, assertions, and fixtures must make failure condition and result readable in 30 seconds; test data is the minimum that triggers the behavior; resource tests state where their thresholds come from. Any real wait, global-env mutation, process spawn, or file sync must justify why a cheaper deterministic substitute cannot work.

### Test classes

Classification language only — tests still run under Cargo and Nextest.

| Class | Guards | Default location | Prohibited |
|---|---|---|---|
| Unit | private pure logic, parsing, local state transitions | inline or explicit source-side test file | process spawn, network, cross-crate |
| Crate behavior | public interfaces, cross-module wiring | domain entry in `crates/<crate>/tests/` | duplicating unit parameter matrices |
| Product boundary | CLI, RPC, terminal, persistence, real processes | final entry crate only | repeating all lower-layer cases |
| Platform | Windows/Linux/macOS differences | explicit platform files, conditional compilation | non-native results for native evidence |
| Resource | volume, output, concurrency, reclamation boundaries | explicit `_resource` domain | arbitrary large data as boundary proof |

### Lifecycle

- Fix a defect by extending the existing primary guard first; add a minimal regression only when nothing covers the fault.
- Flaky tests are defects: fix the determinism. No retries, no permanent quarantine.
- Retiring production behavior retires its tests in the same batch; replacing behavior migrates the primary guard and deletes the old one.

### Value judgment

- **Keep** when it uniquely guards user-visible behavior, public API, or cross-module integration; guards append-only context, cache prefix, persistence, permissions, security, data-loss, or recovery semantics; guards a real historical defect whose fault class can recur; guards platform differences, error branches, resource boundaries, protocol ordering, or concurrency final states; or no cheaper stronger test captures the same fault.
- **Merge** into one table-driven test with named cases when tests vary only input values over the same branch and assertions, or repeat full fixtures for one mapping table. Each case must be named so a failure is directly locatable; never chain sequentially dependent scenarios into one test.
- **Delete** through one of two evidence paths. For a duplicate guard: it has no independent user behavior, risk boundary, or historical defect; a retained test fails on the same production fault; and the precise target proves the retained semantics with a non-zero run count. For a non-guard: its assertions cover only derived capabilities, stdlib behavior, test-helper interfaces, non-empty text, or duplicated snapshot details; record why no meaningful production fault exists, then run the precise target after deletion with a non-zero run count. Do not add a replacement test solely to authorize deletion. High-risk duplicates get one temporary fault injection (reverted before commit) when call paths cannot prove the overlap. Every deletion records the exact single-package/single-target command and evidence path used.
- **Rewrite** when behavior matters but the test depends on fixed waits, real network, fixed ports, shared cwd, global env, or incomplete process reclamation — use paused time, readiness signals, `127.0.0.1:0`, `tempfile`, and existing fake models instead.

### Layering dedup

Per behavior keep at most: one cheapest unit parameter matrix (local branches), one crate-behavior chain (module wiring), and one `neo-agent` end-to-end chain only when cross-process or final-entry risk is real. Upper layers verify only added risk — wiring, serialization, process boundaries, terminal state, persistence — never the lower layer's full case set.

### Local performance

- Measure three segments separately on this machine, never substitute remote numbers: cold build + test discovery; hot deterministic execution after compile; the serial resource group's standalone time.
- Baseline uses the default Nextest config with identical commands (only full status output enabled). `--profile ci` has a different slow-test threshold and is remote-CI-only, never a local baseline.
- `retries = 0` stays. Do not fake performance with `#[ignore]`, nightly escapes, or relaxed timeouts.
- Resource tests still run in full CI, split only into attributable standalone steps.
- Process tests carry readiness deadlines, operation deadlines, and reclamation assertions — state signals, not fixed waits.
- Nextest grouping is precise to tests that actually share resources; never serialize a whole test binary.
- A new test entering the 20-second slow range must show the resource boundary cannot be expressed with smaller data or virtual time.

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
5. Errors typed (`AiError` 8 variants) with exponential backoff retry (300ms–5s, jitter); context-overflow triggers forced multi-round compaction + retry; `Retry-After` honored.
6. Tools authorized against `PermissionMode`, executed by `ToolRegistry`.
7. Skills: project/user/extra/built-in tiers; `<available_skills>` injected into system prompt; activation injects skill body before user message.
8. Goals: autonomous across turns via `update_goal_status`; no turn cap. Stored under `<session_dir>/goals/`.
9. Queue & steer: `Enter` while busy → follow-up (FIFO). `Ctrl+S` → steer at next break point. See `docs/queue-and-steer.md`.
10. Instructions: session-scoped `AGENTS.md` runtime (the only instruction filename, matched by exact stored directory-entry casing). Trust-gated baseline (`$NEO_HOME` global + trusted ancestor chain + workspace root) plus nested scopes discovered from typed tool paths (`Bash`/`Terminal` need explicit `cwd`; shell strings never parsed). Standalone `@path` directives and local Markdown links import `.md` rules under the workspace or `$NEO_HOME`; the user-global bundle may also import Markdown under the platform home without bypassing workspace trust (depth 5, 32 sources/graph, 1 MiB/source, 8 MiB/graph). Each canonical import source expands once; repeated and cyclic edges never block. Preflight defers the whole tool batch on new/changed scopes and the model replans in-turn; blocked scopes allow read-only diagnosis but block mutations. Budget `max(65_536, effective_max_tokens / 8)` clamped to safe capacity; over-budget → deterministic whole-bundle omission with a `⚠ Instructions partially loaded` transcript warning. Epochs are durable JSONL events, append-only (never mutate `system_prompt`), rehydrated byte-for-byte across compaction; transcript cards show metadata only.

### Built-in tools

`read`, `list`, `grep`, `find`, `glob`, `write`, `edit`, `bash`, `terminal` (PTY), `todo`, `enter_plan_mode`, `exit_plan_mode`. With `GoalManager`: `StartGoal`, `ExitGoalMode`, `UpdateGoalStatus`, `GetGoalStatus`. `ask_user` available but not registered by default.

### MCP namespacing

- MCP: `mcp__<server>__<tool>` via `McpStdioToolAdapter` / `McpHttpToolAdapter`. Resources are runtime state, not model context.

### Key TUI/UX contracts

- **Permission modes**: `ask`, `auto`, `yolo` — control tool approval policy.
- **Development modes**: `normal`, `plan`, `goal` — mutually exclusive. Shift+Tab cycles. Independent from permission modes.
- **Blocking dialogs** (`/resume`, `/model`, `/provider`, approval, `AskUserQuestion`, `ExitPlanMode`, `ExitGoalMode`): hide composer (`prompt_height = 0`), route all input to dialog. Tool batches with any blocking-dialog tool must execute sequentially even in parallel mode (exception: `AskUserQuestion` with `background = true`).
- Slash commands: `/ask`, `/auto`, `/yolo`, `/permissions`, `/plan`, `/model`, `/provider`, `/resume`, `/skill:<name>`, `/goal`.

### Provider types

`openai-responses`, `openai-compatible`, `openai-chat`, `anthropic`, `google`. Wire client selected by provider `type`, not model `api`.

### Config sections

`providers.<id>`, `models.<alias>`, `permission_mode`, `runtime` (temp, max_tokens, structured reasoning, queue/execution modes, compaction, extra_skill_dirs), `tui` (image_protocol, keybindings, completion_notification, question_notification), `mcp.servers`. System prompt: `~/.neo/SYSTEM.md`, `~/.neo/APPEND_SYSTEM.md`. Trust: `~/.neo/trust.json` gates project instruction loading (`AGENTS.md` only).

## Security

No unsafe code. API keys inline (`api_key`) or env-ref (`api_key_env`); `neo config show` redacts secrets. Write/execute tools workspace-contained; `Read` allows absolute paths outside workspace. Disabled MCP servers not started. Local-only surface.

## Persistent memory (ICM) — MANDATORY

```bash
icm recall-context "<task>" --limit 5    # before work
icm store -t <topic> -c "<desc>" -i high  # after resolving errors, making decisions, discovering preferences, completing significant work, or every ~20 tool calls
```

Never store trivial details, existing AGENTS.md facts, or transient logs.

## Git mutation policy — STRICT

The safety boundary is the worktree. `add`/`commit` are autonomous (see below); all other mutations need explicit authorization.

**Forbidden** (discard/rewrite worktree): `git reset --hard/--merge/--keep`, `git checkout/restore -- <path>`, `git stash`, `git rebase`, `git clean -fd`, `git rm`, `git commit --amend`, force push, `git filter-branch/repo`, `git gc --prune`, `git reflog expire`.

**Autonomous** (no authorization needed): `git add`, `git commit` — commit after each verified task per the work loop.

**Per-command authorization required**: `git push`, `merge`, `cherry-pick`, `checkout/switch <branch>`, `branch -d/-D`, `tag`, `worktree add/remove`.

**Read-only allowed**: `status`, `diff`, `log`, `show`, `branch` (no delete), `stash list`, `reflog`, `blame`, `ls-files`, `fsck`.

Blocked work is never a reason to revert files. Undo your own edits with targeted file edits, never `git checkout`. Subagent prompts must include this ban.

<!-- CODEGRAPH_START -->

## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

<!-- codebase-memory-mcp:start -->

## Codebase Knowledge Graph (codebase-memory-mcp)

This project uses codebase-memory-mcp to maintain a knowledge graph of the codebase and its reference projects.
ALWAYS prefer MCP graph tools over grep/glob/file-search for code discovery.

### Indexed projects

| Project name | Path | Purpose |
|---|---|---|
| `Users-chenyuanhao-Workspace-neo` | (workspace root) | Neo itself — primary development target |
| `neo-ref-claude-code` | `.references/claude-code` | Anthropic Claude Code — TypeScript agent reference |
| `neo-ref-codex` | `.references/codex` | OpenAI Codex — Rust agent reference |
| `neo-ref-opencode` | `.references/opencode` | OpenCode — Rust/TypeScript agent reference |
| `neo-ref-kimi-code` | `.references/kimi-code` | Kimi Code — TypeScript agent reference |
| `neo-ref-pi` | `.references/pi` | Pi — TypeScript terminal AI reference |
| `neo-ref-reasonix` | `.references/reasonix` | Reasonix — Go reasoning engine reference |

### Priority Order

1. `search_graph` — find functions, classes, routes, variables by pattern
2. `trace_path` — trace who calls a function or what it calls
3. `get_code_snippet` — read specific function/class source code
4. `query_graph` — run Cypher queries for complex patterns
5. `get_architecture` — high-level project summary

### When to fall back to grep/glob

- Searching for string literals, error messages, config values
- Searching non-code files (Dockerfiles, shell scripts, configs)
- When MCP tools return insufficient results

### Examples

- Find a handler: `search_graph(name_pattern=".*OrderHandler.*", project="Users-chenyuanhao-Workspace-neo")`
- Who calls it: `trace_path(function_name="OrderHandler", project="Users-chenyuanhao-Workspace-neo", direction="inbound")`
- Read source: `get_code_snippet(qualified_name="pkg.orders.OrderHandler", project="Users-chenyuanhao-Workspace-neo")`
- Explore a reference project: `get_architecture(project="neo-ref-codex", aspects=["overview"])`

<!-- codebase-memory-mcp:end -->
