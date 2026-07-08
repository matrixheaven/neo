# Skills

Skills are reusable Markdown instruction packs that let Neo distill "how to do a certain kind of task" into files. A skill is defined by a `SKILL.md`, scanned at runtime across three priority tiers, and can be auto-activated by the model or triggered manually with `/skill:<name>`. The core implementation lives in `crates/neo-agent-core/src/skills/`.

## What Is a Skill

A skill = a directory + a `SKILL.md`. The top of `SKILL.md` is YAML frontmatter (metadata); the body is Markdown (instructions for the model). A skill is not code, but a structured prompt:

- **When to use**: the model picks it automatically based on `whenToUse`, or the user invokes it explicitly via a slash command;
- **How to use**: on activation the body is injected into context, guiding the model to complete the task along established steps;
- **Reusable**: across sessions and projects; teams can share directories.

Skill scanning is loaded centrally by `SkillStore::load`, defined in `crates/neo-agent-core/src/skills/mod.rs`.

## Skill Package Layout

A skill package is a directory whose canonical entry point is `SKILL.md`. `SKILL.md` is the only file Neo loads into context automatically; any supporting file must be named explicitly from the skill instructions, usually with `${NEO_SKILL_DIR}` so the model can resolve it relative to the package root.

Canonical packages use these paths:

| Path | Purpose |
| --- | --- |
| `SKILL.md` | Required skill manifest and instructions; the only automatic context entry point |
| `references/` | Optional text references such as checklists, policy notes, schemas, or long examples that the skill can ask the model to read |
| `scripts/` | Optional helper scripts that the skill can tell the model to run, for example `${NEO_SKILL_DIR}/scripts/check_schema.py` |
| `assets/` | Optional manually managed assets such as templates, sample files, or binary media |

`CreateSkill` v1 can write `SKILL.md` plus optional text resources under `references/`, `scripts/`, and `assets/`. Binary assets are manual: place them in the package yourself and reference them from `SKILL.md` with `${NEO_SKILL_DIR}`.

Resource directories inside a skill package are not scanned for nested skills. Put child skills under `skills/` when a package needs sub-skills.

## SKILL.md Format

```markdown
---
name: deploy-staging
description: Deploys the app to staging. Use when the user asks to deploy.
type: prompt
whenToUse: When the user asks to deploy to staging or update the staging environment.
---

# Deploy to Staging

## Steps
1. Run `cargo build --release`
2. ...
```

### Frontmatter Fields

| Field | Required | Description |
| --- | --- | --- |
| `name` | ✅ | Skill identifier; must match the directory name; child skill packages form a `parent/child` name |
| `description` | ✅ | One-line summary, referenced by the model when selecting |
| `type` | ✅ | `prompt` (injected as a context message, default) / `inline` (expanded directly into the prompt) / `flow` (multi-step interactive workflow) |
| `whenToUse` | recommended | Natural-language trigger description, used for auto-activation |
| `disableModelInvocation` | bool | When `true`, excludes the skill from model auto-invocation listings; intended for explicit `/skill:<name>` use |
| `arguments` | array | Declarative parameters (`name` / `description` / `required` / `default`) |
| `slashCommands` | array | Parsed metadata reserved for slash-command integrations; not currently bound as aliases |

> Skills with `type: flow` never participate in auto-activation; `disableModelInvocation: true` also excludes them from model auto-invocation listings.

## Three-Tier Scan Priority

At startup Neo scans skills in the following order — **a higher-priority skill with the same name overrides a lower-priority one**:

| Priority | Source | Path | Purpose |
| --- | --- | --- | --- |
| 1 | **user** | `~/.neo/skills/`, `~/.neo/.agents/skills/` | Private user skills, highest priority |
| 2 | **extra** | Directories pointed to by `extra_skill_dirs` / `skill_path` in config | Team-shared directories |
| 3 | **builtin** | `~/.neo/skills/.builtin/` (extracted from the binary on first launch) | Neo's built-in skills |

The actual load order: built-in skills are extracted into the Neo-managed `.builtin/` directory from the current binary, then extra and user layers are injected in turn; the user layer is written into the `HashMap` last, so **user skills can override same-named built-in skills**. Directories can be nested; when a parent directory has its own `SKILL.md`, child skills under `skills/` are prefixed as `parent/child`. Customize built-ins by copying them outside `.builtin/`.

```toml
# config.toml — append team-shared skill directories
extra_skill_dirs = ["~/work/team-skills", "/srv/neo-skills"]
skill_path = ["~/work/more-skills"]
```

## Built-in Skills

Neo ships with the following skills (source in `crates/neo-agent-core/src/skills/builtin/`):

| Skill | Type | Description |
| --- | --- | --- |
| `mcp-config` | prompt | Configure MCP servers, handle OAuth login, edit `[[mcp.servers]]` |
| `sub-skill` | prompt | Review, group, and reorganize the skill library into hierarchical sub-skill bundles |
| `self-evo` | prompt | Summarize a concrete current, recent, session, or topic scope into reusable skills |
| `create-skill` | prompt | Create a Neo skill from the user's requirements, including verification guidance |

Workflow-authoring built-ins such as `self-evo` and `create-skill` have `disableModelInvocation: true`, meaning they require explicit user invocation. Neo refreshes shipped built-ins under `~/.neo/skills/.builtin/` from the current binary; put custom copies outside `.builtin/`.

`/skill:self-evo` without arguments asks for a distillation scope before creating skills. In Auto permission mode, Neo opens an interactive preflight before the model turn so the workflow does not block unattended execution later.

`/skill:create-skill` creates one focused skill through the `CreateSkill` tool. If no requirement is provided, it asks for the desired capability before drafting. Created skills include verification guidance and are reloaded into the active skill store when `CreateSkill` succeeds.

## Activation Methods

| Method | Trigger | Behavior |
| --- | --- | --- |
| Model auto-invocation | Model | Activates automatically when `whenToUse` matches and not disabled; body injected into context |
| `/skill:<name>` | User | Invoked directly in the TUI input box; supports `parent/child` nested names |
| `Skill` tool | Model | Programmatic invocation, often orchestrated by other skills |
| `mcp__<server>__authenticate` | Model / User | Specialized tool for MCP OAuth, handled under the `mcp-config` skill |

Prerequisites for model auto-activation: `disableModelInvocation` is false and `type` is not `flow` (determined by `SkillManifest::auto_invokable`).

## Creating Custom Skills

### Using the `CreateSkill` Tool

The model can invoke the `CreateSkill` tool directly in conversation to generate a skill file:

```jsonc
// Invocation parameters
{
  "name": "schema-review",
  "description": "Reviews JSON schemas against project rules.",
  "skill_type": "prompt", // prompt / inline / flow, default prompt
  "body": "# Schema Review\n\nReview schemas using `${NEO_SKILL_DIR}/references/schema-rules.md`. Run `${NEO_SKILL_DIR}/scripts/check_schema.py` when validation is needed.\n\n## Verify\nReport any schema rule violations.",
  "resources": [
    {
      "path": "references/schema-rules.md",
      "content": "# Schema Rules\n\n- Require stable `$schema` declarations.\n- Prefer explicit `additionalProperties` behavior.\n"
    },
    {
      "path": "scripts/check_schema.py",
      "content": "#!/usr/bin/env python3\nimport json\nimport sys\n\nfor path in sys.argv[1:]:\n    with open(path, encoding=\"utf-8\") as handle:\n        json.load(handle)\nprint(\"schemas parsed\")\n",
      "executable": true
    }
  ]
}
```

The tool auto-generates frontmatter, writes `~/.neo/skills/<name>/SKILL.md`, writes optional text resources, backs up an existing skill package before overwrite to `~/.neo/backups/skills/<timestamp>/`, and reloads the active skill store when available.

### Creating Manually

```bash
mkdir -p ~/.neo/skills/deploy-staging
$EDITOR ~/.neo/skills/deploy-staging/SKILL.md
```

The file must start with YAML frontmatter delimited by `---`, followed by a Markdown body. On the next Neo startup or after a skill rescan, you can invoke it with `/skill:deploy-staging`.

### Management Tools

| Tool | Effect |
| --- | --- |
| `ListSkills` | List all discovered skills hierarchically (`include_builtin=true` includes built-ins) |
| `CreateSkill` | Create a new skill; auto-backs up old packages |
| `MoveSkill` | Move a skill directory under a parent bundle to regroup |

> Rule of thumb: turn multi-step flows that recur, pitfalls you have hit, and error-recovery procedures into skills; one-off trivial tasks need not be distilled, and content already in `AGENTS.md` need not be duplicated.

## Next Steps

- [MCP Servers](mcp.md) — The `mcp-config` skill works with MCP
- [Sub-agents](agents.md) — Combine skills with sub-agent orchestration
- [Configuration Files Overview](../configuration/config-files.md) — Where the `extra_skill_dirs` field lives
