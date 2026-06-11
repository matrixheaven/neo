# Neo Local-Only Slimming Plan Completion Record

> Supersedes the earlier Pi-parity/cloud/marketplace plan. Neo is now scoped as
> a local AI coding agent, not a hosted collaboration or package ecosystem
> product.

## Goal

Slim Neo back to a strong local agent runtime:

- Keep the agent turn loop, local tools, permissions, JSONL sessions,
  resume/fork/compact/export, MCP stdio/http/sse, provider abstraction,
  reasoning handoff/replay, image generation, remote image URL fetch policy,
  and conservative TUI inline image protocol selection.
- Keep local prompt, theme, skill, and extension discovery.
- Remove cloud services, profile/config sync, hosted share/import/remote
  resume, hosted MCP registry/lifecycle, marketplace search/install/publish,
  publisher/root trust, package archive lifecycle, and pricing/product-catalog
  CLI surface.

## Local Architecture

- `neo-ai`: provider/model/image/reasoning primitives.
- `agent-core`: tool loop, runtime, permissions, local session primitives, MCP
  adapters.
- `neo-agent`: CLI/config/session commands, local extensions/prompts/themes,
  image generation command, local MCP commands, and TUI entrypoint.
- `neo-tui`: terminal UI, app controls, diff/transcript rendering, inline image
  metadata/protocol fallback.
- `neo-extensions`: local directory extension discovery/install/update/status
  and RPC runner support.
- `xtask`: local-only parity, docs, catalog, examples, and release-smoke gates.

## Removed Product Surfaces

- `neo-cloud` service crate and cloud protocol crate.
- SDK cloud client and tests.
- CLI `login`, `logout`, `auth`, `cloud`, `config sync`.
- Session `sync`, `share`, `import`, hosted continuation, and remote resume.
- Cloud MCP transport/registry.
- Marketplace package search/install/publish/update/uninstall.
- Publisher/root/key trust chain and package archive verification lifecycle.
- Model `--pricing` CLI output/source metadata product surface.

## Preserved Advanced Local Features

- Multi-provider reasoning effort and thinking replay controls.
- Image generation plus explicit remote URL fetch policy.
- TUI inline image rendering with conservative protocol configuration/fallback.
- PTY/process supervision and stdio/http/sse MCP lifecycle for configured local
  or directly specified endpoints.

## Verification Shape

Focused completion should prove:

- `cargo check -p neo-agent`
- `cargo test -p neo-extensions`
- `cargo test -p xtask`
- `cargo run -p xtask -- check --docs`
- targeted session/MCP/CLI tests for edited surfaces

Do not reintroduce hosted/cloud/marketplace gates as release blockers for the
local-only agent.
