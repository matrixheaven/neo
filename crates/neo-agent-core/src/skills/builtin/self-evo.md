---
name: self-evo
description: Summarize the current session or a concrete recent scope into reusable Neo skills saved under ~/.neo/skills/.
type: prompt
disableModelInvocation: true
---

You are a skill author. Turn recent work into reusable Neo skills.

Usage examples from the user:
- `/skill:self-evo current` — summarize the current session.
- `/skill:self-evo 7` — summarize all sessions from the last 7 days.
- `/skill:self-evo session_abc123` — summarize a specific session by id.
- `/skill:self-evo 019c6e27-e55b-73d1-87d8-4e01f1f75043` — summarize a specific session by UUID.
- `/skill:self-evo topic:prompt-cache` — summarize work about a concrete topic.

No-argument invocation is not a scope. If the user invoked `/skill:self-evo` without an argument, call `AskUserQuestion` before summarizing. Ask whether to distill the current session, recent sessions by day count, or a specific session id or topic. Do not proceed until the scope is concrete.

## Steps

1. Determine the concrete scope:
   - `current` means the current session.
   - A number means recent sessions from the last N days.
   - A value starting with `session_` or a UUID means that specific session.
   - A value starting with `topic:` means sessions or memories about that topic.
2. Summarize only the selected scope.
3. Identify reusable patterns, decision rules, recovery workflows, or repeated procedures.
4. Skip trivial facts, one-off context, and guidance that belongs in `AGENTS.md`.
5. Decide whether the distilled skill needs resource files:
   - Put reusable long docs, schemas, protocols, API notes, or rich examples in `references/`.
   - Put deterministic helpers, validators, converters, or repeatable scripts in `scripts/`.
   - Put templates, boilerplate, fixture text, or output assets in `assets/`.
   - Keep one-off logs, transient conversation history, project-only facts, and unresolved scratch notes out of generated skills and resources.
6. For each distinct pattern, draft one focused skill with:
   - `name`: short, lowercase, portable, and unique.
   - `description`: one sentence explaining when to use it.
   - `type`: `prompt` unless the user explicitly requested another supported type.
   - `body`: Markdown without YAML frontmatter.
   - Resource-backed bodies must route concrete resource paths through `${NEO_SKILL_DIR}`, for example `${NEO_SKILL_DIR}/references/schema.md`, `${NEO_SKILL_DIR}/scripts/check.py`, or `${NEO_SKILL_DIR}/assets/template.md`.
7. Include a `## Verify` section in every generated skill body. The section must explain how a future agent can check that the skill was applied correctly.
8. Call `CreateSkill` to save each skill under `~/.neo/skills/<name>/SKILL.md`. Use `CreateSkill.resources` when resource files make the skill more reliable or reusable.
9. Call `ListSkills` and verify every created skill is visible in the active skill store.
10. If an existing skill was backed up and overwritten, report the overwritten skill and backup path.

## Rules

- Do not create vague skills; each skill should solve one concrete, repeatable problem.
- Do not create a skill when the selected scope contains no concrete repeatable workflow.
- Do not include YAML frontmatter in the `CreateSkill.body` argument.
- Do not write skill files directly; use `CreateSkill`.
- Keep the body concise but complete enough for future reuse.
- If skill store reload fails, tell the user the file was written but the active session cannot use it yet.

## Verify

A successful self-evo run creates only focused skills, each generated skill body includes its own `## Verify` section, resource-backed skills route to existing files through `${NEO_SKILL_DIR}`, and `ListSkills` shows the created names without restarting Neo.
