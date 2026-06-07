# Neo Documentation

Neo is a Rust-native coding-agent workspace. The repository is intentionally split into small crates so provider adapters, agent runtime policy, terminal UI, and developer tooling can evolve independently.

## Start Here

- [Quickstart](quickstart.md) - run the current CLI and development checks.
- [Architecture](architecture.md) - crate boundaries and request flow.
- [Configuration](config.md) - intended config model and precedence.
- [Providers](providers.md) - model/provider abstraction and capability flags.
- [Tools](tools.md) - tool schema conventions and execution boundary.
- [Sessions](sessions.md) - intended durable session and resume model.
- [MCP](mcp.md) - conceptual Model Context Protocol interface.

## Repository Map

- `crates/ai` owns provider-neutral chat, stream, model, and tool schema types.
- `crates/agent-core` is the intended home for runtime loops, tools, permissions, sessions, and MCP adapters.
- `crates/tui` is reserved for reusable terminal UI primitives.
- `crates/neo-agent` is the CLI/TUI binary.
- `xtask` contains repository maintenance commands.
- `docs` and `examples` contain the public operating model for contributors.

## Stability Notes

The project is early-stage. Some crates are skeletal while other workers build adjacent pieces. Documentation in this directory labels planned behavior as intended, and Rust stubs avoid depending on unstable runtime APIs.
