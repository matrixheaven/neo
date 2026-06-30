# Neo Extension Removal Design

## Goal

Remove Neo's extension product surface completely. Neo should no longer discover, install, enable, list, execute, complete, document, or plan around local extensions. MCP remains Neo's external tool integration boundary.

This removes the extension line because the current extension system duplicates MCP's model-tool job with less capability and adds another integration model for future agents to maintain.

## Decisions

- Hard-delete extension-specific product code, tests, and Neo-owned docs.
- Keep generic JSONL RPC infrastructure when it is not extension-specific.
- Keep MCP, skills, prompt templates, themes, workflow, multi-agent, and other local Neo features.
- Do not read, migrate, or clean old user data such as `~/.neo/extensions`, `extensions-state.toml`, or `extensions-sources.toml`.
- Do not document old extension data as a supported product behavior or migration path.
- Do not touch vendored reference directories such as `docs/pi`, `docs/codex`, or `docs/kimi-code`.
- Delete old Neo extension planning docs instead of archiving or marking them superseded.
- Do not add compatibility shims, deprecation warnings, hidden commands, or fallback reads.

## Architecture Boundary

After removal, Neo has one external tool integration path: configured MCP servers. Runtime tool names may include MCP tools such as `mcp__<server>__<tool>`, but must not include `extension__<id>__<tool>`.

The generic RPC layer is not part of the extension product by itself. It may remain if `neo rpc`, JSONL codec tests, or other non-extension surfaces still use it. Extension-specific RPC runners, manifests, lifecycle state, tool adapters, and command dispatch are removed.

## Removal Scope

### CLI

Remove the `neo extensions` command tree entirely:

- `Command::Extensions`
- `ExtensionCommand`
- `dispatch_extensions`
- extension path helpers
- `crates/neo-agent/src/extension_commands.rs`

`neo extensions ...` should fail as an unknown clap subcommand. It should not print a custom deprecation or migration message.

### Runtime Tool Registration

Remove extension tool discovery and registration:

- no `register_enabled_extension_tools`
- no `ExtensionTool`
- no `neo-extension.toml` discovery
- no extension lifecycle state checks
- no model request tool named `extension__...`

The built-in tool registry and MCP manager continue to work normally.

### Interactive Completion

Remove extension command completion from the TUI:

- no `CompletionSource::ExtensionCommand`
- no `CompletionCatalog.extension_commands`
- no extension discovery during completion catalog construction
- no "source: extension command" completion descriptions

Slash completion should continue to cover real remaining sources such as slash prompts, prompt packages, sessions, and static commands.

### Tests

Delete extension-specific tests rather than rewrite them into compatibility tests:

- extension runner tests
- extension lifecycle/command tests
- E2E assertion that `extension__...` appears in model tools

Update focused completion/runtime tests to assert the remaining behavior. If a test needs a negative assertion, it should assert that no model tool starts with `extension__`.

Generic RPC tests may remain, but their fixture method names should be neutral. For example, replace sample method names like `extension.describe` with a non-product name such as `rpc.describe`.

### Docs

Clean Neo-owned documentation and planning references:

- `README.md`
- `AGENTS.md`
- `docs/index.md`
- `docs/packages.md`
- `docs/quickstart.md`
- `docs/architecture.md`
- Neo-owned `docs/superpowers` extension plans, especially the old NEO-35 host-plugin handoff

Remove extension as a documented Neo feature. Do not replace it with an "extensions are deprecated" section.

Vendored reference docs and source under `docs/pi`, `docs/codex`, and `docs/kimi-code` are out of scope and may still contain the word "extension".

## Implementation Order

1. Delete the CLI surface and extension command module.
2. Delete runtime extension tool registration and the `tools/extensions` module.
3. Delete extension completion source and catalog plumbing.
4. Remove or rewrite tests around the new absence of extension behavior.
5. Clean Neo-owned docs and delete old extension planning docs.
6. Run targeted verification and a final residue scan.

This order intentionally lets compiler errors expose remaining code references after each major deletion.

## Verification

Use narrow targets, not broad `cargo test`.

Suggested checks. The implementation plan may adjust names to match the final edited tests, but it must keep the same narrow target shape:

```bash
cargo nextest run -p neo-agent --test cli_commands extensions_subcommand_is_unknown
cargo nextest run -p neo-agent --test mock_provider_e2e run_text_does_not_register_extension_tools_in_model_request
cargo nextest run -p neo-agent --lib completion_catalog_excludes_extension_commands
cargo nextest run -p neo-agent-core --test rpc_jsonl jsonl_codec_encodes_and_decodes_rpc_messages
cargo fmt --all --check
```

Run clippy only for touched crate/target boundaries if the implementation risk justifies it:

```bash
cargo clippy -p neo-agent --lib -- -D warnings
cargo clippy -p neo-agent-core --lib -- -D warnings
```

Final residue scan:

```bash
rg -n "neo extensions|extension__|neo-extension\\.toml|tools::extensions|ExtensionCommand|ExtensionRunner|ExtensionLifecycle" \
  crates README.md AGENTS.md docs/*.md docs/superpowers
```

Expected result: no Neo-owned product/runtime residue. Hits under vendored reference directories are not failures.

## Success Criteria

- Neo no longer has an extension CLI command.
- Neo no longer reads extension directories, manifests, source registries, or lifecycle state.
- Neo no longer registers extension tools into model requests.
- TUI completion no longer exposes extension command sources.
- Neo-owned docs no longer describe extension as a feature or future direction.
- Generic RPC support remains only where independently used by non-extension surfaces.
- Existing local extension files on a user's machine are ignored as inert leftover files.
