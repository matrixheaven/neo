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

## Follow-on Remediation

- `TaskAnswer.answer` accepts any JSON value rather than object-only input.
- `TaskAnswer` delegates authorization to the runtime request's
  `human`/`human_or_model` policy and remains root-only with `Workflow`.
- Every `TaskOutput` workflow view exposes actionable `pending_user` fields;
  human-only gates say `next_action: wait_for_human`, while model-allowed gates
  provide the exact `TaskAnswer(task_id, request_id, answer)` action.
- Pending user input is cached and restored from the durable journal after
  restart.
- High-confidence one-off evaluation routing rejects pre-validation exploration
  and mixed tool batches before any side effect. Create/save and known-saved
  `run_saved` requests remain legal.
- `create-workflow` and user docs teach direct per-item `neo.swarm`, canonical
  `details.structured_output`, and collect-before-mark immutable JSON arrays.
- Builtin `deep-research`, `code-review`, and `large-refactor` workflows and
  fixtures now consume the production structured-output shape; manifests pin
  the corrected source bytes.

### Focused Regression Evidence

Fresh exact tests passed on macOS aarch64:

- `runtime::tool_dispatch::tests::workflow_route_blocks_mixed_skill_batches_and_prevalidation_exploration`
- `skills::builtin::tests::create_workflow_builtin_is_auto_invokable_host_reference`
- `available_skills_prompt_prioritizes_task_specific_skills_over_generic_methods`
- `task_answer_schema_accepts_scalar_array_and_object_answers`
- `workflow_tool_is_root_only_and_run_workflow_is_gone`
- `await_user_releases_permits_and_survives_restart`
- `task_answer_adapter_uses_runtime_model_policy`
- `task_output_exposes_actionable_pending_request_without_journal_view`
- `deep_research_builtin_fixture`
- `code_review_builtin_is_read_only_and_findings_first`
- `large_refactor_builtin_requires_explicit_merge_decision`

Final validation also passed:

- `cargo fmt --all --check`
- `git diff --check`
- `cargo build -p neo-agent` (existing macOS `mlua` minimum-version linker
  warning only; zero errors)
- stale scans found no active capability/nonce/authorization-mode contract and
  no old builtin `details.findings`/`details.risks` consumers; `RunWorkflow`
  remains only in a negative registry assertion.

### Audited Real Sessions

- `session_d99ac5cf-a4be-480f-8827-9b29507375b6` strictly began
  `Skill(create-workflow) -> Workflow(validate_inline) ->
  Workflow(run_inline) -> TaskOutput`. Its durable journal records
  `user_input_requested(req_c1) -> awaiting_user -> TaskAnswer(req_c1) ->
  completed`.
- `session_b2025de1-1871-427a-bfe6-6724f9ecfb08` attempted `List` after skill
  activation. Runtime rejected it before execution with `status=blocked`,
  `side_effect_occurred=false`, and `No tool executed`, then the model entered
  the correct validate/run/output path. Later assistant CLI use means this is
  not a strict end-to-end acceptance session.
- Model-generated reports under `.tmp` conflict with canonical journals on run
  counts and failure causes; they are not acceptance authority.

### Stale Scan
Zero active source references: WorkflowCapability, LaunchAuthorizationMode, launch_nonce, "Use the exact /workflow slash command first". One RunWorkflow absence assertion remains.

### Fresh Three-Session Black-Box — Pending

The exact three-session Chinese acceptance request has not produced three
consecutive strict sessions. One strict entry session and one zero-side-effect
correction session are audited above, but they do not satisfy the approved
three-session gate. Fresh acceptance remains pending and must record three real
sessions demonstrating `Skill(create-workflow) -> Workflow(validate_inline) ->
Workflow(run_inline) -> TaskOutput -> report` without assistant CLI or
capability routing.

### Focused Acceptance Status
| # | Scenario | Status |
|---|----------|--------|
| 1 | Manual skill activation | Pending fresh session evidence. |
| 2 | Create-only -> offer run | Pending fresh session evidence. |
| 3 | Create-and-test -> save -> run_saved | Pending fresh session evidence. |
| 4 | Known saved discovery | Pending fresh session evidence. |
| 5 | Implementation-debug request | Pending fresh session evidence. |
| 6 | Bare /workflow authoring | Focused integration-test evidence recorded above; not a fresh black-box session. |
| 7 | Named /workflow launch | Focused integration-test evidence recorded above; not a fresh black-box session. |
| 8 | Headless CLI | Focused integration-test evidence recorded above; not a fresh black-box session. |
| 9 | Root vs child policy | Focused policy-test evidence recorded above; not a fresh black-box session. |

### ADR/Baseline
- `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
- `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`

### Residual Risk
- The exact fresh three-session black-box acceptance remains pending. Current
  model evidence proves one strict entry and one fail-closed correction only;
  it is insufficient to generalize three-run consistency.
- No native Windows/Linux model-behavior sessions exist; the tool contract is
  platform-independent but model routing has not been demonstrated there.
