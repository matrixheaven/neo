# Neo Documentation

Neo is a Rust-native local coding-agent workspace. The repository is
intentionally split into small crates so provider adapters, agent runtime
policy, and terminal UI can evolve independently.

## Start Here

- [Quickstart](quickstart.md) - run the current CLI and development checks.
- [Architecture](architecture.md) - crate boundaries and request flow.
- [Configuration](config.md) - single `~/.neo/config.toml`, environment, and CLI override model.
- [Providers](providers.md) - model/provider abstraction, request options, production resolver, and test client.
- [Tools](tools.md) - implemented built-in tools, schemas, permissions, and runtime boundary.
- [Sessions](sessions.md) - JSONL event persistence and current resume expectations.
- [MCP](mcp.md) - Model Context Protocol interface, adapters, and safety rules.
- [Message Queue & Steer](queue-and-steer.md) - queue follow-ups and steer running turns with Ctrl+S.
- [Local Assets](packages.md) - local extensions, prompt templates, themes, and unsupported distribution surfaces.

## Repository Map

- `crates/neo-ai` owns provider-neutral chat, stream, model, and tool schema types.
- `crates/neo-agent-core` owns runtime loops, tools, permissions, sessions, MCP adapters, local extensions, skill loading, JSONL RPC, and HTML export.
- `crates/neo-tui` owns reusable terminal UI primitives.
- `crates/neo-agent` is the CLI/TUI binary.
- `docs` and `examples` contain the public operating model for contributors.

## Stability Notes

The project is early-stage and other workers may be changing crate APIs while
docs are updated. Documentation in this directory separates implemented Rust
surface from future behavior.
