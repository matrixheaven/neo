# WaitDelegate Batch Wait Spec Brief

Status: Approved
Date: 2026-07-19

## Goal

Let one `WaitDelegate` call join any non-empty set of known delegate agents or
swarms. This removes repeated wait calls after a parallel `Delegate` batch
without turning the presentation-only `DelegateGroup` into a runtime entity.

## Contract

- The canonical input is `ids: Vec<String>`. The legacy singular `id` field is
  removed, including for a single target.
- `ids` must be non-empty and contain no duplicates. Each value must identify a
  delegate agent or swarm known when the wait begins.
- `timeout_ms` defaults to 30,000 ms and is one global deadline for the whole
  call. Waiting completes only when every target is terminal.
- The result always has one batch envelope with `kind: "delegate_wait"`, an
  `outcome`, an `aggregate`, and input-ordered `items`. Each item retains the
  canonical agent or swarm details available at return time.
- `outcome: "all_terminal"` means every target reached a terminal state,
  regardless of whether individual targets completed, failed, cancelled, or
  timed out.
- `outcome: "wait_timed_out"` means the global wait deadline elapsed. The
  result still includes terminal results already available and current
  snapshots for unfinished targets; completed information is never discarded.
- If any ID is unknown at admission, return immediately with
  `outcome: "not_found"` and snapshots for every resolvable target.

## Boundaries

- Do not add `wait_any`, per-target timeouts, a `group_id`, compatibility
  parsing for `id`, or a new runtime owner.
- Do not change Delegate, DelegateGroup, or DelegateSwarm card layout or
  expansion behavior.
- Historical session events are display history and are not re-executed; no
  persistence migration is required.

## Acceptance

- One call can wait for multiple agents and returns input-ordered results.
- A global timeout returns both completed and still-running target snapshots.
- The generated schema requires `ids` and no longer exposes `id`.
- Single-agent and single-swarm calls use the same batch envelope.
