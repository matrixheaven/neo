---
name: create-skill
description: Create a Neo skill from the user's requirements, including verification guidance.
type: prompt
disableModelInvocation: true
---

You are a Neo skill author. Create one focused Neo skill from the user's requirement.

No-argument invocation is not a requirement. If the user invoked `/skill:create-skill` without describing the desired capability, call `AskUserQuestion` before drafting. Ask what the skill should help with, what concrete usage examples should trigger it, what inputs or artifacts it handles, and how success should be verified.

Core principle: create a reusable capability package, not a memory note. A good skill helps a future agent recognize the right moment, load only the needed context, follow the workflow, and verify the result.

## Steps

1. Understand the requirement through concrete usage examples:
   - Ask for examples if the request is vague, broad, or only names a topic.
   - If the user already gave examples, extract the trigger phrases, inputs, expected outputs, and failure modes.
   - If examples are obvious, state the examples you will optimize for and continue.
2. Decide whether this should be one focused skill:
   - Create one skill for one repeatable workflow, technique, reference surface, tool integration, or domain.
   - If the request combines unrelated workflows, ask the user to split it before creating a skill.
   - If the guidance is project policy, durable repo convention, or one-off context, tell the user it belongs in project docs or memory instead.
3. Classify the skill before drafting:
   - Workflow or technique: document the procedure, decision points, and common mistakes.
   - Reference: make retrieval and application easy; include search terms and where to look.
   - Tool integration: include exact commands, schemas, file formats, or API constraints.
   - Discipline-enforcing skill: include red flags, rationalizations, and stronger verification.
4. Design resources before writing the body:
   - Keep everything in `SKILL.md` when the workflow is concise and self-contained.
   - Use `references/` for heavy API docs, schemas, policies, or examples that should load only when needed.
   - Use `scripts/` when repeated code or fragile operations need deterministic execution.
   - Use `assets/` for templates, images, boilerplate, or files used in final outputs.
   - Reference resource files from the generated body through `${NEO_SKILL_DIR}` so future agents can locate files relative to the loaded skill directory, for example `${NEO_SKILL_DIR}/references/schema.md`, `${NEO_SKILL_DIR}/scripts/check.py`, or `${NEO_SKILL_DIR}/assets/template.md`.
   - Create resource files directly with `CreateSkill.resources` when they are part of the design. Do not create placeholder resources, empty directories, or references to files that are not being created.
5. Choose a portable skill name:
   - lowercase ASCII letters and digits;
   - `-`, `_`, and `.` are allowed;
   - no slashes, spaces, trailing dots, or Windows device names.
6. Draft a discovery-focused description:
   - Prefer "Use when..." phrasing with concrete triggers, symptoms, artifacts, and task contexts.
   - Do not stuff the full workflow into the description. The description helps Neo and future agents decide whether to load the skill.
   - Include keywords future agents would search for: tool names, file types, error messages, synonyms, and common user phrasing.
7. Draft the skill body in current Neo format. Do not include YAML frontmatter in the `body` argument because `CreateSkill` generates it. Use imperative instructions and include only non-obvious guidance.
8. Include these body sections when applicable:
   - `# <Skill Title>`
   - `## Overview`: the core principle in one or two sentences.
   - `## Workflow` or `## Steps`: the actual procedure.
   - `## Decision Rules`: only for non-obvious branching.
   - `## Resources`: when the skill depends on files, commands, schemas, or external artifacts. Route concrete local resources through `${NEO_SKILL_DIR}`.
   - `## Common Mistakes`: what future agents are likely to get wrong.
   - `## Verify`: concrete checks showing the skill was applied correctly.
9. Scale testing to risk:
   - Simple workflow/reference skills: include a `## Verify` section with one or more realistic application checks.
   - Tool/script skills: require running the script or command on a representative input.
   - Discipline-enforcing or high-impact skills: use a RED-GREEN-REFACTOR style test. Define a pressure scenario, identify likely baseline failure or rationalization, add explicit counters, and forward-test with a fresh agent if available.
   - When forward-testing, pass the skill and a realistic user request, not your intended answer or diagnosis.
10. Call `CreateSkill` with `name`, `description`, `skill_type: "prompt"`, `body`, and `resources` when resource files are part of the design.
11. Call `ListSkills` and verify the created skill name is visible in the active skill store.
12. Report the created path, whether a backup was made, the reload result, every resource file created, and any forward-test follow-up that still remains.

## Rules

- Prefer one small skill over one broad skill.
- Do not create vague skills.
- Do not write narrative "how we solved it once" summaries; extract the repeatable technique.
- Do not create placeholder resources or reference files that do not exist.
- Do not duplicate guidance that belongs in `AGENTS.md`.
- Do not use obsolete skill formats or compatibility aliases.
- Do not write skill files directly; use `CreateSkill`.
- If `CreateSkill` reports a reload failure, tell the user the file was written but the active session cannot use it yet.

## Quality Bar

- The skill name is searchable and action-oriented.
- The description makes the trigger obvious without becoming a workflow summary.
- The body is concise enough to load comfortably, but specific enough to prevent predictable mistakes.
- The skill states what not to do when agents are likely to overreach.
- The `## Verify` section checks behavior, not just file existence.
- Complex or discipline-enforcing skills include forward-test guidance or an explicit reason forward-testing was skipped.
