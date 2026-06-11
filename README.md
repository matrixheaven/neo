# neo

Rust-native local AI agent monorepo inspired by pi. The workspace contains:

- `neo-ai`: provider abstraction and streaming event normalization
- `neo-agent-core`: agent loop, tools, permissions, sessions, and harness
- `neo-tui`: reusable terminal UI primitives
- `neo-agent`: CLI/TUI application binary
- `neo-sdk`: JSONL RPC, skill loading, and HTML export helpers
- `neo-extensions`: extension discovery and stdio JSONL runner

## Start

```bash
cargo run -p neo-agent -- print "hello from neo"
cargo run -p neo-agent -- models list
cargo run -p xtask -- check --docs
cargo run -p xtask -- release-smoke
```

The default maintenance gate intentionally checks the stable `xtask` slice while
other workers are building adjacent crates. Use
`cargo run -p xtask -- check --workspace` when you want the full workspace fmt,
clippy, and test gate.
`cargo run -p xtask -- check --docs` also runs the docs/examples parity gate,
including local Markdown link checks, production fake/local/placeholder guidance
scans, and example TOML/JSON validation.
`cargo run -p xtask -- release-smoke` is a local-only smoke flow for CLI help,
models, local sessions/export, local extensions, MCP fixtures, catalog, and
docs checks. It does not start cloud services or marketplace fixtures.

## Documentation

- [Docs index](docs/index.md)
- [Quickstart](docs/quickstart.md)
- [Configuration](docs/config.md)
- [Providers](docs/providers.md)
- [Tools](docs/tools.md)
- [Sessions](docs/sessions.md)
- [Pi parity gap map](docs/gap/INDEX.md)
