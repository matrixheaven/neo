# Neo Multi-Agent Hardening Design

Date: 2026-06-30

Status: Draft for user review

## 1. Purpose

Neo's first Multi-Agent implementation proves that foreground/background
delegation, swarm execution, workflow scripting, and task browser integration can
all run through the Rust runtime. The latest tool-level usage test found 18
issues that show the current contracts are still incomplete.

This spec hardens the Multi-Agent feature into a coherent product contract:

- Delegate lifecycle states are immutable once terminal.
- `Delegate` supports resuming an existing subagent, aligned with Kimi Code's
  `Agent(resume=...)` model.
- `MessageDelegate` is only for live running/background follow-up, not offline
  mailbox queuing.
- Swarms become first-class entities addressable by control/query tools.
- Lua workflow APIs return usable structured data and expose reports visibly.
- Subagent roles become real profiles with prompt addenda and tool policies,
  not only a `Role: ...` label inside the child prompt.

This is a breaking canonicalization pass. It intentionally does not keep legacy
aliases or compatibility branches for incorrect behavior.

## 2. References

Local reference project findings from `docs/kimi-code`:

- `docs/kimi-code/docs/en/customization/agents.md`
  - Kimi has built-in `coder`, `explore`, and `plan` subagents.
  - `explore` is read-only.
  - `plan` has no shell access and focuses on planning.
  - Subagents can run in the background and can be resumed later.
- `docs/kimi-code/packages/agent-core/src/profile/default/*.yaml`
  - Kimi roles are profiles with prompt text and explicit tool lists.
  - `explore.yaml` allows read/search tools and read-only shell operations.
  - `plan.yaml` removes shell and write tools.
- `docs/kimi-code/packages/agent-core/src/tools/builtin/collaboration/agent.ts`
  - `Agent` accepts `resume` and forbids combining `resume` with
    `subagent_type`.
  - Background results include a resume hint.
- `docs/kimi-code/packages/agent-core/src/tools/builtin/collaboration/agent-swarm.ts`
  - `AgentSwarm` supports `resume_agent_ids`.
  - It returns structured per-subagent XML results.
  - It rejects invalid placeholders and duplicate expanded prompts before
    starting children.
- `docs/kimi-code/apps/kimi-code/test/tui/components/messages/tool-call.test.ts`
  - A single foreground subagent renders as `Explore Agent Running (...)`.
  - The generic `Using Agent` card is hidden.
  - The subagent activity window keeps only the latest visible entries.
- `docs/kimi-code/apps/kimi-code/src/tui/components/messages/agent-swarm-progress-estimator.ts`
  - Swarm progress is smoothed from child lifecycle/tool-call observations,
    not a direct jump from one static value to 100%.

Neo should borrow the product contract, not the TypeScript implementation.

## 3. Problem Inventory

The latest test report found these failures and UX gaps:

| ID | Severity | Issue | Design owner |
| --- | --- | --- | --- |
| 1 | High | `InterruptDelegate` rewrites completed tasks to cancelled | Lifecycle state machine |
| 2 | High | `MessageDelegate` queues completed/idle messages that never deliver | Resume and live mailbox contract |
| 3 | High | `MessageDelegate` cannot address swarms | Swarm first-class entity |
| 4 | High | Lua `neo.swarm` cannot access per-item results | Lua workflow result handles |
| 5 | High | `neo.verify` is a shell runner despite assertion-like docs | Lua workflow API split |
| 6 | Medium | `TaskOutput` omits delegate summary/transcript | Task output contract |
| 7 | Medium | `TaskStop` says stopped but persisted state is cancelled | Status vocabulary |
| 8 | Medium | `DelegateSwarm` and Lua `neo.swarm` disagree on required fields | Schema unification |
| 9 | Medium | Returning Lua userdata causes serialization errors | Lua table-safe return values |
| 10 | Medium | Summary context leaks internal prefix into child summaries | Child prompt/result cleanup |
| 11 | Medium | `neo.report` only increments count, content invisible | Workflow report visibility |
| 12 | Medium | Swarm cannot be stopped/interrupted/messaged by swarm id | Swarm first-class entity |
| 13 | Low | `ListDelegates` sorts by id and lacks paging/filtering | Listing contract |
| 14 | Low | Display names repeat and confuse long sessions | Display identity contract |
| 15 | Low | Background swarm lacks partial failure signal | Swarm aggregate outcome |
| 16 | Low | No resume/rerun tool | Delegate resume |
| 17 | Low | Lua errors expose Rust internal paths | User-facing error normalization |
| 18 | Low | Mailbox cannot peek/drain | Remove offline mailbox semantics |

## 4. Goals

1. Make the behavior explainable to a model from tool schema alone.
2. Make terminal state immutable and impossible to accidentally rewrite.
3. Make every control/query tool accept either an `agent_id` or a `swarm_id`
   where that operation naturally applies.
4. Replace dead mailbox queues with explicit resume semantics.
5. Make Lua workflow output useful for real workflow orchestration.
6. Turn roles into enforceable profiles with prompt and tool policy.
7. Keep Delegate Swarm visible in the chat transcript, not a separate page.
8. Preserve foreground-by-default delegation.

## 5. Non-Goals

- No compatibility aliases for old role values. `harness` is gone; the role is
  `orchestrator`.
- No additional swarm template placeholders. Only `{{item}}` and optional
  `{{description}}` are supported.
- No offline mailbox API in this pass. `MessageDelegate` is live-only.
- No hosted service, marketplace, or cross-machine collaboration.
- No nested subagent spawning by subagents unless explicitly designed later.

## 6. Canonical Entities

### 6.1 Agent Entity

```text
AgentEntity
  id: AgentId                  # agent_<uuid>
  display_name: AgentDisplayName
  path: AgentPath              # /root/Gibbs or /root/<swarm_id>/Gibbs
  role: AgentRole
  profile: AgentProfile
  mode: foreground | background
  state: queued | running | completed | failed | cancelled | timed_out
  task_title: string           # short UI title, not the whole task prompt
  task_prompt: string          # full prompt
  created_at: timestamp
  updated_at: timestamp
  started_at?: timestamp
  terminal_at?: timestamp
  tool_count: integer
  token_count: integer
  activity: AgentActivityEntry[]
  summary?: string
  error?: string
  parent_swarm_id?: SwarmId
```

`task_title` must be short and UI-friendly. If the caller only provides a long
task prompt, Neo derives a deterministic truncated title locally; no LLM title
generation is allowed.

### 6.2 Swarm Entity

```text
SwarmEntity
  id: SwarmId                  # swarm_<uuid>
  description: string
  role: AgentRole
  mode: foreground | background
  state: queued | running | completed | failed | cancelled | timed_out
  created_at: timestamp
  updated_at: timestamp
  terminal_at?: timestamp
  children: SwarmChildRef[]
  aggregate: SwarmAggregate
```

```text
SwarmChildRef
  index: integer
  item?: string
  agent_id: AgentId
  prompt: string
  state: AgentLifecycleState
  summary?: string
  error?: string
```

```text
SwarmAggregate
  total: integer
  queued: integer
  running: integer
  completed: integer
  failed: integer
  cancelled: integer
  timed_out: integer
```

Swarms are first-class task entities. They appear in list/query APIs and can be
waited, stopped, interrupted, and messaged by `swarm_id`.

## 7. Lifecycle Contract

### 7.1 States

```text
queued -> running -> completed
                 -> failed
                 -> cancelled
                 -> timed_out
```

Terminal states:

- `completed`
- `failed`
- `cancelled`
- `timed_out`

Terminal states are immutable. No control tool may rewrite a terminal entity to
another terminal state.

### 7.2 Interrupt and Stop

`InterruptDelegate` and `TaskStop` only apply to `queued` or `running` entities.

If the target is terminal:

```text
is_error: true
message: "agent already completed; terminal delegate state is immutable. To continue this agent, call Delegate with resume."
```

The status word must match persisted state. If a running entity is stopped by
the user, the persisted state and tool output are both `cancelled`. Do not emit
`stopped` as a state.

For a swarm target, stop/interrupt cancels only non-terminal children. If all
children are already terminal, the swarm returns an `already <state>` error.

## 8. Delegate Tool

### 8.1 Schema

```text
Delegate
  task: string
  resume?: string
  title?: string
  role?: coder | explorer | planner | reviewer | orchestrator
  mode?: foreground | background
  context?: inherit | summary | none
```

### 8.2 New Agent

If `resume` is absent:

- `task` is required.
- `role` defaults to `coder`.
- `mode` defaults to `foreground`.
- `context` defaults to `inherit`.
- Neo creates a new `AgentEntity`.

### 8.3 Resume Existing Agent

If `resume` is present:

- `resume` must be an existing `agent_id`, not a `swarm_id`.
- `task` is required and becomes the next user prompt to that same child agent.
- `role` must be absent. The resumed agent keeps its original role/profile.
- Display name, path, history, and prior summary remain associated with the
  same agent.
- If the agent is currently running, return an error:

```text
agent is already running; use MessageDelegate for live follow-up
```

- If the agent is terminal or idle, start a new run on the same agent.

This is the only canonical way to continue a completed/failed/timed-out/cancelled
agent. Do not use `MessageDelegate` for offline queueing.

### 8.4 Result

Foreground success result:

```text
agent_id: agent_xxx
actual_role: explorer
status: completed

[summary]
...
```

Background result:

```text
task_id: agent_xxx
status: running
agent_id: agent_xxx
actual_role: explorer
automatic_notification: true
next_step: The completion arrives automatically; do not poll unless you need early status.
resume_hint: To continue this same subagent later, call Delegate with resume="agent_xxx".
```

## 9. MessageDelegate

### 9.1 Schema

```text
MessageDelegate
  id: string       # agent_id or swarm_id
  message: string
```

### 9.2 Agent Target

Allowed only when the target agent is running or background-running.

If target is idle or terminal:

```text
is_error: true
message: "agent is not running; use Delegate with resume to continue it"
```

### 9.3 Swarm Target

For a running swarm, `MessageDelegate(swarm_id, message)` broadcasts the message
to running/background child agents only.

If no child can receive the message:

```text
is_error: true
message: "swarm has no running children; use DelegateSwarm with resume_agent_ids to continue unfinished children"
```

The result includes per-child delivery status:

```json
{
  "target": "swarm_xxx",
  "delivered": ["agent_a", "agent_b"],
  "skipped": [
    { "agent_id": "agent_c", "state": "completed" }
  ]
}
```

## 10. DelegateSwarm

### 10.1 Schema

```text
DelegateSwarm
  description: string
  items?: string[]
  prompt_template?: string
  resume_agent_ids?: map<agent_id, prompt>
  role?: coder | explorer | planner | reviewer | orchestrator
  mode?: foreground | background
  max_concurrency?: integer
```

`description` is required everywhere, including Lua. The tool and Lua API must
share the same schema.

### 10.2 Template Rules

- If `items` is present, `prompt_template` is required.
- `prompt_template` must contain `{{item}}`.
- `{{description}}` is optional and expands to the swarm description.
- No other placeholders are supported.
- Duplicate expanded prompts are rejected before any child starts.

### 10.3 Resume Rules

`resume_agent_ids` maps existing `agent_id` to the prompt used to resume that
child.

```json
{
  "resume_agent_ids": {
    "agent_abc": "continue and finish the failed verification",
    "agent_def": "retry the same task after the rate limit"
  }
}
```

It can be used alone or combined with new `items`.

### 10.4 Result

Swarm result must include aggregate and per-child details:

```json
{
  "swarm_id": "swarm_xxx",
  "status": "completed",
  "aggregate": {
    "total": 3,
    "completed": 2,
    "failed": 1,
    "cancelled": 0,
    "timed_out": 0
  },
  "items": [
    {
      "index": 1,
      "item": "crates/neo-agent-core",
      "agent_id": "agent_a",
      "status": "completed",
      "summary": "..."
    },
    {
      "index": 2,
      "item": "crates/neo-tui",
      "agent_id": "agent_b",
      "status": "failed",
      "error": "..."
    }
  ],
  "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children."
}
```

Foreground `DelegateSwarm` returns this after all children are terminal.
Background `DelegateSwarm` returns the `swarm_id` immediately and exposes the
same structure through `WaitDelegate` and `TaskOutput`.

## 11. ListDelegates

### 11.1 Schema

```text
ListDelegates
  include_completed?: boolean = false
  kind?: agent | swarm | all = all
  state?: queued | running | completed | failed | cancelled | timed_out | all = all
  limit?: integer = 20
  cursor?: string
  order?: newest | oldest = newest
```

### 11.2 Output

Swarms appear as their own rows. Child agents still appear as agent rows unless
filtered out.

Default output hides completed terminal rows but still shows running swarms and
running children.

Ordering defaults to newest first, not lexicographic id order.

## 12. WaitDelegate

`WaitDelegate(id)` accepts `agent_id` or `swarm_id`.

For agent:

- Waits until the agent is terminal or timeout is reached.
- Returns summary/error and activity tail.

For swarm:

- Waits until all children are terminal or timeout is reached.
- Returns aggregate and per-child summaries.
- If only some children are failed/timed out/cancelled, the swarm status follows
  this precedence:
  1. `running` if any child is running.
  2. `queued` if no child is running and at least one child is queued.
  3. `failed` if any child failed or timed out.
  4. `cancelled` if at least one child was cancelled and none failed/timed out.
  5. `completed` only if every child completed.

## 13. TaskOutput

`TaskOutput(task_id)` must return useful output for delegate and swarm tasks.

For agent:

```json
{
  "kind": "delegate",
  "agent_id": "agent_xxx",
  "status": "completed",
  "summary": "...",
  "activity_tail": [...]
}
```

For swarm:

```json
{
  "kind": "swarm",
  "swarm_id": "swarm_xxx",
  "status": "failed",
  "aggregate": {...},
  "items": [...]
}
```

This removes the current split where `WaitDelegate` has the useful result but
`TaskOutput` only shows metadata.

## 14. TaskStop

`TaskStop` accepts agent and swarm ids. It uses the same lifecycle rules as
`InterruptDelegate`.

Output state must be `cancelled` when a running task is stopped. Do not use
`stopped` as a persistent or returned state.

Calling `TaskStop` on a terminal task returns an error with `already <state>`.

## 15. Lua Workflow API

### 15.1 Shared Schema

Lua `neo.delegate` and `neo.swarm` use the same schema and validation rules as
`Delegate` and `DelegateSwarm`.

### 15.2 Table-Safe Handles

Lua handles are convenient objects during execution but must serialize cleanly
when returned from the workflow.

`return neo.delegate({...})` returns a JSON/table value, not an unsupported
userdata error.

Delegate handle methods:

```lua
local d = neo.delegate({ task = "...", role = "explorer" })
d:id()
d:status()
d:summary()
d:result()
d:to_table()
```

Swarm handle methods:

```lua
local s = neo.swarm({
  description = "review crates",
  prompt_template = "Review {{item}}",
  items = { "crates/neo-agent-core", "crates/neo-tui" },
})
s:id()
s:status()
s:summary()
s:items()
s:results()
s:has_failures()
s:to_table()
```

`items()` and `results()` return per-child structured tables.

### 15.3 Verify API Split

`neo.verify(condition, message)` is an assertion:

```lua
neo.verify(count == 3, "expected three completed children")
```

If the condition is false, the workflow fails with `message`.

Shell verification is a separate API:

```lua
neo.verify_command("cargo check -p neo-agent-core", "core check failed")
```

`neo.verify_command` must document that it uses the normal Bash permission path.
If permission is denied, the error says:

```text
verify_command denied by Bash permission policy
```

Do not expose Rust source paths in workflow errors.

### 15.4 Reports

`neo.report(value)` appends to `reports[]` and emits a workflow update.

`RunWorkflow` output includes report content:

```json
{
  "reports": [
    { "index": 1, "value": "started review" },
    { "index": 2, "value": { "completed": 3 } }
  ]
}
```

The textual result should include a short report preview, not only `reports: N`.

## 16. Role Profiles

Roles become enforceable profiles.

```text
AgentProfile
  role: AgentRole
  display_label: string
  prompt_addendum: string
  tool_policy: ToolPolicy
```

Profiles are built into Neo. They are not generated by the LLM.

### 16.1 Coder

Purpose: implement bounded code changes.

Tools:

- `read`, `list`, `grep`, `find`, `glob`
- `bash`
- `write`, `edit`
- `todo`

Prompt addendum:

- Treat the parent agent as the caller.
- Return a compact technical summary.
- Do not ask the end user questions.
- Never mutate git state.

### 16.2 Explorer

Purpose: read-only investigation.

Tools:

- `read`, `list`, `grep`, `find`, `glob`
- `bash` in read-only mode

Read-only Bash allows commands such as:

- `ls`
- `find`
- `rg`
- `git status`
- `git diff`
- `git log`
- `git blame`

Explorer cannot use `write`, `edit`, or mutating shell commands.

Prompt addendum:

- Search/read/analyze only.
- Prefer parallel read/search calls when independent.
- Report findings with file references and confidence.

### 16.3 Planner

Purpose: implementation planning and architecture design.

Tools:

- `read`, `list`, `grep`, `find`, `glob`

No Bash. No write/edit.

Prompt addendum:

- Identify unknowns.
- Recommend explorer subagents if more investigation is required.
- Produce step-by-step implementation plans.

### 16.4 Reviewer

Purpose: review code, risks, regressions, and test gaps.

Tools:

- `read`, `list`, `grep`, `find`, `glob`
- `bash` in read-only mode

No write/edit.

Prompt addendum:

- Findings first, ordered by severity.
- Include file/line references.
- Focus on bugs, behavioral regressions, missing tests, and risk.

### 16.5 Orchestrator

Purpose: coordinate multi-agent workflows.

Tools:

- `Delegate`
- `DelegateSwarm`
- `WaitDelegate`
- `ListDelegates`
- `MessageDelegate`
- `InterruptDelegate`
- `TaskOutput`
- `TaskStop`
- `RunWorkflow`
- `todo`

No direct `write`/`edit`. No direct Bash unless a later workflow-specific
permission design explicitly adds `verify_command`.

Prompt addendum:

- Break work into bounded subagent tasks.
- Prefer foreground blocking unless background collaboration is explicitly
  useful.
- Wait for foreground subagents and summarize results.
- Use resume for continuing old agents.

## 17. Tool Policy Enforcement

Tool policy is enforced before child runtime execution. Prompt text is only a
secondary safety layer.

Tool policy can narrow parent permissions but cannot widen them.

For example:

- If parent is `yolo`, explorer still cannot write files.
- If parent lacks Bash permission, reviewer cannot run Bash.
- All subagents are denied git mutations regardless of role.

Read-only Bash should use a structured classifier, not a broad prompt-only
instruction. If a command cannot be classified as read-only, reject it.

## 18. Prompt and Summary Hygiene

Child prompts should be profile-specific and should not leak setup boilerplate
into final summaries.

Requirements:

- The child prompt tells the model not to repeat system/profile setup text.
- The parent summary extractor strips known boilerplate such as
  `Acknowledged. Ready as ...`.
- `context: summary` passes a concise parent summary but not internal
  implementation notes.
- Final summaries should contain only useful findings/results.

## 19. TUI and Transcript

Delegate and swarm transcript rendering remains in the chat transcript.

### 19.1 Single Delegate Card

Expected shape:

```text
● Explorer Agent Running (Map auth module) · 3 tools · 24s · 25.6k tok
  Press Ctrl+B to run in background
  • Used Read (crates/neo-agent-core/src/auth.rs)
  • Used Grep (AuthState|TokenStore)
  ◌ Read auth module and token storage...
  └ Auth state is persisted through...
```

Rules:

- Use role display label: `Coder`, `Explorer`, `Planner`, `Reviewer`,
  `Orchestrator`.
- Header uses `task_title`, not the full task prompt.
- Activity area has a stable max height.
- Older activity scrolls out of the card.
- Completion keeps only useful final tail, not duplicate parent tool output.

### 19.2 Swarm Card

Swarm card shows:

- First-class `swarm_id` state.
- Real-time child rows.
- Queued/running/completed/failed/cancelled/timed_out counts.
- Smoothed progress estimate based on child lifecycle and tool activity.
- Latest child result or latest child activity, not the full prompt forever.

The progress bar must start near zero when no child has started. It must not
jump directly from a small static value to 100% unless all children complete
immediately.

### 19.3 Tasks Browser

Background agents and swarms appear in `/tasks`.

- Agent tasks show summary/activity tail in `TaskOutput`.
- Swarm tasks show aggregate and per-child results.
- Task browser state vocabulary uses `cancelled`, not `stopped`.

## 20. Error Message Contract

Errors should be actionable and user-facing.

Examples:

```text
agent already completed; terminal delegate state is immutable. To continue it, call Delegate with resume="agent_xxx".
```

```text
agent is not running; MessageDelegate only sends live follow-up. To continue it, call Delegate with resume="agent_xxx".
```

```text
swarm has no running children; use DelegateSwarm with resume_agent_ids to continue unfinished children.
```

```text
verify_command denied by Bash permission policy.
```

Do not expose Rust source locations such as
`crates/neo-agent-core/src/workflow/lua.rs:50` in user-facing workflow errors.

## 21. Acceptance Criteria

### Lifecycle

- Completed delegates cannot be changed to cancelled by `InterruptDelegate`.
- `InterruptDelegate(completed_agent)` returns error with `already completed`.
- `TaskStop(completed_agent)` returns error with `already completed`.
- Running stop/interrupt persists and returns `cancelled`.

### Resume and Message

- `Delegate({ resume: agent_id, task: "continue" })` resumes an existing idle or
  terminal agent.
- `Delegate({ resume: agent_id, role: "coder", task: "continue" })` is rejected.
- `MessageDelegate(completed_agent)` is rejected and points to `Delegate resume`.
- `MessageDelegate(running_agent)` delivers a live follow-up.

### Swarm

- `ListDelegates(kind="swarm")` returns swarm rows.
- `WaitDelegate(swarm_id)` returns aggregate and per-child results.
- `TaskOutput(swarm_id)` returns the same useful aggregate data.
- `InterruptDelegate(swarm_id)` cancels running children only.
- `MessageDelegate(swarm_id)` broadcasts only to running children.
- Background swarm output includes partial failures and resume hints.

### Lua

- `neo.swarm(...):items()` returns per-item child results.
- `return neo.swarm(...)` serializes successfully.
- `neo.verify(true, "msg")` succeeds.
- `neo.verify(false, "msg")` fails with `msg`.
- `neo.verify_command("...")` uses Bash permission and reports denial clearly.
- `neo.report(...)` contents appear in `RunWorkflow` output details and preview.
- Lua errors do not expose Rust source paths.

### Roles

- `explorer` cannot write/edit.
- `explorer` cannot run mutating shell commands.
- `planner` has no Bash.
- `reviewer` is read-only.
- `orchestrator` can coordinate subagents but cannot directly edit code.
- Role tool policy is enforced by runtime, not only by prompt.

### TUI

- Single delegate card title uses short title, not full prompt.
- Delegate activity window has a fixed max height.
- Explore/Planner/etc. role labels render in card headers.
- Swarm progress starts from queued/near-zero and updates through intermediate
  states.
- Swarm rows show latest activity/result instead of permanently showing the
  original prompt.

## 22. Implementation Decomposition

This spec should be implemented as multiple plans, not one large pass.

Recommended plan split:

1. Lifecycle and resume core
   - State machine terminal immutability.
   - `Delegate.resume`.
   - Live-only `MessageDelegate`.
2. Swarm first-class entity
   - `SwarmEntity` registry.
   - Swarm addressing across list/wait/output/stop/message.
   - `resume_agent_ids`.
3. Lua workflow API hardening
   - Structured handles.
   - `neo.verify` assertion and `neo.verify_command`.
   - Visible reports and clean errors.
4. Role profile enforcement
   - Built-in profiles.
   - Tool policy filtering.
   - Read-only Bash classifier for explorer/reviewer.
5. TUI and task UX polish
   - Short titles.
   - Max-height activity cards.
   - Swarm progress and latest activity/result rows.
   - `/tasks` output parity.

Each plan must include focused tests for the changed contracts before
implementation.

## 23. Open Decisions

No open product decisions remain for this spec.

Important fixed decisions:

- `Delegate.resume` is the canonical continuation path.
- `MessageDelegate` is live-only.
- Terminal stop/interrupt returns errors with `already <state>`.
- Swarms are first-class entities.
- Role `harness` is not retained; the canonical role is `orchestrator`.
- `neo.verify` is assertion-only; shell verification is `neo.verify_command`.
- Only `{{item}}` and `{{description}}` placeholders are supported.
