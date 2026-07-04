# Neo

A Rust-native, fully local AI coding assistant.

Neo runs as a CLI/TUI on your own machine — no hosted backend, no account, no telemetry. Bring your own API key and talk to OpenAI, Anthropic, Google, or any OpenAI-compatible endpoint. It ships with built-in tools for reading, editing, grep, glob, bash, plan mode, and goal tracking — all gated by a layered permission system.

| | |
| --- | --- |
| **Local-first** | Sessions, configuration, skills, and trust decisions all live in `~/.neo/`. Nothing leaves the machine except the API calls you explicitly configure |
| **Multi-provider** | OpenAI Responses, Anthropic Messages, Google Generative AI, Ollama, vLLM, and more |
| **Resumable sessions** | Every conversation is a resumable, forkable local JSONL transcript |
| **Cross-platform** | macOS, Linux, Windows |

## Next steps

- [Quickstart](quickstart.md) — Install and run your first conversation in five minutes
- [Guides](guides/interaction.md) — Interaction modes, permissions, and slash commands
