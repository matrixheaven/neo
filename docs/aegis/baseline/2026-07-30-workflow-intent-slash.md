# Workflow Intent Slash Entry - Landed Baseline

Status: `recorded-from-adr`
Date: `2026-07-30`
ADR: `docs/aegis/adr/ADR-0009-workflow-intent-slash-entry.md`

This baseline records the current interactive workflow entry point after the
intent-slash redesign.

## Product Baseline

- `/workflow` opens a searchable picker and only fills `/workflow:<name> ` on
  selection; it does not start a model turn.
- `/workflow <task>` sends the original slash text as a visible user turn and
  gives the model the complete effective workflow catalog.
- `/workflow:<name> <task>` sends the original slash text as a visible user
  turn and gives the model the resolved definition and full input schema.
- Missing input, discovery failures, unknown names, and busy turns preserve
  the composer and start no model or workflow task.
- Existing workflow use does not activate `create-workflow`; automatic no-match
  guidance asks before authoring.

## Ownership Baseline

- `WorkflowDefinitionRegistry` is the only discovery and resolution owner.
- The model selects and maps; the existing `Workflow` tool executes.
- TUI picker/composer code owns selection and local display state.
- Workflow guidance is one-turn system context; the exact slash user message is
  the only user transcript input persisted for that request.
- Complete-or-error capacity uses the existing context estimator. New-session
  capacity rejection happens before session creation.

## Retirement Baseline

Active source no longer contains the former host-direct named/JSON launcher,
space-form generated launch values, or static built-in-only completion source.
No compatibility alias was retained. Workflow definitions, sessions, journals,
cards, task controls, Lua execution, authoring, permissions, and headless CLI
were not removed or migrated.

## Verification

Focused macOS tests cover parser/catalog/context, picker selection, automatic
and named model turns, local and busy errors, exact replay, no-match authoring
guidance, capacity rejection before session creation, and the combined picker
to workflow-event path. Retirement scans, touched-file formatting, and
whitespace checks are recorded in the linked work evidence.

## Residual Risk

- Windows/Linux native tests and remote CI were not run in this workspace.
- The integration test uses a deterministic fake model/event driver; a live
  provider session is not claimed.
