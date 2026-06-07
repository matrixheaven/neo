# Quickstart

This guide gets a contributor from a clean checkout to the currently stable Neo
development slice.

## Prerequisites

- Rust toolchain compatible with the workspace `rust-version` in `Cargo.toml`.
- `cargo`, `rustfmt`, and `clippy`.
- `OPENAI_API_KEY` for the default `openai/gpt-4.1` provider-backed CLI turn.

## Install and Inspect

```bash
cargo metadata --no-deps
export OPENAI_API_KEY=...
cargo run -p neo-agent -- print "hello from neo"
```

The binary currently exposes `print`, `run`, `resume`, `sessions`, `skills`,
`extensions`, `config`, `models`, and `mcp`. `print` and `run` resolve the
configured model through the production provider registry. The built-in CLI
default is `openai/gpt-4.1`, so a missing `OPENAI_API_KEY` is reported as a
configuration error instead of returning a synthetic response.

Inspect the current project config view:

```bash
cargo run -p neo-agent -- config show
```

Set project-local provider intent under `.neo/config.toml`:

```bash
cargo run -p neo-agent -- config set default_provider openai
cargo run -p neo-agent -- config set default_model gpt-4.1
cargo run -p neo-agent -- config set permissions.file_read Allow
```

Those provider values are persisted and shown by `config show`.

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

`--docs` also runs the docs/examples parity gate: it scans for production docs
that point at fake or placeholder provider paths, validates local Markdown links,
and parses the example TOML/JSON artifacts.

When all workspace crates are ready for broad verification, opt in explicitly:

```bash
cargo run -p xtask -- check --workspace
```

`--quick` is accepted as a compatibility alias for the default xtask-only gate.

## Example Configs

The files in `examples/config` show the current configuration shapes:

- [minimal.toml](../examples/config/minimal.toml)
- [mcp-server.toml](../examples/config/mcp-server.toml)

`minimal.toml` matches the current `neo-agent` project config loader.
It is a deterministic fixture for config-shape validation, not production
provider guidance.
`mcp-server.toml` documents the intended MCP server shape; the MCP client
adapter is not wired into `neo-agent-core` yet, but `neo mcp list` reads the
same config shape.

## SDK and Extension Commands

```bash
cargo run -p neo-agent -- sessions export-html <session-id> > session.html
cargo run -p neo-agent -- skills show path/to/skill
cargo run -p neo-agent -- extensions list path/to/extensions
cargo run -p neo-agent -- extensions call echo tool.echo '{"value":42}' --root path/to/extensions
```

`skills show` uses `neo-sdk` skill loading, `sessions export-html` uses the
safe HTML exporter, and `extensions call` uses the JSONL RPC stdio runner.

## Rust API Examples

- [provider_registry.rs](../examples/rust/provider_registry.rs) shows
  `neo_ai::ModelRegistry` and `RequestOptions`.
- [tool_schema.rs](../examples/rust/tool_schema.rs) shows
  `ToolSpec::from_schema`.
- [runtime_turn.rs](../examples/rust/runtime_turn.rs) shows the fake harness and
  runtime event stream shape.
- [session_replay.rs](../examples/rust/session_replay.rs) shows JSONL session
  replay.
