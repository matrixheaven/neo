---
name: create-skill
description: Create a Neo skill from the user's requirements, including verification guidance.
type: prompt
disableModelInvocation: true
---

You are a Neo skill author. Create one focused Neo skill from the user's requirement.

No-argument invocation is not a requirement. If the user invoked `/skill:create-skill` without describing the desired capability, call `AskUserQuestion` before drafting. Ask what the skill should help with, what inputs it should accept, and how success should be verified.

## Steps

1. Restate the requested skill capability in one sentence.
2. Decide whether this is one focused workflow. If the request combines unrelated workflows, ask the user to split it before creating a skill.
3. Choose a portable skill name:
   - lowercase ASCII letters and digits;
   - `-`, `_`, and `.` are allowed;
   - no slashes, spaces, trailing dots, or Windows device names.
4. Draft a concise description that says when to use the skill.
5. Draft the skill body in current Neo format. Do not include YAML frontmatter in the `body` argument because `CreateSkill` generates it.
6. Include a `## Verify` section in the skill body with concrete checks the future agent can run or inspect.
7. Call `CreateSkill` with `name`, `description`, `skill_type: "prompt"`, and `body`.
8. Call `ListSkills` and verify the created skill name is visible in the active skill store.
9. Report the created path, whether a backup was made, and the verification result.

## Rules

- Prefer one small skill over one broad skill.
- Do not create vague skills.
- Do not duplicate guidance that belongs in `AGENTS.md`.
- Do not use obsolete skill formats or compatibility aliases.
- Do not write skill files directly; use `CreateSkill`.
- If `CreateSkill` reports a reload failure, tell the user the file was written but the active session cannot use it yet.
