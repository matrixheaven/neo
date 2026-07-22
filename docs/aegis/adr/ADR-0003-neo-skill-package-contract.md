# ADR-0003 - Neo local skill package contract

Status: `recorded-from-work`
Date: `2026-07-22`

## Source Evidence

- Implemented and focused-tested skill package completion work recorded under docs/aegis/work/2026-07-22-skill-package-completion.
## Context

Neo needed resource-bearing local skills, host-only display metadata, symlink-safe discovery, and identical manual/model activation semantics without adopting a hosted plugin runtime or duplicate policy owners.

## Decision

Keep SKILL.md as model-policy owner; use optional agents/neo.yaml only for Neo display metadata and declared MCP server dependencies; keep SkillStore/discovery as bounded fail-soft catalog owner; render both manual and model activation through one path-aware envelope that preserves the discovered symlink-view root.

## Alternatives Considered

- Limit the change to symlink discovery only; rejected because metadata and activation would retain duplicate or incomplete owners.
- Adopt full Codex plugin parity; rejected because Neo is local-only and has no consumer for icons, brands, installers, remote providers, or dynamic catalog selection.
## Consequences

- Skill packages can carry resources and narrow host metadata, malformed packages produce diagnostics without hiding siblings, and both activation paths expose one stable package root; package authors must migrate retired type and slash alias fields.
## Compatibility Boundary

Preserve NEO_HOME/skills, extra_skill_dirs, skill_path, references/scripts/assets, NEO_SKILL_DIR, canonical /skill:<name>, Skill tool activation, tier precedence, complete catalog snapshots, transcript events, session replay, and Aegis symlink-view paths.

## Retirement Impact

Delete SkillType, type/skill_type, slashCommands/slash_commands, duplicate manual rendering, raw automatic bodies, and fail-closed traversal; no compatibility parser or fallback remains.

## Baseline Sync

- Needed: needed
- Target: docs/aegis/baseline/2026-07-22-neo-skill-package-contract.md
- Action: create snapshot
- Reason: The decision changes package shape, owner map, compatibility contract, and discovery failure behavior.

## Evidence References

- docs/aegis/specs/2026-07-22-skill-package-completion-design.md
- docs/aegis/plans/2026-07-22-skill-package-completion.md
- docs/aegis/work/2026-07-22-skill-package-completion/proof-bundle.md
## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
