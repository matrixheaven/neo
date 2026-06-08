# neo-agent Gap Map

## Implemented Surface

- Global flags: `--model`/`NEO_MODEL`, `--provider`/`NEO_PROVIDER`,
  `--api-base`/`NEO_API_BASE`, and `--config`/`NEO_CONFIG`.
- Commands: `print`, `run`, `resume`, `sessions list`, `sessions show`,
  `sessions tree`, `sessions rename`, `sessions fork`, `sessions compact`,
  `sessions export-html`, `skills show`, `extensions list`,
  `extensions install`, `extensions update`, `extensions uninstall`,
  `extensions status`, `extensions enable`, `extensions disable`,
  `extensions call`, `config show`, `config set`,
  `models list`, and `mcp list`.
- Project config defaults to `.neo/config.toml`.
- Config loading merges CLI overrides, environment overrides, project config,
  user-global `~/.neo/config.toml`, and built-in defaults.
- Supported config keys include `default_model`, `default_provider`,
  `api_base`, `api_key_env`, `model_catalogs`, `sessions_dir`,
  `permissions.file_read`, `permissions.file_write`, `permissions.shell`,
  `defaults.mode`,
  provider-specific API base URLs and API key env names, and runtime
  generation/agent options such as temperature, max tokens, queue modes, tool
  execution mode, and compaction thresholds.
- User-global config is merged below project config with `~` expansion for
  paths such as `sessions_dir`.
- Session commands read project session files from `sessions_dir`, store local
  tree/name metadata next to JSONL records, compact sessions with a local
  deterministic transcript summary event, store deterministic local branch
  summaries, render a local parent/child tree, and can export replayed messages
  to standalone HTML.
- `skills show` loads TOML-frontmatter skill files through `neo-sdk`.
- `extensions install` copies a local extension directory or clones an explicit
  git URL into the project `.neo/extensions/<id>`, records its source in the
  project `.neo/extensions-sources.toml`, and `extensions update` refreshes
  from that recorded source without changing enable/disable state.
- `extensions uninstall` removes the installed extension directory and source
  registry entry without mutating explicit enable/disable lifecycle state.
- `extensions list`, `extensions status`, `extensions enable`, and
  `extensions disable` discover project-local extension manifests and persist
  local enablement state under the project `.neo/extensions-state.toml`, even
  when `--config` points at the project from a different invocation directory.
- `extensions call` refuses disabled extensions and round-trips JSONL RPC over
  stdio for enabled local extension manifests.
- `mcp list` reads project MCP server entries without starting servers.
- `mcp resources <server-id> list/read` explicitly fetches configured MCP
  server resource catalogs and content over the same stdio or HTTP/SSE
  transport adapters. `mcp resources <server-id> watch <uri>` subscribes to a
  stdio resource or a remote HTTP/SSE resource backed by a live SSE subscribe
  response, prints real update notifications, and unsubscribes.
- `print` and `run` discover enabled project MCP servers with
  `transport = "stdio"`, `transport = "http"`, or `transport = "sse"` and
  register their tools in the runtime tool registry.
- RPC mode supports `get_state`, `prompt`, JSONL-backed `get_messages`, and
  local `sessions.list` / `sessions.tree` metadata payloads; state reports real
  project/session counts and omits unsupported streaming state.
- Interactive mode has a testable controller and a live crossterm/raw-mode TTY
  loop slice. TTY execution renders `neo-tui`, accepts text input, submits
  prompts through a streaming runtime driver, redraws on terminal resize,
  dispatches real keybinding actions for prompt editing and approval overlays,
  routes approval overlay choices back to pending async runtime approval
  handlers, scrolls the transcript viewport with Up/Down/PageUp/PageDown in
  editing mode, completes prompt file paths from `AppConfig.project_dir` on
  Tab, exits on Esc/Ctrl-C, and keeps the no-tty snapshot fallback for command
  tests and redirected stdout. `ctrl+r` opens a local session picker backed by
  `sessions_dir` metadata and JSONL files; the picker uses local tree ordering
  and indents child sessions. Selecting a session replays its compacted context
  into the TUI, and subsequent prompts use that context while appending new
  events to the selected JSONL session. With the session picker focused,
  `ctrl+n` forks the selected session through
  `SessionMetadataStore::fork()`, loads the child transcript, and routes later
  prompts to the child JSONL session. `ctrl+o` opens a model picker backed by
  the resolved `ModelRegistry`; selecting a model updates the TUI header and
  uses that provider/model for subsequent turns.

## Pi Parity Pressure

Pi's coding-agent docs include interactive setup, provider login, settings,
TUI controls, session tree navigation UI, hosted sharing, compaction, richer
JSON/RPC modes, extension installation/update flows, themes, terminal setup,
and platform-specific guidance.

## High-Priority Gaps

- Keep quickstart scoped to currently wired commands until interactive mode has
  full controls beyond the current raw-terminal prompt/edit/approval/session/model
  slice.
- Keep config docs scoped to project/global TOML layering until profile sync or
  hosted settings exist.
- Do not document `/login`, `/tree`, hosted sharing, hosted extension
  marketplace catalog/search/install flows, or themes as available Neo features
  yet.
- Add hosted session tree navigation/share only when real hosted backing
  behavior exists.
- Keep MCP runtime config limited to tools and explicit resource
  subscription/watch flows until hosted server lifecycle, OAuth/trust flows, or
  remote servers requiring alternate notification channels are implemented.
