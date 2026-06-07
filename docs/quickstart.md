# Quickstart

This guide gets a contributor from a clean checkout to the currently implemented Neo development slice.

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
hello from neo
```

The interactive agent loop is not implemented yet. The binary currently exposes the command shape for `print`, `run`, `resume`, `sessions`, `config`, `models`, and `mcp`.

## Development Checks

Use the stable maintenance slice while other crates are under active construction:

```bash
cargo run -p xtask -- check
```

That runs:

- `cargo fmt --all --check`
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

## Example Configs

The files in `examples/config` show the intended user-facing configuration format:

- [minimal.toml](../examples/config/minimal.toml)
- [mcp-server.toml](../examples/config/mcp-server.toml)

They are documentation examples. Runtime config loading is still owned by the future `neo-agent-core` configuration module.
