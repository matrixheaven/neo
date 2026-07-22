---
name: self-evo
description: Use when the user explicitly asks to distill a concrete history scope into reusable Neo skills.
disableModelInvocation: true
---

Distill reusable Neo skill packages from an explicit evidence scope. Creating zero skills is a valid successful result.

Accept `current`, a day count, a session id or UUID, or a concrete `topic:` value. No-argument invocation is not a scope: call `AskUserQuestion` and do not read history until the user chooses one.

## Workflow

1. Read only the selected scope. Extract repeatable cross-session techniques, workflows, references, or deterministic helpers. Reject credentials, secrets, raw transcripts, transient logs, unresolved scratch notes, session narrative, one-off facts, and project-only policy.
2. Call `ListSkills` before drafting candidates. Deduplicate against canonical installed skills. Update an existing skill only when the selected evidence supports replacing it; otherwise skip the duplicate.
3. Keep only strong candidates. Each accepted pattern becomes one focused package with a discovery-focused description, a concise Markdown body without YAML frontmatter, and a behavior-oriented `## Verify` section. Return success with no writes when no candidate meets this bar.
4. Add `resources` only when a reference, script, or asset makes that package smaller, deterministic, or reusable. Supply new files through `CreateSkill.resources`; otherwise reference only a resource already preserved in the existing package. Never reference an absent resource.
5. Add `host_metadata` only when a distinct human-facing label or summary has a real consumer, or when the body requires a concrete configured MCP server identifier. Do not duplicate the canonical name or description. Do not invent icons, brands, transports, commands, URLs, installers, permissions, connection data, `agents/openai.yaml`, or other host metadata.
6. Process candidates sequentially. For each candidate, call `CreateSkill` with `name`, `description`, `body`, and only the needed `resources` and `host_metadata`; require a successful reload; call `ListSkills` and confirm the active store contains it; then perform its strongest available representative behavior check.
7. After each successful write, report the package path, backup result, reload result, resources written or preserved, whether the sidecar was created, the representative behavior check and its result, and any remaining verification. Report the backup when replacing an existing package.
8. If creation, reload, discovery, or representative verification fails, report the exact failure and stop before processing the next candidate.

`CreateSkill` is the sole package writer. Never write package files directly or emit any other package schema.
