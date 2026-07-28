# Edit Match-Mismatch Compact Recovery Spec Brief

Status: approved 2026-07-28

This brief supersedes
`docs/aegis/specs/2026-07-25-edit-mismatch-readback-brief.md`.

## Goal

Stop injecting automatic file snapshots into failed `Edit` results. Snapshot
content is new model input that cannot rely on a cache hit; the observed
automatic readback averaged about 8.85k estimated tokens per mismatch.

## Contract

When `actual_matches != expected_matches`, `Edit` returns compact diagnostics
only:

- zero writes were committed;
- the affected path and edit index;
- expected and actual exact-match counts;
- exact match line numbers when matches exist;
- guidance to use `Grep` on a distinctive fragment or `Read` the smallest
  relevant range before submitting a fresh complete `Edit` call.

The failed result must not include file contents, comparison snapshots, an
automatic `Read`, or an automatic `Grep`. The AI chooses the next inspection
tool and therefore controls the returned context size.

Strict exact matching, staged edit ordering, approval, stale detection, and
zero-write mismatch behavior remain unchanged.

## Retirement

- Deletion class: internal code retirement.
- Old path: automatic bounded comparison snapshots on match-count failure.
- Canonical recovery path: compact `Edit` diagnostics followed by an explicit
  AI-selected `Read` or `Grep` call.
- Decision: delete-first; no compatibility branch or fallback snapshot.
- External boundary and source-of-truth data risk: none.

## Non-Goals

- automatic fragment selection, fuzzy matching, or similarity scoring;
- automatic retry or automatic `expected_matches` changes;
- new configuration, token budgets, or adaptive readback modes;
- changes to Edit input schema, commit semantics, or finalized TUI card layout.

## Acceptance

1. A `found 0` mismatch returns compact diagnostics and no file body.
2. A `found > 0` mismatch preserves exact match line numbers and returns no
   surrounding file body.
3. Recovery guidance names explicit, narrow `Read` or `Grep` as the next step.
4. The automatic snapshot implementation and its snapshot-specific tests have
   no remaining production references.
5. Existing strict exact-match and zero-write behavior remains intact.
