# Assistant-native unified Workflow tool - Reflection

## Completion Candidate

- Goal: remove the user-first launch prerequisite and make the complete
  workflow lifecycle assistant-native.
- Result: the canonical path is now `Skill(create-workflow) -> Workflow ->
  TaskOutput -> TaskAnswer when allowed`, with saved-workflow direct execution
  and human/script CLI kept as separate intentional surfaces.
- Deeper cause: no. The original defect was split authorization and launch
  ownership between model tools and a user-only slash capability. That owner is
  deleted; no compatibility alias or fallback remains.
- Complexity: bounded. Remediation reuses `WorkflowRuntime`,
  `WorkflowDefinitionRegistry`, permission, background task, and skill owners;
  it adds no engine, scheduler, registry, or state owner.
- Evidence: focused route, policy, schema, restart, TaskOutput, and builtin
  regressions pass. One strict real session reaches TaskOutput and completes a
  durable TaskAnswer gate; another proves pre-execution route correction.
- Residual risk: the approved three-consecutive-session black-box acceptance is
  still pending, and no native Windows/Linux model-behavior session exists.
- Decision: implementation-complete candidate, but model-consistency acceptance
  remains `needs-verification` and must not be reported as passed.

## Repair And Retirement

- Repair track: unified root `Workflow`, runtime-policy-gated root
  `TaskAnswer`, actionable `TaskOutput.pending_user`, and fail-closed one-off
  routing at the existing dispatch owner.
- Retirement track: `RunWorkflow`, capability/nonces, authorization modes,
  slash-first launch, assistant CLI fallback, and stale builtin output shapes
  are absent from the active path. Historical plans/ADRs retain names only as
  evidence.

Method Pack output does not grant completion authority.
