# Neo Workflow Product Surface Redesign

Date: `2026-07-27`

Status: `approved; implementation plan authorized`

Decision owner: user-directed correction of the landed workflow CLI,
assistant contract, and `/tasks` workflow experience.

Architecture review required: `yes`

ADR signal: `yes`. After implementation and verification, supersede the
human-facing, model-facing, and operator-surface portions of ADR-0007 and the
current workflow product baseline. Do not rewrite historical records.

## 1. Executive Decision

Neo SHALL present Workflow as one powerful product with three deliberately
different surfaces:

1. Humans in an interactive Neo session use `/workflow` to create or launch and
   `/tasks` to understand, answer, pause, resume, stop, and review a run.
2. Automation uses exactly four same-level headless commands:
   `neo workflow list`, `run`, `check`, and `test`.
3. The top-level assistant keeps the complete seven-action `Workflow` tool:
   `list`, `show`, `validate_inline`, `validate_saved`, `save`, `run_inline`,
   and `run_saved`.

The redesign removes user-visible runtime mechanics and mandatory action
choreography. It does not remove useful capability.

The `/tasks` Workflow Operator SHALL learn the useful presentation pattern from
Grok Build's Workflows page: one selected run, a phase rail, a child-agent
roster, stable selection during live updates, automatic following of the
active phase, and direct drill-down. It SHALL apply only to Workflow tasks.
All other task kinds retain the existing Task Browser.

The redesign SHALL NOT copy Grok Build's model-controlled child budget, fixed
agent-row ceiling, slash-command forwarding from the UI, same-process-only
recovery, or missing assistant save/list/show capabilities.

## 2. Why This Spec Exists

The landed workflow platform is durable and functionally broad, but its public
surface exposes implementation mechanics instead of user intent.

### 2.1 Human CLI defect

The current CLI mixes ordinary launch, author validation, durable request IDs,
journal checkpoints, definition scopes, storage reclamation, and linked-run
recovery in one public command family:

```text
list show check test run save answer fork prune
```

The default `run` path cannot provide a coherent human-input journey in the
same terminal. The current headless runner also completes with a synthetic
result instead of executing the workflow script. The defect is therefore not
only visual complexity; the primary journey is incomplete.

### 2.2 Workflow task visibility defect

The current `/tasks` projection exposes only aggregate workflow counts. It
cannot reliably associate every live direct delegate with its workflow run and
phase, and its current answer action sends an empty object instead of rendering
the durable typed request.

### 2.3 Assistant contract defect

The current model contract requires this ritual for an inline run:

```text
Skill(create-workflow)
-> Workflow(validate_inline)
-> Workflow(run_inline)
-> TaskOutput
```

This confuses optional no-side-effect checking with a runtime safety
requirement. It asks an imperfect model to assemble operations that the owning
runtime can perform deterministically.

### 2.4 Product-language defect

Product names and help text must describe an exact user purpose. Mechanical,
ambiguous, or source-aware terminology is forbidden. Users and coding agents
must be able to understand each surface without reading Neo source code.

## 3. Product Principles

These principles are requirements, not commentary:

1. **Intent over mechanics.** Users state the outcome; Neo owns validation,
   identity, persistence, recovery, and routing.
2. **One-flow completion.** A common journey completes in one surface. A human
   must never need a second terminal merely to answer a running workflow.
3. **Progressive disclosure.** The default view shows what is happening, what
   needs attention, and what happened. Technical evidence is available only in
   a details view when it helps an explicit decision.
4. **Justified surface.** Every public command, action, key, field, and pane must
   have a distinct user purpose. Redundant mechanics are deleted.
5. **Black-box clarity.** Product use must not depend on source code, journal
   knowledge, hashes, IDs, or internal architecture.
6. **First-call correctness.** Model tools, skills, and prompts must make the
   correct first business action obvious. Runtime invariants own safety; prompt
   choreography must not pretend to be enforcement.
7. **Capability preservation.** Simplification may combine duplicate routes or
   remove useless behavior, but it must not remove a real use case.

## 4. Confirmed Decision Ledger

The following decisions are closed for this design and MUST NOT be reopened
during planning or implementation without new contradictory evidence and
explicit user approval.

| Concern | Confirmed decision |
| --- | --- |
| Headless command count | Exactly four same-level commands |
| Headless commands | `list`, `run`, `check`, `test` |
| CLI author subgroup | None |
| Model workflow tool count | One top-level `Workflow` tool |
| Model action count | Preserve all seven landed actions |
| Model validation | Explicit validation remains; never mandatory before run/save |
| Inline/saved run | Each action validates internally and launches directly |
| Skill role | Authoring guidance, never a capability or launch license |
| Ordinary task UI | Existing Task Browser unchanged |
| Workflow task UI | Dedicated Operator modeled on Grok's phase/agent page |
| Operator entry | Smart direct entry; no page switch merely from moving selection |
| Main agent rows | State, title, current activity, elapsed time |
| Token usage | Real usage only in agent details, never the main roster |
| Raw IDs/events | Not shown in the normal product UI |
| Operator save | Contextual for inline unsaved runs only |
| Child total | No arbitrary total cap |
| Script engine | Lua only; unchanged by this work |
| Transcript cards | Delegate-family and Workflow cards remain unchanged |

## 5. First-Principles Review

First principle: a person asks Neo to perform and oversee a workflow; Neo must
hide the state-machine assembly required to do so.

Non-negotiables:

- no functional regression;
- no second durable owner;
- no second task system;
- no second terminal for ordinary human input;
- no arbitrary child ceiling;
- no predictive cost or token governance;
- no change to existing Delegate-family transcript card design;
- no implicit retry of uncertain external effects.

Historical assumptions to delete:

- that every durable operation deserves a public CLI command;
- that validation must be a separate model call before every run;
- that exposing a journal sequence makes recovery understandable;
- that a user will maintain workflow storage manually;
- that a dashboard should expose every available backend field.

Smallest sufficient product path:

```text
Human: /workflow or natural language -> /tasks -> answer/result
Script: neo workflow run <name> -> terminal result
Assistant: one relevant Workflow action -> Task tools or completion notice
```

## 6. Existing Owners and New Surface Check

### 6.1 Existence check

```text
Proposed surface: Workflow Operator for selected Workflow tasks
Existing reuse: /tasks, BackgroundTaskManager, WorkflowRuntime, neo-tui
Why current surface is insufficient: aggregate counts cannot show child lineage,
  live work, typed input, or per-phase outcomes
Creation proof: the user must oversee long-running multi-child workflows without
  reading journals or opening another terminal
Entropy impact: replace current workflow detail/actions; do not add a dashboard,
  slash command, runtime, or persistence owner
Decision: add within the existing /tasks ownership boundary
```

### 6.2 Canonical ownership

| Fact or behavior | Canonical owner | Explicitly not an owner |
| --- | --- | --- |
| Run lifecycle, phase, pause, resume, stop, terminal state | `WorkflowRuntime` | TUI, task browser |
| Durable child lifecycle and lineage | workflow journal | live agent snapshot |
| Live activity and live usage | `MultiAgentRuntime` snapshot | journal, TUI |
| Definition resolution and save | `WorkflowDefinitionRegistry` | tool adapter, CLI |
| Launch normalization | `WorkflowLaunchCoordinator` | slash, CLI, model tool |
| Task query and workflow control forwarding | `BackgroundTaskManager` | TUI |
| Human answer validation | `WorkflowRuntime::answer` | dialog state |
| Selection, focus, scrolling, responsive layout | `neo-tui` | runtime |
| Completion delivery to the root session | existing session/background notification path | polling loop |

No new manager, state machine, database, registry, or background-task kind is
created by this design.

## 7. Grok Build: Adopt and Reject

This design learns from live reference code, not only from its screenshot.

### 7.1 Adopt

1. A single model-visible workflow tool name.
2. Run is the default business operation; validation-only behavior is optional.
3. Registered and inline definitions converge before launch.
4. A session-unique human display name is shown while machine IDs stay internal.
5. One run detail uses a phase rail plus a child roster.
6. Active phase follows running children unless the user manually selects a
   different phase.
7. Selection is anchored by stable IDs so live updates do not move the cursor.
8. Live activity is merged into the durable roster instead of persisted on
   every token or tool update.
9. Workflow children do not also appear as unrelated top-level tasks.
10. Completion actively notifies the root session; the model does not poll or
    sleep-wait.

### 7.2 Improve rather than copy

1. Neo keeps assistant `list`, `show`, and `save`; Grok places parts of that
   lifecycle in slash/UI paths.
2. Neo keeps `TaskPause`, `TaskResume`, `TaskStop`, and `TaskAnswer` as the
   canonical model control owners instead of adding resume to `Workflow`.
3. Neo directly calls task-control owners from the Operator; it does not close
   the page and synthesize slash-command text.
4. Neo shows no model-controlled child budget and no predicted cost.
5. Neo has no fixed 256-row or similar total child ceiling.
6. Neo recovers durable terminal child facts after restart and marks unresolved
   live work as recovering.
7. Neo renders typed human input rather than representing it as a generic pause.
8. Neo keeps actual token usage out of the default roster.

## 8. User Journeys

### 8.1 Run a saved workflow in TUI

```text
/workflow code-review {"target":"working-tree"}
-> typed launch approval when required
-> background run starts
-> transcript shows the existing Workflow card
-> /tasks opens the active Workflow Operator
-> completion appears in transcript and Operator
```

### 8.2 Ask Neo to create and run a one-off workflow

```text
User intent
-> assistant activates create-workflow when authoring guidance is needed
-> Workflow(run_inline) performs validation internally
-> typed launch approval when required
-> background task starts
-> completion notification arrives automatically
```

There is no mandatory validation call and no CLI/source/Cargo detour.

### 8.3 Check without running

```text
Workflow(validate_inline | validate_saved)
-> structured validation result
-> zero run, task, file, or permission side effect
```

This is the only reason to use the explicit validation actions.

### 8.4 Human input

```text
workflow enters AwaitingUser
-> /tasks marks it Needs input and gives it first-entry priority
-> Operator opens the typed answer dialog
-> user answers in the same Neo process
-> runtime validates and resumes the same run
```

### 8.5 Headless TTY input

```text
neo workflow run release-check
-> workflow reaches AwaitingUser
-> the same terminal renders the prompt and typed fields
-> answer is validated
-> the same process continues to terminal state
```

### 8.6 Non-interactive automation input

If stdin is not a TTY and a workflow reaches a human-only gate:

1. Neo emits one structured `awaiting_user` result containing the prompt,
   answer shape, default, and durable task handle.
2. Neo exits with code `3`.
3. The durable run remains `AwaitingUser` and may be continued later from an
   interactive Neo `/tasks` surface.
4. Neo does not guess an answer, silently accept a default, or reintroduce a
   public `answer` CLI command.

Automation workflows that require unattended completion must not contain a
human-only gate.

## 9. Headless CLI Contract

### 9.1 Public grammar

The complete public command family SHALL be:

```text
neo workflow list [--json]

neo workflow run <name>
    [--args <JSON_OBJECT> | --args-file <PATH>]
    [--output text|json|jsonl]

neo workflow check <name-or-path> [--json]

neo workflow test <name-or-path> --case <fixture-path> [--json]
```

`neo workflow --help` SHALL list exactly these four commands.

There is no `author` subgroup and no hidden compatibility command.

### 9.2 `list`

Purpose shown to the user:

> Show the workflows available here and what each one does.

Rules:

- Lists the effective trusted registry only.
- Text rows contain name, display name, and plain-language description.
- Scope, revision hash, source hash, locator, and manifest details are absent
  from text output.
- `--json` may include stable machine fields required for automation, but no
  secret or absolute internal storage path.
- Results are sorted by display name, then canonical name.

### 9.3 `run`

Purpose shown to the user:

> Run a saved workflow and wait for its result.

Rules:

- Resolves one saved trusted definition by name.
- Validates arguments and the resolved definition before durable creation.
- Uses the same launch coordinator and real Lua runner as slash and model
  launches.
- Waits for completion by default.
- Has no `--detach`. Neo has no long-lived daemon behind the headless command;
  returning from the process would not provide a supervised continuing run.
  Interactive Neo sessions remain the real background owner.
- Text output shows the human display name, current macro state, result, files,
  or a clear failure reason. It does not show machine IDs by default.
- JSON emits one terminal or awaiting-user document.
- JSONL emits started, phase, child-summary, awaiting-user, and terminal events
  as they occur; it is flushed per event rather than buffered until return.
- Ctrl+C requests a controlled stop for the owned run and exits with the
  platform-standard interrupted code after the stop is acknowledged.

### 9.4 `check`

Purpose shown to the user:

> Check that a workflow is valid without running it.

Rules:

- Accepts a registry name or a trusted definition path.
- Performs metadata, source, schema, compile, and deterministic smoke checks.
- Does not create a task, run, journal, artifact, or permission prompt.
- Reports that a smoke check does not prove every live branch.

### 9.5 `test`

Purpose shown to the user:

> Try a workflow safely with recorded test results instead of real agents and tools.

Rules:

- Accepts a registry name or trusted definition path plus one fixture.
- Uses the deterministic harness only.
- Does not call live providers or live tools.
- Produces a clear case result and the first actionable mismatch.
- Does not expose harness queues, invocation hashes, or journal internals in
  text output.

### 9.6 Removed CLI surface

The following commands and their flags are deleted from parser, help, guides,
completion scripts, tests, and examples:

```text
show save answer fork prune
```

Also deleted from the public CLI:

```text
--scope --detach --checkpoint --older-than --max-bytes --dry-run --yes
```

Removal from CLI does not delete canonical runtime abilities that remain in use
through Task tools, the Operator, retention policy, or internal recovery.

### 9.7 Exit semantics

| Exit | Meaning |
| --- | --- |
| `0` | requested operation completed successfully |
| `1` | workflow completed with a user/workflow failure |
| `2` | CLI input, definition, args, or fixture invalid; no launch side effect |
| `3` | non-interactive run is durably waiting for human input |
| `4` | Neo host/runtime failure or uncertain launch outcome |
| `130` | user interruption after controlled stop request |

### 9.8 Host-owned retention

Removing `prune` does not permit unbounded storage growth. Neo SHALL turn the
existing read-only retention classifier into the single automatic retention
owner.

Policy:

- Reuse `RetentionSubject`, `RetentionPolicy`, `preview_mark_sweep`, and
  `runtime.workflow.global_storage_bytes`.
- Run a bounded retention pass at application startup, after a workflow becomes
  terminal, and immediately before denying a new run for global workflow
  storage.
- Trigger reclamation only when actual durable workflow storage reaches 90% of
  `global_storage_bytes`.
- Reclaim oldest eligible runs until actual storage is at or below 80% of the
  limit or no eligible run remains.
- A run is eligible only when it is terminal, unreferenced by lineage, and at
  least 30 days old.
- Running, queued, paused, AwaitingUser, non-terminal, and lineage-referenced
  runs are never candidates.
- Selection is deterministic: oldest terminal timestamp, then run ID.
- Revalidate eligibility and path containment immediately before deletion.
- Delete one explicit run directory at a time, sync the parent directory using
  existing platform-safe behavior, then release that run's storage admission.
- A failed deletion is reported and skipped; it never causes another run to be
  deleted beyond the target.
- If protected data alone exceeds the limit, Neo keeps it and reports in plain
  language that workflow storage is full. It does not delete protected work.
- A run becomes eligible only after its terminal summary has been persisted to
  the session transcript. Retention may remove detailed journal/artifact data;
  the terminal transcript summary and generated workspace files remain.

The 30-day floor and 90/80 hysteresis are host constants in this version, not
new user-facing knobs. Add configuration only when a concrete deployment needs
different retention behavior.

Automatic retention is reported once in ordinary logs as a count and reclaimed
byte total. The normal UI does not ask the user to perform storage maintenance.

## 10. Model-Visible Workflow Contract

### 10.1 Tool and actions

The top-level model SHALL continue to see one tool named `Workflow` with all
seven actions:

```text
list
show
validate_inline
validate_saved
save
run_inline
run_saved
```

No alias or second workflow tool exists.

### 10.2 Self-contained mutation actions

`run_inline`, `run_saved`, and `save` SHALL each perform their complete
preflight internally:

```text
parse input
-> resolve or build definition
-> validate metadata/source/schema
-> compile Lua
-> validate args when applicable
-> request typed permission when applicable
-> perform save or launch
```

If any preflight step fails:

- no file, run, task, journal, or approval side effect occurs;
- the error names the exact field and expected shape;
- the response suggests only relevant next actions;
- the response never tells the model to use CLI, slash, source search, Cargo,
  or another validation action before retrying.

### 10.3 Explicit validation actions

`validate_inline` and `validate_saved` remain complete, supported capabilities.
They are used only when the requested intent is to check without running or
saving.

System prompt, tool description, skill text, examples, and errors SHALL NOT
describe either validation action as a prerequisite for `run_*` or `save`.

### 10.4 Discovery and explanation

- `list` returns canonical name, display name, plain-language purpose, when to
  use it, and a concise input summary.
- `show` returns the selected saved definition, complete schemas, phases,
  origin label, and source needed to understand or modify it.
- Hashes remain structured internal metadata and are not emphasized in prose.

### 10.5 Skill contract

The `create-workflow` discovery description SHALL cover these intents:

```text
create, author, write, design, adapt, evaluate, test, or run a custom workflow
```

The skill SHALL teach Lua authoring quality and host APIs. It SHALL NOT:

- require a skill activation token or runtime grant;
- require explicit validation before run/save;
- teach the headless CLI as an assistant path;
- instruct source-code or Cargo exploration to learn the product contract;
- claim that prompt order is a safety boundary.

Known saved workflows may use `list`, `show`, or `run_saved` without activating
the authoring skill.

### 10.6 Launch output and completion

A successful run action returns immediately with:

```text
status: started
task_id
display_name
purpose
next_action: wait_for_completion | TaskOutput
```

Machine run IDs remain structured and non-prominent.

When a workflow reaches a reportable terminal state, the existing session
notification path SHALL deliver one completion event to the root session. If a
model turn is active, delivery waits for the next safe break point. If idle,
Neo surfaces the completion without requiring polling.

The completion message contains:

- workflow display name;
- completed/failed/stopped state;
- final summary or failure reason;
- generated files;
- the task handle for optional details.

It does not dump the journal or automatically start another workflow.

## 11. `/tasks` Entry and Navigation

### 11.1 Smart direct entry

When the user invokes `/tasks`:

1. If any workflow is `AwaitingUser`, open the most recently updated one in the
   Workflow Operator.
2. Else if any workflow is active, open the most recently updated active one in
   the Workflow Operator.
3. Else open the existing general Task Browser.

### 11.2 Entry from the general browser

- Moving selection onto a Workflow row does not switch pages.
- `Enter` on a Workflow row opens its Operator.
- `Enter` on ordinary task kinds keeps existing behavior.
- `Esc` in the Operator returns to the general Task Browser with its prior
  selection and viewport intact.
- A second `Esc` follows the existing Task Browser close behavior.

This prevents surprising page changes while preserving direct access.

## 12. Workflow Operator Information Model

### 12.1 Header

The header shows only:

- display name;
- plain-language purpose;
- macro state;
- elapsed time;
- completed/running/queued child counts;
- a Needs input banner when applicable.

It does not show run ID, revision, source scope, hash, journal sequence, token
total, prediction, or checkpoint.

### 12.2 Steps

User-facing terminology is **Steps**, not backend phase mechanics.

Each row contains:

- status marker;
- ordinal;
- title;
- observed child counts by state.

Declared steps appear in declared order. Runtime-observed undeclared steps are
appended in first-observed order. A workflow without `neo.phase()` has one
synthetic projection step named `Execution`; this is a view fallback, not a
durable runtime phase.

Dynamic workflows must not show a predicted denominator. While active, use:

```text
6 done · 1 working · 2 queued
```

Do not show `6/9` unless the denominator is a durable, already-known closed set.

### 12.3 Agents

The selected step's roster shows every direct workflow child:

- direct `neo.delegate` child;
- every individual `neo.swarm` item.

Main-row fields:

```text
status marker | title | role when helpful | latest activity | elapsed
```

Main rows do not show token usage, model name, provider, machine ID, or raw
error payload.

Rows keep durable creation order. Live updates never reorder rows underneath
the cursor. Failed rows are visually prominent but are not moved.

### 12.4 Details

`Enter` on an agent opens one Details page containing:

- title and role;
- macro state and elapsed time;
- current activity or terminal summary;
- recent tool activity using the existing compact child-activity projection;
- generated/changed files;
- final structured summary or clear failure reason;
- actual token usage when available;
- model/provider only when helpful for diagnosis.

The page does not expose raw journal JSON, hashes, workflow IDs, or schema
transport payloads.

## 13. Wide-Screen UI

Target: terminal content width `>= 100` columns.

```text
+ Workflows ----------------------------------------------------------------------+
| workflow-platform                                      Running · 4h02m          |
| Execute the approved platform plan with parallel agents and final review.       |
| 32 done · 1 working · 0 queued                                                |
|                                                                                |
| Steps                         | Agents · ChildTool                              |
|                               |                                                 |
|   1 Bootstrap        done     |  ✓ schema repair        done · 15m53s           |
|   2 Substrate        done     |  ✓ root commit          done · 1m07s            |
|   3 Services         done     |  ✓ swarm child plan     done · 17m10s           |
|   4 DefsLaunch       done     |  ✓ swarm child user     done · 2m06s            |
| > 5 ChildTool        active   |  · awaiting user task   done · 1m04s            |
|     6 done · 1 working        |  ● neo tool              Thinking · 10m06s      |
|   6 Surfaces         pending  |                                                 |
|   7 Closeout         pending  |                                                 |
|   8 Final Review     pending  |                                                 |
|                               |                                                 |
|                               |                                                 |
| Tab steps/agents · Enter details · P pause · X stop · S save · Esc tasks        |
+--------------------------------------------------------------------------------+
```

Differences from the Grok reference are deliberate:

- no token column in the main roster;
- `S save` appears only for inline unsaved runs;
- `X stop` replaces the retired prune binding;
- controls call `BackgroundTaskManager` directly rather than generating slash
  command text;
- no child budget is displayed;
- no row ceiling exists.

The sample footer depicts an inline unsaved run. Saved and builtin runs omit
`S save` and reclaim that width for status text.

### 13.1 Focus

There are only two focus areas: Steps and Agents.

- `Tab` moves between them.
- `Up/Down` moves within the focused area.
- When Steps has focus, changing the step refreshes the roster.
- When Agents has focus, `Enter` opens the selected agent Details page.
- Mouse click selects a step or agent; wheel scrolls the pane under the pointer.

### 13.2 Active-step following

- On open, select the effective active step.
- While the user has not manually changed the step, follow a newly active step.
- The first manual `Up/Down` in Steps pins the chosen step.
- Live updates preserve that pin.
- Closing and reopening the Operator resets to the effective active step.

## 14. Needs-Input UI

When the selected workflow is waiting for a human, the Operator gives the
request visual priority and opens the answer dialog automatically.

```text
+ Workflow needs your input ------------------------------------------------------+
| Merge strategy                                                                 |
|                                                                                |
| Choose how the completed branches should be combined.                          |
|                                                                                |
|  > Merge automatically                                                         |
|    Review each branch first                                                     |
|    Keep branches separate                                                       |
|                                                                                |
| Enter choose · Up/Down move · Esc answer later                                  |
+--------------------------------------------------------------------------------+
```

There is no `A` shortcut and no confirmation that submits an empty object.

### 14.1 Schema-driven answer form

The UI renders the durable answer schema as human controls:

| Schema shape | Control |
| --- | --- |
| `boolean` | Yes/No choice |
| string `enum` | single-choice list |
| array of `enum` | multi-choice list |
| plain string | text or multiline input |
| integer/number | validated numeric input |
| object | labeled field form in schema order |
| nested object | drill-down field page with breadcrumb |
| array of objects | repeatable row editor |
| `oneOf`/`anyOf` with titled branches | branch choice, then branch form |

Rules:

- Show title, prompt, field descriptions, required state, and defaults in plain
  language.
- Do not show JSON Schema keywords to the user.
- Validate before submission and attach each error to the relevant field.
- Never silently coerce invalid values.
- Defaults require explicit confirmation; opening the dialog does not answer.
- Unsupported advanced schema constructs open an advanced structured editor
  with a generated template and continuous validation. This is a fallback, not
  the default experience.
- Secret collection remains forbidden.

### 14.2 Answer later

`Esc` closes the dialog without answering. The workflow remains Needs input,
stays first in `/tasks` entry priority, and shows this compact banner:

```text
! Needs your input: Choose how completed branches should be combined.
  Enter answer
```

The TUI records the dismissed `request_id` only as ephemeral view state. A
periodic refresh does not reopen the same dismissed request. `Enter answer`, a
new request ID, or closing and reopening the Operator may open the dialog again.
Dismissal never changes durable workflow state.

## 15. Agent Details UI

```text
+ Agent details ------------------------------------------------------------------+
| neo tool                                                     Working · 10m06s  |
| Role: implementation                                                               |
|                                                                                |
| Current work                                                                   |
| Thinking through the workflow control binding and focused regression.          |
|                                                                                |
| Recent activity                                                                |
|   ✓ Read crates/neo-agent-core/src/tools/workflow.rs                            |
|   ✓ Edit crates/neo-agent/src/modes/task_browser.rs                             |
|   ● Bash cargo nextest run ...                                                  |
|                                                                                |
| Files                                                                          |
|   M crates/neo-agent/src/modes/task_browser.rs                                  |
|                                                                                |
| Actual usage: 193.8K tokens                                                     |
|                                                                                |
| Esc back                                                                        |
+--------------------------------------------------------------------------------+
```

Terminal agent state replaces Current work with Result or Failure. Long activity
uses the existing bounded output/read-more behavior and must never silently cut
an issued shell command.

The existing transcript Delegate and DelegateSwarm cards are not embedded,
restyled, or duplicated here.

## 16. Narrow-Screen UI

### 16.1 Medium width: `70-99` columns

Steps and Agents stack vertically while retaining one page:

```text
+ Workflow · workflow-platform -------------------------------+
| Running · 32 done · 1 working · 4h02m                        |
|                                                              |
| Steps                                                        |
|   4 DefsLaunch       done                                    |
| > 5 ChildTool        6 done · 1 working                      |
|   6 Surfaces         pending                                 |
|                                                              |
| Agents · ChildTool                                           |
| ✓ schema repair         done · 15m53s                        |
| ✓ root commit           done · 1m07s                         |
| ● neo tool              Thinking · 10m06s                    |
|                                                              |
| Tab section · Enter details · P pause · X stop · S save      |
| Esc tasks                                                    |
+--------------------------------------------------------------+
```

### 16.2 Small width: `< 70` columns

Use sequential full pages:

```text
Workflow summary -> Steps -> Agents -> Agent details
```

Character art:

```text
+ workflow-platform ----------------+
| Running · 4h02m                    |
| 32 done · 1 working                |
|                                    |
| Current step                       |
| ChildTool · 6 done · 1 working     |
|                                    |
| Needs input: none                  |
|                                    |
| Enter steps · P pause · X stop     |
| S save · Esc tasks                 |
| Esc tasks                          |
+------------------------------------+
```

Every fixed header/footer has bounded height. Long names wrap once or truncate
with an ellipsis; they never resize the surrounding pane or overlap controls.

## 17. Operator Controls

The complete normal shortcut set is:

| Key | Contextual action |
| --- | --- |
| `Up/Down` | move within current pane |
| `Tab` | switch Steps/Agents on split or stacked layouts |
| `Enter` | answer, open selected agent, or advance narrow page |
| `P` | pause an active workflow; resume a paused workflow |
| `X` | request stop and show confirmation |
| `S` | save an inline unsaved workflow; otherwise no binding |
| `Esc` | close dialog/details or return to general tasks |

There is no manual refresh key; the page uses the existing periodic refresh.
There is no main-page output-cycle key, transcript-jump key, fork key, prune key,
or answer key. Save is contextual rather than a permanent action.

### 17.1 Pause truthfulness

Pause controls the parent workflow. If work is already executing:

- parent state becomes `Pausing`;
- existing children display `Working (finishing current work)` when they are
  allowed to drain;
- no queued child starts after pause acknowledgement;
- the UI does not claim all children are paused before this is true.

Resume continues the same durable run only when runtime invariants allow it.

### 17.2 Stop

Stop requires confirmation:

```text
Stop workflow-platform?
Current child work will be cancelled where safely supported.

Enter stop · Esc keep running
```

The Operator calls `BackgroundTaskManager` control methods directly. The TUI
never manufactures slash-command strings.

### 17.3 Save an inline run

`S save` is visible only when all of these are true:

- the selected run came from an inline definition;
- that exact definition is not already registered;
- the run has durable source/metadata sufficient for `WorkflowSaveRequest`;
- the current permission/trust context allows at least one destination.

The dialog is:

```text
+ Save this workflow ------------------------------------------------------------+
| Name                                                                           |
|   release-review                                                               |
|                                                                                |
| Available in                                                                   |
| > This project                                                                 |
|   All projects                                                                 |
|                                                                                |
| Enter save · Esc cancel                                                        |
+--------------------------------------------------------------------------------+
```

User-facing destinations map internally as follows:

| Label | Registry target |
| --- | --- |
| This project | trusted project workflow registry |
| All projects | user workflow registry |

Rules:

- The name defaults to the definition name and is editable.
- Internal scope names, paired-file paths, revisions, and hashes are hidden.
- Save rebuilds the request from the run's pinned exact definition; it never
  reads an edited workspace file by accident.
- The registry performs full validation and no-clobber checks.
- A different existing definition opens a typed Replace/Cancel decision showing
  the two plain-language names and destinations, not hashes.
- Builtins and already-saved runs do not show Save.
- Saving does not relaunch or mutate the completed/running run.
- The action routes through the existing typed permission and
  `WorkflowDefinitionRegistry` owners; TUI state never writes files.

## 18. Projection Data Contract

### 18.1 Workflow snapshot

The host-to-TUI projection is equivalent to:

```text
WorkflowOperatorSnapshot
  task_id
  run_id                    internal stable selection only
  display_name
  purpose
  state
  elapsed
  updated_at
  current_step_key
  child_counts
  steps[]
  pending_user?
  final_summary?
  failure_reason?
  generated_files[]
```

### 18.2 Step row

```text
WorkflowStepRow
  key = (phase_id, phase_marker_sequence)
  title
  order
  state: pending | active | completed | failed | paused
  done_count
  working_count
  queued_count
  failed_count
```

`phase_marker_sequence` is derived from the durable journal envelope sequence.
No new durable `phase_instance_id` is created.

### 18.3 Child identity

No new random child UUID is introduced.

```text
WorkflowChildKey
  DirectDelegate { invocation_id }
  SwarmItem { swarm_id, item_id }
```

### 18.4 Child row

```text
WorkflowChildRow
  key
  step_key
  child_kind: delegate | swarm_item
  agent_id?                 absent while queued
  title
  role?
  state
  queued_at
  started_at?
  updated_at
  terminal_at?
  terminal_summary?
  error_summary?
  actual_usage?             details only
  latest_activity?          live only
  generated_files[]
```

Child states are:

```text
queued
running
completed
failed
cancelled
interrupted
recovering
```

Schema repair and resource admission are activity/reason fields, not alternate
child lifecycle owners.

## 19. Durable Child Lifecycle

### 19.1 New journal version

Adding generic child events changes a durable, fail-closed format. New runs
SHALL therefore write Workflow journal format V3 rather than silently extending
V2 under the same version.

V3 adds:

```text
ChildQueued
  child_key
  child_kind
  invocation_id
  phase_id?
  spec_payload_ref

ChildStarted
  child_key
  agent_id

ChildFinished
  child_key
  agent_id?
  outcome_payload_ref
```

The envelope sequence and timestamps remain the ordering/time owners.

`spec_payload_ref` points to the canonical child specification already used for
dispatch. `outcome_payload_ref` points to the canonical child outcome payload.
For a direct delegate, `InvocationFinished` and `ChildFinished` reference the
same outcome payload instead of serializing two independent copies. For a swarm,
each item has one canonical item outcome payload while the parent invocation may
retain its aggregate return value.

### 19.2 Writer rules

- New direct delegates and swarm items write the same generic lifecycle.
- `ChildQueued` and its canonical specification payload are durable before
  dispatch.
- `ChildStarted` binds the runtime agent ID before live work is reported.
- `ChildFinished` binds the child to the already-written canonical outcome
  payload after terminal status, summary, error, references, and actual usage
  are known.
- Live token/tool/text activity is not journaled per update.
- Invocation events remain effect/replay owners. Child events own lineage and
  roster lifecycle only; they do not create a second serialized effect result.

### 19.3 Read compatibility

- New Neo reads V2 and V3.
- V2 runs are never migrated or rewritten.
- V2 `SwarmItemQueued/Started/Finished` events project to the generic child row.
- A V2 direct delegate that only has a terminal `child_ref` appears after
  terminal reconstruction; Neo does not invent a missing live history.
- New runs write only the V3 generic child lifecycle, not both generic and old
  swarm events.

This is a one-way writer retirement, not two durable owners.

### 19.4 Recovery

After restart:

- terminal children rebuild from journal facts;
- queued children remain queued only if the runtime can safely re-admit them;
- a started child without a terminal record displays `Recovering` until the
  runtime resolves it;
- it is never shown as Working merely because it was working before exit;
- uncertain external effects are not automatically retried.

## 20. Live Merge

`BackgroundTaskManager` SHALL produce the Operator projection by joining:

```text
WorkflowRuntime snapshot + V2/V3 journal child projection
                     |
                     +-- agent_id --> MultiAgentRuntime live snapshot
```

Merge rules:

- durable identity/state wins for terminal facts;
- live state may enrich only non-terminal rows;
- latest activity and current usage disappear when no live snapshot exists;
- stale live timestamps cannot overwrite newer durable terminal records;
- duplicate rows with the same `WorkflowChildKey` are a projection error, not
  silently merged by title;
- workflow children are excluded from unrelated top-level Delegate task rows.

The TUI receives immutable snapshots and stores only selection, focus,
viewport, and dialog drafts.

## 21. Scale and Paging

- There is no total child limit introduced by the Operator.
- Child rows are loaded by selected step with opaque stable cursors.
- The host page size is a transport/rendering choice, not a workflow limit.
- The TUI virtualizes rows and keeps only visible rows plus a small overscan.
- Selection is anchored by `WorkflowChildKey`, not numeric index.
- On refresh, a selected row remains selected if present; otherwise choose the
  nearest prior row.
- 1,000 and 10,000 child fixtures must remain navigable without loading the
  entire journal or all activity transcripts into one frame.
- Search/filter is not added in this version. Add it only when measured runs
  show navigation is insufficient.

## 22. Error Presentation

Default user-visible errors contain:

```text
what failed
where in the workflow it failed
whether completed work is preserved
what the user can do now
```

Examples:

```text
Step "Verify" failed because the test agent returned invalid structured output.
Completed work is preserved. Open the failed agent for details or stop the run.
```

```text
Workflow is waiting for available child capacity.
No action is required; queued work will start automatically.
```

Do not show error codes, paths, IDs, or transport objects unless the user opens
advanced diagnostic output outside the normal Operator.

## 23. Security and Permissions

- Workflow save and launch continue through the existing typed permission
  protocol.
- Pause, resume, stop, and answer retain existing actor and policy checks.
- `TaskAnswer` remains allowed only when the durable `answer_policy` permits the
  model; human-only gates require the human UI.
- Child, restricted, schema-repair, and workflow-script registries do not gain
  the `Workflow` tool.
- Project definitions and saves retain workspace trust and path containment.
- No TUI action directly edits the journal or definition registry.
- No secret is accepted through workflow human input.

## 24. Responsive and Cross-Platform Requirements

- All persisted paths use `Path`/`PathBuf` and existing platform-safe writers.
- Layout is pure terminal-cell layout with Unicode-width-aware truncation.
- Windows, Linux, and macOS key handling uses existing crossterm abstractions.
- No bare shell is introduced by the Operator or CLI control flow.
- Alternate-screen mouse capture remains active while the Operator is visible.
- At widths `32`, `70`, `99`, `100`, `120`, and `180`, no title, status, row,
  footer, or dialog may overlap another region.
- At heights `12`, `20`, and `40`, content scrolls rather than overwriting the
  footer.
- The answer dialog must remain usable with keyboard only.

## 25. Transcript Boundary

Existing transcript cards remain canonical for conversational history:

- Workflow card;
- Delegate card;
- DelegateGroup card;
- DelegateSwarm card;
- Bash/Terminal activity inside those cards.

The Operator is an aggregate locator and control surface. It does not alter card
layout, expansion behavior, elapsed-time behavior, activity rows, or transcript
placement.

The first version does not add jump-to-transcript behavior. That would require
a stable cross-surface transcript index and is not needed because Agent Details
already provides the relevant result and activity.

## 26. Retired Paths

Implementation SHALL delete, not hide:

- CLI `show`, `save`, `answer`, `fork`, and `prune` variants;
- CLI parsing/help/docs/tests for their flags;
- headless synthetic runner behavior;
- buffered fake JSONL output;
- TUI workflow fork/prune actions and confirmations;
- TUI answer confirmation that sends `{}`;
- main-page manual refresh and output-cycle shortcuts for workflow detail;
- mandatory assistant validation-before-run wording;
- tool/skill errors that direct the model to CLI or slash prerequisites.

Runtime fork/lineage, retention, answer, and save capabilities remain only where
another approved product path still uses them. A planning-time caller scan must
delete any internal API left with no production owner.

## 27. Complexity Governance

Likely high-pressure files already mix substantial responsibilities:

- `crates/neo-agent-core/src/tools/background_tasks.rs`
- `crates/neo-agent/src/modes/interactive/input.rs`
- current Task Browser rendering/input modules
- workflow runtime and journal modules

Implementation planning SHALL classify each edit:

- wiring-only;
- local replacement;
- move-out/extract-first;
- new responsibility.

New Workflow Operator rendering/state belongs in a bounded workflow-specific
module under the existing Task Browser ownership, not as another large branch
inside a generic renderer. Journal V3 projection belongs beside workflow
journal/state projection, not in TUI or `BackgroundTaskManager` parsing code.

This governance is about clear ownership, not maximizing file count.

## 28. Verification Requirements

### 28.1 CLI

1. Help lists exactly `list/run/check/test` at one level.
2. Removed commands fail as unknown; no aliases remain.
3. `run` executes real Lua and returns the actual final result.
4. JSONL events are observable before terminal completion.
5. A TTY human gate is answered in the same process.
6. A non-TTY human gate emits structured state and exits `3`.
7. `check` and `test` produce zero run/task/file side effects.
8. Crossing the storage high watermark runs deterministic protected retention;
   active, waiting, referenced, and younger-than-30-day runs remain untouched.

### 28.2 Assistant

1. `run_inline` succeeds without a preceding validation action.
2. `run_saved` succeeds without a preceding validation action.
3. `save` validates internally and leaves zero writes on failure.
4. Explicit validation actions still work and remain side-effect free.
5. A known saved workflow runs without skill activation.
6. Custom authoring uses the skill and first-party tool, not Neo CLI.
7. Completion arrives without polling.

### 28.3 Operator

1. `/tasks` prioritizes Needs input, then active workflow, then general tasks.
2. Moving selection never changes page; Enter and Esc preserve browser state.
3. Wide, stacked, and sequential layouts match the documented hierarchy.
4. Direct delegate and every swarm item appear exactly once in the correct step.
5. Live activity matches `MultiAgentRuntime`; terminal state matches journal.
6. Token usage is absent from the roster and present in Details when known.
7. Answer forms render and validate boolean, enum, object, nested, and array
   cases without exposing schema mechanics.
8. Pause shows truthful draining state.
9. Stop confirmation reaches the existing runtime owner.
10. Save appears only for inline unsaved runs and persists the pinned exact
    definition through the existing registry/permission owners.
11. Existing transcript cards render byte-for-byte-equivalent logical content.

### 28.4 Durability and scale

1. V3 direct and swarm child lifecycles replay after restart.
2. V2 terminal runs remain readable without migration.
3. Started-without-terminal children display Recovering.
4. Unknown or torn journal data retains existing fail-closed/recovery rules.
5. A 1,000-child and 10,000-child fixture can be paged with stable selection.
6. No test or production constant imposes a total workflow child ceiling.

### 28.5 Native platform evidence

Focused deterministic tests run locally first. Before completion, native
Windows, Linux, and macOS evidence is required for:

- CLI TTY prompt behavior;
- non-TTY exit behavior;
- Operator key/mouse input;
- narrow/wide terminal rendering;
- journal V3 create/replay and path safety.

Local proof must not be reported as remote CI or native proof for another OS.

## 29. Acceptance Scenarios

### Scenario A: novice human

The user says "run the release workflow", opens `/tasks`, watches named steps
and agents, answers one typed question, and receives the final result without
seeing an ID, hash, journal, checkpoint, scope, or second terminal.

### Scenario B: imperfect model

The user asks for a custom evaluation workflow. The model activates the
authoring skill, writes Lua, calls `Workflow(run_inline)` directly, and receives
a task. It does not call validation first, source tools, Cargo, or Neo CLI.

### Scenario C: explicit no-side-effect check

The user asks Neo to check a saved workflow without running it. The model calls
`Workflow(validate_saved)`. No run, task, file, or approval dialog appears.

### Scenario D: large swarm

A workflow creates more than 1,000 swarm items across multiple steps. The
Operator pages every child, preserves selection under live updates, and imposes
no total limit.

### Scenario E: restart

Neo restarts after some children finish and one child is unresolved. Completed
children and failures rebuild from the journal. The unresolved child shows
Recovering, never falsely Working or Completed.

### Scenario F: headless interactive run

A user runs `neo workflow run` in one terminal. The workflow asks a typed
question in that terminal and continues after the answer. No second command or
terminal is required.

### Scenario G: storage fills without operator maintenance

Actual workflow storage crosses 90% of its configured physical limit. Neo
automatically deletes only eligible terminal runs older than 30 days until it
reaches 80%, reports the reclaimed count, and starts the requested workflow.
If protected work alone fills storage, Neo preserves it and explains why the new
run cannot start.

## 30. Non-Goals

This design does not:

- replace Lua with Rhai;
- add a second script engine or engine abstraction;
- add a new slash command;
- add `/tasks workflow`;
- add a second dashboard or task system;
- redesign existing transcript cards;
- add per-agent stop/retry;
- add automatic retry of uncertain effects;
- add predictive token, cost, time, or agent governance;
- add a total child cap;
- add hosted services or a web dashboard;
- add jump-to-transcript indexing in the first version;
- migrate or rewrite old session/workflow journals;
- preserve removed CLI commands as hidden aliases.

## 31. Baseline and ADR Sync

After implementation and verification:

1. Create a new ADR covering the human CLI, assistant action semantics,
   workflow journal V3 child lifecycle, completion delivery, and Operator.
2. Create a new landed baseline snapshot.
3. Supersede only the affected portions of ADR-0007 and current workflow
   baselines.
4. Keep historical specs, plans, ADRs, and V2 evidence unchanged.
5. Update English/Chinese guides, tool references, slash references, CLI help,
   and the `create-workflow` skill from the new landed baseline.

## 32. Approval Boundary

The user approved this design on `2026-07-28`. Approval authorizes writing an
implementation plan; it does not by itself authorize source edits or
implementation.

There are no unresolved design questions in this approved design.
