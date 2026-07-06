# Quickstart

This page walks you from zero to a working install, a configured API key, and your first conversation.

## Prerequisites

| Dependency | Version | Notes |
| --- | --- | --- |
| Rust | 1.96.1+ (stable) | The repo pins the toolchain via `rust-toolchain.toml`; `rustup` installs it automatically |
| `cargo` / `rustfmt` / `clippy` | Bundled with Rust | A standard install is enough |
| API key | At least one provider | e.g. `OPENAI_API_KEY` |

Don't have Rust yet? Install it in one shot with [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Installation

### Option 1: Build from source (recommended)

```bash
git clone https://github.com/matrixheaven/neo.git
cd neo
cargo install --path crates/neo-agent --locked --force
```

`cargo install` builds a release binary and drops it into `~/.cargo/bin/`. Make sure that directory is on your `PATH` (it is by default when you use rustup).

Verify the install:

```bash
neo --version
neo models list        # inspect the resolved model catalog
```

### Option 2: Direct `cargo install`

> Available once Neo is published to crates.io. For now, building from source is recommended to get the latest features.

```bash
cargo install neo-agent --locked
```

## First launch

Run `neo` from any directory to enter the interactive TUI:

```bash
neo
```

The first run generates a default config at `~/.neo/config.toml`. If no provider is configured yet, the TUI prompts you to set one up.

## Configure an API key

Neo reads a single config file at `~/.neo/config.toml` (or `$NEO_HOME/config.toml` when `NEO_HOME` is set). Keys can be supplied in two ways.

### Option 1: Environment variable (recommended)

Keep secrets in your shell environment and reference the variable name from config:

```toml
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"   # variable name only, never the real key
```

```bash
export OPENAI_API_KEY=sk-...
neo
```

### Option 2: Write directly into config.toml

```toml
[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key = "sk-..."                # inline key
```

> Security note: Option 2 writes the key to disk. Use it only if you accept that risk.

### Common provider configs

```toml
# Anthropic
[providers.anthropic]
type = "anthropic_messages"
api_key_env = "ANTHROPIC_API_KEY"

# Google
[providers.google]
type = "google_generative_ai"
api_key_env = "GEMINI_API_KEY"

# OpenAI-compatible endpoint (e.g. Ollama / vLLM)
[providers.local]
type = "openai_response"
base_url = "http://localhost:11434/v1"
```

You can also add a provider via the CLI:

```bash
neo provider add openai \
  --type openai_response \
  --base-url https://api.openai.com/v1 \
  --api-key-env OPENAI_API_KEY
```

Import from the [models.dev](https://models.dev) catalog (auto-fills model metadata):

```bash
neo provider catalog list openai
neo provider catalog add openai --api-key sk-... --default-model gpt-4.1
```

## Your first conversation

### Interactive TUI

```bash
neo                        # enter the interactive UI
> Explain the code structure of the current directory
```

Type a question at the prompt and send. `Enter` submits; `Alt+Enter` or `Ctrl+J` inserts a newline.

### One-shot task (headless)

```bash
neo run "Write a linked-list reversal function in Rust"
```

`neo run` takes a prompt argument and prints the result as an event stream to stdout, which is handy for scripting. Switch the output format with `--output`:

```bash
neo run --output text "Summarize this project's architecture"   # plain text
neo run --output json "List all TODOs"                          # JSON events
```

You can also splice file contents into the prompt with `@filename`:

```bash
neo run "Review this code @src/parser.rs"
```

## Cheat sheet: common operations

| Goal | Command |
| --- | --- |
| Start the interactive TUI | `neo` |
| One-shot prompt | `neo run "<prompt>"` |
| Resume the last session | `neo -c` |
| Open the session picker | `neo -r` |
| List sessions | `neo sessions list` |
| Resume a specific session | `neo resume <session-id>` |
| List configured models | `neo models list` |
| Add a model alias | `neo models add <alias> --provider <p> --model <m>` |
| Set the default model | `neo models set <alias>` |
| List providers | `neo provider list` |
| List MCP servers | `neo mcp list` |
| Trust the current workspace | `neo trust approve` |

### Common launch flags

```bash
neo --auto             # Auto permission mode: auto-approve every tool call
neo --yolo             # YOLO mode: auto-approve tools and plan transitions, but may still ask the user questions
neo --verbose          # Print verbose startup diagnostics
neo --config <path>    # Use a specific config file (overrides ~/.neo/config.toml)
```

## Next steps

- [Interaction mode guide](guides/interaction.md) — Multi-line input, slash commands, permission modes, and approvals
- [Session management](guides/sessions.md) — Resume, fork, compact, and export
- [Goal mode](guides/goals.md) — Let Neo autonomously drive a verifiable objective
