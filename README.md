# neo

Rust-native agent monorepo inspired by pi. The workspace contains:

- `neo-ai`: provider abstraction and streaming event normalization
- `neo-agent-core`: agent loop, tools, permissions, sessions, and harness
- `neo-tui`: reusable terminal UI primitives
- `neo-agent`: CLI/TUI application binary

## Start

```bash
cargo run -p neo-agent -- print "hello from neo"
cargo run -p xtask -- check --docs
```

The default maintenance gate intentionally checks the stable `xtask` slice while
other workers are building adjacent crates. Use `cargo run -p xtask -- check
--workspace` when you want the full workspace fmt, clippy, and test gate.

## Documentation

- [Docs index](docs/index.md)
- [Quickstart](docs/quickstart.md)
- [Configuration](docs/config.md)
- [Providers](docs/providers.md)
- [Tools](docs/tools.md)
- [Sessions](docs/sessions.md)
- [Pi parity gap map](docs/gap/INDEX.md)
