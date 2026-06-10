# Neo Pi-Parity 8x Multi Agent Implementation Plan And Completion Record

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the recommended Neo product-parity path, not a strict Pi clone: build the missing local-first Rust coding-agent product capabilities with real implementations, no placeholders, no fake providers, and no overstated hosted/product claims.

**Architecture:** Neo remains a Rust monorepo with explicit crate ownership: `neo-ai` owns provider/model/image/reasoning primitives, `agent-core` owns tool/runtime/session primitives, `neo-agent` owns CLI/config/cloud/session command surfaces, `neo-tui` owns terminal UX, `neo-extensions` owns local package and extension lifecycle, `neo-cloud` owns the self-hosted service boundary, and `xtask` owns parity gates. Eight logical agents run on disjoint write scopes; the main coordinator integrates one module at a time into `codex/neo-pi-parity`.

**Tech Stack:** Rust workspace, Tokio, reqwest/axum/sqlx where already used, ratatui/crossterm style TUI stack where already used, SQLite-backed self-hosted `neo-cloud`, JSONL local sessions, `rtk` command wrapper, `xtask` parity/check/release-smoke gates.

---

## Current Branch State

- Main repo: `/Users/chenyuanhao/Workspace/neo`
- Branch: `codex/neo-pi-parity`
- Main has integrated the planned local-first parity tracks through these
  feature commits:
  - `e4e42e4 feat(tui): add pi-style app controls`
  - `4c2e56a feat(xtask): add release smoke parity gates`
  - `38a0298 feat(cloud): add self-hosted auth profile sync`
  - `0f5bbbd feat(cloud): align smoke health check`
  - `28888e6 feat(cloud): add hosted session share sync`
  - `d6e0112 docs(plan): add neo parity multi-agent plan`
  - `5f182de feat(ai): add provider pricing image controls`
  - `844422c feat(extensions): add marketplace package trust lifecycle`
  - `0b1bb48 feat(tui): add conservative inline image rendering`
  - `00e8a31 feat(agent-core): add terminal process supervision`
- Final coordination work is limited to gate hardening and documentation truth:
  RPC tests isolate `HOME`, cloud/session client tests avoid long/hanging
  helpers, and gap docs reflect the features that landed.

## Non-Negotiable Rules

- Do not revert user, main-thread, or other-worker changes.
- Use `apply_patch` for manual edits in this repo.
- Do not write secrets into the repo or user config. Config examples must use env var names such as `DEEPSEEK_API_KEY`.
- Preserve the user's DeepSeek preference. Internal provider ids may still appear as `anthropic/deepseek...` when that is the existing compatibility path, but do not replace DeepSeek defaults with Claude defaults.
- Do not claim full Pi cloud/product parity where Neo only has a self-hosted or local-first implementation.
- Do not claim full trust chain if the implemented marketplace trust is only manifest self-signing.
- Hosted MCP must fail closed unless there is a real local/self-hosted lifecycle implementation.
- Terminal image protocol selection must conservatively degrade; do not claim
  auto-detection without tested runtime protocol negotiation.
- Each integrated worker/module must land as its own commit after focused verification.

## Parallelism Model

The desired model is eight logical Multi Agent tracks. The current tool/session limit may allow fewer simultaneously active subagents, so execution uses waves while preserving the 8x ownership model:

- Wave 1: main coordinator plus the currently available workers with non-overlapping write sets.
- Wave 2: close completed workers, then start queued tracks that would otherwise exceed the tool limit.
- Main coordinator keeps the critical path local: resolve compile errors, merge one worker at a time, run focused tests, commit, then advance.

## Integration Order

1. Finish and commit Agent 4 AI/provider/catalog/pricing/image/reasoning because it is already partially applied in the main worktree.
2. Integrate Agent 5 marketplace/package trust because it mostly touches `crates/extensions` and `neo-agent` CLI command surfaces.
3. Integrate Agent 6 TUI inline image/diff/autocomplete, preserving `app.clear`, `app.exit`, and `app.suspend`.
4. Integrate Agent 7 PTY/process supervisor/hosted MCP lifecycle after fixing output-leak risks.
5. Integrate Agent 8 docs/xtask/parity/release gates.
6. Run final full workspace gates.

---

## Agent 1: App Controls And Terminal Parity Guard

**Status:** Already integrated in `e4e42e4`; keep as a guard track during later TUI/PTX merges.

**Owned Files:**
- `crates/tui/**`
- `crates/neo-agent/**` only for app action wiring
- TUI tests covering transcript rows, keybindings, clear/exit/suspend

**Do Not Touch:**
- `crates/ai/**`
- `crates/cloud/**`
- Marketplace package trust files

**Acceptance:**
- `app.clear` clears visible transcript state without corrupting session history.
- `app.exit` exits through the same path as Pi-style Ctrl-D exit.
- `app.suspend` restores terminal state before sending suspend behavior.
- `EditorUndo` remains available without colliding with Ctrl-Z suspend.

**Verification:**
- [ ] Run `rtk cargo test -p neo-tui`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands`
- [ ] Before merging Agent 6, inspect diffs for accidental changes to `app.clear`, `app.exit`, `app.suspend`

**Commit Rule:**
- No new commit required unless Agent 6 regresses these behaviors and this guard track fixes them.

---

## Agent 2: Self-Hosted Auth And Profile Sync

**Status:** Already integrated in `38a0298` and `0f5bbbd`; this is the cloud identity boundary that later provider credential work must reuse.

**Owned Files:**
- `crates/neo-agent/src/cloud_commands.rs`
- `crates/neo-agent/src/config.rs`
- `crates/cloud/**`
- `crates/sdk/**` cloud client methods
- Cloud/auth related CLI tests

**Do Not Touch:**
- `crates/ai/src/auth.rs` except through explicit main-coordinator integration
- TUI rendering internals
- Extension archive/trust internals

**Acceptance:**
- `neo cloud login` stores self-hosted profile/auth state without embedding provider secrets in logs.
- `neo cloud status` reports reachable/unreachable states honestly.
- `neo config sync` uses the same cloud auth path, not a separate `.neo/cloud-profiles.json` bypass.
- Release smoke can start `neo-cloud` and run `neo cloud status`.

**Verification:**
- [ ] Run `rtk cargo test -p neo-cloud`
- [ ] Run `rtk cargo test -p neo-sdk --test cloud_client`
- [ ] Run `rtk cargo run -p xtask -- release-smoke`

**Commit Rule:**
- No new commit required unless later AI credential work needs an adapter into this already-integrated auth/profile path.

---

## Agent 3: Hosted Session Share And Remote Continuation

**Status:** Already integrated in `28888e6`; this is the session/share boundary for later TUI, CLI, and cloud work.

**Owned Files:**
- `crates/cloud/**` session/share routes
- `crates/sdk/**` session/share client calls
- `crates/agent-core/**` session metadata/tree support
- `crates/neo-agent/**` session share/sync/import/resume command surfaces

**Do Not Touch:**
- Provider-specific request code
- Marketplace package code
- PTY process supervisor internals unless Agent 7 needs session process metadata and coordinates through main

**Acceptance:**
- `POST /v1/sessions/import` stores remote sessions in real SQLite state.
- `POST /v1/sessions/{session_id}/shares` creates real share records.
- `GET /v1/shares/{share_id}.html` and `.json` return real exported session content.
- `neo sessions sync push|pull|status` uses cloud auth and local JSONL state.
- `neo resume cs_...` forks a remote session and writes a local JSONL continuation.

**Verification:**
- [ ] Run `rtk cargo test -p neo-cloud`
- [ ] Run `rtk cargo test -p neo-sdk --test cloud_client`
- [ ] Run `rtk cargo test -p neo-agent-core --test session_tree`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands`
- [ ] Run `rtk cargo run -p xtask -- release-smoke`

**Commit Rule:**
- No new commit required unless later modules need session/share adjustments.

---

## Agent 4: AI Provider Auth, Generated Catalog, Pricing, Image Generation, Reasoning

**Status:** Integrated in `5f182de`.

**Source Worker:**
- `/Users/chenyuanhao/.config/superpowers/worktrees/neo/codex/neo-pi-parity-worker4`

**Owned Files:**
- `crates/ai/src/auth.rs`
- `crates/ai/src/image_generation.rs`
- `crates/ai/src/reasoning.rs`
- `crates/ai/src/providers/openai_images.rs`
- `crates/ai/src/providers/mod.rs`
- `crates/ai/src/registry.rs`
- `crates/ai/src/lib.rs`
- `crates/ai/tests/**`
- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-agent/src/modes/run.rs`
- `crates/neo-agent/Cargo.toml`
- Workspace `Cargo.toml` only for real dependency wiring such as `base64`

**Do Not Touch:**
- `crates/cloud/**` route semantics except through existing auth/profile APIs.
- TUI image rendering internals; Agent 4 only generates image bytes/files through CLI/API.
- Marketplace package code.

**Required Implementation:**
- [ ] Keep `neo-ai` generated catalog parsing already applied in `ModelRegistry`.
- [ ] Keep `ModelPricing`, `TokenPricing`, and `ImageGenerationPricing` as structured registry data.
- [ ] Keep `supports_image_generation(provider, model)` as an explicit registry query.
- [ ] Keep `ReasoningPolicy::Auto` deterministic and model-capability-aware.
- [ ] Keep `sanitize_reasoning_continuation` so signed/redacted opaque thinking is not carried across provider/API boundaries.
- [ ] Keep `CredentialResolver` with priority order: CLI key, env var, auth file, cloud profile.
- [ ] Adapt cloud profile credentials to the main `config::cloud_auth_file` and `cloud_commands` path. Do not keep a separate `.neo/cloud-profiles.json` bypass.
- [ ] Add `models list --pricing`.
- [ ] Add `models list --pricing --json`.
- [ ] Add `images generate <prompt> --model <provider/model> --output <path> --size <size>`.
- [ ] Make image generation use a real HTTP client path for supported OpenAI image models.
- [ ] If the provider returns base64 image data, write decoded bytes to the requested output path.
- [ ] If the provider returns a URL-only image response and Neo does not download URLs yet, fail with a clear error that names the unsupported response shape.
- [ ] Do not synthesize image bytes in tests; use local HTTP fixture responses.

**Focused Verification:**
- [x] Run `rtk cargo test -p neo-ai`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands`
- [ ] Run the provider E2E integration test suite.
- [ ] Run `rtk cargo run -p neo-agent -- models list --pricing`
- [ ] Run `rtk cargo run -p neo-agent -- models list --pricing --json`

**Commit Rule:**
- Commit after focused verification:
  - `feat(ai): add provider pricing image and reasoning controls`

---

## Agent 5: Extension Marketplace, Package Archives, And Trust Checks

**Status:** Integrated in `844422c`.

**Source Worker:**
- `/Users/chenyuanhao/.config/superpowers/worktrees/neo/codex/neo-pi-parity-worker3`

**Owned Files:**
- `crates/extensions/**`
- `crates/neo-agent/src/cli.rs` only marketplace/package commands
- `crates/neo-agent/src/main.rs` only marketplace/package dispatch
- `crates/neo-agent/src/modes/run.rs` only command implementation glue
- Extension marketplace/package docs and tests

**Do Not Touch:**
- `crates/ai/**`
- `crates/cloud/**`
- TUI rendering internals
- PTY process supervisor code

**Required Implementation:**
- [ ] Search local/self-hosted marketplace index.
- [ ] Install extension packages from a real archive source.
- [ ] Update installed extension packages.
- [ ] Uninstall packages and remove installed files.
- [ ] Enable and disable installed packages.
- [ ] Reject archive path traversal entries.
- [ ] Reject malformed manifests.
- [ ] Verify manifest self-signatures if that is the implemented trust level.
- [ ] Name the trust level honestly as manifest self-signing unless a root/publisher trust chain is actually implemented.
- [ ] Preserve existing local extension manifest behavior.

**Focused Verification:**
- [ ] Run `rtk cargo test -p neo-extensions`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands marketplace`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands package`
- [ ] Run `rtk cargo run -p neo-agent -- extensions --help`

**Commit Rule:**
- Commit after focused verification:
  - `feat(extensions): add marketplace package trust lifecycle`

---

## Agent 6: TUI Inline Images, Diff UI, And Command Autocomplete

**Status:** Integrated in `0b1bb48`.

**Source Worker:**
- `/Users/chenyuanhao/.config/superpowers/worktrees/neo/codex/neo-pi-parity-worker5`

**Owned Files:**
- `crates/tui/**`
- `crates/neo-agent/**` only TUI wiring and command autocomplete registration
- TUI snapshot/unit tests

**Do Not Touch:**
- `crates/ai/**`
- `crates/cloud/**`
- `crates/extensions/**`
- PTY process supervisor internals

**Required Implementation:**
- [ ] Add terminal capability representation that distinguishes Kitty, iTerm2/OSC, Sixel, and none.
- [ ] Add runtime image capability negotiation or a conservative fallback.
- [ ] In auto mode, degrade to a file/link/plain preview if capability is unknown.
- [ ] Render inline images only when a supported terminal protocol is positively selected.
- [ ] Connect generated or attached image artifacts to the TUI transcript without corrupting session JSONL.
- [ ] Improve diff view navigation and visual grouping without putting cards inside cards or introducing marketing-style UI.
- [ ] Add command autocomplete for hosted/share/extension commands only where command sources are real.
- [ ] Preserve `app.clear`, `app.exit`, `app.suspend`, Ctrl-C, Ctrl-D, and Ctrl-Z behavior.

**Focused Verification:**
- [ ] Run `rtk cargo test -p neo-tui`
- [ ] Run `rtk cargo test -p neo-agent --test cli_commands tui`
- [ ] Run the provider E2E integration test suite.
- [ ] Manually inspect at least one text-only fallback transcript path.

**Commit Rule:**
- Commit after focused verification:
  - `feat(tui): add inline media diff and command polish`

---

## Agent 7: PTY, Process Supervisor, And Hosted MCP Lifecycle

**Status:** Integrated in `00e8a31`.

**Source Worker:**
- `/Users/chenyuanhao/.config/superpowers/worktrees/neo/codex/neo-pi-parity-worker6-e4`

**Owned Files:**
- `crates/agent-core/**` process, shell, tool runtime, MCP lifecycle
- `crates/neo-agent/**` only CLI/config glue for PTY/process/MCP lifecycle
- `crates/sdk/**` only if a real hosted/self-hosted MCP client boundary exists
- PTY/process/MCP tests

**Do Not Touch:**
- `crates/ai/**`
- `crates/extensions/**`
- TUI transcript rendering except minimal event display wiring coordinated through main

**Required Implementation:**
- [ ] Add a real PTY path for interactive shell sessions where supported.
- [ ] Keep non-PTY command execution stable for existing tests.
- [ ] Add process supervisor cleanup for spawned child processes.
- [ ] Add deterministic shutdown behavior for cancellation and session exit.
- [ ] Enforce `max_output_bytes` consistently across user-visible output, tool details, events, and persisted session records.
- [ ] Fix the known worker risk: capped output must not leak full content through details/events.
- [ ] Add hosted MCP lifecycle only if it maps to real local/self-hosted startup/auth/health/shutdown operations.
- [ ] If hosted MCP is not implemented, fail closed with a clear unsupported error.

**Focused Verification:**
- [ ] Run `rtk cargo test -p neo-agent-core`
- [ ] Run the provider E2E integration test suite.
- [ ] Run PTY/process filtered tests by exact test names from the worker branch.
- [ ] Add or keep a regression test proving no full-output leak past `max_output_bytes`.

**Commit Rule:**
- Commit after focused verification:
  - `feat(agent-core): add pty process supervisor lifecycle`

---

## Agent 8: Xtask, Docs, Gap Ledger, And Final Release Gates

**Status:** Partially integrated through `4c2e56a`; final documentation and
gate-hardening pass is handled by the main coordinator after all module commits.

**Source Worker Candidates:**
- `/Users/chenyuanhao/.config/superpowers/worktrees/neo/codex/neo-pi-parity-worker7`
- Main worktree after all feature commits

**Owned Files:**
- `xtask/**`
- `docs/gap/**`
- `docs/providers.md`
- `docs/sessions.md`
- `docs/config.md`
- `docs/mcp.md`
- `docs/api/**`
- `examples/**` only when examples are real and compile/run under existing gates

**Do Not Touch:**
- Feature implementation crates except to fix docs-test compilation or stale API references.

**Required Implementation:**
- [ ] Update `docs/gap/INDEX.md` with honest residual gaps after all commits.
- [ ] Update `docs/gap/neo-ai.md` for provider auth, generated catalog, pricing, image generation, and reasoning.
- [ ] Update `docs/gap/neo-agent.md` for CLI/cloud/session/marketplace command surfaces.
- [ ] Update `docs/gap/neo-agent-core.md` for PTY/process/MCP truth.
- [ ] Update `docs/gap/tui.md` for inline image capability negotiation and fallback truth.
- [ ] Update `docs/gap/xtask.md` for final gates.
- [ ] Ensure `xtask parity` does not mark a module complete unless code and docs agree.
- [ ] Ensure stand-in implementation scans are meaningful and do not whitelist invented public behavior.
- [ ] Ensure release smoke starts only real local services and uses local HTTP fixtures where external credentials would otherwise be required.

**Focused Verification:**
- [ ] Run `rtk cargo run -p xtask -- parity`
- [ ] Run `rtk cargo run -p xtask -- check --docs`
- [ ] Run `rtk cargo run -p xtask -- release-smoke`

**Commit Rule:**
- Commit after final docs/gate verification:
  - `docs(parity): update neo pi parity gap ledger`
  - or `feat(xtask): tighten final parity release gates` if code changes are substantial

---

## Final Workspace Gate

Run these from `/Users/chenyuanhao/Workspace/neo` after all module commits land:

- [x] `rtk cargo fmt --all --check`
- [x] `rtk cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] `rtk cargo test --workspace --all-features`
- [x] `rtk cargo run -p xtask -- parity`
- [x] `rtk cargo run -p xtask -- check --docs`
- [x] `rtk cargo run -p xtask -- release-smoke`
- [x] `rtk cargo run -p neo-agent -- --help`
- [x] `rtk cargo run -p neo-agent -- models list --pricing`

## Final Acceptance Bar

Neo can be called substantially closer to the recommended Pi-like local coding-agent product when all of the following are true:

- All eight logical tracks above are either integrated or explicitly marked as honestly unsupported in docs and CLI errors.
- Main branch has no uncommitted feature work.
- No stand-in implementation is used to satisfy a public command.
- Cloud/session/share features work against the self-hosted `neo-cloud` boundary.
- Marketplace package trust claims match implemented behavior.
- Image generation performs real HTTP calls and writes real provider-returned image bytes.
- TUI inline image behavior degrades safely when terminal capability is unknown.
- PTY/process output caps apply everywhere user-visible or persisted.
- Final workspace gate passes.

## Stop Conditions

Stop and ask the user only if one of these occurs:

- A live external credential is required and no local fixture or self-hosted path can verify the behavior.
- A third-party API contract is unavailable and guessing would create invented behavior.
- A legal/security-sensitive secret or signing root is required.
- Two active workers produce incompatible implementations for the same public command and neither can be adapted without changing product semantics.

## Immediate Next Actions

- [x] Finish Agent 4 `neo-agent` CLI integration for `models list --pricing` and `images generate`.
- [x] Run Agent 4 focused tests and commit.
- [x] Read Agent 5 worker result, merge marketplace/package trust, verify, commit.
- [x] Read Agent 6 worker branch manually, merge TUI polish, verify, commit.
- [x] Merge Agent 7 after fixing output-leak risk, verify, commit.
- [x] Run Agent 8 docs/gates and final workspace gate after the closing docs/test-isolation commit.
