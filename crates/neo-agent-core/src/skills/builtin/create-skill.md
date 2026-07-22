---
name: create-skill
description: Use when the user explicitly requests one reusable Neo skill package from concrete requirements.
disableModelInvocation: true
---

Create exactly one focused Neo skill package from the user's requirement.

No-argument invocation is not a requirement. If the request does not state a concrete capability, call `AskUserQuestion` before drafting. Establish concrete triggers, inputs, expected outputs, failure modes, and verification evidence.

## Workflow

1. Reject one-off context and guidance that belongs in project policy or memory. Never copy credentials, secrets, raw transcripts, transient logs, unresolved scratch notes, or project-only policy into the package.
2. Draft a portable, searchable name; a discovery-focused description; and a concise Markdown body without YAML frontmatter. Include a behavior-oriented `## Verify` section.
3. Add `resources` only when a reference, script, or asset makes the package smaller, deterministic, or reusable. Supply new files through `CreateSkill.resources`; otherwise reference only a resource already preserved in the existing package. Never reference an absent resource.
4. Add `host_metadata` only when a distinct human-facing label or summary has a real consumer, or when the body requires a concrete configured MCP server identifier. Do not duplicate the canonical name or description. Do not invent icons, brands, transports, commands, URLs, installers, permissions, connection data, `agents/openai.yaml`, or other host metadata.
5. Call `CreateSkill` exactly once with `name`, `description`, `body`, and only the needed `resources` and `host_metadata`. `CreateSkill` is the sole package writer; never write package files directly or emit any other package schema.
6. After a successful write, call `ListSkills` and confirm that the active store contains the created skill. Then perform the strongest available representative behavior check from the generated `## Verify` section.
7. Report the package path, backup result, reload result, resources written or preserved, whether the sidecar was created, the representative behavior check and its result, and any remaining verification. If creation, reload, discovery, or the behavior check fails, report the exact failure without claiming completion.

Prefer the smallest package that satisfies the concrete requirement. Do not create placeholders or references to absent resources.
