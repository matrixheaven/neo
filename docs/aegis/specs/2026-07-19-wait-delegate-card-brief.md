# WaitDelegate Card Spec Brief

Status: Approved
Date: 2026-07-19

## Goal

Replace the generic `Using WaitDelegate` transcript row with a compact card
that tells the user what is being awaited and how the wait ended.

## Presentation

- While pending or running, the header shows `Waiting for N delegates`, the
  global timeout, and live elapsed time. The collapsed card remains one row.
- Expanding a running card shows the requested IDs in input order.
- `all_terminal` shows the terminal count and any failed, cancelled, or
  delegate-timed-out counts.
- `wait_timed_out` shows terminal progress and the number still running.
- `not_found` shows the number of unknown targets.
- A finalized card lists target titles when available, otherwise IDs, together
  with their terminal/current status. The collapsed preview uses the existing
  tool-card row limit and expansion affordance.
- Runtime elapsed time is not persisted, so replayed finalized cards do not
  invent or display a completion duration.

## Boundaries

- Reuse `ToolCallComponent` and the existing tool renderer; do not add a card
  type, runtime event, or result field.
- Read running state from existing tool arguments and finalized state from the
  existing `delegate_wait` details envelope.
- Do not change the runtime `WaitDelegate` contract or any Delegate,
  DelegateGroup, or DelegateSwarm card.
- Do not render the raw `kind`, `outcome`, `aggregate`, and `items` result text
  when structured wait details are available.

## Acceptance

- A running four-target wait is distinguishable from a generic tool call.
- Completed, partial-timeout, and not-found outcomes have distinct headers.
- Final target rows preserve input order and expose meaningful statuses.
- Narrow terminals truncate safely and `ctrl+o` retains the existing expansion
  behavior.
