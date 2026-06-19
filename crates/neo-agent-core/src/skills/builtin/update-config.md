---
name: update-config
description: Help the user inspect or edit Kimi Code configuration files (config.toml, tui.toml, trust, etc.).
type: prompt
whenToUse: When the user asks what a setting does or wants to change a configuration value.
disableModelInvocation: false
---

Help the user inspect or edit their Kimi Code configuration.

Supported config files:
- `config.toml` — model, provider, permissions, MCP servers, skill dirs.
- `tui.toml` — theme, editor, notifications, auto-update.
- `trust.json` — project trust decisions.

Before editing:
1. Read the current config file with the user's permission.
2. Explain what the setting does.
3. Propose the exact change and ask for confirmation.

Never write API keys or secrets into examples in docs or config comments.
