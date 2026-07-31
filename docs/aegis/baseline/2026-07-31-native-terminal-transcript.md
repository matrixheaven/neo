# Native Terminal Transcript Presentation — Landed Baseline

Status: `recorded-from-work`
Date: `2026-08-01`
ADR: `docs/aegis/adr/ADR-0010-native-terminal-transcript-presentation.md`

This baseline records the landed native-scrollback progressive transcript
presentation implemented by these commits, in task order:

- `28efaf4f` — `refactor(tui): add progressive transcript facts`
  (typed fact identities, store capture cache, acknowledged-fact ledger,
  bounded live area, two commit rules, `QueuedMessage` retirement);
- `5a7e765c` — `fix(tui): stream stable delegate activity`
  (Delegate/DelegateGroup/DelegateSwarm tool and terminal facts captured at
  store update time, one terminal status per completed card);
- `943a3abd` — `fix(tui): preserve blocking transcript focus`
  (the original implementation captured workflow transition facts by
  projection sequence; that workflow-specific rule is superseded below.
  Transcript position for every approval, typed `QuestionPrompt` entry as the
  single visible question owner, and earliest-blocking-entry input routing
  remain current);
- `336e8d1f` — `fix(tui): bound mutable transcript entries`
  (every live-producing entry family covered by a progressive projector, a
  blocking projector, or the bounded finalization fallback);
- `1ee41660` — `fix(tui): keep ordinary transcript on normal screen`
  (automatic alternate-screen overflow, latch, viewport, fixed chrome, mouse
  capture, and wheel routing deleted);
- `3e9fb6fb` — `test(tui): cover native progressive scrollback`
  (virtual-terminal proof for tall Delegate/workflow/approval content on the
  normal screen without capture, facts exactly once).

The workflow-specific presentation rule was amended by the approved
`docs/aegis/specs/2026-08-01-workflow-dynamic-transcript-design.md`. The
implementation branch evidence available at this baseline update is:

- `29fa1cfc` — `fix(core): preserve live workflow child origin`
  (live-only typed origin for workflow-hosted tools, Delegate-family activity,
  and questions without changing persisted session JSON);
- `39d6b40d` — `fix(tui): group typed workflow activity`
  (one workflow transcript entry owns direct tools and child snapshots;
  mutable workflow transition facts and isolated workflow-origin entries are
  removed).

These commits establish provenance and the single transcript state owner. This
baseline update does not claim later presentation, terminal-frame, or focused
test evidence that was not yet part of the branch at `39d6b40d`.

## Landed behavior

- Ask, auto, and yolo conversation never enters the alternate screen
  automatically. Only explicit `Ctrl+O` review and Task Browser use the
  alternate screen; only those explicit surfaces capture the mouse.
- On the normal screen the terminal owns wheel, selection, and scrollback;
  the shell launch line stays reachable, and Todo/composer/footer scroll away
  with the viewport.
- Stable facts (completed non-workflow child tools, terminal non-workflow agents
  and swarm items, and resolved blocking dialogs) enter native scrollback
  exactly once with typed identity and typed finality. Accepted non-terminal
  workflow transitions are no longer history facts.
- One workflow run has one dynamic logical group: one main card plus a workflow
  Delegate summary and workflow DelegateSwarm summary when those families are
  present. Direct workflow-origin tools belong to the main card; each visible
  child occupies one row in its sibling summary.
- Queue, running, phase, log, report, paused, and waiting updates mutate that
  group only in the live area. A terminal event submits the whole logical group
  to history once, at the terminal event position.
- The live area is actually bounded by `live_budget`; completion appends
  remaining unacknowledged facts plus one final status and never a duplicate
  complete card.
- Final live cost is the true rendered row cost, including wrapping and every
  separator, after reserving the fitted Todo, pending input, composer, footer,
  borders, gutters, and cursor row. Tail truncation after `append_chrome` is
  retired rather than used to delete the bottom interaction region.
- The earliest unresolved approval/question owns input by transcript order;
  later events cannot displace it, and deferred rows commit once in canonical
  order after resolution.
- `InlineTerminal` write/flush/acknowledge ordering and failure rollback are
  unchanged; assistant source proofs never rewind; failed model attempts stay
  out of terminal history until the attempt is canonical; tool and shell live
  output still reassemble split lines and control sequences.
- Non-workflow Delegate, DelegateGroup, and DelegateSwarm card layouts,
  expansion, ordering, activity rows, and transcript placement are unchanged.
  Approval and question entries remain independent input owners. Workflow
  runtime, journal, result, recovery, and persistence remain unchanged;
  `Ctrl+O` review still shows complete current state from the same transcript
  data.
- Ordinary conversation remains on the normal screen with native scrollback;
  terminal wheel, selection, shell launch-line access, write ordering, and
  acknowledgement rules are unchanged.

## Original focused verification

The commands below are the evidence recorded for the original native-scrollback
implementation. The Task 3 workflow-transition assertion is historical
evidence only; its accepted-transition rule is superseded by the single
terminal-group rule above and is not current workflow acceptance evidence.

Each plan task's exact commands passed (one package, one target selector, one
exact test):

- Task 1: `transcript::presentation::tests::progressive_facts_retry_until_ack_then_never_replay`,
  `progressive_transcript::unsupported_live_entry_stays_bounded_and_commits_once`,
  `progressive_transcript::stable_facts_after_ordinary_live_entry_keep_canonical_order`.
- Task 2: `transcript_store::delegate_family_captures_terminal_facts_before_activity_trimming`,
  `transcript_store::delegate_to_group_replacement_preserves_progressive_fact_identity`,
  `progressive_transcript::delegate_family_completion_appends_one_terminal_status_without_complete_card_duplicate`,
  `multi_agent_transcript::option_b_expanded_swarm_preserves_full_child_transcripts`.
- Task 3: `workflow_transcript::workflow_phase_and_terminal_facts_commit_once_by_projection_sequence`,
  `progressive_transcript::pending_approval_defers_later_facts_in_canonical_order`,
  `todo_question::earliest_blocking_entry_keeps_focus_across_later_requests`,
  `modes::interactive::tests::pending_approval_keeps_input_while_later_delegate_events_arrive`
  and `pending_question_keeps_input_while_later_workflow_events_arrive` (both `--include-ignored`).
- Task 4: `transcript_pane::retry_attempt_stays_out_of_terminal_history_until_message_finishes`,
  `transcript::presentation::tests::assistant_stable_prefix_never_rewinds_when_markdown_becomes_reference_based`,
  `progressive_transcript::every_live_entry_family_is_bounded_or_progressive`,
  `tool_cards::tool_call_live_output_reassembles_split_lines_and_ansi`,
  `tool_cards::shell_run_live_output_reassembles_split_control_sequences`.
- Task 5: `terminal_frame::tall_live_projection_stays_on_normal_screen_without_mouse_capture`,
  `terminal_frame::transcript_browser_frame_is_bounded_and_marked_review_surface`,
  `modes::interactive::tests::tall_transcript_keeps_prompt_input_on_normal_screen` (`--include-ignored`).
- Task 6: `terminal_scrollback::native_scrollback_keeps_shell_and_progressive_history_exactly_once`,
  `terminal_scrollback::review_surface_transition_preserves_primary_scrollback`,
  `terminal_scrollback::review_acknowledgement_does_not_advance_normal_history_ledger`,
  `terminal_scrollback::delegate_workflow_approval_live_content_stays_on_normal_screen_without_capture`,
  plus the Task 3 controller tests.

Lingering-reference search is clean:

```text
rg -n "automatic_overflow|live_overflow|has_live_frontier|handle_automatic_overflow_event|scroll_automatic_overflow" crates/neo-tui crates/neo-agent/src/modes/interactive
```

returns zero results. Every remaining alternate-screen and mouse-capture caller
belongs to explicit `Ctrl+O` review or Task Browser. There is one visible
question owner (the transcript card), one active blocking-focus decision, no
complete-card replay after progressive facts, and no production `QueuedMessage`
path.

The workflow amendment retires these additional paths without aliases or
fallbacks: `WorkflowTransition` history, `WorkflowTransitionFact`,
`ProgressiveFactId::WorkflowTransition`, `capture_workflow_transition`,
isolated top-level workflow-origin tool and Delegate-family cards,
post-`append_chrome` tail truncation, any compatibility renderer, and any
second workflow state owner.

## Workflow amendment evidence boundary

- This update records the current architecture rule and the landed task-one and
  task-two commits listed above.
- It does not claim task-three through task-six exact tests, their implementation
  commits, remote pipeline results, or native terminal verification on Windows,
  Linux, or macOS.
- Documentation consistency and whitespace checks for this amendment are local
  documentation evidence only; they do not prove the remaining Rust behavior.

## Evidence not run

- Physical mouse selection on a real terminal was not exercised in this
  session; the automated virtual-terminal tests prove the absence of
  alternate-screen enter/leave and mouse-capture sequences, not physical
  selection.
- macOS/Linux/Windows native terminal smoke testing was not run here.
  Presentation decisions are platform-independent, but native terminal
  evidence must be reported separately from Rust test evidence.
- The full workspace test suite was not run as completion evidence; focused
  exact-target tests above are the evidence. Known unrelated failures observed
  in the shared worktree: `modes::interactive::tests::workflow_operator_answers_controls_and_saves_through_canonical_owners`
  fails at the plan commit before any implementation (pre-existing), and
  `modes::interactive::tests::shell_mode_ctrl_b_detaches_running_command` is a
  timing-dependent flake that passes in isolation.
