# Neo Workflow Intent Slash Implementation Plan

Date: `2026-07-30`

Status: `approved specification; ready for delegated implementation`

## Goal

Implement the approved workflow slash design so that:

1. `/workflow` opens a searchable effective-workflow picker;
2. `/workflow <natural-language task>` gives the model the complete effective
   summary and lets the model select an existing definition;
3. `/workflow:<name> <natural-language task>` gives the model the exact
   definition and full input schema;
4. `/skill:create-workflow` remains the separate authoring entry;
5. the former host-direct space-form named launch and JSON slash arguments are
   fully removed;
6. user-visible messages, permission behavior, workflow cards, and resume
   history remain truthful.

Primary requirement:

- `docs/aegis/specs/2026-07-30-workflow-intent-slash-design.md`

## Architecture

Reuse the existing registry, picker plumbing, turn pipeline, model tool, skill
activation, and session events:

```text
composer
   |
   v
workflow slash parser
   |
   +---- bare ----------> effective registry list ----------> workflow picker
   |                                                        Enter fills composer
   |
   +---- automatic -----> complete effective summary ----+
   |                                                      |
   +---- named ---------> resolve + full input schema ----+--> TurnRequest
                                                               |
                                                               v
                                                    turn-local system context
                                                               |
                                                               v
                                                        visible model turn
                                                               |
                                                               v
                                               existing Workflow tool + cards
```

There is one definition registry and one execution path. The slash layer only
parses intent, reads the existing registry, prepares model context, and starts a
normal turn. It does not parse argument JSON, launch `WorkflowRuntime`, or own
workflow state.

## Tech Stack

- Rust 2024, minimum Rust `1.96.1`;
- existing `WorkflowDefinitionRegistry` and registry summary types;
- existing `neo-tui` overlay, theme, text input, width, wrapping, and selection
  primitives;
- existing slash completion matcher and `PickerItem`;
- existing `TurnRequest`, `AgentContext`, `AgentRuntime`, context budget
  estimator, and JSONL user-message events;
- existing `Workflow` and `Skill` tools;
- no new production dependency.

## Baseline and Authority Refs

Requirement authority:

- `docs/aegis/specs/2026-07-30-workflow-intent-slash-design.md`

Current implementation records to preserve where they do not conflict:

- `docs/aegis/adr/ADR-0007-assistant-native-workflow-contract.md`
- `docs/aegis/adr/ADR-0008-workflow-product-surface-contract.md`
- `docs/aegis/baseline/2026-07-27-assistant-native-workflow-contract.md`
- `docs/aegis/baseline/2026-07-28-workflow-product-surface-contract.md`

The approved specification supersedes only their conflicting slash-entry
statements. It does not reopen the workflow runtime, `/tasks`, headless CLI,
tool action set, Lua host, or transcript card decisions.

## Compatibility Boundary

Preserve:

- all seven model-visible `Workflow` actions;
- effective registry precedence and trust behavior;
- paired workflow definitions and save behavior;
- `ask`, `auto`, and `yolo` semantics;
- existing Workflow and Delegate-family cards;
- workflow tasks, journal, recovery, controls, results, and artifacts;
- the four `neo workflow` headless commands;
- `/skill:create-workflow` and model `Skill(create-workflow)`;
- exact persisted user message and normal session resume.

Intentionally break and delete:

- bare `/workflow` authoring-skill activation;
- host-direct space-form named launch;
- workflow JSON parsing in slash commands;
- slash-owned named launch approval state;
- static built-in workflow completion generation;
- space-form named workflow completion values;
- user docs and tests that teach those paths.

No external compatibility exception is authorized. Unknown downstream use is
not evidence for retaining the former behavior.

## TDD Route

- Mode: `off`
- Decision: `skipped`
- Strict authority: `not applicable`
- Test posture: minimum implementation followed by focused post-change
  regression and product acceptance
- Reason: neither the user nor project rules request strict test-first work;
  each non-trivial behavior still receives one focused regression
- Verification: exact package, target, and test names are listed per task; no
  broad package-wide test command is completion evidence

## Scope Check

### Facts

- `slash_commands.rs` currently treats bare `/workflow` as manual
  `create-workflow` activation and space-form arguments as host-direct launch.
- `prompt_completion.rs` currently builds workflow candidates from compiled
  built-ins only and writes values as `/workflow <name>`.
- `WorkflowDefinitionRegistry::list(Effective)` already returns every effective
  resolved definition with name, display name, description, source, and
  required input names.
- `WorkflowDefinitionRegistry::resolve` already returns the full resolved input
  schema for a named definition.
- `PickerState` and `SelectListState` already provide filtering, selection,
  paging, and themed rows, but the existing generic overlays do not provide the
  approved workflow-specific searchable multi-line presentation.
- `TurnRequest` already separates `prompt_display_text` from model content and
  has a dedicated skill context field. Workflow context must not reuse the
  skill field.
- `AgentRuntime` already owns model context budgeting and token estimation.
- `create-workflow` is model-invocable and the `Workflow` tool already exposes
  all seven actions.

### Assumptions

- The current effective registry is the only discovery source needed by the
  picker, completion, and automatic model context.
- A workflow-specific picker may wrap existing list and text primitives without
  introducing a reusable picker framework.
- A turn-local system message appended before the user message is sufficient;
  no workflow-specific session event or persistence schema is needed.
- Existing Workflow cards already show the definition selected by the model and
  the run result, so no new transcript card is needed.

### Unknowns resolved by this plan

- Large catalog handling uses the existing `AgentRuntime` context budget
  estimator with a fresh minimal context containing only the workflow helper
  message and user message. If that minimum request cannot fit, the turn is
  rejected before a provider call. No second estimator is added.
- Busy workflow slash input is not placed in the ordinary follow-up queue. It
  remains in the composer with the approved local status because queued prose
  cannot carry the required turn-local context safely.

## Requirement Ready Check

- Requirement source refs: approved specification sections 1-23
- Goals and scope refs: specification sections 1-4
- User/scenario refs: specification sections 8-17 and 21
- Requirement item refs: grammar, catalog, picker, completion, turn context,
  errors, permissions, creation, failure, resume, and retirement sections
- Acceptance and verification refs: specification sections 21-22
- Open blocker questions: none
- Decision: `ready`

## Baseline Usage Draft

- Required baseline refs: ADR-0007, ADR-0008, 2026-07-27 assistant-native
  baseline, 2026-07-28 product-surface baseline
- Acknowledged before plan refs: all four
- Cited in plan refs: all four
- Missing refs: none
- Decision: `continue`

## Change Necessity

- User-visible need: workflow use must be discoverable, semantic, and separate
  from workflow authoring.
- No-change or documentation-only option: insufficient because current slash
  routing directly launches the host, current completion omits non-built-in
  definitions, and bare workflow activates the wrong skill.
- Why source change is necessary: only code can change the parser, picker,
  dynamic catalog, turn context, capacity check, and deletion of direct launch.
- Minimum change boundary: existing interactive workflow slash modules, one
  focused TUI picker, turn preparation, and model guidance.
- Decision: `code-change`

## Existence Check

- Proposed new surface: workflow-specific searchable picker state.
- Existing reuse candidates: `PickerState`, `SelectListState`, input events,
  overlay rendering, theme, width helpers, and dialog result plumbing.
- Why existing surface is insufficient: prompt completion has no titled
  workflow view or empty state; choice picker has no search query or approved
  multi-line workflow metadata.
- Creation proof: the approved bare command requires search, source, purpose,
  required inputs, empty state, and responsive rendering.
- Entropy impact: one focused state wrapping existing primitives; no general
  framework, dashboard, manager, registry, or persistence.
- Decision: `add-with-proof`

## Architecture Integrity Lens

- Invariant: the registry decides what definitions exist; the model decides
  semantic fit; the Workflow tool decides execution.
- Responsible path: interactive parser -> effective registry -> picker or
  turn-local context -> normal model turn -> Workflow tool.
- Overlap to avoid: a second registry cache, host keyword selection, host JSON
  argument mapping, slash-owned runtime launch, or a second user-message event.
- Higher-level simplification: delete the entire host-direct launch branch and
  feed both automatic and named forms through one normal turn path.
- Retirement falsifier: any active parser, candidate, test, or guide still
  launches a workflow from space-form slash arguments.
- Verdict: proceed with delete-first internal retirement.

## Anti-Entropy Declaration

- Deletion class: internal code retirement and public command behavior code
- Old path: bare authoring activation plus space-form host-direct named launch
- New responsible path: colon grammar plus normal model turn and existing
  Workflow tool
- Preserved behavior: all workflow definition, execution, permission, task,
  card, and recovery capabilities
- Retired behavior: user-authored slash JSON and zero-model host-direct launch
- External boundary touched: yes, interactive slash syntax intentionally breaks
- Source-of-truth data risk: none
- User confirmation required: no; the user explicitly approved clean deletion

Retirement decision:

- Path: `delete-first`
- Why: the former route is internal command behavior with no approved external
  compatibility requirement
- Non-edits: user definitions, sessions, journals, live tasks, runtime, and
  headless commands are untouched

## Complexity Budget

- Artifact class: interactive parser, TUI dialog, turn request, runtime budget
  helper
- High-pressure files: `interactive/mod.rs` is about 2,800 lines,
  `modes/run/mod.rs` about 3,800 lines, `slash_commands.rs` about 850 lines
- Projected pressure: adding all parsing, formatting, and picker logic directly
  to those files would make ownership less clear
- Budget result: `at-risk`
- Planned governance: place pure slash grammar/catalog/context formatting in
  `workflow_slash.rs`; place only workflow picker state/rendering in
  `workflow_picker.rs`; keep host wiring small in existing files

Plan-time complexity check:

- Target files: large interactive controller plus TUI overlay plumbing
- Existing shape: controller already splits slash, turn, input, completion, and
  dialog-result responsibilities into modules
- Owner fit: pure workflow slash logic belongs beside those modules; TUI state
  belongs in `neo-tui::dialogs`
- Add-in-place risk: high for parser/formatter and renderer; low for enum
  registration and one-field plumbing
- Better file boundary: two focused new files, no extra abstraction layer
- Recommendation: add the two focused files and make minimal edits elsewhere

## File Map

### New source files

- `crates/neo-agent/src/modes/interactive/workflow_slash.rs`
  - exact grammar;
  - effective catalog mapping and stable sorting;
  - source labels and required input projection;
  - automatic and named context rendering;
  - reliable unique-name suggestion;
  - pure unit tests.
- `crates/neo-tui/src/dialogs/workflow_picker.rs`
  - picker item/options/result/state;
  - query editing, filtering, selection, paging, empty state;
  - wide/narrow rendering and focused tests.

### Modified host files

- `crates/neo-agent/src/modes/interactive/mod.rs`
  - register `workflow_slash`;
  - add turn-local pending workflow context;
  - add `workflow_context` to `TurnRequest`;
  - enforce idle workflow slash handling before ordinary follow-up queueing.
- `crates/neo-agent/src/modes/interactive/slash_commands.rs`
  - replace current workflow handler with picker/automatic/named routing;
  - delete host-direct launch parsing, approval state, and coordinator launch.
- `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
  - replace static built-in generation with effective registry candidates;
  - change values to `/workflow:<name>`.
- `crates/neo-agent/src/modes/interactive/turn.rs`
  - move pending workflow context into each `TurnRequest` exactly once.
- `crates/neo-agent/src/modes/interactive/dialog_results.rs`
  - consume picker choose/cancel result and update the composer only.
- `crates/neo-agent/src/modes/interactive/input.rs`
  - route workflow picker keyboard input only if the existing rich-dialog route
    cannot consume it without a workflow-specific branch.
- `crates/neo-agent/src/modes/interactive/tests.rs`
  - replace former slash tests with approved behavior regressions.
- `crates/neo-agent/src/modes/run/mod.rs`
  - append workflow context as a turn-local system message before the user;
  - preserve `prompt_display_text` and session user-message events;
  - call the runtime capacity check before provider dispatch.

### Modified TUI files

- `crates/neo-tui/src/dialogs/mod.rs`
  - export workflow picker types.
- `crates/neo-tui/src/shell/overlay.rs`
  - add and render the workflow picker overlay.
- `crates/neo-tui/src/shell/input_dispatch.rs`
  - forward input and expose/take its result.
- `crates/neo-tui/src/shell/dialog_factory.rs`
  - open the workflow picker and return its result.

Do not add another TUI file unless the existing overlay module requires its
normal module registration pattern.

### Modified runtime budget files

- `crates/neo-agent-core/src/runtime/mod.rs`
  - expose one narrow `AgentRuntime` method if required by current module
    privacy.
- `crates/neo-agent-core/src/runtime/context_budget.rs`
  - reuse `ContextBudgetEstimator` to test whether the helper plus user message
    can fit after ordinary history is removed;
  - no new estimator, token formula, config, or retry path.

If `AgentRuntime` already exposes sufficient budget access at implementation
time, omit these core edits and test the same behavior through the existing
method. The required result is fixed; redundant API is forbidden.

### Model guidance and documentation

- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- `crates/neo-agent-core/src/skills/builtin/mod.rs`
- `crates/neo-agent-core/tests/workflow_tool_policy.rs`
- `docs/en/reference/slash-commands.md`
- `docs/zh/reference/slash-commands.md`
- `docs/en/guides/interaction.md`
- `docs/zh/guides/interaction.md`
- `docs/en/guides/workflows.md`
- `docs/zh/guides/workflows.md`
- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

### Completion records after code acceptance

- create `docs/aegis/adr/ADR-0009-workflow-intent-slash-entry.md`;
- create `docs/aegis/baseline/2026-07-30-workflow-intent-slash.md`;
- update `docs/aegis/INDEX.md`;
- record focused evidence under the existing Aegis work pattern only if the
  implementation session already uses that pattern. Do not create bookkeeping
  merely for appearance.

## Shared Type Shapes

These shapes pin responsibilities. Exact visibility and derives may follow the
existing module style.

### Workflow slash parser

```rust
pub(super) enum WorkflowSlashRequest {
    Picker,
    Automatic { task: String },
    Named { name: String, task: String },
}

pub(super) enum WorkflowSlashError {
    MissingName,
    MissingTask { name: String },
}

pub(super) fn parse_workflow_slash(
    prompt: &str,
) -> Option<Result<WorkflowSlashRequest, WorkflowSlashError>>;
```

Required parser rules:

- return `None` for `/workflowish` and prose containing the token;
- exact bare command returns `Picker`;
- whitespace form always returns `Automatic`;
- colon form validates only command shape here; registry name validation occurs
  through `WorkflowDefinitionRegistry::resolve`;
- never parse JSON;
- never infer that the first task word is a definition name.

### Host catalog row

```rust
pub(super) struct WorkflowCatalogItem {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub source_label: &'static str,
    pub required_inputs: Vec<String>,
}
```

Build it on demand from `RegistryDefinitionSummary`. Do not retain it in
controller state after opening the picker or rendering turn context.

### Turn request

```rust
pub(crate) struct TurnRequest {
    // existing fields
    pub workflow_context: Option<String>,
}
```

`workflow_context` is independent from `skill_context`. It is consumed once by
turn preparation, inserted as `AgentMessage::system_text`, and never rendered
as a Skill card or user message.

### Workflow picker

```rust
pub struct WorkflowPickerItem {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub source: String,
    pub required_inputs: Vec<String>,
}

pub struct WorkflowPickerOptions {
    pub items: Vec<WorkflowPickerItem>,
    pub theme: TuiTheme,
}

pub enum WorkflowPickerResult {
    Selected(WorkflowPickerItem),
    Cancelled,
}
```

The picker state owns only query, filtered/selected position, render state, and
result. It does not know the registry, controller, workflow runtime, or
composer.

### Capacity helper

Prefer one boolean method rather than a new error hierarchy:

```rust
impl AgentRuntime {
    pub fn turn_messages_fit_after_compaction(
        &self,
        messages: &[AgentMessage],
    ) -> bool;
}
```

Implementation rules:

1. create a fresh `AgentContext`;
2. append the supplied workflow helper message and user message;
3. call the existing `ContextBudgetEstimator` with the runtime's existing
   configuration and request projection;
4. compare the projected request plus reserved headroom with the smaller known
   effective/absolute cap;
5. return `true` when no model capacity is known, allowing the existing
   provider overflow path to report failure without catalog truncation;
6. do not add a size setting, secondary estimate, retry, or partial-catalog
   mode.

## Task 1: Add Pure Grammar, Catalog, and Context Rendering

Files:

- Create: `crates/neo-agent/src/modes/interactive/workflow_slash.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Test: unit tests inside `workflow_slash.rs`

Why:

One pure module gives the parser, registry projection, ordering, escaping, and
model context a reviewable home instead of expanding the already-large
controller and slash dispatcher.

Change necessity:

- documentation cannot change runtime grammar or build complete context;
- the minimum source boundary is one pure module plus registration;
- no new manager, cache, registry, or state machine is needed.

Implementation steps:

1. Add `mod workflow_slash;` beside the existing interactive modules.
2. Implement `parse_workflow_slash` with the exact shapes in the specification.
3. Implement `effective_workflow_catalog` by calling
   `WorkflowDefinitionRegistry::list(WorkflowListScope::Effective)` once and
   mapping the existing summaries.
4. Map source labels exactly to `Built-in`, `All projects`, and `This project`;
   reject unexpected source kinds.
5. Sort by lowercase display name and then canonical name. Do not add recent
   use or filesystem ordering.
6. Reuse one XML escaping helper for all tagged dynamic text. If the existing
   slash helper can be moved without widening unrelated scope, move it; do not
   keep duplicate escaping functions.
7. Render automatic context with every catalog row exactly once and
   `complete="true"`.
8. Render named context from `ResolvedWorkflowDefinition` with the full input
   schema and without Lua source, path, hash, revision, or output schema.
9. Implement the optional unknown-name suggestion by reusing the existing slash
   ranking. Return a suggestion only when the best result is unique and passes
   the same reliable exact/prefix/segment threshold; do not invent another
   fuzzy heuristic.
10. Add focused tests:
    - `workflow_slash_parser_distinguishes_picker_automatic_named_and_prose`;
    - `workflow_catalog_is_effective_stable_and_public`;
    - `workflow_context_is_complete_escaped_and_mode_specific`.
11. Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::workflow_slash::tests::workflow_slash_parser_distinguishes_picker_automatic_named_and_prose --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::workflow_slash::tests::workflow_catalog_is_effective_stable_and_public --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::workflow_slash::tests::workflow_context_is_complete_escaped_and_mode_specific --exact --nocapture --include-ignored
```

Expected: all three exact tests pass; the context test proves no definition is
missing or duplicated and forbidden internal metadata is absent.

12. Commit only this slice:

```bash
git add crates/neo-agent/src/modes/interactive/workflow_slash.rs crates/neo-agent/src/modes/interactive/mod.rs
git commit -m "feat(workflow): define intent slash requests"
```

Repair track:

- root cause: existing grammar merges use, authoring, and direct launch;
- stable repair: one typed intent parser and one registry-derived projection;
- verification: pure parser/catalog/context tests.

Retirement track:

- do not delete host-direct launch yet because Task 4 first wires the new path;
- no compatibility branch may be added to the new parser.

## Task 2: Build the Searchable Workflow Picker

Files:

- Create: `crates/neo-tui/src/dialogs/workflow_picker.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`

Why:

Bare `/workflow` needs a focused searchable view that shows purpose, source,
and required inputs and remains readable at narrow widths.

Change necessity:

- existing prompt completion lacks the approved title, empty state, and row
  detail;
- existing choice picker lacks search and the approved responsive row layout;
- the smallest sufficient boundary is one focused picker state reusing existing
  text/list/render helpers.

Implementation steps:

1. Add the three public input types from `Shared Type Shapes` and a private
   state that holds `query`, list selection, and result.
2. Reuse `SelectListState` or `PickerState` filtering and navigation. Do not
   copy their selection arithmetic.
3. Handle `Insert`, `Paste`, `Backspace`, Up, Down, Page Up, Page Down, Submit,
   and Cancel through existing `InputEvent` values.
4. Filter across canonical name, display name, description, source, and required
   inputs with existing case-insensitive behavior.
5. Render exact English title, search line, rows, empty registry state, no-match
   state, and footer from specification section 8.
6. At 80 columns or wider, render source and required inputs on one metadata
   line. Below 80, render them on separate lines.
7. Use existing display-cell width and wrapping helpers. Never slice UTF-8 by
   byte index and never allow a row to overlap the footer.
8. Add `OverlayKind::WorkflowPicker`, rendering, input forwarding, result
   access, result take, and `open_workflow_picker` through normal TUI patterns.
9. Do not add registry or controller dependencies to `neo-tui`.
10. Add focused tests:
    - `workflow_picker_filters_and_returns_selected_name`;
    - `workflow_picker_cancel_and_empty_state_are_non_actionable`;
    - `workflow_picker_narrow_rows_wrap_without_overflow`.
11. Run:

```bash
cargo test --package neo-tui --lib -- dialogs::workflow_picker::tests::workflow_picker_filters_and_returns_selected_name --exact --nocapture
cargo test --package neo-tui --lib -- dialogs::workflow_picker::tests::workflow_picker_cancel_and_empty_state_are_non_actionable --exact --nocapture
cargo test --package neo-tui --lib -- dialogs::workflow_picker::tests::workflow_picker_narrow_rows_wrap_without_overflow --exact --nocapture
```

Expected: selection returns a canonical name; empty state cannot submit a fake
item; every rendered line's visible width is within the requested width.

12. Commit only this slice:

```bash
git add crates/neo-tui/src/dialogs/workflow_picker.rs crates/neo-tui/src/dialogs/mod.rs crates/neo-tui/src/shell/overlay.rs crates/neo-tui/src/shell/input_dispatch.rs crates/neo-tui/src/shell/dialog_factory.rs
git commit -m "feat(tui): add searchable workflow picker"
```

Repair track:

- root cause: no existing overlay expresses the approved browse/search journey;
- stable repair: one purpose-built presentation on existing primitives;
- verification: selection, empty, cancel, and width tests.

Retirement track:

- no existing picker is deleted;
- do not refactor unrelated dialogs into the new state.

## Task 3: Make Slash Completion Use the Effective Registry

Files:

- Modify: `crates/neo-agent/src/modes/interactive/prompt_completion.rs`
- Modify: call sites in `crates/neo-agent/src/modes/interactive/slash_commands.rs`
  and `crates/neo-agent/src/modes/interactive/mod.rs` only as required
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

Why:

Users must see all effective definitions under `/workflow:` and no former
space-form candidates.

Change necessity:

- the current completion source is compiled built-ins, so docs alone cannot
  expose user/project definitions;
- the minimum boundary is the existing completion catalog function and its
  registry-aware call sites.

Implementation steps:

1. Remove imports and code used only by `builtin_workflow_completion_items`.
2. Delete `builtin_workflow_completion_items` entirely.
3. Pass `Option<&WorkflowDefinitionRegistry>` into the existing slash catalog
   builder and `session_completion_items`.
4. Read `list(Effective)` and map each summary to value
   `/workflow:<canonical-name>`.
5. Format candidate descriptions as `<display name>: <plain description>`.
   Do not show paths, revisions, hashes, or internal source names.
6. Preserve `/workflow` itself as the static bare command and update its
   description to `Choose or run a workflow`.
7. On registry failure, emit no workflow-name candidates; do not use compiled
   built-ins as fallback.
8. Keep existing slash scoring and filtering. The colon is already a segment
   separator; do not add a workflow-specific matcher.
9. Replace the current built-in completion test with:
   - `slash_completions_include_effective_workflows_in_colon_form`;
   - `slash_completions_remove_space_form_workflows`.
10. Construct temporary built-in, user, and trusted project definitions through
    the existing registry test helpers. Assert effective shadowing yields one
    candidate.
11. Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_include_effective_workflows_in_colon_form --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::slash_completions_remove_space_form_workflows --exact --nocapture --include-ignored
```

Expected: both pass; the first sees all three public sources and one effective
winner, the second finds no value beginning with `/workflow `.

12. Commit only this slice:

```bash
git add crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(workflow): complete colon slash candidates"
```

Repair track:

- root cause: completion bypasses the registry and hardcodes built-ins;
- stable repair: the existing effective list feeds the existing completion
  matcher;
- verification: multi-source and negative former-value tests.

Retirement track:

- old generator: `builtin_workflow_completion_items`;
- deletion trigger: dynamic completion tests pass;
- no fallback or alias retained.

## Task 4: Route Bare, Automatic, and Named Slash Requests

Files:

- Modify: `crates/neo-agent/src/modes/interactive/slash_commands.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs` only if normal rich
  dialog forwarding needs a small registration branch
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

Why:

This task switches the actual user journey and deletes the old launch path.

Change necessity:

- current host code directly parses JSON, resolves, approves, and launches;
- the approved behavior requires picker fill or a normal visible model turn;
- the minimum boundary is slash dispatch, dialog result handling, and controller
  pending context.

Implementation steps:

1. Detect workflow slash forms before the active-turn follow-up guard. When
   busy, leave the prompt untouched and show the exact busy message.
2. Call `parse_workflow_slash` once.
3. For `Picker`:
   - call `effective_workflow_catalog`;
   - clear the submitted `/workflow` text only after successful discovery;
   - map rows to `WorkflowPickerItem`;
   - open the focused picker even for an empty list.
4. On picker selection, close it and set composer text to
   `/workflow:<name> ` with cursor at the end. Do not submit.
5. On picker cancel, close it and set the composer to empty.
6. For `Automatic`:
   - render the complete automatic context;
   - submit the original slash text as a normal visible user turn;
   - place the rendered context in `pending_workflow_context` for the next
     `TurnRequest` only.
7. For `Named`:
   - resolve through the existing registry;
   - render the named context from the resolved definition;
   - submit the original slash text as a normal visible user turn;
   - never parse the task into JSON in the host.
8. For missing name, missing task, unknown name, registry failure, or busy turn:
   - start no model call and no workflow task;
   - do not call `clear_submitted_prompt`;
   - preserve the current prompt and cursor;
   - show the exact English message from specification section 12.
9. Delete all slash-only host-direct code after the new paths compile:
   - `launch_named_workflow_slash`;
   - `execute_named_workflow_launch`;
   - `parse_named_workflow_slash_args`;
   - `PreparedNamedWorkflowLaunch`;
   - `PendingNamedWorkflowLaunch`;
   - named slash approval presentation/options;
   - controller fields and approval response branches used only by that path.
10. Keep general workflow approvals used by model `Workflow` calls.
11. Replace former behavior tests with:
    - `bare_workflow_slash_opens_picker_and_selection_only_fills_composer`;
    - `automatic_workflow_slash_starts_visible_model_turn_with_complete_context`;
    - `named_workflow_slash_starts_visible_model_turn_with_full_schema`;
    - `workflow_slash_local_errors_preserve_composer_and_start_nothing`;
    - `workflow_slash_is_rejected_while_busy_without_queueing_as_prose`;
    - retain/update `workflowish_is_not_workflow`.
12. Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::bare_workflow_slash_opens_picker_and_selection_only_fills_composer --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::automatic_workflow_slash_starts_visible_model_turn_with_complete_context --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::named_workflow_slash_starts_visible_model_turn_with_full_schema --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_slash_local_errors_preserve_composer_and_start_nothing --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_slash_is_rejected_while_busy_without_queueing_as_prose --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflowish_is_not_workflow --exact --nocapture --include-ignored
```

Expected: exact user messages reach the fake model only for automatic/named
success; all local error counters remain zero; bare choose starts no turn.

13. Run lingering-reference checks:

```bash
rg -n "launch_named_workflow_slash|execute_named_workflow_launch|parse_named_workflow_slash_args|PreparedNamedWorkflowLaunch|PendingNamedWorkflowLaunch" crates/neo-agent/src
rg -n 'format!\("/workflow \{\}' crates/neo-agent/src
```

Expected: both commands return no matches.

14. Commit only this slice:

```bash
git add crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(workflow): route slash intent through model turns"
```

Repair track:

- root cause: slash owns execution instead of user intent preparation;
- stable repair: slash prepares a normal model turn and existing Workflow tool
  owns execution;
- verification: picker, automatic, named, local error, busy, and grammar tests.

Retirement track:

- old owner: slash-specific JSON parser, launch coordinator adapter, and
  approval state;
- active status after this task: deleted;
- reintroduction trigger: none without a new user decision.

## Task 5: Inject Workflow Context and Enforce Complete-or-Error Capacity

Files:

- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/turn.rs`
- Modify: `crates/neo-agent/src/modes/run/mod.rs`
- Modify if necessary: `crates/neo-agent-core/src/runtime/mod.rs`
- Modify if necessary: `crates/neo-agent-core/src/runtime/context_budget.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`
- Test if core helper added: unit tests in `context_budget.rs`

Why:

The model needs complete workflow guidance in system role while the transcript
and session retain the exact user slash text. Oversized catalogs must fail
before a partial provider request.

Change necessity:

- `skill_context` cannot represent workflow selection without producing the
  wrong semantics and card;
- the normal prompt content cannot safely hide system guidance inside the user
  message;
- the smallest boundary is one optional `TurnRequest` field and one existing
  budget check.

Implementation steps:

1. Add `workflow_context: Option<String>` to `TurnRequest`, defaulting to
   `None`.
2. Add `pending_workflow_context: Option<String>` to the interactive controller
   and initialize it to `None`.
3. In `turn.rs`, take the pending value exactly once into the next request. If
   turn startup fails before dispatch, clear it; never leak it into a later
   ordinary prompt.
4. Thread the field through `run_turn_interactive`, streaming preparation, and
   existing-session preparation beside `skill_context`, with its own name.
5. Build `AgentMessage::system_text(workflow_context)` and the exact user
   `AgentMessage` before dispatch.
6. Ask `AgentRuntime::turn_messages_fit_after_compaction` whether those two
   messages can fit in a fresh minimal context under the existing model budget.
7. If false, return the exact user-facing capacity message and make the
   controller restore the original slash text. Do not call the provider.
8. If true, append the workflow system message to `AgentContext` immediately
   before running the user message. Do not persist it as a user event or Skill
   event.
9. Keep `prompt_display_text` equal to the original slash input so local and
   replayed transcript text is exact.
10. Implement the runtime helper only if current privacy prevents direct use of
    `ContextBudgetEstimator`. The helper uses a fresh context and existing
    estimator; do not add any token constants or settings.
11. Add focused tests:
    - `workflow_turn_context_is_system_role_and_user_slash_is_persisted_exactly`;
    - `oversized_workflow_catalog_starts_no_provider_call`;
    - `workflow_context_is_consumed_once_and_never_becomes_skill_context`.
12. If a core helper was added, add:
    - `turn_messages_fit_after_compaction_uses_existing_budget_and_headroom`.
13. Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_turn_context_is_system_role_and_user_slash_is_persisted_exactly --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::oversized_workflow_catalog_starts_no_provider_call --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_context_is_consumed_once_and_never_becomes_skill_context --exact --nocapture --include-ignored
```

If the core helper exists, also run:

```bash
cargo test --package neo-agent-core --lib -- runtime::context_budget::tests::turn_messages_fit_after_compaction_uses_existing_budget_and_headroom --exact --nocapture
```

Expected: fake provider sees one system helper then exact user text; session
replay contains exact user text but no workflow helper/Skill event; tiny-window
test makes zero provider calls.

14. Commit only this slice:

```bash
git add crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/turn.rs crates/neo-agent/src/modes/run/mod.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/src/runtime/mod.rs crates/neo-agent-core/src/runtime/context_budget.rs
git commit -m "feat(workflow): attach complete slash turn context"
```

Stage only core files that were actually changed.

Repair track:

- root cause: no non-skill host field exists for workflow turn guidance;
- stable repair: one turn-local system field plus existing budget calculation;
- verification: role, persistence, one-shot, and provider-call tests.

Retirement track:

- no old persistence format is created or migrated;
- no second context estimator or partial catalog path may remain.

## Task 6: Correct Model Guidance and User Documentation

Files:

- Modify: `crates/neo-agent-core/src/tools/workflow.rs`
- Modify: `crates/neo-agent-core/src/skills/builtin/create-workflow.md`
- Modify: `crates/neo-agent-core/src/skills/builtin/mod.rs`
- Modify: `crates/neo-agent-core/tests/workflow_tool_policy.rs`
- Modify: the eight English/Chinese guide/reference files in `File Map`

Why:

Ordinary prose must route to existing workflow discovery, while creation alone
routes to `create-workflow`. User docs must teach the same three slash forms.

Change necessity:

- current tool and docs still describe the former slash route;
- model-first correctness depends on model-visible wording, not source comments;
- the minimum boundary is existing tool description, skill description/body,
  policy tests, and current public guides.

Implementation steps:

1. Rewrite the `Workflow` tool description to state plainly:
   - user asks to use a workflow without a name -> `list`, choose, optional
     `show`, then `run_saved`;
   - user names a saved workflow -> optional `show` only when needed, then
     `run_saved`;
   - create/change/adapt/one-off authoring -> `Skill(create-workflow)` then the
     appropriate Workflow action;
   - do not use source, Cargo, Bash/Terminal CLI, or ask for slash capability.
2. Remove wording that limits list/show to an explicit request merely to view
   saved workflows.
3. Tighten the `create-workflow` discovery description: it is for authoring,
   modification, adaptation, or confirmed no-match creation; it is not the
   first action for existing-workflow use.
4. Keep every host API and authoring capability in the skill. Do not shorten or
   remove Lua, schemas, delegate/swarm, verification, save, or run guidance.
5. Update tests to assert both positive routing terms and negative former
   mandatory behavior.
6. Rewrite public slash docs to show only:

```text
/workflow
/workflow <natural-language task>
/workflow:<name> <natural-language task>
/skill:create-workflow <authoring request>
```

7. Include the picker behavior, local errors, permission behavior, and examples
   in both languages. Product UI strings remain English even in Chinese docs.
8. Remove all live documentation of space-form named JSON and bare authoring.
   Historical `docs/aegis` records are not rewritten.
9. Add or update focused tests:
   - `workflow_tool_guidance_routes_use_to_list_and_authoring_to_skill`;
   - `create_workflow_builtin_is_authoring_only_without_capability_loss`.
10. Run:

```bash
cargo test --package neo-agent-core --test workflow_tool_policy -- workflow_tool_guidance_routes_use_to_list_and_authoring_to_skill --exact --nocapture
cargo test --package neo-agent-core --lib -- skills::builtin::tests::create_workflow_builtin_is_authoring_only_without_capability_loss --exact --nocapture
```

Expected: both tests pass and still assert all seven Workflow actions remain
available/guided.

11. Run live-doc negative scans:

```bash
rg -n -- '/workflow <name>|/workflow [^:[:space:]]+ \{|host-direct|Bare: activate|Activates `create-workflow`' docs/en docs/zh README.md crates/neo-agent-core/src/skills/builtin/create-workflow.md
```

Expected: no match describing a live former behavior. Natural-language
historical explanation outside those current docs is out of scope.

12. Commit only this slice:

```bash
git add crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/skills/builtin/create-workflow.md crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/tests/workflow_tool_policy.rs docs/en/reference/slash-commands.md docs/zh/reference/slash-commands.md docs/en/guides/interaction.md docs/zh/guides/interaction.md docs/en/guides/workflows.md docs/zh/guides/workflows.md docs/en/reference/tools.md docs/zh/reference/tools.md
git commit -m "docs(workflow): separate use from authoring"
```

Repair track:

- root cause: model and human guidance conflate discovery/use with authoring;
- stable repair: one intent table repeated only at model and human consumption
  boundaries;
- verification: model policy tests and live-doc scans.

Retirement track:

- former guidance is deleted rather than annotated as legacy;
- no compatibility wording remains in current user docs.

## Task 7: Integrated Acceptance and Decision Records

Files:

- Create: `docs/aegis/adr/ADR-0009-workflow-intent-slash-entry.md`
- Create: `docs/aegis/baseline/2026-07-30-workflow-intent-slash.md`
- Modify: `docs/aegis/INDEX.md`
- Modify source/tests only to close failures demonstrated by the exact
  acceptance commands; do not broaden scope.

Why:

The final slice proves the complete human-to-model-to-tool path, confirms old
logic is gone, and records the new current decision without rewriting history.

Change necessity:

- unit slices cannot alone prove picker selection, model context, persistence,
  permission routing, and old-path retirement together;
- the minimum completion boundary is focused integration evidence plus current
  ADR/baseline sync.

Implementation steps:

1. Run the exact primary acceptance tests from Tasks 1-6 again at current HEAD.
2. Add one end-to-end fake-model regression:
   `workflow_intent_slash_end_to_end_selects_runs_and_persists`.
3. The regression must exercise:
   - picker selection fills colon form without a model call;
   - named submission sends full schema and exact visible text;
   - fake model issues `Workflow(run_saved)`;
   - existing Workflow tool produces a task/card event;
   - resume replay shows the exact slash request;
   - no Skill activation occurs for existing-workflow use.
4. Add one automatic no-match fake-model regression:
   `workflow_intent_slash_no_match_asks_before_authoring`.
5. Assert no Workflow run and no Skill activation occurs before the simulated
   user confirms creation. After confirmation, assert the model may invoke
   `Skill(create-workflow)` through its existing path.
6. Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_intent_slash_end_to_end_selects_runs_and_persists --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::workflow_intent_slash_no_match_asks_before_authoring --exact --nocapture --include-ignored
```

Expected: both pass with deterministic fake clients and temporary workflow
roots; no test touches the user's real workflow directories.

7. Run source retirement scans:

```bash
rg -n "launch_named_workflow_slash|execute_named_workflow_launch|parse_named_workflow_slash_args|PreparedNamedWorkflowLaunch|PendingNamedWorkflowLaunch|builtin_workflow_completion_items" crates
rg -n '"/workflow [^"]+"' crates/neo-agent/src/modes/interactive
```

Expected: no active former symbol or space-form generated value. Test strings
that explicitly assert rejection may remain only when clearly named as retired
behavior.

8. Run format checks on touched Rust files only:

```bash
rustfmt --check --edition 2024 crates/neo-agent/src/modes/interactive/workflow_slash.rs crates/neo-agent/src/modes/interactive/slash_commands.rs crates/neo-agent/src/modes/interactive/prompt_completion.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/turn.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/run/mod.rs crates/neo-tui/src/dialogs/workflow_picker.rs crates/neo-tui/src/dialogs/mod.rs crates/neo-tui/src/shell/overlay.rs crates/neo-tui/src/shell/input_dispatch.rs crates/neo-tui/src/shell/dialog_factory.rs crates/neo-agent-core/src/tools/workflow.rs crates/neo-agent-core/src/skills/builtin/mod.rs crates/neo-agent-core/src/runtime/context_budget.rs crates/neo-agent-core/src/runtime/mod.rs
```

Remove unchanged optional core paths from the command if Task 5 did not edit
them.

9. Run whitespace checks:

```bash
git diff --check
git diff --cached --check
```

10. On macOS, run the picker width tests and interactive binary tests natively.
For Windows and Linux, rely on remote CI unless the implementation owner has
explicit safe access to the configured machines. Do not claim native evidence
that was not actually run.
11. Create ADR-0009 with decision, rejected alternatives, preserved capability,
   retirement boundary, and evidence links. State that it supersedes only the
   slash-entry sections of ADR-0007 and ADR-0008.
12. Create the dated baseline containing the three invocation forms, dynamic
   effective discovery, model context behavior, permission/resume behavior, and
   removed path.
13. Add the spec, plan, ADR, and baseline to `docs/aegis/INDEX.md` in newest-first
   order if the spec/plan entries were not already added by the documentation
   commit.
14. Commit the final slice:

```bash
git add crates/neo-agent/src/modes/interactive/tests.rs docs/aegis/adr/ADR-0009-workflow-intent-slash-entry.md docs/aegis/baseline/2026-07-30-workflow-intent-slash.md docs/aegis/INDEX.md
git commit -m "test(workflow): accept intent slash entry"
```

Include only any additional source file whose change is directly required by a
failed acceptance test and explain that inclusion in the commit message body.

Repair track:

- root cause closure: one end-to-end route now covers human selection, model
  translation, existing tool execution, and resume;
- verification: deterministic integrated tests plus deletion scans and focused
  formatting.

Retirement track:

- expected retired behavior: old space-form direct launch is rejected;
- stale internal consumers: migrate within the task, never restore an alias;
- external compatibility: no exception approved;
- persistent data: untouched.

## Verification Matrix

| Requirement | Primary evidence |
| --- | --- |
| Exact three-form grammar | Task 1 parser test |
| Bare searchable picker | Task 2 picker tests + Task 4 host test |
| All effective completion candidates | Task 3 multi-source test |
| Colon form only | Task 3 negative test + Task 7 scan |
| Automatic semantic context | Task 1 context test + Task 4 fake-model test |
| Named full schema | Task 1 context test + Task 4 fake-model test |
| No host JSON mapping | Task 4 deletion scan |
| Local errors preserve composer | Task 4 local error test |
| Busy slash is not ordinary prose | Task 4 busy test |
| Complete-or-error capacity | Task 5 tiny-window test |
| Exact visible and persisted message | Task 5 persistence test |
| No Skill card for use | Task 5 one-shot test + Task 7 end-to-end test |
| Ordinary prose discovers existing definitions | Task 6 model guidance test |
| Authoring capability preserved | Task 6 skill test |
| Permission behavior unchanged | Task 7 end-to-end path through existing tool |
| Former route removed | Task 4 and Task 7 scans |
| Current docs aligned | Task 6 live-doc scan |
| Cross-platform source behavior | portable code review + CI; report actual native proof only |

## Plan Pressure Test

- Owner and retirement: registry/model/tool owners remain; slash-owned launch is
  deleted.
- Architecture integrity: no host semantic selection, second registry, second
  runtime, or duplicate transcript card.
- Verification scope: each boundary has a focused test and old symbols have
  negative scans.
- Task executability: every task names exact files, actions, commands, expected
  results, and commit boundary.
- Pressure result: `proceed`.

## Plan Self-Review

- Specification coverage: every specification section maps to Tasks 1-7 or an
  explicit preserved/non-goal boundary.
- Placeholder scan: no deferred implementation placeholders.
- Type consistency: parser, catalog, picker, turn, and budget shapes align
  across tasks.
- Compatibility: runtime, tool actions, definitions, permissions, cards,
  sessions, `/tasks`, and headless CLI are preserved.
- Change necessity: each task states why docs/no-change is insufficient and
  names the minimum source boundary.
- Existence check: only the workflow picker is new, with evidence; all other
  behavior reuses existing owners.
- Complexity: pure logic and TUI rendering leave the large controller files;
  no speculative framework is added.
- Architecture: execution remains in `Workflow`; selection remains in the
  model; discovery remains in the registry.
- Verification: all commands are exact package + target + test filters or
  explicit text/format scans.
- Retirement: old parser, approval state, launch adapter, static completion,
  tests, and current docs are deleted without aliases.

## Execution Readiness View

- Intent Lock: workflow use, automatic choice, exact named choice, and authoring
  are four distinct understandable intents.
- Scope Fence: interactive slash/completion/picker/turn context/model guidance
  only; runtime, `/tasks`, CLI, engine, and cards stay unchanged.
- Baseline Lock: approved 2026-07-30 specification plus current workflow ADRs
  and baselines where non-conflicting.
- Approved Behavior: three invocation forms, dynamic effective discovery,
  complete model context, local preservation errors, existing permissions.
- Owner Constraints: registry discovers, model selects/maps, Workflow executes,
  TUI renders.
- Compatibility Boundary: preserve all capability; intentionally break only the
  former interactive slash syntax and direct-launch behavior.
- Retirement Boundary: delete host JSON/direct launch/static candidates and
  their live docs/tests; never delete workflow data or history.
- Task Batches: Tasks 1-3 establish pure model/picker/completion; Tasks 4-5
  switch routing and context; Tasks 6-7 align guidance and prove acceptance.
- Test Obligations: exact tests per task, negative source/doc scans, touched-file
  formatting, truthful platform evidence.
- Review Gates: review after Task 3 before deleting the old path; review after
  Task 5 before changing docs; final review after Task 7.
- Drift Rules: if implementation requires a second registry/runtime, host
  semantic selection, hidden JSON mapping, partial catalogs, or capability
  removal, stop and return to the user.
- Evidence Required Before Completion: exact test outputs, zero-match retirement
  scans, current-doc scans, `git diff --check`, and commit list.
- Advisory Boundary: this view guides execution; passing tests and final review
  still determine completion.

## Risks and Stop Conditions

1. **Context role drift**: stop if workflow guidance can only be inserted by
   misusing `skill_context` or altering visible user content. Add the dedicated
   turn field as planned.
2. **Partial catalog temptation**: stop if a fixed candidate cap or truncation
   is proposed. The approved behavior is complete or error.
3. **Host semantic selection**: stop if keywords, regular expressions, or a
   local ranking are proposed to choose the workflow. Host ranking is allowed
   only for UI filtering and a reliable typo suggestion.
4. **Compatibility growth**: stop if an alias or fallback for the former slash
   launch is proposed without new active external dependency evidence and user
   approval.
5. **Capability loss**: stop if any `Workflow` action, authoring guidance, Lua
   host API, permission behavior, task control, or card is removed.
6. **Picker framework growth**: stop if implementation starts generalizing all
   dialogs. Build only the workflow picker on existing primitives.
7. **Persistence expansion**: stop if a new session event or workflow state file
   is proposed. Exact user text plus existing tool/card events are sufficient.
8. **Cross-platform regression**: stop if code introduces shell commands,
   platform paths, or byte-width slicing.

## Completion Definition

Implementation is complete only when:

- all three workflow invocation forms behave exactly as specified;
- bare picker and colon completion use all effective definitions;
- automatic/named forms start visible model turns with correct system context;
- local errors and busy state preserve input and start nothing;
- large catalogs are complete or rejected before provider dispatch;
- existing workflow use does not activate `create-workflow`;
- authoring still retains full skill and Workflow capability;
- exact user slash text survives session replay;
- former host-direct launch and static candidates are absent from active source,
  tests, and current user docs;
- focused verification passes and platform evidence is reported truthfully;
- each logical task is committed separately.

## Handoff Prompt

```text
Implement the approved Neo Workflow Intent Slash plan in
/Users/chenyuanhao/Workspace/neo.

Authority, in order:
1. docs/aegis/specs/2026-07-30-workflow-intent-slash-design.md
2. docs/aegis/plans/2026-07-30-workflow-intent-slash.md
3. current AGENTS.md

Do not redesign the feature. Execute Tasks 1-7 in order and make one verified
commit per task. The worktree may contain other users' changes: never revert,
stash, reset, restore, clean, amend, or rewrite them. Stage only your task files.

Non-negotiable behavior:
- /workflow opens the searchable picker.
- /workflow <natural-language task> gives the model the complete effective
  catalog and lets the model choose.
- /workflow:<name> <natural-language task> gives the model the exact definition
  and full input schema.
- /skill:create-workflow is authoring.
- delete the former host-direct space-form named/JSON route completely.
- keep every Workflow action, runtime feature, permission mode, task behavior,
  workflow/Delegate card, Lua API, and headless command.
- no second registry/runtime/task system, no host keyword selection, no partial
  catalog, no compatibility alias, and no hidden host argument parser.

Use CodeGraph before targeted source reading. Reuse
WorkflowDefinitionRegistry::list(Effective), resolve, existing TUI primitives,
TurnRequest/run preparation, ContextBudgetEstimator, Workflow, and Skill.
Do not add a dependency. Do not modify .references.

Run only the exact verification commands named by each task. If an exact test
name changes to match the final module path, preserve its stated assertion and
record the exact command used. Do not replace focused proof with broad cargo
test. At Task 7, run retirement scans, touched-file rustfmt checks, and
git diff --check.

Stop and report before continuing if any requirement would force capability
loss, a second source of truth, partial workflow summaries, host semantic
selection, a new persistence format, or restoration of the former slash route.

At handoff, report:
- commit hash and message for every task;
- exact tests and results;
- retirement scan results;
- files changed;
- any skipped native platform evidence;
- residual risks only, without claiming remote CI is green unless verified.
```
