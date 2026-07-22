# ADR-0002 - Exact AGENTS.md Discovery and Non-Blocking Import Cycles

Status: `recorded-from-plan`
Date: `2026-07-22`

## Source Evidence

- Approved design, implemented resolver diff, three passing neo-agent-core regressions, and independent code review
## Context

Case-folded AGENTS.md discovery misclassified ordinary agents.md documentation, while recursive Markdown imports treated cycle back-edges as fatal even though duplicate canonical sources were already deduplicated.

## Decision

Recognize only the exact stored AGENTS.md directory entry and expand each canonical instruction source once; repeated and cyclic edges are non-blocking, while other atomic import validation remains strict.

## Alternatives Considered

- Keep case-insensitive discovery and special-case documentation paths; rejected as a caller-side exception that preserves false positives.
- Build a full graph and condense strongly connected components; rejected because the existing canonical visited set already provides deterministic termination.
- Keep cycles blocking; rejected because a back-edge is semantically the same already-visited source as an ordinary duplicate import.
## Consequences

- Lowercase and mixed-case agents.md files are ordinary documents, cyclic instruction graphs terminate deterministically, and first discovery still emits the normal successful instruction epoch before tool execution.
## Compatibility Boundary

Historical include_cycle and ambiguous_agents_file failure-kind values remain deserializable from existing JSONL sessions; no new runtime path produces them.

## Retirement Impact

Remove case-folded selection, ambiguity production, recursion-stack cycle errors, and their current failure documentation; retain only historical serialized failure-kind variants.

## Baseline Sync

- Needed: needed
- Target: docs/aegis/baseline/2026-07-22-instruction-import-contract.md
- Action: create snapshot
- Reason: The change replaces the current instruction discovery and import behavior contract, and no instruction baseline snapshot exists.

## Evidence References

- docs/aegis/specs/2026-07-22-non-blocking-instruction-cycles-design.md
- docs/aegis/plans/2026-07-22-non-blocking-instruction-cycles.md
- crates/neo-agent-core/src/instructions/resolver.rs

## Supersedes

This ADR supersedes only the case-insensitive filename matching, import-cycle
failure, and case-fold ambiguity clauses in
`docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`. The
remaining path-scoped instruction architecture stays authoritative.

## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
