# Neo Skill Package Completion - Intent

## TaskIntentDraft

- Requested outcome: design, implement, repair, and finally accept Neo's local
  skill package runtime, including both built-in author skills.
- Goal: make skill packages path-aware, resiliently discoverable,
  host-metadata capable, and honest about supported manifest semantics; restore
  every `~/.neo/skills` symlink view and canonical `/skill:<name>` completion.
- Success evidence:
  - focused core and binary tests cover every acceptance boundary;
  - a real-home probe discovers all installed and symlink-view skills;
  - both author prompts pass baseline-versus-current fresh-agent scenarios;
  - the three reported Rust warnings are absent from `cargo check`;
  - retired paths have zero production/shipped matches;
  - independent reviews have no unresolved in-scope finding;
  - task-owned code and documents are committed without unrelated worktree
    changes and are not pushed.
- Stop condition: done when the evidence above is recorded and committed;
  `needs-verification` if an acceptance boundary lacks fresh evidence;
  `blocked` only for an in-scope build/test failure; `scope-exceeded` for a new
  hosted/plugin/runtime requirement.
- Non-goals: project-local implicit discovery, marketplace, hosted sync,
  plugin runtime, remote/orchestrator provider, dynamic selector, silent
  catalog truncation, automatic MCP install/auth, binary CreateSkill payload,
  icons/brand UI, or transcript/session/permission redesign.
- Scope: `neo-agent-core` skill package owners, `neo-agent` completion and
  activation consumers, both built-in author skills, bilingual user docs,
  Aegis execution evidence, ADR, and baseline.
- Change kinds: contract retirement, bug repair, typed authoring, runtime
  integration, documentation, and architecture recording.
- Risk hints: external hand-authored package compatibility, symlink/reparse
  discovery, complete catalog stability, safe writes, duplicate metadata owner,
  and shared dirty-worktree isolation.

## Baseline Read Set

- `docs/aegis/specs/2026-07-09-skill-resources-design.md`
- `docs/aegis/specs/2026-07-13-skill-invocation-transcript-design.md`
- `docs/aegis/specs/2026-07-20-append-only-skill-catalog-brief.md`
- `docs/aegis/adr/ADR-0001-aegis-dual-host-skill-discovery.md`
- `docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md`
- `docs/aegis/specs/2026-07-22-skill-package-completion-design.md`
- `docs/aegis/adr/ADR-0003-neo-skill-package-contract.md`
- `docs/aegis/baseline/2026-07-22-neo-skill-package-contract.md`

## Impact

- Owners: `SkillStore`, `LoadedSkill`, `skills::discovery`,
  `skills::metadata`, `skills::context`, `CreateSkill`, and existing TUI
  completion wiring.
- Invariants: `SKILL.md` is the only automatically injected package entry;
  `$NEO_HOME/skills` remains the sole implicit user root; explicit roots remain
  supported; catalogs remain deterministic and complete.
- Compatibility: resources, `${NEO_SKILL_DIR}`, canonical slash invocation,
  model activation, tier precedence, Aegis symlink views, transcript events,
  and replay remain functional.

These records are advisory Method Pack evidence, not authoritative runtime
decisions or completion authority.

