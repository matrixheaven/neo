# Neo/Pi Gap Map

This directory tracks high-priority documentation and automation parity gaps
between the pi package docs under `pi/` and the current Neo Rust workspace.
Each file is module-scoped so code workers can close gaps without rewriting the
whole map.

## Priority Map

| Module | Current Neo surface | Pi reference pressure | High-priority gap |
| --- | --- | --- | --- |
| [`neo-ai`](neo-ai.md) | Provider-neutral `ChatRequest`, `RequestOptions` including typed OpenAI reasoning effort, `ModelRegistry`, strict local JSON model catalogs including a supported custom-model subset of Pi `models.json`, `ProviderRegistry`, provider/API compatibility validation in `ProviderResolver`, OpenAI Responses, Anthropic Messages, Google Generative AI, OpenAI-compatible adapters with chat image-input serialization, OpenAI Responses reasoning-summary stream events, Anthropic Messages extended-thinking stream events with signature passthrough, Google Generative AI thought-part stream events with signature passthrough, Anthropic/Google budget-based thinking request payloads, tool schemas, test fake provider, and environment key helpers. | `pi-ai` documents broad provider discovery, credential resolution, tool streaming, provider-native reasoning, images, and context handoff. | Keep Neo docs focused on implemented Rust contracts; leave Pi pricing/generated catalog/provider override/auth APIs plus adaptive/off-state reasoning controls and encrypted reasoning replay as explicit gaps. |
| [`neo-agent-core`](neo-agent-core.md) | Runtime turn loop, `AgentConfig`, `AgentContext`, normalized lifecycle events including run/message/turn/thinking barriers, `run_turn_with_cancel` for in-flight model stream and tool-future cancellation, permissions with synchronous and async approval handlers, recoverable tool errors returned as model-visible error tool results, built-in workspace tools including edit diff details and compact background bash stop with Unix shell process-group cleanup, MCP adapter/provider boundary with stdio and HTTP/SSE tool/resource transports, JSONL session reader/writer, local session metadata fork/rename, deterministic JSONL compaction, fake harness. | `pi-agent-core` documents richer event lifecycle, hooks, steering, parallel tool execution, and cancellation. | Document implemented event/tool/MCP/session APIs now; leave hosted/alternate-channel MCP lifecycle, daemonized process reaping, and richer hook lifecycle gaps explicit. |
| [`neo-agent`](neo-agent.md) | Clap command surface, project `.neo/config.toml`, user-global `~/.neo/config.toml`, environment overrides including `--mode`/`NEO_MODE`, provider-specific base URL/API key env config, config setters including runtime `AgentConfig` options, provider-backed print/run with piped stdin merging, project-relative `@file` text prompt expansion, project `.neo/prompts/*.md` plus user-global `~/.neo/prompts/*.md` slash prompt templates, TOML `prompt_templates` selectors, explicit repeatable `--prompt-template <NAME_OR_PATH>` selectors for local names/files/non-recursive directories with duplicate-name diagnostics, `--no-prompt-templates` automatic-discovery disablement, `run --output json` and `--mode json run ...` stable typed JSONL including assistant thinking content events, sessions commands including exact/prefix/path resolution plus local tree/fork/rename/summarize/compact/HTML export, skill loading, extension install/update/uninstall plus JSONL RPC calls and status/enable/disable lifecycle, JSONL-backed RPC `get_commands` prompt-template discovery, `get_messages`, plus local `sessions.list` / `sessions.tree` / `sessions.get` / `sessions.export_html`, models list, MCP server listing, explicit MCP resource list/read/watch commands, and live TUI local command/session/model pickers plus project-directory prompt file path, local slash prompt-template Tab completion, and inline `@provider/model` Tab completion that replay selected JSONL sessions, order local session trees, fork selected sessions before continuation, append continuation turns, and apply selected model overrides to later turns. | pi coding-agent docs cover settings, providers, sessions, TUI, MCP/resources, and trust. | Describe actual project/global config, session, extension, prompt-template, and RPC commands while marking package/trust/hosted marketplace/share gaps explicitly. |
| [`neo-tui`](tui.md) | Prompt/editor with undo/kill-ring yank, internal copy buffer plus live OS clipboard prompt and transcript-selection copy, filesystem-backed prompt Tab completion, local slash prompt-template completion, inline `@provider/model` completion metadata, rendered transcript viewport scrolling, item-range transcript selection/highlighting, keybinding, command palette, selection/list paging, width-safe rendering primitives, unified diff transcript classification/coloring, safe image-content metadata summaries in transcripts, session transcript loading, streamed thinking notices, bracketed paste buffering, and a live raw-mode keybinding dispatcher for prompt/approval/command/session/model-picker/session-fork/transcript-selection actions. | `pi-tui` has a richer terminal renderer and interaction stack. | Keep TUI docs scoped to implemented primitives and live prompt/overlay actions; Kitty/Sixel/OSC image protocols, hosted/extension command catalogs, and advanced diff affordances remain explicit gaps. |
| [`xtask`](xtask.md) | Stable xtask gate, opt-in workspace gate, docs/examples parity scan, Markdown local-link validation, example TOML/JSON validation, and Rust example compile checks through `examples/rust/Cargo.toml`. | pi has npm check/supply-chain automation and generated docs metadata. | Preserve a small Rust gate, block fake/placeholder production guidance, and add future checks only when Neo has stable generated artifacts. |

## Cross-Cutting Gaps

- OpenAI Responses, Anthropic Messages, Google Generative AI, and
  OpenAI-compatible adapters are wired through `ProviderRegistry::production()`
  and `ProviderResolver`, which now fails provider/API mismatches before
  credential lookup, serializes supported chat image inputs instead of
  silently dropping image parts, and maps typed reasoning effort into OpenAI
  Responses / OpenAI-compatible request payloads and Anthropic/Google
  budget-based thinking request payloads. OpenAI Responses reasoning
  summary SSE events are mapped into provider-neutral thinking events that
  `neo-agent-core`, stable JSONL output, and `neo-tui` consume without mixing
  thinking content into plain assistant text. Strict local JSON model catalogs
  can extend
  `ModelRegistry`, including a supported custom-model subset of Pi
  `models.json` that rejects request-affecting provider/model metadata until
  Neo has explicit runtime contracts for those fields. Neo provider-specific
  base URLs and API key env names are configured through `.neo/config.toml`,
  while Pi pricing metadata, generated catalog sources, automatic
  provider-metadata migration, auth-file/OAuth flows, image generation, and
  local APIs remain catalog/API gaps.
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
- RPC mode now exposes local prompt-template command discovery through
  `get_commands`, real session message replay through `get_messages`, local
  session navigation through `sessions.list` / `sessions.tree`, metadata plus
  replayed message payloads through `sessions.get`, and sanitized local HTML
  export through `sessions.export_html`; hosted
  streaming/session services remain out of scope until they have backing
  infrastructure.

## Docs Updated In This Pass

- Quickstart now describes the stable `xtask` docs gate and the opt-in
  workspace gate, including the docs/examples parity scan.
- Configuration docs now reflect `.neo/config.toml`, `NEO_*` overrides, and
  supported `neo config set` keys without treating deterministic development
  fixtures as production defaults.
- Providers docs now reflect `ModelSpec`, strict local model catalogs,
  `RequestOptions`, typed OpenAI reasoning effort, environment key helpers,
  production provider resolution, real OpenAI/Anthropic-compatible clients,
  fail-closed Pi catalog metadata import, and the fake test provider.
- Tools docs now list the implemented built-in workspace tools and permissions.
- Sessions docs now describe JSONL event persistence, local session metadata
  fork/rename, deterministic extractive compaction, and current CLI/session
  reader behavior.
- Examples now include Rust snippets for provider registry, tool schemas,
  runtime turns, and JSONL session replay.
