# Local Workflow Platform

Neo runs **durable Lua workflows** as first-class local background tasks. A workflow is a reviewed script plus structured metadata: it can fan out children, call ordinary tools, wait for typed user answers, and leave a journaled trail you can view, pause, resume, or stop.

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

# Optional input JSON Schema (Draft 2020-12) for stored paired definitions
# only: omitting it means this saved definition accepts no arguments. Inline
# Workflow(validate_inline), Workflow(save), and Workflow(run_inline) always
# require an explicit input_schema; a no-argument inline workflow uses
# {"type":"object","additionalProperties":false}.
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

The assistant saves through `Workflow(save)`. Builtin scope is not writable.

## Assistant-native workflow route

For inline authoring, a new saved definition, or a one-off test/evaluation, the
assistant may activate `create-workflow` when authoring guidance is useful. A known
saved workflow may be discovered or run directly with
`Workflow(list|show|run_saved)` without activating the authoring skill.
`Workflow` owns every lifecycle action: `list`, `show`, `validate_inline`,
`validate_saved`, `save`, `run_inline`, and `run_saved`.

A one-off evaluation can launch directly through `Workflow(run_inline)` after
the definition is authored. If the user asks for a check-only result, use
`Workflow(validate_inline)` first; this is optional and creates no task. No
source analysis, shell/CLI, Cargo, TodoList, or saved-workflow discovery is
needed for the normal product path:

```text
Skill(create-workflow) -> Workflow(run_inline)
```

Create-and-test requests instead use `Workflow(save) -> Workflow(run_saved)`.
Run actions return a task ID and continue under the workflow runtime. `TaskOutput`
is the workflow task's only reading and waiting entry point: use that task ID for
status, bounded result or journal pages, artifact content, or pending input.
`WaitDelegate` is only for delegate and swarm IDs, never workflow task IDs.
These routes need no slash command,
capability, manual manifest/hash work, or `neo workflow` CLI invocation.

Every `TaskOutput` view exposes an actionable `pending_user` object while a
workflow waits: `request_id`, `prompt`, `answer_schema`, optional `default`,
`answer_policy`, and `next_action`. Only when `next_action` is `TaskAnswer`
does the assistant call `TaskAnswer(task_id, request_id, answer)` with those
exact IDs. `wait_for_human` means the user must answer in the TUI or human CLI.

## Manual workflow entries

```text
/workflow
/workflow <natural-language task>
/workflow:<name> <natural-language task>
/skill:create-workflow <authoring request>
```

`/workflow` opens a searchable picker. Choosing a row only fills
`/workflow:<name> ` in the composer. The automatic and named forms each start
one visible model turn and preserve the exact slash input in the transcript.
The automatic form receives the complete effective catalog; the named form
receives the selected definition and full input schema. Neither form accepts
workflow argument JSON or launches directly from the host.

Use `/skill:create-workflow` for creation, change, adaptation, or confirmed
one-off authoring. `/workflowish` and prose containing `/workflow` remain
ordinary prompts. Existing permissions, workflow cards, task controls, and
headless CLI behavior are unchanged after model selection.

### Headless CLI (humans and scripts only)

```text
neo workflow list [--output text|json]
neo workflow run <name> [--args <object> | --args-file <path>]
                  [--output text|json|jsonl]
neo workflow check <name-or-path> [--json]
neo workflow test <name-or-path> --case <fixture> [--json]
```

Rules:

- `list`, `check`, and `test` are read-only.
- `run` waits for a terminal state.
- `--args` and `--args-file` are mutually exclusive.

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
| `neo.tool({ name, input })` | Eligible tools via canonical `ToolRegistry`; only `{ name, input }` is accepted. A call-shape decode error aborts the host operation; an executed tool failure returns `ok = false`. |
| `neo.await_user(input)` | Durable typed user input; returns the raw read-only answer value (see below) |
| `neo.verify(condition, message)` | Returns an immutable outcome; check `outcome.ok` directly |
| `neo.verify_command({ command, cwd?, failure_message? })` | Runs through Bash and returns an outcome for both success and ordinary failure |
| `neo.report(value)` | Intermediate report; returns no value — statement only |
| `neo.fail(message)` | Explicit terminal failure; `pcall` cannot undo or recover it |
| `neo.json_array(table)` | Require a table; return a marked table (never a string); `nil` is invalid |
| `neo.json_object(table)` | Require a table; return a marked table (never a string); `nil` is invalid |

There is no `neo.parallel`, recursive workflow launch, detached workflow task, raw shell escape, or engine-selection API.

### Effect outcomes

Host effects fall into three return groups:

- Outcome-table calls (`neo.delegate`, `neo.swarm`, `neo.tool`,
  `neo.verify`, `neo.verify_command`) return one immutable table:

  ```text
  ok, status, summary, details?, actual_usage?, agent_id?, swarm_id?, task_id?
  ```

- `neo.await_user` returns the raw read-only answer value, not an outcome
  table.
- `neo.report` records an intermediate report and returns no value; use it
  only as a statement.

`status` is one of: `completed` | `failed` | `denied` | `cancelled` | `resource_limited` | `interrupted`.

Ordinary verification and tool failures return `ok = false` values that the
script can branch on; they do not require `pcall`. `neo.fail`, uncaught Lua
errors, resource exhaustion, cancellation, and invalid final results terminate
the workflow. `neo.fail` is a terminal run decision that `pcall` cannot undo
or recover. Workflow task IDs are read and waited through `TaskOutput`; never
pass a workflow ID to `WaitDelegate`.

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

**Do not request credentials, API keys, or other secrets.** Answers are persisted in the local journal and remain readable after restart. The run enters durable `awaiting_user`, releases active VM/worker admission, and remains visible in `/tasks` and CLI. The assistant calls `TaskAnswer` only for `human_or_model`; human-only requests are answered by the user in `/tasks`.

`TaskResume` without an answer **cannot** clear `awaiting_user`.

## Machine limits and admission

Configured under `[runtime.workflow]` in `~/.neo/config.toml`. Scripts, model tool inputs, and definitions **cannot** set or raise these values. Rejected keys include predictive fields such as `token_cap`, `max_concurrency` (as a workflow model limit), and `projected_usage`.

Limits cover source/manifest bytes, Lua VM memory and instruction hooks, journal and artifact sizes, global storage, TaskOutput page size, active VMs/workers/executors, and default swarm concurrency. See [Config Files](../configuration/config-files.md#runtimeworkflow-sub-table).

**Global admission** tracks **actual occupancy** (active VMs, workers, executors, storage). When a permit is unavailable the run stays durable and **queued**; `/tasks` and `TaskOutput` can show the wait reason. No wall-clock workflow timeout is inferred. Pause and stop remain available.

## Artifacts and storage layout

Each run directory:

```text
<session_dir>/workflows/<run_id>/
  run.json                 # immutable launch metadata
  journal.jsonl            # append-only state and invocations
  artifacts/               # content-addressed immutable bytes
  recovery-quarantine/     # torn-tail quarantine only
```

Large final results, reports, and raw schema-attempt output may be stored as artifact references. Reads revalidate size/digest. When workflow storage reaches its configured high watermark, automatic retention reclaims terminal runs older than the configured minimum age until the low watermark is restored.

## Run immutability

Terminal runs are immutable. Running the workflow again creates an independent run with its own arguments, result, usage, and journal. Only canonical run directories are readable and resumable; retired journal layouts are not migrated or projected.

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

Use `TaskOutput` with the returned task ID to wait for completion and read the
actual bounded result, journal pages, or artifact content. `WaitDelegate` does
not read workflow tasks.

## `/tasks` dashboard

`/tasks` is extended for workflows: filterable list, phase/progress, queue/admission reason, awaiting-input state, actual usage, details/output, and valid controls (pause, resume, answer, stop). It remains a projection over background tasks and workflow snapshots — not a second state owner. Delegate / Bash / Terminal card layouts are unchanged.

## Built-in workflows

Shipped ordinary definitions (same public Lua APIs, no privileged host functions):

| Name | Intent |
| --- | --- |
| `code-review` | Read-only multi-domain review; never modifies code |
| `deep-research` | Structured multi-step research |
| `large-refactor` | Phased refactor orchestration |

The assistant uses `Workflow(list)`, `Workflow(show)`, and `Workflow(run_saved)`. Humans may use the slash entries above or the headless CLI described above.

## Author checklist

### Assistant route

1. Author through `Workflow`; activate `create-workflow` when authoring guidance is useful.
2. Persist only through `Workflow(save)` and run through `Workflow(run_inline)` or `Workflow(run_saved)`.
3. Use the returned task ID with `TaskOutput` for status, result, artifacts,
   journal pages, or pending input.
4. Use `TaskAnswer` only for a `human_or_model` gate; leave human-only answers to the user.
5. Never ask the user for a bare slash, invoke `neo workflow`, or hand-author a manifest/hash.

### Manual script file authoring

1. Pair `.lua` + `.workflow.toml` with matching stem and `source_sha256`.
2. Declare ordered `phases` and a required final `output_schema`.
3. Give every `neo.delegate` / `neo.swarm` child an `output_schema`.
4. Never request secrets through `neo.await_user`.
5. Validate with `neo workflow check`; use `neo workflow test --case` for fixture harness cases.
6. Use `/workflow` to browse, `/workflow <task>` for automatic selection, or `/workflow:<name> <task>` for an explicit definition; use the headless CLI for scripted operation.
7. View results with `TaskOutput` views/cursors.

## Next steps

- [Built-in Tools](../reference/tools.md) — `Workflow`, `TaskAnswer`, `TaskOutput`, pause/resume/stop
- [Slash Commands](../reference/slash-commands.md) — `/workflow`, `/tasks`
- [Config Files](../configuration/config-files.md) — `[runtime.workflow]`
- [Data Locations](../configuration/data-locations.md) — run layout under the session
