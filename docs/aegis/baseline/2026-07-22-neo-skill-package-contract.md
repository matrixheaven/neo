# Neo Skill Package Contract Baseline

Status: `recorded-from-adr`
Date: `2026-07-22`
ADR: `docs/aegis/adr/ADR-0003-neo-skill-package-contract.md`

## Product / Requirement Baseline

- Neo loads local skill packages from `$NEO_HOME/skills`, configured
  `extra_skill_dirs`, and explicit `skill_path` inputs.
- Manual completion displays and inserts `/skill:<canonical-name>` for every
  discovered skill. Host display names are descriptive metadata only.
- Directory symlink views remain discoverable. The emitted package root keeps
  the discovered view while canonical paths are used only for cycle detection.
- A malformed package or optional sidecar records a diagnostic without hiding
  unrelated valid skills.
- `create-skill` authors one requirement-driven package per invocation;
  `self-evo` distills zero or more evidence-driven packages sequentially.

## Architecture / Runtime Boundary Baseline

- `SKILL.md` owns canonical name, model-facing selection text, arguments, and
  `disableModelInvocation`.
- Optional `agents/neo.yaml` owns only human-facing display strings and
  declared MCP server dependencies. It never installs, starts, enables, or
  authenticates a server.
- `SkillStore` and `skills::discovery` own tier precedence, bounded traversal,
  diagnostics, and the immutable loaded package snapshot.
- `skills::context` is the only model-visible activation envelope owner for
  automatic `Skill` calls and manual `/skill:*` activation.
- Discovery is bounded per configured root to depth 6, 2,000 visited
  directories, and 20,000 directory entries.
- `agents`, `references`, `scripts`, and `assets` are resource directories,
  not nested skill roots. Nested packages use `skills/`.

## Compatibility And Retirement

- Preserved: references/scripts/assets, `${NEO_SKILL_DIR}`, arguments,
  `whenToUse`, `disableModelInvocation`, user > extra > built-in precedence,
  complete catalog snapshots, `SkillInvocation` transcript events, replay,
  and Aegis direct-child symlink views.
- Retired without fallback: `SkillType`, manifest `type`,
  `CreateSkill.skill_type`, `slashCommands` / `slash_commands`, the duplicate
  manual renderer, raw automatic bodies, and fail-closed traversal.
- Unknown third-party `SKILL.md` frontmatter remains tolerated, but Neo tools
  and documentation emit only the canonical contract.

## Verification Boundary

- Exact core tests cover symlink/cycle/depth traversal, sidecar diagnostics,
  duplicate names, metadata serialization, typed authoring preflight, durable
  write reporting, and the shared activation envelope.
- Exact binary tests cover canonical completion labels, manual activation, and
  one semantic invocation card.
- Fresh-agent comparisons cover both shipped author prompts; static substring
  tests are not sufficient behavior evidence.
- A real-home probe must distinguish physical files from followed symlink
  views and confirm the active host contains all expected Aegis links.
- Windows reparse behavior remains platform-CI evidence; the macOS host cannot
  execute Windows-specific filesystem paths.

## Residual Risk

- Symlink and sidecar target behavior is exercised directly on macOS. Windows
  uses the shared reparse-point guard and conditional tests, but needs Windows
  CI for release-grade platform evidence.
- Declared MCP dependencies are informational. Availability still depends on
  the user's existing Neo MCP configuration.

