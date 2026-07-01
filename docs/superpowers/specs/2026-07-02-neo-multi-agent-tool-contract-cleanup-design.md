# Neo Multi-Agent Tool Contract Cleanup Design

Date: 2026-07-02

Status: Draft for user review

## 1. Purpose

Neo's multi-agent implementation now supports foreground and background
delegation, live follow-up, swarm control, resume, list, wait, and task-browser
integration. The latest tool-level test report found that the behavior mostly
works, but the tool contract is still too hard for a model to infer from the
model-visible tool schema and tool results alone.

This spec defines a new canonical cleanup pass for the confirmed twelve gaps.
It treats the current implementation as the baseline and updates the product
contract without compatibility branches or legacy output modes.

The goal is simple: a parent agent must be able to use Neo's multi-agent tools
correctly from the system-injected tool schema and the structured tool results,
without reading source code, docs, or prior specs.

## 2. Scope

This spec fixes the twelve confirmed issues from the July 2026 multi-agent tool
test report:

| ID | Issue | Owner |
| --- | --- | --- |
| 1 | `MessageDelegate` follow-up can leave the parent-visible summary looking like only the last addition | Summary and run result contract |
| 2 | Terminal-state wording says immutable while resume is allowed | Error wording and resume guidance |
| 3 | `ListDelegates` cursor semantics are unclear across filters | Pagination contract |
| 4 | Resuming a cancelled agent returns `completed` without explaining that this is a new run | Resume metadata |
| 5 | `ListDelegates` default output is token-heavy | List output shape |
| 6 | Foreground `Delegate` result lacks top-level duration/timestamp fields | Delegate result shape |
| 7 | `ListDelegates` default active-only empty result is confusing | Empty-state hints |
| 8 | `title` is not returned as a reliable independent field | Title contract |
| 9 | Foreground and background swarm result shapes differ | Swarm result shape |
| 10 | Cancelled and timed-out delegates lack obvious lifecycle timing fields in list/wait output | Lifecycle timing fields |
| 11 | `context` modes are hard to observe from results | Context-mode echo and usage metadata |
| 12 | `failed` and `timed_out` lifecycle states are not explained well enough in model-visible schema | Tool schema completeness |

The report's thirteenth item, `MessageDelegate` support for swarm targets, is
not in scope because the current implementation already supports
`MessageDelegate(swarm_id, message)` and the tool schema already says the target
ID may be an agent or swarm ID.

## 3. Non-Goals

- No hosted service, sync, marketplace, or remote collaboration.
- No second legacy output format. New result shapes replace the current
  ambiguous defaults.
- No offline mailbox API. `MessageDelegate` remains live-only.
- No new subagent role values.
- No status-specific timestamp aliases such as `cancelled_at_ms` or
  `timed_out_at_ms`; lifecycle timing uses the shared fields below.
- No attempt to make a summary represent a hidden full transcript unless the
  tool result explicitly says that is its scope.

## 4. Design Principles

1. Structured details are the source of truth. Human-readable tool text may be
   compact, but JSON details must carry the canonical fields.
2. Defaults must be cheap. Listing delegates should not pull large prompts or
   summaries into context unless explicitly requested.
3. Summary scope must be explicit. A parent agent should never have to guess
   whether `summary` means the latest run, the whole agent history, or a swarm
   aggregate.
4. Pagination cursors must be safe to reuse only when they are valid for the
   query that produced them.
5. Tool schema descriptions are part of the model-facing contract. Any behavior
   needed to use a tool correctly must be present in the schema or result hints.

## 5. Canonical Shared Fields

Every multi-agent result that reports an agent or swarm must use these shared
field names in structured details.

```text
kind: "delegate" | "delegate_list" | "delegate_wait" | "delegate_swarm" | "swarm"
id: agent_id or swarm_id
status: queued | running | completed | failed | cancelled | timed_out | not_found
mode?: foreground | background
role?: coder | explorer | planner | reviewer
title?: string
task?: string                    # only present when explicitly included
context_mode?: inherit | summary | none
created_at_ms?: integer
updated_at_ms?: integer
started_at_ms?: integer
terminal_at_ms?: integer
elapsed_ms?: integer
token_count?: integer
tool_count?: integer
summary?: string
summary_scope?: "current_run" | "agent_history" | "swarm_items" | "none"
activity_tail?: AgentActivityEntry[]
resume_hint?: string
next_steps?: string[]
```

`terminal_at_ms` is the canonical terminal timestamp for `completed`,
`failed`, `cancelled`, and `timed_out`. Consumers that need state-specific names
can derive them from `status` plus `terminal_at_ms`.

Text output should mirror the same contract with a short readable subset. It
must not include full prompts or large summaries by default when the structured
details omit them.

## 6. Summary And Resume Contract

### 6.1 Agent Runs

An `AgentEntity` may have multiple runs over its lifetime:

- the initial `Delegate` call creates run `1`;
- each `Delegate(resume=agent_id, task=<next prompt>)` creates the next run;
- `MessageDelegate` delivers a live message into the currently running run and
  does not create a new run by itself.

Each agent snapshot must expose:

```text
run_index: integer              # 1 for the first run
run_count: integer              # same as latest run_index on snapshots
current_run_started_at_ms?: integer
current_run_terminal_at_ms?: integer
previous_status?: AgentLifecycleState
resumed_from?: agent_id
```

The persisted conversation history remains available to the runtime for resume,
but result summaries must not pretend to include hidden history unless they
actually do.

### 6.2 Delegate Result

Foreground `Delegate` returns a `DelegateResult`:

```json
{
  "kind": "delegate",
  "mode": "foreground",
  "agent_id": "agent_xxx",
  "id": "agent_xxx",
  "status": "completed",
  "actual_role": "explorer",
  "title": "Investigate MVCC",
  "context_mode": "inherit",
  "run_index": 1,
  "run_count": 1,
  "created_at_ms": 1783000000000,
  "started_at_ms": 1783000000100,
  "terminal_at_ms": 1783000002100,
  "elapsed_ms": 2000,
  "tool_count": 3,
  "token_count": 4200,
  "summary_scope": "current_run",
  "summary": "Summarized the MVCC visibility rules and identified relevant files."
}
```

For resume:

```json
{
  "kind": "delegate",
  "mode": "foreground",
  "agent_id": "agent_xxx",
  "id": "agent_xxx",
  "status": "completed",
  "actual_role": "explorer",
  "title": "Transaction ID wraparound",
  "context_mode": "inherit",
  "run_index": 2,
  "run_count": 2,
  "resumed_from": "agent_xxx",
  "previous_status": "cancelled",
  "summary_scope": "current_run",
  "summary": "Added transaction ID wraparound notes for the resumed run."
}
```

The text output must say that `status: completed` describes the resumed run, not
the original terminal state:

```text
agent_id: agent_xxx
status: completed
run_index: 2
previous_status: cancelled
summary_scope: current_run
```

### 6.3 MessageDelegate Summary Semantics

`MessageDelegate` sends a live follow-up into an active run. The final summary
for that run remains `summary_scope: current_run`.

If the child agent responds only to the live follow-up and does not restate
earlier material, Neo must not label that summary as complete agent history.
The result should make the safer interpretation obvious:

```json
{
  "summary_scope": "current_run",
  "run_index": 1,
  "live_messages_received": 1,
  "full_history_available_via_resume": true
}
```

This fixes the observed case where the parent saw only the added wraparound
section and could not tell whether the original MVCC body was intentionally
omitted or lost.

## 7. Error Wording Contract

Terminal entities are immutable for mutation tools, but resume is a new run on
the same agent. Error messages must say that distinction plainly.

For `MessageDelegate` or `InterruptDelegate` on a terminal agent:

```text
agent already cancelled; terminal agents cannot receive live messages or be interrupted. To continue this agent, call Delegate with resume="agent_xxx".
```

For terminal swarms:

```text
swarm already completed; terminal swarms cannot be interrupted. To continue unfinished child agents, call DelegateSwarm with resume_agent_ids.
```

Avoid the shorter wording `terminal delegate state is immutable` by itself. It
is technically true but misleading when `Delegate(resume=<agent_id>)` is valid.

## 8. ListDelegates Contract

### 8.1 Input

`ListDelegates` keeps the existing filters and adds explicit output controls:

```text
include_completed?: boolean = false
kind?: agent | swarm | all = all
state?: queued | running | completed | failed | cancelled | timed_out
limit?: integer = 20
cursor?: string
order?: newest | oldest = newest
include?: array<meta | task | summary | activity> = ["meta"]
```

There is no `verbose` alias. `include` is the single canonical expansion
mechanism.

### 8.2 Default Output

The default result is meta-only:

```json
{
  "kind": "delegate_list",
  "count": 2,
  "total": 12,
  "include_completed": false,
  "include": ["meta"],
  "order": "newest",
  "query": {
    "kind": "all",
    "state": null
  },
  "delegates": [
    {
      "kind": "agent",
      "id": "agent_xxx",
      "status": "running",
      "display_name": "Hypatia",
      "title": "Investigate MVCC",
      "mode": "background",
      "role": "explorer",
      "created_at_ms": 1783000000000,
      "updated_at_ms": 1783000005000,
      "started_at_ms": 1783000000100,
      "terminal_at_ms": null,
      "elapsed_ms": 4900,
      "tool_count": 2,
      "token_count": 1300
    }
  ]
}
```

`task`, `summary`, and `activity_tail` are omitted unless requested through
`include`.

### 8.3 Empty Results

When the default active-only query finds nothing, return a hint:

```json
{
  "kind": "delegate_list",
  "count": 0,
  "total": 0,
  "delegates": [],
  "next_steps": [
    "No active delegates found.",
    "Pass include_completed=true to list completed, failed, cancelled, or timed_out delegates."
  ]
}
```

### 8.4 Cursor Safety

`next_cursor` must bind to the query that produced it. Use an opaque cursor that
encodes or references:

- kind
- state
- include_completed
- order
- include
- offset

Reusing a cursor with different query parameters is invalid and returns a
tool error:

```text
cursor was created for a different ListDelegates query; restart pagination without cursor
```

The response should include a model-readable cursor description:

```json
{
  "next_cursor": "cursor_v1_example",
  "cursor_query": {
    "kind": "agent",
    "state": "completed",
    "include_completed": true,
    "order": "oldest",
    "include": ["meta"]
  }
}
```

## 9. Title Contract

`title` is a reliable independent field. It is the short caller-provided or
locally derived human label for the agent task. `task` is the full prompt.

All relevant tool details must use:

```text
title: short label
task: full prompt, present only when include contains "task" or for direct Delegate results
```

`ListDelegates` must never hide the title inside a formatted task string. The
TUI may render `title`, while model-facing structured details can use it for
reference and disambiguation.

## 10. Context Mode Observability

`Delegate` must echo the actual context mode in both foreground and background
results:

```json
{
  "context_mode": "summary"
}
```

The tool schema description for `context` must explain:

- `inherit`: the child receives the selected parent context;
- `summary`: the child receives a compact parent summary;
- `none`: the child receives only its task and role/profile prompt.

The result does not need to promise a fixed token-savings percentage. It should
include `context_mode` and normal token counts when available so the parent can
observe the run.

`DelegateSwarm` children continue to use isolated child context unless a future
spec adds swarm-level context controls.

## 11. Lifecycle State Documentation

Tool schema descriptions must explain these terminal states:

- `completed`: child run reached a normal assistant message end without tool or
  runtime errors.
- `failed`: child run hit a model, tool, runtime, permission, validation, or
  child event error.
- `cancelled`: user or parent interrupted a running/queued child.
- `timed_out`: Neo stopped waiting or a managed background command exceeded its
  configured timeout. `WaitDelegate` returning `outcome: "timed_out"` means the
  wait call timed out while the target may still be running; a delegate status
  of `timed_out` means the delegate itself reached a terminal timeout state.

The distinction between wait timeout and delegate terminal timeout must be
visible in `WaitDelegate` details:

```json
{
  "kind": "delegate_wait",
  "id": "agent_xxx",
  "outcome": "wait_timed_out",
  "status": "running"
}
```

## 12. Swarm Result Contract

Foreground `DelegateSwarm`, `WaitDelegate(swarm_id)`, and `TaskOutput(swarm_id)`
must return the same `SwarmResult` details shape.

```json
{
  "kind": "delegate_swarm",
  "swarm_id": "swarm_xxx",
  "id": "swarm_xxx",
  "status": "completed",
  "mode": "foreground",
  "role": "coder",
  "description": "Audit crates",
  "created_at_ms": 1783000000000,
  "updated_at_ms": 1783000005000,
  "started_at_ms": 1783000000100,
  "terminal_at_ms": 1783000005000,
  "elapsed_ms": 4900,
  "summary_scope": "swarm_items",
  "aggregate": {
    "total": 2,
    "queued": 0,
    "running": 0,
    "completed": 2,
    "failed": 0,
    "cancelled": 0,
    "timed_out": 0
  },
  "items": [
    {
      "index": 0,
      "item": "neo-agent-core",
      "agent_id": "agent_a",
      "name": "Hypatia",
      "status": "completed",
      "title": "neo-agent-core",
      "elapsed_ms": 2100,
      "tool_count": 4,
      "token_count": 2500,
      "summary": "Finished the neo-agent-core audit with no high-risk findings."
    }
  ],
  "resume_hint": "Call DelegateSwarm with resume_agent_ids for unfinished children."
}
```

Text output may be shorter, but it must not omit fields that are present in one
path and absent from another structured path. The same JSON shape is the
compatibility contract.

## 13. WaitDelegate Contract

For agent targets, terminal results return the same agent lifecycle fields as
`DelegateResult`. For non-terminal wait timeout:

```json
{
  "kind": "delegate_wait",
  "id": "agent_xxx",
  "agent_id": "agent_xxx",
  "status": "running",
  "outcome": "wait_timed_out",
  "next_steps": [
    "The delegate is still running.",
    "Increase timeout_ms, call ListDelegates, or wait for automatic completion."
  ]
}
```

For swarm targets, terminal results return `SwarmResult`. Non-terminal wait
timeout preserves current swarm aggregate and uses `outcome: "wait_timed_out"`.

## 14. TaskOutput Contract For Delegates And Swarms

`TaskOutput(agent_id)` and `TaskOutput(swarm_id)` must be useful for delegate
tasks, not only generic background metadata.

Agent task output returns the same compact lifecycle fields as `DelegateResult`,
plus `activity_tail` when available. Swarm task output returns `SwarmResult`.

Generic background task fields may remain for bash/question tasks, but delegate
and swarm task IDs must use the multi-agent shapes above.

## 15. Model-Visible Schema Requirements

The following behavior must be present in tool descriptions or input field
descriptions, because the system prompt injects the tool schema catalog:

- `Delegate`: foreground is default; background returns immediately; resume
  starts a new run on an existing agent; `role` must be omitted with `resume`;
  `context` values and meanings.
- `MessageDelegate`: live-only; accepts `agent_id` or `swarm_id`; terminal
  agents require `Delegate(resume=<agent_id>)`; swarm target broadcasts only to
  running children.
- `ListDelegates`: defaults to active-only and meta-only; use
  `include_completed=true` for history; use `include` for task/summary/activity;
  cursors are valid only for the same query.
- `WaitDelegate`: accepts `agent_id` or `swarm_id`; distinguishes wait timeout
  from target terminal `timed_out`.
- `DelegateSwarm`: foreground is default; background returns immediately;
  `WaitDelegate` and `TaskOutput` expose the same structured swarm result;
  only `{{item}}` and optional `{{description}}` are supported.

If a model would need to read `docs/`, `AGENTS.md`, or Rust source to know one
of these facts, the schema is incomplete.

## 16. Acceptance Criteria

1. `Delegate` foreground details include top-level `elapsed_ms`, lifecycle
   timestamps, `context_mode`, `summary_scope`, `run_index`, and `run_count`.
2. `Delegate(resume=<agent_id>)` details include `previous_status`, `resumed_from`,
   and a `run_index` greater than `1`.
3. A run that received `MessageDelegate` reports `live_messages_received` and
   `summary_scope: "current_run"`.
4. Terminal-agent errors explain that live mutation is blocked but resume is
   allowed.
5. `ListDelegates()` defaults to meta-only and does not include full task or
   summary text.
6. `ListDelegates(include=["summary"])` is the canonical way to include
   summaries in list output.
7. `ListDelegates()` with no active delegates returns a hint mentioning
   `include_completed=true`.
8. Reusing a `ListDelegates` cursor with changed query parameters returns an
   invalid cursor error instead of silently paginating a different result set.
9. `ListDelegates` rows include independent `title` fields.
10. Lifecycle timing fields use the shared `created_at_ms`, `updated_at_ms`,
    `started_at_ms`, `terminal_at_ms`, and `elapsed_ms` names.
11. Foreground `DelegateSwarm`, `WaitDelegate(swarm_id)`, and
    `TaskOutput(swarm_id)` expose the same structured `SwarmResult` shape.
12. `WaitDelegate` uses `outcome: "wait_timed_out"` for wait-call timeout while
    preserving the target's current `status`.
13. Tool schema catalog text is sufficient for a model to understand context
    modes, failed/timed-out states, resume semantics, cursor safety, and
    list-output expansion.

## 17. Verification

Use narrow tests for each touched tool boundary:

- `neo-agent-core` tool tests for `Delegate`, `MessageDelegate`,
  `ListDelegates`, `WaitDelegate`, `DelegateSwarm`, and `TaskOutput`.
- Runtime tests for run indexing, resume metadata, live-message counts, and
  lifecycle timestamps.
- Schema tests that inspect the registered tool specs and assert the critical
  model-facing guidance is present.

Do not use broad package-wide test runs as evidence unless a specific failure
requires widening scope.
