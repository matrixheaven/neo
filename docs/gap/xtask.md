# xtask Gap Map

## Implemented Surface

- `cargo run -p xtask -- check` runs the stable xtask-only gate:
  `cargo fmt -p xtask --check`, `cargo clippy -p xtask --all-targets --
  -D warnings`, and `cargo test -p xtask`.
- `cargo run -p xtask -- check --docs` adds local Markdown link validation for
  `README.md`, `docs/**/*.md`, and `examples/**/*.md`.
- `cargo run -p xtask -- check --workspace` opts into full workspace fmt,
  clippy, and tests.
- `--quick` remains an xtask-only compatibility alias.

## Pi Parity Pressure

Pi's repo-level automation includes npm checks, dependency pinning,
shrinkwrap-generation checks, docs metadata, and release smoke tests. Neo should
not inherit those Node-specific gates.

## High-Priority Gaps

- Keep docs link validation in xtask because it is cheap and stable.
- Add generated-docs or example compilation gates only after Neo examples become
  workspace targets.
- Keep the default gate narrow while independent crate workers are making API
  migrations.
