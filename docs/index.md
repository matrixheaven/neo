# Neo Documentation

Neo is a Rust-native local coding-agent workspace. The repository is
intentionally split into small crates so provider adapters, agent runtime
policy, terminal UI, and developer tooling can evolve independently.

## Start Here

- [Quickstart](quickstart.md) - run the current CLI and development checks.
- [Architecture](architecture.md) - crate boundaries and request flow.
- [Configuration](config.md) - current `.neo/config.toml`, environment, and CLI override model.
- [Providers](providers.md) - model/provider abstraction, request options, production resolver, and test client.
- [Tools](tools.md) - implemented built-in tools, schemas, permissions, and runtime boundary.
- [Sessions](sessions.md) - JSONL event persistence and current resume expectations.
- [MCP](mcp.md) - conceptual Model Context Protocol interface.
- [Local Assets](packages.md) - local extensions, prompt templates, themes, and unsupported distribution surfaces.
- [Gap Map](gap/INDEX.md) - module-by-module pi parity map.

## Repository Map

- `crates/neo-ai` owns provider-neutral chat, stream, model, and tool schema types.
- `crates/neo-agent-core` owns runtime loops, tools, permissions, sessions, and MCP adapters.
- `crates/neo-tui` owns reusable terminal UI primitives.
- `crates/neo-agent` is the CLI/TUI binary.
- `xtask` contains repository maintenance commands.
- `docs` and `examples` contain the public operating model for contributors.

## Stability Notes

The project is early-stage and other workers may be changing crate APIs while
docs are updated. Documentation in this directory separates implemented Rust
surface from pi-inspired future behavior. Use the [gap map](gap/INDEX.md) when
deciding whether a missing capability is a docs omission or a code gap.
