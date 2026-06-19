# Quickstart

This guide gets Neo running as a local-only Rust agent from a clean checkout.

## Prerequisites

- Rust toolchain compatible with the workspace `rust-version` in `Cargo.toml`.
- `cargo`, `rustfmt`, and `clippy`.
- A provider API key for the model you configure, such as `OPENAI_API_KEY` for
  the built-in `openai/gpt-4.1` default.

## Run Neo

```bash
cargo metadata --no-deps
export OPENAI_API_KEY=...
cargo run -p neo-agent -- models list
cargo run -p neo-agent -- print "hello from neo"
cargo run -p neo-agent -- --thinking high print "solve this carefully"
```

The binary exposes local CLI/TUI surfaces for provider-backed `print` / `run`,
local JSONL sessions, local skills, local extensions, project config, model
catalogs, image generation, trust decisions, and configured MCP servers. It
does not require or start a hosted service for normal operation.

Use `--list-models [search]` or `models list --json` to inspect the resolved
catalog without entering interactive mode:

```bash
cargo run -p neo-agent -- --list-models gpt
cargo run -p neo-agent -- models list --json
```

Use Pi-style per-invocation tool filters to shape the model-facing tool
registry for `print`, `run`, RPC prompts, and live TUI turns:

```bash
cargo run -p neo-agent -- --no-tools print "answer without tools"
cargo run -p neo-agent -- --no-builtin-tools print "use configured MCP tools only"
cargo run -p neo-agent -- --tools Read,Bash print "inspect with only Read and Bash"
cargo run -p neo-agent -- --tools Read,mcp__docs__search --exclude-tools Read print "use docs search"
```

`print` and `run` merge piped stdin with the CLI prompt, and prompt arguments
prefixed with `@` read project-relative text files before the turn is sent:

```bash
printf 'diff context\n' | cargo run -p neo-agent -- print "summarize this"
cargo run -p neo-agent -- print @docs/context.txt "summarize this"
```

## Local Project Inputs

Project system instructions can live in `.neo/SYSTEM.md`; Neo sends them as
the provider system message before the user prompt. `.neo/APPEND_SYSTEM.md`
adds follow-up instructions. User-global fallbacks live under `~/.neo/`.

Project context files can live in `AGENTS.md` or `CLAUDE.md` at the project or
an ancestor directory. User-global context files always load; project context
files require project trust:

```bash
cargo run -p neo-agent -- trust status
cargo run -p neo-agent -- trust approve
cargo run -p neo-agent -- trust deny
```

Project prompt templates live in `.neo/prompts/*.md` and are invoked by slash
name:

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

## Sessions

Neo sessions are local JSONL files plus `sessions.metadata.json` next to them:

```bash
cargo run -p neo-agent -- sessions list
cargo run -p neo-agent -- sessions tree
cargo run -p neo-agent -- sessions show <session-id>
cargo run -p neo-agent -- sessions fork <session-id> --name "branch"
cargo run -p neo-agent -- sessions export-json <session-id>
cargo run -p neo-agent -- resume <session-id>
```

Live TUI mode opens the local session picker with `ctrl+r` or `/resume`, and can
fork the selected local session with `ctrl+n`.

## Local Extensions And Skills

```bash
cargo run -p neo-agent -- skills show path/to/skill
cargo run -p neo-agent -- --skill path/to/skill print "use this skill"
cargo run -p neo-agent -- extensions install path/to/extension
cargo run -p neo-agent -- extensions update echo
cargo run -p neo-agent -- --offline extensions update echo
cargo run -p neo-agent -- extensions list
cargo run -p neo-agent -- extensions status echo
cargo run -p neo-agent -- extensions disable echo
cargo run -p neo-agent -- extensions enable echo
cargo run -p neo-agent -- extensions call echo tool.echo '{"value":42}'
```

Default skills are discovered from `~/.neo/skills` and project `.neo/skills`.
Extensions install from local directories or explicit git/file URLs into the
project `.neo/extensions` tree, persist local enablement state, and expose
enabled tools through each extension's JSONL RPC `tools.list`.

## MCP

Configure MCP servers in `.neo/config.toml`, or manage them from the CLI:

```bash
# list configured MCP servers and their advertised tools
cargo run -p neo-agent -- mcp list

# add a local stdio MCP server
cargo run -p neo-agent -- mcp add filesystem -t studio \
  -C "npx -y @modelcontextprotocol/server-filesystem ." --cwd .

# add a remote HTTP or SSE MCP server
cargo run -p neo-agent -- mcp add remote-docs -t remote-http \
  --url https://mcp.example.test/rpc \
  --header authorization="Bearer $TOKEN"

# enable, disable, or delete an entry
cargo run -p neo-agent -- mcp disable remote-docs
cargo run -p neo-agent -- mcp enable remote-docs
cargo run -p neo-agent -- mcp del remote-docs
```

Enabled `studio` (stdio), `remote-http`, and `remote-sse` entries are discovered
for provider-backed turns and exposed as `mcp__<server>__<tool>` functions.

## Image Generation

`images generate` uses OpenAI-style image endpoints for models that advertise
image-generation support in the local model catalog:

```bash
cargo run -p neo-agent -- images generate "a compact terminal workstation" \
  --model openai/gpt-image-1 \
  --output .neo/generated/workstation.png
```

Base64 provider image data is written directly. If a provider returns only a
remote image URL, Neo refuses to fetch it unless project config explicitly sets
`tui.fetch_remote_images = true`; remote fetches must use HTTP(S), return an
image content type, and stay under the configured size guard.

## TUI Images

The TUI can render byte-backed transcript images inline with
`tui.image_protocol = "kitty"`, `"iterm2"`, `"sixel"`, or `"none"`.
`"auto"` uses conservative local terminal hints and falls back instead of
claiming full runtime protocol negotiation. Remote transcript image fetching is
controlled separately by `tui.fetch_remote_images`.

## Development Checks

Use the stable maintenance slice while other crates are under active
construction:

```bash
cargo run -p xtask -- check
cargo run -p xtask -- check --docs
cargo run -p xtask -- release-smoke
```

`check --docs` runs Markdown link validation, docs/examples parity checks,
example TOML/JSON validation, generated catalog schema checks, and Rust example
compilation. `release-smoke` is local-only: it does not start cloud services or
marketplace fixtures.
