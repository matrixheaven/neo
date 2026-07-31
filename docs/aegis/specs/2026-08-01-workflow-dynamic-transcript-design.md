# Workflow Dynamic Transcript Presentation Design

## Status

ASCII direction approved by the user on 2026-08-01. This written design is
ready for user review. Workflow presentation implementation must not begin
until the written design is approved.

The permission-picker width containment described here already landed in
commit `253e711f`. It is recorded as a prerequisite and shared presentation
invariant, not as unimplemented workflow work.

This design selectively supersedes the workflow-specific presentation rules
in `2026-07-30-native-scrollback-progressive-transcript-design.md`: workflow
phase transitions and workflow-origin tool activity no longer become unrelated
standalone history rows. The landed native-terminal ownership, normal-screen
behavior, explicit `Ctrl+O` review surface, and all non-workflow Delegate-family
card behavior remain unchanged.

## Problem

Four presentation defects combine into one broken workflow experience:

1. The permission choice picker previously rendered long labels, descriptions,
   and hints without constraining them to the provided width. A 142-column row
   could reach a 120-column terminal and be rejected by `LiveRenderer`.
2. Every queued, running, and phase snapshot could become a progressive
   workflow history fact, producing repeated `Workflow ... running` rows.
3. `workflow_origin` is discarded when tool execution events enter the
   transcript. Direct tools and Delegate-family tools therefore appear as
   unrelated top-level cards.
4. Live-height budgeting counts content but can omit separators and the actual
   bottom-region height. The assembled frame is then truncated from the tail,
   which can remove Todo, composer, and footer until the workflow finishes.

The runtime state is correct. The defect is in transcript projection and frame
composition; adding another workflow runtime or transcript store would create
duplicate state without solving the presentation problem.

## Goals

- Present one bounded, in-place workflow main card from queueing through a
  terminal state.
- Keep workflow-origin direct tools visibly associated with that main card.
- Present workflow-origin `Delegate` and `DelegateSwarm` activity in two
  dedicated sibling summary-card families, with one row per visible subagent.
- Never nest the existing full Delegate-family cards inside a workflow card.
- Stop repeated workflow status rows and orphaned workflow tool cards.
- Keep approval and question input ownership unchanged and unambiguous.
- Use one width calculation and one height budget for the assembled frame.
- Preserve composer and footer during ordinary progress; preserve the existing
  intentional composer hiding while a blocking dialog owns input.
- Remain bounded with long commands, many tools, many subagents, narrow widths,
  and short terminal heights.

## Non-Goals

- Changing workflow execution, scheduling, persistence, journal, result, or
  recovery behavior.
- Changing tool schemas or model-visible workflow results.
- Redesigning non-workflow `Delegate`, `DelegateGroup`, or `DelegateSwarm`
  cards, their expansion behavior, ordering, activity rows, or placement.
- Embedding approval choices, question forms, complete tool output, complete
  child transcripts, or an internal scroll area inside workflow cards.
- Adding a feature flag, compatibility renderer, second transcript store,
  display-time agent limit, or configurable card-height system.
- Making every subagent row simultaneously visible when the physical terminal
  has fewer rows than the required content.

## Existing Owners

- `WorkflowRuntime` remains the only owner of workflow lifecycle, journal,
  result, and child references.
- `TranscriptStore` remains the typed transcript state and canonical ordering
  owner.
- `TranscriptPresentation` remains the history-versus-live projection owner.
- `InlineTerminal` and `LiveRenderer` remain the physical frame and terminal
  safety owners.
- `NeoChromeState` remains the Todo, composer, footer, approval, and question
  input owner.

No renderer may infer workflow origin, lifecycle, or child identity by parsing
human-readable text. All routing uses typed event data.

## Selected Presentation

### Workflow Main Card

```text
● Workflow  003-lifecycle-test · running · 18s
│ phase greet · 12 calls · 0 failed
│ ● Bash  $ sleep 0.3 · 1s
│ ✓ Read  report.md
│ Report  lifecycle checks passed
│ … 8 completed
```

There is exactly one main card per workflow run. Queueing, running, phase,
pausing, paused, waiting, cancellation, failure, and completion update that
same transcript entry. Non-terminal snapshots never create additional history
rows.

The body is a bounded projection, not a growing audit log. It selects, in
order: current phase or wait reason, active direct tools, most recent relevant
completed direct activity, latest report, and aggregate counts. Complete
history remains available through the existing explicit review surface and
workflow result or journal views.

### Workflow Delegate Summary

```text
● Workflow Delegates · 2 running · 1 done
├─ ✓ Euclid       [explorer] completed · 4s
├─ ● Archimedes   [coder]    Bash cargo test · 12s
└─ ◌ Ptolemy      [reviewer] queued
```

All `Delegate` calls originating from one workflow run share one sibling
summary card. Each visible subagent receives exactly one row containing
identity, role when present, state, current activity or terminal outcome, and
elapsed time when it fits.

### Workflow Swarm Summary

```text
● Workflow Swarms · schema review · 2/3 done
├─ ✓ Alpha  [reviewer] completed · 8s
├─ ● Beta   [coder]    Bash cargo test · 11s
└─ ◌ Gamma  [explorer] queued
```

All `DelegateSwarm` calls originating from one workflow run use the dedicated
swarm sibling summary. A swarm label may prefix a row when multiple swarms are
active, but each subagent still occupies one row. The card reports observed
counts only and never imposes a workflow or product-level subagent limit.

### Sibling Ordering

The three possible cards form one logical workflow presentation group in this
fixed order:

1. workflow main card;
2. workflow Delegate summary, when present;
3. workflow swarm summary, when present.

They are siblings, not nested cards. In the live area their order is the
workflow launch order. Once terminal, the group is committed at the terminal
event position; unrelated finalized transcript entries retain their canonical
event order.

## Interaction States

### Approval Or Question

```text
◆ Workflow  003-lifecycle-test · waiting for approval
│ phase verify · 7/12 calls
│ Bash  $ cargo test --workspace

Approval  Bash
▸ Allow once
  Allow for this session
  Reject
```

The workflow main card shows only the wait state and requested action summary.
The existing approval or question card remains a separate blocking transcript
entry and the sole input owner. Later workflow, Delegate, or swarm events may
update typed state but cannot displace the earliest blocking entry. Resolving
the dialog resumes updates in the same workflow cards.

### Failure

```text
✗ Workflow  003-lifecycle-test · failed · 22s
│ phase verify · 7/12 calls
│ failed  Bash cargo test
│ Reason  exit 101 · 2 tests failed
```

The main card shows the first actionable failure and terminal reason. When a
child caused the failure, the main card names the child and the relevant
Delegate or swarm summary keeps that child's failed row. Raw journals and
large output do not spill into the normal card.

### Completion

```text
✓ Workflow  003-lifecycle-test · completed · 22s
│ 3 phases · 12 calls · 0 failed
│ Report  lifecycle checks passed
```

Terminalization commits one final workflow group. It does not replay a full
tool history, append another complete card, or retain prior non-terminal status
rows. Cancelled and interrupted states use the same single-finalization rule
with their distinct terminal reason.

## Width Rules

The assembled normal frame computes `content_width` once after reserving its
left gutter and anti-autowrap column. Every dialog, card, Todo row, composer
row, and footer row receives that same effective width or a documented inner
width derived from it.

Every rendered row must satisfy:

```text
visible_width(row) <= content_width
```

Required information is retained in this order: state marker, workflow or
agent identity, failure or wait reason, active tool name, and explicit
truncation marker. Elapsed time, role, counts, and secondary descriptions are
removed from the right as space shrinks.

Commands must never be silently clipped. A shortened Bash or Terminal command
contains an explicit `…`; the exact command remains available in explicit
review. Subagent rows remain one row even when their activity is shortened.

The landed permission-picker fix is the first regression proof of this rule:
title, item content, description, and hint use the shared Unicode-aware width
truncation primitive before ANSI styling. No choice-picker-specific width
framework is added.

## Height Rules

Frame composition reserves actual bottom-region rows before asking transcript
presentation for its live budget. The calculation includes Todo, pending input,
composer, footer, gutters, borders, and every separator that will be appended.
The final frame must not rely on tail truncation to become valid.

When height is insufficient, presentation degrades in this order:

1. remove older completed direct-tool previews;
2. replace completed subagent rows with aggregate counts;
3. keep failed, running, then queued subagent rows in that priority;
4. reduce each workflow card to its header and one actionable row;
5. collapse both sibling summaries into counts on the main workflow header;
6. reduce Todo to its existing compact summary;
7. retain composer and footer during ordinary progress.

When a blocking dialog intentionally owns input, the existing rule hides the
composer and reserves the dialog plus footer instead. At physically impossible
terminal heights, the active blocking surface or composer wins; the renderer
must still produce a width-valid and height-valid frame without panicking.

Example compact projection:

```text
● Workflow  003-lifecycle-test
│ running · greet · 18s
│ ● Bash  sleep 0.3
│ … 11 more
● Delegates · 2 active · 1 done
│ … 3 agents
Todo · 4 pending
>
[yolo] provider/model · working
```

When space permits, every subagent is shown. When it does not, full current
state remains in explicit review; the normal surface shows real omitted counts
instead of imposing a fixed child count.

## Event Routing And Projection

- Preserve `workflow_origin` from runtime tool events through transcript event
  handling. Do not reconstruct it later.
- Route workflow-origin ordinary tools into the main-card activity projection.
- Route workflow-origin `Delegate` events into the Delegate summary.
- Route workflow-origin `DelegateSwarm` events into the swarm summary.
- Continue routing non-workflow tools and Delegate-family events through their
  existing card paths unchanged.
- Keep approvals and questions as independent transcript entries even when the
  requesting tool has a workflow origin.
- Use workflow run identity plus typed child identity for stable row identity;
  display names are not keys.
- Retain full typed tool and child activity once in `TranscriptStore`; normal
  presentation uses the bounded summaries while explicit review projects the
  complete detail from the same stored activity.
- Keep each non-terminal workflow group in the bounded live projection rather
  than making its launch entry a permanent mutable history barrier. On
  terminalization, acknowledge the group once at the terminal event position.
- Keep presentation state bounded. Runtime and persisted JSONL remain the only
  sources used to restore current workflow truth.

## Retirement Boundary

Implementation retires the following normal-path behavior without a fallback:

- standalone queued, running, and phase history rows for the same workflow;
- generic top-level tool cards for workflow-origin direct tools;
- generic full Delegate or DelegateSwarm cards for workflow-origin children;
- frame-tail truncation as the mechanism that protects terminal height.

It does not rewrite historical completed transcripts. Old persisted sessions
remain readable as recorded; new and resumed live events use the single new
projection path. No format migration, compatibility renderer, or dual card path
is introduced.

## Options Considered

### Keep Ordinary Tool Cards And Add An Origin Label

Rejected. It leaves one card per call, preserves transcript flooding, and makes
the workflow's current state difficult to scan.

### Nest Existing Full Delegate-Family Cards

Rejected. Nested mutable cards multiply height, duplicate interaction and
expansion semantics, and recreate the exact frame-pressure failure being fixed.

### One Main Card Plus Two Sibling Summaries

Selected. It keeps origin visible, bounds normal-screen state, preserves
existing non-workflow cards, and reuses current runtime and transcript owners.

## Complete UI Coverage

This section fixes the visible shape for every workflow lifecycle and the
surrounding interaction surface. These are projections of existing typed state,
not new lifecycle variants.

### Visual Grammar

```text
· queued       › running       ◆ input required
… pausing      Ⅱ paused        ↻ recovering
✓ completed    ✕ failed        ■ cancelled       ! resource limited
```

Queued and cancelled use neutral styling; running uses the active accent;
input-required and pausing use warning styling; completion uses success;
failure and resource limits use error styling. Only the active marker and
elapsed time animate. Status color never carries information without its text
marker.

### Frame Anatomy

The normal terminal keeps one stable vertical order. While non-terminal, a
workflow group is projected into the existing bounded live area above Todo and
the composer. Active groups are ordered by launch time, but they do not pin
later finalized assistant or tool rows behind a long-lived mutable transcript
entry:

```text
history already committed above

● Workflow  003-lifecycle-test · running · 18s
│ phase greet · 12 calls · 0 failed
│ ● Bash  $ sleep 0.3 · 1s
● Workflow Delegates · 2 running · 1 done
├─ ✓ Euclid     [explorer] completed · 4s
├─ ● Archimedes [coder]    Bash cargo test · 12s
└─ ◌ Ptolemy    [reviewer] queued
● Workflow Swarms · schema review · 2/3 done
├─ ✓ Alpha  [reviewer] completed · 8s
├─ ● Beta   [coder]    Bash cargo test · 11s
└─ ◌ Gamma  [explorer] queued

Todo · 2 pending
> view the failing test
[yolo] provider/model · working
```

The composer remains usable while a background workflow runs. Only the
earliest unresolved approval or question can replace the composer as input
owner. When the workflow terminalizes, the same logical group leaves the live
area and is committed to history exactly once at the terminal event position.
The live and final forms are never visible simultaneously, and no separate
start row is printed.

### Launch And Preflight

The workflow picker remains the existing searchable surface:

```text
╭─ Run a workflow ───────────────────────────────────────────╮
│ Search  lifecycle█                                         │
├─────────────────────────────────────────────────────────────┤
│ ▸ 003-lifecycle-test                                       │
│   Run lifecycle checks in three phases                      │
│   inputs: workspace, strict                                │
│                                                             │
│   004-provider-smoke                                       │
│   Verify provider availability                              │
├─────────────────────────────────────────────────────────────┤
│ ↑↓ navigate · Enter choose · Esc cancel                     │
╰─────────────────────────────────────────────────────────────╯
```

Once admission succeeds, the ordinary `Used Workflow` tool row is replaced by
the workflow main card; it is never followed by a second status card. A
preflight or definition failure that prevents admission still uses one compact
terminal workflow row:

```text
✕ Workflow  003-lifecycle-test · not started
│ invalid definition · phase `verify` is missing
```

No run identifier means no dynamic updates are possible, but the failed launch
still has the workflow identity and does not create a generic orphaned tool
card.

If the workflow picker has no definitions or no search result, the existing
picker remains the owner:

```text
╭─ Run a workflow ─────────────────────╮
│ Search  nightly█                     │
│                                     │
│ No matching workflows.              │
│                                     │
│ ↑↓ · Enter choose · Esc cancel      │
╰─────────────────────────────────────╯
```

### Permission Picker Width Gallery

The permission picker follows the same frame width rule. Its title, selected
label, description, page hint, borders, and ANSI styling all fit inside the
effective content width:

```text
╭ Select permission mode ───────────────────────────────────────────────╮
│ ▸ ask  ← current · Ask before ordinary tool actions                   │
│   auto          Approve safe actions; ask for risky actions            │
│   yolo          Approve ordinary actions automatically; questions…    │
│ ↑↓ navigate · Enter select · Esc cancel                               │
╰────────────────────────────────────────────────────────────────────────╯
```

At a narrow width, descriptions visibly shorten instead of producing an
invalid live row:

```text
╭ Select permission mode ─────────╮
│ ▸ ask  ← current · Ask before… │
│   auto · Approve safe actions… │
│   yolo · Approve ordinary…     │
│ ↑↓ · Enter select · Esc cancel │
╰─────────────────────────────────╯
```

The ellipsis is deliberate display truncation, never an unreported width
overflow. Permission behavior itself is unchanged by the width repair.

### Lifecycle State Gallery

The main card uses one visual row for each real `WorkflowState` and does not
invent a second runtime state for display. Labels such as `resuming` and
`not started` describe a transition or preflight result; they are not new
runtime states.

```text
· Workflow  003-lifecycle-test · queued
│ waiting for a worker permit · 0 calls
```

```text
› Workflow  003-lifecycle-test · running · 18s
│ phase greet · 12 calls · 0 failed
```

```text
◆ Workflow  003-lifecycle-test · awaiting answer
│ phase choose_target · request 4f2a
│ question is shown in the separate blocking card below
```

```text
… Workflow  003-lifecycle-test · pausing
│ current Bash continues · no new child starts
```

```text
Ⅱ Workflow  003-lifecycle-test · paused
│ paused at invocation boundary · 12 calls
│ resume is available from the task controls
```

```text
› Workflow  003-lifecycle-test · resuming
│ restoring phase greet · 12 calls already retained
```

```text
✓ Workflow  003-lifecycle-test · completed · 22s
│ 3 phases · 12 calls · 0 failed
│ Report  lifecycle checks passed
```

```text
✕ Workflow  003-lifecycle-test · failed · 22s
│ phase verify · failed Bash cargo test
│ exit 101 · 2 tests failed
```

```text
■ Workflow  003-lifecycle-test · cancelled
│ stopped by user · current child was drained or cancelled
```

```text
! Workflow  003-lifecycle-test · resource limited
│ execution stopped at the configured machine-safety limit
│ 40 calls · 2 children still pending
```

The task browser may show a short-lived recovery projection while a persisted
run is rehydrated:

```text
↻ Workflow  003-lifecycle-test · recovering
│ replaying journal · 8/12 invocation records
```

`recovering` is a view state only. If rehydration cannot safely continue, the
projection converges to one failed terminal card:

```text
✕ Workflow  003-lifecycle-test · recovery failed
│ persisted run stopped before replay completed
│ Reason  external effect status is unknown
```

### Direct Tool Gallery

Direct tools use one-line activity rows inside the main card. Their existing
typed detail remains available to the explicit review surface.

```text
› Workflow  003-lifecycle-test · running
│ ✓ Read      crates/neo-agent-core/src/workflow/state.rs
│ ✓ Edit      crates/neo-tui/src/transcript/presentation.rs +12 -4
│ ✓ Write     docs/report.md · 184 bytes
│ ● Bash      cargo test --lib … --nocapture · 3s
│ … 16 completed
```

Admission waiting is visibly different from process execution:

```text
› Workflow  003-lifecycle-test · running
│ ◌ Bash      waiting for admission · cargo test --workspace
│ ◌ Terminal  waiting for admission · cargo nextest run
```

After admission, live output stays attached to the same row:

```text
› Workflow  003-lifecycle-test · running
│ ● Terminal  cargo nextest run --workspace … · 02:41
│   running 142/450 tests · latest: workflow_user_input
```

Permission and input failures are also attached, without pretending that the
tool is still running:

```text
◆ Workflow  003-lifecycle-test · awaiting approval
│ ◆ Bash  approval requested · cargo test --workspace
```

```text
› Workflow  003-lifecycle-test · running
│ ✕ Bash  denied · permission mode rejected the command
│ workflow continues because the script handled the failed outcome
```

```text
✕ Workflow  003-lifecycle-test · failed
│ ✕ Edit  invalid input · expected exactly one replacement
│ terminal reason · required edit could not continue
```

Commands that do not fit are marked with an explicit ellipsis. They are never
silently shortened, especially in Delegate or DelegateSwarm child activity:

```text
│ ● Bash  cargo test --package neo-agent-core … --nocapture
```

### Parallel Direct Tools

Parallel work is shown as multiple active rows only while the available height
allows it. The header reports observed counts, not a display-imposed limit:

```text
› Workflow  004-provider-smoke · running · 3 active
│ ● Read      provider.rs · 0.4s
│ ● Bash      cargo test --lib … · 1.2s
│ ● Terminal  cargo check … · 1.0s
│ … 9 completed · 0 failed
```

On a narrow frame the active set collapses deterministically, retaining the
most actionable row and real counts:

```text
› Workflow  004-provider-smoke
│ 3 active · 9 completed · 0 failed
│ ● Bash  cargo test …
```

### Delegate Summary Gallery

The dedicated Delegate summary covers child admission, current activity,
approval waiting, answer waiting, success, failure, cancellation, and a child
that finished while another remains active:

```text
● Workflow Delegates · 1 running · 1 waiting · 2 done
├─ ✓ Euclid       [explorer] completed · 4s
├─ ◆ Archimedes   [coder]    approval needed · Bash cargo test
├─ ● Hypatia      [reviewer] Read report.md · 12s
└─ ✓ Ptolemy      [reviewer] completed · 19s
```

```text
✕ Workflow Delegates · 1 failed · 2 done
├─ ✕ Archimedes   [coder]    failed · provider rejected response format
├─ ✓ Euclid       [explorer] completed · 8s
└─ ✓ Ptolemy      [reviewer] completed · 9s
```

```text
■ Workflow Delegates · cancelled · 1 stopped
├─ ■ Archimedes   [coder]    cancelled · stopped by user
├─ ✓ Euclid       [explorer] completed · 8s
└─ ◌ Ptolemy      [reviewer] never started
```

### Swarm Summary Gallery

The swarm card keeps the group label and one row per child without embedding
the full expanded swarm card:

```text
● Workflow Swarms · security review · 2/4 done
├─ ✓ Alpha   [reviewer] completed · 8s
├─ ● Beta    [coder]    Bash cargo test … · 11s
├─ ◆ Gamma   [explorer] waiting for answer
└─ ◌ Delta   [reviewer] queued
```

```text
✕ Workflow Swarms · security review · 3/4 done
├─ ✓ Alpha   [reviewer] completed · 8s
├─ ✕ Beta    [coder]    failed · exit 101
├─ ✓ Gamma   [explorer] completed · 10s
└─ ✓ Delta   [reviewer] completed · 12s
│ reason · required child result unavailable
```

When multiple swarm groups exist, the card header identifies the selected group
and every child row keeps its group label:

```text
● Workflow Swarms · 2 groups · 5/7 done
├─ schema review / Alpha   ✓ [reviewer] completed
├─ schema review / Beta    ● [coder]    Bash cargo test …
├─ provider review / Gamma ✓ [explorer] completed
├─ provider review / Delta ◆ [reviewer] waiting for answer
└─ … 2 more child rows
```

### Phase, Log, Report, Artifact, And Repair Rows

Workflow-native semantic activity stays in the main card and does not pretend
to be an ordinary tool call:

```text
› Workflow  003-lifecycle-test · running
│ Phase     verify · entered after prepare
│ Log       checking provider compatibility
│ Report    18 checks passed · 2 warnings
│ Artifact  lifecycle-report.json · 48 KiB
│ … 12 direct tools completed
```

Structured child-result repair remains visible on the owning child row:

```text
● Workflow Delegates · 1 repairing · 2 done
├─ ● Archimedes [coder] repairing structured result · attempt 1
├─ ✓ Euclid     [explorer] completed · 8s
└─ ✓ Ptolemy    [reviewer] completed · 9s
```

If repair fails, the child and main card converge without claiming a successful
requested result:

```text
✕ Workflow  003-lifecycle-test · failed
│ child Archimedes completed work but returned an invalid result
✕ Workflow Delegates · 1 failed · 2 done
├─ ✕ Archimedes [coder] schema repair failed · expected object
├─ ✓ Euclid     [explorer] completed · 8s
└─ ✓ Ptolemy    [reviewer] completed · 9s
```

### Final Workflow Group With Children

When a workflow with children completes, the three sibling cards terminalize
together. The main card does not absorb or duplicate child rows:

```text
✓ Workflow  003-lifecycle-test · completed · 31s
│ 4 phases · 24 calls · 0 failed
│ Report  lifecycle checks passed
✓ Workflow Delegates · 3/3 done
├─ ✓ Euclid       [explorer] completed · 8s
├─ ✓ Archimedes   [coder]    completed · 21s
└─ ✓ Ptolemy      [reviewer] completed · 12s
✓ Workflow Swarms · security review · 4/4 done
├─ ✓ Alpha  [reviewer] completed · 8s
├─ ✓ Beta   [coder]    completed · 14s
├─ ✓ Gamma  [explorer] completed · 10s
└─ ✓ Delta  [reviewer] completed · 12s
```

If every terminal child row cannot fit, the normal terminal keeps failed rows,
then the most recent completed rows, plus exact omitted counts. Explicit review
retains every child identity.

### Blocking Approval

The main and sibling cards announce the block, but the approval card remains
the only input surface:

```text
◆ Workflow  003-lifecycle-test · awaiting approval
│ phase verify · Delegate Archimedes requested Bash
│ Bash  cargo test --workspace

Approval  Archimedes · Bash
│ command: cargo test --workspace
│ cwd: /Users/chenyuanhao/Workspace/neo
▸ Allow once
  Allow for this session
  Reject
```

After rejection, the approval card terminalizes once and the workflow card
shows the typed outcome:

```text
› Workflow  003-lifecycle-test · running
│ ✕ Archimedes · Bash rejected · script handled failure
```

If the rejection is required for progress, the main card converges instead to:

```text
✕ Workflow  003-lifecycle-test · failed
│ required approval rejected · phase verify did not continue
```

### Blocking Question And Answer

`AwaitingUser` is a separate question owner. The workflow card never embeds a
JSON editor:

```text
◆ Workflow  003-lifecycle-test · awaiting answer
│ phase choose_target · 4 calls · 1 pending question

Question  Choose deployment target
│ Select the environment for the verification run.
│
│ target: [staging________________________]
│ strict: [✓]
│
▸ Submit
  Cancel
```

Structured-answer validation stays inside the question surface:

```text
Question  Choose deployment target
│ target: []
│ Error: target is required
│
▸ Submit
  Cancel
```

After a valid answer, the question card becomes a single resolved row and the
workflow returns to `queued` or `running` according to the existing transition:

```text
✓ Answered  Choose deployment target · staging
› Workflow  003-lifecycle-test · running
│ phase verify · resumed after answer
```

Ordinary resume cannot bypass an unanswered request. The task controls show the
reason instead of silently changing workflow state:

```text
◆ Workflow  003-lifecycle-test · answer required
│ resume is unavailable until request 4f2a is answered
```

### Pause, Resume, And Stop

Pause is a request, not an immediate claim that all work stopped:

```text
… Workflow  003-lifecycle-test · pausing
│ current invocation may finish · no new invocation will start
│ [pause requested]
```

After children drain, the same card becomes:

```text
Ⅱ Workflow  003-lifecycle-test · paused
│ safe boundary reached · 12 calls · 2 children retained
```

Resume keeps the card identity and returns through `queued` before `running`:

```text
› Workflow  003-lifecycle-test · resuming
│ restoring phase verify · pending child state retained
```

Stop uses one explicit confirmation surface in task controls, then converges:

```text
Stop workflow?
│ 003-lifecycle-test · 1 active child · 2 queued children
▸ Stop
  Cancel
```

```text
■ Workflow  003-lifecycle-test · cancelled
│ stopped by user · 12 calls · 1 child cancelled · 2 never started
```

### Multiple Workflows And Background Updates

Independent workflows never merge their state or children:

```text
› Workflow  003-lifecycle-test · running · 18s
│ phase verify · 1 active tool

Ⅱ Workflow  004-provider-smoke · paused
│ safe boundary reached · 7 calls

· Workflow  005-nightly-check · queued
│ waiting for a worker permit
```

If the terminal cannot show all groups, the oldest and currently-blocking group
remain visible, while other groups collapse to real counts:

```text
› Workflow  003-lifecycle-test · running
│ … 3 other workflows · 2 paused · 1 queued
Todo · 2 pending
>
```

No background update may move a later workflow ahead of an earlier blocking
dialog or change the focused input owner.

### Task Browser Overview

The task browser is the explicit complete-review surface, not a second live
transcript. Its list uses the existing workflow row states:

```text
Tasks  ·  All
────────────────────────────────────────────────────────────
› 003-lifecycle-test     running    phase verify       18s
Ⅱ 004-provider-smoke     paused     safe boundary       7 calls
✓ 002-lifecycle-test     completed  3 phases           22s
✕ 001-provider-smoke     failed     exit 101            9s
↻ 006-recovery-check     recovering replaying journal
────────────────────────────────────────────────────────────
Workflow detail · Enter open · S stop · Esc close
```

The workflow detail page exposes complete state without changing the normal
card layout:

```text
Workflow  003-lifecycle-test · running
────────────────────────────────────────────────────────────
Overview   phase verify · 12/20 calls · 0 failed · 18s
Steps      ✓ greet   ✓ prepare   › verify   · report
Children   2 delegates · 1 swarm · 2 active
Reports    lifecycle checks passed
Output     result · journal · artifacts
────────────────────────────────────────────────────────────
Selected step: verify
  Bash cargo test --workspace
  Read crates/neo-tui/src/transcript/presentation.rs
```

Selecting a child opens complete child detail rather than nesting it in the
normal transcript:

```text
Workflow 003-lifecycle-test / Archimedes
────────────────────────────────────────────────────────────
Role      coder
State     running · 12s
Activity  Bash cargo test --workspace
Tools     Read 3 · Edit 1 · Bash 2
Result    pending
────────────────────────────────────────────────────────────
```

The explicit transcript review can expand the same workflow group without
altering normal scrollback:

```text
Transcript Review / Workflow 003-lifecycle-test
────────────────────────────────────────────────────────────
Main       running · phase verify · 12 calls · 0 failed
Direct     Read 4 · Edit 2 · Bash 3 · Terminal 1
  Bash     cargo test --workspace --all-features
  cwd      /Users/chenyuanhao/Workspace/neo
  output   running 142/450 tests
Delegates  2 active · 1 done
Swarms     security review · 2/4 done
Reports    lifecycle checks passed
────────────────────────────────────────────────────────────
```

Review may scroll and show complete commands and child activity. Closing it
returns to the unchanged normal screen and does not acknowledge new history.

The workflow result, journal, artifacts, and artifact content views keep their
existing paging and byte limits. A large result is shown as an artifact
reference plus a range-readable content view, never dumped into the dynamic
card.

### Task Browser Answer And Save Surfaces

Pending workflow answers can be completed from the task browser when the main
transcript is no longer at the bottom:

```text
Workflow 003-lifecycle-test · answer required
────────────────────────────────────────────────────────────
Prompt    Choose deployment target
Schema    {"target":"string","strict":"boolean"}
Value     {"target":"staging","strict":true}
────────────────────────────────────────────────────────────
▸ Submit answer
  Dismiss
```

Workflow save and replace remain explicit controls and do not masquerade as a
running card:

```text
Save workflow
────────────────────────────────────────────────────────────
Name      003-lifecycle-test
Scope     project
Replace   existing definition 003-lifecycle-test
Target    .neo/workflows/003-lifecycle-test.workflow.toml
────────────────────────────────────────────────────────────
▸ Save replacement
  Cancel
```

### Narrow Width Gallery

At 80 columns the card keeps identity and action while dropping secondary
metadata:

```text
› Workflow 003-lifecycle-test · running
│ verify · 12 calls · 0 failed
│ ● Bash cargo test …
● Delegates · 2 active · 1 done
├─ ● Archimedes Bash test …
└─ ✓ Euclid done
Todo · 2 pending
>
```

At 40 columns the workflow group collapses but remains identifiable:

```text
› Workflow 003-lifecycle-test
│ verify · 12 calls
│ ● Bash  cargo …
● Delegates · 2 active
│ … 1 done
Todo · 2 pending
>
```

At a physically impossible height, the renderer keeps the input owner and
valid frame geometry; the workflow group is reduced to one status row rather
than removing the composer, footer, or blocking dialog:

```text
› Workflow 003-lifecycle-test · running
>
[yolo] working
```

### Animation, Ordering, And Scrollback

- Only elapsed time and the currently active row animate. Animation never
  appends a new transcript line.
- `projection_sequence` rejects stale snapshots; a late child event cannot
  rewind a completed or newer workflow card.
- Normal-screen scrollback contains finalized workflow groups exactly once.
  Automatic alternate-screen entry and automatic mouse capture never occur.
- `Ctrl+O` remains the explicit complete-review surface; it may capture the
  mouse and render full child detail without changing the normal history ledger.
- Approval and question cards retain their canonical transcript position even
  when background workflow events arrive.
- A workflow-origin event without a valid typed parent identity fails closed as
  one bounded terminal error row; it does not become an unowned mutable card.

## Acceptance Criteria

1. A workflow moving through queued, running, multiple phases, waiting, and
   completion produces one main card and one terminal workflow group, with no
   repeated non-terminal history rows.
2. Workflow-origin Read, Edit, Write, Bash, Terminal, and other ordinary tools
   never appear as orphaned top-level cards.
3. Workflow-origin Delegate and DelegateSwarm children appear only in their
   dedicated sibling summaries; each visible subagent uses one row.
4. Non-workflow Delegate, DelegateGroup, and DelegateSwarm cards render exactly
   as before, including expansion and activity behavior.
5. Approval and question dialogs retain canonical ordering, keyboard focus,
   and intentional composer hiding.
6. Long Unicode labels, descriptions, paths, commands, and ANSI-styled rows fit
   at widths 20, 40, 80, 120, and 200 without `LiveRenderer` rejection.
7. At representative short and normal heights, workflow progress never removes
   composer or footer except for the established blocking-dialog rule.
8. One hundred direct tools and one hundred subagents remain bounded, report
   real aggregate counts, and impose no execution limit.
9. Completed, failed, cancelled, and interrupted workflows finalize exactly
   once without full-card duplication.
10. Existing explicit review shows complete current workflow, child, command,
    and tool detail after the normal surface compacts it.
11. Focused virtual-terminal evidence covers Windows-style and Unix-style
    command text; native Windows, Linux, and macOS smoke evidence is reported
    separately and is not inferred from Rust tests.
12. Queued, running, awaiting-user, pausing, paused, completed, failed,
    cancelled, and resource-limited snapshots each render their distinct state
    without creating a second lifecycle model.
13. Pause remains visibly pending until the invocation boundary, resume cannot
    bypass awaiting-user, and confirmed stop converges to one cancelled card.
14. Recovery is a view projection only; successful replay returns to canonical
    state and recovery failure produces one actionable terminal card.
15. Shell admission waiting, running, output progress, approval waiting,
    rejection, and terminal failure never share a misleading status marker.
16. Multiple workflows retain independent groups, stable order, and the
    earliest blocking input owner under continuous background updates.
17. The permission picker fits wide and narrow frames without changing
    permission behavior or producing an over-width live row.
18. Task-browser overview, workflow detail, child detail, answer, stop, save,
    result, journal, and artifact views expose complete information that the
    bounded normal card omits.
19. A long non-terminal workflow does not pin later finalized transcript rows;
    terminalization moves the same logical group from live projection to
    history exactly once.

## Verification Boundary

The implementation plan must use focused tests for:

- choice-picker and shared visible-width containment;
- workflow snapshot replacement and single terminalization;
- exhaustive lifecycle projection including resource limits and recovery;
- typed `workflow_origin` routing for ordinary tools, Delegate, and swarm;
- distinct shell admission, execution, approval, denial, and completion rows;
- bounded main and sibling-card projections under large activity counts;
- pause, boundary drain, resume, answer-required, and confirmed-stop flows;
- multiple concurrent workflow groups under background updates;
- approval and question focus during later workflow activity;
- exact frame-height accounting including separators, Todo, composer, footer,
  and blocking overlays;
- long-lived workflow live projections releasing later finalized rows;
- explicit-review and task-browser completeness after normal compaction;
- unchanged non-workflow Delegate-family rendering.

The plan must name one package, one target selector, and a narrow test filter for
each proof. Focused local Rust evidence must not be reported as native-terminal
or cross-platform acceptance.

## Design Working Artifacts

### Task Intent

- Outcome: one coherent, bounded workflow presentation that keeps tools and
  child agents associated without displacing input chrome.
- Success evidence: no repeated status rows, no orphaned workflow tools, valid
  frames at narrow dimensions, and unchanged non-workflow Delegate-family
  cards.
- Stop condition: written design approved; implementation remains a separate
  planned phase.
- Non-goal: workflow runtime or persistence redesign.

### Baseline Read Set

- `docs/aegis/specs/2026-07-30-native-scrollback-progressive-transcript-design.md`
- `docs/aegis/baseline/2026-07-31-native-terminal-transcript.md`
- `.tmp/workflow-ai-usability-report-003.md`
- current workflow card, transcript event handler, progressive presentation,
  frame composition, live renderer, and choice picker sources.

### Baseline Usage

- Product requirement: the user-approved ASCII presentation in this design.
- Runtime boundary: existing workflow runtime and transcript ownership remain.
- Result: the current implementation has presentation drift and one previously
  landed width repair; proceed with a single replacement projection after the
  written specification is approved.

### Impact Statement

- Affected layers: TUI transcript event routing, workflow presentation,
  progressive projection, and normal-frame height composition.
- Invariants: typed origin, one runtime, one transcript store, canonical
  blocking focus, explicit review completeness, and unchanged non-workflow
  Delegate-family cards.
- Compatibility: persisted formats and historical transcript text remain
  readable; no compatibility branch is added to the live path.
- Architecture review signal: yes. Completion should record the selected
  workflow projection and synchronize the landed terminal baseline after the
  implementation is verified.
