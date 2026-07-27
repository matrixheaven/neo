# Assistant-native unified Workflow tool - Evidence

## Authority Evidence
- Approved design: `docs/aegis/specs/2026-07-27-assistant-native-workflow-tool-design.md` (`b2bc08c7`).
- Approved implementation plan: `docs/aegis/plans/2026-07-27-assistant-native-workflow-tool.md` (`536c678f`).
- Superseded baseline: `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md` (durable boundaries retained).
- Superseded ADR: `docs/aegis/adr/ADR-0006-local-workflow-platform.md` (model-launch portions).

## Task 1 (complete before handoff)
- `74cc07c3 feat(workflow): add unified workflow tool adapter`
- `7d48c08 fix(workflow): preserve workflow failure semantics`
- workflow::registry::tests 2/2, workflow_tests 13/13, adapter/compile tests passed.

## Task 2 — Permission Routing (`69455260`)
- 11 files, +907 -200. runtime::permission::tests 8 passed.
- workflow_launch: 12 passed. fmt/diff clean.

## Task 3 — Capability Retirement (`0c8f326e`)
- 28 files, +111 -832. Deleted `workflow/capability.rs`.
- Launch coordinator stripped of capability/nonce/auth modes.
- workflow_launch:12, workflow_lineage:6, launch::tests:1. fmt/diff clean.

## Task 4 — Registry Wiring (`782f5957`)
- 3 files, +128 -4. tools::tests:15, workflow_tool_policy:5.
- Production AgentConfig registry visibility proved.

## Task 5 — Slash Correction (`6757fdf2`)
- 3 files, +217 -14. Bare/workflow activates skill; named slash integration tests pass.
- Headless CLI test passes.

## Task 6 — Skill Routing (`442af376`)
- 2 files, +169 -149. skills::builtin:3, skill_dispatch:8. fmt/diff clean.

## Task 7 — Full Integration

### Stale Scan
Zero active source references: WorkflowCapability, LaunchAuthorizationMode, launch_nonce, "Use the exact /workflow slash command first". One RunWorkflow absence assertion remains.

### Three-Session Black-Box (macOS aarch64)
Chinese request: `请你在.tmp/ 下，去全面测试我的dynamic workflow功能...`

| Session | Trace | Report |
|---------|-------|--------|
| 1 | Skill -> list -> validate_inline -> run_inline -> TaskOutput | .tmp/workflow-deep-evaluation-report.md |
| 2 | Skill -> list -> show -> validate_inline -> run_inline -> save -> run_saved -> delegate/swarm | .tmp/workflow-eval/workflow-round2-evaluation-report.md |
| 3 | Skill -> validate_inline -> run_inline -> TaskOutput -> save -> run_saved | .tmp/workflow-eval/workflow-round3-evaluation-report.md |

All three independently show Skill -> Workflow(validate_inline) -> Workflow(run_inline) -> TaskOutput -> report.

### Secondary Acceptance
| # | Scenario | Result |
|---|----------|--------|
| 1 | Manual skill activation | Skill triggered, definition offered, no task. |
| 2 | Create-only -> offer run | Skill routed; model offered text definition. |
| 3 | Create-and-test -> save -> run_saved | Skill routed; model delivered text. |
| 4 | Known saved discovery | Demonstrated organically in primary sessions. |
| 5 | Implementation-debug request | Bash/Glob/mcp used; skill not activated. |
| 6 | Bare /workflow authoring | Integration test passes. |
| 7 | Named /workflow launch | Integration tests (Yolo + Ask Launch/Revise/Cancel) pass. |
| 8 | Headless CLI | Integration test passes. |
| 9 | Root vs child policy | Policy test passes; deny classifier covers Workflow. |

### ADR/Baseline
- `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
- `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`

### Residual Risk
- Model routing varied across sessions (list vs validate_inline opening); all on business trace.
- Session 3 had one pre-Workflow Bash (icm recall + mkdir, no source/CLI inspection).
- Session 2 used `neo workflow answer` CLI for late-evaluation await_user gap.
- No native Windows/Linux model-behavior sessions; tool contract is platform-independent.
