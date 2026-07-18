# Aegis Dual-Host Install Baseline

Status: `recorded-from-adr`
Date: `2026-07-18`
ADR: `docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md`

## Product / Requirement Baseline

- Codex and Neo expose the complete Aegis Method Pack from one canonical checkout.
- Superpowers is absent from both active host discovery trees.
- Neo has exactly one implicit user skill root: `$NEO_HOME/skills/`.
- Neo continues to support `extra_skill_dirs` and `skill_path` as explicit roots.
- All tracked feature specs and plans are canonical under `docs/aegis/`; their
  unfinished task state is preserved during migration.
- Aegis workspace, doctor, updater, and host activation flows remain available.

## Architecture / Runtime Boundary Baseline

- Canonical method-pack owner: `~/.codex/aegis/`.
- Codex discovery view: `~/.agents/skills/aegis/<skill>` direct-child links.
- Neo discovery view: `~/.neo/skills/aegis/<skill>` direct-child links.
- Neo implicit discovery owner: `user_skill_dirs`, returning only
  `$NEO_HOME/skills/`.
- Explicit configured roots remain owned by `extra_skill_dirs` and `skill_path`.
- Skill metadata parsing remains strict YAML first. After strict failure, the
  parser may perform one bounded Codex-compatible scalar repair and parse once
  more; otherwise it returns the original strict-parser error.
- Project `.agents/skills/` is not a Neo trust input because Neo does not load it
  implicitly.
- Aegis remains an advisory method pack, not an authoritative runtime core.

## Compatibility And Retirement

- Preserved: `extra_skill_dirs`, `skill_path`, `$NEO_HOME/skills/`, built-in
  skills, and other installed skill packs.
- Retired: both Superpowers discovery trees, implicit
  `$NEO_HOME/.agents/skills/`, and project `.agents/skills/` trust handling.
- Retired: the legacy Superpowers documentation directory; no compatibility
  directory or duplicate document owner remains.
- No Superpowers alias, backup, mixed-loading path, or second editable Aegis
  checkout remains.

## Verification Boundary

- The current installation has 22 direct-child Aegis links in each host view.
- Fresh Neo activation must emit `SkillInvocation` for `using-aegis` and finish
  with `AEGIS_NEO_OK`.
- Aegis doctor and updater status must verify both registered host views.
- The workspace helper must pass `check` for this project.
- The workspace helper must validate every migrated spec and plan as indexed.

## Residual Risk

- The direct-child symlink topology is verified on the current macOS host. This
  record does not claim that every Windows environment permits symlink creation.
- The scalar repair is intentionally bounded to Codex-compatible prose cases;
  unrelated malformed YAML remains unsupported.
