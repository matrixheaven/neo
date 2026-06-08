# Neo/Pi Gap Map

This directory tracks high-priority documentation and automation parity gaps
between the pi package docs under `pi/` and the current Neo Rust workspace.
Each file is module-scoped so code workers can close gaps without rewriting the
whole map.

## Priority Map

| Module | Current Neo surface | Pi reference pressure | High-priority gap |
| --- | --- | --- | --- |
| [`neo-ai`](neo-ai.md) | Provider-neutral `ChatRequest`, `RequestOptions`, `ModelRegistry`, strict local JSON model catalogs including a supported custom-model subset of Pi `models.json`, `ProviderRegistry`, `ProviderResolver`, OpenAI Responses, Anthropic Messages, Google Generative AI, OpenAI-compatible adapters with chat image-input serialization, stream events, tool schemas, test fake provider, and environment key helpers. | `pi-ai` documents broad provider discovery, credential resolution, tool streaming, reasoning, images, and context handoff. | Keep Neo docs focused on implemented Rust contracts and mark unsupported Pi pricing/generated catalog/provider override/auth APIs as future work. |
| [`neo-agent-core`](neo-agent-core.md) | Runtime turn loop, `AgentConfig`, `AgentContext`, normalized lifecycle events including run/message/turn barriers, permissions with synchronous and async approval handlers, built-in workspace tools, MCP adapter/provider boundary with stdio and HTTP/SSE tool/resource transports, JSONL session reader/writer, local session metadata fork/rename, deterministic JSONL compaction, fake harness. | `pi-agent-core` documents richer event lifecycle, hooks, steering, parallel tool execution, and cancellation. | Document implemented event/tool/MCP/session APIs now; leave hosted/alternate-channel MCP lifecycle and richer hook lifecycle gaps explicit. |
| [`neo-agent`](neo-agent.md) | Clap command surface, project `.neo/config.toml`, user-global `~/.neo/config.toml`, environment overrides, provider-specific base URL/API key env config, config setters including runtime `AgentConfig` options, provider-backed print/run, sessions commands including local tree/fork/rename/summarize/compact/HTML export, skill loading, extension install/update/uninstall plus JSONL RPC calls and status/enable/disable lifecycle, JSONL-backed RPC `get_messages` plus local `sessions.list` / `sessions.tree`, models list, MCP server listing, explicit MCP resource list/read/watch commands, and live TUI local session/model pickers plus project-directory prompt file path completion that replay selected JSONL sessions, order local session trees, fork selected sessions before continuation, append continuation turns, and apply selected model overrides to later turns. | pi coding-agent docs cover settings, providers, sessions, TUI, MCP/resources, and trust. | Describe actual project/global config, session, extension, and RPC commands while marking hosted trust/marketplace/share gaps explicitly. |
| [`neo-tui`](tui.md) | Prompt/editor with undo/kill-ring yank and internal copy buffer, filesystem-backed prompt Tab completion, rendered transcript viewport scrolling, keybinding, selection/list paging, width-safe rendering primitives, session transcript loading, and a live raw-mode keybinding dispatcher for prompt/approval/session/model-picker/session-fork actions. | `pi-tui` has a richer terminal renderer and interaction stack. | Keep TUI docs scoped to implemented primitives and live prompt/overlay actions until diff rendering, images, richer autocomplete, OS clipboard integration, and stdin buffering land. |
| [`xtask`](xtask.md) | Stable xtask gate, opt-in workspace gate, docs/examples parity scan, Markdown local-link validation, example TOML/JSON validation, and Rust example compile checks through `examples/rust/Cargo.toml`. | pi has npm check/supply-chain automation and generated docs metadata. | Preserve a small Rust gate, block fake/placeholder production guidance, and add future checks only when Neo has stable generated artifacts. |

## Cross-Cutting Gaps

- OpenAI Responses, Anthropic Messages, Google Generative AI, and
  OpenAI-compatible adapters are wired through `ProviderRegistry::production()`
  and `ProviderResolver`, and they serialize supported chat image inputs instead
  of silently dropping image parts. Strict local JSON model catalogs can extend
  `ModelRegistry`, including a supported custom-model subset of Pi
  `models.json`; Neo provider-specific base URLs and API key env names are
  configured through `.neo/config.toml`, while Pi pricing metadata, generated
  catalog sources, automatic provider-metadata migration, auth-file/OAuth
  flows, image generation, and local APIs remain catalog/API gaps.
- MCP is documented as a runtime boundary. `neo-agent-core` now has the
  adapter/provider abstraction, stdio JSON-RPC process adapter, HTTP/SSE
  JSON-RPC adapter, discovery-to-`ToolSpec` bridge, namespaced `ToolRegistry`
  registration, persistent initialized stdio session reuse, explicit
  resources/list and resources/read, stdio and HTTP/SSE resource subscriptions
  backed by real notification streams, queued resource update notifications,
  and async call delegation. `neo-agent print` and `neo-agent run` load enabled
  stdio, HTTP, and SSE MCP entries from project config; `neo mcp resources ...
  watch` exposes stdio and live remote SSE resource updates without injecting
  them into model context. Hosted MCP management and remote servers that require
  a separate event channel remain gaps.
- Session storage is implemented as JSONL event persistence in
  `neo-agent-core`; local tree fork/rename metadata, deterministic extractive
  compaction, schema metadata, local branch summaries, and HTML export are
  wired, while hosted share and model-generated/richer compaction remain
  pi-inspired future work.
- Project runtime config now maps generation options, queue modes, tool
  execution mode, permission policy, live interactive approval decisions, and
  compaction thresholds into real `AgentConfig` fields for provider-backed
  runs.
- RPC mode now exposes real session message replay through `get_messages` and
  local session navigation through `sessions.list` / `sessions.tree`; hosted
  streaming/session services remain out of scope until they have backing
  infrastructure.

## Docs Updated In This Pass

- Quickstart now describes the stable `xtask` docs gate and the opt-in
  workspace gate, including the docs/examples parity scan.
- Configuration docs now reflect `.neo/config.toml`, `NEO_*` overrides, and
  supported `neo config set` keys without treating deterministic development
  fixtures as production defaults.
- Providers docs now reflect `ModelSpec`, strict local model catalogs,
  `RequestOptions`, environment key helpers, production provider resolution,
  real OpenAI/Anthropic-compatible clients, and the fake test provider.
- Tools docs now list the implemented built-in workspace tools and permissions.
- Sessions docs now describe JSONL event persistence, local session metadata
  fork/rename, deterministic extractive compaction, and current CLI/session
  reader behavior.
- Examples now include Rust snippets for provider registry, tool schemas,
  runtime turns, and JSONL session replay.
