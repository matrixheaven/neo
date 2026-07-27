---
name: create-workflow
description: >
  Create, save, run, use, test, evaluate, inspect, or deeply assess a Neo
  workflow, including black-box evaluation of workflow behavior while the
  current directory is the Neo repository. Route every workflow lifecycle
  step through the registered Workflow tool (validate_inline/run_inline for
  one-off evaluation, save + run_saved for reusable definitions, list/show
  for discovery) and inspect runs with TaskOutput. Also the complete Lua host
  API reference for authoring workflow scripts. Use for any natural-language
  workflow request or /create-workflow. Not for modifying or debugging Neo's
  own workflow implementation or CLI — that is ordinary repository work.
disableModelInvocation: false
---

# Create Workflow

Workflows are deterministic Lua scripts that orchestrate child agents and tools
through the `neo.*` host table. Definitions are a **paired** `<name>.lua` +
`<name>.workflow.toml` resolved by the definition registry. `WorkflowRuntime` is
the sole durable owner of runs; this skill only authors definitions and routes
their validation, save, and launch through the `Workflow` tool.

When talking to the user, call these "workflows," not "Lua scripts."

Lua is the **only** workflow language. Do not author Rhai, dual-engine, or
Grok-style `agent()` / `parallel()` scripts for Neo.

## Routing: use the Workflow tool, never the CLI

The registered `Workflow` tool is the canonical first-party interface for every
workflow lifecycle step. Saved and inline execution are directly available and
require **no slash command, no capability, and no CLI**.

| User intent | Required procedure |
|-------------|--------------------|
| Create/save only | author -> `Workflow(save)` -> ask whether to run now |
| Create and run/test | author -> `Workflow(save)` -> `Workflow(run_saved)` -> `TaskOutput` |
| One-off run/test/evaluate | author -> `Workflow(validate_inline)` -> `Workflow(run_inline)` -> `TaskOutput` |
| Run a known saved workflow | `Workflow(run_saved)` -> `TaskOutput` |
| Discover saved workflows | `Workflow(list)` / `Workflow(show)` -> `Workflow(run_saved)` -> `TaskOutput` |
| Modify/debug Neo's workflow implementation | leave this skill's route; use normal repository diagnosis |

Rules when the user is using workflows as a **product feature** (even inside
the Neo repository):

- Do **not** call `neo workflow ...` through Bash or Terminal. The CLI is a
  human/headless surface, never the assistant path.
- Do **not** compute `source_sha256` or hand-write the `.workflow.toml`
  manifest. Pass the Lua source and schemas to `Workflow(save)`; the host owns
  hashing, manifest construction, pair paths, and atomic persistence.
- Do **not** read Neo's workflow implementation source or run Cargo tests to
  learn how to use, run, or black-box test workflows.
- Do **not** ask the user to run `/workflow` first, and never claim validation
  or execution from authored files alone.
- Do not inspect the repository to decide whether a workflow feature request
  applies: a workflow evaluation request is product use even when cwd is the
  Neo repo.

The one escape hatch: when the user **explicitly** asks to debug, modify, or
test Neo's workflow implementation or CLI itself, ordinary source inspection,
Cargo tests, and CLI invocation are allowed and this skill's routing rules do
not apply.

## Authoring procedure

1. **Gather intent**, conversationally: what should it do, what fans out, what
   must be verified, what is the final structured result, and whether children
   are read-only or may mutate (isolated worktrees for mutation).
2. **Pick a name** (portable grammar `[a-z0-9][a-z0-9_-]{0,63}`) and, for
   saves, a scope: `project` (trusted workspace only) or `user` (all
   workspaces). Builtin scope is not writable.
3. **Author the Lua source** from the example below. It reads `neo.args`,
   calls host APIs, and **returns** one JSON-compatible table matching
   `output_schema`. Keep child prompts imperative and self-contained (see
   Pitfalls). Declare ordered phases and JSON Schemas alongside the source.
4. **Validate**: `Workflow(validate_inline)` for inline definitions or
   `Workflow(validate_saved)` for saved ones. Iterate until the structured
   result reports valid. Validation compiles the definition and Lua; it
   creates no files, runs, or tasks.
5. **Route by intent** using the table above. `Workflow(save)` persists the
   pair (use `replace: true` only when the user wants to overwrite an existing
   definition). Run actions return a task ID; collect the terminal result with
   `TaskOutput`.
6. **Report**: workflow name, validation/save/run structured outcomes, task
   terminal state, saved scope, and remaining risks (e.g. live child quality,
   human gates).

Prefer the smallest workflow that satisfies the request. Do not invent host
APIs that are not listed below. Do not launch workflows from workflows.

## Workflow tool arguments (for the definition fields)

`validate_inline`, `save`, and `run_inline` all take the same definition
fields the host validates canonically:

- `name`, `description`, ordered `phases` (`{id, description}`), exact Lua
  `script`, `input_schema` (object, optional-but-preferred), `output_schema`
  (object, **required**).
- `save` additionally requires `scope` (`user` | `project`) and accepts
  `replace` (default `false`).
- Run actions accept `args` (object, default `{}`).
- Every `neo.phase("id")` call must use an `id` listed in `phases`.

## Example: read-only fan-out → assemble

```lua
-- review-scope.lua
local args = neo.args
local scope = args.scope
if type(scope) ~= "string" or scope == "" then
  neo.fail("review-scope requires non-empty args.scope")
end

local READ_ONLY = { "Read", "List", "Grep", "Find", "Glob" }
local finding_schema = {
  type = "object",
  additionalProperties = false,
  required = { "findings" },
  properties = {
    findings = {
      type = "array",
      items = {
        type = "object",
        additionalProperties = false,
        required = { "path", "issue" },
        properties = {
          path = { type = "string" },
          issue = { type = "string" },
        },
      },
    },
  },
}

neo.phase("scope")
neo.report({ kind = "scope", scope = scope })
neo.log("scope accepted")

neo.phase("review")
local security = neo.delegate({
  title = "security",
  task = "Security review of `" .. scope .. "`. Use Read/Grep; do not modify files. "
    .. "Return at most 8 concrete findings as {path, issue}. Empty findings only after inspection.",
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY,
  output_schema = finding_schema,
})
neo.verify(security.ok, "security failed: " .. tostring(security.summary))

local correctness = neo.delegate({
  title = "correctness",
  task = "Correctness review of `" .. scope .. "`. Use Read/Grep; do not modify files. "
    .. "Return at most 8 concrete findings as {path, issue}.",
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY,
  output_schema = finding_schema,
})
neo.verify(correctness.ok, "correctness failed: " .. tostring(correctness.summary))

local findings = {}
local function append(outcome)
  local list = outcome.details and outcome.details.findings
  if type(list) ~= "table" then
    return
  end
  for i = 1, 64 do
    local f = list[i]
    if f == nil then
      break
    end
    findings[#findings + 1] = {
      path = tostring(f.path or ""),
      issue = tostring(f.issue or ""),
    }
  end
end
append(security)
append(correctness)

neo.phase("finalize")
return {
  ok = true,
  summary = "review complete for " .. scope,
  findings = findings,
}
```

Matching definition fields for the tool call: phases `scope` / `review` /
`finalize`, required `args.scope` in `input_schema`, and an `output_schema`
matching the return table.

## The dialect

- Host surface is the global table `neo`. There is no `agent`, `parallel`,
  `complete`, `pause`, `budget`, or recursive workflow launch.
- Maps/arrays are ordinary Lua tables. For JSON empty-array vs empty-object
  disambiguation use `neo.json_array({...})` and `neo.json_object({...})`.
- Std libs: `table`, `string`, `utf8`, `math`. `math.random` / `math.randomseed`
  are removed. `dofile`, `loadfile`, `print`, and `rawset` are removed.
- Control flow must derive from `neo.args` and host outcomes only. Wall-clock
  randomness is unavailable; pass any needed seed/time through args.
- `neo.args` is **read-only**. Host outcomes are **read-only**.
- Unknown fields on host input tables are rejected (`deny_unknown_fields`).
- Single chunk; final result is the **return value** (one table / scalar).
  Zero returns or a single `nil` become JSON `null` (usually fails schema).
  Multiple return values fail.
- Exactly one tools-disabled schema repair is allowed for child structured
  output; still require strong schemas and fail closed on evidence gates.

## Host API

### Read-only inputs

- `neo.args` — launch arguments object (JSON → Lua). Prefer validating required
  fields early with `neo.fail(...)`.

### Local (no external child/tool effect beyond journal)

| Call | Behavior |
|------|----------|
| `neo.phase(id)` | Select phase `id` declared in `phases`. Unknown id fails. |
| `neo.log(message)` | Non-empty progress line for the user/dashboard. |
| `neo.report(value)` | Record a JSON-compatible intermediate report. |
| `neo.fail(message)` | Fatal abort; subsequent host calls fail. |
| `neo.verify(condition, message)` | If `condition` is false, throws (fail-closed gate). If true, returns nil. |
| `neo.json_array(t)` / `neo.json_object(t)` | Mark table JSON kind for serialization. |

### Children

| Call | Behavior |
|------|----------|
| `neo.delegate({...})` | One child agent. Returns outcome table. |
| `neo.swarm({...})` | Fan-out. Homogeneous template **or** heterogeneous items. |

**`neo.delegate` fields** (new child):

- `task` (required string)
- `title` (optional non-empty)
- `role`: `"coder"` \| `"explorer"` \| `"planner"` \| `"reviewer"`
- `model` / `provider` (optional strings)
- `context`: `"inherit"` \| `"summary"` \| `"none"` (default inherit)
- `worktree`: `"shared"` \| `"isolated"` (default shared). Use **isolated** for
  mutation; merge/retire is always an explicit human decision — never
  auto-merge or delete worktrees in script policy.
- `tool_allow`: optional array of exact tool names; may only **reduce** parent
  tools (e.g. read-only ceiling).
- `output_schema`: **required** for workflow-origin children (JSON Schema object).

**Resume union:** when `resume` is set, only `resume`, `task`, and
`output_schema` are allowed.

**`neo.swarm` fields:**

- `description` (required)
- `role` (swarm default role)
- `items` (required, non-empty)
- Homogeneous form: each item is `{ title, value }`; optional
  `prompt_template`, `resume_agent_ids`. Per-item task/role/worktree/schema
  must be absent.
- Heterogeneous form: each item uses delegate-like fields with **required**
  `task` and **required** `output_schema`. `prompt_template` and
  `resume_agent_ids` are invalid in this form.
- There is **no** hard-coded total child cap. Active concurrency is host-limited
  (`swarm_concurrency`); excess work stays queued. Scripts must not invent
  predictive token budgets.

### Tools and verification

| Call | Behavior |
|------|----------|
| `neo.tool({ name, input })` | Canonical `ToolRegistry` tool. `input` must be a JSON object. |
| `neo.verify_command({ command, cwd?, failure_message? })` | Runs via `Bash`. Success returns outcome; failure throws (wrapper). |
| `neo.await_user({...})` | Durable human (or policy-allowed) gate; returns **answer value**. |

**`neo.tool` deny list** (exact names; not eligible):  
`Workflow`, `Delegate`, `DelegateSwarm`, `TaskPause`, `TaskResume`,
`TaskStop`, `TaskAnswer`, `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`,
`StartGoal`, `ExitGoalMode`, `UpdateGoalStatus`, `GetGoalStatus`, `Todo`,
`TodoList`, `ListDelegates`, `WaitDelegate`, `InterruptDelegate`,
`MessageDelegate`.  
`TaskOutput` cannot target the **current** workflow run id. Unknown names fail.
Ordinary registered tools are eligible by default.

**`neo.await_user` fields:**

- `prompt` (required non-empty)
- `answer_schema` (required JSON Schema)
- `default` (optional; must validate)
- `title` (optional non-empty)
- `answer_policy`: `"human"` (default) \| `"human_or_model"`  
  Never request secrets/passwords through this surface.

### Outcome shape

Successful/failed host effects that return outcomes expose a read-only table:

- `ok` (boolean)
- `status`: `completed` \| `failed` \| `denied` \| `cancelled` \|
  `resource_limited` \| `interrupted`
- `summary` (string)
- `details` (JSON value; structured child output often lives here)
- `actual_usage` (optional)
- `agent_id` / `swarm_id` / `task_id` when child refs are present

Treat missing, failed, or unusable verification as **unverified**, not success.
Evidence gates must fail closed (`neo.verify` / `neo.fail`).

## Determinism, durability, resume

- Host calls are journaled. Recovery never auto-retries an external effect whose
  completion is uncertain after host exit (`interrupted(host_exit)`).
- Make effectful steps idempotent or re-check state before repeating work after
  resume/answer.
- `AwaitingUser` is durable and independent of the worker loop; answers go
  through `neo workflow answer` / TUI, not ad-hoc files.
- Resource policy uses **actual occupancy only**. Do not encode predictive
  `token_cap`, projected usage, or model-supplied machine limits in scripts or
  manifests.

## Patterns that work

- **Plan → fan-out → synthesize.** Build the work list in plain Lua from args or
  a trusted tool result; re-filter agent-discovered paths in Lua before
  sharding.
- **Read-only review:** `tool_allow = { "Read", "List", "Grep", "Find", "Glob" }`,
  `worktree = "shared"`.
- **Mutation slices:** `worktree = "isolated"`, then `neo.await_user` for
  merge/retire. Never auto-merge.
- **Heterogeneous `neo.swarm`:** distinct roles/schemas per item when children
  are not template-identical.
- **Homogeneous `neo.swarm`:** title/value + `prompt_template` for large uniform
  shards.
- **Adversarial verify:** independent reviewer children prompted to refute;
  require concrete evidence fields in schema.
- **Inspect builtins through the tool:** `Workflow(show)` on `code-review`,
  `deep-research`, or `large-refactor` returns their paired sources. Prefer
  adapting those patterns over inventing new host APIs.

## Pitfalls (each maps to real failures)

- **Wrong product dialect.** Rhai `agent()` / `parallel()` / `complete()` /
  `let meta = #{...}` is not Neo. Use paired definition fields + `neo.*` +
  `return`.
- **CLI fallback.** `neo workflow check/run/save` through Bash is the human
  surface. The assistant validates, saves, and runs through the `Workflow`
  tool only.
- **Hand-computed manifests.** `source_sha256` and the `.workflow.toml` pair
  are host-owned by `Workflow(save)`; never author them yourself.
- **Missing `output_schema`.** The final definition schema and every workflow
  child require it. Heterogeneous swarm items must set per-item
  `output_schema`.
- **Terse child prompts.** Cold children return empty structured shells without
  tools. Command tool use and define what a valid empty answer requires.
- **Unguarded outcomes.** Always check `outcome.ok` (or `neo.verify`) before
  trusting `details`.
- **Phase id typos.** `neo.phase` only accepts ids declared in `phases`.
- **Project scope without trust.** Untrusted project definitions cannot be
  saved; use `user` scope or ask the user about trust.
- **`neo.tool` control-plane bypass.** Denied tools stay denied; do not try
  `Workflow` / `Delegate` / plan-mode / goal tools from scripts.
- **Silent truncation.** If you cap findings, `neo.log` what was dropped.
- **Agents do not enforce invariants — scripts do.** Re-check paths, schemas,
  and counts in Lua after children return.
- **Empty Lua arrays vs objects.** Use `neo.json_array` / `neo.json_object` when
  schema distinguishes `[]` and `{}`.

## Built-in references

Product ships three ordinary registry builtins (inspect via `Workflow(show)`):

- `code-review` — read-only multi-domain review, findings-first final output
- `deep-research` — heterogeneous research children + structured report
- `large-refactor` — isolated mutation slices + human merge gate

## Human/headless CLI reference (not the assistant path)

These commands remain supported for humans, scripts, and explicit CLI testing.
**The assistant must use the registered `Workflow` tool unless the user
explicitly asked for CLI operation.**

```bash
neo workflow list
neo workflow show <name>
neo workflow check <name-or-path> --output json
neo workflow test <name-or-path> --case <fixture.json>
neo workflow run <name> --args-json '{"scope":"crates/neo-agent-core"}'
neo workflow save <run-or-pair-path> --scope user|project [--name <name>] [--force]
neo workflow answer <run> <request_id> --json '<answer>'
```

Interactive users may also launch a saved definition host-direct with the exact
`/workflow <name> [JSON_OBJECT]` slash command (zero model calls before
execution). Never tell the user a slash command is required for the assistant
to act; it is optional convenience only.

## Done criteria

Report only after the requested terminal state is real:

1. Create/save: a structured `Workflow(save)` success exists (ok, saved
   status, resolved definition).
2. Validate: a structured `Workflow(validate_inline)` or
   `Workflow(validate_saved)` success exists.
3. Run/test/evaluate: a real task was launched through `Workflow(run_inline)`
   or `Workflow(run_saved)` **and** inspected to a terminal state through
   `TaskOutput`, or a real typed failure is reported verbatim.
4. No slash capability, CLI invocation, or manual hash/manifest step was used
   or requested.
5. Any intentional limits (read-only, no auto-merge, schema caps) and
   remaining risks are stated accurately.

If validation or launch fails, report the structured error exactly; do not
claim the workflow is ready.
