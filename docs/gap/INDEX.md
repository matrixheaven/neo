# Neo/Pi Gap Map

This directory tracks documentation and automation parity pressure between pi
and the current Neo Rust workspace. Neo's documented product direction here is
a local-only AI agent, not a hosted collaboration or marketplace product.

## Priority Map

| Module | Current Neo surface | Pi reference pressure | High-priority gap |
| --- | --- | --- | --- |
| [`neo-ai`](neo-ai.md) | Provider-neutral `ChatRequest`, `RequestOptions` with typed reasoning effort, strict local model catalogs with pricing/image-generation metadata, production provider resolver, OpenAI Responses, Anthropic Messages, Google Generative AI, OpenAI-compatible adapters, OpenAI-style image generation, provider-neutral thinking events, and tool schemas. | `pi-ai` documents broader provider discovery, provider-native reasoning, images, and context handoff. | Keep Neo docs focused on implemented Rust contracts; OAuth, provider override/auth APIs, adaptive/off-state reasoning, and model-aware cross-provider thinking conversion remain explicit gaps. |
| [`neo-agent-core`](neo-agent-core.md) | Runtime turn loop, normalized lifecycle/thinking events, cancellation, permissions, recoverable model-visible tool errors, built-in workspace tools, background bash cleanup, PTY terminal sessions, MCP adapters, JSONL session reader/writer, local session metadata fork/rename, deterministic compaction, and fake harness. | `pi-agent-core` documents richer lifecycle, hooks, steering, parallel tool execution, and cancellation. | Document implemented event/tool/MCP/session APIs now; leave hosted lifecycle, daemonized process reaping beyond tracked groups, and richer hook phases as gaps. |
| [`neo-agent`](neo-agent.md) | Local CLI/TUI surface for provider-backed print/run, project/global config, local system prompt resources, trust-gated context files, `--thinking`, local JSONL sessions/export, local skills, local extension lifecycle/RPC tooling, local prompt/theme inspection, models list, image generation, configured MCP commands, RPC local prompt/session methods, and live local session/model/command pickers. | pi coding-agent docs cover settings, providers, sessions, TUI, MCP/resources, trust, packages, and hosted flows. | Keep docs scoped to local operation. Do not present profile sync, hosted share/import, remote resume, hosted MCP registry, marketplace search/install/publish, package publisher/root trust, or managed collaboration as supported local-agent features. |
| [`neo-tui`](tui.md) | Prompt/editor controls, transcript viewport and selection/copy, filesystem and slash-template completion, inline `@provider/model` completion, diff rendering, streamed thinking notices, bracketed paste, Kitty/iTerm2/Sixel encoding primitives, and conservative byte-backed inline image rendering under explicit protocol preferences. | `pi-tui` has a richer terminal renderer and interaction stack. | Preserve explicit protocol behavior: `auto` uses conservative hints and does not claim full runtime negotiation; advanced diff affordances remain lower priority. |
| [`xtask`](xtask.md) | Stable xtask gate, docs/examples parity scan, Markdown local-link validation, example TOML/JSON validation, Rust example compile checks, generated catalog schema validation, docs/export auth-token leak scan, package-signature fixture scan, and local-only `release-smoke`. | pi has npm check/supply-chain automation, generated docs metadata, and release smoke tests. | Keep release-smoke aligned with local surfaces: help, models, local sessions/export, local extensions, MCP fixture, catalog, and docs checks. |

## Cross-Cutting Facts

- Reasoning handoff is implemented through provider-neutral thinking events and
  persisted `ContentPart::Thinking` blocks. OpenAI Responses, Anthropic
  Messages, and Google Generative AI can replay signed or opaque reasoning in
  provider-native shapes where supported. Adaptive/off-state behavior and
  model-aware cross-provider conversion remain gaps.
- Image generation is implemented for OpenAI-style image endpoints through
  local catalog metadata. Base64 provider outputs are written directly; URL-only
  outputs require explicit `tui.fetch_remote_images = true` and still pass
  HTTP(S), image content-type, and size checks.
- MCP is documented as a runtime boundary. Neo supports configured stdio,
  HTTP, and SSE endpoints plus explicit resource list/read/watch flows. Hosted
  MCP registries, OAuth onboarding, and hosted server lifecycle management are
  out of scope for the local-only surface.
- Session storage is local JSONL event persistence with local tree metadata,
  fork/rename, deterministic compaction, HTML export, and sanitized JSON
  export. Hosted share/import, profile sync, and remote resume are not part of
  the supported local session surface.
- RPC mode exposes local prompt-template command discovery, prompt execution,
  local session replay/navigation, and sanitized local HTML/JSON export.

## Docs Updated In This Pass

- Quickstart now presents Neo as a local-only agent and removes cloud,
  profile-sync, share/import, remote-resume, and marketplace flows.
- Configuration docs preserve reasoning, TUI image protocol, and remote image
  fetch policy.
- Providers docs preserve image generation and reasoning handoff contracts.
- Sessions docs now describe local JSONL sessions/export only.
- Packages docs were replaced with local extension/prompt/theme asset guidance
  and explicit unsupported distribution surfaces.
- `xtask release-smoke` now points at local help/models/sessions/extensions/MCP
  plus catalog/docs checks instead of cloud or marketplace fixtures.
