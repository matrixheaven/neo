# Immutable Terminal Scrollback Design

## Status

The architecture was approved in conversation on 2026-07-13. This document is
the written-review version required before implementation planning.

## Problem

Neo currently renders the complete transcript plus live chrome as one mutable
frame. The event loop also renders periodically while busy, even when no
application state changed. When a dynamic row changes after it has moved above
the renderer's synthetic viewport, the renderer cannot address that row in the
terminal's native scrollback and falls back to a destructive full redraw:

```text
ESC[2J ESC[H ESC[3J
```

`CSI 3 J` purges terminal scrollback. Replaying Neo's current frame cannot
restore shell history that existed before Neo started. It also invalidates the
terminal emulator's scroll and selection anchors, which causes the observed
jump to the bottom, selection expansion, and selection disappearance.

This is an architectural conflict: native terminal scrollback is append-only
from the application's perspective, while Neo treats every historical row as
mutable.

## Goals

- Preserve shell history that existed before Neo started.
- Preserve every committed Neo transcript row until the terminal evicts it
  according to the user's own scrollback limit.
- Let users scroll and select committed history while a response or tool is
  still running.
- Keep streaming text, live tool cards, Delegate/Swarm progress, dialogs, and
  the composer responsive.
- Avoid terminal writes when application state and visible animation state have
  not changed.
- Work on Windows, Linux, and macOS, including common terminal multiplexers.
- Replace the old whole-transcript renderer instead of retaining two rendering
  architectures or a compatibility flag.

## Non-Goals And Terminal Limits

- Neo cannot query the native terminal viewport offset or native text-selection
  state through a portable terminal protocol.
- Neo cannot override a terminal profile that explicitly enables
  scroll-on-output.
- Text selected inside rows that are themselves still changing may change with
  those rows. The strict stability guarantee applies to committed history.
- Neo will not re-render committed history merely to reflow it after a resize.
  The terminal may reflow soft-wrapped content according to its own behavior.
- This design does not add a separate Delegate/Swarm page or status panel.

## Required Invariants

1. Normal TUI startup, rendering, resize, suspend/resume, and exit never emit
   `CSI 3 J`.
2. A committed terminal row is written exactly once and is never addressed,
   cleared, deleted, or replayed by a later frame.
3. A row that may change again never leaves the bounded live surface.
4. The live surface never exceeds the addressable terminal viewport.
5. Terminal output state advances only after the complete write transaction is
   flushed successfully.
6. Normal-screen operation never enables mouse capture; native wheel scrolling
   and selection remain owned by the terminal emulator.
7. Rendering is request-driven. There is no unconditional polling-based frame
   tick.

`/clear` is a logical Neo transcript/session operation. It may reset canonical
and live Neo state, but it does not purge pre-Neo shell history or already
committed terminal scrollback.

## Ownership Model

### Canonical Transcript

`TranscriptStore` remains the canonical structured transcript used for session
replay, export, copy operations, and deterministic re-rendering. The existing
Transcript Boundary Semantics design remains authoritative for deciding whether
an event inserts a visible entry or mutates an existing entry.

The canonical transcript may update an entry in place because it is an
application data model. That does not imply that already committed terminal
bytes can be updated.

### Terminal Presentation

The terminal presentation has two disjoint owners:

- `CommittedHistory` owns finalized rows already inserted into native terminal
  scrollback. Its only operation is append.
- `LiveSurface` owns the bounded rows that may still change, including active
  assistant output, running tool cards, live Delegate/Swarm cards, overlays,
  and the composer.

The presentation layer tracks a stable identity and revision for each canonical
entry. An entry can move from live to committed exactly once after its visual
state becomes terminal. Committed entries cannot return to live state.

## Final Two-Surface Contract

The normal screen and the historical review surface have separate physical
owners:

- The normal screen writes finalized blocks to native terminal scrollback and
  owns only the bounded live suffix. Once bytes are acknowledged, ANSI has no
  portable address for them; Neo therefore never expands, collapses, clears, or
  replays those rows in place.
- Ctrl+O enters an app-owned alternate-screen TranscriptBrowser. It clones the
  canonical TranscriptStore and owns its viewport, width/height reflow, and
  browser scroll offset. Browser rows are review-only and never enter the
  normal presentation ledger.
- Normal input remains routed to the terminal emulator for wheel scrolling and
  selection. Browser input is routed to the browser state until Ctrl+O or
  cancel leaves it; suspend/resume restores the normal live anchor without
  replaying committed history.
- Review frames contain only bounded live rows, carry no history append blocks,
  and are never passed to `acknowledge_history`. Leaving review appends only
  history finalized while the browser was open.

Both surfaces preserve the no-clear invariant: normal lifecycle output and
review transitions emit neither `CSI 2 J` nor `CSI 3 J`. In particular, `CSI
3 J` is never a recovery path because it would purge shell history that Neo
cannot reconstruct.

## Render Transaction

Each draw produces one logical transaction:

```text
RenderTransaction {
    history_append: [FinalizedBlock],
    live_frame: [TerminalLine],
    cursor: optional position inside live_frame,
    next_animation_deadline: optional instant,
}
```

The terminal backend performs the transaction in this order:

1. Begin synchronized output when supported.
2. Clear the current live surface while its anchor is known.
3. Append newly finalized history with normal CRLF scrolling.
4. Redraw only the bounded live surface at the new anchor.
5. Restore the cursor inside the live surface.
6. End synchronized output and flush.
7. Commit the new history cursor, live-frame cache, and hardware cursor state.

If any write or flush fails, step 7 does not run.

## History Insertion

History and live output use one coordinated line-oriented path on ANSI and
ConPTY terminals. Before appending finalized rows, the coordinator clears only
the currently owned live rows, writes history with CRLF, and redraws the live
surface. It does not use scrolling regions, reverse index, absolute cursor
addressing, or a terminal-specific history-writer strategy.

The coordinator never uses `CSI 2 J` or `CSI 3 J`, never replays committed
history, and never truncates history to the current terminal height. A failed
write leaves both presentation acknowledgement and renderer state unchanged.

## Entry Lifecycle Policies

### Static Entries

User messages, finalized status rows, completed approvals, skill activations,
and other immutable entries may be committed immediately.

### Streaming Assistant Text

The streaming renderer separates a stable prefix from a mutable markdown tail.
Complete rows whose rendering can no longer be affected by later deltas are
committed incrementally. The incomplete paragraph, code fence, list item, or
wrapped row remains live. Message completion flushes the remaining tail.

The stable-prefix decision belongs to the markdown streaming layer, not to a
string-prefix comparison in the terminal backend.

### Thinking

A streaming thinking block remains live while its spinner, collapse state, or
content may change. It is committed only after the thinking lifecycle reaches a
terminal state.

### Tools, Delegate, Swarm, And Workflow Cards

Running cards remain normal inline transcript cards inside `LiveSurface`; they
are not moved to a separate panel. Existing card styling and the DelegateSwarm
Bayesian progress semantics remain unchanged.

Live previews have a terminal-height budget. Long output shows a bounded tail
and an omitted-row count while running. At `done`, `failed`, or `cancelled`, the
final card is rendered once without the live preview cap and committed.

The earliest live entry establishes a canonical commit frontier. Finalized
entries after that frontier are held as a bounded live suffix and rendered in
their original transcript order; they are not committed in completion order.
This keeps Delegate, DelegateSwarm, Workflow, tool, thinking, and assistant
segments at their canonical relative positions across turns and resume. A
suppressed live tool still forms a frontier barrier until its lifecycle becomes
terminal; a terminal suppressed tool then releases later history without
emitting a hidden card.

The presentation layer owns semantic block spacing. Adjacent blocks with
different entry owners receive exactly one blank row, while segments owned by
the same assistant entry do not gain an extra row across history/live or
acknowledgement boundaries. Bounded fitting reserves all visible mutable card
headers before deferred static headers, and terminal image blocks are omitted
atomically rather than emitting partial Kitty or iTerm sequences.

Lifecycle terminality is monotonic. A regressive or mutating update received
after a card was committed is a lifecycle invariant violation. It is excluded
from terminal presentation and recorded in bounded diagnostics. Canonical merge
logic must not regress the terminal state, and the committed card is never
rewritten.

### Dialogs And Composer

Blocking dialogs, overlays, footer state, and the composer are live-only chrome.
They never enter committed history unless their resolution creates a distinct
final transcript entry.

Approval prompts are transient interaction state. They are not reconstructed
from historical `ApprovalRequested` events because the decision is delivered
out-of-band and cannot be inferred reliably during replay. Persisted shell-mode
`ShellCommand` aggregates are replayed as finalized shell cards instead.

Replay keeps the raw JSONL event order as the presentation source of truth. A
persisted assistant aggregate is skipped only when the preceding raw text,
thinking, and tool-call projection is consumed by that aggregate; a
`ToolResult` aggregate is skipped only when the same tool id has a raw
`ToolExecutionFinished` event. Aggregate-only or partially written sessions
therefore retain their assistant and tool content, while event-rich sessions do
not render the same content twice. The coverage cursor is consumed in order,
not selected by a session-wide "has detail events" switch.

## Frame Scheduling

Replace the current 50 ms poll plus `dirty || elapsed` condition with an
application-wide frame requester. Transcript, prompt, chrome, overlay, resize,
and animation mutations all request a frame through the same API.

Requests have three scheduling classes:

- Immediate: user input, resize, dialog transition, suspend/resume.
- Coalesced: stream deltas and tool progress, capped by a minimum frame interval.
- Deadline: a visible spinner, elapsed timer, or progress animation requests its
  next actual visual deadline.

Multiple requests coalesce into one pending draw. An idle application with no
visible animation produces no stdout writes. A draw whose rendered transaction
is byte-for-byte empty also produces no cursor or visibility writes.

## Resize And Reflow

Resize must not purge or replay committed history. A resize can reuse the old
live anchor only when the terminal height is unchanged, no live image owns
physical rows, and every old live line remains narrower than a changed width.
In that recoverable case Neo clears and redraws only the live rows.

For any height change or ambiguous width/image reflow, the old physical anchor
is unknowable by the time Neo receives the resize event. Neo must not issue
cursor-up or erase-display commands against that anchor. It establishes a fresh
anchor with CRLF and continues from there. Mutable rows already moved by the
terminal may remain in terminal-owned history; this is the deliberate safety
tradeoff that prevents committed shell or Neo history from being erased. ANSI
provides no portable operation that can identify and remove only those rows.

Committed rows keep the hard-wrap boundaries used when the line-oriented
coordinator emits them. The canonical transcript retains unwrapped source
content, so session replay and export at a new width remain correct even when
already emitted terminal history does not visually reflow.

Live Kitty image IDs remain owned by the live renderer and may be replaced or
deleted there. Once an image block is committed, later live redraws must not
delete its image ID. A textual placeholder remains available on terminals where
graphics do not persist in scrollback.

## Suspend, Resume, And Exit

Suspend clears or finalizes only Neo's live surface, restores terminal modes,
and leaves committed history untouched. Resume establishes a new live anchor and
re-renders the current live state without replaying history.

Normal exit first asks the runtime to move every live entry to a terminal state.
An entry that cannot complete during shutdown receives an immutable
`interrupted` presentation snapshot. Neo then commits safe terminal entries,
clears only obsolete live rows, moves the cursor below Neo's final output, shows
the cursor, and restores every enabled terminal protocol through RAII guards.

An output failure is not recovered with a destructive full redraw. Neo restores
terminal modes, reports the typed I/O error outside raw mode, and exits the TUI.
The canonical session remains the recovery source for a later resume.

## Migration And Deletion

Implementation replaces the current whole-transcript output path in one
migration:

- `TranscriptPane` stops returning the complete historical frame to the
  terminal renderer.
- Delete the screen-output synthetic viewport accounting used to address the
  whole transcript, including its purge fallback, high-water shrink handling,
  and tests that require `CSI 3 J`.
- Replace `TuiRenderer` with the coordinated inline terminal plus bounded live
  renderer. Do not retain the old renderer behind an environment variable or
  feature flag.
- Retain `TranscriptStore` as the canonical model and adapt its rendering API to
  expose finalized blocks and the live surface.
- Review the logical `TranscriptViewport` separately; retain only behavior still
  used for copy/selection semantics, and delete dead terminal-scroll emulation.

## Verification

### Automated Invariants

- A virtual-terminal test seeds more than one screen of pre-Neo shell history,
  starts Neo, commits more than one screen of transcript, performs hundreds of
  live updates, resizes, and exits. Every seeded and committed row remains in
  screen plus scrollback.
- No normal-lifecycle output contains `CSI 3 J`.
- Repeated live frames never address or clear a committed row.
- Finalized blocks are emitted exactly once, including after a coalesced render.
- An idle event loop performs zero renders and zero stdout writes.
- Stream and animation requests coalesce while immediate input remains visible
  without waiting for the stream frame budget.
- Long assistant, tool, Delegate, and Swarm output never grows `LiveSurface`
  beyond the terminal height.
- Recoverable resize changes only live output. Ambiguous resize establishes a
  fresh anchor without addressing unknown rows or acknowledging unwritten
  history.
- A write failure leaves cached terminal state uncommitted and restores raw mode
  without emitting a purge sequence.
- Suspend/resume and normal exit preserve shell history and committed Neo rows.
- `/clear` resets Neo's logical view without emitting a scrollback purge.

### Cross-Platform Verification

Focused automated tests cover ANSI sequence generation on all platforms and
Windows-specific mode restoration behind `cfg(windows)`. Real terminal checks
cover Terminal.app, iTerm2, Ghostty, WezTerm, Windows Terminal, tmux, and
zellij.

The manual interaction matrix exercises:

- scrolling upward throughout streaming and tool execution;
- dragging a selection across committed rows through multiple live frames;
- long-running Delegate/Swarm progress with other messages completing;
- narrow/wide and short/tall resizes;
- suspend/resume, normal exit, interrupt, and renderer I/O failure.

## Acceptance Criteria

- Neo never deletes history above its startup cursor during normal operation.
- Committed-history selection no longer expands or disappears because of Neo
  rendering.
- Scrolling to older committed history is not reset by Neo's own purge or
  replay behavior.
- Conversation history is not capped to the current terminal height.
- Live streaming and tool progress remain visible and responsive.
- No old whole-frame compatibility path remains.
