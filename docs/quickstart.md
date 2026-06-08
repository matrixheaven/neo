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

`print` and `run` also merge piped stdin with the CLI prompt:

```bash
printf 'diff context\n' | cargo run -p neo-agent -- print "summarize this"
```

Prompt arguments prefixed with `@` read project-relative text files before the
turn is sent to the provider:

```bash
cargo run -p neo-agent -- print @docs/context.txt "summarize this"
```

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

`--docs` also runs the docs/examples parity gate: it scans production source,
docs, and examples for fake/local/placeholder production guidance, validates
local Markdown links, parses the example TOML/JSON artifacts, verifies the Rust
example harness, and compiles the Rust examples with Cargo.

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
`mcp-server.toml` documents the CLI config shape that `neo mcp list`,
`neo print`, and `neo run` read. Enabled stdio MCP servers are spawned for
tool discovery, reused for tool calls during that adapter session, and their
provider-safe `mcp__<server>__<tool>` specs are sent to the configured model.

## SDK and Extension Commands

```bash
cargo run -p neo-agent -- sessions export-html <session-id> > session.html
cargo run -p neo-agent -- skills show path/to/skill
cargo run -p neo-agent -- extensions install path/to/extension
cargo run -p neo-agent -- extensions install file:///path/to/git-extension
cargo run -p neo-agent -- extensions update echo
cargo run -p neo-agent -- extensions list
cargo run -p neo-agent -- extensions status echo
cargo run -p neo-agent -- extensions disable echo
cargo run -p neo-agent -- extensions enable echo
cargo run -p neo-agent -- extensions call echo tool.echo '{"value":42}'
```

`skills show` uses `neo-sdk` skill loading, `sessions export-html` uses the
safe HTML exporter, extension install/update commands persist local sources
under the project `.neo/extensions-sources.toml`, lifecycle commands persist
local enablement state under the project `.neo/extensions-state.toml`, and
`extensions call` uses the JSONL RPC stdio runner.

## Rust API Examples

The Rust examples are Cargo targets under
[examples/rust/Cargo.toml](../examples/rust/Cargo.toml), and the docs parity
gate checks that every `.rs` file in `examples/rust` is declared and compiles.

- [provider_registry.rs](../examples/rust/provider_registry.rs) shows
  `neo_ai::ModelRegistry` and `RequestOptions`.
- [tool_schema.rs](../examples/rust/tool_schema.rs) shows
  `ToolSpec::from_schema`.
- [runtime_turn.rs](../examples/rust/runtime_turn.rs) shows the fake harness and
  runtime event stream shape.
- [session_replay.rs](../examples/rust/session_replay.rs) shows JSONL session
  replay.
