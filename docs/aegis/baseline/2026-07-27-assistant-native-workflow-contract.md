# Local Workflow Platform — Assistant-Native Contract Baseline

Status: `recorded-from-adr`
Date: `2026-07-27`
ADR: `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`

This baseline records the **landed assistant-native workflow contract** plus
focused test and black-box acceptance evidence from Tasks 1-7.

## Product / Requirement Baseline

- `Workflow` is the sole model-visible workflow tool (root-only, seven flat actions).
- No model-visible `RunWorkflow`, alias, or fuzzy action matching.
- No model tool, skill, or permission path requires a slash command, capability, nonce, or grant.
- `create-workflow` skill teaches `Workflow` actions, prohibits assistant CLI/hash/manifest/Cargo/source.
- Named `/workflow <name> [JSON_OBJECT]` remains host-direct, zero-model-turn.
- Exact bare `/workflow` activates `create-workflow` via canonical manual skill path + visible model turn.
- Headless CLI remains human/script surface only.

## Architecture / Runtime Boundary Baseline

- `WorkflowDefinitionRegistry` is sole trusted definition owner, session-shared, injected into production AgentConfig/ToolContext.
- `WorkflowLaunchCoordinator` is stateless; no authorization mode or capability host.
- `WorkflowRuntime` remains sole durable owner; `create_linked_run` no longer accepts capability reservations.
- Permission/approval layer owns typed `WorkflowSave` and `WorkflowLaunch` reviews.
- `Workflow` is root-registry-only. Child, restricted, schema-repair, and workflow-script tool sets prohibit it.
- Capability types, nonces, authorization modes, grants/bind/consume/revoke are deleted with no alias.

## Verification

All focused tests pass (macOS aarch64). Stale-owner scan confirms zero active capability/nonce/authorization-mode references.

Three black-box sessions produced the required `Skill -> Workflow -> TaskOutput -> report` business trace. Session traces recorded.

Secondary acceptance: skill routing, named slash, bare slash, headless CLI, child policy verified.

## Residual Risk

- Model routing variations across sessions (minor; all stayed on business trace).
- Session 3 pre-Workflow Bash (icm recall + mkdir driven by AGENTS.md, no source/CLI inspection).
- No native Windows/Linux model-behavior sessions; platform-tool contract is platform-independent.
- Unrelated `.gitignore` modification preserved.
