# Neo Workflow Product Surface Redesign Implementation Plan

Date: `2026-07-28`

Status: `approved spec; implementation delegated through the accompanying handoff`

## Goal

Implement the approved Workflow product redesign so that:

1. humans use one coherent interactive path through `/workflow` and `/tasks`;
2. automation sees exactly four same-level CLI commands: `list`, `run`, `check`,
   and `test`;
3. the top-level model retains all seven `Workflow` actions, while `run_*` and
   `save` perform their complete validation internally;
4. every direct delegate and swarm item has durable, replayable lineage and a
   live Operator row;
5. no useful workflow capability is lost merely to simplify the surface.

The user accepts substantial refactoring. The implementation is therefore
optimized for the strongest coherent product contract, not minimum rewrite
cost.

## Architecture

The implementation keeps the landed owners and changes only their contracts:

```text
Human / Model / CLI launch
          |
          v
WorkflowLaunchCoordinator
          |
          v
WorkflowRuntime + V3 journal  <---- durable lifecycle and child facts
          |
          +---- BackgroundTaskManager ---- MultiAgentRuntime live snapshots
          |              |
          |              v
          |        /tasks Workflow Operator
          |
          +---- existing session completion delivery
```

Canonical ownership remains:

- `WorkflowDefinitionRegistry`: trusted discovery, resolve, save, and no-clobber;
- `WorkflowLaunchCoordinator`: launch preflight and durable create ordering;
- `WorkflowRuntime`: lifecycle, control, journal, recovery, invocation replay,
  child lineage, terminal result, and authoritative answer validation;
- workflow journal: durable child queued/started/finished facts;
- `MultiAgentRuntime`: live activity and actual live usage only;
- `BackgroundTaskManager`: task lookup, workflow control forwarding, and the
  durable/live Operator join;
- `neo-tui`: view state, selection, scrolling, responsive rendering, and answer
  drafts only;
- existing session queue/delivery: terminal completion delivery to the root
  session.

No second runtime, registry, scheduler, task system, persistence store, child
state machine, or completion queue may be introduced.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- `tokio` for existing async runtime paths;
- `serde`, `serde_json`, `schemars`, and the existing `jsonschema` dependency;
- `mlua` as the only workflow script engine;
- `clap` for CLI grammar;
- `crossterm` and existing `neo-tui` primitives for terminal UI;
- existing atomic file, path-containment, session JSONL, workflow artifact,
  admission, and retention helpers;
- no new production dependency.

An already-present workspace dependency may be used as a dev dependency for
PTY-only tests only when the existing test harness cannot express the behavior.

## Baseline And Authority Refs

Primary requirement authority:

- `docs/aegis/specs/2026-07-27-workflow-product-surface-redesign.md`
- approval commit `bff4931b`

Current landed contracts to preserve where not superseded:

- `docs/aegis/adr/ADR-0006-local-workflow-platform.md`
- `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
- `docs/aegis/baseline/2026-07-26-workflow-platform-contract.md`
- `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`

Reference implementation evidence already incorporated by the approved spec:

- `.references/grok-build/crates/codegen/xai-grok-tools/src/implementations/grok_build/workflow/mod.rs`
- `.references/grok-build/crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs`
- `.references/grok-build/crates/codegen/xai-grok-shell/src/session/workflow/registry.rs`
- `.references/grok-build/crates/codegen/xai-grok-shell/src/session/workflow/tracker.rs`
- `.references/grok-build/crates/codegen/xai-grok-pager/src/views/workflows.rs`

The reference tree is read-only comparison material. No product code may be
copied blindly and no second Grok-style runtime may be added.

## Compatibility Boundary

Preserve:

- all seven model-visible `Workflow` actions;
- paired `.lua` plus `.workflow.toml` definitions and existing registry scopes;
- named host-direct `/workflow <name> [JSON_OBJECT]` launch;
- exact bare `/workflow` authoring entry;
- Lua, host schemas, child-effect permission checks, typed approvals, and
  root-only `Workflow` / `TaskAnswer` registration;
- V1 existing read-only behavior;
- V2 journal readability without migration or rewrite;
- linked-run lineage, artifacts, final results, actual usage, and task controls;
- existing Workflow, Delegate, DelegateGroup, and DelegateSwarm transcript card
  layout, content, expansion, timing, and placement.

Intentionally break and delete:

- CLI `show`, `save`, `answer`, `fork`, and `prune`;
- CLI `--detach`, `--scope`, `--checkpoint`, `--older-than`, `--max-bytes`,
  `--dry-run`, `--yes`, and old output/argument spellings;
- mandatory model call ordering before `run_*` or `save`;
- prompt-keyword admission that blocks otherwise valid Workflow actions;
- TUI workflow fork/prune and empty-object answer behavior;
- V3 writes of legacy `SwarmItem*` events;
- hidden aliases, compatibility branches, and duplicate owners.

External CLI scripts may break. The approved spec explicitly chooses a clean
breaking surface; do not retain aliases for unknown consumers.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: targeted post-change regression plus fresh product acceptance
- Reason: the project does not request strict test-first TDD. Each task must add
  the smallest regression that proves its changed contract.
- Verification: exact package + target + test filters listed per task; no broad
  package-wide test command is accepted as completion evidence.

## Scope Check

### Facts

- The current CLI exposes nine commands and contains a synthetic headless
  runner and buffered JSONL path.
- The current model route scans prompt/transcript text and can force
  `validate_inline`, blocking valid `save` and `run_saved` intents.
- `run_inline`, `run_saved`, and `save` already converge on typed preflight
  owners; the defect is contract and gating, not missing validation machinery.
- V2 records swarm item lifecycle but direct delegates expose durable identity
  only in terminal invocation child refs.
- `BackgroundTaskManager` currently projects aggregate workflow counts and has
  no child roster/live merge.
- `/tasks` currently submits `{}` for a workflow answer.
- existing completion delivery already has natural-turn and idle-session tests.

### Assumptions

- Existing `WorkflowDispatchResolver::bind_workflow_runtime`,
  `runtime_for_config`, and the canonical tool registry can be reused to create
  a real headless execution host without a second CLI runner.
- Existing dialog/input primitives can supply text editing, selection, and
  confirmation mechanics; the workflow answer form still needs a
  workflow-specific schema-to-control projection.
- All implementation tests can use temporary roots. No test needs to operate on
  the user's real `~/.neo` workflow data.

### Unknowns To Resolve Inside The Named Task

- Exact terminal signal plumbing for controlled Ctrl+C varies by platform. Task
  9 owns this and must keep platform-specific code behind `cfg` with a portable
  default.
- The terminal generated-files aggregate does not exist before the Operator
  projection is built. Task 2 therefore pins metadata and preserves the base
  completion path; Task 5 creates the runtime-owned aggregate and then enriches
  completion delivery. Neither task may add a queue-local journal scan.

Neither unknown changes the approved contract. If resolving either would require
a new owner or functionality loss, stop that task and return to the user.

## Requirement Ready Check

- Requirement source refs: approved spec sections 1-32
- Goals and scope refs: spec sections 1, 4, 5, 30
- User/scenario refs: spec sections 8 and 29
- Requirement item refs: CLI section 9, model section 10, Operator sections
  11-25, retirement section 26
- Acceptance/verification refs: spec sections 28-29
- Open blocker questions: none
- Decision: `ready`

## Baseline Usage Draft

- Required baseline refs: ADR-0006, ADR-0007, 2026-07-26 baseline,
  2026-07-27 baseline
- Acknowledged before plan refs: all four
- Cited in plan refs: all four
- Missing refs: none
- Decision: `continue`

## Ripple Signal Triage

The changes cross five contracts:

1. durable journal writer/reader/recovery;
2. child dispatch and live multi-agent projection;
3. task manager to TUI view contract;
4. CLI process/output/exit contract;
5. model tool, skill, prompt, and completion contract.

Each downstream consumer is assigned to a named task and an exact regression.
No task may declare completion from a unit test that bypasses its downstream
adapter.

## Change Necessity

- User-visible need: one understandable, powerful workflow product for novice
  humans and imperfect models.
- No-change/non-code option: insufficient; the current CLI execution, model
  admission, child facts, and answer action are behaviorally wrong.
- Why code change is necessary: prompts/docs cannot make a synthetic runner
  execute Lua, create missing durable child lifecycle, produce a live roster, or
  submit typed answers.
- Minimum change boundary: existing launch/runtime/journal/task/TUI/model/CLI
  owners plus workflow-specific projection modules.
- Decision: `code-change`

## Existence Check

- Proposed new surface: Workflow Operator inside `/tasks`.
- Existing owner/reuse candidate: Task Browser overlay, periodic refresh,
  `BackgroundTaskManager`, `WorkflowRuntime`, and `neo-tui`.
- Why existing surface is insufficient: aggregate counts cannot represent child
  identity, live work, typed input, per-step outcomes, or stable paging.
- Creation proof: required by approved human oversight scenarios.
- Entropy/retirement impact: replace current workflow detail/actions; do not add
  a dashboard, overlay kind, slash command, runtime, or manager.
- Decision: `add-with-proof` inside the existing `/tasks` owner.

## Architecture Integrity Lens

- Invariant: journal facts are authoritative for durable lifecycle; live agent
  facts can enrich but never overwrite terminal durable facts.
- Canonical contract: one launch coordinator, one runtime, one task manager,
  one model Workflow tool, one Operator projection.
- Responsibility overlap to avoid: TUI journal parsing, title-based child joins,
  CLI-local Lua execution, queue-local completion reconstruction, and dual V2/V3
  writes.
- Higher-level simplification: all adapters use the same launch/save/control
  owners; V3 child lifecycle replaces new legacy swarm writes.
- Retirement falsifier: any active old CLI route, mandatory action choreography,
  V3 `SwarmItem*` writer, workflow fork/prune key, or `{}` answer path.
- Verdict: proceed with delete-first internal retirement.

## Anti-Entropy Declaration

- Deletion class: internal code retirement and contract-carrying code.
- Old paths: five CLI variants, old flags/helpers, synthetic headless runner,
  buffered JSONL, prompt-keyword route gate, old TUI workflow controls, empty
  answer confirmation, new-run legacy swarm writes.
- New canonical owners: existing coordinator/runtime/task/control owners plus V3
  generic child records and the workflow-specific `/tasks` projection.
- Expected preserved behavior: all seven Workflow actions, runtime fork/lineage,
  registry save, task answer/control, automatic storage safety, and V2 reads.
- Expected retired behavior: public low-level assembly and duplicate writer/UI
  paths.
- External boundary touched: yes, CLI is deliberately breaking.
- Source-of-truth data risk: automatic retention can delete eligible old terminal
  run directories in real use; implementation tests are temp-root-only and may
  never target user data.
- User confirmation required for implementation: no; the exact policy is in the
  approved spec. Running a destructive maintenance command against real data is
  not authorized by this plan.

Retirement Decision:

- Path: `delete-first`
- Why: removed paths are internal or explicitly retired public CLI contracts;
  no active external dependency evidence justifies compatibility aliases.
- Non-edits: no migration or rewrite of V1/V2 journals, historical specs, old
  ADRs/baselines, or prior evidence.

## Complexity Budget

| Artifact | Current pressure | Planned governance |
| --- | --- | --- |
| `workflow/runtime.rs` (~4356 lines) | over budget | wiring/local replacement only; new child projection in a new owner file |
| `tools/background_tasks.rs` (~3856) | over budget | thin forwarding; place join/paging in a nested workflow-specific module |
| `runtime/tool_dispatch.rs` (~2583) | over budget | delete route gate; only minimal origin wiring |
| `workflow/lua.rs` (~1641) | strong pressure | local producer wiring; domain logic in runtime child API |
| `modes/workflow.rs` (~1202) | strong pressure | deletion should make it smaller; do not add a second CLI module unless it remains over budget after retirement |
| `interactive/input.rs` (~1240) | strong pressure | move Operator actions to `interactive/workflow_operator.rs` |
| Task Browser modules | moderate | generic browser remains stable; add a bounded workflow-specific submodule |

Plan-Time Complexity Check:

- Owner fit: new durable types/projection live under `workflow`; UI state/render
  live under `tasks_browser/workflow_operator`.
- Add-in-place risk: high in runtime, task manager, input, and generic renderer.
- Better boundary: new `workflow/child_projection.rs`, nested task-manager join,
  new workflow Operator state/render/answer modules, and thin integration calls.
- Recommendation: extract new responsibility; keep existing large files to
  wiring or deletion.

## File Map

### New Core Files

- `crates/neo-agent-core/src/workflow/child_projection.rs`
- `crates/neo-agent-core/src/workflow/operator.rs`
- `crates/neo-agent-core/src/tools/background_tasks/workflow_operator.rs`
- `crates/neo-agent-core/tests/workflow_journal_v3.rs`
- `crates/neo-agent-core/tests/workflow_operator.rs`

### New TUI/Host Files

- `crates/neo-tui/src/tasks_browser/workflow_operator/mod.rs`
- `crates/neo-tui/src/tasks_browser/workflow_operator/state.rs`
- `crates/neo-tui/src/tasks_browser/workflow_operator/render.rs`
- `crates/neo-tui/src/tasks_browser/workflow_operator/answer.rs`
- `crates/neo-tui/tests/workflow_operator.rs`
- `crates/neo-agent/src/modes/interactive/workflow_operator.rs`

### Primary Modified Core Files

- `crates/neo-agent-core/src/workflow/{mod,state,journal,journal_scan,recovery,effect,runtime,lua,output,retention}.rs`
- `crates/neo-agent-core/src/runtime/{workflow_dispatch,tool_dispatch}.rs`
- `crates/neo-agent-core/src/tools/{mod,delegate,workflow,workflow_tests,background_tasks}.rs`
- `crates/neo-agent-core/src/skills/{mod,builtin/mod}.rs`
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`

### Primary Modified Neo/TUI Files

- `crates/neo-agent/src/{cli,main}.rs`
- `crates/neo-agent/src/modes/{workflow,task_browser}.rs`
- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent/src/modes/interactive/{mod,input,slash_commands,prompt_completion,tests}.rs`
- `crates/neo-agent/tests/{workflow_cli,cli_commands,workflow_notifications}.rs`
- `crates/neo-tui/src/tasks_browser/{mod,state,render,view}.rs`

### Final Documentation Files

- `docs/{en,zh}/guides/{workflows,interaction}.md`
- `docs/{en,zh}/reference/{tools,slash-commands}.md`
- `docs/{en,zh}/configuration/{config-files,data-locations}.md`
- new ADR-0008, new landed baseline, and the current workstream checkpoint,
  evidence, and reflection files
- `docs/aegis/INDEX.md`

Historical workflow specs/plans/handoffs/ADRs/baselines/evidence are immutable.

## Shared Type Contracts

Add the following domain shapes, using project naming conventions and derives:

```rust
pub enum WorkflowChildKey {
    DirectDelegate { invocation_id: String },
    SwarmItem { swarm_id: String, item_id: String },
}

pub struct WorkflowStepKey {
    pub phase_id: Option<String>,
    pub phase_marker_sequence: u64,
}

pub enum WorkflowChildKind {
    Delegate,
    SwarmItem,
}

pub enum WorkflowChildState {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Recovering,
}

pub struct WorkflowOperatorRequest {
    pub step: Option<WorkflowStepKey>,
    pub cursor: Option<String>,
    pub limit: usize,
}

pub struct WorkflowChildPage {
    pub items: Vec<WorkflowChildRow>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub query_hash: String,
}
```

The full row/snapshot fields are those in approved spec sections 18.1-18.4.
Do not add a random child UUID or durable phase-instance UUID.

Pinned run metadata must gain backward-readable optional fields for:

```rust
pub display_name: Option<String>,
pub input_schema: Option<serde_json::Value>,
pub definition_origin: Option<WorkflowSourceOrigin>,
pub inline_unsaved: bool,
```

Use one canonical projection helper to fall back from missing legacy
`display_name` to `name`. Do not repeat legacy fallback logic in the Operator,
completion queue, CLI, and save dialog.

## Task 1: Remove Model Choreography Without Reducing Capability

Files:

- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/tools/workflow_tests.rs`
- `crates/neo-agent-core/src/skills/mod.rs`
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`

Why: the current prompt-keyword route can block valid product actions and makes
the model assemble a ritual that the runtime already owns.

Change Necessity: wording alone is insufficient because
`workflow_route_violation` blocks tool execution in production. The minimum
boundary is the route gate, tool description/result contract, and skill prompt.

Steps:

1. Delete workflow-specific transcript/keyword scanning,
   `workflow_evaluation_route`, its blocked outcome, and its call site.
2. Preserve only the generic single-skill-call isolation invariant. It must not
   select or order product skills by task type.
3. Remove the global task-specific-before-methodology sentence from the skill
   catalog prompt. Keep the ordinary rule that a relevant or explicitly named
   skill should be invoked.
4. Keep exactly the seven existing `WorkflowAction` variants and the existing
   strict action-field parser.
5. Make descriptions and errors state that `run_inline`, `run_saved`, and
   `save` perform complete preflight internally. `validate_*` are for an
   explicit check-only request.
6. Ensure launch output contains `status=started`, task handle, display name,
   purpose, `automatic_notification=true`, and
   `next_action=wait_for_completion`. `TaskOutput` is optional detail, not a
   required polling loop.
7. Rewrite `create-workflow` to teach authoring, Lua, schemas, and host APIs.
   It may recommend itself for custom authoring but may never be a launch grant.
8. Known saved workflows continue to allow direct `list`, `show`, and
   `run_saved` without skill activation.
9. Remove every active source occurrence of the user-forbidden product term
   identified by ICM preference `01KYJ3WSM7SBWJFAJ4KYJARS0Q`; do not quote it
   in code, docs, comments, test names, or commits.

Verification:

```bash
rtk cargo test --package neo-agent-core --lib -- runtime::tool_dispatch::tests::skill_activation_stays_isolated_without_workflow_choreography --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --lib -- tools::workflow::workflow_tests::run_inline_starts_without_prevalidation_and_returns_completion_contract --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --lib -- tools::workflow::workflow_tests::saved_actions_run_before_explicit_validation --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_tool_policy -- workflow_tool_is_root_only_and_description_has_no_choreography --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --lib -- skills::builtin::tests::create_workflow_builtin_teaches_authoring_without_mandatory_choreography --exact --nocapture --include-ignored
```

Expected evidence:

- valid `save`, `run_inline`, and `run_saved` execute as first Workflow actions;
- explicit validation remains read-only and supported;
- invalid mutation preflight leaves zero file/run/task/approval effects;
- no CLI/slash/source/Cargo prerequisite appears in model-facing output.

Commit: `fix(workflow): make workflow actions self-contained`

## Task 2: Pin Product Metadata And Preserve Terminal Delivery

Files:

- `crates/neo-agent-core/src/workflow/state.rs`
- `crates/neo-agent-core/src/workflow/launch.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs` (wiring/local replacement)
- `crates/neo-agent-core/src/runtime/queue.rs`
- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- `crates/neo-agent/tests/workflow_notifications.rs`

Why: Operator display, contextual Save, CLI output, and completion delivery all
need one pinned exact definition projection. Re-parsing `launch_source` or
re-reading an edited workspace file would produce incorrect behavior. This task
must remain independently committable before Task 5 creates the terminal files
aggregate.

Change Necessity: the current `WorkflowRunMetadata` drops display name and input
schema. The minimum fix is to carry optional backward-readable fields through
the existing launch request into immutable run metadata.

Steps:

1. Add the optional metadata fields listed in Shared Type Contracts with serde
   defaults so old run metadata remains readable.
2. Populate them in every launch adapter through `WorkflowLaunchRequest`; do not
   infer them after durable creation.
3. Add one helper on metadata that returns effective display name and exact
   pinned definition/save request.
4. Preserve existing output schema, args, phases, script, revision, lineage,
   and source-origin behavior.
5. Build the base terminal notification from fields already owned at this
   stage: display name, purpose, state, summary/failure, and task handle.
6. Keep existing safe-break-point/idle delivery and exactly-once persistence.
   Do not create another queue or polling loop.
7. Do not add generated files in this task and do not scan the journal inside
   the queue. Task 5 owns both the aggregate and the final notification
   enrichment.

Verification:

```bash
rtk cargo test --package neo-agent --test workflow_notifications -- terminal_workflow_notification_waits_for_natural_turn --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_notifications -- active_turn_completion_notifies_queued_follow_up --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_notifications -- completion_uses_pinned_display_name_purpose_and_task_handle --exact --nocapture --include-ignored
```

Expected evidence: old metadata loads with `name` fallback; new runs preserve
the exact definition; one terminal notification arrives without polling.

Commit: `feat(workflow): pin product metadata for run delivery`

## Task 3: Introduce V3 Journal And Read-Only V2 Projection

Files:

- `crates/neo-agent-core/src/workflow/{journal,journal_scan,recovery,state,output,mod}.rs`
- `crates/neo-agent-core/src/workflow/child_projection.rs`
- `crates/neo-agent-core/tests/workflow_journal_v3.rs`

Why: the Operator cannot truthfully show direct delegates or recover child rows
without generic durable child facts.

Repair Track:

- Root cause: V2 swarm-specific events and terminal-only direct child refs do not
  form one generic lifecycle.
- Canonical owner: journal envelope + runtime child projection.
- Stable repair: V3 generic events and a version-aware read-only projector.
- Compatibility: V1/V2 remain readable and untouched.

Retirement Track:

- Old owner: `SwarmItemQueued/Started/Finished` for new writes.
- Active status: V2 read compatibility only.
- Deletion trigger: no new-run writer references after Task 4.

Steps:

1. Add `JOURNAL_FORMAT_V3` and version-aware validation/scanning. Do not silently
   accept unknown versions.
2. Generalize the current V2 envelope writer name/constructor so new runs write
   version 3 while an existing V2 run is never opened for append.
3. Add `ChildQueued`, `ChildStarted`, and `ChildFinished` with the exact keys and
   payload-ref rules from spec section 19.
4. Reuse `JournalPayloadRef` and the artifact store. A specification/outcome is
   serialized once; invocation and child records refer to the same canonical
   payload.
5. Add bounded child projection by journal scan/page. Duplicate keys fail with a
   typed projection error.
6. Map V2 `SwarmItem*` to generic rows; reconstruct a V2 direct delegate only
   from terminal child refs.
7. Project a started child without a durable finish as `Recovering`; never
   `Running` after restart.
8. Make V2 strictly read-only: no migration, tail repair append, reconciliation
   append, or writer open. Existing fail-closed corruption behavior remains.
9. Derive step occurrences from phase marker envelope sequence. A workflow with
   no marker gets view-only `Execution`.

Verification:

```bash
rtk cargo test --package neo-agent-core --test workflow_journal_v3 -- journal_v3_generic_child_lifecycle_round_trips_and_replays --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_journal_v3 -- v2_terminal_children_project_read_only_without_rewrite --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_journal_v3 -- started_without_finished_projects_recovering --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_journal_v3 -- unknown_or_torn_v3_data_remains_fail_closed --exact --nocapture --include-ignored
```

Commit: `feat(workflow): add journal v3 child lifecycle`

## Task 4: Emit One Generic Lifecycle For Direct And Swarm Children

Files:

- `crates/neo-agent-core/src/workflow/{effect,runtime,lua}.rs`
- `crates/neo-agent-core/src/runtime/{workflow_dispatch,tool_dispatch}.rs`
- `crates/neo-agent-core/src/tools/{mod,delegate}.rs`
- `crates/neo-agent-core/tests/{workflow_dispatch,workflow_journal_v3}.rs`

Why: the V3 schema is useful only if every production child path records it
before live work and exactly once at terminal state.

Steps:

1. Add runtime methods that prepare/commit queued, started, and finished child
   events. The runtime, not Delegate or TUI code, performs journal writes.
2. Direct delegate key is the existing invocation ID. Persist `ChildQueued` and
   canonical spec before tool dispatch.
3. Pass typed `WorkflowExecutionOrigin` through
   `run_one_with_origin` into `ToolContext`. Do not infer ownership from active
   invocations or titles.
4. After Delegate creates an `AgentSnapshot`, bind its agent ID through the
   runtime before emitting live started/model work.
5. Materialize terminal outcome once and make `InvocationFinished` and
   `ChildFinished` reference the same outcome payload.
6. For swarm items, preserve `(swarm_id,item_id)` and bind agent ID from the
   prepared child snapshot.
7. Normalize all supported `neo.swarm` forms to `ChildPlan` and one per-item
   producer path. Remove any new-run homogeneous special writer.
8. Delete V3 writes of `SwarmItem*`; retain only V2 reader projection.
9. Do not create top-level Delegate/Swarm background task rows for workflow
   children.
10. Keep schema repair/resource admission as activity/reason fields, not child
    lifecycle states.

Verification:

```bash
rtk cargo test --package neo-agent-core --test workflow_dispatch -- delegate_usage_and_child_ref_are_journaled_and_aggregated --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_dispatch -- swarm_preserves_ids_terminal_children_and_aggregate_usage --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_journal_v3 -- direct_and_swarm_children_replay_exactly_once_with_unresolved_started_as_recovering --exact --nocapture --include-ignored
```

Commit: `feat(workflow): journal direct and swarm children uniformly`

## Task 5: Build Durable/Live Operator Projection And Paging

Files:

- `crates/neo-agent-core/src/workflow/operator.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs` (thin methods only)
- `crates/neo-agent-core/src/tools/background_tasks.rs` (thin forwarding only)
- `crates/neo-agent-core/src/tools/background_tasks/workflow_operator.rs`
- `crates/neo-agent-core/src/multi_agent/runtime.rs` only if an existing public
  snapshot method is insufficient
- `crates/neo-agent-core/src/runtime/queue.rs` (terminal projection wiring only)
- `crates/neo-agent-core/tests/workflow_operator.rs`
- `crates/neo-agent/tests/workflow_notifications.rs`

Why: the TUI needs one immutable, user-oriented query result. It must not parse
journals or own merge logic.

Steps:

1. Implement the shared types in approved spec section 18 and this plan's
   Shared Type Contracts.
2. Add `WorkflowHandle::operator_snapshot(request)` that reads durable state,
   steps, child page, pending input, summary/failure, and generated files.
3. Add `BackgroundTaskManager::workflow_operator_snapshot(task_id, request,
   &MultiAgentRuntime)`.
4. Join only by durable `agent_id`; never by title, role, or row position.
5. Durable terminal facts win. Live data may enrich non-terminal latest
   activity and actual usage only. Stale live timestamps cannot replace newer
   terminal records.
6. Cursor binds run ID, step key, and query hash; reject cross-run/step reuse.
7. Preserve durable creation order. Do not reorder failures or running rows.
8. Page by selected step; use an opaque stable cursor and bounded scan. No total
   child cap and no full-journal/full-activity materialization per frame.
9. Counts are observed state counts. Do not invent a denominator for an open
   dynamic step.
10. Expose actual usage for Details only; the main row model must not contain a
    preformatted usage column.
11. Make terminal generated files part of the same runtime-owned aggregate.
    After that aggregate exists, enrich the existing completion notification
    with generated files through thin queue wiring. Do not rescan the journal or
    create a second completion projection.
12. Project schema repair, resource admission, human input, recovery, terminal
    failure, and cancellation as typed reasons. Do not collapse them into a
    generic waiting label.

Verification:

```bash
rtk cargo test --package neo-agent-core --test workflow_operator -- operator_projection_merges_live_activity_without_overwriting_durable_terminal_state --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_operator -- operator_rejects_duplicate_keys_and_cross_query_cursors --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_operator -- operator_pages_one_and_ten_thousand_children_by_stable_key_without_total_limit --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_notifications -- completion_uses_runtime_owned_files_projection_without_polling --exact --nocapture --include-ignored
```

Commit: `feat(workflow): project workflow operator state`

## Task 6: Add Workflow Operator Navigation, Rendering, And Details

Files:

- `crates/neo-tui/src/tasks_browser/workflow_operator/{mod,state,render}.rs`
- `crates/neo-tui/src/tasks_browser/{mod,state,render,view}.rs`
- `crates/neo-agent/src/modes/task_browser.rs`
- `crates/neo-agent/src/modes/interactive/{mod,input,workflow_operator,slash_commands,tests}.rs`
- `crates/neo-tui/tests/workflow_operator.rs`

Why: workflow tasks need a comprehensible Steps/Agents/Details view while
ordinary task behavior remains unchanged.

Steps:

1. Keep the existing Task Browser overlay. Add a `WorkflowOperatorState` child
   state; do not add another overlay or dashboard.
2. `/tasks` entry priority is most-recent Needs input workflow, then most-recent
   active workflow, else current general browser.
3. In the general browser, selection movement never switches pages. `Enter` on
   Workflow opens Operator; `Esc` restores prior task selection and viewport.
4. Operator focus is exactly Steps and Agents. `Tab` switches; Up/Down moves;
   `Enter` opens selected agent Details.
5. Selection is keyed by `WorkflowStepKey`/`WorkflowChildKey`, not numeric index.
   Live refresh preserves selection. Active-step following stops after manual
   step movement and resets on reopen.
6. Wide (`>=100`), stacked (`70-99`), and sequential (`<70`) rendering follows
   the approved character layouts. Use stable dimensions and Unicode-width-aware
   truncation.
7. Header shows display name, purpose, macro state, elapsed, observed counts,
   and Needs input only. No IDs, hashes, revisions, scopes, checkpoints, raw
   events, predictions, or token total.
8. Main agent rows show state, title, optional useful role, current activity,
   and elapsed. Details alone may show actual usage/model/provider.
9. Reuse the existing compact child-activity projection for tool/file activity.
   Do not modify transcript components or card contracts.
10. Keep the existing one-second refresh loop. There is no manual refresh key,
    output-cycle key, or transcript-jump key.
11. Mouse click selects; wheel scrolls the pane under the pointer; alternate
    screen mouse capture remains enabled.
12. Render failure, recovery, schema-repair, resource-wait, and input reasons in
    plain language in the appropriate step/agent/details region. Never show an
    ambiguous generic waiting state when a typed reason exists.

Verification:

```bash
rtk cargo test --package neo-tui --test workflow_operator -- operator_preserves_keyed_selection_and_hides_usage_from_agent_rows --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test workflow_operator -- operator_layouts_fit_all_required_widths_and_heights_without_overlap --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test workflow_operator -- operator_renders_failure_recovery_resource_and_input_reasons_in_plain_language --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_tasks_prioritizes_waiting_then_active_workflow_and_restores_browser_state --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test task_browser -- task_browser_populated_renderer_shows_counts_detail_preview_and_footer --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test multi_agent_transcript -- option_b_expanded_swarm_preserves_full_child_transcripts --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test workflow_transcript -- workflow_card_projects_orchestration_without_child_duplication --exact --nocapture --include-ignored
```

Commit: `feat(tui): add workflow operator to tasks`

## Task 7: Implement Typed Answers, Truthful Controls, And Contextual Save

Files:

- `crates/neo-tui/src/tasks_browser/workflow_operator/answer.rs`
- `crates/neo-tui/src/tasks_browser/workflow_operator/{state,render}.rs`
- `crates/neo-agent/src/modes/interactive/workflow_operator.rs`
- `crates/neo-agent/src/modes/interactive/input.rs` (routing/deletion only)
- `crates/neo-agent-core/src/workflow/{state,runtime,user_input}.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- focused tests above plus `crates/neo-agent/src/modes/interactive/tests.rs`

Why: the current answer route is unusable and pause must not claim that active
children have stopped when they are still finishing work.

Steps:

1. Add durable `WorkflowState::Pausing` and only the transitions required by
   spec section 17.1. Pausing prevents queued child starts while allowing
   already-approved children to finish when cancellation is unsafe.
2. `P` toggles pause/resume by calling existing BackgroundTaskManager methods.
   UI shows `Working (finishing current work)` until the runtime reaches Paused.
3. `X` opens the approved stop confirmation and calls `stop_with_actor(Human)`.
4. Delete workflow fork/prune actions, confirmations, keys, host handlers, and
   product text from the ordinary Task Browser.
5. Replace empty-object answer confirmation with a form driven by the durable
   pending request.
6. Support boolean, string enum, array enum, string, number/integer, object,
   nested object, array of objects, and titled `oneOf`/`anyOf` as documented.
7. Reuse existing choice/text/form primitives. Add only workflow-specific
   schema projection and draft state.
8. Submit-time UI validation uses the same `CompiledSchema`; runtime
   `WorkflowRuntime::answer` validates authoritatively again.
9. Field errors attach to the relevant path; unsupported advanced schemas use
   the validated structured editor fallback. Never accept secrets.
10. `Esc` dismisses without answering. Remember dismissal for the same request
    ID until manual reopen or a different request arrives.
11. Show `S save` only for inline unsaved runs. Rebuild from pinned metadata and
    route through existing typed permission + registry save. Use labels `This
    project` and `All projects`; never expose internal scope names.
12. Saving neither relaunches nor mutates the run. Builtin/already-saved runs
    have no Save binding.

Verification:

```bash
rtk cargo test --package neo-agent-core --test workflow_operator -- pausing_stops_new_starts_and_reports_running_agents_as_finishing_current_work --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test workflow_operator -- answer_form_builds_and_validates_supported_shapes_without_exposing_schema_terms --exact --nocapture --include-ignored
rtk cargo test --package neo-tui --test workflow_operator -- dismissed_request_stays_closed_until_request_changes_or_operator_reopens --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_operator_answers_controls_and_saves_through_canonical_owners --exact --nocapture --include-ignored
```

Commit: `feat(workflow): add typed operator controls`

## Task 8: Move Storage Reclamation To Automatic Host Policy

Files:

- `crates/neo-agent-core/src/workflow/retention.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs` (trigger wiring only)
- `crates/neo-agent-core/src/workflow/launch.rs` only if pre-denial hook wiring
  is required
- `crates/neo-agent/src/config/mod.rs` or existing app startup owner
- `crates/neo-agent/src/modes/workflow.rs` (move/delete helpers)
- `crates/neo-agent-core/tests/workflow_admission.rs`
- a focused neo-agent integration test if host filesystem scanning is outside
  core

Why: public `prune` can be deleted only after Neo automatically prevents
unbounded storage without asking novices to perform maintenance.

Steps:

1. Reuse `RetentionSubject`, `RetentionPolicy`, and `preview_mark_sweep`.
2. Move reusable subject collection, byte measurement, path containment,
   explicit-directory deletion, and parent sync out of the CLI adapter into the
   existing retention owner.
3. Use constants: trigger at 90% actual global workflow storage, reclaim to 80%,
   minimum age 30 days.
4. Trigger a bounded pass at app startup, after a terminal workflow whose
   transcript summary is persisted, and immediately before final denial for
   global workflow storage.
5. Eligibility is terminal + unreferenced + unpinned + old enough. Active,
   queued, Pausing, paused, AwaitingUser, younger, or referenced runs are never
   deleted.
6. Order by oldest terminal timestamp then run ID. Revalidate eligibility and
   containment immediately before each one-directory deletion.
7. Delete one explicit run directory at a time and sync its parent through
   existing platform-safe helpers. Failure skips only that target.
8. Never execute tests against real `~/.neo`; every destructive test uses a
   dedicated temporary root with sentinel files outside candidate directories.
9. If protected data alone exceeds the limit, preserve it and return the
   approved plain-language storage-full explanation.
10. Report only reclaimed count and bytes in ordinary logs; no maintenance UI.

Verification:

```bash
rtk cargo test --package neo-agent-core --test workflow_admission -- automatic_retention_reclaims_only_old_terminal_unreferenced_runs_to_low_watermark --exact --nocapture --include-ignored
rtk cargo test --package neo-agent-core --test workflow_admission -- automatic_retention_preserves_protected_runs_and_fails_closed_on_path_escape --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --bin neo -- modes::workflow::tests::automatic_retention_uses_temp_root_and_reports_reclaimed_count --exact --nocapture --include-ignored
```

Commit: `feat(workflow): automate safe run retention`

## Task 9: Replace The Headless CLI With Four Real Commands

Files:

- `crates/neo-agent/src/cli.rs`
- `crates/neo-agent/src/main.rs`
- `crates/neo-agent/src/modes/workflow.rs`
- `crates/neo-agent/src/modes/run/mod.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs` (stale guidance deletion)
- `crates/neo-agent/tests/{cli_commands,workflow_cli}.rs`
- `crates/neo-agent/Cargo.toml` only for an already-present PTY dev dependency

Why: the current public command family exposes backend mechanics, and its main
`run` path does not execute real Lua or handle input in one terminal.

Steps:

1. Make `WorkflowCommand` contain exactly `List`, `Run`, `Check`, and `Test`.
2. Implement exact grammar:

```text
neo workflow list [--json]
neo workflow run <name> [--args <JSON_OBJECT> | --args-file <PATH>]
                         [--output text|json|jsonl]
neo workflow check <name-or-path> [--json]
neo workflow test <name-or-path> --case <fixture-path> [--json]
```

3. Delete old parser variants/flags and every handler/helper used only by them.
   Do not hide or alias them.
4. `list` uses effective trusted registry only, sorts by display name/name, and
   presents name/display name/purpose. Machine output contains only stable
   automation fields and no absolute storage path.
5. Keep `check` and `test` side-effect free. `test` remains fixture-only and
   never calls live providers/tools.
6. Delete `ensure_headless_runner`. Extract the smallest reusable preparation
   helper from `modes/run/mod.rs` so CLI uses the existing model/provider/tool
   registry, dispatch resolver, `bind_workflow_runtime`, coordinator, and Lua
   runner.
7. Do not create a CLI-specific ToolRegistry, runtime, state machine, or polling
   state.
8. Stream JSONL as durable projections arrive and flush every record. Required
   record kinds: started, step, child-summary, awaiting-user, terminal.
9. Text output shows macro progress/result/files/failure without machine IDs by
   default. JSON emits one terminal or awaiting-user document.
10. If both stdin and stdout are TTY and the run awaits a human, render the typed
    prompt on stderr, validate the answer, submit through the current handle, and
    continue in the same process.
11. If non-interactive, emit one structured awaiting-user result and exit `3`;
    leave the run durable. Never guess/default/restore public answer CLI.
12. Return exact exits `0/1/2/3/4/130`. Use a narrow workflow command outcome or
    typed error; do not remap every Neo command.
13. Ctrl+C requests runtime stop, waits for acknowledgement, then exits 130.
    Platform-specific signal code must be cfg-isolated with portable behavior.
14. Remove stale assistant-facing guidance that directs users/models to retired
    CLI commands.

Verification:

```bash
rtk cargo test --package neo-agent --test workflow_cli -- workflow_run_executes_real_lua_and_returns_actual_result --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_cli -- workflow_run_non_tty_streams_events_and_returns_exact_exit_codes --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_cli -- workflow_run_tty_answers_human_request_in_same_process --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_cli -- workflow_run_ctrl_c_stops_owned_run_before_exit_130 --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test cli_commands -- workflow_cli_exposes_exactly_four_commands_and_plain_language_list --exact --nocapture --include-ignored
rtk cargo test --package neo-agent --test workflow_cli -- workflow_check_and_test_are_deterministic_side_effect_free_and_actionable --exact --nocapture --include-ignored
```

Commit: `feat(cli): replace workflow command surface`

## Task 10: Run Integrated And Native Acceptance

Files:

- tests only when an uncovered approved behavior lacks a runnable check;
- `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/{20-checkpoint,90-evidence}.md`

Why: local focused proof cannot establish native terminal/process behavior on
Windows, Linux, and macOS.

Steps:

1. Run each prior task's exact regressions from a clean build state as needed;
   do not replace them with broad test commands.
2. Run `cargo fmt --all --check` only after touched Rust files are formatted.
3. Run `git diff --check` and a scoped stale-path scan covering active crates,
   current EN/ZH docs, and README.
4. Run a black-box model session for custom authoring:

```text
user intent -> Skill(create-workflow) when needed -> Workflow(run_inline)
            -> automatic completion
```

   The first Workflow business action must be allowed to run directly; no
   source, Cargo, CLI, slash prerequisite, or explicit validation is required.
5. Run a known saved workflow directly with `run_saved` and no skill activation.
6. Run explicit check-only validation and prove zero durable side effects.
7. Exercise `/tasks` with direct delegate + swarm + multiple steps + typed human
   input + pause/resume/stop + inline Save.
8. Exercise 1,000-child and 10,000-child deterministic cases in one scale test
   and verify stable paging without total cap or full-frame materialization.
9. Verify existing transcript cards are logically unchanged.
10. Obtain native macOS, Linux, and Windows proof for CLI TTY/non-TTY, Ctrl+C,
    Operator key/mouse/layout, and V3 create/replay/path safety.
11. Follow project VM rules: check memory, use only one VM at a time, and shut it
    down after use. Never treat a cross target build as native execution.
12. Record command, platform, commit, exit, and key assertion in `90-evidence`.
    Do not claim CI or another platform passed from local evidence.

Review gates:

- contract review after Tasks 1-2;
- durability review after Tasks 3-5;
- UX review after Tasks 6-7;
- CLI/storage review after Tasks 8-9;
- independent final code review before Task 11.

Commit: `test(workflow): prove redesigned product surfaces`

## Task 11: Publish User Docs, ADR, Baseline, And Final Evidence

Files:

- `docs/{en,zh}/guides/{workflows,interaction}.md`
- `docs/{en,zh}/reference/{tools,slash-commands}.md`
- `docs/{en,zh}/configuration/{config-files,data-locations}.md`
- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md` (use
  actual landing date if different)
- `docs/aegis/work/2026-07-28-workflow-product-surface-redesign/{20-checkpoint,90-evidence,99-reflection}.md`
- `docs/aegis/INDEX.md`

Why: product behavior must be understandable without source access, and the
landed architecture decision must supersede only the affected current contract.

Precondition: Tasks 1-10 are complete and native evidence is recorded. Do not
create the new ADR/baseline early.

Steps:

1. Document the four CLI commands, same-terminal human input, non-interactive
   exit 3, automatic retention, and stable machine output.
2. Document all seven Workflow actions, self-contained mutation preflight,
   optional check-only validation, skill role, and automatic completion.
3. Document `/tasks` Steps/Agents/Details, smart entry, Needs input, controls,
   responsive behavior, and actual-usage placement.
4. Document V3 writes and V2 read-only compatibility without telling users to
   operate on journals.
5. Keep EN/ZH behavior equivalent.
6. ADR-0008 records CLI four-command surface, seven-action semantics, V3 child
   lifecycle, completion delivery, Operator, automatic retention, and V2 reads.
7. ADR-0008 supersedes only the human/model/operator portions of ADR-0007.
   ADR-0006/0007 remain historical files and are not edited.
8. New baseline cites exact tested commit and evidence. Do not call pending or
   unrun checks passed.
9. Final checkpoint states every task/commit/evidence location and remaining
   risk. Reflection records rejected duplicate owners and retired paths.
10. Update Aegis index for spec, plan, handoff, work records, ADR, and baseline.

Verification:

```bash
rtk git diff --check
rtk rg -n -i 'workflow_evaluation_route|MUST.*validate_inline|start with validate_inline|REQUIRED FIRST ACTION|validate_inline.*then.*run_inline' crates docs/en docs/zh README.md
rtk rg -n -- '--detach|workflow (show|save|answer|fork|prune)' docs/en docs/zh README.md crates/neo-agent-core/src/skills/builtin/create-workflow.md
```

Expected: `git diff --check` exits 0; both active-surface scans return no stale
positive contract. Historical `docs/aegis` files are excluded intentionally.

Commit: `docs(workflow): publish redesigned product surfaces`

## Verification Matrix

| Requirement | Primary proof |
| --- | --- |
| CLI only four commands | Task 9 CLI grammar exact test |
| Real Lua result | Task 9 real-run test |
| Flush-per-event JSONL | Task 9 non-TTY stream test |
| Same-terminal answer | Task 9 PTY test + native proof |
| Exact exit codes | Task 9 exit test |
| Check/test zero side effects | Task 9 side-effect test |
| Automatic protected retention | Task 8 admission/temp-root tests |
| Seven Workflow actions preserved | Task 1 policy/action tests |
| Run/save self-contained | Task 1 mutation tests |
| Skill optional as capability | Task 1 skill/policy tests |
| Completion without polling | Task 2 notification tests |
| Completion includes generated files | Task 5 runtime-owned aggregate test |
| V3 direct/swarm lifecycle | Tasks 3-4 journal tests |
| V2 read-only | Task 3 byte-for-byte no-write test |
| Recovering truth | Task 3 replay test |
| Durable/live merge | Task 5 merge test |
| 10,000 rows/no cap | Task 5 scale test |
| Typed failure/wait/recovery reasons | Tasks 5-6 projection/render tests |
| Smart `/tasks` entry | Task 6 host test |
| Stable selection/layout | Task 6 TUI tests |
| Main roster hides usage | Task 6 TUI test |
| Typed answer shapes | Task 7 form test |
| Pausing truth | Task 7 runtime/operator test |
| Contextual Save | Task 7 host wiring test |
| Transcript unchanged | Task 6 transcript regressions |
| Three native platforms | Task 10 evidence |

## Plan Pressure Test

- Owner/contract/retirement: each new behavior is assigned to an existing
  canonical owner; retired paths have negative checks.
- Architecture integrity/higher-level path: adapters converge on launch,
  runtime, save, answer, control, and retention owners.
- Verification scope: each cross-module boundary has an integration check; TTY
  and cross-platform claims require native evidence.
- Task executability: tasks name exact files, symbols, contracts, commands,
  expected evidence, and commit boundaries.
- Pressure result: `proceed`.

## Plan Self-Review

- Spec coverage: sections 9-29 map to Tasks 1-10 and the verification matrix.
- Capability check: all seven model actions, runtime lineage/fork, save, answer,
  retention, artifacts, and task controls remain through approved owners.
- Placeholder check: no unresolved placeholder markers.
- Dependency check: Task 2 is independently committable; Task 5 alone owns the
  generated-files aggregate and completion enrichment.
- Type consistency: keys, step occurrence, request/page, metadata, and state
  contracts match the approved spec.
- Compatibility check: V1/V2 are read-only; V3 only is written for new runs;
  no migration or alias.
- Complexity check: oversized files receive deletion/wiring only; new
  responsibilities have bounded owner files.
- UI check: ordinary browser and transcript cards remain unchanged; no new page
  command or dashboard.
- Verification check: every task has exact target-scoped commands; destructive
  tests are temp-root-only.
- ADR check: new ADR/baseline are delayed until implementation and native proof.

## Execution Readiness View

- Intent Lock: implement approved spec `bff4931b` without reopening design.
- Scope Fence: Tasks 1-11 only; unrelated defects and `.gitignore` are excluded.
- Baseline Lock: ADR-0006/0007 and 2026-07-26/27 baselines are read-only inputs.
- Approved Behavior: four CLI commands, seven self-contained model actions,
  V3 child facts, Grok-inspired workflow-only Operator, typed input, automatic
  retention, and completion delivery.
- Owner/Contract Constraints: no second owner; TUI never parses or persists
  runtime state; CLI never builds a second runner.
- Compatibility Boundary: V1/V2 read-only, new V3 writer, no old CLI aliases.
- Retirement Boundary: remove old CLI/gate/TUI/writer paths only after their
  approved replacement is proven.
- Task Batches: model (1-2), durability (3-5), TUI (6-7), storage/CLI (8-9),
  acceptance/docs (10-11).
- Test Obligations: exact per-task regressions plus native platform evidence.
- Review Gates: after each batch and before ADR/baseline.
- Drift/Rewind Rules: on failure, fix the canonical owner; do not add fallback,
  duplicate owner, hidden alias, fuzzy match, automatic effect retry, or feature
  reduction. If the plan is wrong, stop and ask the user.
- Evidence Required Before Completion: per-task commits/tests, stale-path scans,
  black-box model traces, 10,000-child proof, and three native platform records.
- Advisory Boundary: this plan guides execution; test evidence and user review
  remain completion authority.

## Risks And Stop Conditions

Stop and ask the user if any of these occur:

- preserving an active external CLI dependency would require an alias;
- V2 recovery would require writing or migrating a V2 journal;
- child identity cannot be established without title matching or a random UUID;
- CLI real execution would require a second ToolRegistry/runtime/model resolver;
- contextual Save cannot use pinned metadata and canonical permission/registry;
- automatic retention cannot prove containment or protected-run exclusion;
- generated files require a queue-local second projection;
- ordinary Task Browser or transcript card behavior would need redesign;
- an approved real capability would have to be removed.

Do not stop for ordinary compile errors, test failures, file-size pressure, or
implementation difficulty. Resolve those within the named canonical owner.

## Completion Definition

The work is complete only when:

1. Tasks 1-11 each have one verified logical commit;
2. exact regressions and retirement scans pass;
3. native macOS/Linux/Windows evidence exists for required terminal/runtime UI
   behavior;
4. fresh black-box traces prove first-call model correctness and automatic
   completion;
5. final independent review finds no P0/P1 issue and no capability regression;
6. ADR-0008, landed baseline, EN/ZH docs, evidence, and reflection truthfully
   match the final code.
