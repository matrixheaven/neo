---
name: self-evo
description: Summarize the current session (or recent sessions) into one or more reusable skills saved under ~/.neo/skills/.
type: prompt
disableModelInvocation: true
---

You are a skill author. Turn recent work into reusable Neo skills.

Usage examples from the user:
- `/skill:self-evo` — summarize the current session.
- `/skill:self-evo 7` — summarize all sessions from the last 7 days.
- `/skill:self-evo session_abc123` — summarize a specific session by id.

Steps:
1. Determine the scope:
   - If the argument is a number, call `SummarizeSessions` with `days: <number>`.
   - If the argument looks like a session id (starts with `session_` or is a UUID), call `SummarizeSessions` with `session_id: <argument>`.
   - Otherwise, use the current session. Ask the user for the session id if you do not have it.
2. Read the summary and identify reusable patterns, decision rules, or workflows.
3. For each distinct pattern, draft a skill with:
   - `name`: short, kebab-case, unique.
   - `description`: one sentence explaining when to use it.
   - `type`: `prompt` (default).
   - `body`: Markdown with YAML frontmatter, containing the reusable instructions.
4. Call `CreateSkill` to save each skill under `~/.neo/skills/<name>/SKILL.md`.
5. If a skill with the same name already exists, create a backup automatically and overwrite only if the user confirmed.

Rules:
- Do not create vague skills; each skill should solve one concrete, repeatable problem.
- Include `$ARGUMENTS` or named placeholders if the skill needs user input.
- Keep the body concise but complete enough for future reuse.
