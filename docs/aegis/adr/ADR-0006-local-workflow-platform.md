# ADR-0006 - Local workflow platform (definitions, V2 journal, host-direct launch)

Status: `recorded-from-work`
Date: `2026-07-26`

## Source Evidence

- Workflow platform Tasks 1-25 landed under docs/aegis/plans/2026-07-25-workflow-platform.md; native platform tests passed on macOS, Fedora Linux aarch64, and Windows 11 ARM64 at commit base b088a6db plus Task 25 test files.

## Context

ADR-0004 established durable RunWorkflow with WorkflowRuntime as sole lifecycle owner, append-only journal replay, and retirement of foreground recorder paths. The approved P0-P2 platform expands that substrate into reusable paired definitions, V2 journal recovery, content-addressed artifacts, heterogeneous swarm, named host-direct /workflow launch with zero model calls, operator surfaces, and builtins — while keeping WorkflowRuntime the sole durable owner and Lua the only engine.

## Decision

Ship a local-only workflow platform on the durable RunWorkflow substrate: WorkflowRuntime remains sole durable state/result owner; registry owns only trusted definitions (builtin<user<trusted project, paired .lua+.workflow.toml, exact SHA-256 revision framing); coordinator is stateless; named /workflow is host-direct with zero model calls; final and every child require output_schema; exactly one tools-disabled schema repair; neo.tool uses canonical ToolRegistry plus one deny classifier; heterogeneous swarm with no MAX_SWARM_CHILDREN or arbitrary total cap; AwaitingUser is durable and independent; V1 is read-only linked upgrade only; global admission uses actual occupancy only (no token_cap prediction); /tasks is extended not duplicated; shell admission stays pending/unbounded; cross-platform Path/PathBuf with native path/link/sync/replace proof.

## Alternatives Considered

- Keep only ad-hoc RunWorkflow scripts without a definition registry — rejected because operators need reusable, trust-gated, revision-pinned definitions.
- Add a second script engine (Rhai) or engine trait/factory — rejected; Lua-only is the closed decision.
- Predictive token/agent/time caps and MAX_SWARM_CHILDREN hard limits — rejected; admission is actual occupancy and physical storage only.
- In-place V1 journal/session migration or dual durable owners — rejected; V1 remains read-only linked upgrade; no second owner for state/result/registry/schema/tool/child/answer.

## Consequences

- Named workflows, headless CLI, /tasks dashboard, and builtins share one launch and durability contract; torn-tail recovery quarantines invalid EOF suffixes before truncate; artifacts are content-addressed and integrity-revalidated; operators must explicitly answer, prune, and handle linked upgrades.

## Compatibility Boundary

Preserve Delegate/DelegateGroup/DelegateSwarm/Bash/Terminal tools and card designs unchanged. Historical ADR-0004 and the 2026-07-23 baseline remain historical records. V1 runs stay readable projections with linked upgrade only. No hosted marketplace, no unsafe Neo code, no shell admission/timeout semantic change.

## Retirement Impact

Retire active writers for WorkflowHostRecorder, run_script, host_api, child_tools.run, model token_cap/max_concurrency governance, MAX_SWARM_CHILDREN, dual engines/engine abstractions, and mode=background launch. Documentation may mention retired names only as rejected keys or historical notes.

## Baseline Sync

- Needed: needed
- Target: docs/aegis/baseline/2026-07-26-workflow-platform-contract.md
- Action: create snapshot
- Reason: Platform closeout records only landed behavior plus native macOS/Linux/Windows evidence for path/link/sync/replace contracts.

## Evidence References

- docs/aegis/specs/2026-07-25-workflow-platform-design.md
- docs/aegis/plans/2026-07-25-workflow-platform.md
- crates/neo-agent-core/tests/workflow_registry.rs
- crates/neo-agent-core/tests/workflow_journal_v2.rs
- crates/neo-agent-core/tests/workflow_artifacts.rs

## Supersedes

- ADR: docs/aegis/adr/ADR-0004-durable-runworkflow-runtime.md
- Reason: ADR-0004 recorded the durable RunWorkflow substrate. The platform expansion has landed with registry, V2 recovery, artifacts, host-direct launch, operator surfaces, builtins, and native cross-platform verification; current authority is the platform contract baseline, not the substrate-only ADR.

## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
