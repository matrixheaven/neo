# Neo Local Workflow Platform Design

## Status

- Date: `2026-07-25`
- Design status: `approved by user; implementation not started`
- Scope: complete P0-P2 expansion of Neo's local workflow product
- Engine decision: Lua is the sole canonical workflow engine
- Architecture review required: yes
- ADR signal: amend or supersede the applicable portions of ADR-0004 only after
  implementation evidence exists

This document is the canonical forward design for the workflow platform. It
supersedes the forward-looking exclusions and limits in
`2026-07-20-runworkflow-dynamic-workflow-design.md` where this document says so.
The implemented runtime baseline dated `2026-07-23` remains the truth about
current behavior until implementation lands and a new baseline is recorded.

## 1. Executive Decision

Neo will evolve its existing durable workflow runtime into a complete local
workflow platform. The platform adds reusable definitions, deterministic named
launch, durable user input, strict structured results, generic canonical tool
dispatch, large heterogeneous swarms, artifacts, linked runs, global resource
admission, bounded output, a useful task dashboard, validation, testing, and
built-in workflows.

The expansion does not replace the working runtime foundations:

1. `WorkflowRuntime` remains the only durable owner of run lifecycle, state,
   journal, replay, recovery, lineage, and aggregate output.
2. `journal.jsonl` remains append-only durable truth for state and effect
   identity. Session events, task handles, indexes, and UI state are projections.
3. Lua remains the only workflow language and executor.
4. External effects run only through Neo's canonical instruction, permission,
   tool, child-agent, shell, and scheduler owners.
5. Recovery never automatically retries an external effect whose completion is
   uncertain.
6. Resource policy uses actual occupancy and actual usage. It does not predict
   token, cost, time, agent count, or task scale.
7. No arbitrary small total child limit exists. Active concurrency can be
   bounded for machine safety; excess work stays queued and inspectable.

## 2. Why the Supplied Comparison Is Not a Baseline

The supplied `.tmp/grok_vs_neo_workflow_report.md` is an idea source, not an
authoritative requirements document. The implementation plan MUST use this
specification and live code, not translate that report line by line.

Material corrections established by source review include:

- Neo already has an exact `/workflow` launch-capability command.
- Neo's workflow token cap is optional and based on observed usage; it is not a
  Grok-style logical agent budget.
- Neo's default swarm concurrency is configurable and is not a total child cap.
- Neo does not currently have child `output_schema`; validation is absent rather
  than delegated to an existing child schema path.
- Neo's production recovery resolver is not bound even though the runtime seam
  exists.
- Neo currently discards the JSON value returned by the Lua runner.
- Neo's journal append is not atomic as a complete record, and a torn final
  record currently prevents useful-prefix recovery.
- Grok's `await_user` is a pause barrier and does not return a user answer.
- Grok's batch replay can repeat completed child work after partial failure.
- Grok's schema repair uses fuzzy JSON extraction and an implicit model turn.
- Grok's `agent_budget` and active-run caps are product limits, not cost safety.
- Rhai does not supply registry, recovery, schema, scheduling, or tool safety by
  itself; these are host-runtime responsibilities.

## 3. Goals

### 3.1 Product goals

- Make named workflows easy to discover, validate, launch, inspect, save, and
  test without a model regenerating their source.
- Allow dynamic model-authored workflows when the user explicitly requests one.
- Support long-running workflows that pause for typed human input and survive
  process exit.
- Support large heterogeneous child work sets with durable per-item progress.
- Make all model calls, retries, child work, tool effects, artifacts, and actual
  usage observable.
- Provide built-in deep-research, code-review, and large-refactor workflows that
  exercise the real platform rather than special-case private APIs.
- Preserve local-only operation and cross-platform behavior.

### 3.2 Correctness goals

- Never duplicate an uncertain external effect during recovery.
- Recover the maximum valid journal prefix after a torn final append without
  tolerating interior corruption.
- Keep launch authorization bound to the exact source and arguments executed.
- Keep large journals and artifacts out of synchronous runtime locks and model
  tool responses.
- Preserve one result owner and one state owner.

### 3.3 Handoff goals

- Give a weaker implementation AI exact types, state transitions, file seams,
  tests, stop conditions, and forbidden shortcuts.
- Decompose implementation into dependency-ordered tasks small enough to review
  and commit independently.

## 4. Non-Goals

This design does not add:

- Rhai or any second script engine;
- an engine abstraction whose only purpose is future engines;
- hosted workflows, cloud synchronization, collaboration, or a marketplace;
- model-supplied machine limits, agent budgets, cost budgets, or time budgets;
- predictive pause, cancellation, degradation, or concurrency decisions;
- automatic retry of Delegate, Swarm, Bash, Terminal, MCP, or generic tool
  effects;
- fuzzy JSON extraction from prose or Markdown fences;
- automatic merge of child worktrees;
- arbitrary filesystem access through the artifact API;
- a second workflow dashboard or state database;
- redesign of Delegate, DelegateGroup, DelegateSwarm, Bash, or Terminal cards;
- an implicit timeout for shell admission or commands;
- compatibility aliases for retired new-run APIs.

## 5. Decision Ledger

The following decisions are closed and MUST NOT be reopened during planning or
implementation without new contradictory evidence and explicit user approval.

| Decision | Canonical choice |
| --- | --- |
| Script engine | Lua only |
| Durable owner | `WorkflowRuntime` only |
| Named slash launch | Host resolves and launches directly; no model round-trip |
| Bare slash launch | Model-authored dynamic definition remains supported |
| Registry precedence | builtin < user < trusted project |
| Same-scope duplicate | deterministic error |
| Definition revision | SHA-256 content hash over canonical definition inputs |
| Generic tool access | canonical `ToolRegistry`, default allowed, minimal explicit deny set |
| Terminal retry | new linked run only |
| Paused resume | same run when source and args are unchanged |
| Human input | first-class durable `AwaitingUser` state |
| Child schema repair | exactly one automatic same-session output-only continuation |
| External effect retry | never automatic |
| Total child count | no arbitrary fixed count limit |
| Active concurrency | host-owned machine-safety admission only |
| Usage governance | actual usage and explicit controls only |
| Final result | one persisted top-level Lua return value |
| UI | extend `/tasks`; preserve existing transcript cards |

## 6. Alternatives Considered

### 6.1 Selected: strengthen the Rust host around Lua

All missing capabilities are implemented as typed Rust contracts around the
existing Lua runner. This preserves the proven async host bridge and total VM
memory limit while fixing the actual weak points.

### 6.2 Rejected: replace Lua with Rhai

Rhai is viable but not materially stronger for Neo's requirements. The Grok
features under comparison are Rust host APIs. A language rewrite would consume
effort without solving journal recovery, output paging, authorization binding,
global admission, or result ownership.

### 6.3 Rejected: support both engines

Dual engines would duplicate validation, persistence versioning, recovery,
documentation, built-ins, and testing. It is permanently higher entropy and is
forbidden.

## 7. Canonical Ownership

| Concern | Canonical owner | Explicitly not an owner |
| --- | --- | --- |
| Run lifecycle and state | `WorkflowRuntime` | task manager, TUI, session JSONL |
| Journal and replay | `WorkflowRuntime` journal component | child transcript, UI cache |
| Definition discovery | `WorkflowDefinitionRegistry` | runtime journal, skill store |
| Launch normalization | `WorkflowLaunchCoordinator` | slash handler, CLI handler, tool |
| Launch capability | session workflow capability owner | model-visible token, dialog |
| Tool lookup | `ToolRegistry` | workflow-local registry |
| Tool permission | existing permission owner | Lua, launch approval |
| Applicable instructions | existing instruction owner | Lua source, registry |
| Child lifecycle | existing multi-agent runtime | workflow journal projection |
| Shell process lifecycle | `ShellRuntime` | Lua worker |
| Active resource permits | workflow admission component | script input, model budget |
| Artifact bytes | workflow artifact component | arbitrary workspace paths |
| Artifact references/state | workflow journal | artifact directory scan |
| User answer validation | `WorkflowRuntime` control path | TUI dialog state |
| Human display | `/tasks` and transcript projections | durable state |

New components are bounded modules, not alternate state owners. The registry
owns definitions, not runs. The admission component owns permits, not lifecycle.
The artifact component owns immutable bytes, while journal records own whether
an artifact belongs to a run.

## 8. High-Level Data Flow

```text
definition source
  -> WorkflowDefinitionRegistry or dynamic RunWorkflow adapter
  -> ResolvedWorkflowDefinition
  -> compile + manifest/schema validation
  -> WorkflowLaunchIntent
  -> exact capability/approval match
  -> WorkflowLaunchCoordinator
  -> WorkflowRuntime durable creation
  -> Queued or Running
  -> Lua replay/live execution
       -> journaled host invocation
       -> canonical dispatch owner
       -> journaled outcome/usage/reference
  -> final result record
  -> terminal state
  -> TaskOutput and /tasks projections
```

No arrow may skip the launch coordinator or canonical dispatch owners.

## 9. Core Identity Types

The implementation MUST use typed wrappers rather than raw interchangeable
strings for the following identities:

```text
WorkflowRunId        durable UUID for one run
WorkflowHandle       human-readable session-local handle, for example review-2
WorkflowName         validated registry name
WorkflowRevision     lowercase SHA-256 content hash
WorkflowInvocationId deterministic host-call identity
WorkflowRequestId    durable AwaitingUser request identity
WorkflowArtifactId   SHA-256 content identity plus run association
WorkflowCheckpoint   run ID + durable journal sequence boundary
```

The UUID remains the canonical machine identity. A human handle is stored in
`run.json`, is stable after creation, and is never used as a journal key.

Names MUST match the exact portable grammar
`[a-z0-9][a-z0-9_-]{0,63}`. Resolution is case-sensitive. Unicode display names
may live in metadata but do not become path components or lookup keys.

## 10. Definition Model

### 10.1 Canonical resolved definition

Every source adapter MUST produce one `ResolvedWorkflowDefinition` equivalent
to:

```text
name
display_name
description
ordered phases
input_schema (optional JSON Schema)
output_schema (required JSON Schema)
Lua source bytes
source origin: builtin | user | project | dynamic
source locator for display only
content revision
definition format version
```

Runtime creation accepts only this resolved form. It does not rescan files,
re-resolve precedence, or infer metadata.

### 10.2 File-backed definition shape

A file-backed definition consists of two same-stem files:

```text
<name>.lua
<name>.workflow.toml
```

The TOML manifest is the structured metadata owner. It contains the definition
format version, display name, description, phases, exact `source_sha256`, and
an optional input JSON Schema and required output JSON Schema. The filename
stem is the canonical lookup name; a conflicting manifest name is rejected
rather than aliased.

Using structured TOML avoids executing arbitrary top-level Lua to discover
metadata and avoids ad-hoc parsing of comments. Built-ins compile the same
manifest and source representation into the binary.

The content revision is SHA-256 over the exact byte framing:

```text
ASCII bytes "neo-workflow-definition-v2\0"
|| manifest_length as unsigned 64-bit big-endian
|| canonical_manifest_json
|| source_length as unsigned 64-bit big-endian
|| exact Lua source bytes
```

`canonical_manifest_json` is produced from the fully typed manifest after TOML
decode. JSON object keys, including keys inside embedded schemas, are sorted by
UTF-8 byte order; arrays retain order; compact JSON contains no insignificant
whitespace. The manifest includes the verified lowercase `source_sha256`.

Path, mtime, registry scope, and current precedence are not hash inputs.

### 10.3 Dynamic definitions

The model-facing `RunWorkflow` input remains a strict structured definition
adapter. It must include the same metadata and source fields needed to build a
resolved definition, including a final `output_schema`. Unknown fields are
rejected. It does not accept runtime limits, concurrency, budgets, output
parsing modes, or execution backends.

Dynamic definitions use origin `dynamic` and receive the same content hash,
compile, schema, authorization, and durable snapshot treatment as saved ones.

## 11. WorkflowDefinitionRegistry

### 11.1 Scopes and precedence

Registry scopes are exactly:

```text
builtin
$NEO_HOME/workflows
<trusted-workspace>/.neo/workflows
```

Effective precedence is `builtin < user < trusted project`.

- A higher-scope definition with the same name shadows the lower scope.
- Two candidates with the same name in one scope make that name invalid.
- Invalid higher-scope content does not silently fall back to a lower scope.
- Disabled or untrusted project discovery produces no project candidates.
- There are no extra search directories in this design.

### 11.2 Trust and path safety

Project discovery and project save MUST reuse Neo's existing workspace trust
decision. The registry must not invent a second trust file or prompt.

Discovery and save MUST:

- use `Path` and `PathBuf`;
- reject symlink/reparse-point definition files and parent escapes;
- avoid following directory links;
- accept only regular files with exact expected suffixes;
- enforce manifest and source byte limits before allocation;
- report all same-scope conflicts deterministically;
- never execute Lua during directory scanning.

### 11.3 Refresh and caching

The registry may cache parsed definitions as a projection keyed by scope path,
file identity, size, and mtime. A cache miss or invalidation always recomputes
the content hash. Cache data is rebuildable and cannot authorize launch.

A run always uses its pinned source snapshot. Editing, deleting, or shadowing a
registry definition never changes an existing run or resume.

### 11.4 Save semantics

`save` validates before writing. The default is no-clobber:

- absent name: write source and manifest through atomic temporary files;
- identical existing content: succeed idempotently;
- different existing content: fail and require explicit `--force`;
- project target: require current workspace trust;
- partial pair write: fail closed through manifest/source hash mismatch without
  exposing a half-definition as valid.

Save writes and syncs the source temporary file, replaces the source, then
writes and replaces the manifest last. Discovery accepts a pair only when the
manifest's `source_sha256` matches the exact source bytes. A crash may make the
definition temporarily invalid, but can never make mismatched source and
metadata launchable. The existing validated registry cache cannot authorize a
new run after a file-change notification or explicit refresh.

The registry does not maintain a hidden revision database. The content hash is
the real revision, each run snapshots its definition, and project history may
be managed by Git. This avoids a second persistence system.

## 12. WorkflowLaunchIntent and Coordinator

### 12.1 Exact launch intent

All launch adapters produce a single immutable intent:

```text
session identity
workspace identity
launch nonce
launch source
resolved definition including exact source snapshot
definition revision
canonical arguments
arguments SHA-256
permission mode at launch
optional lineage source and checkpoint
```

The coordinator is stateless normalization/orchestration code. It validates the
intent and calls canonical owners in order, but it does not write `run.json` or
journal records, own rollback state, retain capability state, register task
state, or retain admission permits.

`WorkflowRuntime` performs the complete durable create transaction and rollback
for `run.json` plus initial journal state. The capability owner consumes exact
authorization only after runtime creation succeeds. `BackgroundTaskManager`
registers the returned handle, and the admission component receives the durable
run identity. No adapter may implement a partial copy of that sequence.

### 12.2 Named slash launch

```text
/workflow <name> [JSON_OBJECT]
```

- resolves through the registry;
- requires the remainder, when present, to be one complete JSON object;
- validates arguments against `input_schema` before creation;
- launches directly through the host coordinator;
- does not ask a model to regenerate or reinterpret the workflow.

In Ask permission mode, the existing launch-review protocol displays manifest,
origin, revision, canonical arguments, and complete source. In Auto/Yolo, the
explicit slash action is still required but no second approval dialog is added.

### 12.3 Bare slash dynamic launch

```text
/workflow
```

The bare command grants one session-scoped dynamic-authoring capability. The
model may then call `RunWorkflow` once with a complete definition. The host
validates and binds that proposal before durable creation.

Ask mode shows the complete review. Auto/Yolo does not add an Ask dialog, but
the capability still binds to the exact validated source and arguments before
creation. A changed proposal cannot reuse the bound intent.

### 12.4 Headless launch

`neo workflow run` is an explicit human launch action and does not need an
interactive slash capability. It still constructs the same intent and follows
the configured permission policy for child effects.

Headless execution supports machine-readable output and may either wait for a
terminal result or return after durable start using an explicit flag. There is
no hidden daemon or hosted coordinator.

## 13. Capability and Approval Binding

The existing generation/status-only capability is insufficient. The bound
authorization record MUST include:

```text
session identity
workspace identity
launch nonce
definition revision
source SHA-256
arguments SHA-256
lineage source/checkpoint when present
authorization actor and permission mode
```

The coordinator compares every field immediately before durable creation.
There is no wall-clock TTL. Capability lifetime is event-based and owned by the
session capability state machine:

```text
Unavailable
-> Available { generation, launch_nonce }
-> Bound { generation, launch_nonce, intent_digest }
-> Consumed | Revoked
```

- exact bare `/workflow` creates `Available`;
- a validated dynamic proposal or named command creates `Bound`;
- Ask `Revise` returns the same generation to `Available` and discards the old
  binding;
- Ask `Cancel`, `/new`, session-owner teardown, or process exit revokes it;
- successful durable creation consumes it;
- any generation, nonce, session, workspace, source, args, or lineage mismatch
  rejects launch without changing the valid state unless the user cancels.

Capability state is not durable. Process restart always returns to
`Unavailable`.

Capability consumption occurs only after immutable metadata, initial journal
state, and task registration are durable/successful. Input, compile, schema,
review-revise, or storage-reservation failure does not consume it.

Launch approval authorizes orchestration only. Every later child or tool effect
still passes through the ordinary instruction and permission path.

## 14. Preflight and Validation Before Creation

The coordinator MUST complete these steps before creating a run:

1. strict definition and manifest decoding;
2. portable name and phase validation;
3. source, manifest, args, and schema byte-limit checks;
4. Lua compilation without execution;
5. input and output JSON Schema compilation;
6. input argument validation;
7. registry trust and source-origin validation;
8. content and argument hashing;
9. capability/approval binding;
10. minimum disk and journal-tail reservation.

A syntax-invalid or schema-invalid definition never creates a failed run. It is
a launch error and leaves reusable authorization available when safe.

## 15. Workflow States

Canonical V2 states are:

```text
queued
running
awaiting_user
paused
completed
failed
cancelled
resource_limited
```

`completed`, `failed`, `cancelled`, and `resource_limited` are terminal and
immutable.

Allowed transitions are:

```text
create -> queued
queued -> running | paused | cancelled | resource_limited
running -> awaiting_user | paused | completed | failed | cancelled
        | resource_limited
awaiting_user -> queued | cancelled | resource_limited
paused -> queued | cancelled
terminal -> no transition
```

Rehydration never starts a worker. A persisted `running` or `queued` run is
reconciled and projected as `paused(reason=host_exit)`; only an explicit human
or model resume transitions it to `queued`. `awaiting_user` remains
`awaiting_user` across restart and still requires its typed answer. Active
external effects are never preempted into a queue transition.

An unchanged paused run resumes under the same ID. Answering an
`awaiting_user` request resumes the same run. A terminal run, changed source,
changed args, changed definition revision, or changed machine limit starts a
new linked run.

## 16. Durable Run Layout

Each V2 run uses:

```text
<session_dir>/workflows/<run_id>/
  run.json
  journal.jsonl
  artifacts/
    <content-derived files>
  recovery-quarantine/
    <sha256>.tail
```

`run.json` is immutable and includes:

- format and journal versions;
- run ID and stable human handle;
- optional parent run and checkpoint identity;
- link reason;
- exact resolved definition metadata and Lua source;
- definition origin and revision;
- canonical args and args hash;
- launch actor/source/time/workspace/session information;
- configured machine-safety values captured for diagnosis.

Machine-safety snapshots are descriptive. Live admission remains owned by the
current runtime and is not model-controlled.

The journal owns state transitions, invocation identity and outcome, user
requests and answers, artifact commits, final result, usage, and recovery
actions. Artifact files contain immutable payload bytes referenced by journal
records; an unreferenced file is not run state.

## 17. Journal V2 Contract

### 17.1 Required record families

V2 includes typed records for:

- run creation and state changes;
- invocation started and finished;
- swarm item queued, started, and finished;
- schema-repair started and finished;
- user-input requested and answered;
- artifact committed;
- final result recorded;
- lineage seed imported;
- recovery action applied.

Every record contains format version, contiguous sequence, timestamp, run ID,
and a typed payload. Unknown record kinds are not ignored during recovery.

### 17.2 Effect ordering

For every external effect:

```text
validate and reserve
-> append and sync InvocationStarted
-> execute through canonical owner
-> append and sync InvocationFinished
-> expose completion to Lua
```

The runtime must reserve enough bytes for a compact terminal outcome and a
terminal workflow state before starting the effect.

### 17.3 Payload indirection

Large details, reports, raw schema-attempt output, and final results use a typed
payload reference:

```text
inline JSON value
or
artifact { id, sha256, byte_len, media_type, logical_name }
```

Actual usage, terminal reason, and canonical child/task references MUST remain
available even when verbose payloads move to artifacts.

## 18. Torn-Tail Recovery

Recovery scans once in sequence order and tracks the last validated byte
offset.

1. A malformed newline-terminated record, interior fragment, sequence gap,
   run-ID mismatch, or canonical hash mismatch is corruption and fails closed.
2. A valid final JSON record without its terminating newline is retained,
   normalized by appending the newline, and synced before recovery continues.
3. An invalid non-newline EOF suffix is a torn tail. Before truncation, the exact
   suffix is written and synced to a content-hash-named file under the run's
   recovery quarantine. If quarantine persistence fails, recovery leaves the
   journal untouched and exposes an inspectable recovery error. After the
   quarantine succeeds, the journal is truncated to the last validated offset,
   synced, and a recovery record references the quarantine hash and removed
   byte count.
4. Recovery then reconciles any durable `InvocationStarted` lacking a durable
   finish. It never assumes a truncated finish means the effect failed.

If the durable start itself was in the truncated suffix, the host had not
returned from the sync-before-effect boundary and the effect is treated as not
started. If a start remains but its finish was torn, resolver reconciliation is
required.

Recovery never executes Lua, starts children, requests permission, or opens a
model turn.

## 19. Production Recovery Resolver

The production composition root MUST bind a read-only resolver before workflow
rehydration. The resolver queries the canonical terminal result stores for the
recorded invocation kind.

Lookup identity includes invocation ID, kind, and canonical input hash.

- One matching terminal result: append a canonical finish derived from it.
- No result: append `interrupted(host_exit)`.
- Conflicting or unverifiable result: append a typed recovery conflict and keep
  the run inspectable; do not choose one heuristically.
- Dispatch dependencies unavailable: reconstruction still completes; later
  resume remains queued/paused until dependencies are available.

The resolver may read only. It cannot wait, resume, dispatch, retry, or create a
fallback child.

## 20. Worker Supervision

Every spawned workflow worker MUST be supervised. Panic, cancellation, channel
closure, and ordinary return all release admission permits and clear the
in-memory active marker.

A worker panic cannot leave a run permanently reported as live. The supervisor
must attempt a typed durable failure transition; if journal I/O prevents that,
it publishes the existing unsequenced recovery-failure projection without
claiming durable termination.

## 21. Async I/O and Lock Discipline

Journal scanning, paging, hashing large payloads, artifact I/O, fsync, directory
sync, and large JSON serialization MUST use async filesystem support or a
bounded blocking pool.

The runtime MUST NOT hold a run-state mutex or global runs-map lock while:

- reading or writing files;
- awaiting a child/tool/shell result;
- compiling Lua or JSON Schema;
- serializing a potentially large value;
- waiting for admission.

Lock-protected data is copied into a small immutable snapshot, then expensive
work happens outside the lock, followed by a sequence-checked update.

## 22. Global Admission and Backpressure

The admission component enforces actual machine occupancy, not expected project
size. It controls:

- simultaneously active Lua VMs and reserved VM memory;
- workflow worker CPU permits;
- workflow-owned journal and artifact bytes;
- per-run and global pending record bytes;
- handoff into existing child and shell schedulers.

It does not impose a workflow-lifetime child count, model call count, or total
queued-item count. Serialized input, VM memory, actual disk, and journal limits
remain real physical boundaries.

When a permit is unavailable:

- the run or swarm item remains durable and queued;
- `TaskOutput` and `/tasks` show the admission reason and queue position when
  available;
- no timeout is inferred;
- pause and stop remain available;
- admission uses fair FIFO ordering with starvation prevention.

Permits are released on pause boundaries, terminal transitions, failed worker
start, panic, and process rehydration. Tests must prove no permit leak.

## 23. Storage Limits and Retention

Machine limits are host configuration. Scripts and model tool inputs cannot set
or raise them.

Required limit classes include:

- source and manifest bytes;
- Lua VM total memory and uninterrupted instructions;
- single journal record and total run journal bytes;
- single artifact and total run artifact bytes;
- global workflow storage bytes;
- TaskOutput page bytes;
- active VM and worker permits.

Default retention is non-destructive: terminal runs remain until explicit prune
or an explicit user-configured retention policy. Reaching global storage
admission blocks new durable work with an actionable error; it does not silently
delete runs.

Automatic configured retention may consider only actual terminal age and
actual bytes. It never deletes active, queued, paused, awaiting-user, lineage-
referenced, or pinned runs. Prune is mark-and-sweep over run/journal references
with a dry-run preview and explicit confirmation.

Enabling automatic retention is itself scoped deletion authorization for only
terminal, unreferenced runs matching the configured age/byte policy. It is
accepted only from explicit user-global configuration after a first dry-run
preview. Project definitions, workflow source, model calls, and tool inputs
cannot enable or broaden it.

## 24. Compatibility and Anti-Entropy

### 24.1 Internal retirement decision

- Deletion class: contract-carrying code.
- Path: `delete-first` for obsolete internal APIs after their canonical
  replacements pass focused tests.
- New canonical owners: the registry, launch coordinator, and expanded runtime
  contracts in this document.
- No compatibility aliases, dual input schemas, duplicate result fields,
  workflow-local tool registry, or second engine are retained for new runs.

### 24.2 Persistent artifact boundary

Existing run and session files are persistent user data and MUST NOT be deleted
or silently rewritten. New writes use only V2. V1 run metadata and journals are
rehydrated into inspectable read-only task projections; the V1 writer is not
retained.

Attempting to resume a nonterminal V1 run returns a typed
`linked_upgrade_required` result. The user may explicitly create a linked V2 run
from the latest valid V1 checkpoint. Creation snapshots the original Lua source
and args, imports the verified completed invocation prefix through the V2
lineage-seed contract, and requires current launch authorization. It never
repeats an incomplete V1 external effect.

The old V1 run remains immutable and inspectable. There is no in-place rewrite,
automatic migration, V1 append path, second runtime owner, or second engine.
The normal same-ID paused-resume rule applies to V2 runs only.

Historical session projections remain readable. They never synthesize durable
run state.

### 24.3 Old design sections superseded

This design supersedes the old specification's:

- prohibition on saved workflow definitions;
- fixed eight-child validation boundary;
- prohibition on generic `neo.tool`;
- six-state model without `queued` or `awaiting_user`;
- unbound launch-capability shape;
- complete-journal TaskOutput behavior;
- discarded/ambiguous final Lua return behavior.

ADR-0004's owner, replay, no-effect-retry, canonical dispatch, and projection
decisions remain mandatory.

## 25. Lua Runtime Contract

### 25.1 Engine and sandbox

The runtime uses `mlua` as the sole executor. It retains:

- total VM memory control;
- instruction hooks for stop, pause, and uninterrupted-work limits;
- no standard filesystem, process, network, package, debug, time, random, or
  environment APIs;
- recursively read-only arguments;
- strict typed host functions;
- deterministic JSON conversion.

Lua source cannot load native modules, inspect host pointers, spawn threads, or
obtain a raw `ToolRegistry` reference.

### 25.2 Canonical host API

The canonical API becomes:

| API | Purpose | Durable effect class |
| --- | --- | --- |
| `neo.phase(id)` | select declared phase | local journal |
| `neo.log(message)` | bounded progress log | local journal |
| `neo.delegate(input)` | one canonical child | external child effect |
| `neo.swarm(input)` | large heterogeneous child batch | external child effects |
| `neo.tool(input)` | invoke eligible canonical tool | external tool effect |
| `neo.await_user(input)` | durable typed business input | control checkpoint |
| `neo.artifact_put(input)` | store immutable text/JSON | local durable artifact |
| `neo.artifact_get(input)` | read run artifact | local durable read |
| `neo.artifact_list()` | list journal-referenced artifacts | local durable read |
| `neo.json_array(table)` | mark a table as a JSON array | pure local value |
| `neo.json_object(table)` | mark a table as a JSON object | pure local value |
| `neo.verify(condition, message)` | local assertion | local error |
| `neo.verify_command(input)` | canonical Bash verification | external shell effect |
| `neo.report(value)` | bounded non-final report | local journal/artifact |
| `neo.fail(message)` | explicit terminal failure | local terminal control |

There is no `neo.parallel`, recursive workflow call, detached workflow task,
raw shell escape, direct child registry access, or engine-selection API.

`neo.swarm` is the canonical agent fan-out primitive. `neo.tool` calls are
explicit sequential workflow steps unless the invoked canonical tool itself
owns background work. This avoids a second generic parallel-effect replay
system while preserving full registered-tool access.

### 25.3 Canonical host outcome

Effectful calls return one strict immutable Lua table shape:

```text
ok: boolean
status: completed | failed | denied | cancelled | resource_limited
      | interrupted | schema_invalid
summary: string
details: table
actual_usage: optional table
references: optional table of canonical agent/swarm/task/artifact IDs
schema: optional validation/repair summary
```

Unknown result fields are not generated. Large details are represented by
artifact references rather than silently truncated nested values.

Failures normally return typed outcomes so the workflow chooses policy.
`neo.fail` is unconditionally terminal. `neo.verify` and
`neo.verify_command` may raise catchable typed errors as already designed.

## 26. Canonical Final Result

The JSON-serializable top-level Lua return is the only final result owner. Every
V2 definition has a required final `output_schema`.

Execution ordering is:

```text
Lua returns
-> convert exact Lua value to canonical JSON
-> validate final output schema
-> inline or commit as artifact
-> append FinalResultRecorded
-> append Completed state
```

The chunk must return zero or one value. Zero values and one `nil` convert to
JSON `null` and succeed only when the schema allows `null`; multiple return
values fail.

Lua-to-JSON conversion is exact:

- string-key-only tables become objects;
- integer-key-only tables become arrays only when keys are exactly `1..n`;
- mixed-key and sparse tables fail;
- an unmarked empty table becomes an object;
- `neo.json_array({})` is the canonical empty array;
- `neo.json_object({})` is the explicit empty object;
- markers are immutable and reject a table with the wrong key shape;
- cycles, invalid UTF-8, functions, threads, userdata, NaN, infinity, excessive
  depth, and values beyond configured bytes fail deterministically.

`neo.report` remains useful for intermediate or multi-part reports, but it does
not become a fallback final result. `TaskOutput` exposes reports separately from
the canonical result.

A final-result schema failure does not trigger a hidden model call because no
child session owns the Lua return. It records a typed validation failure and
the workflow terminates `failed(schema_invalid_final_result)`.

Crash recovery treats final-result ordering as a commit protocol:

- durable `FinalResultRecorded` plus missing terminal state: validate the record
  and append `Completed`; do not execute Lua again or rewrite the artifact;
- `Completed` without one valid `FinalResultRecorded`: fail closed as journal
  corruption and never synthesize `null` or a report as the result;
- multiple final-result records: fail closed unless later records are explicitly
  typed recovery duplicates with the same payload hash;
- no final-result record and a nonterminal run: ordinary replay may re-execute
  pure Lua while returning every completed host-call outcome from the journal.

## 27. JSON Schema Contract

### 27.1 Standard

Neo supports JSON Schema Draft 2020-12 for workflow input, child output,
awaited user input, and final output. The canonical validator is the Rust
`jsonschema` crate pinned to a release compatible with Neo's MSRV. All runtime,
CLI, and test-harness validation routes through one wrapper around that crate.

Schema compilation happens before launch for definition-level schemas and
before effect dispatch for dynamically supplied child/user schemas.

Rejected or unsupported schema features fail explicitly; they are not ignored.
Remote `$ref` resolution and network fetches are disabled. Internal JSON
Pointer references within the supplied schema document are allowed.

### 27.2 Strict JSON extraction

Child structured output accepts exactly one JSON value from the canonical final
assistant output or provider-native structured-output value.

Neo MUST NOT:

- scan prose for a first `{` or last `}`;
- unwrap Markdown code fences;
- choose one object from multiple values;
- repair quotes, commas, or invalid UTF-8;
- treat tool output as the final structured value.

Provider-native structured output should be used when the selected provider
supports the same schema contract. Host validation remains authoritative.

### 27.3 Validation error shape

Schema errors exposed to Lua and TaskOutput include:

```text
attempt number
instance path
schema path
stable error code
human-readable message
raw-output artifact reference when needed
```

The schema itself is referenced by definition revision or a journaled hash; it
is not duplicated into every outcome.

## 28. Child Structured Output and Corrective Continuation

### 28.1 Delegate input

`neo.delegate` accepts only:

```text
task: required non-empty string
title: optional non-empty string
resume: optional canonical agent ID
role: optional canonical role
model: optional registered model alias
provider: optional compatible provider override
context: optional inherit | summary | none
worktree: optional shared | isolated; default shared for a new child
tool_allow: optional exact tool-name array forming a capability ceiling
output_schema: required JSON Schema
```

Unknown fields are rejected. A child cannot select a permission mode more
permissive than the parent/session policy. Model/provider combinations resolve
through canonical registries.

The input is a strict union:

- new child: `resume` absent; all listed new-child policy fields are allowed;
- resumed child: `resume`, `task`, and `output_schema` only.

A resumed child inherits its original role, model/provider binding, context,
worktree, and capability ceiling. Missing inherited resources fail explicitly;
the runtime never falls back to a new child or shared worktree.

### 28.2 Exactly one repair turn

For every child output:

1. run the child normally through the canonical agent runtime;
2. parse one strict JSON value and validate it;
3. if valid, persist the validated value and finish;
4. if invalid, append `SchemaRepairStarted` before another model call;
5. continue the same child session with a deterministic correction message;
6. disable all tools and external effects for the correction turn;
7. parse and validate one strict JSON value;
8. persist success or terminal `schema_invalid` and append
   `SchemaRepairFinished`.

There is exactly one automatic correction turn. It is an additional model
effect selected explicitly by this design, not a re-execution of the original
child task or any child tool effect. It is visible, journaled before dispatch,
and counted in actual usage. This is the sole automatic model continuation
allowed after a schema failure; it does not weaken the prohibition on
automatically retrying uncertain external effects.

The correction prompt includes the schema validation errors and requests only
the replacement JSON value. It does not include hidden product advice or ask
the child to repeat research or tool work.

If the child attempts a tool call during repair, the host rejects it with
`schema_repair_tool_forbidden` and the repair fails. It does not silently enable
the tool or start another repair turn.

### 28.3 Crash semantics

If the process exits after `SchemaRepairStarted` but before a durable repair
finish, recovery never repeats the corrective model turn. A terminal child
result may be adopted only if the canonical child/session owner proves it.
Otherwise the invocation becomes `interrupted(host_exit)` and the workflow or
user explicitly decides the next action.

Both attempts' actual usage is aggregated. Raw output is stored inline only
when small; otherwise an artifact reference is recorded.

## 29. Durable AwaitingUser

### 29.1 Host API

`neo.await_user` accepts:

```text
prompt: required non-empty string
answer_schema: required JSON Schema
default: optional JSON value that must validate
title: optional short display title
answer_policy: optional human | human_or_model; default human
```

The API does not accept secret/password semantics because answers are persisted
in the local journal. Documentation must tell workflow authors not to request
credentials through this surface.

### 29.2 Request lifecycle

At the host-call boundary:

1. derive deterministic invocation and request IDs;
2. compile the answer schema and validate any default;
3. append and sync `UserInputRequested`;
4. transition `running -> awaiting_user`;
5. release active VM/worker admission while retaining durable run state;
6. return control to the task system without blocking a runtime thread.

The request is visible through `/tasks`, `TaskOutput`, and headless workflow
commands after process restart.

### 29.3 Answer lifecycle

All answer surfaces call one runtime control method equivalent to:

```text
answer(run_id, request_id, JSON value, actor)
```

The method verifies current state, request ID, answer policy, and schema. It
then appends and syncs `UserInputAnswered` before transitioning the run to
`queued`. Admission later returns it to `running`.

Lua replay reaches the same `neo.await_user` call and returns the journaled JSON
answer. The UI is not the answer owner.

Stale, duplicate, wrong-run, wrong-schema, or unauthorized answers are rejected
without changing state. A duplicate identical answer may return an idempotent
success only after proving the same durable record already exists.

`TaskResume` without an answer cannot resume `awaiting_user`. A separate typed
`TaskAnswer` control avoids overloading ordinary resume semantics.

## 30. Generic `neo.tool`

### 30.1 Input and dispatch

`neo.tool` accepts exactly:

```text
name: required exact registered tool name
input: required JSON object
```

The call performs:

```text
strict decode
-> exact ToolRegistry lookup
-> workflow eligibility check
-> durable InvocationStarted
-> instruction preflight
-> permission resolution
-> canonical ToolRegistry execution
-> durable InvocationFinished
-> typed Lua outcome
```

Tool descriptions and schemas remain owned by `ToolRegistry`. The workflow API
does not copy them into another registry.

### 30.2 Default-open eligibility and deny set

Registered tools are eligible by default. One centralized deny decision rejects
the following canonical semantic IDs:

```text
RunWorkflow
Delegate
DelegateSwarm
TaskPause
TaskResume
TaskStop
TaskAnswer
AskUserQuestion
EnterPlanMode
ExitPlanMode
StartGoal
ExitGoalMode
UpdateGoalStatus
GetGoalStatus
Todo
ListDelegates
WaitDelegate
InterruptDelegate
MessageDelegate
```

Dedicated workflow APIs own child creation, user input, and workflow control;
`neo.tool` cannot bypass them. `TaskOutput` is allowed for other tasks but is
rejected when its target is the current workflow run and the call would recurse
through the same output lock/path.

Any future tool registered as a blocking dialog, development-mode transition,
goal/session control, workflow control, or multi-agent control MUST be added to
this same centralized semantic deny classification before release. Ordinary new
tools remain allowed by default.

The deny decision uses canonical tool identity and capability metadata, not
fuzzy name matching. MCP tools remain eligible when their ordinary permission
and instruction contracts allow them.

New ordinary registered tools become eligible without adding a workflow
allowlist entry. Security validation, permission, workspace containment, and
tool-specific schemas remain fully active.

### 30.3 Deadlock and blocking behavior

The workflow cannot wait on or control its own run through `neo.tool`.
Task-output reads of other tasks may be allowed; same-run recursive reads must
use the runtime's local snapshot path or be denied when they would acquire a
conflicting lock.

Shell and Terminal calls preserve their existing admission semantics. Waiting
for shell admission remains pending and does not time out implicitly. A command
without explicit timeout/cancel may run indefinitely.

No generic tool effect is automatically retried after denial, error, timeout,
disconnect, schema error, or host exit.

## 31. Heterogeneous `neo.swarm`

### 31.1 Canonical item shape

The workflow swarm surface uses direct child specifications, each with a
required output schema:

```text
description: required non-empty string
items: required array of child specs

child spec:
  task: required non-empty string
  title: optional non-empty string
  resume: optional canonical agent ID
  role: optional canonical role
  model: optional registered model alias
  provider: optional compatible provider override
  context: optional inherit | summary | none
  worktree: optional shared | isolated; default shared for a new child
  tool_allow: optional exact tool-name capability ceiling
  output_schema: required JSON Schema
```

This direct form is the workflow DSL contract. There is no workflow input field
for `max_concurrency`, total agent budget, token budget, or wall-clock timeout.

The existing model-facing `DelegateSwarm` input schema remains a supported
product adapter. Both adapters lower into one canonical internal
`Vec<ChildPlan>` batch API owned by the multi-agent runtime:

- `DelegateSwarm` expands its current template/items contract into child plans;
- `neo.swarm` builds heterogeneous child plans directly;
- the internal owner performs validation, admission, creation, lifecycle, and
  event emission once;
- the arbitrary eight-child validator is removed at that owner boundary.

This is two authoring adapters over one child lifecycle owner, not two swarm
runtimes. Neither adapter may maintain independent child state or recovery.
The existing card layout and projection are unchanged.

### 31.2 No arbitrary total count cap

The hard-coded `MAX_SWARM_CHILDREN = 8` is deleted. Validation is based on:

- total serialized request bytes;
- per-item field and schema bytes;
- Lua VM memory;
- durable queue/journal capacity;
- actual global storage admission.

There is no fixed 8, 128, 1,024, or similar total child-count ceiling. Real
finite machine resources remain valid boundaries.

### 31.3 Per-item durability

The batch does not use all-or-nothing replay. For every item:

```text
SwarmItemQueued
-> SwarmItemStarted before child dispatch
-> SwarmItemFinished after terminal child result
```

Completed items are never repeated because another item failed or the host
exited. A queued item with no durable start may be admitted later. A started
item without a durable finish uses the production recovery resolver and is
never blindly relaunched.

The Lua call returns only after all items are terminal or the workflow itself is
stopped/resource-limited. Results preserve input order while progress may
complete out of order.

### 31.4 Pause and stop

- Pause prevents new queued item starts and lets active children reach terminal
  outcomes before the workflow enters `paused`.
- Stop cancels active children through the canonical agent owner and records
  queued items as cancelled without dispatch.
- Admission pressure leaves items queued; it is not a timeout or failure.

## 32. Per-Child Isolation and Capability Ceiling

### 32.1 Context policy

- `inherit`: canonical inherited session/workspace context.
- `summary`: host-generated bounded summary using existing context owners.
- `none`: only task, role, applicable instructions, and tool contract.

The workflow does not inject arbitrary hidden system prompts outside existing
instruction ownership.

### 32.2 Capability ceiling

`tool_allow` may only reduce the child's available tools. Effective tools are
the intersection of:

```text
registered tools
session/provider capability
parent workflow eligibility
child tool_allow when present
ordinary permission and instruction decisions
```

No child field can elevate Ask to Auto/Yolo, bypass workspace containment,
enable a disabled MCP server, or restore a denied workflow-control tool.

### 32.3 Worktree policy

- `shared`: child uses the current canonical workspace.
- `isolated`: child receives a dedicated worktree/workspace through an existing
  or new single worktree manager, never by ad-hoc shell commands.

Isolation requires a supported repository/workspace. Unsupported cases return a
typed error before child start. Paths and cleanup state are recorded in child
metadata and workflow provenance.

Neo does not auto-merge isolated worktrees and does not delete a dirty or
unreviewed worktree. Cleanup is explicit or governed by a separately confirmed
safe retention policy.

## 33. Run-Scoped Artifact Store

### 33.1 Supported values

The workflow artifact API stores only:

- UTF-8 text with an explicit media type; or
- canonical JSON values.

Logical names use a portable identifier grammar and never become filesystem
paths. Binary files and arbitrary workspace reads/writes remain tool concerns.

### 33.2 Put contract

`neo.artifact_put` accepts:

```text
name: required logical name
kind: text | json
value: required matching value
media_type: optional valid media type; defaults by kind
```

It returns artifact ID, version, SHA-256, bytes, media type, and logical name.
Writing the same name creates a new immutable version; it does not mutate old
bytes.

### 33.3 Atomicity and integrity

Artifact commit ordering is:

```text
serialize canonical bytes
-> enforce limits
-> write same-directory temporary file
-> sync and validate hash/size
-> atomic rename/replace into content-derived location
-> sync directory where supported
-> append ArtifactCommitted journal record
```

A crash after file commit but before the journal record creates an orphan, not a
visible artifact. Orphan collection uses a grace period and journal mark/sweep.

Reads revalidate expected size and digest. Missing or corrupt bytes return a
typed error and never become empty content.

### 33.4 Get and list

`neo.artifact_get` selects by exact artifact ID or logical name plus version.
`neo.artifact_list` returns bounded metadata only. Large artifact content is
read with a byte range/cursor through TaskOutput or the artifact API and never
inserted whole into a tool result by default.

## 34. Workflow Checkpoints and Linked Runs

### 34.1 Checkpoint identity

Every durable host-call boundary with no incomplete invocation is an eligible
checkpoint. A checkpoint is identified by source run ID, journal sequence, and
prefix digest.

The UI and CLI may give checkpoints human labels, but the durable identity is
the verified sequence/digest pair.

### 34.2 Linked-run creation

Terminal runs are immutable. A new run is required for:

- retrying Completed, Failed, Cancelled, or ResourceLimited work;
- changing definition source or revision;
- changing arguments;
- raising machine limits;
- explicit fork from an earlier checkpoint.

The new `run.json` records parent run ID, parent checkpoint, link reason, and
the new exact launch intent. A fresh launch authorization is required.

### 34.3 Seed import

To avoid a runtime dependency on mutable/prunable parent files, linked-run
creation copies the selected completed invocation prefix into typed
`LineageSeedImported` records in the new journal. Referenced artifacts are
copied or content-addressed with verified hashes.

Imported outcomes retain their original child/task references and source run
identity. They are excluded from new-run actual-usage totals but displayed as
inherited usage in lineage summaries.

On execution, Lua replays from the beginning against the imported prefix.

- Any mismatch before the seed is fully consumed fails `lineage_mismatch`
  without starting a new external effect.
- After the seed is consumed, new host calls execute normally under the new run
  identity.

This makes an edited fork explicit and prevents partially matching new source
from silently branching before the selected checkpoint.

## 35. TaskOutput Contract

### 35.1 No full-journal result

Workflow TaskOutput MUST never synchronously load or serialize the complete
journal. `max_output_bytes` applies to the complete returned `ToolResult`,
including text and structured details.

The tool supports explicit views:

```text
summary
journal
result
artifacts
artifact_content
```

Each non-summary view accepts a stable cursor and byte limit. Unknown fields or
unsupported view/task-kind combinations are rejected rather than ignored.

### 35.2 Summary view

The bounded summary includes:

- run ID and human handle;
- name, origin, revision, lineage, and state;
- current phase and admission/wait reason;
- started/queued/terminal child counts;
- invocation and failure counts;
- actual usage split into current and inherited;
- pending user request metadata;
- final result or artifact reference;
- latest bounded reports and artifact metadata;
- next cursors for more journal/artifact data.

It does not embed full child transcripts, full shell output, or complete large
artifacts.

### 35.3 Journal paging

Journal pages are ascending contiguous sequence summaries. A cursor binds at
least run ID, view, next sequence, and query/filter hash. Responses report:

```text
first_seq
last_seq
has_more
next_cursor
returned_bytes
```

No record is silently cut in the middle. If one record cannot fit the requested
limit, the result returns its metadata and an artifact/payload reference or an
explicit minimum-size error.

Paging and artifact reads occur outside runtime locks through async or bounded
blocking I/O.

## 36. Workflow Provenance

Every child approval, tool approval, tool event, child event, and task
projection originating from a workflow carries a typed execution origin:

```text
run_id
human_handle
definition name and revision
phase ID
invocation ID
optional swarm item ID
```

Canonical approval and event structures own this field. TUI code does not parse
`inv_*` strings or infer provenance from card adjacency.

Provenance is metadata only. It must not duplicate full workflow state into
every child record.

## 37. CLI and Slash Command Product Surface

### 37.1 Slash commands

```text
/workflow
/workflow <name> [JSON_OBJECT]
```

Bare and named forms have the distinct behavior defined above. Existing exact
slash parsing remains the only TUI path that creates launch authority.

### 37.2 Headless commands

The canonical command family is:

```text
neo workflow list [--scope builtin|user|project|effective] [--output text|json]
neo workflow show <name> [--scope ...] [--output text|json]
neo workflow check <name-or-path> [--output text|json]
neo workflow test <name-or-path> --case <fixture> [--output text|json]
neo workflow run <name> [--args-json <object> | --args-file <path>]
                  [--detach] [--output text|json|jsonl]
neo workflow save <run-id-or-path> --scope user|project
                  [--name <name>] [--force]
neo workflow answer <run-id-or-handle> <request-id>
                    (--json <value> | --file <path>)
neo workflow fork <run-id-or-handle> --checkpoint <seq>
                  [--name <name>] [--args-json <object> | --args-file <path>]
neo workflow prune [--older-than <duration>] [--max-bytes <bytes>]
                   [--dry-run] [--yes]
```

Rules:

- `list/show/check/test` are read-only.
- `run` waits for terminal state by default; `--detach` returns after durable
  creation.
- args source flags are mutually exclusive.
- `save` is no-clobber unless `--force` is explicit.
- `prune` defaults to dry-run; actual deletion requires `--yes` and may delete
  terminal unreferenced data only.
- output modes have stable schemas suitable for scripting.
- commands route through core registry/runtime APIs, not a CLI-only owner.

## 38. `/tasks` Workflow Dashboard

The current task browser is extended with a workflow filter and richer workflow
detail. It remains a projection over `BackgroundTaskManager` and
`WorkflowRuntime` snapshots.

### 38.1 List behavior

- stable sort by most recently updated, then created, then run ID;
- pagination rather than arbitrary truncation at 50;
- filters for state, definition name, source scope, awaiting input, and lineage;
- human handles such as `deep-research`, `deep-research-2`;
- concise columns for phase, child progress, queue reason, actual usage, and
  elapsed time.

### 38.2 Detail behavior

The detail view exposes:

- manifest and pinned revision;
- source origin and launch actor;
- lineage and checkpoint information;
- phase rail and journal-derived steps;
- queued/running/terminal children and per-child provenance;
- schema-repair attempt status;
- pending typed user request and answer action;
- actual usage;
- reports, final result, and artifact list;
- paged journal/output access;
- pause, resume, answer, stop, fork, and prune-safe actions when valid.

`WorkflowSnapshot.steps` must be populated from journal/child references rather
than remaining an empty vector.

### 38.3 Card preservation

Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal card layout,
ordering, expansion, progress semantics, and output previews are byte-for-byte
out of scope. Workflow provenance may be shown by the surrounding approval or
task-browser chrome without rewriting those cards.

The existing workflow transcript card remains an orchestration summary. The
dashboard does not pull child card content into it.

## 39. User-Facing Validator and Test Harness

### 39.1 `check`

Validation performs no external effects. It checks:

- manifest and source pairing;
- name/path/scope rules;
- Lua syntax;
- phase uniqueness;
- input/output schema compilation;
- source, schema, and manifest limits;
- forbidden static API names when detectable;
- builtin manifest consistency;
- content revision calculation.

Static analysis is advisory where Lua names are dynamic. Runtime enforcement
remains authoritative.

### 39.2 `test`

The deterministic harness runs the real Lua host contract against fixture-owned
fake outcomes. A fixture may provide:

```text
args
delegate outcomes and optional schema-repair outputs
swarm item outcomes
generic tool outcomes
await_user answers
expected final result
expected reports/artifacts
expected invocation trace
```

Tests use `FakeModelClient`, fake tool dispatch, temporary run storage, and the
real journal/replay/schema code. They do not contact providers or execute shell
commands by default.

Live provider/tool execution is outside the V2 test harness. The harness has no
flag that silently converts a deterministic fixture test into live execution.

## 40. Built-In Workflows

Built-ins are ordinary immutable registry definitions using public workflow
APIs. They receive no privileged host functions.

### 40.1 Deep research

Required behavior:

- validate question and desired output;
- create a research plan artifact;
- launch heterogeneous research children;
- require structured findings with source/evidence fields;
- run contradiction/gap verification;
- optionally await user clarification;
- synthesize and schema-validate a final report;
- preserve research artifacts and actual usage.

### 40.2 Code review

Required behavior:

- accept scope and review criteria;
- dispatch independent review domains with read-only capability ceilings;
- require structured findings with severity, path, line, evidence, and test gap;
- deduplicate and challenge weak findings;
- return findings-first final output;
- never modify code.

### 40.3 Large refactor

Required behavior:

- capture approved spec/plan inputs;
- partition independent implementation slices;
- default mutation-capable children to isolated worktrees when available;
- preserve per-slice verification and review results as artifacts;
- await explicit human decisions at merge/retirement boundaries;
- never auto-merge or delete worktrees;
- return lineage, commits, verification, and unresolved risks.

Built-ins must be tested through the same public deterministic harness.

## 41. Security Contract

- Local-only; no network service or workflow marketplace.
- Registry project scope is trust-gated.
- Definition files and artifact paths reject symlink/reparse escapes.
- Lua has no direct system APIs.
- Generic tools retain their original schemas, containment, permission, and
  instruction checks.
- Launch approval cannot authorize later effects.
- Child capability settings only reduce authority.
- Awaited answers are persisted locally and must not request secrets.
- Prune is explicit, previewable, and restricted to terminal unreferenced data.
- Source, args, schemas, outputs, and artifacts have byte limits before large
  allocation or serialization.
- No fuzzy parsing or exact-name fallback is allowed at security boundaries.

## 42. Cross-Platform Contract

- All paths use `Path`/`PathBuf`; no hard-coded separators.
- Registry, run, artifact, temp, and worktree handling must work on Windows,
  Linux, and macOS.
- Symlink and Windows reparse-point behavior must be tested separately.
- File replace/sync code uses existing helpers or explicit `cfg` isolation with
  a portable default.
- No bare `sh -c`, Unix signal, executable-bit, inode, or advisory-lock
  assumption is allowed in portable paths.
- Tests use temporary directories, explicit cwd, isolated env, and no fixed
  ports.
- Shell/Terminal semantics remain owned by their existing cross-platform
  runtime.

## 43. Error Taxonomy

Implementation must expose typed stable categories including:

```text
invalid_definition
definition_conflict
definition_not_found
untrusted_project_definition
invalid_manifest
lua_compile_failed
invalid_schema
input_schema_invalid
launch_authorization_missing
launch_authorization_mismatch
storage_admission_denied
journal_corrupt
journal_torn_tail_recovered
recovery_conflict
interrupted_host_exit
tool_not_workflow_eligible
permission_denied
instruction_replan_required
schema_invalid
schema_repair_tool_forbidden
awaiting_user
invalid_user_answer
stale_user_request
lineage_mismatch
artifact_missing
artifact_corrupt
resource_limited
worker_panicked
```

Stable codes are separate from human messages. String parsing is not used to
route control flow.

## 44. Observability

Every run summary reports actual values only:

- actual provider input/output/cache/reasoning usage when supplied;
- actual child counts and states;
- actual schema repair attempts;
- actual journal/artifact bytes;
- actual active and queued durations;
- actual tool/permission failures;
- actual lineage inheritance.

Neo does not estimate remaining tokens, cost, agents, completion time, or task
size for automatic governance. UI may display elapsed time and measured queue
position without turning either into a timeout.

## 45. P0 Delivery Requirements

P0 is the correctness and usable-product foundation. All items are mandatory:

1. recover valid journal prefix after torn final append;
2. page and cap workflow TaskOutput and remove blocking full-journal reads;
3. bind production terminal-result recovery resolver;
4. supervise worker lifecycle and release stuck active markers/permits;
5. add global VM/storage/executor admission and non-destructive retention;
6. remove hard-coded swarm child count and add per-item durable queue state;
7. support heterogeneous child specifications;
8. add builtin/user/trusted-project definition registry;
9. add direct named slash and headless launch through one coordinator;
10. bind authorization to exact definition revision/source/args/lineage;
11. precompile and validate before durable creation;
12. extend `/tasks` with paged workflow list and useful detail;
13. require and compile final output schemas, implement exact Lua-to-JSON
    conversion, and persist the top-level return as canonical final result;
14. provide the minimal immutable artifact indirection needed for oversized
    final results/reports and a paged `result` TaskOutput view;
15. provide safe `neo workflow prune` dry-run/explicit-delete controls so global
    storage admission has an operator release path;
16. keep V1 runs read-only and provide the explicit verified linked-V2 upgrade
    path without retaining a V1 writer.

P0 completion means the platform is safe to use for large durable workflows;
it does not require the three built-ins to be feature-complete.

## 46. P1 Delivery Requirements

P1 adds expressive workflow authoring:

1. strict child and final output schemas;
2. exactly one visible output-only child repair continuation;
3. first-class durable `AwaitingUser` and typed answer controls;
4. generic `neo.tool` through canonical ToolRegistry and deny set;
5. per-child context, model/provider, worktree, and capability ceiling;
6. run-scoped immutable text/JSON artifacts;
7. workflow provenance on approvals and events;
8. linked-run fork/recovery from explicit checkpoints;
9. schema, repair, user-input, artifact, lineage, and final-result journal
   records;
10. complete TaskOutput views for results, artifacts, and journal pages.

## 47. P2 Delivery Requirements

P2 completes author and operator experience:

1. list/show/check/test/run/save/answer/fork commands and the complete prune UX;
2. deterministic workflow fixture harness;
3. deep-research builtin;
4. code-review builtin;
5. large-refactor builtin;
6. human handles, stable sorting, filters, and pagination;
7. improved output summaries and artifact navigation;
8. English and Chinese user/author documentation.

## 48. Acceptance Matrix

### 48.1 Registry and launch

- builtin/user/project precedence resolves deterministically;
- same-scope conflict invalidates the name;
- invalid higher-scope content never falls back silently;
- untrusted project definitions are absent and cannot be saved;
- symlink/reparse and path escapes are rejected;
- hash is stable across path and mtime changes;
- definition-revision golden vectors match on Windows, Linux, and macOS;
- manifest/schema object-key reordering preserves revision while field-boundary
  collision counterexamples produce different revisions;
- existing run resumes pinned source after registry edit/delete/shadow;
- named slash launch performs zero model calls before workflow execution;
- bare slash cannot be forged by ordinary text or tool input;
- any source/args/session/lineage hash mismatch creates no run;
- compile/schema/storage failure creates no run and does not consume reusable
  authorization;
- capability state has no TTL, is invalidated by process exit/session teardown,
  preserves its generation on Ask revise, and is consumed exactly once after
  durable creation;
- all launch adapters reach the same coordinator tests.

### 48.2 Journal and recovery

- crash before write, mid-JSON, after JSON before newline, after newline before
  sync, and after sync before memory update are fault-injected;
- valid unterminated JSON final record is normalized safely;
- invalid EOF suffix is truncated and valid prefix survives;
- truncated suffix bytes survive in a hash-addressed recovery quarantine and
  are referenced by the recovery record;
- quarantine write failure leaves the original journal byte-for-byte intact;
- malformed interior and newline-terminated invalid JSON fail closed;
- sequence/hash/run-ID mismatch fails closed;
- started effect is executed at most once in every fault case;
- production resolver adopts one proven terminal child result;
- unknown or conflicting result is never retried;
- worker panic cannot leave `worker_active=true` forever;
- rehydrate starts no worker; persisted Running/Queued become
  `Paused(host_exit)` until explicit resume;
- crash after `FinalResultRecorded` and before `Completed` appends only the
  terminal state and never reruns Lua;
- `Completed` without a valid final-result record fails closed;
- every terminal run has terminal outcomes for all started children.

### 48.2.1 V1 compatibility fixtures

- real V1 metadata/journal fixtures rehydrate as read-only projections;
- same-ID V1 resume returns `linked_upgrade_required` and appends no V1 bytes;
- explicit linked V2 upgrade imports the valid completed prefix and artifacts;
- unresolved V1 started effects are never repeated;
- source V1 files remain byte-for-byte unchanged after inspection and upgrade.

### 48.3 Output and I/O

- logical multi-gigabyte journal fixtures page under a strict ToolResult cap;
- summary never serializes the full journal or artifacts;
- cursor is stable, query-bound, and rejects wrong run/view/filter;
- `has_more`, next cursor, and returned bytes are accurate;
- slow journal/artifact I/O under single-thread Tokio does not block another
  run's heartbeat, pause, stop, or TaskOutput summary;
- no runtime mutex is held across blocking I/O or external awaits.

### 48.4 Admission and scale

- many runs and children queue rather than fail from a count constant;
- active permits never exceed configured machine limits;
- queued run/item can pause and stop;
- admission resumes fairly after permit release;
- panic, failed start, pause, terminal state, and rehydrate leak no permits;
- global storage reservations are race-safe;
- no model/tool input can raise a host limit;
- no predicted token/cost/agent/time rule changes state.
- reaching global storage admission blocks new durable work, confirmed prune of
  terminal unreferenced runs releases actual bytes, and admission then succeeds;

### 48.5 Schema

- exact JSON succeeds; prose, fences, multiple values, and malformed JSON fail;
- provider-native value and text fallback share host validation;
- first invalid child output records repair start before continuation;
- repair uses same session with tools disabled;
- attempted repair tool call fails explicitly;
- exactly one repair occurs;
- both attempts' raw output/error/actual usage are observable;
- crash during repair never automatically repeats it;
- invalid final Lua result fails without hidden model repair.

### 48.6 AwaitingUser

- request and state are durable before worker release;
- restart preserves prompt/schema/default/policy;
- human answer validates and replay returns identical JSON;
- human-only request rejects model answer;
- stale, duplicate conflicting, wrong-run, and wrong-schema answers do not
  change state;
- ordinary TaskResume cannot bypass a missing answer;
- stop while awaiting produces a terminal run without losing request history.

### 48.7 Tool and child policy

- eligible builtin and MCP tools dispatch through canonical ToolRegistry;
- recursive workflow, dialog, and self-control tools are denied;
- new ordinary registry tool is eligible without workflow allowlist edit;
- permission and instruction denial return typed outcomes;
- child tool ceiling can reduce but never elevate authority;
- shared and isolated worktree behavior is explicit and cross-platform;
- no external effect is automatically retried.

### 48.8 Swarm

- arrays larger than eight validate when within byte/resource limits;
- heterogeneous child fields reach canonical child creation;
- every item has queued/started/finished durable identity;
- completed item is not replayed after sibling failure or crash;
- unstarted queued item may start after resume;
- started unresolved item becomes adopted terminal or interrupted, not rerun;
- result order follows input order while completion order may differ;
- DelegateSwarm card golden output remains unchanged.

### 48.9 Artifacts and lineage

- text and JSON canonicalization/hash/versioning are deterministic;
- temp-write/rename crash produces either referenced artifact or collectable
  orphan, never a partial visible artifact;
- corrupt/missing artifact fails typed integrity check;
- logical names cannot escape storage paths;
- linked run imports a verified prefix and referenced artifacts;
- prefix mismatch fails before any new effect;
- terminal source never changes state;
- inherited usage is displayed but not charged to new-run actual usage.

### 48.10 UX and CLI

- `/tasks` pages beyond 50 and sorts deterministically;
- handle allocation is stable across restart;
- filters produce query-bound cursors;
- awaiting-user answer, pause, resume, stop, and fork actions call canonical
  control APIs;
- headless JSON/JSONL schemas are stable;
- save is no-clobber and project trust-gated;
- prune is dry-run by default and cannot delete referenced/nonterminal runs;
- workflow card and Delegate/Group/Swarm/Bash/Terminal card golden output is
  unchanged.

### 48.11 Cross-platform

- registry, journal, artifact, prune, and worktree tests run on Windows, Linux,
  and macOS;
- platform-specific path/link semantics have native tests;
- no test depends on ambient cwd/env, fixed ports, Unix-only signals, or path
  separators.

## 49. Exact Implementation Seams

The implementation plan must verify live names/locations before editing, but
the current expected seams are:

| Area | Primary current seam |
| --- | --- |
| Runtime/state/output | `crates/neo-agent-core/src/workflow/runtime.rs`, `state.rs` |
| Journal/recovery | `crates/neo-agent-core/src/workflow/journal.rs` |
| Lua host | `crates/neo-agent-core/src/workflow/lua.rs` |
| Limits | `crates/neo-agent-core/src/workflow/limits.rs` |
| Workflow tool adapter | `crates/neo-agent-core/src/tools/workflow.rs` |
| TaskOutput adapter | `crates/neo-agent-core/src/tools/background_tasks.rs` |
| Production dispatch | `crates/neo-agent-core/src/runtime/workflow_dispatch.rs` |
| Canonical tool registry | `crates/neo-agent-core/src/tools/` and runtime registry owner |
| Delegate/Swarm | `crates/neo-agent-core/src/tools/delegate.rs` |
| Approval provenance | `crates/neo-agent-core/src/approval.rs` and event envelopes |
| Slash commands | `crates/neo-agent/src/modes/interactive/slash_commands.rs` |
| CLI | `crates/neo-agent/src/cli.rs`, `main.rs` |
| Task browser adapter | `crates/neo-agent/src/modes/task_browser.rs` |
| Task browser view | `crates/neo-tui/src/tasks_browser/` |
| Workflow transcript card | `crates/neo-tui/src/transcript/workflow_card.rs` |
| Focused tests | `crates/neo-agent-core/tests/workflow_*.rs` and exact TUI/CLI targets |

Candidate bounded core module boundaries are:

```text
workflow/registry.rs
workflow/launch.rs
workflow/admission.rs
workflow/artifacts.rs
workflow/schema.rs
workflow/lineage.rs
```

These filenames are advisory, not required architecture. The plan must inspect
current file pressure and extract only when a boundary removes real complexity.
It must not create one-implementation traits or factories merely to match this
conceptual list.

## 50. Documentation Requirements

Implementation must update English and Chinese documentation together for:

- definition file locations and manifest format;
- trust and precedence;
- slash and CLI commands;
- Lua API and strict schemas;
- schema correction behavior and actual usage;
- AwaitingUser durability and non-secret warning;
- swarm scale/backpressure semantics;
- artifacts and linked runs;
- TaskOutput pagination;
- retention/prune safety;
- machine limits versus product budgets.

The 2026-07-23 runtime baseline and ADR-0004 are updated only after the
corresponding implementation and evidence land. A proposal must not be written
as current behavior.

## 51. Plan Constraints

The implementation plan derived from this spec MUST:

- order work by durable substrate before product surfaces;
- give each task exact files, types, tests, commands, and stop conditions;
- use narrow package/target/test filters required by `AGENTS.md`;
- require one logical commit per verified task;
- keep subagents Git-mutation-free unless the root explicitly owns staging and
  commit;
- forbid checkout/restore/reset/stash/clean/rebase/rm/amend and unrelated fixes;
- stop on persistent-state ambiguity, effect-retry ambiguity, or a proposed
  second owner;
- include residue scans for retired constants, APIs, aliases, and duplicate
  result shapes;
- distinguish focused local evidence from CI and native-platform evidence.

## 52. Requirement Ready Check

- Requirement source refs: user decisions, live Neo implementation, ADR-0004,
  2026-07-23 runtime baseline, corrected Grok comparison.
- Goals and scope refs: Sections 1-5.
- User/scenario refs: named launch, dynamic launch, long-running human input,
  large swarm, headless automation, built-in workflows.
- Requirement item refs: Sections 10-47.
- Acceptance refs: Section 48.
- Open blocker questions: none.
- Decision: `ready`.

## 53. Baseline Role Alignment

- Product/Requirement Baseline: this approved forward design.
- Architecture/Runtime Boundary Baseline: ADR-0004 and the implemented
  2026-07-23 runtime contract.
- Result: `Design Defect` in the old forward spec where it excluded now-approved
  registry/tool/input/scale capabilities; implemented runtime remains a valid
  substrate with identified correctness gaps.
- Scope: both requirements and architecture.
- Next action: write an implementation plan after user review of this committed
  spec; do not patch product code in the documentation task.

## 54. Existence Checks

| Proposed surface | Existing reuse | Why a new bounded component exists | Decision |
| --- | --- | --- | --- |
| Definition registry | Skill discovery is semantically different | Workflow manifests/source/revision/precedence need a definition owner | add with proof |
| Launch coordinator | Runtime creation exists but adapters duplicate intent | One exact authorization and durable-create path is required | add with proof |
| Admission component | Child/shell schedulers exist | Lua VM and workflow storage are not owned by them | add with proof |
| Artifact component | Workspace tools write files | Run-scoped immutable typed values must not expose arbitrary paths | add with proof |
| Schema component | Provider/tool schemas exist | Child/final/user JSON instance validation needs one host validator | add with proof |
| Second dashboard | Existing `/tasks` | Existing surface can be extended | reject; reuse existing |
| Second engine | Lua exists | No proven capability gap | reject |
| Generic parallel-effect engine | Swarm and canonical tools exist | Adds unsafe duplicate replay machinery | reject |

## 55. Architecture Integrity Lens

- Invariant: one durable run owner and no uncertain effect replay.
- Canonical contract: exact launch intent -> append-only journal -> canonical
  dispatch -> typed outcome -> paged projection.
- Responsibility overlap: explicitly prohibited in Section 7.
- Higher-level simplification: all launch surfaces converge before runtime; all
  tool effects converge at ToolRegistry; all UI reads converge at TaskOutput
  and snapshots.
- Retirement falsifier: any new V2 path still writes old ambiguous results,
  uses `MAX_SWARM_CHILDREN`, reads a complete journal for TaskOutput, or executes
  through a workflow-local registry.
- Verdict: coherent if implementation follows dependency order and deletion
  gates.

## 56. Product Risk Lens

- Value: reusable, inspectable, durable, high-scale local workflows without
  model regeneration or arbitrary agent limits.
- Non-goals: hosted ecosystem, predictive governance, hidden effect retry.
- Trade-offs: more durable record types and product surfaces; one visible schema
  repair model call; explicit manifests alongside Lua source.
- Decision: accepted by user.

## 57. Open Questions

None. Implementation may discover code-level choices, but it must not reinterpret
the closed product and architecture decisions in Section 5. A discovery that
would change an owner, durable format, retry semantic, user-facing command, or
compatibility boundary stops the affected task and requests a spec amendment.
