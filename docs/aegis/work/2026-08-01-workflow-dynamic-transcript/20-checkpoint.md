# Workflow Dynamic Transcript Execution - Checkpoint

- Task ID: 2026-08-01-workflow-dynamic-transcript
- Current todo: Define first execution slice.
- Active slice: initial
- Blocked on: none
- Next step: Read baseline refs and start the next safe slice.

## Checkpoint Update

- Current todo: Tasks 1-7: implement approved workflow dynamic transcript
- Active slice: Task 1: carry live workflow provenance through Delegate-family and question events
- Completed todos:
- none
- Evidence refs:
- docs/aegis/plans/2026-08-01-workflow-dynamic-transcript.md
- ed7ceb49
- Blocked on: none
- Next step: Dispatch Task 1 implementer, then spec review and code-quality review

## DriftCheckDraft

- Scope status: Aligned with approved Tasks 1-7
- Compatibility status: Runtime, persistence, non-workflow cards, and blocking input remain protected
- Retirement status: WorkflowTransition, workflow-origin top-level cards, and tail truncation remain scheduled for deletion
- New risk signals:
- none
- Advisory decision: continue

## Checkpoint Update

- Current todo: Task 2: make one workflow entry own typed activity
- Active slice: Task 2 preparation
- Completed todos:
- Task 1: carry live workflow provenance through Delegate-family and question events
- Evidence refs:
- task1-workflow-origin-tests
- task1-two-stage-review
- Blocked on: none
- Next step: Commit Task 1, then dispatch a fresh Task 2 implementer with the approved slice boundary.

## DriftCheckDraft

- Scope status: Task 1 remains aligned: one live-only typed origin, no inference, no session migration. File scope expanded only to existing carriers, consumers, and test constructors required by the enum change.
- Compatibility status: Session JSON remains unchanged; non-workflow events use None; non-workflow Delegate-family presentation behavior is unchanged.
- Retirement status: No fallback, alias, queue correlation, or duplicate provenance owner was added.
- New risk signals:
- Representative JSON test covers one variant; all nine fields carry identical serde and schemars skip attributes, leaving low residual schema-regression risk.
- Advisory decision: continue

## DriftCheckDraft

- Scope status: Task 2 aligned after review repairs; terminal_scrollback assertion changed only to retire the obsolete transition-history expectation.
- Compatibility status: Session persistence and origin-free Delegate-family paths remain unchanged; shell events now update only existing model tool entries.
- Retirement status: WorkflowTransition and its capture/render paths remain deleted; no queue, origin inference, compatibility renderer, or second activity store was added.
- New risk signals:
- Task 2 proof is deterministic local Rust coverage; native terminal and cross-platform behavior remain for later tasks.
- Advisory decision: continue

## Checkpoint Update

- Current todo: Tasks 3-7 concurrent implementation followed by unified review
- Active slice: Dispatch disjoint renderer, presentation-geometry, and decision-record implementation agents
- Completed todos:
- Task 1: live workflow provenance
- Task 2: one workflow entry owns typed activity
- Evidence refs:
- task2-typed-workflow-activity
- Blocked on: none
- Next step: Commit Task 2, then dispatch Tasks 3-7 in parallel with disjoint file ownership and perform one unified spec review plus one unified quality review.

## Checkpoint Update

- Current todo: Commit the completed workflow dynamic transcript implementation.
- Active slice: Unified review and final verification complete.
- Completed todos:
- Task 3: bounded workflow main card with direct-tool activity.
- Task 4: workflow Delegate and DelegateSwarm sibling summaries.
- Task 5: single terminal workflow-group commit and blocking-input ownership.
- Task 6: final-frame row budgeting and removal of normal-path tail truncation.
- Task 7: ADR and current-baseline amendment.
- Unified review repairs: main-card context ordering and stable swarm item ordering.
- Evidence refs:
- task3-task7-unified-verification
- unified-review-repairs
- Blocked on: none for the implementation commit.
- Next step: Stage the scoped files and create one conventional commit.

## DriftCheckDraft

- Scope status: Aligned with the approved design and implementation plan; task3-7 changes stay in workflow projection, transcript presentation, frame composition, focused tests, and the approved decision records.
- Compatibility status: Non-workflow Delegate-family cards, workflow runtime and persistence, blocking input ownership, normal-screen behavior, and native scrollback ownership remain unchanged.
- Retirement status: Workflow transition history, isolated workflow-origin cards, and post-chrome normal-path tail truncation remain removed with no aliases or fallback renderer.
- New risk signals:
- Windows and Linux native terminal behavior and remote CI remain unverified.
- The Aegis workspace structure check is still blocked by pre-existing missing INDEX targets and legacy ADR format findings outside this task.
- Advisory decision: continue to scoped commit.

## Checkpoint Update

- Current todo: commit the transcript ordering and resume-provenance repair.
- Active slice: final review and scoped commit.
- Completed todos:
- Retained typed terminal child-fact capture so trimmed commands are not lost, while removing standalone fact history; ordinary Delegate-family entries now commit one parent-owned block with the header before children and tools.
- Updated repeated fact identities with later complete payloads while retaining their original tool position; Group and Swarm terminal rows follow parent-card order even when children finish out of order.
- Kept a zero-budget terminal Workflow as a history barrier so later assistant history cannot commit ahead of it.
- Bounded terminal Workflow history before the final assistant message.
- Persisted optional question and Delegate-family workflow provenance, including compact progress events.
- Preserved old-session readability without migration or source inference.
- Evidence refs:
- transcript-ordering-resume-regression
- Blocked on: final unified reviewer rerun only.
- Next step: rerun the reviewer and exact verification, then stage and commit the scoped repair.

## DriftCheckDraft

- Scope status: The user-approved follow-up supersedes the earlier live-only provenance boundary because new sessions otherwise lose grouping after resume.
- Compatibility status: Old JSONL without provenance still deserializes to `None`; no historical records are rewritten and no text/order inference was added.
- Retirement status: Standalone progressive history and its separate acknowledgement owner remain deleted; typed facts are capture-only until the existing parent entry terminalizes and commits once.
- New risk signals:
- Already persisted child events without provenance cannot be reliably reparented.
- Native Windows and Linux terminal behavior and remote CI remain unverified.
- Advisory decision: continue to unified review and scoped commit.

## Checkpoint Update

- Current todo: commit the verified workflow transcript repair.
- Active slice: scoped staging and commit.
- Completed todos:
- Second unified review found that older terminal progress could clear a newer complete outcome at the shared progress merge owner.
- `apply_agent_progress` now rejects older snapshots before changing any field; equal timestamps can still supply a more complete terminal payload.
- The direct regression plus workflow-origin round-trip, JSONL replay grouping, Swarm terminal ordering, zero-budget Workflow barrier, permission-width, formatting, diff, and retired-symbol checks pass.
- Evidence refs:
- final-stale-progress-review-repair
- Blocked on: none for the scoped commit.
- Next step: run the workspace structure check, store the completion context, then stage and commit only the listed task files.

## DriftCheckDraft

- Scope status: The repair stays at the existing shared progress merge owner and one focused integration test.
- Compatibility status: Equal-timestamp payload completion remains accepted; only strictly older snapshots are ignored.
- Retirement status: No fallback, compatibility path, duplicate state owner, or retired progressive-history symbol was added.
- New risk signals:
- Native Windows and Linux terminal behavior and remote CI remain unverified.
- Advisory decision: proceed to scoped commit.

## Checkpoint Update

- Current todo: commit the terminal Delegate-family ordering repair.
- Active slice: final verification and scoped commit.
- Completed todos:
- Corrected the parent-owned terminal projection so each DelegateGroup and DelegateSwarm child is immediately followed by that child's tools and result.
- Kept parent child order stable even when children finish out of order, and matched tools by typed agent id plus run count.
- Preserved the existing single-Delegate terminal order and all live card rendering.
- Evidence refs:
- delegate-family-terminal-child-grouping
- Blocked on: none for the scoped commit.
- Next step: run final focused verification, store the resolved error, then stage and commit the four task files.
