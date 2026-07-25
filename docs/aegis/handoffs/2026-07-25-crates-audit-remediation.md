# Handoff Prompt: Neo 2026-07-25 Crates Audit Remediation

Copy the prompt below into the implementation task unchanged.

---

You are implementing the approved Neo crates-audit remediation in
`/Users/chenyuanhao/Workspace/neo`.

## Authority

Read these files first and treat them as authoritative:

1. `AGENTS.md`
2. `docs/aegis/specs/2026-07-25-crates-audit-remediation-design.md`
3. `docs/aegis/plans/2026-07-25-crates-audit-remediation.md`
4. `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`

Run exactly one focused recall before code work:

```bash
rtk icm recall-context "2026-07-25 crates audit remediation canonical owners" --limit 5
```

The audit and design are complete. Do not repeat a full-repository audit, reopen
approved design decisions, or spend tokens rereading unrelated modules. Exact
paths and symbols already named by the plan need no new discovery. Whenever
actual code discovery or call-path understanding is still needed, obey
`AGENTS.md` and use CodeGraph first, scoped to the current named task; then use
focused `rg`/source reads only where allowed.

## Mandatory Subagent-Driven Development

Use at least three implementation subagents. The root agent remains accountable
for integration, reviews, exact verification, staging, and commits.

Dispatch only these five disjoint single-task leases initially:

- Subagent A: Task 1.
- Subagent B: Task 5.
- Subagent C: Task 10.
- Subagent D: Task 13.
- Subagent E: Task 4.

Each subagent stops and returns after that one task. Root reviews, reruns fresh
checks, and commits before sending its next follow-up. Use the dependency queues
from the plan. In particular: Task 7 must commit before Tasks 9 and 11; Task 17
must commit before Task 12; Task 12 must commit before Task 15. This prevents
shared edits to `multi_agent/runtime.rs`, `multi_agent_background.rs`, transcript
pane code, and `interactive/tests.rs`. Task 18 runs last.

Before dispatching, give each subagent:

- its exact task number(s), file lease, tests, scans, and stop conditions;
- the ban on `.references/` edits and broad exploration;
- the dirty-worktree rule: never revert/reset/restore/stash/clean another change;
- the Git rule: subagents perform no Git mutation at all, including `git add`
  and `git commit`, and do not push, merge, rebase, switch branch, or perform
  destructive Git actions; only root stages and commits;
- the fixed Delegate-card and ShellRuntime contracts.

No two active subagents may edit the same file. If a file overlap appears,
serialize those tasks. Subagents must return changed files, exact command
results, stale-scan results, and unresolved risks. Root rereads the diff and
runs fresh checks before commit.

## Approved Decisions: Do Not Re-Ask

1. Delete built-in `ANTHROPIC_OAUTH_TOKEN`. Keep only API-key auth and existing
   `x-api-key`; no Bearer guess, prefix sniff, adapter, alias, or fallback.
2. `ListDelegates` is the sole delegate/swarm discovery owner. `TaskList` lists
   metadata for bash/question/workflow only and reads no logs. `TaskOutput`
   alone hydrates output by ID.
3. Plan replay uses persisted
   `ToolExecutionFinished.result.details.{plan_content,plan_path}` only. Old
   events without details show the card/header and no body. Never read current
   disk to fabricate history.
4. Canonical owner wins. Delete old internal owner in the same task; do not
   retain compatibility branches or two implementations.

## Fixed Product Boundaries

- Do not modify Delegate, DelegateGroup, or DelegateSwarm card content, layout,
  ordering, expansion, or transcript behavior.
- Do not change Bash/Terminal admission: queued calls remain pending. Commands
  without explicit timeout/cancel remain unbounded. Clipboard's short private
  deadline applies only to clipboard helper children.
- Do not migrate/rewrite/delete session JSONL, workflow journals, goals, or user
  data. Do not add hosted services or predictive token/cost/time limits.
- Do not edit `.references/`; reference projects are comparison-only.
- Do not add unsafe Neo code. Add no new third-party package. The approved
  `neo-tui -> tracing.workspace` direct edge is allowed because tracing already
  exists in the workspace.

## Execution Protocol

1. Inspect `rtk git status --short` and record pre-existing unrelated changes.
2. Follow Tasks 1-18 in the plan. Independent path-leased streams may run in
   parallel; overlapping files are serialized.
3. For each task, implement the canonical repair and delete the old path.
4. Run only the task's exact package/target/filter tests and stale scans.
5. Run file-scoped `rtk rustfmt` and `rtk proxy git diff --check` for exact paths.
6. Root reviews the complete diff, stages exact files, and creates the specified
   conventional commit. One logical task equals one commit.
7. Store an ICM progress summary whenever required by `AGENTS.md`, especially
   after resolving an error, making a decision, completing significant work, or
   exceeding roughly 20 tool calls.
8. After all tasks, run the platform and final integration gates from the plan.
9. Check host memory before a Parallels VM, boot only one VM at a time, and stop
   it after native proof. Do not disturb a VM still running required work.
10. Do not push without explicit user authorization.

## Test Discipline

TDD route is `off/skipped`. Add one valuable post-change regression per
non-trivial behavior. Do not add tests for pure deletion, serde derives,
re-exports, trivial wrappers, or library behavior. Every test command must name
one package, one target selector (`--lib`, `--bin`, or `--test`), and a precise
filter. A local focused pass is not remote CI evidence.

## Stop And Escalate

Stop the affected task instead of inventing a workaround if it requires real
OAuth, text-based MCP auth classification, a durable schema migration, live-
disk replay fallback, a second state owner, unsafe code, dependency upgrade,
lossy paths, silent Windows degradation, Delegate-card changes, ShellRuntime
timeouts, destructive Git, or overwriting unrelated work.

When stopped, report the exact task, current source evidence, why the approved
contract cannot be met, and the smallest decision needed. Continue independent
tasks when their file/state ownership does not overlap the blocker.

## Completion Report

Return:

- task -> commit SHA -> exact verification matrix;
- stale-owner scans and their expected empty/positive results;
- macOS/Linux/Windows native evidence, clearly separated from cross-compiles;
- confirmation that `.references/`, Delegate-family cards, ShellRuntime
  semantics, and unrelated dirty files were untouched;
- VMs started/stopped;
- residual risks or blocked tasks;
- no claim that CI passed unless CI actually ran.

Begin with `rtk icm recall-context`, the four authority files, and
`rtk git status`.
Then dispatch at least three non-overlapping single-task subagents and execute
the plan with a root review/verification/commit checkpoint after every task.

---
