# Non-Blocking Instruction Import Cycles

Status: `approved`
Date: `2026-07-22`
ArchitectureReviewRequired: `yes`
Supersedes: filename matching, cycle handling, and ambiguity-failure clauses in
`docs/aegis/specs/2026-07-17-path-scoped-agents-instructions-design.md`

## Goal

Instruction discovery must recognize only deliberately named `AGENTS.md`
files. A real cyclic import graph must terminate deterministically, load every
reachable canonical source once, and never block the AI's tool workflow merely
because an import edge points back to an already visited source.

## Root Cause

Two contracts composed into a false-positive blocking scope:

- case-insensitive filename discovery treated an ordinary `agents.md` product
  document as a nested instruction root; and
- local Markdown links recursively expanded the documentation navigation graph
  until a repeated on-stack source produced `IncludeCycle`.

Cycle failure is also inconsistent with the resolver's existing duplicate
import rule: both cases target an already visited canonical source, but only the
on-stack duplicate is fatal.

## Canonical Contract

1. The only instruction entry filename is the exact directory entry
   `AGENTS.md`. Stored directory-entry casing is authoritative on every
   supported platform. Lowercase or mixed-case documentation files are not
   instruction scopes.
2. Import expansion is canonical-source-once traversal. The first edge to a
   canonical Markdown source expands it in place; every later edge to the same
   canonical source, including a cycle back-edge, preserves the link where
   applicable but inserts no second copy of the source body.
3. Cycles are not failures, warnings, retries, or partial-bundle recovery. They
   are ordinary repeated edges governed by the existing `visited` set.
4. Missing, unreadable, invalid UTF-8, untrusted, unstable, and structurally
   oversized sources remain atomic bundle failures.
5. A newly discovered successful scope still follows the normal epoch contract:
   defer the initiating tool batch once, inject the complete instruction epoch,
   then allow the model's reissued tool call to execute.

## Owner And Retirement

`InstructionResolver` remains the only owner of entry discovery and import
expansion. The implementation removes case-folded discovery, ambiguous
case-fold collision production, the recursion stack, and runtime
`InstructionError::IncludeCycle` production rather than adding a fallback.

Persisted session events may already contain serialized
`InstructionFailureKind::IncludeCycle` or `AmbiguousAgentsFile`. Those enum
variants remain historical read compatibility only so existing sessions can
rehydrate. New runtime resolution must not produce either failure kind.

Retirement decision: `delete-first` for internal discovery/cycle logic;
`compat-exception` only for the two serialized historical failure-kind values.
No live session data is rewritten or deleted.

## Non-Goals

- Do not stop importing local Markdown links.
- Do not make unrelated instruction errors fail-open.
- Do not parse Bash or Terminal command strings for instruction paths.
- Do not add graph, SCC, retry, fallback, or migration subsystems.
- Do not change instruction epoch, transcript card, compaction, or permission
  behavior.

## Acceptance

- An ordinary lowercase `agents.md` beneath a tool target is ignored as a
  scope, even when it contains cyclic local Markdown links.
- `AGENTS.md -> a.md -> b.md -> a.md` loads successfully and contains each
  canonical source body exactly once.
- A cyclic nested instruction scope produces a successful instruction epoch;
  after the normal one-time defer, a reissued mutation tool is not blocked.
- Existing fatal import validation remains covered and unchanged.
- Historical instruction failure events using the retired serialized variants
  still deserialize.

## ADR Signal

This changes the durable instruction discovery and import contracts. The
implemented decision is recorded by
`docs/aegis/adr/ADR-0002-non-blocking-instruction-import-cycles.md` and reflected
in the current instruction-contract baseline.
