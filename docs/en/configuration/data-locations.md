# Data Storage Locations

Neo keeps all persistent data centralized under `~/.neo/` (or `$NEO_HOME`). Sessions, configuration, skills, prompts, themes, and approval rules are all laid out by convention, making backup, migration, and cleanup easy.

## Neo Home Directory

| Variable | Path | Description |
| --- | --- | --- |
| `NEO_HOME` (environment variable) | User-defined | When set, this directory is used as the neo home first |
| Default | `~/.neo/` | Used when `NEO_HOME` is not set |

Any path relative to `~/.neo/` in the documentation can be replaced with `$NEO_HOME`.

## `~/.neo/` Directory Structure

```
~/.neo/
├── config.toml              # Main configuration file (single source)
├── SYSTEM.md                # Optional replacement for Neo's built-in system prompt
├── APPEND_SYSTEM.md         # Optional instructions appended after the base system prompt
├── AGENTS.md                # User-global instructions (always trusted)
├── approval_rules.json      # Persisted prefix approval rules (Layer 2)
├── trust.json               # Project trust decision records
├── sessions/                # Session root directory
│   └── wd_<slug>_<hash12>/  # One bucket per workspace
│       └── session_<uuid>/  # One directory per session
│           ├── state.json   # Session state (model, timestamps, etc.)
│           ├── workflows/   # Durable workflow runs (V2)
│           │   └── <run_id>/
│           │       ├── run.json
│           │       ├── journal.jsonl
│           │       ├── artifacts/
│           │       └── recovery-quarantine/
│           └── agents/
│               └── main/    # Main agent records
│                   ├── wire.jsonl
│                   ├── plans/
│                   ├── goals/
│                   └── tasks/
├── workflows/               # User-scoped workflow definitions (paired .lua + .workflow.toml)
├── prompts/                 # Global prompt templates
├── skills/                  # Built-in + user skills
├── themes/                  # Theme JSON files (e.g. magenta-dark.json)
└── ...
```

## Session Storage Path

Sessions are bucketed by workspace and split into directories by session id, structured as:

```
<sessions_dir>/wd_<slug>_<hash12>/session_<uuid>/agents/<agent_id>/wire.jsonl
```

| Segment | Generation rule | Example |
| --- | --- | --- |
| `sessions_dir` | `sessions_dir` from `config.toml`, default `~/.neo/sessions` | `~/.neo/sessions` |
| `wd_<slug>_<hash12>` | `wd_` + slug of the workspace directory basename + `_` + the first 12 hex characters of the SHA-256 of its absolute path | `wd_neo_a1b2c3d4e5f6` |
| `session_<uuid>` | A fresh UUID generated per session | `session_4f7c...` |
| `agents/<agent_id>` | Agent id; the main agent is always `main`; Delegate subagents use their own ids | `agents/main` |
| `wire.jsonl` | This agent's event stream (JSON Lines) | — |

> Slug rule: lowercase the basename, replace characters outside `[a-z0-9._-]` with `-`, strip leading/trailing `-`, truncate to 40 characters; fall back to `workspace` when empty. The hash ensures workspaces with the same name but different paths have separate buckets.

Fixed files inside each session directory:

| File / Directory | Description |
| --- | --- |
| `state.json` | Session metadata (schema version, creation time, etc.) |
| `agents/main/wire.jsonl` | Main agent's complete event stream (`neo.session.jsonl` format, schema v1), including durable instruction epochs |
| `agents/main/plans/` | Main agent's plan files |
| `agents/main/goals/` | Main agent's goal files |
| `agents/main/tasks/` | Main agent's background task artifacts |
| `workflows/<run_id>/run.json` | Immutable launch metadata: identity, pinned definition revision/source, args, phases, lineage parent/checkpoint when linked, launch actor/source, machine-limit snapshot |
| `workflows/<run_id>/journal.jsonl` | Append-only state transitions, invocations, schema-repair markers, user requests/answers, artifact commits, final result, actual usage, recovery |
| `workflows/<run_id>/artifacts/` | Content-addressed immutable payload bytes referenced by journal records |
| `workflows/<run_id>/recovery-quarantine/` | Torn-tail journal suffix quarantine (content-hash named); not run state |
| `agents/<agent_id>/...` | Corresponding records for subagents (e.g. produced by Delegate) |

Workflow **runs** live under the session directory, not under transcript or background-task projections. `run.json` is immutable launch metadata. Current state, control, and actual provider usage come only from append-only `journal.jsonl`. Large results may be artifact-referenced. `TaskOutput` pages journal/artifacts via cursors and never loads a complete journal synchronously.

**Definitions** (not runs) are paired files:

| Path | Scope |
| --- | --- |
| Built into Neo | `builtin` (e.g. `code-review`, `deep-research`, `large-refactor`) |
| `$NEO_HOME/workflows/<name>.{lua,workflow.toml}` | user |
| `<trusted-workspace>/.neo/workflows/<name>.{lua,workflow.toml}` | project (trust-gated) |

Precedence is `builtin < user < trusted project`. Historical sessions remain readable; cards without durable `workflows/<run_id>/` files are projections only and cannot be resumed. Prune with `neo workflow prune` (dry-run by default; `--yes` deletes only terminal, unreferenced, unpinned storage). See [Workflows](../guides/workflows.md).

## Other Configuration File Locations

| Path | Source | Description |
| --- | --- | --- |
| `~/.neo/config.toml` | Main config | See [Configuration Files](config-files.md) |
| `~/.neo/SYSTEM.md` | System prompt | Optional replacement for Neo's built-in base system prompt |
| `~/.neo/APPEND_SYSTEM.md` | System prompt append | Optional instructions appended after the base system prompt |
| `~/.neo/AGENTS.md` | User-global instructions | Always trusted; loaded as a session instruction epoch (never mutates the system prompt), see [AGENTS.md](../customization/agents.md#agentsmd) |
| `~/.neo/approval_rules.json` | Prefix approval rules | See [Permission Modes](permissions.md#prefix-level-layer-2) |
| `~/.neo/trust.json` | Project trust | Records whether each workspace is trusted by the user (triggered when inputs like `AGENTS.md` are present); gates project `AGENTS.md` instruction loading |
| `~/.neo/prompts/` | Global prompt templates | Directory returned by `global_prompts_dir()` |
| `~/.neo/skills/` | Skill directory | Plus extra directories declared via `skill_path` / `extra_skill_dirs` in `config.toml` |
| `~/.neo/workflows/` | User workflow definitions | Paired `<name>.lua` + `<name>.workflow.toml`; project copies under trusted `.neo/workflows/` |
| `~/.neo/themes/*.json` | Themes | e.g. `magenta-dark.json`, loaded at TUI startup |

`sessions_dir` supports a custom location (with `~` expansion), letting you place sessions on an external disk or tmpfs:

```toml
# config.toml
sessions_dir = "~/neo-sessions"
```

## Cleanup Guide

### Delete all sessions for a workspace

```
rm -rf ~/.neo/sessions/wd_<slug>_<hash12>/
```

You can run `ls ~/.neo/sessions/` to view all workspace buckets and find the relevant project by its slug.

### Clear all sessions

```shell
rm -rf ~/.neo/sessions/
```

Neo rebuilds them as needed on next startup.

### Reset approval rules

```shell
rm ~/.neo/approval_rules.json
```

Once deleted, all prefix rules are invalidated and Ask mode will re-prompt item by item.

### Full reset

```shell
mv ~/.neo ~/.neo.bak    # Backup
# or
rm -rf ~/.neo           # Wipe entirely
```

> `trust.json` is also stored under the neo home; deleting it loses all "trusted project" decisions, and you will need to reconfirm on next startup.

## Next Steps

- [Workflows](../guides/workflows.md) — authoring, launch, prune
- [Configuration Files](config-files.md) — definitions of fields like `sessions_dir`
- [Permission Modes](permissions.md) — semantics of `approval_rules.json`
- [Provider Configuration](providers.md) — API key and endpoint configuration
