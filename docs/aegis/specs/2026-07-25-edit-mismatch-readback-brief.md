# Edit Match-Mismatch Readback Spec Brief

Status: superseded by
`docs/aegis/specs/2026-07-28-edit-mismatch-compact-recovery-brief.md`

## Goal

Let an AI correct an `Edit` match-count failure without spending a separate
`Read` call when the comparison evidence fits in the existing Read limits.

## Contract

When `actual_matches != expected_matches`, the failed model-visible
`ToolResult.content` includes the exact text snapshot used for that comparison.
For later edits to the same file, this is the staged text after earlier items in
the same call; the result must still state that the call committed zero writes.

The snapshot uses Read's existing safety and output rules:

- at most 1000 leading lines;
- at most 100 KiB of rendered output;
- at most 2000 characters per rendered line;
- the same line numbers, line-ending notices, sensitive-path refusal, and NUL
  refusal as Read.

If every file line is not present, the result must explicitly state:

- the included line range;
- the total file line count;
- the exact number of remaining unread lines;
- the next `Read.line_offset` needed to continue.

The recovery guidance tells the model to correct `edits[index].old` from the
returned evidence and resubmit the complete Edit call. It must not imply that
earlier staged replacements were committed.

## Non-Goals

- fuzzy matching or automatic retry;
- changing `expected_matches` automatically;
- changing Edit input schema, approval, commit, or stale semantics;
- embedding snapshots for non-match preparation failures;
- changing finalized Edit or Delegate-family card layouts.

## Acceptance

1. A small-file mismatch returns line-numbered comparison text in the failed
   Edit result and requires no Read instruction.
2. A large-file mismatch reports that only the leading prefix was included,
   gives the exact remaining line count, and names the next Read offset.
3. A later same-file mismatch identifies staged comparison state while the
   underlying file remains unchanged.
4. Existing strict exact-match and zero-write behavior remains intact.
