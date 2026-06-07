# neo-agent Gap Map

## Implemented Surface

- Global flags: `--model`/`NEO_MODEL`, `--provider`/`NEO_PROVIDER`,
  `--api-base`/`NEO_API_BASE`, and `--config`/`NEO_CONFIG`.
- Commands: `print`, `run`, `resume`, `sessions list`, `sessions show`,
  `config show`, `config set`, `models list`, and `mcp list`.
- Project config defaults to `.neo/config.toml`.
- Config loading merges CLI overrides, environment overrides, project config,
  and built-in defaults.
- Supported config keys include `default_model`, `default_provider`,
  `api_base`, `api_key_env`, `sessions_dir`, `permissions.file_read`,
  `permissions.file_write`, `permissions.shell`, and `defaults.mode`.
- Session commands read project session files from `sessions_dir`.

## Pi Parity Pressure

Pi's coding-agent docs include interactive setup, provider login, settings,
TUI controls, session tree navigation, compaction, JSON/RPC modes, skills,
extensions, themes, terminal setup, and platform-specific guidance.

## High-Priority Gaps

- Keep quickstart scoped to currently wired commands until interactive mode is
  no longer placeholder-level.
- Document project-local config before user-global config because Neo currently
  resolves `.neo/config.toml` from the current working directory.
- Do not document `/login`, `/resume`, `/tree`, compaction, extensions, or
  themes as available Neo features yet.
- Keep `sessions show` and `resume` aligned on `.jsonl` files as session
  persistence evolves.
