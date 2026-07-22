# Neo Skill Package Completion - Intent

## TaskIntentDraft

- Requested outcome: Produce an approved-scope design spec, executable implementation plan, and resume-safe handoff for completing Neo's local skill package runtime.
- Goal: Make Neo skill packages path-aware, resiliently discoverable, host-metadata capable, and honest about supported manifest semantics without adding remote distribution or duplicate owners.
- Success evidence:
- Indexed spec and plan define exact owners, contracts, retirement, focused tests, checkpoints, and handoff boundaries; Aegis workspace check passes; docs-only commit contains no unrelated worktree files.
- Stop condition: Done when documents are indexed, validated, and committed; needs-verification if structural checks fail; blocked if existing authority contradicts the approved direction; scope-exceeded if implementation begins.
- Non-goals:
- No project-local implicit skill discovery, marketplace, hosted sync, plugin runtime, remote/orchestrator provider, dynamic skill selector, silent catalog truncation, automatic MCP installation, binary CreateSkill payload, icon or brand-color UI.
- Scope: Documentation-only design and planning for neo-agent-core skill loading/invocation/management and neo-agent TUI completion integration.
- Change kinds:
- architecture-plan
- Risk hints:
- Cross-module contract change, external user skill compatibility, symlink discovery, append-only catalog stability, and duplicate metadata owner risk.

## BaselineReadSetHint

- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Acknowledged before plan:
- none
- Cited in plan:
- none
- Missing refs:
- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Advisory decision: needs-baseline-readback

## ImpactStatementDraft

- Compatibility boundary: Existing references/scripts/assets packages, NEO_SKILL_DIR expansion, manual /skill activation, Skill tool invocation, tier precedence, Aegis symlink views, transcript SkillInvocation events, and session replay remain functional.
- Affected layers:
- neo-agent-core/skills
- neo-agent-core/runtime
- neo-agent/interactive
- documentation
- Owners:
- SkillStore
- LoadedSkill
- Skill invocation context renderer
- Invariants:
- SKILL.md remains the only automatically injected package entry point.
- NEO_HOME/skills remains Neo's sole implicit user root; extra_skill_dirs and skill_path remain explicit roots.
- Skill catalog updates remain deterministic full append-only snapshots and never silently omit skills.
- Non-goals:
- No project-local implicit skill discovery, marketplace, hosted sync, plugin runtime, remote/orchestrator provider, dynamic skill selector, silent catalog truncation, automatic MCP installation, binary CreateSkill payload, icon or brand-color UI.

These records are Method Pack drafts / hints, not authoritative runtime decisions.

## BaselineUsageDraft

- Required baseline refs:
- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Delivered context refs:
- none
- Acknowledged before plan:
- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Cited in plan:
- docs/aegis/specs/2026-07-09-skill-resources-design.md
- docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md
- docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md
- docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md
- docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Missing refs:
- none
- Advisory decision: continue
