# xtask Gap Map

## Implemented Surface

- `cargo run -p xtask -- check` runs the stable xtask-only gate:
  `cargo fmt -p xtask --check`, `cargo clippy -p xtask --all-targets --
  -D warnings`, and `cargo test -p xtask`.
- `cargo run -p xtask -- check --docs` runs the docs/examples parity gate:
  local Markdown link validation for `README.md`, `docs/**/*.md`, and
  `examples/**/*.md`; production fake/local/placeholder guidance scans; and
  stale gap-claim scans for implemented MCP/session/extension surfaces; TOML,
  JSON, and Rust example harness validation for the documented example
  artifacts; and `cargo check --manifest-path examples/rust/Cargo.toml
  --examples`.
- `cargo run -p xtask -- parity` runs the docs/examples parity gate without the
  fmt, clippy, and test steps.
- `cargo run -p xtask -- check --workspace` opts into full workspace fmt,
  clippy, and tests.
- `--quick` remains an xtask-only compatibility alias.

## Parity Scan Allowlists

Intentional fixture lines in `examples/**`, `**/tests/**`, or explicit source
fixture modules must be preceded by an inline comment:

```text
# xtask-parity: allow fake-provider-example - deterministic development fixture.
```

Keep the reason specific. This hook is ignored in production source and should
not be used for production or deployment guidance.

The stale gap-claim scan is intentionally symbol-driven. It rejects statements
such as "no MCP adapter is wired" once `McpToolAdapter`/`McpToolProvider`
exist, "extension lifecycle unavailable" once status/enable/disable commands
exist, and "session branching and naming are future work" once
`SessionMetadataStore::fork`/`rename` exist. It still allows honest gaps for
hosted MCP management, remote MCP servers that require alternate notification
channels, hosted share, OAuth login, raw TUI loops, and other surfaces that are
not implemented.

## Pi Parity Pressure

Pi's repo-level automation includes npm checks, dependency pinning,
shrinkwrap-generation checks, docs metadata, and release smoke tests. Neo should
not inherit those Node-specific gates.

## High-Priority Gaps

- Keep the deployment-fixture guidance scan narrow enough that honest "not
  implemented" gap language and provider-rejection documentation remain allowed.
- Add generated-docs checks only after Neo has stable generated documentation
  artifacts.
- Keep the default gate narrow while independent crate workers are making API
  migrations.
