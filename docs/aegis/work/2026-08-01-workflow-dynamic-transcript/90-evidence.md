# Workflow Dynamic Transcript Execution - Evidence

Focused evidence is recorded below.

## EvidenceBundleDraft

- Artifact key: baseline-question-event-serialization
- Type: test
- Source: cargo test --package neo-agent-core --lib events::tests::question_requested_serializes -- --exact --nocapture
- Summary: Baseline passed: 1 test passed, 0 failed, 680 filtered out
- Verifier: root

## EvidenceBundleDraft

- Artifact key: transcript-ordering-resume-regression
- Type: test-review
- Source: nine exact Rust tests, cargo fmt --all --check, git diff --check, and retired-symbol negative search
- Summary: Optional workflow child provenance round-trips and survives compact progress persistence; old missing fields remain readable; ordinary Delegate cards commit once with headers before child tools; Workflow history remains bounded before the final assistant message; parent placeholders, workflow grouping, non-workflow card layout, recovery sequence gating, and the permission picker width regression pass.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: task1-workflow-origin-tests
- Type: test
- Source: exact cargo tests for event JSON, stamp coverage, AskUser path, core runtime, TUI transcript, and neo-agent replay
- Summary: Initial slice evidence passed for stamping nine event variants and crossing PendingQuestion. The later `transcript-ordering-resume-regression` evidence supersedes its live-only persistence boundary.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: task1-two-stage-review
- Type: review
- Source: task1_spec_review and task1_quality_review subagents
- Summary: Specification review passed after constructor fixes. Quality review found real PendingQuestion provenance loss and downstream target compile gaps; both were repaired. Final remaining issue was formatting, now confirmed clean by cargo fmt --all --check and git diff --check.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: task2-typed-workflow-activity
- Type: test-review
- Source: ten exact neo-tui tests plus task2 spec and quality review loops
- Summary: Workflow activity is grouped under one entry; origin conflicts are atomic and terminal; missing workflow and late shell events create no orphan; newer delegate and swarm runs supersede old terminal snapshots; finalized direct tools reject late regressions; non-terminal workflow transitions are absent from history. Ten fresh exact tests, cargo fmt --all --check, git diff --check, and retired-symbol negative search passed.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: task3-task7-unified-verification
- Type: test-review
- Source: exact neo-tui workflow, terminal-frame, terminal-scrollback, approval-width, neo-agent replay tests; cargo check -p neo-tui -p neo-agent --tests; cargo fmt --all --check; git diff --check
- Summary: The workflow main card, two sibling child summaries, non-workflow compatibility, single terminal history commit, earliest blocking input owner, bottom-region preservation, frame bounds, and permission-width regression all passed fresh focused checks.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: unified-review-repairs
- Type: review
- Source: unified reviewer plus main-agent repair and rerun
- Summary: Unified review found main-card ordering drift and unstable swarm child ordering. Context now precedes controls, swarm children sort before priority selection, reversed input order is covered, and the affected exact tests plus cargo check and formatting checks pass.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: final-review-regressions
- Type: focused-test
- Source: exact neo-tui tests for parent placeholder absorption, Delegate fact retention and ordering, zero-height Workflow commit, and JSONL replay grouping
- Summary: Progress updates suppress only an absorbed parent placeholder; trimmed child commands survive until one parent-owned terminal block; later complete payloads replace partial facts without changing tool order; Group and Swarm child rows follow parent order after reversed completion; zero history budget neither acknowledges nor lets later assistant history pass a terminal Workflow; JSONL replay keeps Bash, question, Delegate, and DelegateSwarm source grouping.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: workspace-structure-check
- Type: structural-check
- Source: aegis-workspace.py bundle and check
- Summary: The current work proof bundle assembled. The repository-wide check remains blocked by pre-existing missing INDEX targets and legacy ADR format findings unrelated to this implementation; no unrelated workspace records were changed.
- Verifier: root

## EvidenceBundleDraft

- Artifact key: final-stale-progress-review-repair
- Type: review-test
- Source: second unified review; six exact Rust tests; cargo fmt --all --check; git diff --check; retired-symbol negative search
- Summary: Older progress is rejected at the shared merge owner before it can clear a newer terminal outcome. The direct regression, workflow-origin event round-trip, JSONL replay grouping, Swarm terminal order, zero-budget Workflow barrier, and permission picker width regression all pass.
- Verifier: root
