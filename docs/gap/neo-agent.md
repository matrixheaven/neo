# neo-agent Gap Map

## Implemented Local Surface

- Global flags: `--model`/`NEO_MODEL`, `--provider`/`NEO_PROVIDER`,
  `--api-base`/`NEO_API_BASE`, `--config`/`NEO_CONFIG`,
  `--mode`/`NEO_MODE`, `--offline`/`NEO_OFFLINE`, `--verbose`,
  `--thinking`, tool filters, prompt-template selectors, skill paths, extension
  paths, theme paths, trust overrides, and local session selection flags.
- Provider-backed `print`, `run`, and local `resume` resolve models through
  config plus the production provider registry, merge piped stdin, expand
  project-relative `@file` prompt arguments, and load local system/context
  resources.
- Config loading merges CLI overrides, environment overrides, project
  `.neo/config.toml`, user-global `~/.neo/config.toml`, and built-in defaults.
  It supports provider-specific base URLs and API key environment names,
  runtime generation options, queue modes, tool execution mode, compaction
  thresholds, `[tui.keybindings]`, `tui.image_protocol`, and
  `tui.fetch_remote_images`.
- Session commands read local `sessions_dir` files, store local tree/name
  metadata next to JSONL records, resolve exact ids, unique prefixes, and
  in-directory JSONL paths, compact sessions with deterministic transcript
  summaries, render local trees, and export standalone HTML or stable local JSON
  artifacts without leaking absolute session paths.
- Local skills load from `~/.neo/skills`, project `.neo/skills`, and explicit
  `--skill <PATH>` entries.
- Local extensions install from directories or explicit git/file URLs into
  project `.neo/extensions/<id>`, record local sources, persist enablement
  state, round-trip JSONL RPC over stdio, and expose enabled tools through
  `tools.list` as `extension__<extension>__<tool>` functions.
- Prompt templates and themes are local project/user assets. `prompts
  list/preview` and `themes list/preview` inspect local install roots.
- MCP commands inspect configured servers, discover tools, list/read/watch
  resources, and manage local configured server entries. Provider-backed turns
  load enabled stdio, HTTP, and SSE MCP entries and register their tools.
- `images generate` uses OpenAI-style image endpoints for catalog models with
  image-generation capability metadata. URL-only provider responses require
  explicit `tui.fetch_remote_images = true` and still pass the remote image URL
  policy.
- RPC mode supports local prompt-template command discovery, prompt execution,
  JSONL-backed message replay, local session navigation, and local HTML/JSON
  export payloads.
- Interactive mode renders the live TUI, dispatches editor/approval/session
  picker/model picker/command palette actions, streams provider thinking as
  visible notices, preserves reasoning separately from final assistant text,
  supports local `/tree`, local fork-before-continue, transcript selection
  copy, and conservative inline image rendering under configured terminal
  protocol preferences.

## Pi Parity Pressure

Pi's coding-agent docs include provider login, settings sync, hosted sharing,
extension distribution, themes, terminal setup, and platform-specific guidance.
Neo should borrow only the local ergonomics that are backed by Rust code.

## Local-Only Non-Goals

- Do not document `neo-cloud`, profile sync, hosted share/import, remote resume,
  or managed collaboration as supported local-agent features.
- Do not document hosted MCP registries, OAuth onboarding, or hosted server
  lifecycle management as supported MCP features.
- Do not document marketplace search/install/publish, package accounts,
  publisher/root trust chains, or hosted distribution UX as supported local
  features.
- Keep `/tree` documented as the local session picker slash command only.
- Keep package/archive trust language limited to unsupported distribution
  surfaces unless product scope changes and code is updated first.
