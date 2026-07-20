# Append-Only Skill Catalog Spec Brief

## Goal

Keep the provider-visible session prefix stable when Neo restarts or the live
skill catalog changes.

## Contract

- The fixed system prompt never contains `<available_skills>`.
- `SkillStore` renders one deterministic full catalog ordered by source
  (`User`, `Extra`, `Builtin`) and then canonical skill name.
- Before the user message, Neo appends the current catalog as a persisted
  `MessageOrigin::Injection("available_skills")` system reminder.
- Replay restores that message in wire order. An unchanged catalog appends
  nothing. Any addition, edit, or removal appends one complete replacement
  snapshot containing `DISREGARD any earlier skill listings`.
- Empty catalogs are explicit snapshots so removing the final skill retires
  earlier listings.

## Compatibility

Existing sessions remain resumable without prompts or migration. Their first
turn after this change may reset the provider cache once because the old
randomized system-prompt catalog was never persisted; subsequent turns follow
the append-only contract.

## Non-Goals

- MCP tool-schema deltas.
- AGENTS.md instruction epochs.
- A generic dynamic-context framework.

## Acceptance

- Catalog rendering is deterministic.
- The first turn appends one catalog reminder before the user message.
- An unchanged later turn appends no duplicate.
- Adding, editing, removing, or removing all skills appends one full snapshot.
- The system prompt contains no skill catalog.
