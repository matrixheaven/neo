# Skills

Skills are reusable prompt fragments that the model can invoke automatically or
that the user can trigger explicitly from the TUI. They live in the agent core
and are loaded from four tiers, in order of precedence:

1. **Project**: `.neo/skills/**/SKILL.md` (and sub-directories).
2. **User**: `~/.neo/skills/**/SKILL.md`.
3. **Extra**: directories listed in `extra_skill_dirs` in config.
4. **Built-in**: shipped with `neo-agent-core`.

A higher-tier skill with the same fully-qualified name overrides a lower-tier
skill.

## Manifest format

Each skill is a single Markdown file with YAML frontmatter:

```yaml
---
name: review
description: Review a file or change for correctness and style.
type: prompt
whenToUse: When the user asks for a code review.
arguments:
  - name: target
    description: File or snippet to review.
    required: true
  - name: focus
    description: Aspect to focus on.
    default: general
---

Review $target with a focus on $focus.
```

Fields:

- `name` (required): skill identifier. Must match the containing directory for
  top-level skills; sub-skills use their qualified path (see below).
- `description` (required): short summary shown in the skill list and used by
  the model to decide when to invoke the skill.
- `type` (optional): `prompt` (default), `inline`, or `flow`. Currently only
  `prompt` is implemented.
- `whenToUse` (optional): guidance for the model on when to invoke the skill.
- `disableModelInvocation` (optional): when `true`, the skill is never offered
  to the model for automatic invocation; it can still be triggered manually via
  `/skill:<name>`.
- `arguments` (optional): list of named arguments. Each argument may declare
  `name`, `description`, `required` (default `false`), and `default`. Declaring
  arguments is optional: undeclared named arguments are never rejected. If the
  body contains a matching `$<name>` marker it is replaced with the
  supplied value; otherwise the argument is passed through as part of
  `$ARGUMENTS`. Only a **declared `required` argument that is missing** produces
  an error.
- `slashCommands` (optional): list of slash command aliases such as `/review`.

## Skill body and placeholders

The Markdown body after the frontmatter is the prompt template. The following
placeholders are expanded when the skill is invoked:

- `$<name>` / `${name}`: declared argument value.
- `$0`, `$1`, …: positional arguments.
- `$ARGUMENTS`: the raw argument string.
- `$ARGUMENTS[0]`, `$ARGUMENTS[1]`, …: positional arguments by index.
- `${NEO_SKILL_DIR}`: absolute path to the skill's root directory.

If the body contains no placeholders, the raw arguments are appended after the
body as:

```text
ARGUMENTS: <raw arguments>
```

## Sub-skills and naming

Nested directories become namespaced skills. Intermediate directories that do
not contain a `SKILL.md` are treated as namespace containers and are skipped.

For example:

```text
.neo/skills/superpowers/SKILL.md                -> superpowers
.neo/skills/superpowers/skills/brainstorming/SKILL.md -> superpowers/brainstorming
```

## Built-in skills

The following skills ship with Neo and can be overridden by project/user
skills:

- `sub-skill`: review and consolidate skills into hierarchical bundles.
- `self-evo`: turn recent session work into reusable skills.

Built-in skills are embedded in the binary, but on startup they are released to
`~/.neo/skills/.builtin/` so you can read and override them. Files in
`.builtin/` are skipped when scanning user skills, so a regular skill under
`~/.neo/skills/<name>/` takes precedence over the built-in copy.
The removed built-in `define-goal` is pruned from `.builtin/` on startup; goal
authoring now lives in Goal mode and the `ExitGoalMode` review flow.

## Self-evolution (`self-evo`)

`self-evo` turns recent session work into reusable skills:

```text
/skill:self-evo              # current session
/skill:self-evo 7            # last 7 days of sessions
/skill:self-evo session_abc  # specific session id
```

It uses `SummarizeSessions` to extract patterns and `CreateSkill` to save
each new skill under `~/.neo/skills/<name>/`.

## Skill bundles (sub-skills)

The `sub-skill` skill is a user-triggered librarian that reviews your skill
inventory and can consolidate related skills into hierarchical bundles. It uses
the `ListSkills` and `MoveSkill` tools; every move creates a timestamped
backup under `~/.neo/backups/skills/<timestamp>/`.

## Automatic invocation

When skills are enabled, the runtime adds a special `Skill` tool spec to
the model's tool list and injects an `<available_skills>` block into the system
prompt. The model may call:

```json
{
  "skill": "review",
  "arguments": { "target": "src/lib.rs", "focus": "safety" }
}
```

The runtime expands the skill body and returns it as a tool result. The model
can then continue with the expanded prompt.

## Manual invocation

In the interactive TUI, type:

```text
/skill:<name> [arguments]
```

For example:

```text
/skill:review src/lib.rs --focus=safety
```

Each available skill appears in the slash-command completion list as
`/skill:<name>`. Selecting a skill activates it and expands its body into the
prompt input, allowing the user to review or edit it before sending. A
`▶ Activated skill: <name>` notice is added to the transcript.

If the skill body contains no placeholders, the raw arguments are appended
after the body so the user's request is preserved.

## Configuration

`~/.neo/config.toml` (or `$NEO_HOME/config.toml`):

```toml
[runtime]
extra_skill_dirs = ["~/my-skills"]
```

Paths support the `~/` prefix and are resolved relative to the user's home
directory.
