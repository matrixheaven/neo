# Neo Workflow Intent Slash Design

Date: `2026-07-30`

Status: `approved for implementation planning`

Architecture review required: `yes`

ADR signal: `yes`. After implementation and acceptance, record this as the
current slash-entry decision and supersede only the conflicting slash sections
of the workflow product records. Historical documents remain historical.

## 1. Decision

Neo SHALL expose three clear workflow invocation forms and one separate
authoring form:

```text
/workflow
/workflow <natural-language task>
/workflow:<name> <natural-language task>
/skill:create-workflow [authoring request]
```

Their meanings are fixed:

| Form | Human intent | Neo behavior |
| --- | --- | --- |
| `/workflow` | Show me what workflows are available | Open a searchable workflow picker |
| `/workflow <task>` | Choose the best existing workflow for this task | Start a visible model turn with the complete effective workflow summary |
| `/workflow:<name> <task>` | Use this exact workflow for this task | Start a visible model turn with that definition's description and full input schema |
| `/skill:create-workflow <request>` | Create or change a workflow | Activate the authoring skill through the existing skill path |

The former host-direct `/workflow <name> [JSON_OBJECT]` form SHALL be deleted.
It SHALL NOT remain as an alias, fallback, hidden parser, help example, or test
fixture. A person never writes workflow argument JSON in a slash command.

## 2. Problem

The current `/workflow` surface reverses the user's intent:

- bare `/workflow` unexpectedly activates an authoring skill;
- `/workflow <name> [JSON_OBJECT]` exposes registry names and schema arguments
  as a host command instead of letting the model understand the user's task;
- built-in definitions appear as static `/workflow <name>` candidates while
  project and user definitions do not appear in the same way;
- the same word looks like both "use a workflow" and "create a workflow";
- a user must understand internal definition input before running anything.

The redesign makes workflow use behave like skill use while keeping authoring
separate:

```text
/workflow:deep-research Research solid-state batteries and cite primary sources
/skill:create-workflow Create a release-readiness workflow for this repository
```

## 3. Product Rules

These rules are requirements:

1. A user states the result they want. Neo and the model translate that intent
   into workflow inputs.
2. Each entry is understandable without reading source code, files, registry
   rules, schemas, or runtime documentation.
3. Workflow discovery comes from the existing effective definition registry.
   There is no static built-in-only list and no second registry.
4. Selection is semantic model work. Keyword tables and regular expressions
   may parse command grammar, but they SHALL NOT choose a workflow.
5. The model receives enough structured guidance to make the correct first
   action without an obligatory discovery call.
6. The seven existing `Workflow` tool actions remain available. This redesign
   changes invocation guidance, not capability.
7. Permission modes continue to decide approvals. Slash syntax does not grant,
   reserve, or bypass permission.
8. The user-visible message is always the exact submitted slash text.
9. Internal helper context is complete or rejected. Neo SHALL NOT silently
   truncate a workflow catalog and then claim that model selection covered all
   available definitions.
10. One final path replaces the former path. No compatibility branch remains.

## 4. Scope

### 4.1 In scope

- interactive TUI slash parsing;
- bare `/workflow` picker behavior and responsive layout;
- dynamic `/workflow:<name>` completion candidates;
- automatic and named workflow model-turn context;
- local validation and error messages;
- user-message persistence and resume behavior;
- model guidance in the `Workflow` tool and `create-workflow` skill;
- English and Chinese user documentation;
- deletion of the former host-direct slash launch path and its tests.

### 4.2 Out of scope

- the four `neo workflow` headless commands;
- workflow runtime, journal, recovery, task controls, or `/tasks` layout;
- Lua host behavior or a script-engine change;
- workflow definition file formats or registry precedence;
- the seven `Workflow` tool actions;
- Delegate, DelegateGroup, DelegateSwarm, or Workflow transcript card layout;
- a recently-used list, favorites, ranking history, or usage analytics;
- automatic workflow creation without user confirmation;
- predictive token, cost, or agent budgets;
- a second dashboard, registry, runtime, permission mode, or task type.

## 5. Existing Owners

| Responsibility | Existing owner to reuse |
| --- | --- |
| Effective definition discovery and precedence | `WorkflowDefinitionRegistry::list(Effective)` |
| Exact definition resolution and full schema | `WorkflowDefinitionRegistry::resolve` |
| Slash parsing and host routing | interactive slash modules in `neo-agent` |
| Slash completion filtering | existing prompt completion pipeline |
| Picker rendering and input | `neo-tui` overlay and selection primitives |
| Model turn creation and visible user message | `TurnRequest` and the existing run pipeline |
| Workflow execution and approval | existing `Workflow` tool and permission system |
| Skill activation | existing `Skill` tool and `/skill:<name>` path |
| Session replay | existing JSONL user-message events |

No owner in this table is replaced. The change adds one workflow-specific
turn context and one workflow-specific picker presentation at the existing
boundaries.

## 6. Command Grammar

### 6.1 Exact grammar

```text
workflow_picker  := "/workflow" whitespace*
workflow_auto    := "/workflow" whitespace+ task
workflow_named   := "/workflow:" name whitespace+ task
name             := existing WorkflowName grammar
task             := first non-whitespace character through end of submission
```

Parsing is case-sensitive. Only lowercase `/workflow` is a command. The task
may contain spaces, punctuation, pasted markers, file references, and newlines.
Neo trims boundary whitespace but does not rewrite task content.

Examples:

| Input | Result |
| --- | --- |
| `/workflow` | open picker |
| `/workflow   ` | open picker |
| `/workflow Research this API` | automatic selection turn |
| `/workflow:deep-research Research this API` | named selection turn |
| `/workflow:deep-research` | local missing-task error |
| `/workflow:` | local missing-name error |
| `/workflowish research this` | ordinary prompt, not a workflow command |
| `Please use /workflow for this` | ordinary prompt, not a workflow command |

### 6.2 Removed ambiguity

Everything after whitespace in `/workflow <task>` is task text. Neo SHALL NOT
try to reinterpret the first word as a saved definition name. The only named
form uses a colon.

The following former form is invalid after this change:

```text
/workflow deep-research {"topic":"battery recycling"}
```

The correct form is:

```text
/workflow:deep-research Research battery recycling
```

### 6.3 Busy turn

Workflow slash forms start or prepare a model turn and therefore require an
idle main composer. If a model turn or shell command is active, Neo SHALL keep
the submitted text in the composer and show:

```text
Finish or interrupt the current turn before starting a workflow.
```

Neo SHALL NOT enqueue a workflow slash as ordinary prose because that would
lose its definition context. `/tasks` and live permission commands retain their
existing special behavior.

## 7. Effective Workflow Catalog

Every interactive workflow surface uses the same effective list returned by
`WorkflowDefinitionRegistry::list(WorkflowListScope::Effective)`.

The list contains only definitions that resolved successfully under existing
precedence:

```text
Built-in < All projects < This project
```

The user-facing source labels are:

| Registry source | UI label |
| --- | --- |
| `Builtin` | `Built-in` |
| `User` | `All projects` |
| `Project` | `This project` |

Internal source locators, revisions, hashes, paths, and registry object names
never appear in the picker or model selection summary. An unexpected source
kind is a local error; Neo does not invent a fourth public label.

Definitions sort by case-insensitive display name, then canonical name. The
order is stable and does not depend on discovery order, filesystem order, or
recent use.

For each definition, the catalog provides:

```text
name
display_name
description
public source label
required input property names
```

Required input names come from the resolved input schema's `required` array.
If none are required, the UI shows `Required: None`.

## 8. Bare `/workflow` Picker

### 8.1 Opening

Submitting exact bare `/workflow` clears the command from the composer and
opens a focused searchable picker titled `Run a workflow`.

The picker lists all effective definitions, not only built-ins. It uses the
existing Neo overlay theme, selection colors, borders, and keyboard behavior.
It is a workflow-specific presentation built from existing selection and text
input primitives; it is not a new general dashboard.

### 8.2 Wide layout

At 80 columns or wider:

```text
╭─ Run a workflow ───────────────────────────────────────────────────────────╮
│ Search  deep research█                                                    │
├────────────────────────────────────────────────────────────────────────────╢
│ > Deep Research                                                          │
│   Research a topic deeply with parallel evidence gathering               │
│   Built-in · Required: topic                                              │
│                                                                           │
│   Release Readiness                                                       │
│   Review a release and return blocking issues                             │
│   This project · Required: target                                         │
├────────────────────────────────────────────────────────────────────────────╢
│ ↑↓ navigate · Enter choose · Esc cancel                                   │
╰────────────────────────────────────────────────────────────────────────────╯
```

### 8.3 Narrow layout

Below 80 columns, fields wrap into dedicated lines and remain readable:

```text
╭─ Run a workflow ─────────────╮
│ Search  deep█                │
├──────────────────────────────╢
│ > Deep Research              │
│   Research a topic deeply    │
│   with parallel evidence     │
│   gathering                  │
│   Built-in                   │
│   Required: topic            │
│                              │
│   Release Readiness          │
│   Review a release and       │
│   return blocking issues     │
│   This project               │
│   Required: target           │
├──────────────────────────────╢
│ ↑↓ · Enter choose · Esc      │
╰──────────────────────────────╯
```

The picker must never rely on viewport-scaled font size. Long descriptions and
required names wrap or truncate at a visible boundary without overlapping the
footer or neighboring rows.

### 8.4 Search

Typing filters by canonical name, display name, description, source label, and
required input names using existing case-insensitive selection behavior.
Backspace edits the query. Pasted text is accepted. Up, Down, Page Up, and
Page Down move selection inside the filtered result.

When search has no match:

```text
╭─ Run a workflow ─────────────╮
│ Search  deployment█          │
├──────────────────────────────╢
│ No matching workflows.       │
│ Try another search.          │
├──────────────────────────────╢
│ Esc cancel                   │
╰──────────────────────────────╯
```

### 8.5 Empty registry

When no effective definition exists:

```text
╭─ Run a workflow ─────────────╮
│ No workflows are available.  │
│                              │
│ Create one with:             │
│ /skill:create-workflow       │
├──────────────────────────────╢
│ Esc close                    │
╰──────────────────────────────╯
```

The empty state does not activate the skill and has no selectable fake item.

### 8.6 Selection and cancellation

- `Enter` closes the picker and writes `/workflow:<name> ` into the composer.
- `Enter` does not start a model turn or workflow.
- The cursor is placed after the trailing space so the user can describe the
  task immediately.
- `Esc` closes the picker and returns to an empty composer.
- Picker navigation does not persist recently used state.

## 9. `/workflow:` Completion

The existing slash candidate overlay dynamically includes one candidate per
effective workflow:

```text
/workflow:deep-research     Deep Research: Research a topic deeply...
/workflow:release-check     Release Readiness: Review a release...
```

Rules:

1. Values use `/workflow:<canonical-name>`.
2. Candidates include built-in, user, and trusted project definitions.
3. Candidate generation reads the effective registry; it does not call the
   `Workflow` tool and does not maintain a second cache.
4. Typing after `/workflow:` filters through the existing slash matching path.
5. Choosing a candidate writes `/workflow:<name> ` into the composer.
6. Choosing a candidate does not submit.
7. Static built-in workflow completion generation is deleted.
8. Former `/workflow <name>` completion values are deleted.
9. A registry read failure yields no stale workflow candidates. Bare
   `/workflow` remains available and can present the error when opened.

## 10. Automatic Selection Turn

### 10.1 User flow

```text
User: /workflow Research battery recycling and cite primary sources
  -> Neo resolves the complete effective workflow summary
  -> Neo starts a visible model turn with the original user message
  -> model selects an appropriate saved workflow
  -> model calls Workflow(run_saved), or asks for missing information
  -> existing permission mode and Workflow card handle execution
```

Neo SHALL NOT require the model to call `Workflow(list)` before selecting.
Neo SHALL NOT select a definition through keywords, string patterns, or a
host-side semantic substitute.

### 10.2 Model-visible automatic context

The host adds one system-role message immediately before the submitted user
message. It is turn-local and uses this stable shape:

```text
<neo-workflow-request mode="automatic">
The user explicitly asked to use an existing Neo workflow.
Choose the best matching definition from the complete catalog below.
Do not create a workflow, silently continue as ordinary execution, or choose by
keyword matching alone. If no definition fits, ask whether to create one. If a
definition fits but the task lacks required information, ask for it. After
choosing, use Workflow(run_saved). Use Workflow(show) for the chosen definition
only when its full input schema is needed before a safe run.

<workflow-catalog complete="true">
<workflow name="deep-research" source="Built-in">
<display-name>Deep Research</display-name>
<description>Research a topic deeply with parallel evidence gathering.</description>
<required-inputs>topic</required-inputs>
</workflow>
</workflow-catalog>
</neo-workflow-request>
```

All dynamic text is XML-escaped. The context is not a skill activation, is not
stored in `skill_context`, and does not create a Skill card.

### 10.3 Complete-or-error rule

The automatic context contains every effective summary. Before starting the
model call, Neo checks it against the existing selected model context capacity
using the same token estimation rules as normal turn preparation.

If the complete context cannot fit even after the normal turn preparation
rules, Neo does not submit the model call and shows:

```text
The workflow catalog is too large for the selected model. Remove unused
workflow definitions or choose a model with a larger context window.
```

The original command remains in the composer. Neo SHALL NOT truncate the list,
page only part of it, choose on the user's behalf, or mark a partial catalog as
complete.

### 10.4 No suitable definition

If no workflow matches, the assistant says so plainly and asks whether to
create one. It does not:

- silently execute the task without a workflow;
- silently choose a weak match;
- automatically author a workflow;
- ask the user to read workflow files or schemas.

If the user confirms creation, the model activates `Skill(create-workflow)`
through the existing model-invocable skill path and follows Section 14.

## 11. Named Selection Turn

### 11.1 User flow

```text
User: /workflow:deep-research Research battery recycling
  -> Neo resolves deep-research locally
  -> Neo starts a visible model turn with the original user message
  -> model translates the task into schema-valid arguments
  -> model calls Workflow(run_saved)
```

The host does not parse user prose into JSON and does not launch the runtime
directly.

### 11.2 Model-visible named context

The host adds one system-role message immediately before the user message:

```text
<neo-workflow-request mode="named" name="deep-research">
The user explicitly selected this workflow. Use it unless it clearly cannot
satisfy the request. Translate the natural-language task into arguments that
match the full input schema. Ask for missing required information instead of
guessing. If the selected workflow is clearly unsuitable, explain why and ask
permission before choosing another workflow or continuing without one.

<workflow-definition source="Built-in">
<display-name>Deep Research</display-name>
<description>Research a topic deeply with parallel evidence gathering.</description>
<input-schema>{"type":"object","required":["topic"],"properties":{...}}</input-schema>
</workflow-definition>
</neo-workflow-request>
```

The input schema is the full resolved JSON Schema, serialized deterministically
and XML-escaped. The Lua source, output schema, filesystem path, revision, and
hash are not included because they are not needed to map user intent to input.

### 11.3 Mismatch behavior

The model may not silently switch away from a named workflow. When the selected
definition clearly cannot satisfy the request, it explains the mismatch and
asks the user to choose one of these paths:

- choose another existing workflow;
- create a new workflow;
- continue as ordinary execution.

Only the user's explicit answer authorizes a different path.

## 12. Local Errors

Local grammar and registry errors start no model call and no workflow task.

### 12.1 Missing named task

For `/workflow:<name>` with no task:

```text
Describe what you want this workflow to do.
```

Neo keeps the full input and cursor position in the composer.

### 12.2 Missing name

For `/workflow:`:

```text
Choose a workflow with /workflow.
```

Neo keeps the input. It does not guess a name.

### 12.3 Unknown name

For an unresolved name:

```text
Workflow `deep-reseach` was not found. Did you mean `deep-research`?
```

A suggestion is shown only when one candidate is a reliable unique match under
the existing slash completion ranking. Neo never auto-replaces or runs it.

Without a reliable unique suggestion:

```text
Workflow `unknown` was not found. Use /workflow to choose one.
```

The original input remains in the composer.

### 12.4 Registry failure

Registry failures are shown as local workflow discovery errors. Neo does not
use stale candidates, a built-in fallback list, or a lower-precedence
definition that the registry rejected.

## 13. Permission and Execution

Slash routing adds no approval before the model chooses a workflow. After the
model calls `Workflow`, existing modes apply:

| Mode | Behavior |
| --- | --- |
| `ask` | Existing workflow and child-effect approval behavior remains |
| `auto` | Existing automatic policy remains |
| `yolo` | Existing no-confirmation policy remains |

The slash command does not grant a capability. `Workflow(run_saved)` and
`Workflow(run_inline)` remain ordinary model tool calls. Child effects continue
through the same permission checks as any other workflow run.

The existing Workflow transcript card shows the chosen definition, script when
applicable, run status, task identity, and terminal outcome. This design does
not add a duplicate selection card.

## 14. Creation Path

Workflow use and workflow creation remain separate.

### 14.1 Explicit authoring

The direct authoring entry is:

```text
/skill:create-workflow Create a workflow that reviews release readiness
```

### 14.2 Creation after no match

After an automatic selection turn finds no suitable definition:

```text
assistant: No available workflow fits this task. Should I create a one-off workflow?
user: Yes
assistant -> Skill(create-workflow) -> Workflow(run_inline)
```

The authoring skill produces the smallest sufficient workflow and explains its
plain purpose. Before execution, the existing Workflow presentation displays
the complete script. In `ask`, the existing launch review blocks for approval.
In `auto` and `yolo`, the presentation remains visible but execution follows
the current permission mode without an extra confirmation.

The one-off workflow is not saved by default. After a successful run, the
assistant may ask whether to save it. Saving happens only after explicit user
agreement and uses `Workflow(save)`.

## 15. Failure After Launch

When a selected workflow fails, Neo and the assistant preserve the actual
failure, error summary, Workflow card, and task record. They do not silently:

- switch to another workflow;
- create a replacement workflow;
- rerun with guessed inputs;
- continue as ordinary execution.

The assistant offers three understandable options:

1. revise the inputs or workflow and retry;
2. choose another workflow;
3. explicitly continue without a workflow.

Only the user's choice advances to another path.

## 16. Ordinary Natural-Language Requests

Slash commands are the deterministic entry, not the only entry. For ordinary
requests such as:

```text
Use a workflow to research battery recycling.
```

the model-visible `Workflow` tool guidance SHALL direct the assistant to:

1. call `Workflow(list)` when the user asks to use a workflow but names none;
2. call `Workflow(show)` only when the chosen definition's full input schema is
   needed;
3. call `Workflow(run_saved)` for an existing suitable definition;
4. activate `create-workflow` only for creation, modification, adaptation, or a
   confirmed no-match authoring path.

The current wording that limits `list` or `show` to an explicit request to view
saved workflows SHALL be deleted. The model must understand workflow use as a
product action even when Neo itself is the current repository.

## 17. User Message and Resume

The visible and persisted user message is the complete original slash input:

```text
/workflow:deep-research Research battery recycling
```

The internal workflow turn context:

- is a system-role helper message for the current turn;
- is not substituted for the user message;
- is not rendered as user text;
- is not presented as a skill activation;
- does not alter the durable workflow definition registry;
- may be rebuilt from the persisted original slash request and current
  definition only if a pending turn must be reconstructed before dispatch.

Once the turn has been submitted, normal session events preserve the original
user message and all resulting tool calls/cards. Resume therefore shows what
the user asked and what workflow the model selected without requiring a new
workflow-specific session format.

## 18. Security and Trust

- Effective discovery preserves existing workspace trust and precedence.
- Untrusted project definitions do not enter the picker, candidates, or model
  summary.
- Dynamic text is escaped before insertion into tagged system context.
- Named resolution uses the existing validated `WorkflowName` parser.
- Slash routing never reads workflow source files directly.
- No hidden launch grant or permission bypass is created.
- Registry failure is fail-closed; there is no lower-scope fallback.

## 19. Accessibility and Cross-Platform Behavior

- All visible UI strings in this feature are English.
- Keyboard-only use supports search, navigation, choose, and cancel.
- Selection is expressed by both marker and theme, not color alone.
- Width uses display-cell measurement, not byte length.
- Unicode names and descriptions wrap safely; workflow canonical names retain
  their existing portable grammar.
- Input and rendering behavior is identical on macOS, Linux, and Windows
  terminals.
- No shell command, path separator, Unix signal, or platform-specific file
  behavior is added by this design.

## 20. Retirement Boundary

Delete these internal paths during implementation:

- bare `/workflow` activation of `create-workflow`;
- host-direct named workflow parsing and launch from slash commands;
- JSON parsing from workflow slash arguments;
- slash-specific named launch approval state and handlers that have no other
  caller;
- static built-in workflow completion generation;
- `/workflow <name>` candidate values;
- tests and docs that teach the former slash behavior;
- comments that describe workflow slash as a launch license or authoring alias.

Preserve:

- `/skill:create-workflow` and model `Skill(create-workflow)`;
- all seven `Workflow` actions;
- `WorkflowDefinitionRegistry` resolution, precedence, and save behavior;
- existing permission modes and workflow launch review;
- workflow runtime, tasks, journal, recovery, cards, and `/tasks`;
- four headless `neo workflow` commands.

This is internal code retirement. It does not delete user workflow definitions,
session history, journals, or live tasks.

## 21. Acceptance Scenarios

### Scenario A: browse and fill

Given built-in, user, and project definitions, submitting `/workflow` opens one
searchable list containing the effective winner for every name. Choosing Deep
Research writes `/workflow:deep-research ` and starts nothing.

### Scenario B: automatic selection

Submitting `/workflow Research battery recycling` starts one visible model
turn. The model receives the complete effective summary without calling
`Workflow(list)`, selects a suitable definition semantically, and runs it
through `Workflow(run_saved)`.

### Scenario C: exact named selection

Submitting `/workflow:deep-research Research battery recycling` supplies the
full resolved input schema to the model. The model translates the task and
calls `Workflow(run_saved)` without host-side argument parsing.

### Scenario D: missing information

When a required input cannot be inferred safely, the assistant asks the user.
It does not guess, start a partial run, or expose raw schema maintenance work.

### Scenario E: no match

When no definition fits, the assistant asks whether to create one. It does not
fall back to ordinary execution or author automatically.

### Scenario F: unknown name

Submitting an unknown named command starts no model call and no task, keeps the
input intact, and offers only a reliable unique suggestion or the bare picker.

### Scenario G: completion

Typing `/workflow:` shows all effective definitions using colon-form values.
Project and user definitions appear alongside built-ins. No space-form workflow
candidate remains.

### Scenario H: resume

After a named or automatic run and application restart, the transcript shows
the original slash request and the existing Workflow card/result. No helper
context appears as a user message or Skill card.

### Scenario I: oversized catalog

When the complete effective summary cannot fit the selected model context,
Neo starts no model call, keeps the command, and reports the capacity error. It
does not send a partial catalog.

### Scenario J: permission modes

The same slash request under `ask`, `auto`, and `yolo` reaches the same model
selection behavior. Only the existing tool and child-effect approval policy
differs.

### Scenario K: ordinary prose

For `Use a workflow to research this topic`, the model calls `Workflow(list)`
instead of activating `create-workflow`, reading source, running Cargo, or using
the workflow CLI.

### Scenario L: retired path

The former space-form named JSON input no longer launches directly and is not
offered by completion or documentation.

## 22. Verification Requirements

Implementation evidence must prove:

1. exact grammar boundaries, including `/workflowish`;
2. local missing-name, missing-task, unknown-name, registry-error, and busy-turn
   behavior with composer preservation;
3. dynamic completion from built-in, user, and project definitions;
4. bare picker search, empty state, choose, cancel, and narrow rendering;
5. automatic context includes every effective summary exactly once;
6. named context includes the full input schema and excludes Lua/path/hash data;
7. tagged dynamic text is escaped;
8. oversized catalogs fail before a model call;
9. persisted visible messages keep exact slash text;
10. former host-direct launch code and completion values have no active
    references;
11. model guidance routes existing-workflow intent to discovery/run and
    authoring intent to `create-workflow`;
12. focused tests pass on the touched Rust targets and documentation contains no
    live former syntax.

## 23. Approval Boundary

This specification is complete. Implementation may choose local Rust type and
function names that match existing style, but it may not change:

- the three invocation meanings;
- colon-only named selection;
- model-mediated natural-language argument mapping;
- dynamic effective definition discovery;
- complete-or-error automatic context;
- the picker behavior and English UI;
- local preservation errors;
- permission and resume behavior;
- deletion of the former host-direct slash route;
- preservation of the complete `Workflow` tool.

Any proposal to reintroduce JSON slash arguments, host-side semantic selection,
a static built-in list, a second registry, or silent ordinary execution after
failure requires a new user decision before implementation.
