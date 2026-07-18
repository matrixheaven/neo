# New Session Context Reset and Sessions Alias

## Problem

`/new` clears cumulative token usage but leaves the current `ContextWindow`
usage fields intact, so the footer carries the previous session's `ctx used/max`
value into the fresh session. Neo also exposes the session picker only through
`/resume`, while `/sessions` should be an equivalent command.

## Design

- In the existing new-session reset path, replace the current context window
  with a fresh window that preserves only the selected model's maximum context
  size. This clears used and projected tokens without losing the footer's model
  capacity.
- Dispatch `/sessions` through the same `open_session_picker()` branch as
  `/resume` and list it in the existing slash completion/help catalog.
- Do not add an alias registry, compatibility layer, or new state type.

## Verification

- Extend the `/new` behavior test to seed used/projected context and assert the
  fresh session retains only the maximum context size.
- Add one slash-command test proving `/sessions` opens the session picker.
- Run only those exact `neo-agent` binary tests, then formatting and diff checks.
