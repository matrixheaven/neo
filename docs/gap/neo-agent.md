# neo-agent Gap Map

## Implemented Surface

- Global flags: `--model`/`NEO_MODEL`, `--provider`/`NEO_PROVIDER`,
  `--api-base`/`NEO_API_BASE`, `--config`/`NEO_CONFIG`,
  `--mode`/`NEO_MODE`, `--offline`/`NEO_OFFLINE`, and `--verbose`.
- Commands: `print`, `run`, `resume`, `sessions list`, `sessions show`,
  `sessions tree`, `sessions rename`, `sessions fork`, `sessions compact`,
  `sessions export-html`, `sessions export-json`, `skills show`, `extensions list`,
  `extensions install`, `extensions update`, `extensions uninstall`,
  `extensions status`, `extensions enable`, `extensions disable`,
  `extensions call`, `config show`, `config set`,
  `models list`, `mcp list`, and `mcp tools`.
- Root `--list-models [search]` lists the resolved model catalog with optional
  search filtering, then exits without entering interactive mode.
- Project config defaults to `.neo/config.toml`.
- Config loading merges CLI overrides, environment overrides, project config,
  user-global `~/.neo/config.toml`, and built-in defaults.
- Supported config keys include `default_model`, `default_provider`,
  `api_base`, `api_key_env`, `model_catalogs`, `sessions_dir`,
  `permissions.file_read`, `permissions.file_write`, `permissions.shell`,
  `defaults.mode`,
  provider-specific API base URLs and API key env names, and runtime
  generation/agent options such as temperature, max tokens, queue modes, tool
  execution mode, compaction thresholds, and TUI keybinding overrides under
  `[tui.keybindings]`.
- User-global config is merged below project config with `~` expansion for
  paths such as `sessions_dir`.
- Session commands read project session files from `sessions_dir`, store local
  tree/name metadata next to JSONL records, resolve exact ids, unique prefixes,
  and in-directory JSONL paths, compact sessions with a deterministic transcript
  summary event, store deterministic local branch summaries, render
  a local parent/child tree, and can export replayed messages to standalone
  HTML or a stable local JSON artifact without leaking absolute session paths.
- `skills show` loads TOML-frontmatter skill files through `neo-sdk`.
  Provider-backed turns auto-discover project `.neo/skills` and user-global
  `~/.neo/skills`, load explicit `--skill <PATH>` entries, and preserve
  explicit skill paths when `--no-skills` disables automatic discovery.
- `extensions install` copies a local extension directory or clones an explicit
  git URL into the project `.neo/extensions/<id>`, records its source in the
  project `.neo/extensions-sources.toml`, and `extensions update` refreshes
  from that recorded source without changing enable/disable state.
  `--offline` or truthy `NEO_OFFLINE` skips extension updates with an explicit
  offline message instead of pretending the source was refreshed.
- `extensions uninstall` removes the installed extension directory and source
  registry entry without mutating explicit enable/disable lifecycle state.
- `extensions list`, `extensions status`, `extensions enable`, and
  `extensions disable` discover project-local extension manifests and persist
  local enablement state under the project `.neo/extensions-state.toml`, even
  when `--config` points at the project from a different invocation directory.
- `extensions call` refuses disabled extensions and round-trips JSONL RPC over
  stdio for enabled local extension manifests.
- Provider-backed turns discover enabled project `.neo/extensions` and
  explicit `--extension <PATH>` extension roots, call each extension's
  `tools.list` RPC, and register returned tools as model-facing
  `extension__<extension>__<tool>` functions. `--no-extensions` disables
  automatic project extension discovery while keeping explicit extension paths.
- `mcp list` reads project MCP server entries without starting servers.
- `mcp tools <server-id>` discovers a configured enabled MCP server over its
  real stdio or HTTP/SSE adapter, then prints model-facing tool names,
  descriptions, and compact JSON input schemas.
- `mcp resources <server-id> list/read` explicitly fetches configured MCP
  server resource catalogs and content over the same stdio or HTTP/SSE
  transport adapters. `mcp resources <server-id> watch <uri>` subscribes to a
  stdio resource or a remote HTTP/SSE resource backed by a live SSE subscribe
  response, prints real update notifications, and unsubscribes.
- `print` and `run` merge non-TTY piped stdin with CLI prompt arguments, expand
  project-relative `@file` text prompt arguments, load project `.neo/SYSTEM.md`
  or user-global `~/.neo/SYSTEM.md` into the provider system message, append
  project `.neo/APPEND_SYSTEM.md` or user-global `~/.neo/APPEND_SYSTEM.md`
  after the base system prompt, support `--system-prompt <TEXT_OR_PATH>` plus
  repeatable `--append-system-prompt <TEXT_OR_PATH>` CLI overrides that treat
  existing paths as UTF-8 files and other values as literal text, support
  `--thinking <off|minimal|low|medium|high|xhigh>` as a single-invocation
  override for `runtime.reasoning_effort`, support Pi-style single-invocation
  tool registry filters with `--no-tools`/`-nt`,
  `--no-builtin-tools`/`-nbt`, `--tools`/`-t`, and
  `--exclude-tools`/`-xt` across registered built-in and MCP tool names,
  expand project-local
  `.neo/prompts/*.md` and user-global `~/.neo/prompts/*.md` slash prompt
  templates with project templates taking precedence, merge `prompt_templates`
  selectors from user-global and project TOML config, support repeatable
  explicit `--prompt-template <NAME_OR_PATH>` entries for template names,
  project-contained `.md` files, and non-recursive `.md` directories, fail
  explicit selector collisions with the duplicate template name and both paths,
  support `-selector` prompt-template filters for auto-discovered local prompt
  files without requiring the excluded file to exist or disabling explicitly
  included positive selectors,
  preserve explicit template entries when `--no-prompt-templates` disables
  automatic discovery, expand `$1`, `$@`, `$ARGUMENTS`, and `${@:N}` / `${@:N:L}`
  argument placeholders, then discover enabled project MCP servers with
  `transport = "stdio"`, `transport = "http"`, or `transport = "sse"` and
  register their tools in the runtime tool registry.
- Provider-backed turns load user-global `~/.neo/AGENTS.md`/`CLAUDE.md`
  context files unconditionally and trust-gated project/ancestor
  `AGENTS.md`/`CLAUDE.md` files into a Pi-shaped `<project_context>` system
  prompt section. `--no-context-files` disables these context files without
  disabling `SYSTEM.md` or explicit prompt overrides. Project trust decisions
  are stored under `~/.neo/trust.json` and managed with `neo trust
  status|approve|deny|clear`; `--approve` and `--no-approve` override the
  stored decision for one invocation.
- `run --output json` and top-level `--mode json run ...` emit stable typed
  JSONL with a session header, Pi-style lifecycle event names, and assistant
  thinking start/delta/end content events when the provider streams reasoning
  summaries. The default `run` output remains the internal `AgentEvent` JSONL
  stream for existing scripts, with additive event variants as runtime
  capabilities grow.
- RPC mode supports `get_state`, local prompt-template `get_commands`,
  `prompt`, JSONL-backed `get_messages`, and local `sessions.list`,
  `sessions.tree`, `sessions.get`, `sessions.export_html`,
  `sessions.export_json`, and
  `set_session_name` payloads.
  `get_commands`
  exposes configured, project, and user-global prompt-template slash commands
  with stable command metadata and the same configured > project > user
  selection priority used by runtime slash prompts. Session RPC methods resolve
  exact ids, unique prefixes, and in-directory JSONL paths through the local
  session resolver. `sessions.get` returns the session metadata, child ids,
  JSONL path, and replayed messages; `sessions.export_html` returns the
  resolved session id plus the same standalone sanitized HTML used by
  `sessions export-html`; `sessions.export_json` returns a local-only portable
  JSON artifact with session metadata and replayed messages but no hosted share
  URL or absolute JSONL path; `set_session_name` updates the same local session
  metadata used by `sessions rename` after resolving an existing session.
  State reports real project/session counts and omits unsupported streaming
  state.
- Interactive mode has a testable controller and a live crossterm/raw-mode TTY
  loop slice. TTY execution renders `neo-tui`, accepts text input, submits
  prompts through a streaming runtime driver, redraws on terminal resize,
  dispatches real keybinding actions for prompt editing and approval overlays,
  applies project/global `[tui.keybindings]` overrides to the live crossterm
  parser after validating action IDs, key syntax, text-insertion reserved keys,
  and same-context conflicts,
  routes approval overlay choices back to pending async runtime approval
  handlers, cancels the active runtime token on interruption, drains cooperative
  cancelled message/turn/run barriers before falling back to abort, scrolls the
  transcript viewport with Up/Down/PageUp/PageDown in editing mode, completes
  prompt file paths from `AppConfig.project_dir` and local project slash prompt
  templates from `.neo/prompts/*.md` on Tab, completes inline
  `@provider/model` prefixes from the resolved model catalog, uses exact
  leading `@provider/model` tokens as per-turn model overrides, exits on
  Esc/Ctrl-C, and keeps the no-tty snapshot fallback for command tests and
  redirected stdout. `--verbose` forces a real startup notice block in the TUI
  transcript with project, session directory, selected model, scoped models,
  resource toggles, offline state, and project trust state. `ctrl+r` and the
  exact `/tree` slash command open a local
  session picker backed by
  `sessions_dir` metadata and JSONL files; the picker uses local tree ordering
  and indents child sessions. Selecting a session replays its compacted context
  into the TUI, and subsequent prompts use that context while appending new
  events to the selected JSONL session. With the session picker focused,
  `ctrl+n` forks the selected session through
  `SessionMetadataStore::fork()`, loads the child transcript, and routes later
  prompts to the child JSONL session. `ctrl+o` opens a model picker backed by
  the resolved `ModelRegistry`; selecting a model updates the TUI header and
  uses that provider/model for subsequent turns. `ctrl+p` opens a local
  command palette that executes implemented local actions for sessions, models,
  project `.neo/prompts/*.md` slash prompt-template invocation insertion,
  active-session HTML export to `sessions_dir/<session_id>.html`, prompt copy,
  transcript selection/copy, and prompt submit. Transcript
  item-range selection starts with Ctrl-Space, extends with Shift-Up/Down or
  Shift-PageUp/PageDown, and Ctrl-C writes the selected transcript text to the
  OS clipboard before falling back to prompt copy when no transcript selection
  is active.

## Pi Parity Pressure

Pi's coding-agent docs include interactive setup, provider login, settings,
TUI controls, session tree navigation UI, hosted sharing, compaction, richer
JSON/RPC modes, extension installation/update flows, themes, terminal setup,
and platform-specific guidance.

## High-Priority Gaps

- Keep quickstart scoped to currently wired commands and local interactive
  controls until hosted or profile-synced surfaces exist.
- Keep stable JSONL docs scoped to the current typed event family until the full
  Pi event family is backed by code.
- Add package prompt-template discovery only when Neo has real local package
  infrastructure. The current implemented local resource scope is project/user
  `SYSTEM.md` and `APPEND_SYSTEM.md`, trust-gated user/project/ancestor
  `AGENTS.md`/`CLAUDE.md` context files, project `.neo/prompts`,
  user-global `~/.neo/prompts`, user/project TOML `prompt_templates`
  selectors, default plus explicit local skills, enabled plus explicit local
  extension tool roots, and explicit local name/file/directory selectors plus
  `-selector` filters for auto-discovered local prompts, with collision
  diagnostics for duplicate explicit selector names.
- Keep config docs scoped to project/global TOML layering until profile sync or
  hosted settings exist.
- Do not document `/login`, hosted sharing, hosted extension marketplace
  catalog/search/install flows, or themes as available Neo features yet. Keep
  `/tree` documented only as the local session picker slash command until
  hosted tree/share backing exists.
- Add hosted session tree navigation/share only when real hosted backing
  behavior exists.
- Keep MCP runtime config limited to tools and explicit resource
  subscription/watch flows until hosted server lifecycle, OAuth/trust flows, or
  remote servers requiring alternate notification channels are implemented.
