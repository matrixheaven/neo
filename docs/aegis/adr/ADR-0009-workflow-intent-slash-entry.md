# ADR-0009 - Workflow Intent Slash Entry

Status: `recorded-from-work`
Date: `2026-07-30`

## Source Evidence

- Approved design: `docs/aegis/specs/2026-07-30-workflow-intent-slash-design.md`.
- Implementation plan: `docs/aegis/plans/2026-07-30-workflow-intent-slash.md`.
- Focused evidence: `docs/aegis/work/2026-07-30-workflow-intent-slash/90-evidence.md`.

## Context

The previous interactive workflow slash path resolved a named definition in
the host, translated the task into host-owned arguments, and launched the
workflow directly. That duplicated model selection and approval ownership,
and the bare form did not provide one clear distinction between using a saved
workflow and authoring a new one.

## Decision

Interactive workflow use has exactly three forms:

- `/workflow` opens a searchable picker backed by the effective registry.
- `/workflow <task>` submits the original slash text as a normal visible user
  turn with the complete effective catalog in one-turn system guidance.
- `/workflow:<name> <task>` submits the original slash text as a normal visible
  user turn with the resolved definition and full input schema in one-turn
  system guidance.

The ownership boundary is:

- `WorkflowDefinitionRegistry` discovers effective definitions and resolves a
  named definition.
- The model chooses the workflow and maps natural language to arguments.
- The existing `Workflow` tool executes the chosen action and owns its normal
  permission, task, card, recovery, and persistence behavior.
- The TUI owns picker selection and composer state only.

Workflow guidance is turn-local system context. The exact slash text remains
the visible and persisted user message. It does not create a Skill event or a
new session event. Complete catalogs are rejected before provider dispatch if
they exceed the selected model's existing context budget; a rejected new turn
does not create an empty session.

The former host-direct named/JSON slash launcher, space-form generated values,
and static workflow completion candidates are retired without a compatibility
alias. `/skill:create-workflow` remains the authoring path, including when an
automatic request has no matching saved definition and the model asks for
confirmation before authoring.

## Alternatives Considered

- Keep host-direct named launch with a new parser. Rejected: it preserves a
  duplicate selection and execution owner.
- Put the catalog in the visible user message or Skill context. Rejected: it
  changes the user transcript or misrepresents workflow use as authoring.
- Truncate oversized catalogs. Rejected: incomplete discovery can cause an
  incorrect model choice; the behavior is complete-or-error.
- Keep the old space-form route as a compatibility alias. Rejected: no active
  external dependency justifies the duplicate entry point.

## Consequences

- Users can choose a saved workflow through one discoverable picker or express
  automatic/named intent in a normal model turn.
- Existing Workflow actions, permissions, task controls, cards, recovery,
  headless CLI, and authoring capability remain unchanged.
- The former interactive slash syntax is intentionally incompatible and must
  be updated by callers that issued host-direct JSON launches.
- The workflow context preflight constructs a lightweight runtime before a
  new session is created, so oversized guidance fails without leaving empty
  session state.

## Compatibility Boundary

Preserve: saved definitions, effective registry precedence, the seven
`Workflow` actions, permission modes, workflow task/card projections, resume,
session history, authoring, Lua execution, and headless `neo workflow` CLI.

Retire: interactive host JSON mapping, direct slash launch approval state,
space-form named launch, and static built-in-only workflow completion.

## Baseline Sync

- Needed: `resolved`
- Target: `docs/aegis/baseline/2026-07-30-workflow-intent-slash.md`
- Action: current snapshot created and linked
- Reason: the canonical interactive entry point, context ownership, capacity
  boundary, and retired host path changed.

## Retirement Impact

The host-direct JSON slash launcher, space-form named launch, direct slash
approval state, and static workflow completion candidates are retired without
aliases. Saved definitions, authoring, headless CLI, and the assistant Workflow
tool remain active through their existing owners.

## Evidence References

- `modes::interactive::tests::bare_workflow_slash_opens_picker_and_selection_only_fills_composer`
- `modes::interactive::tests::automatic_workflow_slash_starts_visible_model_turn_with_complete_context`
- `modes::interactive::tests::named_workflow_slash_starts_visible_model_turn_with_full_schema`
- `modes::interactive::tests::workflow_intent_slash_end_to_end_selects_runs_and_persists`
- `modes::interactive::tests::workflow_intent_slash_no_match_asks_before_authoring`
- `modes::run::tests::oversized_workflow_catalog_starts_no_provider_call`
- `modes::run::tests::workflow_turn_context_is_system_role_and_user_slash_is_persisted_exactly`
- `modes::run::tests::workflow_runtime_dispatches_saved_run_from_model_tool_call`
- `modes::interactive::tests::idle_shell_mode_workflow_slash_returns_to_model_mode`

## Supersedes

This ADR supersedes only the interactive slash-entry sections of ADR-0007 and
ADR-0008. Those ADRs remain historical and authoritative for the runtime,
registry, tool, journal, task, and product decisions not changed here.

## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
