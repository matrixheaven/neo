# Interactive Preflight and Skill Creation Design

## Goal

Make Neo's interaction-sensitive workflows explicit before they start.

Some workflows can run well in `auto` mode. Others need the user to answer a
question before the agent can produce a good result. The bad version of this is
an agent that starts coding, works for three minutes, then blocks on
`AskUserQuestion` while the user is sleeping or playing a game. The user comes
back hours later and discovers the run stopped almost immediately. The sky has
fallen, and not in a useful way.

Neo should preserve the useful spirit of "let the AI vibe code": do not surprise
block a long-running workflow midway through execution. If a workflow might need
interactive clarification, Neo should surface that fact before the model turn
starts. The user can switch to `ask`, continue with best-effort assumptions when
that is safe, or cancel before any work begins.

This spec extends the existing `/init` auto-mode preflight into a canonical
interactive preflight contract and uses it for two skill workflows:

- `/skill:self-evo`
- `/skill:create-skill`

It also improves both built-in skills so they produce higher-quality durable
skills and reload Neo's skill store after creation.

## Current State

`/init` already has an auto-mode preflight. The implementation uses a choice
picker and stores `/init` state in `pending_init_instruction`. The behavior is
right for `/init`, but the abstraction is still init-shaped:

- `InitPreflight`
- `init_preflight()`
- `preflight:init:*` choice ids
- `init_command::preflight_decision`
- `pending_init_instruction`

Skill activation currently goes through inline `/skill:` parsing. The slash
handler expands the skill body into pending skill context, but it does not have a
workflow-level preflight step. Built-in `self-evo` exists, but with no argument
it defaults toward the current session rather than requiring the user to choose
the distillation scope. There is a `CreateSkill` tool, but no user-facing
`/skill:create-skill` built-in skill that guides the model to author a complete
Neo-format skill with verification.

`CreateSkill` already writes skill files under `~/.neo/skills/<name>/SKILL.md`
and can reload the shared skill store. This design keeps that tool as the only
writer for generated skills.

## Product Principles

Interactive preflight is a start-of-workflow contract, not an excuse for the
agent to interrupt an unattended run later.

`auto` means "do not stop for user questions during execution." Neo must not
silently change `auto` to `ask`. If Neo recommends switching to `ask`, the user
must choose it in a local dialog before the model turn starts.

Not every workflow deserves a question. If a question is merely helpful, the
workflow may offer "stay Auto and run best effort." If a question is required to
avoid producing junk or mutating durable state with guessed requirements, the
workflow must fail fast or open a preflight dialog before starting; it must not
pretend best-effort output is acceptable.

The preflight mechanism is generic. New workflows declare their interaction
needs through data, not through slash-command-specific hardcoded branches.

## Non-Goals

This design does not add a non-interactive CLI skill-creation command.

This design does not allow the model to change permission mode by itself.

This design does not make all `AskUserQuestion` calls available in `auto` mode.
`auto` remains non-interactive during execution.

This design does not introduce hosted skill sharing, marketplaces, profile sync,
or remote collaboration.

## Interactive Preflight Contract

Add a reusable preflight module for interactive TUI workflows. The exact file
placement should follow the existing interactive-mode module layout, but the
types should not mention `/init`.

Suggested shape:

```rust
pub(super) struct InteractivePreflightSpec {
    pub(super) workflow_id: WorkflowId,
    pub(super) title: String,
    pub(super) body: String,
    pub(super) recommended: PreflightChoice,
    pub(super) alternate: Option<PreflightChoice>,
    pub(super) cancel: PreflightChoice,
    pub(super) auto_mode_policy: AutoModePolicy,
}

pub(super) enum AutoModePolicy {
    /// The workflow does not need a special auto-mode preflight.
    None,
    /// Clarification can improve output, but best-effort auto execution is valid.
    OptionalClarification {
        best_effort_note: String,
    },
    /// The workflow cannot produce acceptable output without user input.
    RequiredClarification {
        missing_input: String,
    },
}

pub(super) struct PreflightChoice {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) description: String,
    pub(super) action: PreflightAction,
}

pub(super) enum PreflightAction {
    SwitchPermissionMode(PermissionMode),
    ContinueAutoBestEffort,
    Cancel,
}
```

The actual implementation can use slimmer names, but the contract must preserve
these semantics:

- every choice id is generated from the workflow id;
- the recommended choice may switch live permission mode to `ask`, but only
  after the user selects it;
- optional workflows may include a `ContinueAutoBestEffort` alternate;
- required workflows must not include a continue-auto choice unless they also
  collect all required input locally before the model turn starts;
- cancel clears the submitted prompt and starts no model turn;
- resolving a preflight choice starts exactly one continuation, then clears the
  pending continuation state.

## Pending Workflow Continuations

Replace init-specific pending state with a generic pending continuation:

```rust
pub(super) enum PendingInteractiveWorkflow {
    Init {
        instruction: String,
    },
    Skill {
        directives: InlineSkillDirectives,
        auto_mode_best_effort: bool,
    },
}
```

The generic preflight handler should:

1. Read the selected choice.
2. Apply the selected local action, such as `SwitchPermissionMode(Ask)`.
3. Start the pending continuation.
4. Inject a workflow-specific best-effort note only when the selected choice was
   `ContinueAutoBestEffort`.
5. Clear pending state on selected choice or cancellation.

This avoids parallel special cases such as `pending_init_instruction`,
`pending_self_evo_instruction`, and `pending_create_skill_instruction`.

## Slash Command Dispatch

Slash dispatch should keep the existing order, but skill activation gets one
additional decision point.

When `/skill:` directives are parsed:

1. Determine whether the invocation set contains an interaction-sensitive
   built-in skill.
2. If not, activate the skill directives as today.
3. If yes and the current permission mode is not `auto`, activate as today.
4. If yes and permission mode is `auto`, open the generic preflight instead of
   starting the model turn.

The first version should support one preflighted skill workflow per submitted
prompt. If the prompt contains multiple `/skill:` directives and more than one
of them requires preflight, Neo should show a concise status error asking the
user to run one interactive skill workflow at a time. This is simpler and safer
than trying to merge multiple workflow contracts.

## `/skill:self-evo` Improvements

`self-evo` distills recent work into one or more durable skills. Because it can
write persistent instructions that affect future agent behavior, it must not
guess its scope when the user gives no argument.

### Invocation Rules

Supported examples:

```text
/skill:self-evo current
/skill:self-evo 7
/skill:self-evo session_abc123
/skill:self-evo 019c6e27-e55b-73d1-87d8-4e01f1f75043
/skill:self-evo topic:prompt-cache
```

No-argument invocation is not a scope. It means "ask me what to distill."

If `/skill:self-evo` is invoked without arguments:

- in `ask` or `yolo`, the skill must use `AskUserQuestion` before summarizing;
- in `auto`, Neo must open a required-clarification preflight before the model
  turn starts;
- the auto-mode preflight must not offer best-effort continuation unless Neo
  adds a local scope picker and passes a concrete scope into the workflow.

Suggested user-facing preflight copy:

- Title: `Choose self-evo scope?`
- Body: `self-evo writes reusable skills. It needs a scope before it can safely distill recent work.`
- Recommended: `Switch to Ask and choose scope`
- Cancel: `Cancel`

The no-argument workflow should ask for scope with structured options. The
minimum useful options are:

- current session;
- recent sessions by day count;
- specific session id or topic.

If the user selects "recent sessions" or "specific session/topic", the model may
ask a follow-up question for the day count, session id, or topic. It should not
proceed until the scope is concrete.

### Skill Authoring Rules

`self-evo` should generate a skill only when it finds a concrete repeatable
workflow, recovery pattern, or decision rule. It should not create a skill for
trivial facts, ordinary project guidance already covered by `AGENTS.md`, or one
off conversational context.

Each generated skill must:

- use the current Neo skill format;
- include concise frontmatter fields: `name`, `description`, and `type`;
- include clear activation guidance in the description or first section;
- include arguments or placeholders only when future invocations need them;
- include a `## Verify` section explaining how to check that the skill works;
- be written through `CreateSkill`, not by direct file writes.

After creating skills, the workflow must rely on the `CreateSkill` reload path
and then verify the skills are visible to Neo's skill store. A narrow
verification can be `ListSkills` with a check for the created names.

## `/skill:create-skill` Built-In Skill

Add a new built-in prompt skill named `create-skill`.

The purpose is to let the user say what capability they want, then have Neo
create a valid, verified skill in the currently supported Neo format.

### Manifest

The built-in skill should live beside other built-ins under
`crates/neo-agent-core/src/skills/builtin/` and be included by the built-in skill
loader.

Recommended frontmatter:

```yaml
---
name: create-skill
description: Create a Neo skill from the user's requirements, including verification guidance.
type: prompt
disableModelInvocation: true
---
```

`disableModelInvocation: true` is required. Skill creation is a user-directed
workflow, not something the model should auto-trigger.

### Invocation Rules

Supported examples:

```text
/skill:create-skill make a skill for reviewing Rust panic paths
/skill:create-skill a docs parity checker that keeps zh/en guides aligned
/skill:create-skill
```

If the user provides an instruction after `/skill:create-skill`, the skill uses
that instruction as the requirement and may ask targeted follow-up questions
only when essential.

If the user invokes `/skill:create-skill` with no instruction:

- in `ask` or `yolo`, the skill must call `AskUserQuestion` before drafting;
- in `auto`, Neo must open a required-clarification preflight before the model
  turn starts;
- the auto-mode preflight must not offer best-effort continuation, because
  there is no meaningful skill requirement to infer.

Suggested user-facing preflight copy:

- Title: `Describe the skill to create?`
- Body: `create-skill writes a persistent skill. It needs your requirement before it can create one safely.`
- Recommended: `Switch to Ask and describe it`
- Cancel: `Cancel`

### Created Skill Requirements

The generated skill must match Neo's current supported skill format. It must not
emit obsolete skill shapes, compatibility aliases, or copied instructions from
other agents unless Neo supports them.

Each created skill must include:

- a valid lowercase skill name using Neo's portable name rules;
- a one-sentence description that says when to use the skill;
- `type: prompt` unless the user explicitly asks for another supported type and
  the use case fits;
- a body with concrete steps, inputs, outputs, and boundaries;
- a `## Verify` section with a concrete verification method;
- no placeholder text such as `TODO`, `TBD`, or `<fill me>`.

The workflow must prefer a small, focused skill over a broad "do everything"
skill. If the user's request describes multiple unrelated workflows, it should
ask to split them rather than creating a vague combined skill.

The workflow must create the skill with `CreateSkill`, then verify reload by
checking that the created skill appears in the skill list. If `CreateSkill`
reports a backup because the skill already existed, the final response must say
which skill was overwritten and where the backup lives.

## Auto-Mode Behavior Matrix

| Workflow | Missing user input | `ask` / `yolo` behavior | `auto` behavior |
| --- | --- | --- | --- |
| `/init` | Helpful but not always required | May ask during workflow | Preflight offers switch to `ask`, continue best-effort, or cancel |
| `/skill:self-evo 7` | None | Runs directly | Runs directly |
| `/skill:self-evo` | Required scope | Must ask for scope | Preflight offers switch to `ask` or cancel |
| `/skill:create-skill make X` | Usually none | Runs directly, may ask essential follow-up | Runs directly unless the workflow declares an essential missing detail |
| `/skill:create-skill` | Required requirement | Must ask for requirement | Preflight offers switch to `ask` or cancel |

This matrix is the heart of the design. Optional clarification can be skipped in
`auto`; required clarification must happen before work starts or not at all.

## Prompt Injection and Best-Effort Notes

When the user chooses `ContinueAutoBestEffort`, Neo should add a concise
workflow note to the generated or skill context:

```text
Auto permission mode remained active. User questions are unavailable during this workflow. Proceed with explicit best-effort assumptions and report any assumption that materially affects the result.
```

This note should be inserted only for workflows that allow best-effort auto
execution. It must not be inserted for required-clarification workflows that
cannot continue in auto.

For skill workflows, the note should become part of the pending skill context
rather than a normal user message. The visible transcript should keep showing a
normal `SkillActivation` entry, not a large internal preflight note.

## Skill Store Reload

Skill creation workflows must not require the user to restart Neo.

`CreateSkill` remains the canonical write path and should reload the shared
skill store after successful creation. The built-in skill workflows should then
perform a narrow visibility check, such as listing skills and confirming the
created name is present.

If reload fails, the tool result should report the written path and the reload
error. The workflow should tell the user the skill was written but is not yet
available in the active session.

## Documentation Updates

Update English and Chinese docs together:

- slash command reference: document auto-mode preflight for interactive
  workflows;
- skills customization guide: add `create-skill`, update `self-evo` no-argument
  behavior, and explain reload after `CreateSkill`;
- interaction guide: clarify that `auto` does not ask mid-run and that
  preflight exists to avoid surprise blocking.

Docs should state the principle plainly: Neo may ask before a workflow starts,
but should not surprise-pause unattended auto work halfway through.

## Testing Strategy

Use focused tests only.

Add interactive controller tests for:

- `/init` still opens the generic preflight in `auto`;
- `/init` continue-auto still starts with the best-effort note;
- `/skill:self-evo` with no args in `auto` opens required preflight and starts
  no model turn;
- selecting the recommended self-evo preflight option switches to `ask` and
  starts the skill workflow;
- `/skill:self-evo 7` in `auto` does not open preflight;
- `/skill:create-skill` with no instruction in `auto` opens required preflight
  and starts no model turn;
- `/skill:create-skill make X` in `auto` does not open preflight;
- cancelling any preflight clears pending continuation state;
- submitting multiple preflight-required skills in one prompt returns a status
  error and starts no model turn.

Add skill manager or built-in skill tests for:

- `create-skill` is included in built-in skills;
- `self-evo` instructions require a concrete no-argument scope;
- created skill examples include a `## Verify` section;
- `CreateSkill` reload behavior remains covered by the existing reload test.

Suggested narrow command shape:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::<test_name> --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::skills_manager::tests::<test_name> --exact --nocapture
```

Use the exact final test names from the implementation.

## Rollout and Migration

Do not preserve the old init-specific preflight path as a compatibility layer.
Move `/init` to the generic preflight contract and delete the old init-shaped
types once the generic path covers the existing behavior.

The first version should support these workflow ids:

- `init`
- `skill:self-evo`
- `skill:create-skill`

Future workflows can add preflight specs by declaring their `AutoModePolicy` and
pending continuation. They should not add separate ad hoc `pending_*` fields.

## Open Decisions

The implementation plan should decide the exact module name and file placement.
The preferred direction is a small `interactive_preflight` module under
interactive mode, with no dependency on `/init`.

The implementation plan should also decide whether no-argument `self-evo` gets a
local scope picker in the first version. If it does, auto mode can collect the
scope locally and continue without switching to `ask`. If it does not, auto mode
must offer only switch-to-ask or cancel.

For the first implementation, the safer recommendation is no local scope picker:
reuse the existing choice picker only for the permission preflight, then let
`AskUserQuestion` handle the domain questions after the user chooses `ask`.

## Self-Review

Placeholder scan: PASS. The only `TODO`, `TBD`, and `<fill me>` strings are
literal examples of text that generated skills must reject; there are no
unresolved placeholders or empty sections in this spec.

Internal consistency: PASS. The spec distinguishes optional clarification from
required clarification and uses that distinction consistently for `/init`,
`self-evo`, and `create-skill`.

Scope check: PASS. This is one coherent implementation plan: a generic preflight
contract plus two built-in skill workflows. It is larger than a tiny change but
does not require decomposition into unrelated projects.

Ambiguity check: PASS. The spec explicitly chooses not to silently switch
`auto` to `ask`, not to continue required-clarification workflows in auto, and
not to preserve the init-specific preflight path as compatibility.
