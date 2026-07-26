# Neo Workflow Platform Implementation Plan

> Executor note: this is the implementation plan for the approved design in
> `docs/aegis/specs/2026-07-25-workflow-platform-design.md`. It assumes the
> executor has zero Neo context and must be followed task by task. Do not edit
> product code until the task's file lease, owner boundary, and verification
> command are understood. Do not reopen the approved Lua-only decision.

## Goal

Implement the approved P0-P2 expansion of Neo's local workflow platform. The
finished platform must provide durable V2 runs, torn-tail recovery, paged
output, global machine admission, registry-backed definitions, exact named
launch, schema-validated results and child repair, durable human input,
heterogeneous large swarms, canonical generic tools, artifacts, linked runs,
CLI/TUI operation, deterministic validation/harnesses, three public built-ins,
and bilingual documentation.

The implementation is complete only when every P0, P1, and P2 item in sections
45-47 of the approved spec has a task, focused verification evidence, and a
retirement scan. This plan does not claim that product behavior exists today.

## Architecture

```text
definition files or dynamic RunWorkflow
  -> WorkflowDefinitionRegistry / source adapter
  -> ResolvedWorkflowDefinition (pinned revision)
  -> WorkflowLaunchIntent
  -> WorkflowLaunchCoordinator (stateless normalization only)
  -> exact capability and approval match
  -> WorkflowRuntime (only durable owner)
  -> global admission -> Lua host
       -> canonical ToolRegistry / MultiAgentRuntime / ShellRuntime
       -> journaled effects, schemas, artifacts, user input, provenance
  -> TaskOutput paging + BackgroundTaskManager projection + /tasks TUI
```

`WorkflowRuntime` owns run state, journal, replay, recovery, lineage, terminal
result, and durable controls. `WorkflowDefinitionRegistry` owns only discovery,
resolution, validation, revision, and save. `WorkflowLaunchCoordinator` owns
only stateless normalization and sequencing of existing owners. Session JSONL,
`BackgroundTaskManager`, task handles, transcript cards, and TUI state remain
projections/adapters. Shell and child lifecycles remain in their existing
canonical owners.

Lua is the sole workflow engine. No Rhai, dual-engine mode, engine trait,
workflow-local registry, model-supplied budget, predictive governance, or
arbitrary total-child cap may be introduced.

## Tech Stack

- Rust 2024, MSRV 1.96.1, `unsafe_code = "forbid"`.
- Existing `mlua` Lua 5.4 host with async/serialize features.
- Existing Tokio, serde/serde_json, TOML, SHA-256, tempfile, session atomic
  file helpers, canonical permission/instruction/tool/child/worktree owners.
- Existing workspace `jsonschema = 0.37.4`; add it to `neo-agent-core` only if
  the current dependency declaration is absent.
- `cargo nextest` or exact `cargo test`, `rustfmt --check --edition 2024`,
  `git diff --check`, `rtk`/`cx` where available.
- Native macOS proof in this workspace plus one-at-a-time Parallels Linux and
  Windows probes when a task requires platform behavior. Native evidence is
  separate from local cross-compilation and remote CI.

## Baseline / Authority Refs

Required before implementation:

1. `AGENTS.md`, `RTK.md`, and `CX.md`.
2. `docs/aegis/specs/2026-07-25-workflow-platform-design.md` (approved
   requirement authority; commit `ecaff36e`).
3. `docs/aegis/adr/ADR-0004-durable-runworkflow-runtime.md`.
4. `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`.
5. Current source under the exact file leases in each task.
6. `.tmp/grok_vs_neo_workflow_report.md` only as historical comparison input;
   it is not a requirement or authority.

Baseline usage:

- Required baseline refs: the four files above plus the current source seams.
- Acknowledged before planning: ADR-0004 and the 2026-07-23 baseline.
- Cited in this plan: all four authority documents and task-local source/test
  paths.
- Missing authority: none.
- Decision: ready.

## Requirement Ready Check

- Requirement sources: user approval, approved spec, ADR-0004, runtime
  baseline, corrected comparison report, live Neo source.
- Goals and scope: spec sections 1-5 and 45-47.
- Acceptance: spec section 48 and exact verification commands below.
- Open blocker questions: none. Lua-only, no arbitrary total-child cap, and
  canonical ownership are closed decisions.
- Decision: ready.

## Compatibility Boundary

- V1 run files and journals are readable projections only. V1 has no writer and
  no same-ID resume. Explicit linked V2 upgrade imports a verified prefix into
  a new run; it never rewrites or deletes V1 bytes.
- Existing `/workflow` bare dynamic authoring remains. Named
  `/workflow <name> [JSON_OBJECT]` is an additive host-direct path with zero
  model calls before execution. Both adapters use one coordinator and one
  authorization contract.
- Existing Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal card
  layout, fields, ordering, expansion, progress, and output previews remain
  unchanged. Workflow provenance is surrounding metadata only.
- Shell admission remains pending. Commands without explicit timeout/cancel
  remain unbounded. No workflow-level wall-clock timeout is added.
- Existing ordinary tools remain governed by their existing schema,
  instruction, permission, containment, and MCP rules. `neo.tool` adds a
  centralized semantic deny set; ordinary new tools are eligible by default.
- Existing session JSONL remains a projection. It cannot become a second
  workflow state or result owner.
- No hosted service, network workflow store, marketplace, automatic merge,
  predictive token/cost/time/agent governance, or hidden model repair exists.

## Change Necessity

- User-visible need: the current runtime lacks the durable recovery, definition,
  schema, scale, input, output, operator, and authoring capabilities required
  by the approved spec.
- Non-code option: documentation or config cannot make torn journals recover,
  persist final results, dispatch heterogeneous children, or provide CLI/TUI
  controls.
- Minimum boundary: existing workflow/runtime/tool/CLI/TUI owners plus the
  bounded modules explicitly named below.
- Decision: code-change, followed by synchronized bilingual docs.

## Existence Check

- Registry: no existing definition discovery owner; create one bounded registry
  and reuse existing trust and atomic-file helpers.
- Coordinator: current `tools/workflow.rs` owns launch side effects; move that
  sequencing into one stateless coordinator and delete the adapter transaction.
- Admission/artifact/lineage/output: no existing workflow-specific owner;
  create bounded modules because each has an independent durable contract.
- Isolated worktree: live source search found no reusable cross-platform
  `WorktreeManager`; add one bounded manager only if Task 17's isolation
  acceptance cannot be met by an existing canonical owner discovered before
  editing. It must own path/process/cleanup metadata, not workflow state.
- Validator/harness: no workflow definition checker or deterministic workflow
  fixture owner exists; add one public adapter over the real Lua/journal code.
- Built-ins: use ordinary registry definitions and public APIs; no privileged
  host functions.
- Decision: add-with-proof for the bounded owners above; reuse existing
  ToolRegistry, MultiAgentRuntime, ShellRuntime, trust, worktree, task browser,
  and atomic-file owners.

## Architecture Integrity Lens

- Invariant: one durable owner, one journal, one result, one child lifecycle
  owner, one tool registry, one trust decision.
- Canonical owner: `WorkflowRuntime`; all other workflow surfaces are
  projections, adapters, or stateless source/launch normalization.
- Overlap to remove: direct run creation/rollback/start in `tools/workflow.rs`,
  full-journal reads in TaskOutput, workflow-local tool/child dispatch, any
  V1 writer, and any second registry/hash/schema implementation.
- Higher-level simplification: extend the existing multi-agent runtime with
  canonical `ChildPlan` and extend `/tasks`; do not create a workflow-specific
  scheduler or dashboard.
- Retirement falsifier: an active scan finds a second writer, duplicate hash,
  direct effect retry, full-journal ToolResult, or stale arbitrary child cap.
- Verdict: proceed with owner collapse and bounded new modules.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable; the user did not request strict test-first
  TDD.
- Test posture: use diagnostic fixtures where the current behavior is absent,
  then implement the smallest owner change and add focused post-change
  regression tests. No synthetic RED/GREEN ceremony is prescribed.
- Verification: every command names one package, one target selector, and one
  precise filter. Broad workspace tests are not completion evidence.

## Verification

- Task proof: run only the exact package/target/filter commands in the active
  task, then file-scoped rustfmt and path-scoped diff checks.
- Integration proof: run the dependency-boundary tests named in the final
  matrix after each batch; do not replace them with a broad cargo test.
- Retirement proof: scan for old writers, aliases, count constants, full-
  journal output, predictive limits, fuzzy JSON, and duplicate owners.
- Platform proof: use native macOS, Linux, and Windows tests for path/link/
  replace/sync behavior; cross-compile success is not native behavioral proof.
- Documentation proof: build VitePress and check English/Chinese parity after
  the implementation exists.
- Claim boundary: report local, native, docs, and remote CI evidence separately;
  unexecuted CI and provider-backed live runs remain uncovered risk.

## Plan-Time Complexity Check

- Existing pressure: `runtime.rs` (1569 lines), `background_tasks.rs` (2988),
  and `workflow_dispatch.rs` (1017) already carry multiple responsibilities.
- Recommendation: do not grow those files with new durable features. Extract
  only `recovery`, `admission`, `artifacts`, `lineage`, `output`, `retention`,
  `definition`, `registry`, and `launch` when the task removes real pressure.
- `tools/workflow.rs` remains a thin dynamic adapter after migration; CLI
  business logic belongs in `modes/workflow.rs`, not `main.rs`.
- Budget result: within-budget only if every new module has one owner and old
  responsibilities are deleted from the large files in the same task.

## Execution Readiness View

- Intent Lock: implement the approved P0-P2 workflow platform, not a new
  product direction or comparison-report translation.
- Scope Fence: exact files in the task leases; `.references/`, user data, and
  unrelated dirty files are immutable.
- Baseline Lock: ADR-0004 and the 2026-07-23 runtime baseline describe current
  behavior until implementation evidence and a later baseline sync exist.
- Approved Behavior: Lua-only, durable V2, no arbitrary total-child cap,
  actual usage only, exact named launch, strict schemas, durable user input,
  immutable artifacts, explicit linked runs, paged output.
- Owner/Contract Constraints: `WorkflowRuntime` durable owner; registry
  discovery owner; coordinator stateless; canonical ToolRegistry,
  MultiAgentRuntime, ShellRuntime, permission and instruction owners stay in
  charge.
- Compatibility Boundary: V1 read-only linked upgrade; existing cards and
  shell semantics unchanged; bare `/workflow` retained.
- Retirement Boundary: delete internal duplicate writers, aliases, direct
  dispatch, old count constants, full-journal output, and predictive limits;
  never delete persistent V1 data.
- Task Batches: substrate, recovery/admission, definition/launch, Lua/schema,
  child/tool/input/artifact/lineage, output/CLI/TUI, validator/built-ins/docs.
- Test Obligations: each task's exact command, stale scan, formatting and
  diff-check; native checks for path/link/sync semantics.
- Review Gates: root reviews every subagent diff; one conventional commit per
  task; no task starts with overlapping file leases.
- Drift/Rewind Rules: stop and return to this plan on persistent-state ambiguity,
  effect retry ambiguity, a second owner, unsafe code, dependency upgrade, or
  unrelated dirty-file conflict. Do not revert another change.
- Evidence Required: focused local test output, stale-owner scans, staged diff
  check, native evidence where required, and explicit residual risk. Never call
  local proof remote CI success.
- Advisory boundary: this view is execution guidance only; it is not a runtime
  gate, policy snapshot, or completion authority.

## Task Batches And File Leases

The root agent owns integration, review, staging, and commits. Subagents may
read and edit only their lease and perform no Git mutation. Overlapping leases
are serialized in the order shown.

### Batch A: durable substrate

#### Task 1: Pin V1 fixtures and typed V2 identity/error contracts

Files: `crates/neo-agent-core/src/workflow/state.rs`, `error.rs`, `mod.rs`,
new `crates/neo-agent-core/tests/fixtures/workflow_v1/`, new
`crates/neo-agent-core/tests/workflow_v1_compat.rs`, new
`crates/neo-agent-core/tests/workflow_runtime_v2.rs`, and
`crates/neo-agent-core/tests/workflow_runtime.rs` only for mechanical type
updates.

Why: every later task needs stable run IDs, handles, revisions, request IDs,
artifact IDs, checkpoints, V2 states, and stable error categories.

Steps:

1. Capture byte-for-byte completed and incomplete V1 fixtures from deterministic
   test construction only; never copy a user's session or rewrite existing
   data. Verify current V1 decode before adding V2 fields.
2. Add typed wrappers for `WorkflowRunId`, `WorkflowHandle`, `WorkflowName`,
   `WorkflowRevision`, `WorkflowInvocationId`, `WorkflowRequestId`,
   `WorkflowArtifactId`, and `WorkflowCheckpoint`. Enforce the portable name
   grammar and keep UUID as the machine key.
3. Add V2 `Queued` and `AwaitingUser` state semantics, pinned source metadata,
   lineage metadata, final-result metadata, and stable error codes. Preserve
   read-only V1 decoding without a V1 writer.
4. Export only canonical types from `workflow/mod.rs`; do not add a generic
   engine or state trait.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_v1_compat v1_fixtures_decode_current_format
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 workflow_v2_identity_rejects_invalid_names
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/state.rs crates/neo-agent-core/src/workflow/error.rs crates/neo-agent-core/src/workflow/mod.rs crates/neo-agent-core/tests/workflow_v1_compat.rs
git diff --check -- crates/neo-agent-core/src/workflow/state.rs crates/neo-agent-core/src/workflow/error.rs crates/neo-agent-core/src/workflow/mod.rs crates/neo-agent-core/tests/workflow_v1_compat.rs
```

Stop if fixture creation would mutate persistent user data or if V1 would need
an in-place writer. Commit: `feat(workflow): define v2 identities and states`.

#### Task 2: Version the journal and add streaming validation

Files: `crates/neo-agent-core/src/workflow/journal.rs`, new
`workflow/journal_scan.rs`, `tests/workflow_journal_v2.rs`, and direct journal
call sites in `runtime.rs` required to compile.

Steps:

1. Introduce a versioned envelope carrying version, sequence, timestamp, run
   ID, payload, canonical input hash, and payload references. Preserve the
   append-only byte contract and fail closed on unknown versions/kinds.
2. Add all spec record families: run creation, state/control, invocation,
   child/swarm item, schema repair, user input, artifact, lineage, final
   result, recovery, usage, and provenance. Large payloads use hash-addressed
   references; usage, terminal reason, and child references remain inline.
3. Replace synchronous `read_journal -> Vec` paths with a bounded scanner/index
   that can produce replay state and pages without retaining the full journal.
4. Validate contiguous sequence, run ID, canonical hash, start/finish pairing,
   terminal-child invariant, and final-result ordering.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_v2_round_trips_versioned_envelope
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_scan_rejects_sequence_hash_and_run_mismatch
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_v2_record_families_preserve_terminal_metadata
rustfmt --check --edition 2024 crates/neo-agent-core/src/workflow/journal.rs crates/neo-agent-core/src/workflow/journal_scan.rs crates/neo-agent-core/tests/workflow_journal_v2.rs
```

Stop if a second persistence database or lossy payload fallback is proposed.
Commit: `feat(workflow): add versioned journal scanning`.

#### Task 3: Recover torn tails and preserve V1 read-only behavior

Files: new `workflow/recovery.rs`, `journal.rs`, `runtime.rs` rehydrate seam,
`tests/workflow_journal_v2.rs`, and `tests/workflow_v1_compat.rs`.

Steps:

1. Scan only the final EOF suffix for valid unterminated JSON, invalid partial
   JSON, missing newline, or empty tail. Normalize a valid final record safely.
2. On invalid EOF suffix, write a hash-addressed quarantine before truncating;
   sync quarantine and directory where supported. If quarantine fails, leave
   the original journal byte-for-byte unchanged.
3. Treat newline-terminated invalid JSON, interior malformed JSON, sequence
   mismatch, run-ID mismatch, and hash mismatch as fail-closed corruption.
4. Record a typed recovery action and reconcile started effects as interrupted
   or adopted only by the production resolver. Never relaunch an uncertain
   effect.
5. Rehydrate V1 fixtures as read-only projections. Same-ID resume returns
   `linked_upgrade_required`; explicit linked V2 import is the only writer.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_recovery_normalizes_valid_unterminated_record
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_recovery_quarantines_torn_tail_before_truncate
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_recovery_fails_closed_on_interior_or_newline_corruption
cargo nextest run -p neo-agent-core --test workflow_v1_compat v1_nonterminal_resume_requires_linked_upgrade_without_append
```

Stop on any persistent-data deletion or automatic external-effect retry.
Commit: `fix(workflow): recover torn journal tails safely`.

#### Task 4: Make runtime creation, transitions, and effects durable

Files: `workflow/runtime.rs`, `workflow/state.rs`, `workflow/runtime_support.rs`,
new `workflow/effect.rs`, and `tests/workflow_runtime_v2.rs`.

Steps:

1. Change V2 creation to atomically persist immutable `run.json`, append
   `RunCreated`, and enter `Queued` before task registration or worker start.
   Registration failure rolls back only a never-started V2 directory.
2. Replace ad-hoc transition guards with an explicit transition table. Terminal
   states are immutable; `AwaitingUser` cannot be bypassed by ordinary resume.
3. Implement reserve -> synced `InvocationStarted` -> external effect -> synced
   `InvocationFinished` -> terminal transition. Keep locks out of blocking I/O
   and external awaits.
4. Persist `FinalResultRecorded` before `Completed`; on recovery append only the
   missing terminal state. `Completed` without a valid final result fails closed.
5. Keep `WorkflowRuntime` as the only lifecycle, journal, replay, recovery, and
   aggregate-result owner.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 v2_create_is_durable_and_queued_before_registration
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 workflow_v2_rejects_all_illegal_and_terminal_transitions
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 external_effect_is_never_executed_before_durable_start
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 crash_after_final_result_appends_only_completed_state
```

Stop if runtime mutexes cross file I/O or if another owner begins writing run
state. Commit: `refactor(workflow): make v2 lifecycle transactional`.

### Batch B: supervision, admission, storage, and result transport

#### Task 5: Bind production recovery and supervise workers

Files: `runtime/workflow_dispatch.rs`, new
`runtime/workflow_recovery_dispatch.rs`, `workflow/runtime.rs`, and
`tests/workflow_recovery_dispatch.rs` plus focused runtime tests.

Steps: bind the production read-only resolver at the composition root; query
canonical terminal child/task stores; adopt exactly one proven result; classify
zero/conflicting/unknown results as interruption; clear worker/current
invocation markers and permits on panic, cancellation, failed start, and
terminalization failure; emit only an unsequenced recovery-failure projection
when journal terminalization is impossible.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_recovery_dispatch resolver_is_read_only_and_never_dispatches
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 worker_panic_clears_active_state_and_releases_resources
cargo nextest run -p neo-agent-core --test workflow_runtime_v2 rehydrate_starts_no_worker_and_preserves_awaiting_user
```

Stop on resolver writes, speculative adoption, or automatic retry. Commit:
`fix(workflow): bind recovery resolver and supervise workers`.

#### Task 6: Add global admission and retention preview

Files: new `workflow/admission.rs`, `workflow/retention.rs`, `workflow/limits.rs`,
`runtime/config.rs`, `neo-agent/src/config/types.rs`, `config/loader.rs`,
`config/mod.rs`, and `tests/workflow_admission.rs`.

Steps: define host-owned VM/worker/executor, journal/artifact/global-storage,
queue and page limits; map them from `[runtime.workflow]`; delete predictive
token/agent/time governance and `token_cap`; implement fair queueing and
actual permit release on every exit path; keep queued runs/items pending; add
read-only retention mark/sweep preview that excludes live, referenced, pinned,
and nonterminal data.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_admission unavailable_permit_keeps_run_durably_queued_fifo
cargo nextest run -p neo-agent-core --test workflow_admission workflow_admission_releases_every_runtime_exit_path
cargo nextest run -p neo-agent-core --test workflow_admission workflow_storage_reservations_are_race_safe
cargo test --package neo-agent --bin neo modes::config::tests::workflow_machine_limits_map_all_v2_fields --exact --nocapture --include-ignored
```

Stop if any model/tool field can raise a host limit or if admission rejects
large work solely by child count. Commit: `feat(workflow): add global resource admission`.

#### Task 7: Add immutable artifacts and canonical final result

Files: new `workflow/artifacts.rs`, `workflow/output.rs` result types,
`workflow/runtime.rs`, `workflow/journal.rs`, `tests/workflow_artifacts.rs`.

Steps: serialize text/JSON canonically, enforce limits, write temp/sync/hash/
rename/dir-sync, append `ArtifactCommitted` last, revalidate reads, expose
bounded metadata/ranges, and persist exactly one top-level Lua return value.
Oversized final values/reports become content-addressed artifact references;
actual usage, child refs, and terminal reason stay observable.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_artifacts artifact_is_visible_only_after_durable_commit
cargo nextest run -p neo-agent-core --test workflow_artifacts oversized_final_result_uses_artifact_without_losing_usage
cargo nextest run -p neo-agent-core --test workflow_artifacts corrupt_or_missing_artifact_is_typed_error
```

Stop if artifacts accept arbitrary filesystem paths or if a final result can be
silently synthesized from reports. Commit: `feat(workflow): persist artifacts and final results`.

#### Task 8: Add linked checkpoints and V2 upgrade

Files: new `workflow/lineage.rs`, `workflow/runtime.rs`, CLI/core control
adapters later extended by Task 18, and `tests/workflow_lineage.rs`.

Steps: define checkpoint sequence/digest; import a verified completed prefix
into `LineageSeedImported` records; copy/reference artifacts by verified hash;
separate inherited from new actual usage; fail before any effect on mismatch;
require fresh authorization; keep terminal parents immutable.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_lineage mismatch_stops_before_new_effect
cargo nextest run -p neo-agent-core --test workflow_lineage linked_upgrade_imports_verified_prefix_and_artifacts
cargo nextest run -p neo-agent-core --test workflow_lineage terminal_parent_never_changes_state
```

Stop if lineage reads mutable parent files during execution or imports an
incomplete effect. Commit: `feat(workflow): add verified linked runs`.

### Batch C: definitions, launch, and Lua contract

#### Task 9: Add typed definitions and canonical revision hashing

Files: new `workflow/definition.rs`, `workflow/schema.rs`, `workflow/mod.rs`,
`neo-agent-core/Cargo.toml`, and `tests/workflow_registry.rs`.

Steps: decode paired `.lua`/`.workflow.toml`; require final `output_schema`;
compile input/output schemas; canonicalize manifest JSON with UTF-8 byte-sorted
keys; calculate the exact `neo-workflow-definition-v2\0` + big-endian length
framing; reject name/source/schema/manifest limits before large allocation;
produce one `ResolvedWorkflowDefinition` for file, builtin, and dynamic input.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_registry definition_revision_golden_vectors_are_stable
cargo nextest run -p neo-agent-core --test workflow_registry definition_revision_preserves_object_order_rules_and_field_boundaries
cargo nextest run -p neo-agent-core --test workflow_registry dynamic_definition_requires_final_output_schema
```

Stop if a second schema validator, fuzzy parser, or path/mtime input to the
revision appears. Commit: `feat(workflow): add typed definition revisions`.

#### Task 10: Implement the trusted definition registry

Files: new `workflow/registry.rs`, `config/mod.rs`, `config/loader.rs`,
`tests/workflow_registry.rs`, and bilingual data-location docs only if needed
for path examples.

Steps: discover exactly builtin < user < trusted-project; reject same-scope
duplicates; do not fall back on invalid higher-scope content; reuse existing
trust decision; reject symlink/reparse escapes, parent escapes, non-regular or
wrong-suffix files; cache only as a rebuildable projection; implement atomic
no-clobber save with manifest hash last; pin a resolved source snapshot for
each run.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_registry registry_precedence_conflict_and_no_fallback
cargo nextest run -p neo-agent-core --test workflow_registry untrusted_project_definitions_are_absent_and_unsaveable
cargo nextest run -p neo-agent-core --test workflow_registry registry_rejects_symlink_reparse_and_path_escape
cargo nextest run -p neo-agent-core --test workflow_registry save_is_no_clobber_and_pair_atomic
```

Stop if the registry creates a second trust store, follows links, executes Lua
during scan, or stores a hidden revision database. Commit:
`feat(workflow): add trusted definition registry`.

#### Task 11: Centralize launch normalization and authorization binding

Files: new `workflow/launch.rs`, `tools/workflow.rs`, `workflow/capability.rs`,
`tests/workflow_launch.rs`.

Steps: define immutable `WorkflowLaunchIntent` containing session/workspace,
nonce, source, revision, exact args/schema/lineage hashes, actor, and parent
lineage; make `WorkflowLaunchCoordinator` stateless; route create -> durable
authorization consume -> task registration -> admission/start; bind capability
to exact source/revision/args/lineage; do not consume reusable authorization on
validation/compile/schema/storage failure; delete adapter-owned rollback and
direct start transaction.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_launch all_launch_adapters_reach_one_coordinator
cargo nextest run -p neo-agent-core --test workflow_launch exact_intent_hash_mismatch_creates_no_run
cargo nextest run -p neo-agent-core --test workflow_launch compile_schema_and_storage_failure_preserve_reusable_capability
```

Stop if the coordinator writes journal/run files, owns capability state, or
adds aliases. Commit: `refactor(workflow): centralize launch coordination`.

#### Task 12: Extend Lua host with schemas, strict JSON, and final return

Files: `workflow/lua.rs`, `workflow/schema.rs`, `workflow/runtime.rs`,
`tests/workflow_lua.rs`, new `tests/workflow_schema.rs`.

Steps: keep Lua sandbox and canonical APIs; change the runner result from
`Option<mlua::Value>` to one canonical `serde_json::Value` (`nil` becomes JSON
`null`); reject multiple Lua returns; traverse tables deterministically with
explicit immutable `neo.json_array` and `neo.json_object` markers so empty
containers are unambiguous; reject sparse/mixed tables, cycles, non-finite
numbers, excessive depth, and excessive bytes. Add `neo.await_user`,
`neo.tool`, `neo.artifact_put/get/list`, heterogeneous `neo.swarm`, per-child
policy and output schemas; implement exact one-value Lua-to-JSON conversion;
reject prose/fences/multiple values/fuzzy extraction; validate final result
without hidden model repair; persist the return instead of discarding it in the
production binder; journal invocation/usage/provenance for each host call.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_schema exact_json_succeeds_and_prose_or_fences_fail
cargo nextest run -p neo-agent-core --test workflow_schema invalid_final_lua_result_fails_without_hidden_model_repair
cargo nextest run -p neo-agent-core --test workflow_lua lua_return_conversion_preserves_empty_array_and_object_markers
cargo nextest run -p neo-agent-core --test workflow_lua lua_return_conversion_rejects_sparse_mixed_cyclic_and_non_finite_values
cargo nextest run -p neo-agent-core --test workflow_lua workflow_host_denies_model_supplied_limits
```

Stop if Lua receives filesystem/system APIs, schema parsing scans prose, or
engine abstraction/Rhai is introduced. Commit: `feat(workflow): enforce strict lua result contracts`.

#### Task 12A: Add provider-native structured-output hints

Files: `crates/neo-ai/src/options.rs`, `crates/neo-ai/src/types.rs`, supported
provider wire clients under `crates/neo-ai/src/providers/`, their focused test
targets, and the child-call composition seam in `neo-agent-core`.

Steps: add one provider-neutral optional response-format value carrying schema,
name, and strictness; map it only in providers that can express the same
contract; providers without support omit the wire hint and return ordinary
assistant output; never infer support from URL or model-name strings; keep the
host schema validator authoritative in both paths; keep `neo-ai` independent
of `neo-agent-core` types.

Verification:

```bash
cargo nextest run -p neo-ai --lib response_format_schema_is_provider_neutral
cargo nextest run -p neo-ai --lib provider_native_structured_output_is_optional_and_host_validated
cargo nextest run -p neo-agent-core --test workflow_schema provider_native_and_text_fallback_share_host_validation
```

Stop if provider wire acceptance bypasses host validation or if provider
capability is guessed from endpoint/model text. Commit:
`feat(ai): support JSON Schema response formats`.

### Batch D: child outputs, user input, tools, swarm, lineage provenance

#### Task 13: Implement exactly one child schema repair continuation

Files: `tools/delegate.rs`, canonical child runtime files discovered through
CodeGraph, `workflow/runtime.rs`, `workflow/journal.rs`, and
`tests/workflow_schema.rs`.

Steps: require `output_schema` for new/resumed child; validate provider-native
or final assistant value; append repair-start before the same-session
tools-disabled corrective call; reject repair tool calls; append repair-finish;
aggregate both actual usages; preserve raw output as bounded inline data or
artifact reference; never repeat original child effects and never repeat repair
after crash.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_schema child_schema_invalid_output_gets_exactly_one_tools_disabled_repair
cargo nextest run -p neo-agent-core --test workflow_schema schema_repair_tool_attempt_is_forbidden
cargo nextest run -p neo-agent-core --test workflow_schema crash_during_repair_never_repeats_model_effect
```

Stop if repair becomes fuzzy extraction, a second continuation, or an external
effect retry. Commit: `feat(workflow): add bounded child schema repair`.

#### Task 14: Add durable AwaitingUser and answer control

Files: `workflow/lua.rs`, `workflow/runtime.rs`, `workflow/journal.rs`, new
`workflow/user_input.rs` only if it removes real pressure, control adapters in
`tools/background_tasks.rs`, and `tests/workflow_user_input.rs`.

Steps: compile schema/default before effect; append and sync request; transition
to AwaitingUser; release active VM/worker permit; rehydrate prompt/schema/
default/policy; validate answer through one runtime method; append answer then
queue; reject stale/duplicate/wrong-run/wrong-schema and human-only model
answers; stop awaiting runs without losing request history.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_user_input await_user_releases_permits_and_survives_restart
cargo nextest run -p neo-agent-core --test workflow_user_input answer_validates_request_schema_before_queueing
cargo nextest run -p neo-agent-core --test workflow_user_input task_resume_cannot_bypass_missing_answer
```

Stop if UI becomes answer owner or if the API advertises secret/password
semantics. Commit: `feat(workflow): add durable user input`.

#### Task 15: Lower heterogeneous swarm specs into canonical ChildPlan

Files: `tools/delegate.rs`, multi-agent runtime owner located by CodeGraph,
`workflow/lua.rs`, `workflow/runtime.rs`, and `tests/workflow_swarm.rs`.

Steps: define/reuse canonical `ChildPlan`; lower existing `DelegateSwarm` and
workflow `neo.swarm` into it; delete `MAX_SWARM_CHILDREN = 8`; validate bytes,
schema, VM, journal, and actual storage instead of arbitrary total count;
persist per-item queued/started/finished records; preserve input result order;
pause before new starts, stop active children canonically, and never replay a
completed item.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_swarm heterogeneous_child_specs_reach_one_childplan_owner
cargo nextest run -p neo-agent-core --test workflow_swarm swarm_arrays_larger_than_eight_validate_within_resource_limits
cargo nextest run -p neo-agent-core --test workflow_swarm completed_items_are_not_replayed_after_sibling_failure
cargo nextest run -p neo-agent-core --test multi_agent_runtime delegate_swarm_golden_card_contract_is_unchanged
```

Stop if a workflow-specific child scheduler, arbitrary count cap, or card
rewrite is proposed. Commit: `feat(workflow): support heterogeneous swarms`.

#### Task 16: Add generic `neo.tool`, child ceilings, and provenance

Files: `workflow/lua.rs`, `workflow/runtime.rs`, `tools/mod.rs`/canonical
registry owner, approval/event envelopes, child runtime owner, and
`tests/workflow_tool_policy.rs`.

Steps: decode exact registered name/input; use canonical ToolRegistry; add one
semantic deny classifier for workflow/dialog/goal/plan/task-control/child
control tools; keep ordinary tools open by default; preserve instruction and
permission outcomes; intersect child `tool_allow` with parent authority;
record typed workflow origin on approvals/events/task projections; reject
same-run recursive TaskOutput lock paths; preserve shell pending/unbounded
semantics.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_tool_policy ordinary_registered_tools_are_workflow_eligible_by_default
cargo nextest run -p neo-agent-core --test workflow_tool_policy workflow_dialog_goal_plan_and_child_tools_are_denied
cargo nextest run -p neo-agent-core --test workflow_tool_policy child_tool_allow_only_reduces_parent_capability
cargo nextest run -p neo-agent-core --test workflow_tool_policy workflow_provenance_is_typed_on_approval_and_events
```

Stop if the workflow copies a tool registry, fuzzy-matches deny names, or
changes ShellRuntime timeout/admission behavior. Commit:
`feat(workflow): dispatch canonical tools with provenance`.

#### Task 17: Complete verified lineage execution and isolation policy

Files: `workflow/lineage.rs`, new `crates/neo-agent-core/src/worktree.rs` only
if the pre-edit existence check confirms no canonical manager, child
context/worktree owner, `workflow/lua.rs`, `tests/workflow_lineage.rs`, and
existing worktree tests only as needed.

Steps: implement inherit/summary/none context via current context owners;
resolve model/provider aliases canonically; ensure child permission cannot
escalate. For `isolated`, use the pre-existing canonical worktree manager when
one is found; otherwise add the smallest cross-platform `WorktreeManager`
owner that uses typed `PathBuf`/process arguments, records creation/cleanup
state, refuses unsupported repositories, and never deletes dirty worktrees.
Do not execute ad-hoc shell strings. Shared and isolated paths must be
recorded in child provenance; no auto-merge or automatic cleanup is allowed.
Fail before child start when isolation is unsupported.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_lineage child_context_and_capability_ceiling_are_explicit
cargo nextest run -p neo-agent-core --test workflow_lineage unsupported_isolated_worktree_fails_before_child_start
cargo nextest run -p neo-agent-core --test workflow_lineage isolated_worktree_paths_are_portable_and_cleanup_is_explicit
```

Stop on ad-hoc shell worktree creation, hidden prompt ownership, or authority
elevation. Commit: `feat(workflow): enforce child isolation and ceilings`.

### Batch E: output, UI, CLI, validation, built-ins, docs

#### Task 18: Implement bounded TaskOutput views and adapter

Files: new `workflow/output.rs` if not completed in Task 7,
`tools/background_tasks.rs`, new `tools/workflow_task_output.rs`, workflow
events, `tests/workflow_output.rs`, and `neo-agent` adapter tests.

Steps: support summary/journal/result/artifacts/artifact_content views; bind
cursor to run/view/query; cap complete ToolResult; return accurate sequence,
bytes, has-more, and next cursor; perform reads outside runtime locks; keep
BackgroundTaskManager as a projection/control adapter only.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_output multi_gigabyte_logical_journal_pages_under_tool_result_cap
cargo nextest run -p neo-agent-core --test workflow_output wrong_run_view_or_query_cursor_is_rejected
cargo nextest run -p neo-agent-core --test workflow_output slow_output_io_does_not_block_snapshot_or_pause
```

Stop if any summary serializes the complete journal or artifact. Commit:
`feat(workflow): add paged workflow output`.

#### Task 19: Add headless workflow command family

Files: `crates/neo-agent/src/cli.rs`, new `modes/workflow.rs`, `modes/mod.rs`,
`main.rs`, new `crates/neo-agent/tests/workflow_cli.rs`, and existing
`crates/neo-agent/tests/cli_commands.rs` only for mechanical command dispatch
coverage.

Steps: add `list/show/check/test/run/save/answer/fork/prune`; route business
logic to core registry/runtime; enforce args-json/args-file mutual exclusion;
make run wait by default and detach after durable creation; make output
text/json/jsonl stable; make save no-clobber and project-trust-gated; make
prune dry-run by default, `--yes` required, terminal-unreferenced only.

Verification:

```bash
cargo nextest run -p neo-agent --test cli_commands workflow_command_parsing_and_arg_source_conflicts
cargo nextest run -p neo-agent --test cli_commands workflow_headless_json_and_jsonl_outputs_are_stable
cargo nextest run -p neo-agent --test cli_commands workflow_save_is_no_clobber_and_prune_defaults_to_dry_run
```

Stop if CLI creates a second runtime/registry state. Commit:
`feat(cli): add workflow command family`.

#### Task 20: Add named slash launch and preserve bare semantics

Files: `crates/neo-agent/src/modes/interactive/slash_commands.rs`,
`interactive/mod.rs`, `interactive/input.rs`, `interactive/prompt_completion.rs`,
`interactive/tests.rs`.

Steps: reuse strict `slash_arg` boundary; keep bare `/workflow` as the
one-shot dynamic capability path; parse named name and complete JSON object in
the host; resolve registry, validate input, and call the same coordinator; do
not enter a model turn; reject `/workflowish` and ordinary-text forgery;
preserve Ask revise generation and capability consumption rules.

Verification:

```bash
cargo test --package neo-agent --bin neo modes::interactive::tests::named_workflow_slash_launches_without_model_call --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo modes::interactive::tests::workflow_slash_arguments_do_not_grant_capability --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo modes::interactive::tests::workflowish_is_not_workflow --exact --nocapture --include-ignored
```

Stop if named launch routes through the model or creates a second capability
owner. Commit: `feat(interactive): launch named workflows directly`.

#### Task 21: Extend `/tasks` projection, paging, filters, and actions

Files: `tools/background_tasks.rs`, `neo-agent/src/modes/task_browser.rs`,
`neo-agent/src/modes/interactive/input.rs`, `interactive/slash_commands.rs`,
`neo-tui/src/tasks_browser/{view,state,render,mod}.rs`, and focused tests.

Steps: replace hard `list(false, 50)` with stable paged workflow queries; add
handle, definition/scope/state/awaiting/lineage filters, query-bound cursors,
phase/child/queue/usage/result/artifact metadata; route pause/resume/answer/
stop/fork/prune-safe actions to canonical runtime controls; retain existing
card renderers and action semantics for all non-workflow task kinds.

Verification:

```bash
cargo nextest run -p neo-agent --test cli_commands tasks_workflow_pagination_and_filters_are_stable
cargo nextest run -p neo-tui --test task_browser task_browser_workflow_detail_and_cursor_rules
cargo test --package neo-agent --bin neo modes::interactive::tests::task_browser_workflow_controls_use_human_handle --exact --nocapture --include-ignored
rg -n 'list\(false, 50\)|inv_[A-Za-z0-9_]+.*provenance' crates/neo-agent/src crates/neo-tui/src
```

Stop if TUI state writes durable workflow data or if Delegate/Group/Swarm/Bash/
Terminal cards change. Commit: `feat(tasks): expose paged workflow dashboard`.

#### Task 22: Add deterministic validator and fixture harness

Files: new `neo-agent-core/src/workflow/check.rs`,
`neo-agent-core/src/workflow/harness.rs`, `workflow/mod.rs`,
`neo-agent/src/modes/workflow.rs` check/test adapters, new
`tests/workflow_check.rs`, `tests/workflow_harness.rs`, and fixture files under
`crates/neo-agent-core/tests/fixtures/workflows/`; the CLI adapter test is
`crates/neo-agent/tests/workflow_cli.rs`.

Steps: implement a pure `check` report by calling the existing registry,
schema, and Lua compiler owners rather than copying them. It covers pairing,
scope/name, syntax, phase uniqueness, schema compilation, limits, forbidden
static names as advisory diagnostics, built-in consistency, and revision. Add
a concrete `harness` fixture parser with `deny_unknown_fields` and fields for
args, delegate outcomes/one repair output, swarm outcomes, tool outcomes,
awaited answers, expected result/reports/artifacts, and invocation trace. Run
the real resolved definition, Lua host, WorkflowRuntime, journal/replay,
schema, and artifact code in temporary storage with fake outcomes. There is no
live-provider, shell, MCP, or hidden live-execution switch.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_check workflow_check_rejects_invalid_definition_without_creating_run
cargo nextest run -p neo-agent-core --test workflow_check builtin_manifest_revision_vectors_are_stable
cargo nextest run -p neo-agent-core --test workflow_harness deterministic_fixture_runs_real_lua_and_journal_without_external_effects
cargo nextest run -p neo-agent-core --test workflow_harness deterministic_fixture_records_one_child_schema_repair_with_tools_disabled
cargo nextest run -p neo-agent-core --test workflow_harness deterministic_fixture_replays_await_user_answer_and_artifact
cargo nextest run -p neo-agent-core --test workflow_harness live_execution_is_not_a_fixture_mode
cargo nextest run -p neo-agent --test workflow_cli workflow_check_json_is_stable_and_read_only
```

Stop if `test` silently switches to live providers/tools or duplicates runtime
execution logic. Commit: `feat(workflow): add validator and fixture harness`.

#### Task 23: Add ordinary registry built-ins

Files: new `workflow/builtins.rs` (or `workflow/builtins/mod.rs` if the
existing module convention requires it),
`workflow/builtins/deep-research.lua`,
`workflow/builtins/deep-research.workflow.toml`,
`workflow/builtins/code-review.lua`,
`workflow/builtins/code-review.workflow.toml`,
`workflow/builtins/large-refactor.lua`,
`workflow/builtins/large-refactor.workflow.toml`, `workflow/registry.rs`,
`workflow/harness.rs`, and `tests/workflow_builtins.rs`.

Steps: implement deep research, code review, and large refactor as ordinary
definitions using only public Lua APIs; require structured outputs and
artifacts; use heterogeneous children; preserve read-only code review;
default refactor mutations to isolated worktrees; await explicit human choices
at merge/retirement boundaries; never auto-merge or delete worktrees; cover
each through deterministic fixtures.

Verification:

```bash
cargo nextest run -p neo-agent-core --test workflow_builtins all_builtin_definitions_validate_through_public_registry
cargo nextest run -p neo-agent-core --test workflow_builtins deep_research_builtin_fixture
cargo nextest run -p neo-agent-core --test workflow_builtins code_review_builtin_is_read_only_and_findings_first
cargo nextest run -p neo-agent-core --test workflow_builtins large_refactor_builtin_requires_explicit_merge_decision
```

Stop if built-ins receive privileged host functions or bypass the public
registry/schema/artifact/lineage contract. Commit: `feat(workflow): add builtin workflows`.

#### Task 24: Update English and Chinese user/author documentation

Files: `docs/en/guides/workflows.md`, `docs/zh/guides/workflows.md`,
`docs/en/reference/tools.md`, `docs/zh/reference/tools.md`,
`docs/en/reference/slash-commands.md`, `docs/zh/reference/slash-commands.md`,
`docs/en/configuration/config-files.md`, `docs/zh/configuration/config-files.md`,
`docs/en/configuration/data-locations.md`, `docs/zh/configuration/data-locations.md`,
`docs/.vitepress/config.ts`, and relevant current interaction pages.

Document paired files and precedence/trust, Lua APIs and schemas, exactly-one
repair, actual usage and machine limits, AwaitingUser non-secret warning,
swarm scale/backpressure, artifacts, linked runs, TaskOutput cursors, prune
safety, CLI/slash commands, and built-ins in both languages. Do not describe
unimplemented behavior as current; update docs only after the corresponding
code task evidence exists.

Verification:

```bash
rg -n 'Rhai|agent_budget|max_concurrency|token budget|wall-clock timeout' docs/en docs/zh
rtk npm --prefix docs run build
git diff --check -- docs/en docs/zh docs/.vitepress/config.ts
```

Expected: only explicit non-goal/compatibility explanations remain; no stale
claim says a predictive budget or second engine is supported. Commit:
`docs(workflow): document local workflow platform`.

#### Task 25: Native cross-platform and final retirement verification

Files: platform-specific tests under workflow registry/journal/artifact/
lineage/prune targets, generated superseding ADR
`docs/aegis/adr/ADR-0006-local-workflow-platform.md` (actual helper-selected
number if occupied), new baseline
`docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`,
`docs/aegis/INDEX.md`, and the final implementation diffs.

Steps: run macOS path/link/atomic-replace/sync tests; inspect available host
memory before starting one Parallels VM; boot only one Linux or Windows VM at a
time, run the exact native workflow target, record evidence, and stop the VM;
after all implementation and native evidence, use the Aegis workspace helper
to supersede ADR-0004 with a new ADR that records only landed behavior, and
create the dated current baseline; do not overwrite historical ADR-0004 or
write proposal text as current;
perform all stale-owner scans; stage only workflow plan-approved files.

Verification:

```bash
cargo fmt --all --check
cargo nextest run -p neo-agent-core --test workflow_registry registry_platform_path_and_link_semantics
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_platform_sync_and_quarantine_semantics
cargo nextest run -p neo-agent-core --test workflow_artifacts artifact_replace_and_integrity_are_platform_safe
python /Users/chenyuanhao/.codex/aegis/scripts/aegis-workspace.py check --root /Users/chenyuanhao/Workspace/neo
rg -n 'WorkflowHostRecorder|run_script|host_api|child_tools\.run|MAX_SWARM_CHILDREN|token_cap|read_journal\(&guard\.journal_path|mode=background|engine abstraction|Rhai' crates docs/en docs/zh
git diff --check
git diff --cached --check
```

Expected stale scan: zero active old-owner hits, with only explicitly retained
V1 read fixtures and historical documentation references allowed. Native VM
results must name OS and command; local proof is not remote CI proof.

Stop on a stale active writer, duplicate owner, persistent-state ambiguity,
unverified platform degradation, unrelated dirty-file conflict, or any need
for destructive Git. Commit: `chore(workflow): close platform verification and baseline`.

## Dependency Waves And Parallel Dispatch

- Wave 1 (serial substrate): Tasks 1-4.
- Wave 2 (parallel after Task 4): Tasks 5-8; Task 5 binds runtime dispatch,
  Task 6 owns admission/config, Task 7 owns artifact/result, Task 8 owns
  lineage. No shared file edits without root serialization.
- Wave 3 (definition/launch): Tasks 9-11 serial because all define the launch
  contract and metadata snapshot.
- Wave 4 (parallel after Task 11): Tasks 12-17 with separate leases; Task 15
  and Task 16 serialize on multi-agent/tool registry files; Task 17 waits for
  Task 8.
- Wave 5 (operator/author surfaces): Tasks 18-23; Task 21 waits for Task 18,
  Task 23 waits for Tasks 12-17, and CLI adapters wait for core APIs.
- Wave 6: Task 24 docs after corresponding behavior; Task 25 is last.

Use at least three implementation subagents for independent leases. Each
subagent receives one task lease, returns changed files and exact command output,
performs no Git mutation, and stops on its first boundary violation. The root
reviews, verifies, stages, and commits each task before sending another task to
the same agent.

## Verification Matrix

| Spec area | Plan tasks | Required proof |
| --- | --- | --- |
| Registry and exact launch | 9-11, 20 | precedence, hash vectors, zero-model named launch, capability exact match |
| Journal/recovery | 1-5 | V1 fixtures, torn-tail quarantine, fail-closed corruption, no effect retry |
| Admission/scale | 6, 15 | actual permits, queued pending, >8 heterogeneous children, no predictive cap |
| Output/artifacts | 7, 8, 18 | final result, artifact integrity, linked prefix, bounded cursors |
| Schemas/repair | 9, 12-14 | strict JSON, one repair, durable AwaitingUser |
| Tool/child policy | 13, 15-17 | canonical registry, deny set, capability ceiling, provenance |
| CLI/TUI | 19-21 | stable JSON/JSONL, prune safety, `/tasks` pagination and controls |
| Validator/built-ins | 22-23 | deterministic fixtures, no external effects, public APIs only |
| Cross-platform/docs | 24-25 | bilingual docs, native path/link/sync proof, baseline sync |

## Retirement And ADR Track

Internal code retirement is `delete-first`: old direct launch transaction,
foreground/V1 writer, workflow-local dispatch, count constants, token cap,
full-journal output, aliases, fuzzy schema extraction, and duplicate result
owners are deleted when their replacement is verified. V1 persistent files are
an external durable-data boundary: `compat-exception` for read-only fixtures
and linked upgrade only; no V1 byte mutation or deletion is authorized.

After implementation evidence exists, amend or supersede ADR-0004 and sync the
2026-07-23 runtime baseline with actual current behavior, compatibility, and
residual risk. Do not update either document during planning to claim shipped
behavior.

## Risks And Stop Conditions

- Journal or artifact corruption cannot be classified as a torn tail: stop,
  preserve bytes, and ask for an explicit persistence decision.
- Any proposal to retry uncertain Delegate, Swarm, Bash, Terminal, MCP, generic
  tool, or repair effects: stop and return to the spec.
- Any second durable state/result/tool/definition owner: stop and collapse it.
- Any dependency upgrade, unsafe code, hosted service, live provider in the
  deterministic harness, auto-merge, or hidden timeout: stop.
- Native Windows reparse/replace/sync behavior not proven: mark the task
  unverified; do not claim cross-platform completion.
- Existing `.gitignore` modification is unrelated and must remain untouched.

## Completion Contract

The root agent may claim this plan complete only after fresh evidence names the
commands, exit statuses, covered and uncovered scope, residual risk, and
confidence. The plan itself is not implementation evidence and does not grant
completion authority.
