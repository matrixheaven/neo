# neo

Rust-native local AI agent monorepo inspired by pi. The workspace contains:

- `neo-ai`: provider abstraction and streaming event normalization
- `neo-agent-core`: agent loop, tools, permissions, sessions, MCP, skill loading, JSONL RPC, and HTML export
- `neo-tui`: reusable terminal UI primitives
- `neo-agent`: CLI/TUI application binary

## Start

```bash
cargo run -p neo-agent -- print "hello from neo"
cargo run -p neo-agent -- models list
cargo fmt --all --check
cargo clippy -p neo-agent --bin neo -- -D warnings
cargo nextest run -p neo-agent --bin neo cli_commands
```

For focused test evidence, use cargo-nextest with one package, one explicit
target selector, and one test-name filter.

## Documentation

- [Docs index](docs/index.md)
- [Quickstart](docs/quickstart.md)
- [Configuration](docs/config.md)
- [Providers](docs/providers.md)
- [Tools](docs/tools.md)
- [Sessions](docs/sessions.md)
- [MCP](docs/mcp.md)
- [Local Assets](docs/packages.md)
