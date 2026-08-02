# Instruction Body Reuse

## Goal

Avoid appending the same fully expanded instruction body more than once when
different applicable scopes have identical expanded content, while preserving
scope paths, activation order, active-state metadata, and append-only context
history.

## Boundary

- Canonical owner: `InstructionRegistry` model-content rendering.
- Scope identity remains `(scope path, expanded revision)` for reconciliation.
- Body reuse is keyed by the existing expanded-content revision, not by file
  name or raw source text.
- Different expanded revisions still append their complete bodies.
- The outer Codex tool preflight is outside this repository and is unchanged.

## Implementation

1. Add a renderer helper that determines whether an expanded revision is
   already retained by any scope in `AgentInstructionState`.
2. Use that helper when rendering admitted bodies and when recording
   `body_revisions`, so persisted state matches the body actually emitted.
3. Keep every admitted scope in `instruction_active_state`; metadata-only
   epochs remain model-visible when a new scope reuses an existing body.

## Verification

- Same expanded body at workspace and nested scopes emits one revision body and
  both active scope entries.
- Different nested body emits both bodies.
- Existing unchanged-fingerprint, removal, reactivation, and compaction tests
  remain valid.
- Run the exact `neo-agent-core` instruction registry test target, formatting,
  and `git diff --check`.

## TDD Route

- Mode: off
- Decision: skipped
- Test posture: post-change regression
- Reason: the user approved a focused repair; strict test-first mode was not
  requested.
