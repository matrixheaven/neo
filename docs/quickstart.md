# Quickstart

This guide gets a contributor from a clean checkout to the currently stable Neo
development slice.

## Prerequisites

- Rust toolchain compatible with the workspace `rust-version` in `Cargo.toml`.
- `cargo`, `rustfmt`, and `clippy`.

## Install and Inspect

```bash
cargo metadata --no-deps
cargo run -p neo-agent -- print "hello from neo"
```

Expected CLI output:

```text
fake response: hello from neo
```

The binary currently exposes `print`, `run`, `resume`, `sessions`, `config`,
`models`, and `mcp`. Provider-backed interactive behavior is still in progress;
use the fake/local model paths and Rust tests for runtime development.

Inspect the current project config view:

```bash
cargo run -p neo-agent -- config show
```

Set project-local defaults under `.neo/config.toml`:

```bash
cargo run -p neo-agent -- config set default_provider fake
cargo run -p neo-agent -- config set default_model fake
cargo run -p neo-agent -- config set permissions.file_read Allow
```

## Development Checks

Use the stable maintenance slice while other crates are under active construction:

```bash
cargo run -p xtask -- check
```

That runs:

- `cargo fmt -p xtask --check`
- `cargo clippy -p xtask --all-targets -- -D warnings`
- `cargo test -p xtask`

To include Markdown local-link validation:

```bash
cargo run -p xtask -- check --docs
```

When all workspace crates are ready for broad verification, opt in explicitly:

```bash
cargo run -p xtask -- check --workspace
```

`--quick` is accepted as a compatibility alias for the default xtask-only gate.

## Example Configs

The files in `examples/config` show the intended user-facing configuration format:

- [minimal.toml](../examples/config/minimal.toml)
- [mcp-server.toml](../examples/config/mcp-server.toml)

`minimal.toml` matches the current `neo-agent` project config loader.
`mcp-server.toml` documents the intended MCP server shape; the MCP client
adapter is not wired into `neo-agent-core` yet.

## Rust API Examples

- [provider_registry.rs](../examples/rust/provider_registry.rs) shows
  `neo_ai::ModelRegistry` and `RequestOptions`.
- [tool_schema.rs](../examples/rust/tool_schema.rs) shows
  `ToolSpec::from_schema`.
- [runtime_turn.rs](../examples/rust/runtime_turn.rs) shows the fake harness and
  runtime event stream shape.
- [session_replay.rs](../examples/rust/session_replay.rs) shows JSONL session
  replay.
