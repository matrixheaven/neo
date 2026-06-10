# Neo/Pi Gap Map

This directory tracks high-priority documentation and automation parity gaps
between the pi package docs under `pi/` and the current Neo Rust workspace.
Each file is module-scoped so code workers can close gaps without rewriting the
whole map.

## Priority Map

| Module | Current Neo surface | Pi reference pressure | High-priority gap |
| --- | --- | --- | --- |
| [`neo-ai`](neo-ai.md) | Provider-neutral `ChatRequest`, `RequestOptions` including typed OpenAI reasoning effort, `ModelRegistry`, strict local JSON model catalogs including generated catalog pricing/image-generation metadata and a supported custom-model subset of Pi `models.json`, `ProviderRegistry`, provider/API compatibility validation in `ProviderResolver`, OpenAI Responses, Anthropic Messages, Google Generative AI, OpenAI-compatible adapters with chat image-input serialization, OpenAI-style image generation client primitives, OpenAI Responses reasoning-summary stream events, Anthropic Messages extended-thinking stream events with signature passthrough, Google Generative AI thought-part stream events with signature passthrough, Anthropic/Google budget-based thinking request payloads, tool schemas, test fake provider, and credential resolution helpers. | `pi-ai` documents broad provider discovery, credential resolution, tool streaming, provider-native reasoning, images, and context handoff. | Keep Neo docs focused on implemented Rust contracts; leave Pi `models.json` pricing import, upstream generated catalog production, provider override/auth APIs, OAuth, adaptive/off-state reasoning controls, and cross-provider handoff as explicit gaps. |
| [`neo-agent-core`](neo-agent-core.md) | Runtime turn loop, `AgentConfig`, `AgentContext`, normalized lifecycle events including run/message/turn/thinking barriers, `run_turn_with_cancel` for in-flight model stream and tool-future cancellation, permissions with synchronous and async approval handlers, recoverable tool errors returned as model-visible error tool results, built-in workspace tools including edit diff details, compact background bash stop with Unix shell process-group cleanup, real PTY `terminal` sessions with start/write/read/resize/stop and process-supervisor cleanup, MCP adapter/provider boundary with stdio and HTTP/SSE tool/resource transports, JSONL session reader/writer, local session metadata fork/rename, deterministic JSONL compaction, fake harness. | `pi-agent-core` documents richer event lifecycle, hooks, steering, parallel tool execution, and cancellation. | Document implemented event/tool/MCP/session APIs now; leave hosted/alternate-channel MCP lifecycle, daemonized process reaping beyond tracked groups, and richer hook lifecycle gaps explicit. |
| [`neo-agent`](neo-agent.md) | Clap command surface, project `.neo/config.toml`, user-global `~/.neo/config.toml`, environment overrides including `--mode`/`NEO_MODE`, `--offline`/`NEO_OFFLINE`, and `--verbose`, provider-specific base URL/API key env config, config setters including runtime `AgentConfig` options and `[tui.keybindings]` overrides, self-hosted `neo-cloud` login/auth status/logout, config profile sync, session sync/share/import/remote-resume against a user-run `neo-cloud`, provider-backed print/run with piped stdin merging, project-relative `@file` text prompt expansion, project/user `SYSTEM.md` and `APPEND_SYSTEM.md` system prompt resources plus trust-gated user/project/ancestor `AGENTS.md`/`CLAUDE.md` context files, `--system-prompt` / repeatable `--append-system-prompt` text-or-path overrides, `--thinking <off|minimal|low|medium|high|xhigh>` runtime reasoning override, Pi-style single-invocation tool registry filters `--no-tools`/`-nt`, `--no-builtin-tools`/`-nbt`, `--tools`/`-t`, and `--exclude-tools`/`-xt` across registered built-in, MCP, and extension tool names, project/user slash prompt templates plus explicit prompt-template selectors, default and explicit skill loading, local extension lifecycle/RPC tooling, signed marketplace package search/install/publish for extensions, prompt packs, and themes through `NEO_MARKETPLACE_URL`, prompt/theme package list-preview commands, enabled/default and explicit extension runtime tool discovery through `tools.list`, JSONL-backed RPC local prompt/session methods, models list plus root --list-models [search], MCP server/tool/resource commands, and live TUI local command/session/model pickers plus local `/tree` session picker slash command. | pi coding-agent docs cover settings, providers, sessions, TUI, MCP/resources, and trust. | Document actual project/global config, local system prompt resources, self-hosted cloud session/config sync, local/RPC commands, and the configured marketplace package API; keep managed hosted collaboration, package account flows, publisher/root trust chains, and hosted session backing as explicit gaps. |
| [`neo-tui`](tui.md) | Prompt/editor with undo/kill-ring yank, internal copy buffer plus live OS clipboard prompt and transcript-selection copy, filesystem-backed prompt Tab completion, local slash prompt-template completion, inline `@provider/model` completion metadata, rendered transcript viewport scrolling, item-range transcript selection/highlighting, keybinding override resolution/conflict detection consumed by `neo-agent` config, command palette with project slash prompt-template command insertion, selection/list paging, width-safe rendering primitives, unified diff transcript classification/coloring, safe image-content metadata summaries in transcripts, session transcript loading, streamed thinking notices, bracketed paste buffering, Kitty/Sixel/OSC image protocol encoding primitives, conservative byte-backed inline image rendering under explicit protocol preferences, and a live raw-mode keybinding dispatcher for prompt/approval/command/session-export/model-picker/session-fork/transcript-selection actions. | `pi-tui` has a richer terminal renderer and interaction stack. | Keep TUI docs scoped to implemented primitives and live prompt/overlay actions; runtime detection/negotiation for active terminal image support, hosted/extension command catalogs, and advanced diff affordances remain explicit gaps. |
| [`xtask`](xtask.md) | Stable xtask gate, opt-in workspace gate, docs/examples parity scan, Markdown local-link validation, example TOML/JSON validation, Rust example compile checks through `examples/rust/Cargo.toml`, generated catalog schema validation hook, docs/export auth-token leak scan, generated cloud API schema link scan, package-signature fixture scan, and a `release-smoke` command that starts self-hosted `neo-cloud` before CLI smoke flows. | pi has npm check/supply-chain automation, generated docs metadata, and release smoke tests. | Preserve a small Rust gate, block fake/placeholder production guidance, keep generated catalog checks artifact-backed, keep release smoke aligned with real cloud CLI behavior, and leave generated docs metadata as the remaining automation gap. |

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
  generated catalog pricing and image-generation metadata. Pi `models.json`
  pricing import, upstream generated catalog production, automatic
  provider-metadata migration, OAuth flows, broader provider-native image
  generation modes, and local APIs remain catalog/API gaps.
- MCP is documented as a runtime boundary. `neo-agent-core` now has the
  adapter/provider abstraction, stdio JSON-RPC process adapter, HTTP/SSE
  JSON-RPC adapter, discovery-to-`ToolSpec` bridge, namespaced `ToolRegistry`
  registration, persistent initialized stdio session reuse, explicit
  resources/list and resources/read, stdio and HTTP/SSE resource subscriptions
  backed by real notification streams, queued resource update notifications,
  and async call delegation. `neo-agent print` and `neo-agent run` load enabled
  stdio, HTTP, and SSE MCP entries from project config; `neo mcp tools ...`
  prints the same model-facing tool specs discovered from a configured MCP
  server, and `neo mcp resources ... watch` exposes stdio, live remote SSE
  subscribe streams, same-endpoint HTTP SSE event-channel updates, and JSON
  subscribe ACK alternate event-channel URLs named `eventStreamUrl`,
  `event_stream_url`, or `event_url` without injecting them into model context.
  Hosted MCP management, authorization flows, and provider-specific discovery
  beyond configured endpoints and subscribe ACK URLs remain gaps.
- Session storage is implemented as JSONL event persistence in
  `neo-agent-core`; local tree fork/rename metadata, deterministic extractive
  compaction, schema metadata, local branch summaries, HTML export, sanitized
  local JSON export, and self-hosted `neo-cloud` session sync/share/import are
  wired, while managed collaboration and model-generated/richer compaction
  remain pi-inspired future work.
- Project runtime config now maps generation options, queue modes, tool
  execution mode, permission policy, live interactive approval decisions, and
  compaction thresholds into real `AgentConfig` fields for provider-backed
  runs. Project/global `[tui.keybindings]` entries are validated through
  `neo-tui` action/key parsing, text-insertion reserved-key checks, and
  same-context conflict detection, then applied to the live interactive
  crossterm parser.
- RPC mode now exposes local prompt-template command discovery through
  `get_commands`, real session message replay through `get_messages`, local
  session navigation through `sessions.list` / `sessions.tree`, metadata plus
  replayed message payloads through `sessions.get`, sanitized local HTML
  export through `sessions.export_html`, and sanitized local JSON export
  through `sessions.export_json`; hosted
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
  fork/rename, deterministic extractive compaction, self-hosted cloud
  session/share commands, and current CLI/session reader behavior.
- Examples now include Rust snippets for provider registry, tool schemas,
  runtime turns, and JSONL session replay.
