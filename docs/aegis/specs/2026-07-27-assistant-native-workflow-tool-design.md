# Neo Assistant-Native Workflow Tool Design

Date: `2026-07-27`

Status: `approved design; implementation not started`

Decision owner: user-approved replacement for the completed workflow platform's
model-facing launch contract.

ADR signal: `yes`. After implementation and verification, create a new ADR that
supersedes the model-tool, launch-authorization, and bare-slash portions of
ADR-0006. Do not rewrite ADR-0006 or the earlier workflow specs in place.

## 1. Executive Decision

Neo SHALL expose exactly one model-visible top-level workflow tool named
`Workflow`.

The tool SHALL provide these explicit actions:

1. `list`
2. `show`
3. `validate_inline`
4. `validate_saved`
5. `save`
6. `run_inline`
7. `run_saved`

The tool SHALL use a flat input object with an explicit `action` discriminator.
It SHALL NOT expose a deeply nested `oneOf` tree or require the model to select
between multiple workflow tool names.

The existing model-visible `RunWorkflow` tool SHALL be retired without an alias.
The existing `WorkflowCapability` authorization system SHALL be deleted in full.
Neo's ordinary permission system SHALL become the only interactive human
authorization owner for workflow save and launch operations.

`/workflow <name> [JSON_OBJECT]` SHALL remain a host-direct saved-workflow
launcher. Exact bare `/workflow` SHALL become an authoring entry point that
activates `create-workflow` and starts a normal model turn. Neither slash path
may mint hidden model capabilities.

## 2. Why This Spec Exists

The completed P0-P2 workflow platform implemented its approved launch contract
correctly, but a real Neo session demonstrated that the contract itself is
unusable for the intended assistant-driven workflow experience.

The observed session followed this sequence:

1. `create-workflow` was already activated.
2. The model inspected repository code and tests instead of using workflow
   capabilities as a black box.
3. After correction, the model invoked `neo workflow ...` through Bash because
   the skill explicitly taught CLI validation and launch.
4. After a second correction, the model directly called `RunWorkflow`.
5. `RunWorkflow` rejected the call with:

   ```text
   RunWorkflow requires a launch capability. Use the exact /workflow slash command first.
   ```

This was not random model disobedience:

- the skill described CLI commands as the canonical validate/run path;
- its discovery description covered create/author/write, but not the complete
  use/run/test/evaluate intent surface;
- `RunWorkflow` was only half an entry point because a user-only slash command
  had to mint a one-shot nonce first;
- Neo's ordinary permission system already provided typed
  Launch/Revise/Cancel approval, so the nonce was duplicate authorization;
- the tool failure instructed the model to perform a UI action that model tools
  cannot perform.

The defect therefore stops at the product and architecture contract layer:
Neo defined workflow launch as human-command-first instead of assistant-native.

## 3. Baseline Role Alignment

### 3.1 Product / requirement baseline

The corrected product requirement is:

> When a user asks Neo to create, save, discover, validate, run, use, test, or
> evaluate a workflow, the top-level assistant must be able to complete the
> requested workflow lifecycle using first-party model tools without requiring
> the user to run a slash command or allowing the model to invoke Neo's own CLI
> through Bash.

### 3.2 Architecture / runtime boundary baseline

The durable runtime portions of the implemented workflow platform remain valid:

- `WorkflowRuntime` remains the sole durable run lifecycle, journal, replay,
  recovery, result, and actual-usage owner.
- `WorkflowDefinitionRegistry` remains the sole trusted reusable-definition
  resolution and save owner.
- `WorkflowLaunchCoordinator` remains the shared launch sequencing owner.
- `BackgroundTaskManager` remains the operator projection/control adapter.
- Task tools remain the canonical running-task control and output surface.
- child effects remain independently authorized.

### 3.3 Alignment result

Result: `Design Defect`

Scope: `requirements and architecture`

Required response: create a new superseding design and later a new ADR/baseline
record. Do not patch implementation around the defective capability contract,
and do not rewrite the old historical documents.

## 4. Goals

The implementation SHALL achieve all of the following:

1. A top-level model can discover, inspect, validate, save, and launch workflows
   without CLI or slash prerequisites.
2. Natural-language workflow intent reliably routes to `create-workflow` and/or
   `Workflow` on the first business action.
3. The model has one workflow tool name to remember.
4. Every action has a small, explicit, deterministic host contract.
5. Ask/Auto/Yolo semantics remain coherent with all other Neo tools.
6. Existing reusable definition files and durable workflow runs remain readable
   without migration.
7. Named slash launch remains zero-model-turn and host-direct.
8. The retired capability system leaves no compatibility branch, alias, hidden
   state, or concurrent one-shot lock behind.
9. Removing the capability gate does not accidentally enable recursive workflow
   launch by child agents.
10. The user-visible acceptance test reproduces the original black-box request,
    not a source-aware or test-aware substitute.

## 5. Non-Goals

This design does not:

- replace Lua with Rhai;
- add a second script engine or engine abstraction;
- change durable journal formats, artifact formats, run directories, or saved
  definition pair formats;
- add hosted workflow services, marketplaces, or remote collaboration;
- add predictive token, cost, duration, or child-count governance;
- add an arbitrary total child cap;
- add workflow deletion or rename actions;
- duplicate `TaskOutput`, `TaskPause`, `TaskResume`, `TaskStop`, `TaskAnswer`, or
  `TaskList` inside the `Workflow` tool;
- intentionally add nested workflow launch;
- make model behavior depend on repository source inspection;
- remove the human/headless workflow CLI;
- modify old specs, plans, handoffs, ADRs, or baselines in place.

Lua versus Rhai is a separate capability decision. An engine rewrite cannot
repair assistant routing or duplicate launch authorization and therefore must
not be used as a substitute for this work.

## 6. First-Principles Invariants

### 6.1 Assistant-native completion

For every supported workflow intent, the top-level assistant SHALL have an
executable path from intent to terminal result using registered model tools.
No required step may be available only to the user, slash parser, or shell.

### 6.2 One model-visible workflow owner

The model SHALL see one canonical workflow tool: `Workflow`.

There SHALL be no model-visible `RunWorkflow`, `SaveWorkflow`,
`RunDynamicWorkflow`, `CheckWorkflow`, or compatibility alias.

### 6.3 Existing internal owners remain authoritative

`Workflow` is an adapter, not a new runtime, registry, scheduler, persistence
owner, permission system, or task manager.

### 6.4 One human authorization owner

`runtime/permission.rs` and the existing typed approval protocol SHALL own all
interactive workflow save/launch decisions. No nonce, capability, grant, bind,
reserve, consume, digest, or slash-minted entitlement may remain.

### 6.5 First-party tool preference

When Neo registers a first-party model tool for an operation, the model SHALL
use that tool instead of invoking the current Neo binary through Bash or
Terminal for the same operation.

For workflow operations, invoking `neo workflow ...` through Bash/Terminal is
allowed only when the user explicitly requests the headless CLI, CLI testing,
or implementation-level debugging.

### 6.6 No accidental recursion

The `Workflow` tool SHALL be present only in the top-level agent registry.
Children SHALL NOT receive it by default.

Nested workflow support, if later desired, requires a separate approved design
covering lineage, cycle detection, admission, cancellation, replay, and
operator presentation. It must not appear as a side effect of deleting the
capability gate.

## 7. User Scenarios

### 7.1 Black-box dynamic evaluation

User intent:

```text
Test my dynamic workflow feature comprehensively in .tmp, use the relevant
skill and tool, evaluate it deeply, and write a report.
```

Required path:

```text
Skill(create-workflow)
-> Workflow(validate_inline)
-> Workflow(run_inline)
-> TaskOutput
-> report
```

Before the first `Workflow` call, the model must not use Bash, Terminal, Read,
Grep, Find, Cargo, `neo workflow ...`, or repository implementation tests.

### 7.2 Create a reusable workflow without running

Required path:

```text
Skill(create-workflow)
-> Workflow(save)
-> ask whether to run now
```

No run or background task may be created.

### 7.3 Create and immediately test

Required path:

```text
Skill(create-workflow)
-> Workflow(save)
-> Workflow(run_saved)
-> TaskOutput
```

No duplicate conversational confirmation is required when the user's original
request already asked to run/test. Ask-mode typed approval remains authoritative.

### 7.4 Run a known saved workflow

Required path:

```text
Workflow(run_saved)
-> TaskOutput
```

The authoring skill is not required when the workflow name and arguments are
already known and no source authoring is needed.

### 7.5 Discover and run a saved workflow

Required path:

```text
Workflow(list)
-> Workflow(show)
-> Workflow(run_saved)
-> TaskOutput
```

### 7.6 Modify Neo workflow implementation

If the user explicitly asks to debug, modify, refactor, or test Neo's workflow
implementation, the authoring route SHALL NOT suppress ordinary repository
diagnosis. Source inspection and focused implementation tests are allowed in
that scenario.

The distinction is typed user intent, not whether the current working directory
happens to be the Neo repository.

## 8. Model-Visible Tool Contract

### 8.1 Name

```text
Workflow
```

### 8.2 Required description content

The tool description SHALL explicitly state all of the following:

- use it to list, show, validate, save, run, use, test, or evaluate Neo
  workflows;
- it is the canonical first-party workflow interface;
- for inline authoring, invoke `create-workflow` first unless it is already
  active;
- do not inspect Neo source, run Cargo, or invoke `neo workflow ...` merely to
  use or black-box test workflow functionality;
- saved and inline execution are directly available and require no slash
  capability;
- running returns a task ID and `TaskOutput` is the next inspection step;
- child tool effects remain independently authorized.

The same core routing instruction SHALL appear in both the `Workflow` tool
description and the `create-workflow` skill. This duplication is intentional:
both model-visible decision surfaces must independently lead to the correct
first call.

### 8.3 Flat input schema

The model-visible input object SHALL use these fields:

| Field | Type | Purpose |
| --- | --- | --- |
| `action` | enum | Required action discriminator. |
| `name` | string | Saved or inline workflow name. |
| `description` | string | Inline/save display description. |
| `phases` | array | Inline/save ordered phase declarations. |
| `script` | string | Exact inline script source. |
| `input_schema` | object | Inline/save input JSON Schema. |
| `output_schema` | object | Inline/save final output JSON Schema. |
| `args` | object | Run arguments; defaults to `{}`. |
| `scope` | enum | `user` or `project`; used by `save`, optional list filter. |
| `replace` | boolean | Explicit saved-definition replacement; default `false`. |
| `cursor` | string | Opaque pagination cursor for `list`. |
| `limit` | integer | Requested list page size, host-clamped. |

The schema SHALL set `additionalProperties: false`.

The schema SHALL remain flat. Do not wrap the definition in a nested `workflow`,
`source`, `request`, or `payload` object.

### 8.4 Action field matrix

| Action | Required fields | Optional fields | Forbidden meaningful fields |
| --- | --- | --- | --- |
| `list` | `action` | `scope`, `cursor`, `limit` | inline definition and run args |
| `show` | `action`, `name` | none | inline definition and run args |
| `validate_inline` | `action`, `name`, `description`, `phases`, `script`, `input_schema`, `output_schema` | none | `scope`, `replace`, run args |
| `validate_saved` | `action`, `name` | none | inline definition, `scope`, `replace`, run args |
| `save` | `action`, `name`, `description`, `phases`, `script`, `input_schema`, `output_schema`, `scope` | `replace` | run args |
| `run_inline` | `action`, `name`, `description`, `phases`, `script`, `input_schema`, `output_schema` | `args` | `scope`, `replace` |
| `run_saved` | `action`, `name` | `args` | inline definition, `scope`, `replace` |

Missing or conflicting fields SHALL fail before permission approval or durable
side effects.

Null values SHALL be treated as absent. Empty strings SHALL not satisfy required
string fields. `args`, `input_schema`, and `output_schema` must be JSON objects.

The host SHALL return the exact expected shape for the selected action in a
structured error. It SHALL NOT guess another action, reinterpret ambiguous
fields, invoke a fallback tool, or automatically call the CLI.

### 8.5 Representative calls

List:

```json
{"action":"list","scope":"project","limit":20}
```

Show:

```json
{"action":"show","name":"review-scope"}
```

Validate inline:

```json
{
  "action":"validate_inline",
  "name":"dynamic-review",
  "description":"Run parallel review and aggregate findings.",
  "phases":[
    {"id":"review","description":"Dispatch reviewers"},
    {"id":"finalize","description":"Return structured findings"}
  ],
  "script":"-- exact canonical engine source",
  "input_schema":{"type":"object"},
  "output_schema":{"type":"object","required":["ok","summary"]}
}
```

Save:

```json
{
  "action":"save",
  "name":"review-scope",
  "description":"Review a requested scope.",
  "phases":[{"id":"review","description":"Run review"}],
  "script":"-- exact canonical engine source",
  "input_schema":{"type":"object"},
  "output_schema":{"type":"object","required":["ok","summary"]},
  "scope":"project",
  "replace":false
}
```

Run saved:

```json
{
  "action":"run_saved",
  "name":"review-scope",
  "args":{"scope":"crates/neo-agent-core"}
}
```

### 8.6 Structured output

Every action SHALL return machine-readable `ToolResult.details` with this
stable envelope:

```json
{
  "ok": true,
  "action": "run_saved",
  "status": "running",
  "workflow": {},
  "validation": null,
  "items": null,
  "task": {},
  "next_actions": []
}
```

Fields not applicable to an action may be null or omitted, but `ok`, `action`,
`status`, and `next_actions` SHALL always exist.

The human-readable text SHALL be concise. The structured details are canonical.

### 8.7 `next_actions`

Successful results SHALL provide concrete structured next actions:

- `validate_inline` -> `run_inline`, `save`
- `validate_saved` -> `run_saved`, `show`
- `save` -> `run_saved`, `show`
- `run_inline` / `run_saved` -> `TaskOutput`
- `list` -> `show`, `run_saved`
- `show` -> `validate_saved`, `run_saved`

Example:

```json
{
  "next_actions":[
    {
      "tool":"TaskOutput",
      "arguments":{"task_id":"workflow-123"},
      "reason":"Inspect the running workflow and collect its final result."
    }
  ]
}
```

`next_actions` are model guidance, not automatic execution or authorization.

### 8.8 Error envelope

Expected action/input/runtime failures SHALL return:

```json
{
  "ok": false,
  "action": "save",
  "status": "error",
  "error": {
    "code": "workflow_input_invalid",
    "message": "...",
    "field": "output_schema",
    "expected_shape": {},
    "side_effect_occurred": false
  },
  "next_actions": []
}
```

Required stable error classes include:

- `workflow_action_invalid`
- `workflow_input_invalid`
- `workflow_definition_invalid`
- `workflow_not_found`
- `workflow_scope_untrusted`
- `workflow_conflict`
- `workflow_validation_failed`
- `workflow_admission_waiting`
- `workflow_launch_failed`
- `workflow_feature_unavailable`
- `workflow_top_level_required`

Errors SHALL describe real recovery actions. They SHALL NOT recommend
`neo workflow ...`, bare `/workflow`, source inspection, or Cargo tests unless
the user explicitly requested those surfaces.

## 9. Action Semantics

### 9.1 `list`

`list` SHALL query the existing trusted definition registry projection.

It SHALL:

- support optional user/project scope filtering;
- preserve registry precedence and trust rules;
- paginate using opaque cursors;
- return name, display metadata, scope, revision, and summary schema metadata;
- never read untrusted project definitions into the result;
- never mutate registry state beyond an existing safe refresh mechanism.

It SHALL NOT become a second registry index.

### 9.2 `show`

`show` SHALL resolve exactly one saved definition through
`WorkflowDefinitionRegistry`.

It SHALL return the resolved manifest metadata, canonical source, schemas,
scope, and revision. The registry's machine-size limits remain authoritative.

### 9.3 `validate_inline`

`validate_inline` SHALL compile and validate an in-memory definition without
writing files, creating a run, registering a task, starting a child, or invoking
external tools.

Validation SHALL use the same canonical compiler/schema/runtime preflight later
used by `save` and `run_inline`.

### 9.4 `validate_saved`

`validate_saved` SHALL resolve the saved definition and perform the same
canonical validation/preflight without creating a run.

### 9.5 `save`

`save` SHALL be a thin adapter over the existing registry save owner.

The host, not the model, SHALL:

- validate names, phases, schemas, and source;
- compute lowercase SHA-256 over the exact source bytes;
- construct the paired manifest and source files;
- enforce trusted project scope;
- reject unsafe links, path escapes, invalid suffixes, and non-regular targets;
- reject conflicts unless `replace: true` was explicitly supplied and approved;
- perform atomic pair replacement using the existing cross-platform durability
  path;
- refresh the registry projection only after durable success;
- guarantee no partial pair on failure.

`save` SHALL never launch a run.

### 9.6 `run_inline`

`run_inline` SHALL validate the inline definition and arguments before approval
or durable creation, then launch through `WorkflowLaunchCoordinator`.

No saved pair is created.

The launch source SHALL identify the model `Workflow(run_inline)` adapter; it
must not claim the source was `/workflow`.

### 9.7 `run_saved`

`run_saved` SHALL resolve the saved definition and validate arguments before
approval or durable creation, then launch through the same coordinator.

The launch source SHALL identify the model `Workflow(run_saved)` adapter.

## 10. Permission Contract

### 10.1 Action matrix

| Action | Plan mode | Ask | Auto | Yolo |
| --- | --- | --- | --- | --- |
| `list` | allow | run | run | run |
| `show` | allow | run | run | run |
| `validate_inline` | allow | run | run | run |
| `validate_saved` | allow | run | run | run |
| `save` | deny mutation | typed save approval | run | run |
| `run_inline` | deny launch | typed launch approval | run | run |
| `run_saved` | deny launch | typed launch approval | run | run |

### 10.2 Save approval

Ask mode SHALL show:

- workflow name and description;
- target scope and exact pair paths;
- create versus replace state;
- phases;
- source line/byte counts and complete inspectable source;
- schemas;
- warning that save does not launch.

Available actions SHALL be `Save`, `Revise`, and `Cancel`.

The approved prepared arguments SHALL be the exact arguments executed.

### 10.3 Launch approval

Ask mode SHALL retain the existing typed workflow launch review:

- name and description;
- phases;
- arguments;
- source and source metrics;
- warning that launch approval authorizes orchestration only and child effects
  remain independently authorized.

Available actions SHALL remain `Launch`, `Revise`, and `Cancel`.

Revise and Cancel SHALL create no run or task.

### 10.4 Capability deletion

Permission preparation SHALL NOT inspect, require, grant, bind, reserve,
consume, unbind, or revoke a workflow capability.

Approval integrity comes from the existing prepared-call and typed approval
protocol. The same parsed prepared arguments flow from validation through
approval to execution.

## 11. Canonical Ownership And Data Flow

### 11.1 Read and validation flow

```text
Model
-> Workflow tool adapter
-> action-specific typed validation
-> WorkflowDefinitionRegistry / canonical definition compiler
-> structured result
```

### 11.2 Save flow

```text
Model
-> Workflow(action=save)
-> typed validation
-> permission preparation
-> typed Save/Revise/Cancel approval when Ask
-> WorkflowDefinitionRegistry::save
-> atomic pair durability
-> registry refresh
-> structured saved result
```

### 11.3 Run flow

```text
Model
-> Workflow(action=run_inline|run_saved)
-> definition and argument validation
-> permission preparation
-> typed Launch/Revise/Cancel approval when Ask
-> WorkflowLaunchCoordinator
-> WorkflowRuntime durable create
-> BackgroundTaskManager registration
-> worker start
-> task_id
-> TaskOutput
```

### 11.4 Adapter parity

The following adapters SHALL converge on the same internal owners:

- model `Workflow` tool;
- named `/workflow <name> [JSON_OBJECT]`;
- headless `neo workflow ...` CLI;
- linked-run runtime internals where already supported.

They may differ in caller UX and permission presentation. They SHALL NOT own
parallel registries, launch state machines, validation logic, persistence, or
results.

## 12. `create-workflow` Skill Contract

### 12.1 Discovery description

The model-visible description SHALL cover all of these intents:

- create
- author
- save
- run
- use
- test
- evaluate
- inspect a Neo workflow
- automate a multi-agent pipeline
- black-box test workflow behavior inside the Neo repository

It SHALL distinguish those intents from explicit requests to modify or debug
Neo's workflow implementation.

### 12.2 Required procedure

The skill SHALL route by user intent:

| User intent | Required procedure |
| --- | --- |
| Create/save only | author -> `Workflow(save)` -> offer run |
| Create and run/test | author -> `Workflow(save)` -> `Workflow(run_saved)` -> `TaskOutput` |
| One-off run/test/evaluate | author -> `Workflow(validate_inline)` -> `Workflow(run_inline)` -> `TaskOutput` |
| Run known saved workflow | `Workflow(run_saved)` |
| Discover saved workflow | `Workflow(list/show)` -> `Workflow(run_saved)` |
| Modify Neo implementation | exit authoring route; use normal repository workflow |

### 12.3 Prohibited instructions

The skill SHALL NOT instruct the model to:

- invoke `neo workflow check/test/run/save/list/show` through Bash;
- compute `source_sha256` manually;
- hand-author the persisted manifest pair when `Workflow(save)` is available;
- require the user to enter `/workflow` before a tool call;
- read workflow implementation source to learn how to use the feature;
- run Cargo tests as a substitute for a requested black-box workflow run;
- claim validation or execution from authored files alone.

The CLI may remain documented in a clearly separated human/headless reference
section, but that section SHALL explicitly say the assistant must use the
registered `Workflow` tool unless the user requested CLI operation.

### 12.4 Done criteria

The skill SHALL report completion only when the requested terminal state is
real:

- create/save: structured `Workflow(save)` success exists;
- validate: structured validation success exists;
- run/test/evaluate: a real task was launched and inspected through
  `TaskOutput`, or a real typed failure is reported;
- no slash capability prerequisite was requested;
- any remaining risks are stated accurately.

## 13. Slash And CLI Contract

### 13.1 Named slash

Exact `/workflow <name> [JSON_OBJECT]` SHALL remain host-direct and perform zero
model calls before workflow execution.

It SHALL:

- resolve through the registry;
- validate arguments;
- use the ordinary permission mode;
- open typed launch approval in Ask;
- launch through the shared coordinator;
- never grant or consume model capability state.

### 13.2 Bare slash

Exact bare `/workflow` SHALL:

1. activate the builtin `create-workflow` skill through the same canonical
   manual skill activation path used by `/skill:create-workflow`;
2. start a normal model turn represented in the transcript;
3. allow the model to ask what the workflow should do or act on accompanying
   context;
4. create no hidden grant, nonce, reservation, or launch entitlement.

Bare slash is optional convenience. Natural-language workflow requests must
work without it.

### 13.3 Headless CLI

The CLI remains available for humans, scripts, and explicit CLI testing. It
SHALL continue to use the same registry, compiler, runtime, and coordinator.

The model SHALL not use the CLI as a fallback when the `Workflow` tool is
registered and functional.

## 14. Child And Nested Workflow Boundary

The implementation SHALL distinguish the root tool registry from child tool
registries.

`Workflow` SHALL be registered in the root/top-level registry only.

It SHALL be absent from:

- ordinary Delegate children;
- DelegateGroup children;
- DelegateSwarm children;
- workflow-spawned child agents;
- schema-repair model tool sets;
- other restricted/control-plane tool sets.

The script-level `neo.tool` deny list SHALL continue denying workflow launch.

This preserves the current no-nested-workflow product boundary while removing
the defective root launch capability.

## 15. Retirement And Anti-Entropy Decision

### 15.1 Classification

Deletion class: `code-retirement` plus internal contract-carrying code.

Source-of-truth data risk: `none`.

Existing saved definitions, run journals, artifacts, lineage, and task records
are not deleted or migrated.

### 15.2 Path

Retirement decision: `delete-first`

No compatibility exception is allowed because the retired capability is an
internal session authorization mechanism with no required external consumer.

### 15.3 Required deletion surface

The implementation plan SHALL locate and remove all of the following:

- `workflow/capability.rs`;
- `WorkflowCapability`;
- `WorkflowCapabilityState`;
- `WorkflowCapabilityStatus`;
- `WorkflowCapabilityReservation`;
- `grant`, `inspect`, `launch_nonce`, `bind`, `reserve`, `consume_bound`,
  `unbind`, `revoke`, and related generation state;
- `LaunchAuthorizationMode`;
- capability fields in launch hosts, runtime config, tool context, local config,
  controllers, and fixtures;
- launch nonce fields from intents and bindings;
- intent digest logic used only for capability binding;
- capability lifecycle hooks during clear, new session, resume, fork, cancel,
  revise, and shutdown;
- bare slash grant behavior and status text;
- `LaunchAuthorizationMissing` and `LaunchAuthorizationMismatch` when no
  independent persisted meaning remains;
- tests asserting grant/bind/reserve/consume behavior;
- the old `RunWorkflow` model tool registration, schema, description, and
  compatibility name.

Source/args/schema integrity checks that remain semantically necessary SHALL be
retained under `InvalidInput`, `InvalidDefinition`, or another accurate
non-authorization error class.

### 15.4 Behaviors preserved

- typed workflow approval;
- definition validation;
- source and args hashing used for durable integrity;
- launch sequencing and rollback;
- registry trust and atomic save;
- durable runtime and recovery;
- TaskOutput and task controls;
- named slash launch;
- CLI operation;
- linked-run lineage and actual usage accounting.

### 15.5 Behaviors retired

- user slash required before model launch;
- one-shot session workflow entitlement;
- global reservation conflicts between independent launches/forks;
- capability-specific error messages;
- capability revoke/unbind semantics in approval paths;
- model CLI fallback taught by the authoring skill;
- manually computed persisted definition hashes by the model.

## 16. Compatibility And Persistence

No persisted workflow migration is required.

The implementation SHALL preserve:

- paired `.lua` plus `.workflow.toml` definition format;
- definition format version;
- source SHA-256 revision framing;
- run directory structure;
- journal event formats;
- run IDs and task IDs;
- V1 read-only linked upgrade behavior;
- artifact store and lineage formats;
- AwaitingUser and task-answer behavior.

The only intentionally incompatible internal surface is the model tool contract:
`RunWorkflow` is removed and `Workflow` becomes canonical. Internal tests,
fixtures, prompts, and tool registries SHALL migrate in the same change. No
alias or dual registration is allowed.

## 17. Transcript And Operator UX

The design SHALL reuse existing workflow and task transcript components.

Required visible behavior:

- `Workflow(list/show/validate_*)` renders as ordinary structured tool calls and
  results;
- `Workflow(save)` approval clearly says Save, not Launch;
- `Workflow(run_*)` uses the existing workflow launch review card;
- successful run results show task ID, running status, automatic notification,
  and TaskOutput next step;
- bare `/workflow` shows skill activation and a normal model turn, never
  "capability granted";
- errors name the failed action and real recovery path;
- no Bash/Terminal card appears solely to operate Neo's own workflow CLI during
  assistant-native use.

Existing Delegate/DelegateGroup/DelegateSwarm card design and transcript
placement are outside scope and SHALL remain unchanged.

## 18. Security And Resource Boundaries

The corrected launch path SHALL preserve all real safety boundaries:

- Ask/Auto/Yolo permission semantics;
- independent authorization of child tool effects;
- trusted project definition loading;
- workspace-contained write behavior;
- symlink/reparse/path escape rejection;
- exact schema validation;
- machine-safety source, schema, journal, artifact, memory, and byte limits;
- actual occupancy admission and scheduler backpressure;
- explicit TaskPause/TaskResume/TaskStop controls;
- no predictive governance or arbitrary total child cap.

Removing capability state SHALL not remove validation, permission approval,
admission, rollback, lineage, or durable recovery.

## 19. Cross-Platform Requirements

All new and modified paths SHALL work on Windows, Linux, and macOS.

The implementation SHALL:

- reuse `Path`/`PathBuf` and the existing atomic registry save path;
- avoid shelling out for hash computation, validation, save, or launch;
- avoid Unix-only permissions, signals, executable-bit assumptions, and path
  separators;
- preserve native Windows replacement/sync behavior;
- keep platform-specific code isolated behind existing `cfg` boundaries.

The absence of Bash/CLI from the model workflow path is itself a cross-platform
improvement.

## 20. Failure And Rollback Semantics

### 20.1 Before approval

Invalid action shapes, definitions, schemas, saved names, or arguments SHALL
return a zero-side-effect structured error. Ask approval must not open.

### 20.2 Save failure

Save failure SHALL leave no partial definition pair and no stale successful
registry projection. Replacement failure SHALL preserve the previous complete
pair.

### 20.3 Launch failure

Launch ordering and rollback SHALL remain:

1. preflight;
2. durable run creation;
3. background task registration;
4. started event;
5. worker start.

Failure before durable create creates nothing. Failure after create but before
worker start follows the existing canonical rollback/failure journaling rules.

### 20.4 Revise and Cancel

Revise returns user feedback to the model and creates no save/run side effect.
Cancel creates no save/run side effect. Neither action manipulates capability
state because capability state no longer exists.

## 21. Verification Strategy

Verification SHALL be scoped and layered. Broad workspace test runs are not
required as evidence.

### 21.1 Tool contract tests

Cover:

- all seven actions accept their canonical shapes;
- missing required fields fail with exact expected shape;
- ambiguous saved/inline fields fail before permission;
- `additionalProperties` is false;
- invalid input creates no file, run, task, or approval;
- structured result and error envelopes remain stable;
- next actions are correct.

### 21.2 Registry integration tests

Cover:

- `save` computes the source hash host-side;
- saved pair resolves immediately through the registry;
- project trust is enforced;
- conflict fails unless `replace` is explicit;
- replacement is atomic;
- failure leaves no partial pair;
- `list` pagination and scope filtering are deterministic;
- `show` returns the resolved trusted revision.

### 21.3 Permission tests

Cover:

- read/validate actions do not ask;
- Ask save presents Save/Revise/Cancel and executes the same prepared payload;
- Ask run presents Launch/Revise/Cancel and executes the same prepared payload;
- Auto/Yolo save and run without a preliminary slash;
- Plan mode permits read/validate and rejects save/run;
- Revise/Cancel create no side effects.

### 21.4 Launch tests

Cover:

- `run_inline` launches without capability;
- `run_saved` launches without capability;
- multiple independent launches do not share one-shot authorization state;
- concurrent linked runs/forks do not share a global capability lock;
- invalid definitions/args fail before durable create;
- existing coordinator ordering and rollback remain correct;
- TaskOutput can collect the final result.

### 21.5 Tool registry tests

Cover:

- root registry contains `Workflow` and not `RunWorkflow`;
- child registries do not contain either workflow launch tool;
- restricted repair/tool sets do not contain `Workflow`;
- model-visible tool description contains the mandatory routing statements.

### 21.6 Skill tests

Cover:

- `create-workflow` remains model-invocable;
- discovery description includes create/save/run/use/test/evaluate;
- body teaches `Workflow` actions;
- body contains no assistant procedure requiring `neo workflow ...`;
- body does not require manual SHA-256;
- done criteria require real structured tool outcomes;
- implementation-debug intent remains distinguishable from black-box use.

### 21.7 Slash tests

Cover:

- named slash remains zero-model-turn and launches without capability;
- named Ask path retains typed approval;
- bare slash activates `create-workflow` and starts a model turn;
- bare slash creates no hidden authorization state;
- `/workflowish` remains outside the command boundary.

### 21.8 Stale-owner scan

The final implementation verification SHALL show no active source references to:

- `WorkflowCapability`
- `WorkflowCapabilityReservation`
- `LaunchAuthorizationMode`
- `launch_nonce`
- capability grant/bind/reserve/consume/revoke wording
- `Use the exact /workflow slash command first`
- model-visible `RunWorkflow`
- skill instructions that call `neo workflow` as the assistant path

Historical docs may retain these terms as evidence of the retired design.

## 22. Model-Level Acceptance

Unit tests are necessary but insufficient. The implementation SHALL include a
black-box session acceptance record.

### 22.1 Primary exact scenario

In a fresh normal Neo session, use the user's exact Chinese request:

```text
请你在.tmp/ 下，去全面测试我的dynamic workflow功能，调用相关 skill 和 tool，深度评测，给我结论和报告
```

Required observable trace:

```text
Skill(create-workflow)
-> Workflow(validate_inline)
-> Workflow(run_inline)
-> TaskOutput
-> final report
```

Before the first `Workflow` call, there must be no Bash, Terminal, Read, Grep,
Find, Cargo, CLI help, source inspection, or repository test call.

The scenario SHALL succeed in three consecutive fresh sessions using the target
default model configuration. Record session IDs and exact tool traces.

### 22.2 Manual-skill scenario

Manually activate `create-workflow`, then submit the same request.

The first business tool SHALL be `Workflow`; the model SHALL not invoke the
skill again or inspect source.

### 22.3 Create-only scenario

```text
创建一个可复用 workflow，但先不要运行。
```

Expected: Skill -> Workflow(save) -> conversational offer to run. No task.

### 22.4 Create-and-test scenario

```text
创建一个 workflow 并立即测试它。
```

Expected: Skill -> Workflow(save) -> Workflow(run_saved) -> TaskOutput.

### 22.5 Implementation scenario

```text
修改 Neo 的 workflow runtime。
```

Expected: normal repository diagnosis. The authoring skill/tool route must not
block source work.

### 22.6 Acceptance threshold

The implementation is not complete if the original primary scenario still:

- starts with repository inspection;
- invokes Neo's CLI through Bash/Terminal;
- asks the user to enter `/workflow`;
- receives a missing capability error;
- authors files but never executes a workflow;
- reports success without TaskOutput evidence.

## 23. Likely Implementation Surface

The implementation plan must confirm exact ownership before editing, but the
expected surface includes:

- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/tools/workflow_tests.rs`
- root and child tool registry construction under
  `crates/neo-agent-core/src/tools/`
- `crates/neo-agent-core/src/runtime/permission.rs`
- `crates/neo-agent-core/src/runtime/tool_arguments.rs`
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- `crates/neo-agent-core/src/runtime/config.rs`
- `crates/neo-agent-core/src/workflow/capability.rs` (delete)
- `crates/neo-agent-core/src/workflow/launch.rs`
- `crates/neo-agent-core/src/workflow/registry.rs`
- `crates/neo-agent-core/src/workflow/runtime.rs`
- `crates/neo-agent-core/src/workflow/mod.rs`
- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- interactive config/controller/session plumbing that currently carries
  `workflow_capability`
- workflow launch, registry, lineage, skill, permission, tool registry, and
  interactive slash tests.

This list is a planning hint, not permission to broaden scope or refactor
unrelated modules.

## 24. Implementation Sequencing Constraints

The later implementation plan SHALL obey these dependency constraints:

1. Define and test the new flat `Workflow` action contract.
2. Reuse existing registry validation/save and launch coordinator owners.
3. Wire action-specific permission preparation and typed save/launch review.
4. Register `Workflow` at root and remove it from child surfaces.
5. Delete capability state and all plumbing in the same canonical transition.
6. Replace old `RunWorkflow` registration without a compatibility period.
7. Update skill routing and bare slash behavior.
8. Update tests and run stale-owner scans.
9. Perform model-level black-box acceptance.
10. Only after implementation proof, create the superseding ADR/baseline update.

There SHALL be no intermediate committed state that permanently retains both
model tools or both authorization owners.

## 25. Acceptance Matrix

| Requirement | Evidence required |
| --- | --- |
| One model tool | Root tool listing contains only `Workflow` for workflow lifecycle. |
| No old alias | `RunWorkflow` absent from active model tool registries. |
| No slash prerequisite | Fresh direct run succeeds in Ask/Auto/Yolo without bare slash. |
| No capability owner | Stale scan and type graph contain no active capability path. |
| Correct save | Host validates, hashes, atomically saves, and resolves the pair. |
| Correct run | Inline and saved actions return task IDs and TaskOutput final results. |
| Correct permission | Typed save/launch approval; Revise/Cancel have zero side effects. |
| Correct skill routing | Black-box workflow request reaches Skill/Workflow before repo tools. |
| No CLI fallback | Primary acceptance trace has no Bash/Terminal CLI operation. |
| Named slash preserved | `/workflow <name>` remains host-direct and zero-model-turn. |
| Bare slash corrected | Activates authoring flow and creates no grant state. |
| Child boundary | Child registries cannot launch workflows. |
| Durable compatibility | Existing definitions and run journals need no migration. |
| Cross-platform | Focused native/path tests cover portable save and launch paths. |
| Real UX proof | Exact original Chinese scenario succeeds three consecutive times. |

## 26. Task Intent Draft

- Outcome: replace the defective human-command-first model workflow contract
  with one assistant-native `Workflow` action tool.
- Goal: a top-level assistant can complete discover/author/validate/save/run/
  inspect without CLI or slash prerequisites.
- Success evidence: targeted contract/runtime/TUI tests plus the exact original
  black-box session trace.
- Stop condition: all acceptance matrix rows have evidence, old owners are
  absent, and no required implementation work remains.
- Non-goals: engine replacement, persistence migration, nested workflows,
  workflow deletion/rename, or Task tool duplication.
- Primary risk: replacing the capability gate without restricting child tool
  registries would accidentally open recursion.

## 27. Baseline Read Set Hint

The implementation plan and implementer SHALL acknowledge at least:

- this design spec;
- the current workflow platform contract baseline as historical/current runtime
  evidence, while treating its capability portions as superseded intent;
- ADR-0006 as the implemented platform record that will later be partially
  superseded;
- the current `RunWorkflow`, capability, permission, launch coordinator,
  registry, skill, slash, and child registry implementations;
- the supplied failed session transcript as the user-visible reproduction.

Old P0-P2 plan and handoff may be used for implementation history. They are not
authority for retaining the defective model launch contract.

## 28. Impact Statement Draft

- Product layer: workflow intent recognition and assistant usability change.
- Model contract: one new canonical tool replaces `RunWorkflow`.
- Permission layer: duplicate capability authorization is removed; typed
  approval remains canonical.
- Registry layer: existing owner is reused for model-driven list/show/save.
- Runtime layer: durable ownership and formats remain unchanged.
- TUI layer: bare slash semantics change; named slash remains.
- Skill layer: authoring becomes tool-native and CLI fallback is prohibited.
- Child layer: root/child registry separation becomes explicit.
- Compatibility: internal model tool break is intentional; persisted data and
  human CLI remain compatible.
- ADR: required after implementation proof, not before.

## 29. Architecture Integrity Lens

- Invariant: every supported workflow intent has a complete top-level model
  path.
- Canonical contract: `Workflow` is the sole model adapter; registry, runtime,
  coordinator, permission protocol, and Task tools retain their existing owner
  responsibilities.
- Responsibility overlap removed: capability authorization, CLI-as-model-path,
  and old `RunWorkflow` tool registration.
- Higher-level simplification: delete the second authorization system instead
  of teaching the model how to acquire it.
- Retirement falsifier: if any external supported consumer requires the exact
  `RunWorkflow` model tool name or session capability API, evidence must be
  presented before implementation. Unknown dependency is not evidence.
- Verdict: proceed with canonical replacement and delete-first retirement.

## 30. Approval Record

The user approved:

- a single unified model-visible `Workflow` tool;
- the seven action set in Section 1;
- flat action-discriminated inputs;
- removal of old capability and `RunWorkflow` model contracts;
- retention of named slash as host-direct;
- bare slash as authoring activation;
- top-level-only workflow launch exposure;
- a new independent spec that does not modify the old scheme's documents.

No product implementation is authorized by this document alone. The next step
after user review is a separate implementation plan and handoff.
