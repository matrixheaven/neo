# Terminal Live Viewport Isolation Design

## Status

Approved direction from the 2026-07-19 Neo terminal ghosting investigation.
This document is the written design required before implementation planning.

This design amends
`docs/aegis/specs/2026-07-13-immutable-terminal-scrollback-design.md`.
It preserves that design's append-only history, request-driven rendering,
alternate-screen review, and transcript lifecycle rules. It supersedes only
these terminal-presentation decisions:

- appending finalized history with unrestricted full-screen CRLF scrolling;
- avoiding scroll regions and absolute cursor addressing; and
- accepting mutable live rows in terminal scrollback after anchor loss or
  resize.

## Problem

Neo correctly separates finalized transcript blocks from a bounded mutable live
frame at the data-model level, but its terminal backend does not preserve that
separation physically.

`LiveRenderer` tracks `hardware_cursor_row` relative to the assumed top of the
live frame. When history is finalized, `InlineTerminal` moves upward from that
relative cursor, clears to the end of the visible screen, writes history using
ordinary CRLF in the terminal's default full-screen scroll domain, and redraws
the live frame.

That algorithm is correct only while the relative anchor remains exact. A
resize, terminal reflow, suspend/reenter transition, or other cursor movement
can invalidate it. Once invalid, a later full-screen scroll can move mutable
Todo, composer, footer, or live card rows into native scrollback. Subsequent
clears can affect only the current visible screen, so scrolling upward exposes
old copies even though the canonical transcript contains no duplicates.

Synchronized output is not the root cause. It makes a byte sequence visually
atomic, but it does not change which rows the sequence is allowed to scroll.
Codex uses synchronized output in the same terminal while protecting its live
viewport with absolute geometry and explicit live-row ownership.

The existing regression
`history_commit_does_not_leave_ghost_live_rows_above_terminal_bottom` checks
only visible rows. It can pass while obsolete live chrome already exists in
native scrollback.

## Decision

`InlineTerminal` becomes the sole owner of absolute normal-screen geometry.
Finalized history replaces the cleared prefix of the live viewport at its
absolute origin. Mutable live rows are cleared before any full-screen scroll
that creates native history.

One successful render transaction performs this sequence:

1. Reconcile the current screen size, absolute cursor, and live viewport.
2. Clear the previous live viewport using absolute coordinates.
3. Promote finalized rows at the cleared live origin, scrolling only on actual
   screen overflow.
4. Reset the scroll region unconditionally.
5. Redraw the live frame at its absolute viewport origin.
6. Restore the logical cursor at its absolute screen position.
7. End synchronized output when supported and flush.
8. Only after the flush succeeds, acknowledge history and commit renderer
   geometry/cache state.

The old relative-anchor plus unrestricted-CRLF path is deleted in the same
change. There is no feature flag, terminal-specific compatibility owner, or
fallback renderer.

## Goals

- Keep finalized Neo rows and pre-Neo shell rows in native terminal scrollback.
- Guarantee that Todo, composer, footer, dialogs, running cards, and other
  mutable rows never enter native scrollback.
- Preserve native wheel scrolling and selection while live output changes.
- Preserve current transcript ordering, spacing, card content, card expansion,
  and automatic overflow behavior.
- Survive resize, suspend/resume, Ctrl+O review transitions, and normal exit
  without losing the absolute live viewport.
- Keep synchronized output as an optional atomicity optimization, not a
  correctness dependency.
- Work through the existing ANSI/ConPTY boundary on Windows, Linux, and macOS.
- Add no dependency and no second terminal renderer.

## Non-Goals

- Changing Todo, composer, footer, or dialog layout.
- Changing Delegate, DelegateGroup, DelegateSwarm, Workflow, tool, thinking,
  or assistant card content and lifecycle semantics.
- Reworking `TranscriptStore`, presentation acknowledgement, replay coverage,
  frame scheduling, or Ctrl+O browser ownership.
- Reflowing or rewriting rows already committed to native scrollback.
- Clearing terminal scrollback with `CSI 3 J` or rebuilding shell history.
- Disabling synchronized output for Ghostty or adding a Ghostty-specific path.
- Adding a synthetic scrollback model inside Neo.

## Required Invariants

1. Finalized history and mutable live rows have disjoint physical terminal
   regions during every normal-screen transaction.
2. Only a scroll region ending above `live_viewport.top` may scroll finalized
   history into native scrollback.
3. No scroll operation that includes any live viewport row is emitted while
   that row contains mutable Neo content.
4. `InlineTerminal` stores screen size, live viewport, and hardware cursor in
   absolute zero-based terminal coordinates.
5. `LiveRenderer` may diff row contents, but it does not own or infer the
   terminal viewport origin.
6. A cursor-position report is terminal protocol state, never prompt input.
7. A resize is not rendered until its cursor observation is associated with
   that size generation.
8. Failure to obtain required geometry fails closed with a typed terminal I/O
   error after restoring terminal modes; Neo never guesses a relative anchor.
9. Scroll margins are reset on success, error, suspend, review transition, and
   exit.
10. Presentation acknowledgement and renderer state advance only after the
    complete transaction flushes successfully.
11. Normal lifecycle output emits neither `CSI 2 J` nor `CSI 3 J`.
12. Obsolete live markers occur zero times across visible screen plus complete
    native scrollback after history commit, resize, suspend/resume, review, and
    exit sequences.

## Ownership

### Transcript And Presentation

The existing owners remain unchanged:

- `TranscriptStore` owns canonical structured conversation state.
- `TranscriptPresentation` decides which finalized blocks are pending history
  and which rows remain live.
- `NeoTui` composes the live suffix with chrome and returns a logical cursor.

These layers do not learn terminal coordinates or scroll-region rules.

### Terminal Geometry

`InlineTerminal` owns one normal-screen geometry record:

```text
NormalScreenGeometry {
    screen: { width, height, generation },
    live_viewport: { top, height },
    hardware_cursor: { column, row },
}
```

The record is conceptual; implementation may keep the fields directly on
`InlineTerminal` when that is simpler. No general-purpose viewport framework is
required.

`LiveRenderer` receives the absolute viewport origin from `InlineTerminal` for
each render. Its cached lines and Kitty image IDs remain useful, but the
relative `hardware_cursor_row` owner is removed.

### Cursor Observation

`NeoTerminal` obtains the initial cursor position before the background raw
stdin reader starts. Runtime resize observations stay inside the existing
terminal I/O path:

- on Unix ANSI terminals, the raw stdin owner requests `CSI 6 n`, recognizes
  the matching CPR response, and removes it from user input;
- on Windows, crossterm's console cursor-position API supplies the observation;
- unrelated key and paste bytes received during a Unix probe remain queued in
  original order;
- each observation is tagged with the terminal-size generation that requested
  it.

The probe is bounded. Missing or malformed geometry is an I/O failure, not a
reason to restore the old relative-anchor behavior.

## Viewport Placement

The first normal draw begins at the observed shell cursor. If the requested
live height would exceed the visible screen, `InlineTerminal` scrolls only the
necessary rows to make room and adjusts the absolute viewport. It does not
purge or replay prior screen contents.

Later live-height changes reuse the same absolute owner:

- shrinking clears rows released by the live viewport before reducing it;
- growing clears populated live rows, then scrolls only the rows needed to make
  room;
- a live frame remains bounded by the screen height and the existing automatic
  overflow surface remains authoritative for oversized mutable content.

## Protected History Insertion

Finalized lines are promoted at the cleared live viewport origin:

```text
clear the previously owned live rows
set the full-screen scroll region
move cursor: live_viewport.top
for each finalized line:
    clear the current row
    print the line
    print CRLF
reset scroll region
scroll only the additional rows needed by the new live suffix
restore absolute live cursor
```

Full-screen scrolling is safe only after every mutable row has been cleared.
Starting at the previous `live_top` overwrites those cleared rows with the
finalized prefix, so unused screen capacity never becomes a history gap. The
terminal scrolls only when the promoted history and new live suffix actually
cross the physical bottom; this preserves native scrollback without replaying
or clearing committed rows.

History lines retain the existing terminal wrapping policy and are written once.
No committed block is replayed to repair geometry.

## Resize

Every size change creates a new geometry generation. The resize event is made
renderable only after a cursor observation for that generation is available.
`InlineTerminal` then computes the new absolute viewport from the observed
cursor, previous viewport, old/new screen sizes, and requested live height.

Width-only changes are not exempt because terminal reflow can change the
cursor row. The old design's `fresh_anchor_pending` CRLF escape is removed; it
may not abandon mutable rows into terminal-owned history.

When geometry cannot be reconciled, Neo resets scroll margins, shows the
cursor, restores raw terminal modes, and returns a typed error. It does not
emit a destructive clear or continue from an assumed row.

## Review, Suspend, Resume, And Exit

Ctrl+O continues to use the alternate screen. Entering review snapshots the
normal absolute geometry. Leaving review restores the normal screen, consumes a
fresh cursor observation when terminal geometry changed, inserts only history
finalized while review was open, and redraws the current live frame.

Suspend clears the absolute live viewport and resets scroll margins before
restoring terminal modes. Resume obtains fresh geometry before the first normal
draw; it does not replay committed history.

Normal exit clears only the absolute live viewport, places the cursor below the
last finalized Neo row, resets scroll margins, shows the cursor, and restores
terminal modes. Error cleanup performs the same margin/cursor/mode restoration
on a best-effort basis.

## Compatibility And Cross-Platform Boundary

Neo already requires cursor-addressing support for interactive TUI mode. This
design adds cursor-position reporting and DECSTBM-compatible scroll margins to
that same terminal protocol boundary; it does not add a user setting or feature
flag.

ANSI sequences are emitted through the existing crossterm/virtual-terminal
output path. Windows keeps its native cursor query while using virtual terminal
output for the same scroll-region transaction. tmux and zellij are verified as
external terminal boundaries; unsupported CPR or scroll-margin behavior is a
reported compatibility error, not a hidden fallback renderer.

## Migration And Retirement

### Repair Track

- Add complete-scrollback reproduction before changing the renderer.
- Introduce absolute terminal geometry and protected history insertion at the
  existing `InlineTerminal` owner.
- Route initial and resize cursor observations through the existing terminal
  input/output boundary.
- Preserve existing presentation, card, chrome, review, and scheduling owners.

### Retirement Track

Delete these internal paths in the same implementation:

- relative `LiveRenderer::hardware_cursor_row` ownership;
- `fresh_anchor_pending` and its anchor-abandoning CRLF behavior;
- `clear_for_history_redraw` behavior based on an inferred relative origin;
- bottom-anchored `append_history_lines` that scrolls cleared live capacity; and
- visible-only ghost-row assertions that can pass with scrollback pollution.

Deletion class is internal code retirement. No public wire contract,
persistent data, or external source of truth is deleted. The retirement path is
`delete-first`; no compatibility exception is justified.

## Verification

### Automated

- Seed more than one screen of shell history, render uniquely marked Todo and
  composer rows, commit enough history to scroll, and assert across the complete
  vt100 scrollback that obsolete live markers occur zero times.
- Repeat the assertion across width and height resize generations.
- Assert current live markers occur exactly once and remain ordered below new
  finalized history.
- Assert every shell and finalized-history sentinel remains exactly once.
- Assert CPR bytes are consumed as terminal protocol state and never become
  prompt keys or pasted text, including chunked responses and interleaved keys.
- Assert history insertion emits a bounded scroll region, resets it, and
  restores the absolute cursor inside one transaction.
- Assert synchronized-output enabled and disabled paths have identical final
  terminal state.
- Preserve focused suspend/resume, review, exit, write-failure, Kitty image,
  and no-`CSI 2 J`/no-`CSI 3 J` regressions.

### Manual

In Ghostty, iTerm2, Terminal.app, WezTerm, Windows Terminal, tmux, and zellij:

- produce enough finalized output to exceed one screen;
- leave Todo/composer visible while output finalizes;
- scroll up and down repeatedly during and after output;
- resize narrower, wider, shorter, and taller;
- enter and leave Ctrl+O review;
- suspend/resume where supported; and
- exit normally.

No old Todo/composer/live card may appear in scrollback, while shell and
finalized Neo history remain selectable.

## Acceptance Criteria

- The reported Ghostty afterimage cannot be reproduced after repeated history
  commits and scrolling.
- Complete scrollback contains no obsolete mutable Neo row.
- Current Todo/composer/live chrome appears exactly once.
- Pre-Neo shell history and committed Neo history remain intact and selectable.
- Resize, review, suspend/resume, and exit preserve the same invariants.
- No Ghostty-specific workaround, scrollback purge, relative-anchor fallback,
  feature flag, new dependency, or second renderer exists.
- The old unrestricted-CRLF and relative-anchor owners are deleted.

