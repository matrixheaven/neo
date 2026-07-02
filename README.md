# Neo

A Rust-native, local-only AI coding agent. Neo runs entirely on your machine as a CLI and TUI — no hosted backend, no account, no telemetry. Bring your own API keys and talk to OpenAI, Anthropic, Google, or any OpenAI-compatible endpoint.

## Features

- **Local-first.** All sessions, config, skills, and trust decisions live under `~/.neo/`. Nothing leaves your machine except the API calls you explicitly configure.
- **Multi-provider.** OpenAI Responses, Anthropic Messages, Google Generative AI, and any OpenAI-compatible endpoint (Ollama, vLLM, etc.).
- **Built-in tools.** Read, write, edit, grep, glob, bash, PTY terminal, todo lists, plan mode, and goal tracking — all gated by a layered permission system.
- **MCP support.** Connect stdio or remote MCP servers; their tools are auto-discovered and namespaced as `mcp__<server>__<tool>`.
- **Sessions.** Every conversation is a resumable, forkable JSONL transcript stored locally by workspace.
- **Skills.** Layered prompt-injection system (project → user → extra → built-in) that activates contextually.
- **Queue & steer.** Queue follow-up prompts while the agent is busy, or inject a steering message at the next breakpoint.
- **Cross-platform.** Works on macOS, Linux, and Windows.

## Prerequisites

- **Rust** 1.88+ (stable channel). The repo pins the toolchain via `rust-toolchain.toml`, so `rustup` handles it automatically.
- **`cargo`**, **`rustfmt`**, and **`clippy`** — all included with a standard Rust installation.
- An API key for at least one provider (e.g. `OPENAI_API_KEY`).

<details>
<summary>Don't have Rust yet?</summary>

Install via [rustup](https://rustup.rs):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

On Windows, download and run `rustup-init.exe` from the same site.
</details>

## Installation

```bash
git clone https://github.com/matrixheaven/neo.git
cd neo
cargo install --path crates/neo-agent --locked --force
```

This compiles the release binary and installs it to `~/.cargo/bin/` automatically. Make sure `~/.cargo/bin` is on your `PATH` (it is by default with rustup).

### Verify the install

```bash
neo --version          # if installed to PATH
neo models list        # inspect the resolved model catalog
```

## Configuration

Neo reads a single config file at `~/.neo/config.toml` (or `$NEO_HOME/config.toml` if `NEO_HOME` is set). A minimal setup:

```toml
default_model = "openai/gpt-4.1"

[providers.openai]
type = "openai_response"
base_url = "https://api.openai.com/v1"
api_key_env = "OPENAI_API_KEY"
```

Set the environment variable and you're ready:

```bash
export OPENAI_API_KEY=sk-...
neo run "explain this codebase"
```

For Anthropic, Google, custom providers, model aliases, MCP servers, and all other options, see the **[Configuration Guide](docs/config.md)**.

## Quick Start

```bash
# One-shot prompt
neo print "write a function that reverses a linked list in Rust"

# Interactive TUI session
neo run

# Resume a previous session
neo sessions list
neo resume <session-id>
```

### Useful flags

```bash
neo --no-tools print "answer without any tools"
neo --thinking high print "reason carefully about this problem"
neo --tools Read,Grep print "only let me read and search"
```

## Documentation

| Topic | Link |
|-------|------|
| Quickstart | [docs/quickstart.md](docs/quickstart.md) |
| Configuration | [docs/config.md](docs/config.md) |
| Architecture | [docs/architecture.md](docs/architecture.md) |
| Providers | [docs/providers.md](docs/providers.md) |
| Built-in Tools | [docs/tools.md](docs/tools.md) |
| Sessions | [docs/sessions.md](docs/sessions.md) |
| MCP | [docs/mcp.md](docs/mcp.md) |
| Skills | [docs/skills.md](docs/skills.md) |
| Goals | [docs/goals.md](docs/goals.md) |
| Queue & Steer | [docs/queue-and-steer.md](docs/queue-and-steer.md) |

---

## Development

### Repo layout

```
crates/
  neo-ai/          Provider-neutral request/stream/error types + HTTP clients
  neo-agent-core/  Agent runtime: tools, permissions, sessions, MCP, skills
  neo-tui/         Terminal UI primitives (crossterm + ratatui)
  neo-agent/       The `neo` binary: CLI parsing, config, TUI entry point
```

### Build & lint

```bash
cargo build -p neo-agent                         # build the binary
cargo fmt --all --check                          # formatting check
cargo clippy -p neo-agent --bin neo -- -D warnings   # lint
```

### Testing

Install [cargo-nextest](https://nexte.st) for the best experience:

```bash
cargo nextest run -p neo-agent --bin neo cli_commands    # binary integration tests
cargo nextest run -p neo-agent-core --lib                # library unit tests
```

For a single known test function, exact `cargo test` is fine:

```bash
cargo test --package neo-agent --bin neo -- modes::task_browser::tests::test_name --exact --nocapture
```

### Code conventions

- `unsafe_code = "forbid"`; `clippy::pedantic` is warned.
- Cross-platform is mandatory — no hardcoded path separators or Unix-only assumptions without `#[cfg]` guards.
- Provider code lives in `crates/neo-ai/src/providers/`.
- Session events are normalized `AgentEvent` values — JSONL never depends on provider wire formats.

## License

MIT
