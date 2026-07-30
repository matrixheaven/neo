# Native Scrollback Progressive Transcript Design

## Status

Proposed design for user review. The product direction and the no-duplicate
terminal form were approved in conversation on 2026-07-30. Implementation must
not begin until this written design is approved.

This design supersedes the automatic-overflow mode, overflow latch, fixed
chrome, automatic mouse capture, and automatic alternate-screen requirements in
`2026-07-19-transcript-overflow-tool-results-design.md`. It retains the normal
screen geometry, protected history insertion, transactional rendering, and
explicit `Ctrl+O` review behavior from
`2026-07-19-terminal-live-viewport-isolation-design.md`.

## Problem

Neo currently treats the earliest mutable transcript entry as a commit barrier.
A long-running Delegate, DelegateSwarm, or workflow therefore retains every
later row in one mutable suffix. When that suffix exceeds the rows above chrome,
`NeoTui` automatically enters the alternate screen, captures the mouse, renders
an application viewport, and appends Todo, composer, and footer as fixed chrome.

That behavior has four user-visible failures:

- the shell launch line and normal terminal history disappear while overflow is
  active;
- terminal-native scrolling and ordinary text selection stop working;
- Todo, composer, and footer look like a persistent dock;
- a pending approval can be pushed outside the visible viewport by later
  Delegate or workflow updates even though it still owns input.

The implementation matches the old design. The design is wrong for Neo's
normal inline terminal experience.

## Physical Constraint

Rows in terminal scrollback are immutable from Neo's perspective. Neo cannot
both place an entire changing card in native scrollback and later rewrite its
earlier rows in place. A correct normal-screen design must therefore stop
treating an entire long-running card as one mutable object.

The selected behavior is append-only progressive presentation:

- facts that cannot change again become immutable history immediately;
- only genuinely changing state remains in the bounded live area;
- terminal completion appends a final status instead of repeating the complete
  card;
- a source row is never emitted once as progress and again inside a duplicate
  final card.

## Goals

- Never enter the alternate screen automatically.
- Preserve the shell launch line and all finalized Neo history in native
  terminal scrollback.
- Leave mouse wheel scrolling and ordinary text selection to the terminal on
  the normal screen.
- Let Todo, composer, and footer scroll away with the rest of the terminal
  viewport instead of behaving as a dock.
- Progressively append every proven-stable Delegate, DelegateGroup,
  DelegateSwarm, workflow, assistant, and tool activity row exactly once.
- Keep the pending approval visible and operable while later background work
  continues.
- Preserve typed event and state sources; never infer stability by parsing
  rendered text or regular expressions.
- Keep `Ctrl+O` as the only path that intentionally enters the alternate screen
  for complete application-owned review.

## Non-Goals

- Making terminal scrollback mutable.
- Reconstructing prior shell output inside an application viewport.
- Adding a second transcript store, event log, renderer, feature flag, height
  setting, or compatibility mode.
- Changing Delegate execution, workflow execution, approval responses, Todo
  state, tool schemas, or persistence formats.
- Re-emitting a complete final card after its activity rows have already been
  committed progressively.
- Preserving the old automatic-overflow behavior for any permission mode,
  provider, model, tool, or card type.

## User Experience

### Normal progress

Stable activity moves into terminal history while current activity stays live:

```text
chenyuanhao@Mac-mini neo % neo

● code-review workflow started

✓ Agent A · correctness · done
  Used Read images.rs
  Used Bash cargo test

✓ Agent B · security · done
  Found 2 issues

────────────────────────────────────────
● Agent C · maintainability · running
  Used Read common/mod.rs
  thinking...

Todo
● Review provider errors

>
[yolo] model · working
```

The separator is illustrative; implementation reuses existing spacing and
theme primitives rather than adding a new literal rule.

When Agent C finishes, Neo commits its remaining stable activity and appends
one terminal status. It does not print a second complete copy of Agent C.

### Native scrolling

No mouse capture is active on the normal screen. Scrolling upward moves the
whole terminal viewport, including Todo, composer, footer, finalized Neo rows,
and the shell launch line. Text remains selectable through the terminal's
ordinary selection behavior.

### Pending approval

A pending approval is the interactive live focus:

```text
✓ Agent A · done
● Agent B requested Delegate

Approval: Delegate
> Allow once
  Allow for this session
  Reject

Todo
>
```

While approval is pending:

- its card remains in the bounded live area;
- keyboard selection and confirmation continue to target the same runtime-owned
  option list;
- later stable rows are retained in memory behind the approval barrier so
  canonical transcript order is not violated;
- later mutable background state may update its typed source, but cannot replace
  or displace the approval surface;
- after resolution, deferred stable rows are committed once in canonical order.

This queue is presentation state only. It is not another durable store and does
not change approval execution or persistence.

### Explicit review

`Ctrl+O` continues to enter the alternate screen intentionally. That view may
capture the mouse because the user explicitly requested application-owned
review. Closing review returns to the unchanged normal screen and its native
scrollback. Automatic progress, yolo mode, ask mode, workflow cards, and
approval cards never enter that surface by themselves.

## Presentation Model

### One source of truth

`TranscriptStore` and its typed entries remain the only transcript source.
`TranscriptPresentation` remains the only owner that decides which source facts
are acknowledged history and which rows remain live. `InlineTerminal` remains
the only owner of physical normal-screen geometry and protected history
insertion.

No component may parse rendered ANSI or human-readable card text to determine
whether a row is stable.

### Progressive facts

Each supported mutable entry projects typed facts with stable identities. A fact
contains:

- an identity derived from the source event or structured item identity;
- canonical order within its transcript entry;
- immutable display data;
- a finality proof from typed state.

Examples include a completed child tool activity, a completed child agent, a
workflow phase transition, a finalized assistant prefix, and a completed tool
output line whose producer guarantees append-only output.

`TranscriptPresentation` records acknowledged fact identities alongside its
existing entry revision and assistant-offset ledger. A fact already
acknowledged is never rendered again. New stable facts become
`FinalizedBlock`s and use the existing two-phase terminal acknowledgement path.

### Live state

Only facts that can still change remain live, such as:

- current status and elapsed time;
- the active child tool row;
- incomplete streaming text;
- active workflow phase state;
- pending approval selection and feedback input.

The live projection must remain bounded. Existing card-local preview limits
remain in force. When many concurrent mutable items exist, the normal live area
shows a typed aggregate plus the currently active items that fit; stable facts
are never omitted. Complete current state remains available through explicit
`Ctrl+O` review.

The aggregate is presentation-only and must report real counts and states from
typed data. It must not impose a product-level agent count limit.

### Completion

Completion performs three ordered actions:

1. emit any remaining stable facts not yet acknowledged;
2. append one terminal status carrying the final outcome and final totals;
3. remove the mutable live projection.

It must not render a complete duplicate card. Failed, cancelled, interrupted,
and partially completed entries follow the same rule and preserve actionable
failure information in the final status.

## Supported Entry Families

The first implementation must cover every entry family that can currently hold
the commit frontier long enough to trigger automatic overflow:

- assistant streaming text;
- ordinary running tools with partial output;
- Delegate;
- DelegateGroup;
- DelegateSwarm;
- workflow cards;
- approval prompts;
- question prompts and other blocking dialogs represented in transcript.

An unsupported mutable entry must fail closed as bounded live state and commit
its complete canonical result once at finalization. It must not reactivate
automatic alternate-screen overflow. The implementation plan must enumerate
all current `Finalization::Live` producers and either assign a progressive
projection or justify this bounded-finalization behavior.

## Ownership And Retirement

### Retained owners

- `TranscriptStore`: typed transcript state and canonical order.
- `TranscriptPresentation`: progressive fact ledger, live projection, and
  history versus live decisions.
- existing Delegate and workflow components: typed activity and display data.
- `InlineTerminal`: physical normal-screen rendering and acknowledgement-safe
  history insertion.
- `NeoChromeState`: Todo, composer, footer, and runtime-owned approval input.

### Retired paths

The implementation must delete, not retain behind a flag:

- `NeoTui::automatic_overflow`;
- automatic overflow latching and release;
- automatic overflow viewport rendering;
- automatic overflow wheel and page-key routing;
- automatic overflow mouse capture;
- tests whose expected behavior is automatic alternate-screen entry or fixed
  chrome.

Manual transcript review and Task Browser alternate-screen behavior remain.

## Failure And Lifecycle Behavior

- A terminal write or flush failure does not acknowledge pending stable facts.
- Retried frames resend only unacknowledged stable facts through the existing
  two-phase acknowledgement path.
- Resize recomputes only the bounded live projection; acknowledged history is
  never replayed.
- Suspend, resume, and exit never enter the alternate screen merely because
  live content is tall.
- Approval cancellation, interrupt, session change, and workflow stop flush or
  terminalize their deferred presentation state through existing runtime
  resolution paths.
- Replay rebuilds typed transcript state but does not recreate already emitted
  physical scrollback. Existing session replay semantics remain unchanged.

## Acceptance Criteria

- A yolo session with a tall running workflow never emits an automatic
  alternate-screen enter sequence.
- An ask session with multiple Delegate approvals never emits an automatic
  alternate-screen enter sequence.
- The shell launch line remains reachable through native terminal scrolling
  during long-running work.
- Mouse drag selects text normally on the main surface.
- Scrolling upward moves Todo, composer, and footer out of view with the rest of
  the terminal.
- Every stable activity fact appears exactly once in native scrollback.
- Completion does not duplicate progressively committed activity.
- A pending approval stays visible, selectable, confirmable, and cancellable
  while later Delegate and workflow events arrive.
- Deferred rows after an approval retain canonical order and commit exactly
  once after resolution.
- No Delegate, workflow, provider, permission-mode, or terminal-specific
  compatibility branch preserves automatic overflow.
- `Ctrl+O` remains an explicit full review surface and continues to use one
  balanced alternate-screen enter and leave transition.
- macOS, Linux, and Windows use the same presentation decisions; only existing
  terminal-mode emission remains platform-specific.

## Verification Strategy

- Presentation tests prove progressive fact identity, canonical order,
  two-phase acknowledgement, retry without duplication, and terminal outcome
  convergence.
- Delegate, DelegateGroup, DelegateSwarm, workflow, ordinary tool, assistant,
  approval, and question regressions each prove their stable-versus-live split.
- Controller tests prove later events cannot displace or steal input from a
  pending approval.
- Virtual-terminal tests seed a shell launch line, run a tall live workload,
  and prove no automatic alternate-screen control sequence, no mouse capture,
  native scrollback retention, complete final history, and no duplicated rows.
- Explicit `Ctrl+O` tests continue to prove balanced alternate-screen and mouse
  capture lifecycle.
- Verification remains targeted by package and test target; broad workspace
  tests are not required evidence.

## Architecture Decision Signal

This design changes the transcript presentation architecture and supersedes an
approved automatic-overflow design. After implementation and verification, the
project should record or amend the terminal transcript architecture decision so
future work does not reintroduce automatic alternate-screen overflow or whole
card mutability.

## Design Inputs

### Task Intent

- Outcome: native terminal scrolling and selection remain available throughout
  long Neo sessions without hiding finalized activity or losing approval focus.
- Success evidence: no automatic alternate-screen transition; complete,
  non-duplicated progressive history; operable approval under concurrent events.
- Stop condition: written design approved, implementation plan executed, and
  targeted terminal plus controller regressions pass.
- Non-goals: tool execution changes, card execution semantics, persistence
  migration, or terminal-specific settings.

### Baseline Usage

- Required baseline:
  `2026-07-19-terminal-live-viewport-isolation-design.md`.
- Superseded baseline sections:
  `2026-07-19-transcript-overflow-tool-results-design.md` automatic overflow,
  latch, fixed chrome, and related acceptance criteria.
- Retained evidence: transcript commit frontier, two-phase acknowledgement,
  typed approval ownership, and explicit review surface behavior.
- Decision: continue with one progressive presentation owner and delete the old
  automatic path.

### Impact

- Affected layers: transcript presentation, terminal frame composition,
  interactive input routing, Delegate/workflow activity projection, approval
  focus, and focused tests.
- Canonical owner: `TranscriptPresentation` for progressive history decisions;
  `InlineTerminal` remains unchanged as physical owner unless implementation
  evidence proves a missing generic primitive.
- Main risk: falsely classifying mutable data as stable would make stale history
  permanent.
- Risk control: stability must come from typed finality proof and terminal
  acknowledgement, never display-text comparison.
