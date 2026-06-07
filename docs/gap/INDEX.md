# Neo/Pi Gap Map

This directory tracks high-priority documentation and automation parity gaps
between the pi package docs under `pi/` and the current Neo Rust workspace.
Each file is module-scoped so code workers can close gaps without rewriting the
whole map.

## Priority Map

| Module | Current Neo surface | Pi reference pressure | High-priority gap |
| --- | --- | --- | --- |
| [`neo-ai`](neo-ai.md) | Provider-neutral `ChatRequest`, `RequestOptions`, `ModelRegistry`, `ProviderRegistry`, `ProviderResolver`, OpenAI Responses, Anthropic Messages, OpenAI-compatible adapters, stream events, tool schemas, test fake provider, and environment key helpers. | `pi-ai` documents broad provider discovery, credential resolution, tool streaming, reasoning, images, and context handoff. | Keep Neo docs focused on implemented Rust contracts and mark unsupported provider APIs/model catalog loading as future work. |
| [`neo-agent-core`](neo-agent-core.md) | Runtime turn loop, `AgentConfig`, `AgentContext`, normalized events, permissions, built-in workspace tools, JSONL session reader/writer, fake harness. | `pi-agent-core` documents richer event lifecycle, hooks, steering, parallel tool execution, and cancellation. | Document implemented event/tool/session APIs now; leave hooks/steering/parallel execution as explicit gaps. |
| [`neo-agent`](neo-agent.md) | Clap command surface, project `.neo/config.toml`, environment overrides, config setters, provider-backed print/run, sessions commands including HTML export, skill loading, extension JSONL RPC calls, models list, and MCP server listing. | pi coding-agent docs cover settings, providers, sessions, TUI, MCP/resources, and trust. | Describe actual project-local config/session/extension commands and mark interactive TUI/trust/lifecycle gaps explicitly. |
| [`neo-tui`](tui.md) | Prompt/editor, transcript viewport, keybinding, selection/list, and width-safe rendering primitives. | `pi-tui` has a richer terminal renderer and interaction stack. | Keep TUI docs scoped to primitives until the full app renderer lands. |
| [`xtask`](xtask.md) | Stable xtask gate, opt-in workspace gate, docs/examples parity scan, Markdown local-link validation, and example TOML/JSON validation. | pi has npm check/supply-chain automation and generated docs metadata. | Preserve a small Rust gate, block fake/placeholder production guidance, and add future checks only when Neo has stable generated artifacts. |

## Cross-Cutting Gaps

- OpenAI Responses, Anthropic Messages, and OpenAI-compatible adapters are wired
  through `ProviderRegistry::production()` and `ProviderResolver`. Google
  Generative AI and local APIs remain catalog/API gaps.
- The `neo-ai::ChatRequest` shape currently uses `options: RequestOptions`.
  Some live `neo-agent-core` code in this checkout still constructs the older
  `temperature`/`max_tokens` fields, so broad workspace builds may fail until
  that code worker finishes the API migration.
- MCP is documented as a runtime boundary. Neo has CLI config/list shape and
  example TOML, but no MCP client adapter is wired into `neo-agent-core` yet.
- Session storage is implemented as JSONL event persistence in
  `neo-agent-core`; local HTML export is wired through `neo-sdk`, while tree
  branching, naming, hosted share, and richer compaction remain pi-inspired
  future work.

## Docs Updated In This Pass

- Quickstart now describes the stable `xtask` docs gate and the opt-in
  workspace gate, including the docs/examples parity scan.
- Configuration docs now reflect `.neo/config.toml`, `NEO_*` overrides, and
  supported `neo config set` keys without treating deterministic development
  fixtures as production defaults.
- Providers docs now reflect `ModelSpec`, `RequestOptions`, environment key
  helpers, production provider resolution, real OpenAI/Anthropic-compatible
  clients, and the fake test provider.
- Tools docs now list the implemented built-in workspace tools and permissions.
- Sessions docs now describe JSONL event persistence and current CLI/session
  reader behavior.
- Examples now include Rust snippets for provider registry, tool schemas,
  runtime turns, and JSONL session replay.
