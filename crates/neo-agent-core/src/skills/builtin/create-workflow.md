---
name: create-workflow
description: >
  Create a Neo workflow: author a paired Lua script and .workflow.toml definition
  (phases, neo.delegate / neo.swarm fan-out, schemas, human gates), validate with
  `neo workflow check`, save under user or project scope, and offer a real run.
  Also the complete Lua host API reference for workflow scripts. Use when the user
  wants to create, author, or write a workflow, automate a multi-agent pipeline,
  or runs /create-workflow.
disableModelInvocation: false
---

# Create Workflow

Workflows are deterministic Lua scripts that orchestrate child agents and tools
through the `neo.*` host table. Definitions are a **paired** `<name>.lua` +
`<name>.workflow.toml` resolved by the definition registry. `WorkflowRuntime` is
the sole durable owner of runs; this skill only authors and validates
definitions.

When talking to the user, call these "workflows," not "Lua scripts."

Lua is the **only** workflow language. Do not author Rhai, dual-engine, or
Grok-style `agent()` / `parallel()` scripts for Neo.

## Procedure

1. **Gather intent**, conversationally: what should it do, what fans out, what
   must be verified, what is the final structured result, and whether children
   are read-only or may mutate (isolated worktrees for mutation).
2. **Pick a name and scope** (portable name grammar
   `[a-z0-9][a-z0-9_-]{0,63}`):
   - Project (default inside a trusted workspace):
     `<workspace>/.neo/workflows/<name>.{lua,workflow.toml}`
   - User (all workspaces): `$NEO_HOME/workflows/<name>.{lua,workflow.toml}`
   - Project save requires workspace trust. Builtin scope is not writable.
3. **Author the pair.** Start from the example below. Shape is: TOML manifest
   (display metadata, phases, input/output schemas, `source_sha256`) + Lua body
   that reads `neo.args`, calls host APIs, and **returns** one JSON-compatible
   table matching `output_schema`. Keep child prompts imperative and
   self-contained (see Pitfalls).
4. **Compute `source_sha256`** as the lowercase hex SHA-256 of the **exact**
   `.lua` file bytes (including trailing newline as written). Mismatch fails
   resolution closed.
5. **Validate without creating a run:**
   ```bash
   neo workflow check <name-or-path> --output json
   ```
   Path may be the `.lua` or `.workflow.toml` (or registry name after install).
   Iterate until `ok` is true. Check compiles the definition and Lua; it does
   not execute host effects or live children.
6. **Optional fixture harness** (deterministic, no live providers):
   ```bash
   neo workflow test <name-or-path> --case <fixture.json>
   ```
7. **Offer a real run** with representative args:
   - CLI: `neo workflow run <name> --args-json '...'`
   - Interactive: exact `/workflow <name>` slash launch (host-direct; zero model
     calls before execution)
8. **Report**: pair paths, check output, how to run, scopes, and remaining risks
   (e.g. live child quality, human gates).

Prefer the smallest workflow that satisfies the request. Do not invent host APIs
that are not listed below. Do not launch workflows from workflows.

## Definition pair

### `<name>.workflow.toml`

```toml
definition_format_version = 2
name = "review-scope"          # optional; when present must equal filename stem
display_name = "Review Scope"
description = "Read-only multi-domain review with structured findings."
source_sha256 = "<lowercase-hex-sha256-of-exact-lua-bytes>"

[[phases]]
id = "scope"
description = "Accept inputs"

[[phases]]
id = "review"
description = "Dispatch review children"

[[phases]]
id = "finalize"
description = "Assemble final output"

[input_schema]
type = "object"
additionalProperties = false
required = ["scope"]

[input_schema.properties.scope]
type = "string"
minLength = 1

[output_schema]
type = "object"
additionalProperties = false
required = ["ok", "summary", "findings"]

[output_schema.properties.ok]
type = "boolean"

[output_schema.properties.summary]
type = "string"

[output_schema.properties.findings]
type = "array"
items = { type = "object" }
```

Rules:

- `definition_format_version` must be `2`.
- `output_schema` is **required** on every definition (final return value).
- `input_schema` is optional but preferred when `neo.args` fields are required.
- Every `neo.phase("id")` call must use an `id` listed under `[[phases]]`.
- Precedence when resolving names: `builtin < user < trusted project`.
  Same-scope name conflicts invalidate the name; invalid higher-scope content
  never silently falls back.
- Revision framing is host-owned (manifest + source). Authors only need correct
  paired files and matching `source_sha256`.

### `<name>.lua`

- Single chunk; final result is the **return value** (one table / scalar).
- Zero returns or a single `nil` become JSON `null` (usually fails schema).
- Multiple return values fail.
- Exactly one tools-disabled schema repair is allowed for child structured
  output; still require strong schemas and fail closed on evidence gates.

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

Ship the matching `.workflow.toml` with phases `scope` / `review` / `finalize`,
required `args.scope`, and an `output_schema` that matches the return table.
Recompute `source_sha256` after every Lua edit.

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

## Host API

### Read-only inputs

- `neo.args` — launch arguments object (JSON → Lua). Prefer validating required
  fields early with `neo.fail(...)`.

### Local (no external child/tool effect beyond journal)

| Call | Behavior |
|------|----------|
| `neo.phase(id)` | Select phase `id` declared in the TOML. Unknown id fails. |
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
`RunWorkflow`, `Delegate`, `DelegateSwarm`, `TaskPause`, `TaskResume`,
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
- **Copy shape from builtins:** `code-review`, `deep-research`, `large-refactor`
  under the builtin workflow registry (paired sources in the product). Prefer
  adapting those patterns over inventing new host APIs.

## Pitfalls (each maps to real failures)

- **Wrong product dialect.** Rhai `agent()` / `parallel()` / `complete()` /
  `let meta = #{...}` is not Neo. Use paired TOML + `neo.*` + `return`.
- **Missing `output_schema`.** Final definition schema and every workflow child
  require it. Homogeneous swarm items get schema via host child planning rules;
  heterogeneous items must set per-item `output_schema`.
- **Terse child prompts.** Cold children return empty structured shells without
  tools. Command tool use and define what a valid empty answer requires.
- **Unguarded outcomes.** Always check `outcome.ok` (or `neo.verify`) before
  trusting `details`.
- **Phase id typos.** `neo.phase` only accepts ids declared in the TOML.
- **Stale `source_sha256`.** Any Lua byte change requires a new hash.
- **Project scope without trust.** Untrusted project definitions are absent and
  cannot be saved.
- **`neo.tool` control-plane bypass.** Denied tools stay denied; do not try
  `RunWorkflow` / `Delegate` / plan-mode / goal tools from scripts.
- **Silent truncation.** If you cap findings, `neo.log` what was dropped.
- **Agents do not enforce invariants — scripts do.** Re-check paths, schemas,
  and counts in Lua after children return.
- **Empty Lua arrays vs objects.** Use `neo.json_array` / `neo.json_object` when
  schema distinguishes `[]` and `{}`.

## Built-in references

Product ships three ordinary registry builtins (read them when shaping larger
workflows):

- `code-review` — read-only multi-domain review, findings-first final output
- `deep-research` — heterogeneous research children + structured report
- `large-refactor` — isolated mutation slices + human merge gate

## CLI quick reference

```bash
neo workflow list
neo workflow show <name>
neo workflow check <name-or-path> --output json
neo workflow test <name-or-path> --case <fixture.json>
neo workflow run <name> --args-json '{"scope":"crates/neo-agent-core"}'
neo workflow save <run-or-pair-path> --scope user|project [--name <name>] [--force]
neo workflow answer <run> <request_id> --json '<answer>'
```

## Done criteria

Report only after:

1. Pair files exist at the chosen scope paths.
2. `source_sha256` matches the Lua bytes.
3. `neo workflow check` returns `ok: true`.
4. User was offered a real run path (`/workflow` or `neo workflow run`).
5. Any intentional limits (read-only, no auto-merge, schema caps) are stated.

If check fails, report diagnostics exactly; do not claim the workflow is ready.
