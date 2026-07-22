# Neo Skill Package Completion Design

Status: `approved direction; implementation not started`
Date: `2026-07-22`
ArchitectureReviewRequired: `yes`

## Aegis Visibility

This design fixes the package contract at the existing `SkillStore` and
`LoadedSkill` owners before implementation so path handling, discovery failure,
host metadata, and manifest retirement do not become separate caller-side
patches or duplicate sources of truth.

## 1. Goal

Complete Neo's local skill-package runtime so a skill can carry instructions,
on-demand resources, concise Neo-facing metadata, and declared MCP dependencies
without requiring each package to invent path plumbing or risking that one bad
package disables every other skill.

Success means:

- every activated skill tells the model its stable package root;
- relative references, scripts, and assets have one documented resolution rule;
- discovery is deterministic, bounded, symlink-cycle safe, and fail-soft per
  skill;
- optional Neo host metadata has real TUI and activation consumers;
- the model-visible catalog remains deterministic, append-only, and complete;
- manifest fields describe behavior Neo actually implements;
- existing resource packages, configured roots, manual activation, model
  activation, transcript events, and session replay continue to work.

## 2. Requirement Ready Check

- Requirement source refs: the user's 2026-07-22 request and the approved
  comparison/recommendation immediately preceding this design.
- Goals and scope refs: this document and
  `docs/aegis/work/2026-07-22-skill-package-completion/10-intent.md`.
- User / scenario refs: local skill authors, users installing filesystem skill
  packages, and models activating those packages through `/skill:*` or `Skill`.
- Requirement item refs: Sections 5 through 15.
- Acceptance / verification criteria refs: Section 16.
- Open blocker questions: none.
- Decision: `ready`.

## 3. Baseline Usage

Required and acknowledged before planning:

- `docs/aegis/specs/2026-07-09-skill-resources-design.md`
- `docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md`
- `docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md`
- `docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md`
- `docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md`
- `docs/en/customization/skills.md`
- `docs/zh/customization/skills.md`
- `.references/codex/codex-rs/core-skills/src/{loader,model,render,skill_instructions}.rs`
- `.references/codex/codex-rs/skills/src/assets/samples/skill-creator/`

Baseline decision: `continue`. The design extends the existing resource and
invocation contracts. It does not reopen discovery-root ownership or transcript
presentation.

## 4. First-Principles Decision

First-principles invariants:

- Non-negotiable goal: a filesystem skill package must be self-locating,
  selectively loadable, and unable to take down unrelated skills.
- Non-negotiable constraints: local-only, cross-platform, one canonical owner
  per datum, no silent skill omission, and no new hosted/plugin runtime.
- Historical assumptions to delete: every skill author must write
  `${NEO_SKILL_DIR}` correctly; a skill manifest needs several nominal
  execution types; optional host metadata belongs in model-facing frontmatter.

Owner / retirement matrix:

| Concern | Canonical owner | Retired owner or path |
| --- | --- | --- |
| Model selection and invocation policy | `SKILL.md` manifest | none |
| Neo display metadata | `agents/neo.yaml` | overloading model description |
| Package root and activation envelope | shared core renderer | manual-only renderer and raw automatic body |
| Discovery limits and diagnostics | `skills::discovery` / `SkillStore` | unbounded fail-closed recursion |
| Manual-only behavior | `disableModelInvocation` | `type: flow` |
| Prompt behavior | one skill instruction path | `type: inline` distinction |
| Slash invocation | canonical `/skill:<name>` | unused `slashCommands` metadata |

Verdict: adopt the package-runtime completion approach. Do not adopt full Codex
product parity.

## 5. Options Considered

### A. Path and discovery repair only

Smallest code change, but leaves TUI metadata coupled to model-facing prose and
leaves no package-owned place for declared dependencies.

### B. Complete local package runtime

Chosen. Add path-aware activation, bounded fail-soft discovery, a narrow
`agents/neo.yaml`, typed authoring support, and retirement of misleading
manifest fields. Every new field has a current consumer.

### C. Full Codex parity

Rejected. Icons, brand colors, default prompts, automatic MCP installation,
remote/orchestrator providers, plugin namespaces, dynamic selection, and
catalog truncation require product surfaces Neo does not have and conflict with
the local-only and no-surprise contracts.

## 6. Canonical Package Layout

```text
<skill-root>/
  SKILL.md                 # required; model instructions and invocation policy
  agents/
    neo.yaml               # optional; Neo host metadata
  references/              # optional; read on demand
  scripts/                 # optional; execute through normal tools/permissions
  assets/                  # optional; templates and output assets
  skills/                  # optional; nested skill packages
```

`SKILL.md` remains the only package file automatically injected into model
context. `agents/neo.yaml` is host data, never model instructions.

Inside a package root, `agents`, `references`, `scripts`, and `assets` are
reserved non-skill directories. Nested skills remain under `skills/`.

No alternate directory aliases are introduced.

## 7. `SKILL.md` Contract

The canonical frontmatter is:

```yaml
---
name: schema-review
description: Review JSON schemas against project rules. Use for schema changes.
whenToUse: When a task creates, edits, or reviews JSON Schema files.
disableModelInvocation: false
arguments:
  - name: target
    description: Schema file or directory to review.
    required: true
---
```

Supported fields:

| Field | Contract |
| --- | --- |
| `name` | Required canonical identifier. Existing nested-name qualification remains. |
| `description` | Required model-facing selection description. |
| `whenToUse` | Optional model-facing trigger detail. |
| `disableModelInvocation` | Optional; excludes the skill from the automatic catalog while preserving explicit `/skill:*` use. |
| `arguments` | Optional named/positional argument declarations and defaults. |

Retired fields:

- `type` and the `prompt` / `inline` / `flow` variants;
- `slashCommands` and `slash_commands`.

Reason: `prompt` and `inline` have no distinct runtime behavior, `flow` is only
a second spelling of manual-only policy, and slash aliases have no consumer.
The implementation deletes the enum, parser fields, CreateSkill argument, docs,
and tests together. It does not retain a compatibility branch.

Migration:

- remove `type: prompt` and `type: inline`;
- replace `type: flow` with `disableModelInvocation: true`;
- invoke every skill through `/skill:<canonical-name>` rather than a declared
  slash alias.

Unknown YAML fields remain ignored so third-party frontmatter can coexist, but
Neo documentation and authoring tools emit only the canonical fields above.

## 8. `agents/neo.yaml` Contract

The optional sidecar has this bounded schema:

```yaml
interface:
  display_name: "Schema Review"
  short_description: "Review JSON schemas against project rules"

dependencies:
  tools:
    - type: "mcp"
      value: "jsonSchemaRegistry"
      description: "Schema registry MCP server"
```

Supported fields and consumers:

| Field | Consumer |
| --- | --- |
| `interface.display_name` | TUI completion label and `ListSkills` human label |
| `interface.short_description` | TUI completion description and `ListSkills` summary |
| `dependencies.tools[].type` | Typed dependency parser; only `mcp` is accepted |
| `dependencies.tools[].value` | Activation envelope and `ListSkills` dependency summary |
| `dependencies.tools[].description` | Activation envelope detail |

The sidecar intentionally has no invocation policy. `SKILL.md` remains the
single owner of automatic/manual selection behavior.

The sidecar intentionally excludes icon paths, brand color, default prompt,
transport, command, and URL. Neo has no current UI or safe installer consumer
for them. MCP connection details remain owned by Neo configuration.

Validation rules:

- optional file; absence is normal;
- invalid optional metadata records a diagnostic but does not hide the skill;
- all strings are trimmed and must be single-line;
- `display_name` is at most 64 Unicode scalar values;
- `short_description` and dependency descriptions are at most 256 scalar
  values;
- dependency values are at most 128 scalar values and use the configured MCP
  server identifier, not a namespaced tool name;
- unsupported dependency types are diagnosed and omitted;
- an empty parsed sidecar is treated as absent.

Declared dependencies do not install, enable, authenticate, or start MCP
servers. The activation envelope tells the model which server is required; the
model must use the existing MCP management surface or report the missing tool.

## 9. Path-Aware Progressive Disclosure

One core renderer owns the model-visible activated skill envelope for both
automatic `Skill` calls and manual `/skill:*` activation:

```xml
<neo-skill-loaded name="schema-review" source="user" root="/absolute/package/path">
<dependencies>
  <mcp value="jsonSchemaRegistry">Schema registry MCP server</mcp>
</dependencies>
<instructions>
# Schema Review
...
</instructions>
</neo-skill-loaded>
```

Rules:

- `name`, `source`, and `root` are always present and XML-escaped;
- the dependency block is omitted when empty;
- instructions are the expanded `SKILL.md` body;
- `$ARGUMENTS`, positional/named parameters, and `${NEO_SKILL_DIR}` continue to
  expand before rendering;
- `${NEO_SKILL_DIR}` remains supported, but package authors may use paths
  relative to the displayed `root`;
- the catalog usage reminder explicitly tells the model to resolve relative
  resource paths against `root`, read references only when required, run
  scripts through normal tools, and reuse assets instead of recreating them;
- the renderer does not read references, execute scripts, or embed assets.

The physical symlink target is not exposed as the package root. Neo preserves
the absolute discovered path so host-managed views such as
`~/.neo/skills/aegis/<skill>` remain stable and resources resolve through that
view.

Transcript presentation remains unchanged. The existing semantic
`SkillInvocation` event continues to show canonical names, source, outcome, and
the current body preview; expanded skill instructions remain model context, not
transcript content.

## 10. Discovery and Load Outcome

Discovery remains standard-library filesystem traversal with these limits per
configured root:

- maximum depth: 6 directories below the root;
- maximum visited directories: 2,000;
- maximum directory entries: 20,000;
- visited-directory identity: canonical filesystem path used only for cycle
  detection;
- emitted package paths: absolute discovered paths, preserving symlink views.

Directory symlinks remain supported because the Aegis host layout depends on
them. A canonical visited set prevents cycles and duplicate traversal.

Discovery returns skills plus structured diagnostics. It does not return early
because one `SKILL.md`, optional sidecar, or directory entry fails.

Diagnostic classes:

- root or directory read failure;
- traversal limit reached;
- symlink cycle or already-visited directory;
- malformed or unreadable `SKILL.md`;
- malformed optional `agents/neo.yaml`;
- invalid sidecar field or unsupported dependency;
- duplicate qualified name within a tier.

`SkillStore` keeps diagnostics beside the successfully loaded skills. The
runtime may log them; `ListSkills` prints a concise warning section after the
catalog. Diagnostics never enter the model-visible available-skills catalog.

Tier precedence remains built-in, then extra, then user insertion, with user
winning same-name conflicts. Explicit `extra_skill_dirs` and `skill_path`
remain supported. `$NEO_HOME/skills` remains the only implicit user root.

## 11. Catalog Contract

The append-only catalog keeps its existing owner and replacement semantics:

- model-visible identity is canonical skill name;
- selection prose is `description` plus optional `whenToUse`;
- `agents/neo.yaml` display strings never replace model-facing prose;
- `disableModelInvocation: true` removes a skill from the automatic catalog but
  not from explicit manual lookup;
- ordering remains source tier then canonical name;
- any semantic catalog change appends one complete replacement snapshot;
- unchanged catalogs append nothing;
- no token estimate, dynamic selector, automatic truncation, or silent omission
  is introduced.

## 12. TUI and Management Behavior

TUI completion:

- displays `interface.display_name` when present, otherwise canonical name;
- inserts `/skill:<canonical-name>` regardless of display name;
- displays `interface.short_description`, falling back to the manifest
  description;
- uses existing text-only picker styling; no icons or branded color.

`ListSkills` remains grouped by tier and always prints canonical name and path.
When host metadata exists, it may append the display name and short description.
Resource and dependency summaries remain compact; individual files and MCP
connection details are not dumped.

`CreateSkill` gains one optional typed `host_metadata` object. When absent,
creation omits the sidecar and an update preserves any existing sidecar. When
present, Neo validates and atomically writes the complete
`agents/neo.yaml`. Partial merging is not supported.

Conceptual tool input:

```json
{
  "name": "schema-review",
  "description": "Review JSON schemas against project rules.",
  "body": "# Schema Review\n...",
  "host_metadata": {
    "interface": {
      "display_name": "Schema Review",
      "short_description": "Review JSON schemas against project rules"
    },
    "dependencies": [
      {
        "type": "mcp",
        "value": "jsonSchemaRegistry",
        "description": "Schema registry MCP server"
      }
    ]
  },
  "resources": []
}
```

`CreateSkill.skill_type` is removed. Existing whole-package backup, resource
validation, atomic writes, preservation of unmentioned resources, and hot
reload remain unchanged. `agents/neo.yaml` receives the same symlink/reparse
and atomic-write safety as `SKILL.md`.

## 13. Error and Security Boundaries

- Required `SKILL.md` failure skips that skill and records a diagnostic.
- Optional metadata failure keeps the skill with manifest fallbacks.
- A discovery limit stops only the affected root and records a diagnostic.
- User, extra, and built-in tier precedence is deterministic even with errors.
- Skill resources still execute only through normal tools, permission modes,
  workspace access checks, and MCP configuration.
- Declaring an MCP dependency grants no permission and performs no network or
  config mutation.
- No shell command parses or resolves package structure.
- Paths use `Path` / `PathBuf`; Windows reparse and reserved-name protections
  remain mandatory.

## 14. Compatibility and Retirement

Preserved:

- existing `references/`, `scripts/`, and `assets/` packages;
- `${NEO_SKILL_DIR}` expansion;
- canonical `/skill:<name>` invocation and multiple manual directives;
- model `Skill` invocation;
- `disableModelInvocation`, `whenToUse`, and arguments;
- user > extra > built-in precedence;
- `$NEO_HOME/skills`, `extra_skill_dirs`, and `skill_path` ownership;
- symlinked Aegis skill views;
- append-only catalog replacement snapshots;
- `SkillInvocation` transcript events and replay.

Retired with `delete-first` classification:

- `SkillType`, manifest `type`, and `CreateSkill.skill_type`;
- `slashCommands` / `slash_commands` manifest storage;
- separate manual skill context rendering;
- automatic skill output that contains only a raw body;
- fail-closed recursive discovery.

No runtime fallback or alias is retained for retired fields. Documentation
contains the migration mapping. User skill files are never mutated
automatically.

## 15. Explicit Non-Goals

- project-local implicit skill discovery or `.agents/skills` loading;
- marketplace, registry, hosted sync, plugin runtime, or remote skill provider;
- parsing `agents/openai.yaml` as a fallback;
- icons, brand color, or default-prompt UI;
- MCP auto-install, auto-enable, authentication, or copied transport config;
- binary payloads in `CreateSkill`;
- automatic reading/running of package resources;
- dynamic selection, predicted-cost governance, or automatic catalog omission;
- changing permission modes, transcript card design, or session persistence;
- a general dynamic-context framework.

## 16. Acceptance and Verification

| Requirement | Required evidence |
| --- | --- |
| Path-aware auto activation | Exact core test asserts name/source/root/dependencies/instructions envelope. |
| Path-aware manual activation | Exact `neo` binary test asserts the shared envelope reaches submitted context. |
| Resource compatibility | Exact argument test keeps `${NEO_SKILL_DIR}` expansion. |
| Failure isolation | Exact discovery test loads a valid sibling beside malformed skill and reports one diagnostic. |
| Bounded symlink traversal | Exact discovery test proves a directory cycle terminates and preserves a valid linked skill. |
| Optional metadata | Exact loader test proves valid fields load and malformed sidecar falls back with a diagnostic. |
| Catalog stability | Exact catalog test proves display metadata does not change model-visible identity/prose. |
| TUI display | Exact completion test proves display label, canonical insertion, and short-description fallback. |
| Typed authoring | Exact CreateSkill test writes `agents/neo.yaml`, preserves it when omitted on update, and reloads. |
| Retirement | Lingering-reference search finds no production `SkillType`, `skill_type`, or `slash_commands`. |
| Cross-platform safety | Existing path/reparse tests remain green plus focused sidecar path test. |
| Documentation | English and Chinese docs show identical canonical fields, package layout, and migration. |

Verification follows the repository rule: one package, one target selector, and
at least one test-name filter per test command. Broad workspace test runs are
not completion evidence.

## 17. Complexity and File Boundaries

Complexity budget:

- `skills_manager.rs` is already over 2,500 lines, mostly tool tests. It must
  receive wiring only; sidecar parsing and serialization belong under
  `skills/metadata.rs`.
- `skills/mod.rs` is over 400 lines. Activation rendering belongs under
  `skills/context.rs`; discovery remains in `skills/discovery.rs`.
- interactive slash handling already owns manual activation orchestration but
  must call the shared core renderer instead of retaining markup logic.
- no generic plugin/package abstraction is introduced.

Plan-time verdict: split by real owner files, keep callers wiring-only, and do
not move unrelated existing tests merely to reduce line counts.

## 18. ADR Signal

Implementation completion should evaluate an ADR because this design changes
the durable skill package shape, manifest contract, discovery failure model,
and activation context owner. ADR backfill must cite this spec, the
implementation plan, focused verification, the retirement search, and the
dual-host discovery baseline. No ADR is recorded before implementation proves
the design.

## 19. Self-Review

- Placeholder scan: pass; the document contains no unresolved placeholders.
- Internal consistency: pass; model policy stays in `SKILL.md`, Neo host data
  stays in one sidecar, and every retained sidecar field has a named consumer.
- Scope check: pass; one long-running workstream with owner-separated slices.
- Ambiguity check: pass; retired fields, metadata precedence, failure behavior,
  scan limits, and non-goals are explicit.
- Boundary check: pass; discovery roots, append-only catalog, transcript,
  permission, session, and local-only invariants are preserved.
- Existence check: `agents/neo.yaml`, `metadata.rs`, and `context.rs` are
  `add-with-proof`; existing owners cannot represent host-only display data or
  share one renderer without responsibility overlap.
- Architecture Integrity Lens verdict: `proceed`; `SkillStore` remains the
  catalog/load owner and `LoadedSkill` remains the package snapshot.
