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

They are siblings, not nested cards. Unrelated transcript entries retain their
canonical event order around the group.

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
[yolo] deepseek-v4-flash · working
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

## Verification Boundary

The implementation plan must use focused tests for:

- choice-picker and shared visible-width containment;
- workflow snapshot replacement and single terminalization;
- typed `workflow_origin` routing for ordinary tools, Delegate, and swarm;
- bounded main and sibling-card projections under large activity counts;
- approval and question focus during later workflow activity;
- exact frame-height accounting including separators, Todo, composer, footer,
  and blocking overlays;
- explicit-review completeness after normal-surface compaction;
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
