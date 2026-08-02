# Workflow Output Reliability Handoff

Date: `2026-08-03`

## Handoff State

The design and implementation plan are written. No Workflow source code was
changed in this handoff. The next implementer must read these two files first:

- `docs/aegis/specs/2026-08-03-workflow-output-reliability-design.md`
- `docs/aegis/plans/2026-08-03-workflow-output-reliability.md`

The current worktree contains unrelated user changes. Preserve them. The
existing `Ctrl+O` return-from-alternate-screen fix is not part of this handoff.

## Locked Direction

The old failure chain is retired:

```text
AI output differs from prompt schema
  -> schema_invalid
  -> hidden repair model turn
  -> repair still fails
  -> neo.fail
  -> useful Workflow run discarded
```

The new chain is:

```text
child execution completes
  -> preserve original result and usage
  -> attempt one local structured projection when requested
  -> projection unavailable is data
  -> Workflow continues or returns deterministic partial output
```

Actual provider, runtime, permission, cancellation, resource, host-channel,
and explicit script failures still terminate. A business result such as
`verified = false`, `supported = false`, contradictions, or gaps does not.

## Implementation Order

1. Make `WorkflowInvocationOutcome.status` the host execution state and move
   `neo.verify` truth into result details.
2. Remove the child repair dispatch and mandatory child output-schema checks.
3. Make final Lua result persistence unconditional with respect to output-shape
   mismatch.
4. Update `deep-research`, `code-review`, and `large-refactor` to distinguish
   execution failure from partial evidence.
5. Update the one existing authoring skill/tool description and focused tests.
6. Run retirement scans, formatting, and exact focused regressions.

Do not begin by rewriting the script language, adding a retry, adding a schema
mode, or modifying transcript cards. Those are design drift.

## Source Map

- child repair owner: `crates/neo-agent-core/src/workflow/runtime.rs`;
- foreground Delegate consumer:
  `crates/neo-agent-core/src/tools/delegate.rs`;
- child request lowering: `crates/neo-agent-core/src/workflow/lua.rs`;
- host outcome shape: `crates/neo-agent-core/src/workflow/state.rs`;
- final result persistence: `crates/neo-agent-core/src/workflow/runtime.rs`;
- built-in research/review scripts:
  `crates/neo-agent-core/src/workflow/builtins/`;
- model guidance: `crates/neo-agent-core/src/skills/builtin/create-workflow.md`.

Use CodeGraph or a bounded symbol search for these owners and direct callers.
Do not perform a whole-repository redesign search.

## Acceptance Evidence Required

Before handing back implementation, provide exact evidence for:

- extra child output fields remain completed and consume no repair turn;
- prose/invalid JSON only loses the structured projection;
- provider/runtime errors preserve original message and usage;
- mixed swarm results remain visible and partial;
- final Lua output persists despite schema mismatch;
- `neo.verify(false)` is completed result data;
- `deep-research` partial fallback survives missing verifier projection;
- no new schema-repair journal events or repair dispatch;
- no active Workflow host-status use of `.ok`;
- historical session replay and non-Workflow cards remain unchanged.

Local test output must name the exact package, target, and filter. A focused
macOS pass must not be described as remote CI or cross-platform acceptance.

## Handoff Rules

- Preserve the existing dirty worktree and do not revert other agents' work.
- Do not edit `Ctrl+O`, `neo-tui` transcript files, or unrelated providers.
- Do not use a regular-expression list as semantic validation of model output;
  the JSON parser remains a projection tool only.
- Do not restore the hidden repair turn under another function name.
- Do not call `neo.fail` for schema mismatch, missing projection, or negative
  evidence.
- Keep `neo.fail` terminal for explicit script/host failure.
- Do not claim completion until the exact acceptance checks and retirement scan
  have run.
