# Fullscreen Transcript Document Design

## Status

Approved by the user on 2026-08-04. Implementation planning is authorized;
runtime implementation remains a separate authorization boundary.

After written approval and implementation verification, this design supersedes
the ordinary-conversation presentation direction in:

- `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`;
- `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`.

The discarded assistant-prefix native-history proposal is deleted and is not a
baseline, alternative implementation input, or compatibility requirement.
ADR and landed-baseline updates happen only after implementation and focused
verification; they must not leave a second interactive renderer or fallback.

## Problem

Neo currently tries to combine an unbounded, mutable conversation with a
terminal surface that can only rewrite the rows still visible above the bottom
chrome. The implementation divides the transcript into committed history and a
bounded live area. This creates three independent information-loss mechanisms:

1. `TranscriptPresentation::bound_live_blocks` removes older live blocks or
   older rows from the newest block when the live area exceeds its row budget.
2. `ToolCallComponent` keeps only six completed live-output lines and 50,000
   completed-line characters. Older lines are discarded, and the live buffer
   is cleared when the final result arrives.
3. Bash and Terminal already cap output before it reaches the TUI. The default
   command result budget is 64 KiB; the existing command log is capped at 10
   MiB and can also drop data when its queue fills.

The first mechanism causes the reported assistant bug: while streaming, only
the newest viewport-sized tail remains visible; the complete answer appears
only after finalization. The other two mechanisms mean that a larger viewport
alone cannot recover complete tool output.

The normal terminal screen cannot solve this class of problem. Native
scrollback is append-only, while assistant text, approvals, running tools,
Delegate-family cards, and Workflow groups can change height or content in
place. Any design that continues writing mutable content inline must choose at
least one of these failures:

- rewrite only the visible tail and hide older live rows;
- append provisional states that cannot be corrected;
- enter an application viewport only after overflow, creating two display
  modes and two scroll owners;
- let a frame exceed terminal height and rely on terminal-specific omitted-row
  behavior.

## Confirmed Outcome

Interactive Neo uses one application-owned fullscreen transcript document from
startup until exit. Every transcript entry stays at its canonical document
position and may update or grow there. The physical frame always contains only
the visible viewport, but the document source remains complete and reachable
through application scrolling.

The user-visible behavior is fixed as follows:

- At the bottom, new content keeps the viewport following the document tail.
- Scrolling upward locks the current logical position. Later content changes
  do not pull the user to the bottom.
- While locked, any later document revision shows one new-activity indicator.
  It does not move the viewport. Returning to the bottom clears it.
- Ordinary drag selects text inside Neo across entries and cards. Dragging
  beyond the viewport auto-scrolls. Double-click selects a word.
- A visible copy control copies the current selection. The existing system
  clipboard path remains the clipboard owner.
- Shift-drag uses the terminal emulator's selection bypass where supported.
- Interactive Neo no longer exposes the old `Ctrl+O` transcript browser. Task
  Browser remains a separate explicit product surface inside the same physical
  fullscreen session.
- On normal exit, Neo restores the original terminal screen and prints only
  the final assistant answer, terminal task/workflow status, and a session
  reopen hint.
- Print and pipe modes remain static terminal output and never initialize the
  fullscreen document.

## Goals

- Remove viewport-driven omission from assistant text and every dynamic card.
- Preserve the current Delegate, DelegateGroup, and DelegateSwarm card design,
  ordering, hierarchy, progress, and expansion semantics.
- Preserve the Workflow main-card, Workflow Delegate summary, and Workflow
  DelegateSwarm summary hierarchy and ordering.
- Keep all information emitted to the presentation layer reachable without
  relying on terminal-native scrollback.
- Capture complete Bash and Terminal display output before existing model-result
  and live-preview limits can discard it.
- Keep canonical session history and model context append-only and unchanged.
- Support stable scrolling, text selection, resize, and dynamic height changes
  on macOS, Linux, and Windows terminals.
- Reuse Neo's current typed transcript entries, render cache, frame scheduler,
  terminal lifecycle, and differential renderer.

## Non-goals

- Byte-perfect recording of arbitrary binary stdout or terminal escape
  sequences. Shell tools remain text tools; display output uses the same safe
  text normalization as the TUI.
- Removing deliberate tool-level pagination or result limits from tools such
  as Read, Grep, and task-output APIs. The UI preserves every result actually
  emitted by the tool; it cannot invent data the tool never produced.
- Redesigning the body of non-workflow Delegate-family cards. Their existing
  semantic summaries and activity limits are not viewport truncation.
- Changing Workflow execution, scheduling, journal, recovery, results, or
  model-visible output.
- Writing full display-only command output into model context, provider
  requests, or the context cache prefix.
- Adding search, bookmarks, transcript editing, detachable panes, or a second
  detail viewer.
- Porting grok-build's private terminal fork, its compact/omission modes, or its
  complete rendering stack.

## First-Principles Decision

### Invariants

- The irreducible outcome is that every presentation input remains reachable
  while mutable entries continue to update.
- The canonical conversation remains append-only and retry-safe.
- There is exactly one interactive document, one scroll owner, one layout
  owner, and one physical fullscreen renderer.
- Viewport pressure may choose what is currently visible, but may not alter the
  document source or card renderer output.

### Assumptions Removed

- Native terminal scrollback does not need to own an interactive conversation.
- A transcript does not need separate history and live regions.
- Interactive assistant fragments do not need to enter native history when the
  application retains and redraws the whole document.
- `Ctrl+O` does not need to duplicate the same transcript in another surface.
- A larger line tail is not a substitute for complete output retention.

### Smallest Sufficient Path

Reuse `TranscriptStore` as the single typed document, replace the existing
row-only viewport with a stable logical anchor, render only the visible slice
through the existing differential renderer, and add one session-local output
artifact only for data that existing events cannot retain completely.

No second transcript store, renderer, viewport, compatibility branch, or
third-party terminal dependency is added.

## Existing Infrastructure To Reuse

Neo already has most of the required pieces:

- `TranscriptStore` owns typed entries, stable entry identifiers, revisions,
  and event-order insertion.
- `TranscriptPane` caches rendered entry bodies and entry row starts.
- `TranscriptViewport` already implements bottom following and basic scrolling.
- `LiveRenderer` already performs row-level differential writes and suppresses
  output when a frame is unchanged.
- `InlineTerminal` already owns balanced alternate-screen transitions, mouse
  capture, synchronized output, rollback after failed writes, suspend, and
  terminal restoration.
- `FrameScheduler` already coalesces updates and schedules visible animation.
- `TranscriptBrowserState` proves that the same typed entries can be rendered
  as one scrollable document, although its per-frame clone and row-only scroll
  offset are not suitable for the primary path.

The grok-build reference contributes four patterns, not code dependencies:

1. stable entry identity for in-place streaming updates;
2. per-entry height invalidation and cached virtual row starts;
3. logical content anchors that survive growth and reflow;
4. animation scheduling only for visible dynamic entries.

Its drawing backend alone would not fix Neo. Keeping Neo's current history/live
partition and replacing only the draw function would preserve the same
omission bug.

## Selected Architecture

### 1. One Ordered Document

`TranscriptStore` remains the sole typed content owner. A document entry has:

```text
entry id
entry revision
typed entry state
derived immutable activity facts, when required
cached rendered height for the current width and expansion state
```

Assistant deltas, tool progress, approvals, questions, Delegate-family
snapshots, and Workflow snapshots update the existing entry by identity. They
never move between history and live collections.

Captured progressive Delegate-family facts remain attached to their original
entry so activity already received cannot disappear when a later snapshot
trims its live activity list. They are no longer acknowledged into terminal
history or removed after a physical write.

`TranscriptPresentation`, its history/live partition, acknowledgement ledger,
live row budget, and progressive terminal facts leave the interactive render
path. Any small pure projection helper still needed by the document moves to
the typed entry owner; no renamed history/live wrapper remains.

### 2. Incremental Layout And Height Changes

The document layout stores the rendered height and virtual starting row for
each entry at the active width and expansion state.

When entry `E` changes:

1. invalidate only `E`'s rendered body and height;
2. render `E` at the current width;
3. compute its height delta;
4. shift cached virtual row starts only for entries after `E`;
5. redraw only affected visible physical rows.

Appending to the tail therefore remains proportional to the changed entry and
visible viewport rather than the complete transcript. A width or theme change
invalidates all heights, but reflow runs outside the physical write and keeps a
logical anchor until the new layout is ready.

The physical frame is always exactly bounded by terminal height. Large cards
become large document blocks; they never become large terminal frames.

### 3. Stable Logical Scroll Anchor

The existing absolute `scroll_top_rows` state is replaced, not supplemented,
by one logical top anchor:

```text
entry id
logical row within the entry
cell offset within a wrapped logical row
```

When following the tail, layout changes resolve directly to the new bottom.
When the user scrolls upward, the current top visible position becomes the
anchor. Growth above or inside other entries changes physical row numbers but
does not change the anchored content.

After resize or reflow, the layout resolves the same anchor against the new
wrapped geometry. If a provisional retry entry containing the anchor is
discarded, the anchor falls back to the nearest preceding surviving entry; it
never switches to bottom following implicitly.

The view stores the document revision observed when tail following stopped. A
later revision turns on one new-activity indicator. The indicator is Boolean,
not a line or event count, because one card may revise repeatedly in place.

### 4. Complete Display Output

Existing session JSONL cannot be the complete output store:

- Bash streams at most its configured result budget to the update callback;
- the guardian samples live updates and may skip chunks under pressure;
- Terminal retains only a bounded ring before reads;
- finished result fields are already capped;
- encoding unbounded command streams as session events would make every resume
  parse the entire output before showing the document.

Each tool execution started by a binary supporting this design and capable of
emitting streaming display text gets one opaque, path-safe output reference.
This includes tools run by the main agent, Delegate-family children, and
Workflow children. Normalized text is appended under the existing per-agent
session task directory. The reference, byte/line metadata, and completion state
are persisted as optional typed event metadata; filesystem paths are not
model-visible values.

The output artifact has these rules:

- append-only for the lifetime of the tool execution;
- no configured byte or line truncation limit;
- one writer for each output reference;
- source-side capture before Bash result caps, live-preview sampling, and the
  Terminal ring buffer;
- a sparse derived byte-offset index for viewport reads; the text artifact
  remains the source and the index can be rebuilt;
- bounded writer buffers apply backpressure to the producer instead of dropping
  chunks; no unbounded in-memory output queue is added;
- visible-range reads and bounded look-ahead only; full output is not loaded
  into TUI memory;
- same local-only lifecycle and access boundary as the containing session;
- deletion only with the containing session, with no new cleanup UI or user
  maintenance task.

The complete typed command or tool arguments remain in the typed tool entry and
are never shortened in storage. Tools without streaming text do not create an
unnecessary output artifact; their complete structured presentation fields stay
in typed events.

The model-facing `ToolResult` keeps its current bounded and structured shape.
Compaction and provider projection never read the display output artifact.

If output capture cannot be opened or appended, the affected tool fails or is
cancelled and the card reports the capture failure and possible partial side
effects. Neo never continues while silently claiming that output is complete.
If a derived index is missing or corrupt on resume, Neo rebuilds it from the
text artifact. If the text artifact itself is missing, the card exposes the
persisted result as incomplete legacy data and never labels it complete.

Existing sessions remain readable without migration. Data already discarded
by older binaries cannot be recovered.

### 5. Card Rendering

Viewport code always asks each entry for its full designed render and then
slices the document globally. It never passes terminal-height budgets into a
card renderer.

For non-workflow Delegate, DelegateGroup, and DelegateSwarm entries:

- component source files and their collapsed/expanded output remain unchanged;
- card identity, hierarchy, ordering, progress, and expansion remain unchanged;
- no global overflow marker, header-only fallback, or tail-only projection may
  replace their rendered rows.

Their existing semantic summaries, including the existing bounded child
activity chosen by the card itself, remain part of the approved card design.
This task removes only outer presentation loss.

Output capture applies equally to tools executed by Delegate-family children.
Where the current card expansion exposes a child tool, it reads the complete
typed command and retained output without changing the card's layout or
expansion semantics.

For each Workflow run, one document entry keeps this fixed sibling order:

1. Workflow main card;
2. Workflow Delegate summary, when present;
3. Workflow DelegateSwarm summary, when present.

The document renders every structural row from all three siblings. It does not
pass `available_rows` to `render_workflow_group`, does not produce omitted-child
or omitted-direct-tool rows due to viewport height, and does not fold siblings
into the main header.

Direct Workflow tools remain one-line semantic summaries by default. Activating
a direct tool expands its complete typed command or arguments, received
details, and retained output inline beneath that same row by reusing
`ToolCallComponent` rendering and the output reference. This is the only
Workflow card expansion added by this design. Workflow-origin Delegate-family
children keep their existing summary-card semantics rather than nesting full
standalone cards; their own tool executions still use lossless output capture.

### 6. Assistant Streaming And Retry Safety

One active assistant attempt is one mutable document entry. Text deltas grow it
in place without a live row budget. No parser-proven prefix is submitted to
native terminal history because the primary screen is not the transcript
owner.

Session persistence retains its existing attempt gate, stated here directly
without depending on the discarded native-history proposal:

- an unconfirmed attempt stays display-only;
- retry or compaction discards that provisional attempt from canonical session
  events;
- only the winning assistant message enters append-only session history;
- no provisional prefix is rolled back in a persisted conversation;
- final assistant content appears exactly once on resume.

No assistant prefix is written to native history. Retry safety belongs to the
existing provisional-attempt and winning-message persistence rules, not to a
second presentation path.

### 7. Selection And Copy

Selection endpoints use document coordinates rather than terminal screen rows:

```text
entry id
logical rendered row within the entry
display-cell column
```

The renderer maps mouse coordinates through the visible layout to these
endpoints. Selection may cross entry and card boundaries. Dragging above or
below the viewport scrolls the document while preserving the fixed endpoint.

On mouse release, Neo materializes the selected plain text for the current
document revision. Later dynamic updates do not silently change the copied
selection. If an endpoint row disappears during the drag, it clamps to the
nearest surviving row in the same entry, then to the normal anchor fallback.

Single clicks on controls keep their existing action ownership. A movement
threshold distinguishes a click from a selection drag. Double-click selects a
word using the already-installed `unicode-segmentation` dependency.

The copy control and existing configured copy action use the same materialized
selection and clipboard writer. Shift-drag is not interpreted by Neo, allowing
terminal emulators with the conventional mouse-capture bypass to perform native
selection.

### 8. Fullscreen Lifecycle

Interactive startup enters one balanced alternate-screen session and enables
mouse reporting once. Internal dialogs, Task Browser, themes, model selection,
and transcript interaction do not trigger physical screen transitions.

The renderer uses the current terminal lifecycle and differential output path:

- one application viewport with origin row zero;
- synchronized output around each changed frame;
- row-level differential redraw through `LiveRenderer`;
- no write when rows and cursor are unchanged;
- a full redraw after resize or surface invalidation;
- guaranteed mouse/cursor/screen restoration on normal exit, error, panic, and
  supported suspend/resume paths.

The old `Ctrl+O` browser clone is deleted. No automatic-overflow latch remains
because fullscreen is the only interactive presentation.

### 9. Balanced Animation

Animation is presentation-only and never controls semantic state or document
height.

- Schedule at no more than 30 frames per second.
- Request frames only while a visible entry has an active animation.
- Pause timers and animation work for entries outside the viewport.
- Reuse synchronized differential writes; do not port a cell renderer or add a
  second scheduler.
- Preserve complete rows throughout transitions; animation may change style or
  indicators but may not temporarily collapse or omit content.
- Honor reduced-motion behavior by rendering the final state immediately.

This design establishes the infrastructure and limits. It does not add
decorative animations unrelated to existing card state, progress, focus, or
expansion.

### 10. Exit Projection

Leaving fullscreen does not replay the whole transcript into native
scrollback. After restoring the original terminal, Neo emits one static exit
projection containing:

- the final assistant answer when one exists;
- unresolved, failed, cancelled, or completed terminal task/workflow status;
- the session identifier and reopen command.

This projection is derived from canonical final state and is not appended back
into session history. Abnormal termination prioritizes terminal restoration;
if no safe final projection can be produced, Neo prints a bounded recovery
message with the session identifier.

## Data Flow

### Ordinary Entry Update

```text
AgentEvent
  -> TranscriptStore entry identity/revision
  -> invalidate changed entry layout
  -> resolve tail following or logical anchor
  -> render visible document slice
  -> append chrome and selection overlay
  -> LiveRenderer differential fullscreen write
```

### Complete Tool Output

```text
tool/process bytes
  -> source-side safe text normalization
  -> append session-local output artifact
  -> update line/byte metadata and derived seek index
  -> emit bounded/coalesced card progress event
  -> visible expanded card reads only required output range
```

### Resume

```text
session JSONL + typed output references
  -> rebuild TranscriptStore in event order
  -> validate/rebuild output seek indexes as needed
  -> construct document layout around the requested tail or saved target
  -> enter fullscreen and render visible slice
```

Viewport position, selection, expansion, and tail-follow state are session UI
state, not canonical conversation. A normal resume starts at the document tail
with no selection. Existing entry expansion defaults remain unchanged unless a
separate approved feature later persists UI preferences.

## Failure Handling

- Terminal write failure does not mutate document state; the next frame retries
  from the last successfully drawn state.
- Output capture failure is explicit and stops the affected tool rather than
  silently dropping later data.
- Index rebuild failure leaves the raw text artifact untouched and reports that
  expanded random access is unavailable.
- Missing legacy output never produces a false complete label.
- A disappearing provisional retry entry uses the preceding-entry anchor
  fallback and retains locked-scroll mode.
- Resize/reflow retains the logical anchor and blocks only layout publication,
  not event ingestion or output capture.
- Clipboard failure keeps the selection and reports the error without changing
  document content.
- Terminal restoration runs from the existing lifecycle guard even when frame
  rendering or exit projection fails.

## Retirement

This is internal code retirement with no persistent user-data deletion. The
selected path is delete-first after written design approval.

Retire without aliases, flags, or fallback branches:

- interactive history/live partition and live budget;
- `bound_live_blocks` and every viewport-pressure omission marker;
- normal-screen progressive history acknowledgement and protected history
  insertion for conversation entries;
- every implementation or planning path that writes assistant prefixes into
  native history;
- the old `Ctrl+O` transcript browser and its per-frame `TranscriptPane` clone;
- any card renderer `max_rows` input used only for terminal-height pressure;
- the six-line `LiveOutput` tail as the source for complete expanded output;
- duplicate normal-screen versus review-screen scroll and expansion state.

Retain:

- typed `TranscriptStore` entry identity, revisions, and event order;
- typed Workflow origin and grouping;
- retry-safe session event persistence;
- card-local semantic summaries and existing non-workflow Delegate-family
  component rendering;
- static print/pipe rendering;
- terminal lifecycle, frame scheduling, synchronized output, and differential
  redraw primitives.

No existing session file or output artifact is deleted or rewritten by the
migration. Older sessions use their available persisted result and explicit
incompleteness status.

## Alternatives Considered

### Keep inline mode and enter fullscreen only on overflow

Rejected. It preserves two physical modes, two scroll owners, migration at the
worst possible time, and different selection behavior before and after
overflow. It also cannot recover output already discarded upstream.

### Reuse the existing `Ctrl+O` browser unchanged

Rejected. It clones the complete `TranscriptPane` every frame, uses absolute
row offsets that drift when entries above grow, has entry selection rather than
cross-card text selection, and does not solve source-side output loss.

### Store complete output only in session JSONL

Rejected. Existing command updates are not lossless, and making them lossless
would force every resume and context replay to parse unbounded output events.
Large display artifacts need lazy range reads independent of model context.

### Port the grok-build rendering stack

Rejected. Neo already has the required typed entries, row cache, terminal
lifecycle, scheduler, and differential renderer. The reference project's
private dependencies and additional modes add more code without changing the
essential document model.

## Compatibility Boundary

- Existing session JSONL remains append-only and readable.
- New output-reference metadata is optional for old-session deserialization.
- Model-visible `ToolResult`, provider requests, context compaction, and cache
  prefixes keep their current semantics and limits.
- Non-workflow Delegate-family card component output remains unchanged for the
  same typed snapshot and expansion state.
- Workflow execution and typed origin remain unchanged; only document
  presentation and direct-tool expansion change.
- Task Browser remains available but no longer causes a physical screen switch
  from the already-fullscreen application.
- Print and pipe modes remain static and non-interactive.
- Cross-platform filesystem paths use `Path`/`PathBuf`; terminal and clipboard
  behavior must have Windows, Linux, and macOS evidence.

## Acceptance Criteria

### Document And Scrolling

1. A streaming assistant entry can grow beyond ten terminal heights before
   finalization; the first and latest rows remain reachable, and no omission
   marker appears.
2. A dynamic card above the viewport can grow while the user is scrolled up;
   the same logical top content remains visible.
3. Resizing and rewrapping preserve the same logical anchor when possible.
4. At the tail, new revisions follow automatically. After user scroll-up, the
   viewport remains locked and shows one new-activity indicator until the user
   returns to the tail.
5. A discarded retry attempt never enters canonical session history and never
   forces a locked viewport to the tail.

### Cards And Workflow

6. Delegate, DelegateGroup, and DelegateSwarm card output matches the current
   component output for identical snapshots and expansion state.
7. No outer renderer converts a Delegate-family card to a header-only or
   tail-only view because of terminal height.
8. A Workflow with direct tools, Delegates, and swarms renders every structural
   row in fixed main/Delegate/swarm order without omitted-row summaries caused
   by viewport height.
9. Expanding a Workflow direct tool reveals all presentation input and complete
   retained output inline under that tool; collapsing restores the existing
   one-line summary.
10. Workflow identity and grouping survive resume without duplicate top-level
    tool or Delegate-family cards.

### Output Retention

11. Bash output larger than 64 KiB and 10 MiB is reachable from first byte to
    last normalized display byte after completion and resume.
12. High-throughput Bash output does not lose chunks when the live preview or
    UI event queue is saturated.
13. Terminal output larger than its old ring capacity remains reachable after
    later reads, completion, and resume.
14. The same retention behavior holds for tool executions owned by the main
    agent, Delegate-family children, and Workflow children.
15. The TUI reads bounded visible ranges rather than loading a complete large
    output artifact into memory.
16. The model-facing result remains at its existing configured limit and never
    contains the display artifact path or unbounded output.
17. Capture or disk failure is visible and stops the affected tool; it never
    degrades to silent truncation.

### Selection And Fullscreen Lifecycle

18. Drag selection can cross text, tool, Delegate-family, and Workflow entries;
    dragging beyond the viewport auto-scrolls.
19. Double-click selects one word, and the copy control writes the materialized
    selection through the existing clipboard path.
20. Shift-drag delegates to terminal-native selection on supported terminals;
    unsupported terminals still retain application selection and copy.
21. Interactive startup emits one balanced fullscreen enter and mouse-enable
    sequence; internal views emit no additional physical transition.
22. Normal exit, error, and supported suspend/resume paths restore mouse,
    cursor, and the original terminal screen exactly once.
23. Exit prints the final answer, relevant task/workflow status, and session
    reopen hint without replaying the full transcript.

### Rendering And Performance

24. Frames never exceed terminal height or width, so terminal omitted-row
    behavior is never invoked.
25. An unchanged frame produces no terminal write.
26. Updating one visible entry invalidates that entry and later virtual row
    starts, not every prior rendered entry.
27. Animation runs at no more than 30 frames per second, pauses offscreen, and
    respects reduced motion.
28. A long transcript and a multi-million-line output remain navigable with
    bounded viewport memory; width reflow may rebuild derived layout data in
    the background but may not block output capture.

### Context Integrity

29. Existing canonical records keep byte order and content; new events append.
30. Display output artifacts never enter provider requests, compaction input,
    model context, or cache prefixes.
31. Micro compaction and snip/dedup remain independently opt-in and default off.

## Verification Plan

Use focused tests and native terminal evidence, not a broad workspace run.

### neo-tui

- document height invalidation and virtual row starts;
- logical anchor under growth, removal, resize, and wrap changes;
- tail follow, locked scroll, and new-activity indication;
- assistant growth without omission;
- unchanged Delegate-family component output;
- complete Workflow group and direct-tool expansion;
- selection mapping, cross-entry copy, auto-scroll, and double-click;
- visible-only animation scheduling and reduced motion;
- bounded frame and no-write differential behavior.

### neo-agent-core

- lossless append before Bash and Terminal caps;
- high-throughput capture with a saturated preview consumer;
- output-reference persistence, safe path handling, range reads, index rebuild,
  and capture-failure cancellation;
- unchanged bounded model-facing results and context projection.

### neo-agent

- interactive startup and shutdown physical transitions;
- full event-to-document update path;
- retry attempt removal and winning-message resume;
- Workflow grouping after resume;
- selection versus control-click input routing;
- final exit projection;
- static print/pipe behavior unchanged.

### Native Platforms

Run focused smoke checks on macOS, one Linux environment, and one Windows
environment. Verify fullscreen restoration, resize, wheel scrolling, ordinary
drag, Shift-drag bypass, clipboard copy, suspend behavior where supported, and
Bash/Terminal output capture. VM checks must follow the repository rule to run
only one VM at a time and shut it down after use.

Focused local Rust tests are not evidence that native terminals or remote
pipelines passed.

## Architecture And Baseline Follow-up

After implementation and focused verification:

1. supersede or amend ADR-0010 so fullscreen document ownership is the only
   current interactive direction;
2. replace the native-scrollback landed baseline with the verified fullscreen
   document behavior while retaining historical evidence links;
3. record the complete-output artifact shape and its model-context exclusion;
4. confirm the old history/live, omission, and `Ctrl+O` paths have zero
   remaining production references.

No accepted ADR is created from this unimplemented design alone.

## Working Artifacts

### TaskIntentDraft

- Outcome: one fullscreen document that keeps every presentation input
  reachable while entries update in place.
- Success evidence: no viewport omission, stable logical scrolling, complete
  source-side shell output, unchanged frozen cards, Workflow coverage,
  selection/copy, terminal restoration, and cross-platform smoke evidence.
- Stop condition: written design review, then a separate implementation plan;
  pause on any unresolved source-side loss or context-integrity conflict.
- Non-goals: card-body redesign, model-result expansion, provider/context
  changes, and unrelated transcript features.

### BaselineUsageDraft

- Required: AGENTS.md, ADR-0010, native terminal baseline, Workflow dynamic
  transcript design, current transcript, shell guard, session persistence, and
  grok-build reference evidence.
- Acknowledged: all required references above.
- Missing: none.
- Decision: proceed with a superseding design; the discarded assistant-prefix
  native-history direction is neither a baseline nor an implementation input.

### RequirementReadyCheck

- Requirement source: the confirmed conversation decisions and frozen card
  boundaries.
- Goal and scope: complete interactive presentation without omission, with
  static print/pipe modes retained.
- Acceptance: document, output, selection, lifecycle, performance, platform,
  and context criteria above.
- Open blocking questions: none.
- Decision: ready for written user review.

### ImpactStatementDraft

- Affected layers: `neo-tui` document/layout/input/rendering, `neo-agent`
  interactive lifecycle, and `neo-agent-core` source-side display output.
- Canonical owners: `TranscriptStore` for document content, the replacement of
  `TranscriptViewport` for logical view state, session task artifacts for full
  display output, and the existing terminal lifecycle/differential renderer
  for physical output.
- Compatibility: append-only session history, bounded model results, print and
  pipe modes, Workflow runtime, and frozen non-workflow Delegate-family cards.
- Retirement: one delete-first implementation with no inline compatibility
  branch or second review renderer.

### Complexity Budget

- Artifact class: cross-module source and decision complexity.
- Current pressure: the history/live behavior is spread across transcript
  projection, frame composition, tool output, and terminal lifecycle owners.
- Projected pressure: at risk if the plan adds another store, renderer,
  scheduler, output cache, or compatibility path.
- Governance: reuse existing typed entries, layout caches, lifecycle, and
  differential renderer; add only the lossless output artifact that current
  events cannot supply.

### Plan-Time Complexity Check

- Better boundary: split implementation work by the existing document/layout,
  output capture, input/selection, lifecycle, and Workflow presentation owners.
- Recommendation: edit existing owners and add one focused output-artifact
  owner; do not grow a generic transcript coordinator.
