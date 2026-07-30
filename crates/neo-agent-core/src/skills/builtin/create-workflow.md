---
name: create-workflow
description: "Authoring guidance for creating, designing, writing, adapting, evaluating, testing, or running a custom Neo workflow. Covers the Lua dialect, definition schemas, and the `neo.*` host APIs used to orchestrate child agents and tools. Use this skill for creation, change, adaptation, or confirmed one-off authoring; it does not grant workflow execution capability. Existing saved-workflow use starts with `Workflow(list)` or a known saved workflow through `Workflow(run_saved)`, not this skill. `Workflow(run_inline)`, `Workflow(run_saved)`, and `Workflow(save)` each validate the definition internally. Not for modifying or debugging Neo's own workflow implementation or CLI -- that is ordinary repository work."
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

`Workflow(run_inline)`, `Workflow(run_saved)`, and `Workflow(save)` each perform their complete validation internally, so they can be called directly without a
preceding validation step. `Workflow(validate_inline)` and
`Workflow(validate_saved)` are available for explicit check-only requests that
compile the definition and Lua without persisting, running, or creating tasks.

### Procedure by intent

| User intent | Procedure |
|-------------|-----------|
| Create/save only | author -> `Workflow(save)` -> ask whether to run now |
| Create and run/test | author -> `Workflow(save)` -> `Workflow(run_saved)`; use the returned task ID with `TaskOutput` when status, result, artifacts, or a pending question is needed |
| One-off run/test/evaluate | author -> `Workflow(run_inline)`; use the returned task ID with `TaskOutput` when status, result, artifacts, or a pending question is needed |
| Explicit check-only | author -> `Workflow(validate_inline)` / `Workflow(validate_saved)` -> report |
| Run a known saved workflow | `Workflow(run_saved)`; use the returned task ID with `TaskOutput` for status, result, artifacts, or pending input |
| Discover saved workflows | `Workflow(list)` / `Workflow(show)` -> `Workflow(run_saved)` |
| Use a workflow without naming one | `Workflow(list)` -> choose a suitable definition -> `Workflow(show)` only when needed -> `Workflow(run_saved)` |
| Modify/debug Neo's workflow implementation | leave this skill; use normal repository diagnosis |

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
- Do not examine the repository to decide whether a workflow feature request
  applies: a workflow use request is product use even when cwd is the Neo repo.

The one escape hatch: when the user **explicitly** asks to debug, modify, or
test Neo's workflow implementation or CLI itself, ordinary source examination,
Cargo tests, and CLI invocation are allowed and this skill's routing rules do
not apply.

## Authoring checklist

1. Choose the `Workflow` action before writing its arguments.
2. Declare both `input_schema` and `output_schema` for every inline definition.
3. Make Lua return exactly the result declared by `output_schema`.
4. Check every host outcome's `ok` field before using its result.
5. Call `neo.fail` with the preserved summary when a required outcome fails.
6. Call `neo.tool` with `{ name = "ToolName", input = { ... } }`.
7. Put an `output_schema` on every heterogeneous `neo.swarm` item.
8. Use `neo.json_array` and `neo.json_object` only as Lua table type markers.

Before authoring, gather the goal, fan-out, required checks, final structured
result, and whether children may mutate. Pick a portable name
(`[a-z0-9][a-z0-9_-]{0,63}`) and a save scope (`project` or `user`). Author a
paired definition, then route it through the Workflow tool. `save`, `run_inline`,
and `run_saved` validate internally; explicit validation remains optional.
Use the returned task ID with `TaskOutput`, and report the real terminal state
and remaining risks.

Prefer the smallest workflow that satisfies the request. Do not invent host
APIs that are not listed below. Do not launch workflows from workflows.

## Workflow tool arguments (for the definition fields)

`validate_inline`, `validate_saved`, `save`, `run_inline`, and `run_saved` all
take the same definition fields the host validates canonically:

- `name`, `description`, ordered `phases` (`{id, description}`), exact Lua
  `script`, `input_schema` (object, **required**), and `output_schema` (object,
  **required**). For a workflow with no arguments use
  `{"type":"object","additionalProperties":false}`. For a required argument
  use, for example,
  `{"type":"object","additionalProperties":false,"required":["text"],"properties":{"text":{"type":"string"}}}`.
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
    .. "Return at most 8 concrete findings as {path, issue}. Empty findings only after review.",
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY,
  output_schema = finding_schema,
})
local security_check = neo.verify(
  security.ok,
  "security failed: " .. tostring(security.summary)
)
if not security_check.ok then
  neo.fail(security_check.summary)
end

local correctness = neo.delegate({
  title = "correctness",
  task = "Correctness review of `" .. scope .. "`. Use Read/Grep; do not modify files. "
    .. "Return at most 8 concrete findings as {path, issue}.",
  role = "reviewer",
  worktree = "shared",
  tool_allow = READ_ONLY,
  output_schema = finding_schema,
})
local correctness_check = neo.verify(
  correctness.ok,
  "correctness failed: " .. tostring(correctness.summary)
)
if not correctness_check.ok then
  neo.fail(correctness_check.summary)
end

local findings = {}
local function append(outcome)
  local structured = outcome.details and outcome.details.structured_output
  local list = structured and structured.findings
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
findings = neo.json_array(findings)
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

- `neo.args` -- launch arguments object (JSON → Lua). Prefer validating required
  fields early with `neo.fail(...)`.

### Local (no external child/tool effect beyond journal)

| Call | Behavior |
|------|----------|
| `neo.phase(id)` | Select phase `id` declared in `phases`. Unknown id fails. |
| `neo.log(message)` | Non-empty progress line for the user/dashboard. |
| `neo.report(value)` | Record a JSON-compatible intermediate report. |
| `neo.fail(message)` | Fatal abort; subsequent host calls fail. |
| `neo.verify(condition, message)` | Returns an immutable outcome. Check `outcome.ok`; false is an ordinary failed result, while `neo.fail` is terminal. |
| `neo.json_array(t)` / `neo.json_object(t)` | Mark table JSON kind for serialization. |

### Children

| Call | Behavior |
|------|----------|
| `neo.delegate({...})` | One child agent. Returns outcome table. |
| `neo.swarm({...})` | Fan-out through direct child specs with one schema per item. |

**`neo.delegate` fields** (new child):

- `task` (required string)
- `title` (optional non-empty)
- `role`: `"coder"` \| `"explorer"` \| `"planner"` \| `"reviewer"`
- `model` / `provider` (optional strings)
- `context`: `"inherit"` \| `"summary"` \| `"none"` (default inherit)
- `worktree`: `"shared"` \| `"isolated"` (default shared). Use **isolated** for
  mutation; merge/retire is always an explicit human decision -- never
  auto-merge or delete worktrees in script policy.
- `tool_allow`: optional array of exact tool names; may only **reduce** parent
  tools (e.g. read-only ceiling).
- `output_schema`: **required** for workflow-origin children (JSON Schema object).

**Resume union:** when `resume` is set, only `resume`, `task`, and
`output_schema` are allowed.

**`neo.swarm` fields:**

- `description` (required)
- `items` (required, non-empty)
- Direct form only: every item uses delegate-like fields with **required**
  `task` and **required** `output_schema`; optional `title`, `resume`, `role`,
  `model`, `provider`, `context`, `worktree`, and `tool_allow` follow the
  `neo.delegate` rules above.
- Even when every child performs the same kind of work, emit direct items with
  per-item `task` and `output_schema`. Do not use `title`/`value`,
  `prompt_template`, `resume_agent_ids`, or a top-level `output_schema`; those
  belong to the separate model-facing `DelegateSwarm` adapter, not the workflow
  DSL.
- There is **no** hard-coded total child cap. Active concurrency is host-limited
  (`swarm_concurrency`); excess work stays queued. Scripts must not invent
  predictive token budgets.

### Tools and verification

| Call | Behavior |
|------|----------|
| `neo.tool({ name, input })` | Canonical `ToolRegistry` tool. `input` must be a JSON object. |
| `neo.verify_command({ command, cwd?, failure_message? })` | Runs via `Bash` and returns an immutable outcome for both success and ordinary failure. |
| `neo.await_user({...})` | Durable human (or policy-allowed) gate; returns **answer value**. |

**`neo.tool` deny list** (exact names; not eligible):  
`Workflow`, `Delegate`, `DelegateSwarm`, `TaskPause`, `TaskResume`,
`TaskStop`, `TaskAnswer`, `AskUserQuestion`, `EnterPlanMode`, `ExitPlanMode`,
`StartGoal`, `ExitGoalMode`, `UpdateGoalStatus`, `GetGoalStatus`, `Todo`,
`TodoList`, `ListDelegates`, `WaitDelegate`, `InterruptDelegate`,
`MessageDelegate`.  
`TaskOutput` cannot target the **current** workflow run id. Unknown names return a
failed outcome. Check `outcome.ok` before using details; ordinary failures do not
require `pcall`. Ordinary registered tools are eligible by default.

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
- `details` (JSON value); schema-valid child JSON is at
  `details.structured_output`
- `actual_usage` (optional)
- `agent_id` / `swarm_id` / `task_id` when child refs are present

Treat missing, failed, or unusable verification as **unverified**, not success.
Evidence gates must fail closed (`neo.verify` / `neo.fail`).

## Determinism, durability, resume

- Host calls are journaled. Recovery never auto-retries an external effect whose
  completion is uncertain after host exit (`interrupted(host_exit)`).
- Make effectful steps idempotent or re-check state before repeating work after
  resume/answer.
- `AwaitingUser` is durable and independent of the worker loop. For a
  `human_or_model` gate, use `TaskAnswer(task_id, request_id, answer)`; a
  human-only gate is answered by the user in the TUI or the human CLI, never
  through ad-hoc files.
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
- **Direct `neo.swarm`:** one complete child spec and schema per item, including
  uniform shards.
- **Adversarial verify:** independent reviewer children prompted to refute;
  require concrete evidence fields in schema.
- **Show builtins through the tool:** `Workflow(show)` on `code-review`,
  `deep-research`, or `large-refactor` returns their paired sources. Prefer
  adapting those patterns over inventing new host APIs.

## Pitfalls (each maps to real failures)

- **Wrong product dialect.** Rhai `agent()` / `parallel()` / `complete()` /
  `let meta = #{...}` is not Neo. Use paired definition fields + `neo.*` +
  `return`.
- **CLI fallback.** `neo workflow list/run/check/test` through Bash is the
  human/headless surface. The assistant validates, saves, and runs through the
  `Workflow` tool only.
- **Hand-computed manifests.** `source_sha256` and the `.workflow.toml` pair
  are host-owned by `Workflow(save)`; never author them yourself.
- **Missing `output_schema`.** The final definition schema and every workflow
  child require it. Heterogeneous swarm items must set per-item
  `output_schema`.
- **Terse child prompts.** Cold children return empty structured shells without
  tools. Tell children to use tools and define what a valid empty answer requires.
- **Unguarded outcomes.** Always check `outcome.ok` before trusting `details`.
  Ordinary verification and tool failures are values, so do not wrap them in
  `pcall`; reserve `pcall` for catchable Lua errors.
- **Phase id typos.** `neo.phase` only accepts ids declared in `phases`.
- **Project scope without trust.** Untrusted project definitions cannot be
  saved; use `user` scope or ask the user about trust.
- **`neo.tool` control-plane bypass.** Denied tools stay denied; do not try
  `Workflow` / `Delegate` / plan-mode / goal tools from scripts.
- **Silent truncation.** If you cap findings, `neo.log` what was dropped.
- **Agents do not enforce invariants -- scripts do.** Re-check paths, schemas,
  and counts in Lua after children return.
- **Empty Lua arrays vs objects.** Use `neo.json_array` / `neo.json_object` when
  schema distinguishes `[]` and `{}`.

## Built-in references

Product ships three ordinary registry builtins (review their paired sources via
`Workflow(show)`):

- `code-review` -- read-only multi-domain review, findings-first final output
- `deep-research` -- heterogeneous research children + structured report
- `large-refactor` -- isolated mutation slices + human merge gate

## Human/headless CLI reference (not the assistant path)

These four commands remain supported for humans and headless scripts. They are
documentation for that human-owned surface, never an assistant workflow route.

```bash
neo workflow list
neo workflow run <name> --args-json '{"scope":"crates/neo-agent-core"}'
neo workflow check <name-or-path> --output json
neo workflow test <name-or-path> --case <fixture.json>
```

Interactive users have separate workflow-use and authoring entries:

```text
/workflow
/workflow <natural-language task>
/workflow:<name> <natural-language task>
/skill:create-workflow <authoring request>
```

The first three forms prepare a normal model turn. They never accept workflow
argument JSON and never launch a workflow directly from the host. The skill
entry is the explicit authoring path.

## Done criteria

Report only after the requested terminal state is real:

1. Create/save: a structured `Workflow(save)` success exists (ok, saved
   status, resolved definition).
2. Explicit validation (optional): a structured `Workflow(validate_inline)`
   or `Workflow(validate_saved)` success exists.
3. Run/test/evaluate: a real task was launched through `Workflow(run_inline)`
   or `Workflow(run_saved)` (each validates the definition internally), or a
   real typed failure is reported verbatim. Use `TaskOutput` with the returned
   task ID for terminal details, artifacts, status, journal pages, or pending
   input.
4. No slash capability, CLI invocation, or manual hash/manifest step was used
   or requested.
5. Any intentional limits (read-only, no auto-merge, schema caps) and
   remaining risks are stated accurately.

If validation or launch fails, report the structured error exactly; do not
claim the workflow is ready.
