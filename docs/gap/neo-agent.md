# neo-agent Gap Map

## Implemented Surface

- Global flags: `--model`/`NEO_MODEL`, `--provider`/`NEO_PROVIDER`,
  `--api-base`/`NEO_API_BASE`, and `--config`/`NEO_CONFIG`.
- Commands: `print`, `run`, `resume`, `sessions list`, `sessions show`,
  `sessions rename`, `sessions fork`, `sessions export-html`, `skills show`,
  `extensions list`, `extensions status`, `extensions enable`,
  `extensions disable`, `extensions call`, `config show`, `config set`,
  `models list`, and `mcp list`.
- Project config defaults to `.neo/config.toml`.
- Config loading merges CLI overrides, environment overrides, project config,
  and built-in defaults.
- Supported config keys include `default_model`, `default_provider`,
  `api_base`, `api_key_env`, `sessions_dir`, `permissions.file_read`,
  `permissions.file_write`, `permissions.shell`, and `defaults.mode`.
- Session commands read project session files from `sessions_dir`, store local
  tree/name metadata next to JSONL records, and can export replayed messages to
  standalone HTML.
- `skills show` loads TOML-frontmatter skill files through `neo-sdk`.
- `extensions list`, `extensions status`, `extensions enable`, and
  `extensions disable` discover local extension manifests and persist local
  enablement state under `.neo/extensions-state.toml`.
- `extensions call` refuses disabled extensions and round-trips JSONL RPC over
  stdio for enabled local extension manifests.
- `mcp list` reads project MCP server entries without starting servers.
- `print` and `run` discover enabled project MCP servers with
  `transport = "stdio"` and register their tools in the runtime tool registry.
- Interactive mode has a testable controller that accepts prompt text, delegates
  turns to the existing `run_prompt` path or an injected async runner, reduces
  returned `AgentEvent` values through `neo-tui`, and renders no-tty snapshots.

## Pi Parity Pressure

Pi's coding-agent docs include interactive setup, provider login, settings,
TUI controls, session tree navigation UI, hosted sharing, compaction, richer
JSON/RPC modes, extension installation/update flows, themes, terminal setup,
and platform-specific guidance.

## High-Priority Gaps

- Keep quickstart scoped to currently wired commands until interactive mode has
  a live raw-terminal loop; the current fallback snapshot is honest but not a
  crossterm session.
- Document project-local config before user-global config because Neo currently
  resolves `.neo/config.toml` from the current working directory.
- Do not document `/login`, `/tree`, compaction, extension marketplace
  install/update flows, or themes as available Neo features yet.
- Keep `sessions show` and `resume` aligned on `.jsonl` files as session
  persistence evolves.
- Keep MCP runtime config limited to supported stdio servers until HTTP/SSE or
  long-lived session transports are implemented.
