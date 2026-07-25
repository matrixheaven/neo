# Delegate Tool Activity Summary And Theme Spec Brief

Status: Approved
Date: 2026-07-25
ArchitectureReviewRequired: no

## Goal

Make collapsed DelegateSwarm child rows identify the file currently worth the
user's attention without increasing their one-line height, and restore the
existing Neo theme semantics to Delegate-family child tool activity.

This extends, but does not replace,
`2026-07-24-delegate-edit-write-file-activity-brief.md`. Detailed Delegate,
DelegateGroup, and expanded DelegateSwarm activity continues to show the full
ordered file list.

## Approved Presentation

```text
Single file
• Used Edit · M crates/neo-tui/src/transcript/tool_call.rs · +6 -14

Multiple files completed
• Used Edit · 3 files · M crates/neo-tui/src/a.rs · total +16 -20

Partial failure
✗ Failed Write · 3 files · M docs/existing.md · permission denied

Running
• Using Edit · 3 files · … crates/neo-tui/src/a.rs
```

The examples show activity content only. Existing card prefixes, progress,
indentation, ordering, height, and expansion controls remain unchanged.

## Collapsed Multi-File Contract

1. A collapsed DelegateSwarm child activity remains exactly one visual row.
2. Edit/Write activity with one file shows that path directly and omits the
   redundant `1 file` count.
3. Edit/Write activity with multiple files shows the total file count and one
   stable representative file.
4. Representative selection is deterministic and risk-first:
   `Failed` -> `CommittedUnsynced` -> `Pending` -> first file in canonical
   declaration/result order.
5. Completed aggregate additions/removals are summed from structured file
   fields. A multi-file aggregate is prefixed with `total` so it cannot be
   mistaken for the representative file's own statistics.
6. Do not show aggregate additions/removals when any required structured value
   is absent or the activity is still pending.
7. Failure diagnostics belong to the representative failed file. Do not append
   diagnostics from a different file.
8. Normal width truncation may shorten the tail of the one-line preview. It
   must preserve valid styling and must not replace the expanded full file
   list, which remains the inspectable source for every path.
9. Never rotate or animate representative files. Repeated frames for unchanged
   state must be stable.

## Theme Contract

Delegate, DelegateGroup, collapsed DelegateSwarm, and expanded DelegateSwarm
must use the same semantic styles for child tool activity:

| Segment | Theme style |
|---|---|
| lifecycle marker and `Using`/`Used`/`Failed`/`Queued` verb | phase/status color |
| tool name | `theme.brand`, bold |
| separators, counts, elapsed text, and `total` label | `theme.text_muted` |
| normal path | `theme.text_primary` |
| pending marker | `theme.status_pending` |
| failed marker/diagnostic | `theme.status_error` |
| durability-uncertain marker | `theme.status_warn` |
| created marker and positive count | `theme.diff_added` |
| removed count | `theme.diff_removed` |
| modified marker | `theme.diff_hunk` |
| not-attempted marker | `theme.text_muted` |

The renderer must produce styled `Span` values directly. It must not embed ANSI
codes in strings or parse a rendered summary to recover semantic segments.

## Ownership

```text
AgentActivityKind::Tool { name, summary, phase, files }
                         |
                         v
child_activity semantic status/file span builder
                |                         |
                v                         v
Delegate/Group/expanded Swarm      collapsed Swarm summary
```

`child_activity.rs` remains the shared semantic presentation owner.
`swarm_card.rs` may select the current child activity and compose its existing
one-line prefix, but it must not duplicate Edit/Write file selection, totals,
or style rules.

## Boundaries

- Do not change Edit/Write schemas, results, runtime projection, events, or
  persistence.
- Do not change Delegate-family card headers, hierarchy, row count, progress,
  lifecycle wording, expansion controls, or transcript placement.
- Do not add a new renderer module, card type, theme field, configuration, or
  fallback summary parser.
- Do not inline all paths into the collapsed row.
- Non-Edit/Write summaries retain their current text and width behavior, with
  semantic theme styling restored.

## Acceptance

1. Collapsed DelegateSwarm single-file Edit/Write rows include the path.
2. Collapsed multi-file rows include total count, risk-first representative
   path, and truthful aggregate `total +N -N` only when complete.
3. Failed and durability-uncertain files outrank successful files as the
   representative.
4. Delegate and DelegateSwarm child tool verbs and names use status/brand
   styles instead of terminal-default white.
5. Detailed file rows use semantic marker/stat/diagnostic colors while keeping
   the full ordered list and wrapping behavior from the parent spec.
6. Non-Edit/Write text, Delegate-family structure, and expanded file identity
   coverage remain unchanged.
7. A focused test inspects both visible text and `Span` styles using a custom
   theme, covering single-file, multi-file, partial failure, Delegate, and
   collapsed/expanded DelegateSwarm rendering.

## Design Trace

- Requirement source: user-approved 2026-07-25 character layouts and
  multi-file representative rule.
- Canonical owner: existing `child_activity.rs` presentation seam.
- Change necessity: a plain string cannot preserve semantic theme segments,
  and the collapsed row currently discards the typed file projection.
- Compatibility: presentation-only; no serialized or model-visible contract
  changes.
- Complexity: reuse one span builder and existing typed file data; no new
  module or abstraction layer.
- ADR signal: none. This is a localized presentation correction inside an
  existing owner boundary.
