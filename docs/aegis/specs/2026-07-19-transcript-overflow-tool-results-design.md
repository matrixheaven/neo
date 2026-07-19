# Automatic Transcript Overflow and Tool Result Presentation

## Status

Approved design. This document fixes the outer transcript overflow behavior and
the presentation of `ListDelegates` and `Sleep`. It does not redesign Delegate,
DelegateGroup, DelegateSwarm, or child-activity cards.

This document supersedes only the live-height fitting policy in
`2026-07-13-immutable-terminal-scrollback-design.md`: bounded live tails,
presentation-generated omitted-row markers, and header-only fallback when the
mutable suffix is taller than the terminal. The immutable-history ledger,
canonical commit frontier, source ordering, semantic block spacing, atomic
terminal-image handling, lifecycle monotonicity, and two-phase history
acknowledgement remain authoritative.

## Problem

The earliest mutable transcript entry establishes a commit frontier. Finalized
entries after that frontier must stay in the mutable suffix until the frontier
clears so their canonical order is preserved. A long-running DelegateGroup or
DelegateSwarm can therefore retain an increasingly tall suffix even though most
later entries are already finalized.

`TranscriptPresentation::fit_live_blocks` currently forces that suffix into the
physical live-row budget. Under pressure it first drops body rows and emits
`earlier rows omitted`; at the tightest budget it keeps only headers. This is why
a complete card can later collapse to a line such as `Delegate group ...` even
though the card component itself still renders its original body.

Increasing the budget cannot solve the problem. The current budget already uses
all rows above chrome, and `LiveRenderer` correctly rejects a frame taller than
the terminal. The missing behavior is a bounded viewport for a complete source,
not another card layout or a larger fixed frame.

Two ordinary tool cards also lack useful presentation:

- `ListDelegates` already returns structured `details.kind == "delegate_list"`,
  but the TUI falls back to an opaque text result.
- `Sleep` already receives `duration_seconds` and `reason`, but the TUI does not
  show the total wait, remaining time, or a live countdown.

## Goals

- Preserve every existing Delegate, DelegateGroup, DelegateSwarm, and
  child-activity row exactly as its current component renders it.
- Never replace transcript rows with a presentation-level
  `earlier rows omitted` marker or a header-only fallback.
- Keep the normal inline, native-scrollback experience while the complete
  mutable suffix fits above chrome.
- Automatically provide a bounded, scrollable transcript viewport when the
  complete mutable suffix does not fit.
- Keep composer and footer fixed, visible, editable, and submittable during
  automatic overflow.
- Preserve primary terminal scrollback and append history finalized during
  overflow exactly once after returning to the primary surface.
- Render `ListDelegates` from its existing structured result without exposing
  its opaque pagination cursor.
- Render `Sleep` with total duration, remaining countdown, and reason without a
  duplicate generic `Waited ...` body.

## Non-Goals

- A compact Delegate or DelegateSwarm card variant.
- Semantic summaries, adaptive card layouts, different activity windows, or
  changed expansion rules.
- Moving Delegate or DelegateSwarm content into a dashboard or side panel.
- Changing DelegateSwarm progress estimation or aggregation.
- Changing the `ListDelegates` or `Sleep` core tool contracts.
- Replacing native primary scrollback with an application-owned permanent
  transcript browser.
- Adding a height configuration, feature flag, compatibility path, or terminal
  dependency.
- Removing existing card-local preview limits or existing Ctrl+O expansion
  hints. This design removes only presentation-level transcript omission.

## Card Output Lock

The following files are output-locked for this work:

- `crates/neo-tui/src/transcript/delegate_card.rs`
- `crates/neo-tui/src/transcript/delegate_group.rs`
- `crates/neo-tui/src/transcript/swarm_card.rs`
- `crates/neo-tui/src/transcript/child_activity.rs`

Their rendered content, row order, styling, role badges, activity treatment,
progress text, collapsed/expanded behavior, and Ctrl+O semantics must not
change. Automatic overflow consumes their canonical output; it does not ask
them to render a different mode.

## Product Contract

### Normal inline mode

`TranscriptPresentation` composes the complete mutable suffix in canonical
transcript order. If its row count is at most
`terminal_height - fitted_chrome_height`, Neo uses the existing inline terminal
path: finalized history is appended to native scrollback and the complete
mutable suffix plus chrome is rendered at the primary live anchor.

There is no presentation-level row fitting, header prioritization, tail
selection, or omission line.

### Automatic overflow mode

If the complete mutable suffix exceeds the body budget, Neo automatically uses
the existing alternate-screen capability as a bounded viewport. The viewport
renders the complete canonical transcript source using the current card
expansion state. It must not enable a compact or expanded variant merely because
overflow is active.

The viewport body receives only the rows left after the normal fitted chrome is
reserved. The composer, footer, and logical cursor are appended through the
same chrome path used by normal and Ctrl+O frames. Every emitted frame remains
at most the physical terminal height and every row remains within terminal
width.

The viewport follows the tail on entry and while the user has not scrolled
away. Mouse wheel, PageUp, and PageDown scroll only the automatic transcript
viewport. Ordinary prompt editing and submission continue through the existing
composer path.

### Overflow latch

Automatic overflow is latched after entry. It remains active until the live
commit frontier clears, even if a resize or a shorter intermediate render would
temporarily fit. This avoids alternate-screen enter/leave flicker while the
same long-lived card still owns canonical ordering.

When the frontier clears, Neo returns to the primary surface. Any finalized
history accumulated while the alternate surface was active remains
unacknowledged and is appended by the next normal render transaction exactly
once.

### Manual Ctrl+O precedence

Manual Ctrl+O review remains a logical transcript-browser mode with its existing
collapsed/expanded behavior. If it opens while automatic overflow is latched,
manual review temporarily owns the viewport content, but both modes share one
physical alternate surface. Neo must not emit a second enter transition.

Closing manual review returns to automatic overflow when the latch remains, or
to the primary surface when it has cleared. Physical transitions therefore
depend only on whether any alternate-surface owner is active, not on changes
between logical owners.

## Presentation Contract

`TranscriptTerminalUpdate` reports three independent facts:

```rust
pub struct TranscriptTerminalUpdate {
    pub history: Vec<FinalizedBlock>,
    pub live: Vec<String>,
    pub has_visible_animation: bool,
    pub live_overflow: bool,
    pub has_live_frontier: bool,
}
```

- `live` contains every canonical mutable-suffix row.
- `live_overflow` is `live.len() > live_budget`; it is a signal to `NeoTui`, not
  permission to truncate.
- `has_live_frontier` reports whether canonical commit blocking still exists and
  controls latch release.

The presentation owner continues to decide history versus mutable suffix,
spacing, atomic blocks, and animation visibility. `NeoTui` owns selection of the
normal or alternate viewport because it already composes transcript and chrome.
`InlineTerminal` owns only the physical primary/alternate transition and never
interprets Delegate or tool semantics.

## Alternate-Surface Terminology

The current physical output contract uses review-specific names such as
`review_surface`, even though the same mechanism will serve manual review and
automatic overflow. Those physical names become alternate-surface names.
Logical `TranscriptBrowserState` review terminology remains unchanged.

This is one canonical physical mechanism, not a second renderer or nested
alternate-screen stack. Entering any first owner saves the primary live anchor;
switching logical owners stays on the alternate surface; leaving the last owner
restores the saved anchor.

## Source-Preserving Viewport

`TranscriptPane` exposes a viewport renderer that clones the pane for rendering,
uses its current tool/card expansion state, and slices the complete canonical
body with the existing `TranscriptViewport`. It does not consume the real
pane's dirty flag, mutate source entries, acknowledge history, or force
`tool_output_expanded`.

Manual browser rendering may continue to set the clone's expansion state from
`TranscriptBrowserState`. Automatic overflow uses the new source-preserving
path so the original Card design remains byte-for-byte governed by its existing
components.

## `ListDelegates` Presentation

The TUI consumes the existing structured result only when
`details.kind == "delegate_list"`.

- The header adds `count` and `total` as a compact `N of M` chip.
- The body renders the `delegates` array in returned order.
- Agent rows use the existing `display_name`, `status`, and `title` fields.
- Swarm rows use `description` and `status`, with a child tree row for the
  existing aggregate counts.
- Empty results render `No delegates found` and may render existing structured
  `next_steps` text.
- `next_cursor` and `cursor_query` are never rendered to the user. They remain
  model-facing tool data for pagination.

The renderer falls back to the existing generic result path when the details
kind or required arrays are absent. No parser is added for the human-readable
`ToolResult.content`, and no core result shape changes.

## `Sleep` Presentation

The TUI parses `duration_seconds` and `reason` from the existing tool arguments.
While `Sleep` is running, its card shows:

```text
● Using Sleep · <total> total · <remaining> remaining
  <reason>
```

Remaining time is derived from the component's existing
`streaming_started_at`, saturates at zero, and updates through the existing
animation scheduler. After success, the header retains the total duration and
the body retains the reason. The generic `Waited <N> seconds: <reason>` result
body is suppressed for successful `Sleep` because it duplicates the semantic
card. Existing failure/cancellation status and error content remain available.

No timer, cancellation, schema, result, or shell-admission behavior changes in
`neo-agent-core`.

## Failure And Resize Behavior

- Very short terminals still fit chrome first. The transcript viewport may have
  zero body rows, but the final frame must remain bounded and cursor-safe.
- Width changes re-render canonical rows at the new width and resynchronize the
  viewport without changing Card semantics.
- Height changes update the viewport capacity but do not release an active
  overflow latch before the live frontier clears.
- Terminal write/flush failure leaves presentation acknowledgement and physical
  surface state unchanged under the existing transactional contract.
- Suspend, resume, and application exit leave the alternate surface through the
  same canonical terminal-mode guard used by manual review.

## Acceptance Criteria

- A long-running DelegateGroup or DelegateSwarm never degrades to only its
  header because later transcript rows accumulate.
- No presentation frame contains `earlier rows omitted`.
- The four output-locked Card files are unchanged.
- Normal sessions that fit retain current inline output and native scrollback.
- Overflow automatically enters one bounded alternate viewport with complete
  canonical rows and fixed chrome.
- Wheel/PageUp/PageDown scroll overflow while ordinary editor input and submit
  continue to work.
- Manual Ctrl+O can open and close during automatic overflow without nested
  physical transitions.
- Primary scrollback is byte-preserved across overflow, and history finalized
  during overflow is appended exactly once afterward.
- `ListDelegates` shows structured counts and agent/swarm rows without an opaque
  cursor.
- Running `Sleep` shows total, remaining countdown, and reason; completed
  `Sleep` does not repeat the generic `Waited ...` body.

## Verification Strategy

- Presentation regressions prove complete live rows and explicit overflow/frontier
  signals without fitting or omission.
- Frame regressions prove automatic alternate selection, bounded rows, fixed
  chrome, cursor bounds, latch behavior, and manual-review precedence.
- Virtual-terminal regressions prove one enter/leave transition, preserved
  primary scrollback, deferred history exactly once, and absence of `CSI 2 J` /
  `CSI 3 J`.
- Controller regressions prove overflow scrolling does not consume prompt edit,
  submit, interrupt, suspend, or exit behavior.
- Tool-card regressions prove structured `ListDelegates` and semantic `Sleep`
  presentation from existing arguments/details.
- Existing representative DelegateGroup and DelegateSwarm rendering tests remain
  unchanged and pass as a Card Output Lock check.

## Architecture Signal

This design changes a durable terminal-presentation boundary: overflow moves
from destructive row selection inside `TranscriptPresentation` to viewport
selection in `NeoTui`, while `InlineTerminal` retains one physical
alternate-surface owner. After implementation, architecture review should decide
whether this supersession belongs in an ADR or is sufficiently captured by the
updated immutable-scrollback baseline and this design spec.
