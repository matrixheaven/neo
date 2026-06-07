# Neo Examples

Examples in this directory are small reference artifacts for the documented Neo interfaces.

The Rust examples are declared in [rust/Cargo.toml](rust/Cargo.toml). The
docs/examples parity gate checks that every `rust/*.rs` file is declared there
and compiles as a Cargo example target.

- [config/minimal.toml](config/minimal.toml) shows a deterministic development fixture, not production provider guidance.
- [config/mcp-server.toml](config/mcp-server.toml) shows an MCP stdio server entry.
- [tools/read-file-schema.json](tools/read-file-schema.json) shows a compact model-facing tool schema.
- [rust/provider_registry.rs](rust/provider_registry.rs) shows the provider registry and request options.
- [rust/model_catalog.rs](rust/model_catalog.rs) shows loading a strict local JSON model catalog.
- [rust/tool_schema.rs](rust/tool_schema.rs) shows Rust-driven tool schema generation.
- [rust/runtime_turn.rs](rust/runtime_turn.rs) shows the fake harness runtime event flow.
- [rust/session_replay.rs](rust/session_replay.rs) shows JSONL session replay.
