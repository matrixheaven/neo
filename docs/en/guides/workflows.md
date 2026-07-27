# Local Workflow Platform

Neo runs **durable Lua workflows** as first-class local background tasks. A workflow is a reviewed script plus structured metadata: it can fan out children, call ordinary tools, wait for typed user answers, and leave a journaled trail you can inspect, pause, resume, answer, stop, fork, or prune.

This guide covers authoring definitions, launching them, the Lua host API, schemas, machine limits, and operator surfaces. Only landed behavior is described.

## What a workflow is

| Piece | Role |
| --- | --- |
| **Definition** | Paired `<name>.lua` + `<name>.workflow.toml`, or a dynamic model-authored script |
| **Run** | One durable execution under the session: `workflows/<run_id>/` |
| **Journal** | Append-only truth for state, invocations, answers, artifacts, final result, and actual usage |
| **Task ID** | Same as `run_id`; appears in `/tasks`, `TaskOutput`, and CLI |

Workflows are **always background**. Launch approval authorizes orchestration only; every later child or tool effect still follows Ask / Auto / Yolo through the ordinary permission path.

Neo does **not** predict token cost, duration, agent count, or project size to pause or degrade a run. Admission and limits use **actual occupancy and storage**, not forecasts. There is no second script engine (Lua only; Rhai and dual-engine designs are non-goals).

## Definition files (paired)

A file-backed definition is two same-stem regular files:

```text
<name>.lua
<name>.workflow.toml
```

- The filename stem is the canonical lookup name.
- The TOML manifest owns structured metadata; Neo does not execute top-level Lua to discover name/phases/schemas.
- The Lua file is the sandboxed script body.

### Manifest fields

```toml
definition_format_version = 2
name = "my-workflow"          # must match the filename stem
display_name = "My Workflow"
description = "What this run orchestrates"
source_sha256 = "<lowercase hex of exact Lua bytes>"

[[phases]]
id = "plan"
description = "Scope and approach"

[[phases]]
id = "execute"
description = "Do the work"

# Optional input JSON Schema (Draft 2020-12)
[input_schema]
type = "object"
additionalProperties = false
required = ["topic"]
[input_schema.properties.topic]
type = "string"
minLength = 1

# Required final output JSON Schema
[output_schema]
type = "object"
additionalProperties = false
required = ["summary", "ok"]
[output_schema.properties.summary]
type = "string"
[output_schema.properties.ok]
type = "boolean"
```

`source_sha256` must match the exact Lua file bytes. Manifest and source sizes are bounded by `runtime.workflow` (`manifest_bytes`, `lua_source_bytes`).

### Content revision

Each definition has a **content revision**: SHA-256 over a fixed byte framing of the canonical manifest JSON plus the exact Lua source. Path, mtime, and registry scope are **not** hash inputs. A run pins the revision it launched; editing or shadowing a definition never mutates an existing run.

## Registry scopes and trust

Discovery scopes (only these three):

```text
builtin                          # compiled into Neo
$NEO_HOME/workflows              # user definitions
<trusted-workspace>/.neo/workflows   # project definitions
```

**Precedence:** `builtin < user < trusted project`. Same name at a higher scope shadows lower. Two same-name candidates in one scope make the name invalid. Invalid higher-scope content does **not** silently fall back to a lower scope.

Project discovery and project save reuse Neo's existing **workspace trust** (`trust.json`). Untrusted or disabled project discovery yields no project candidates. Symlink/reparse-point definition files and parent escapes are rejected; directory links are not followed.

The human/script `neo workflow save` command validates first and defaults to **no-clobber**; `--force` is required to overwrite a different definition. Builtin scope is not writable. The assistant saves through `Workflow(save)`.

## Assistant-native workflow route

For inline authoring, a new saved definition, or a one-off test/evaluation, the
assistant activates `create-workflow` unless it is already active. A known
saved workflow may be discovered or run directly with
`Workflow(list|show|run_saved)` without activating the authoring skill.
`Workflow` owns every lifecycle action: `list`, `show`, `validate_inline`,
`validate_saved`, `save`, `run_inline`, and `run_saved`.

A one-off evaluation follows this strict path, with no source inspection,
shell/CLI, Cargo, TodoList, or saved-workflow discovery inserted before it:

```text
Skill(create-workflow) -> Workflow(validate_inline) -> Workflow(run_inline) -> TaskOutput
```

Create-and-test requests instead use `Workflow(save) -> Workflow(run_saved) ->
TaskOutput`. Run actions return a task ID. These routes need no slash command,
capability, manual manifest/hash work, or `neo workflow` CLI invocation.

Every `TaskOutput` view exposes an actionable `pending_user` object while a
workflow waits: `request_id`, `prompt`, `answer_schema`, optional `default`,
`answer_policy`, and `next_action`. Only when `next_action` is `TaskAnswer`
does the assistant call `TaskAnswer(task_id, request_id, answer)` with those
exact IDs. `wait_for_human` means the user must answer in the TUI or human CLI.

## Human launch and operator surfaces

### Slash: named (host-direct)

```text
/workflow <name> [JSON_OBJECT]
```

- Resolves the name through the effective registry.
- Validates args against the optional `input_schema`.
- Launches **directly on the host** — **zero model round-trips**.
- In Ask mode, shows a launch review (Launch / Revise / Cancel). Auto / Yolo still require the explicit slash action but do not add a second approval dialog beyond ordinary child permissions.

### Slash: bare (manual skill activation)

```text
/workflow
```

Activates `create-workflow` through the normal manual-skill path and begins a
normal model turn. The skill routes the assistant through `Workflow`; it grants
no capability.

Exact slash parsing only: `/workflowish` and prose containing `/workflow` do
not activate the skill.

### Headless CLI (humans and scripts only)

```text
neo workflow list [--scope builtin|user|project|effective] [--output text|json]
neo workflow show <name> [--scope ...] [--output text|json]
neo workflow check <name-or-path> [--output text|json]
neo workflow test <name-or-path> --case <fixture> [--output text|json]
neo workflow run <name> [--args-json <object> | --args-file <path>]
                  [--detach] [--output text|json|jsonl]
neo workflow save <run-id-or-path> --scope user|project [--name <name>] [--force]
neo workflow answer <run-id-or-handle> <request-id> (--json <value> | --file <path>)
neo workflow fork <run-id-or-handle> --checkpoint <seq>
                  [--name <name>] [--args-json <object> | --args-file <path>]
neo workflow prune [--older-than <duration>] [--max-bytes <bytes>] [--dry-run] [--yes]
```

Rules:

- `list` / `show` / `check` / `test` are read-only.
- `run` waits for a terminal state by default; `--detach` returns after durable create.
- `--args-json` and `--args-file` are mutually exclusive.
- `prune` defaults to **dry-run**; deletion requires `--yes` and only considers terminal, unreferenced, unpinned storage.

These commands document human and script operation. They are not an assistant
workflow path.

## Lua host API

The sandbox is **mlua only**. No filesystem, process, network, package, debug, time, random, or environment standard libraries. Arguments (`neo.args`) are recursively read-only.

| API | Purpose |
| --- | --- |
| `neo.args` | Read-only launch arguments object |
| `neo.phase(id)` | Select a declared phase (journaled) |
| `neo.log(message)` | Bounded progress log |
| `neo.delegate(input)` | One child agent; **`output_schema` required** |
| `neo.swarm(input)` | Direct child-spec batch; **per-item `output_schema` required**, including uniform fan-out |
| `neo.tool({ name, input })` | Eligible tools via canonical `ToolRegistry` |
| `neo.await_user(input)` | Durable typed user input (see below) |
| `neo.verify(condition, message)` | Local assertion |
| `neo.verify_command({ command, cwd?, failure_message? })` | Shell verification via Bash |
| `neo.report(value)` | Intermediate report (not a final-result fallback) |
| `neo.fail(message)` | Explicit terminal failure |
| `neo.json_array(table)` | Mark a table as a JSON array (including empty) |
| `neo.json_object(table)` | Mark a table as a JSON object (including empty) |

There is no `neo.parallel`, recursive workflow launch, detached workflow task, raw shell escape, or engine-selection API.

### Effect outcomes

Effectful calls return one immutable table shape:

```text
ok, status, summary, details?, actual_usage?, references?, schema?
```

`status` is one of: `completed` | `failed` | `denied` | `cancelled` | `resource_limited` | `interrupted` | `schema_invalid`.

### Final result

The top-level Lua return (at most one value) is the **only** final result. Zero returns / single `nil` become JSON `null`. Mixed-key or sparse tables fail conversion. `neo.report` values never substitute for the final result.

### `neo.delegate` / `neo.swarm`

Child input (new child) includes:

```text
task (required), title?, role?, model?, provider?, context?, worktree?,
tool_allow?, output_schema (required JSON Schema)
```

On success, the schema-valid child JSON is returned at
`outcome.details.structured_output`.

Direct swarm shape:

```lua
neo.swarm({
  description = "review each subsystem",
  items = {
    { task = "review runtime", role = "reviewer", output_schema = runtime_schema },
    { task = "review persistence", role = "reviewer", output_schema = persistence_schema },
  },
})
```

Each item is a complete child spec. Workflow Lua does not accept the separate
model-facing `DelegateSwarm` shorthand: no `prompt_template`, `title`/`value`
items, `resume_agent_ids`, or top-level `output_schema`.

JSON markers are immutable. Build and mutate an ordinary Lua table first, then
call `neo.json_array(table)` or `neo.json_object(table)` only after collection
is complete; do not mutate a marked table.

Resumed children accept only `resume`, `task`, and `output_schema`. Worktree defaults to `shared` for a new child; `isolated` is explicit. There is **no** model/script field for `max_concurrency`, token budget, agent budget, or wall-clock timeout. Host `runtime.workflow.swarm_concurrency` supplies default swarm concurrency (not a total child-count cap). There is no hard-coded `MAX_SWARM_CHILDREN`; real byte, memory, journal, and admission limits apply.

### Exactly one schema repair

For each child structured output:

1. Run the child through the canonical agent runtime.
2. Parse **one** strict JSON value (no fence stripping, no fuzzy extraction).
3. On invalid output: journal `SchemaRepairStarted`, continue the **same** child session with tools **disabled**, request only replacement JSON.
4. One automatic correction turn only; then success or terminal `schema_invalid`.

Tool calls during repair fail with `schema_repair_tool_forbidden`. Uncertain external effects are never auto-retried. Final-result schema failures (when a final schema is attached) do not start a model repair turn.

### `neo.tool` deny set

Registered tools are eligible by default except a centralized deny set for orchestration/control surfaces, including (among others): `Workflow`, `Delegate`, `DelegateSwarm`, `TaskPause` / `TaskResume` / `TaskStop` / `TaskAnswer`, plan/goal tools, and multi-agent control tools. Dedicated workflow APIs own children, user input, and control. `TaskOutput` targeting the **current** run is rejected to avoid recursive lock/path re-entry. Shell admission stays pending with no implicit timeout.

### `neo.await_user` (non-secrets)

```text
prompt (required), answer_schema (required), default?, title?,
answer_policy?  # human | human_or_model; default human
```

**Do not request credentials, API keys, or other secrets.** Answers are persisted in the local journal and are inspectable after restart. The run enters durable `awaiting_user`, releases active VM/worker admission, and remains visible in `/tasks` and CLI. The assistant calls `TaskAnswer` only for `human_or_model`; human-only requests are answered by the user in the TUI or through the human CLI.

`TaskResume` without an answer **cannot** clear `awaiting_user`.

## Machine limits and admission

Configured under `[runtime.workflow]` in `~/.neo/config.toml`. Scripts, model tool inputs, and definitions **cannot** set or raise these values. Rejected keys include predictive fields such as `token_cap`, `max_concurrency` (as a workflow model limit), and `projected_usage`.

Limits cover source/manifest bytes, Lua VM memory and instruction hooks, journal and artifact sizes, global storage, TaskOutput page size, active VMs/workers/executors, and default swarm concurrency. See [Config Files](../configuration/config-files.md#runtimeworkflow-sub-table).

**Global admission** tracks **actual occupancy** (active VMs, workers, executors, storage). When a permit is unavailable the run stays durable and **queued**; `/tasks` and `TaskOutput` can show the wait reason. No wall-clock workflow timeout is inferred. Pause and stop remain available.

## Artifacts and storage layout

Each V2 run directory:

```text
<session_dir>/workflows/<run_id>/
  run.json                 # immutable launch metadata
  journal.jsonl            # append-only state and invocations
  artifacts/               # content-addressed immutable bytes
  recovery-quarantine/     # torn-tail quarantine only
```

Large final results, reports, and raw schema-attempt output may be stored as artifact references. Reads revalidate size/digest. Default retention is non-destructive: terminal runs remain until explicit prune.

## Linked runs and fork

Terminal runs are immutable. Retry, definition change, argument change, raised machine limits, or an earlier checkpoint requires a **new linked run**:

```text
neo workflow fork <run> --checkpoint <seq> [--args-json ...]
```

The child imports a verified completed-invocation prefix as lineage seed. Replay must match the seed before any new external effect; mismatch fails closed as `lineage_mismatch`. Inherited usage is display-only and is not charged to the new run's actual usage.

V1 runs without durable V2 files remain readable as historical projections and cannot be resumed as live workflows.

## TaskOutput cursors

For workflow tasks, `TaskOutput` never loads the complete journal. Supported views:

| View | Content |
| --- | --- |
| `summary` (default) | Bounded status, phase, usage, pending user request, result/artifact refs, next cursors |
| `journal` | Ascending contiguous journal page |
| `result` | Canonical final result projection |
| `artifacts` | Bounded artifact metadata pages |
| `artifact_content` | Byte-range content by artifact id |

Each non-summary view accepts a stable **cursor** bound to run, view, and query hash. Wrong cursors are rejected. Responses report `has_more` / `next_cursor` / returned byte counts. Records are never silently mid-cut.

## `/tasks` dashboard

`/tasks` is extended for workflows: filterable list, phase/progress, queue/admission reason, awaiting-input state, actual usage, and detail actions when valid (pause, resume, answer, stop, fork). It remains a projection over background tasks and workflow snapshots — not a second state owner. Delegate / Bash / Terminal card layouts are unchanged.

## Built-in workflows

Shipped ordinary definitions (same public Lua APIs, no privileged host functions):

| Name | Intent |
| --- | --- |
| `code-review` | Read-only multi-domain review; never modifies code |
| `deep-research` | Structured multi-step research |
| `large-refactor` | Phased refactor orchestration |

The assistant uses `Workflow(list)`, `Workflow(show)`, and `Workflow(run_saved)`. Humans may use the named slash launch or the headless CLI described above.

## Author checklist

### Assistant route

1. Activate `create-workflow`, then author through `Workflow(validate_inline)`.
2. Persist only through `Workflow(save)` and run through `Workflow(run_inline)` or `Workflow(run_saved)`.
3. Inspect every launched task with `TaskOutput`.
4. Use `TaskAnswer` only for a `human_or_model` gate; leave human-only answers to the user.
5. Never ask the user for a bare slash, invoke `neo workflow`, or hand-author a manifest/hash.

### Human/script file authoring

1. Pair `.lua` + `.workflow.toml` with matching stem and `source_sha256`.
2. Declare ordered `phases` and a required final `output_schema`.
3. Give every `neo.delegate` / `neo.swarm` child an `output_schema`.
4. Never request secrets through `neo.await_user`.
5. Validate with `neo workflow check` before save; use `neo workflow test --case` for fixture harness cases.
6. Use named `/workflow <name>` for a host-direct interactive launch, or the headless CLI for scripted operation.
7. Inspect with `TaskOutput` views/cursors; prune only after dry-run preview.

## Next steps

- [Built-in Tools](../reference/tools.md) — `Workflow`, `TaskAnswer`, `TaskOutput`, pause/resume/stop
- [Slash Commands](../reference/slash-commands.md) — `/workflow`, `/tasks`
- [Config Files](../configuration/config-files.md) — `[runtime.workflow]`
- [Data Locations](../configuration/data-locations.md) — run layout under the session
