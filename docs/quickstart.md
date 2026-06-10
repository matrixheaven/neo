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
cargo run -p neo-agent -- --list-models sonnet
cargo run -p neo-agent -- print "hello from neo"
cargo run -p neo-agent -- --thinking high print "solve this carefully"
```

The binary currently exposes `print`, `run`, `resume`, `sessions`, `skills`,
`extensions`, `config`, `models`, and `mcp`. `print` and `run` resolve the
configured model through the production provider registry. The built-in CLI
default is `openai/gpt-4.1`, so a missing `OPENAI_API_KEY` is reported as a
configuration error instead of returning a synthetic response.
Use `--list-models [search]` to print the resolved model catalog and exit
without entering interactive mode; the optional search filters model entries
while retaining provider credential status output.
Use `--verbose` on the interactive root command to force a startup notice block
with the resolved project, session directory, selected model, model scope,
resource toggles, offline state, and project trust state. `--verbose` is a TUI
startup display override; it does not change `print`, `run`, or RPC output.

```bash
cargo run -p neo-agent -- --verbose --models sonnet
```

Use Pi-style per-invocation tool filters to shape the model-facing tool
registry for `print`, `run`, RPC prompts, and live TUI turns:

```bash
cargo run -p neo-agent -- --no-tools print "answer without tools"
cargo run -p neo-agent -- --no-builtin-tools print "use configured MCP tools only"
cargo run -p neo-agent -- --tools read,bash print "inspect with only read and bash"
cargo run -p neo-agent -- --tools read,mcp__docs__search --exclude-tools read print "use docs search"
```

`--tools` is an allowlist across registered built-in and MCP tools, and
`--exclude-tools` removes names after that allowlist. `-nt`, `-nbt`, `-t`, and
`-xt` are accepted as Pi-style aliases for the same filters.

`print` and `run` also merge piped stdin with the CLI prompt:

```bash
printf 'diff context\n' | cargo run -p neo-agent -- print "summarize this"
```

Prompt arguments prefixed with `@` read project-relative text files before the
turn is sent to the provider:

```bash
cargo run -p neo-agent -- print @docs/context.txt "summarize this"
```

Project system instructions can live in `.neo/SYSTEM.md`; Neo sends them as the
provider system message before the user prompt. Use `.neo/APPEND_SYSTEM.md` for
additional instructions that should be appended after the base system prompt.
If the project files are absent, `~/.neo/SYSTEM.md` and
`~/.neo/APPEND_SYSTEM.md` are used as user-global defaults.
For one-off runs, pass literal text or an existing UTF-8 file path directly:

```bash
cargo run -p neo-agent -- --system-prompt .neo/SYSTEM.md --append-system-prompt "Be concise." print "hello"
```

Project context files can live in `AGENTS.md` or `CLAUDE.md` at the project or
an ancestor directory. User-global `~/.neo/AGENTS.md` or `~/.neo/CLAUDE.md`
always load, while project context files require project trust. Manage trust
with:

```bash
cargo run -p neo-agent -- trust status
cargo run -p neo-agent -- trust approve
cargo run -p neo-agent -- trust deny
```

Use `--no-context-files` to disable `AGENTS.md` / `CLAUDE.md` loading for one
invocation.

Project prompt templates live in `.neo/prompts/*.md`. Invoke `review.md` as
`/review`; Neo expands `$1`, `$@`, `$ARGUMENTS`, and simple `${@:N}` slices
before sending the turn:

```bash
mkdir -p .neo/prompts
cat > .neo/prompts/review.md <<'EOF'
---
description: Review a path
argument-hint: "<path> [focus]"
---
Review $1 with focus: ${@:2}
EOF
cargo run -p neo-agent -- print /review src/lib.rs "security pass"
```

User-global templates in `~/.neo/prompts/*.md` are used when the project does
not define the same name. Project templates win on name collisions.

Use `--prompt-template <NAME_OR_PATH>` to force a template without a slash
invocation, or to load a project-relative `.md` file or a non-recursive
directory of `.md` templates:

```bash
cargo run -p neo-agent -- --prompt-template review print src/lib.rs
cargo run -p neo-agent -- --prompt-template prompts print /review src/lib.rs
```

`--no-prompt-templates` disables automatic project/global slash discovery.
Explicit `--prompt-template` entries still load, so the two flags can be
combined to run with exactly the templates you named:

```bash
cargo run -p neo-agent -- --no-prompt-templates print /review src/lib.rs
cargo run -p neo-agent -- --no-prompt-templates --prompt-template prompts print /review src/lib.rs
```

Project or user-global config can also declare prompt template selectors with
the same name/file/directory shape:

```toml
prompt_templates = ["prompts"]
```

Config selectors are merged across `~/.neo/config.toml` and `.neo/config.toml`;
CLI selectors are added for the current invocation.
Use a leading `-` to exclude an auto-discovered local prompt, such as
`prompt_templates = ["-prompts/review.md"]`, while keeping explicit positive
selectors available.

`run --output json` emits a stable typed JSONL event stream with a session
header and Pi-style lifecycle event names. The same stream is selected when
the command runs under top-level `--mode json`:

```bash
cargo run -p neo-agent -- run --output json "summarize this"
cargo run -p neo-agent -- --mode json run "summarize this"
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
cargo run -p neo-agent -- --skill path/to/skill print "use this skill"
cargo run -p neo-agent -- --no-skills --skill path/to/skill print "only this skill"
cargo run -p neo-agent -- extensions install path/to/extension
cargo run -p neo-agent -- extensions install file:///path/to/git-extension
cargo run -p neo-agent -- extensions update echo
cargo run -p neo-agent -- --offline extensions update echo
cargo run -p neo-agent -- extensions list
cargo run -p neo-agent -- extensions status echo
cargo run -p neo-agent -- extensions disable echo
cargo run -p neo-agent -- extensions enable echo
cargo run -p neo-agent -- extensions call echo tool.echo '{"value":42}'
cargo run -p neo-agent -- --extension path/to/extension print "use explicit extension tools"
```

`skills show` uses `neo-sdk` skill loading, `sessions export-html` uses the
safe HTML exporter, default skills are discovered from `~/.neo/skills` and
project `.neo/skills`, explicit `--skill` paths are preserved when `--no-skills`
disables default discovery, extension install/update commands persist local
sources under the project `.neo/extensions-sources.toml`, `--offline` skips
extension update refreshes with an explicit offline message, lifecycle commands
persist local enablement state under the project `.neo/extensions-state.toml`,
and `extensions call` uses the JSONL RPC stdio runner. Provider-backed turns
also discover enabled project `.neo/extensions` tools through each extension's
`tools.list` RPC; `--extension` adds explicit roots and `--no-extensions`
disables only default project extension discovery.

Marketplace package commands require `NEO_MARKETPLACE_URL` and fail closed when
the catalog/server is unavailable. Signed packages use `.neo-package.toml` plus
a sha256-verified, Ed25519-signed tar archive. This is manifest self-sign only,
not a publisher/root trust chain; see [Packages and Marketplace](packages.md)
for the manifest, trust scope, and DTO shape.

```bash
export NEO_MARKETPLACE_URL=http://localhost:8080
cargo run -p neo-agent -- extensions search echo
cargo run -p neo-agent -- extensions install echo@0.1.0 --from marketplace
cargo run -p neo-agent -- extensions publish path/to/.neo-package.toml
cargo run -p neo-agent -- prompts search review
cargo run -p neo-agent -- prompts install review-pack@1.0.0 --from marketplace
cargo run -p neo-agent -- prompts publish path/to/.neo-package.toml
cargo run -p neo-agent -- prompts list
cargo run -p neo-agent -- prompts preview review
cargo run -p neo-agent -- themes search night
cargo run -p neo-agent -- themes install night-owl@2.0.0 --from marketplace
cargo run -p neo-agent -- themes publish path/to/.neo-package.toml
cargo run -p neo-agent -- themes list
cargo run -p neo-agent -- themes preview night-owl
```

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
