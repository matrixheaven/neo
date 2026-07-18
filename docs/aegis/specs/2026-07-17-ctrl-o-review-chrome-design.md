# Ctrl+O Review Chrome Design

## Problem

Committed expandable transcript entries open an alternate-screen review surface.
That surface currently replaces the whole frame, consumes prompt input, and
returns no logical cursor. The terminal transition does not hide the hardware
cursor, so a full review frame removes the composer while leaving a stray cursor.

## Design

- Keep the alternate-screen review surface so committed terminal scrollback is
  never rewritten.
- Render normal chrome first, reserve its fitted height, and give the review body
  only the remaining terminal rows.
- Append the same chrome and cursor used by the normal surface to review frames.
- While review is open, consume only review navigation, Ctrl+O, cancel, and global
  actions. Route ordinary editor input to the composer. Submitting closes review
  before starting the turn.
- A terminal frame with no logical cursor hides the hardware cursor; a later frame
  with a cursor shows it again.
- Rendering a cloned review snapshot must not clear the real transcript pane's
  dirty flag.

## Invariants

- `review.live.len() <= terminal_height` for every terminal height.
- The composer/footer remain visible when expanded review content fills its body
  budget.
- Any returned cursor is inside the emitted live frame and terminal bounds.
- Entering and leaving review does not replay or mutate committed history.
- No compatibility path or feature flag preserves the old full-screen modal
  behavior.

## Verification

- A review-frame regression at exact terminal fill asserts composer, footer, and
  cursor visibility.
- Controller tests assert ordinary editing reaches the prompt and submit closes
  review.
- Renderer tests assert `cursor=None` hides the hardware cursor and a later cursor
  restores it.
- Existing focused transcript-browser and immutable-scrollback tests remain green.
