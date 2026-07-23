# ADR-0004 - Durable RunWorkflow runtime and append-only replay

Status: `recorded-from-work`
Date: `2026-07-23`

## Source Evidence

- Implemented and focused-tested RunWorkflow work recorded under docs/aegis/work/2026-07-22-runworkflow-audit-repair.

## Context

Neo needed Dynamic Workflow execution to survive host exits, preserve canonical tool permission and instruction handling, expose explicit controls, and avoid repeating external effects. The prior foreground recorder and direct child-dispatch shape could not provide one durable owner or reliable recovery.

## Decision

Use WorkflowRuntime as the canonical lifecycle and control owner. Persist immutable launch metadata in run.json and all current state, invocation outcomes, actual provider usage, and replay identity in append-only journal.jsonl. Replay only the matching completed prefix; reconcile any incomplete external effect as interrupted(host_exit) and never auto-retry it. Route workflow effects through Neo canonical dispatch and permission owners, launch only through an exact one-shot /workflow capability, keep Session JSONL and TUI as projections, and require every started child to reach a terminal journal outcome before the workflow becomes terminal.

Recovered resumable workflows prewarm the same session-scoped canonical dispatch owner without starting a worker. If those dependencies are unavailable, resume returns the run to an inspectable Paused state instead of creating a second dispatch path. If a completed invocation outcome exceeds its journal limit, the compact ResourceLimited outcome may replace oversized summary/details but must preserve actual provider usage and canonical child/task references. An unrecoverable journal terminalization failure clears the worker and current invocation and emits an unsequenced recovery-failure projection; it does not retry the external effect or claim a durable terminal record.

## Alternatives Considered

- Retain the foreground recorder and synchronous WorkflowResult; rejected because it leaves no durable lifecycle owner and cannot support safe host-exit recovery.
- Serialize and restore Lua VM execution state; rejected because VM snapshots are brittle, platform-sensitive, and do not solve external-effect identity.
- Replay incomplete tool effects after restart; rejected because Delegate, Swarm, and Bash effects are not generally idempotent and could be duplicated.
- Dispatch directly through a workflow-local tool registry; rejected because it would bypass canonical permission, instruction, provider, and session owners.

## Consequences

- Workflow runs are durable background tasks with explicit human/model pause, resume, and stop control; recovery may require a linked new run when source or arguments change, and interrupted effects need explicit operator judgment.
- The journal format, replay identity, and terminal-child invariant become durable contracts that future schema changes must migrate deliberately.

## Compatibility Boundary

Preserve existing Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal behavior and card designs; preserve historical Session JSONL projection compatibility. Do not add predictive token, cost, time, or agent governance. Cross-platform persistence uses Path/PathBuf and platform-gated directory sync.

## Retirement Impact

Retire foreground execution, WorkflowHostRecorder, run_script, host_api.rs, direct child_tools.run dispatch, background-mode and max-concurrency model fields, aliases, and duplicate workflow state owners without compatibility fallbacks. Existing persistent sessions and workflow artifacts remain readable projections and are not deleted.

## Baseline Sync

- Needed: needed
- Target: docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md
- Action: create snapshot
- Reason: The decision establishes the workflow ownership map, durable source of truth, replay and recovery contract, compatibility boundary, and retirement state.

## Evidence References

- docs/aegis/specs/2026-07-20-runworkflow-dynamic-workflow-design.md
- docs/aegis/plans/2026-07-20-runworkflow-dynamic-workflow.md
- docs/aegis/work/2026-07-22-runworkflow-audit-repair/90-evidence.md
- crates/neo-agent-core/src/workflow/runtime.rs
- crates/neo-agent-core/src/workflow/journal.rs
- crates/neo-agent-core/src/runtime/workflow_dispatch.rs

## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
