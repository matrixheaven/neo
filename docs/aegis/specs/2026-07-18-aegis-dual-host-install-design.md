# Aegis Dual-Host Install and Neo Skill Discovery Design

Status: Implemented

## Goal

Replace the active Superpowers skill packs in Codex and Neo with the complete
Aegis Method Pack, while making `~/.neo/skills/` Neo's only implicit user skill
root.

## Confirmed Requirements

- Codex and Neo both expose the complete Aegis skill set.
- Superpowers is removed from both host discovery trees; mixed loading is not
  allowed.
- Neo keeps `extra_skill_dirs` and `skill_path` as explicit configuration.
- Neo keeps `~/.neo/skills/` as its implicit user skill root.
- Neo stops implicitly scanning `~/.neo/.agents/skills/`.
- Feature specs and plans are canonical under `docs/aegis/specs/` and
  `docs/aegis/plans/`.
- One canonical Aegis checkout supplies both hosts so updates cannot drift.

## Pre-Install State

- Codex loads Superpowers from `~/.agents/skills/superpowers/`.
- Neo loads Superpowers from `~/.neo/skills/superpowers/`.
- Aegis is not installed in either host.
- Codex multi-agent support is already enabled.
- Neo's current config has no extra skill directories.
- The checked-out Aegis source is at the current upstream `main` commit.

## Neo Discovery Contract

Neo has exactly three supported skill inputs:

1. `~/.neo/skills/` (or `$NEO_HOME/skills/`) as the only implicit user root.
2. `extra_skill_dirs` as explicit configured roots.
3. `skill_path` as explicit configured roots.

The implicit `$NEO_HOME/.agents/skills/` root is deleted. Project-local
`.agents/skills/` is also removed from trust-input detection because Neo does
not load it. An explicitly configured extra path may still point anywhere,
including a directory named `.agents/skills`.

Built-in skills remain extracted below `$NEO_HOME/skills/.builtin/`. Existing
priority remains `user > extra > builtin`.

## Codex-Compatible Frontmatter

Neo keeps strict YAML parsing as the primary path. If strict parsing fails, it
applies the same bounded, line-oriented scalar repair used by Codex for
third-party skills whose unquoted prose contains colon-space or invalid flow
prefixes, then parses once more. Unrelated invalid YAML still returns the
original strict-parser error.

This is required by the current Aegis `test-driven-development` description
and keeps the canonical Aegis checkout clean and updater-compatible.

## Installation Topology

The single canonical method-pack checkout is:

```text
~/.codex/aegis/
```

Host discovery views are updater-managed directories whose direct children are
symlinks into that checkout:

```text
~/.agents/skills/aegis/<skill> -> ~/.codex/aegis/skills/<skill>
~/.neo/skills/aegis/<skill>    -> ~/.codex/aegis/skills/<skill>
```

Real discovery-root directories preserve distinct host paths in the Aegis
registry; an umbrella symlink would be canonicalized to its target by the
updater and could not verify each host view independently.

The following obsolete owners are deleted after the canonical Aegis checkout
passes its initial doctor check:

```text
~/.agents/skills/superpowers/
~/.neo/skills/superpowers/
```

No backup or compatibility copy is retained. Other installed skills are
untouched.

## Complete Method-Pack Support

The full checkout is retained, rather than copying individual `SKILL.md`
directories, so both hosts can use:

- all Aegis skills and skill-relative resources
- `aegis-doctor.py`
- `aegis-update.py` and its host registry
- `aegis-workspace.py`
- project-local `docs/aegis/` checkpoints, evidence, drift records, and ADR
  hints
- subagent workflows supported by each host

`~/.config/aegis/config.toml` records the canonical method-pack and workspace
helper. `~/.config/aegis/installations.json` has separate `codex` and `neo`
host registrations pointing to the two discovery views.

Aegis remains a method pack. This installation does not claim the future Aegis
Runtime Core or authoritative completion decisions.

## Execution Order

1. Update Neo's discovery and trust contract, tests, and zh/en docs.
2. Run the narrow Neo tests and build the `neo` binary.
3. Install the rebuilt Neo binary.
4. Clone the canonical Aegis method pack and run doctor before changing either
   active host view.
5. Remove both Superpowers discovery directories.
6. Create the Codex and Neo direct-child discovery roots through the updater.
7. Write Aegis config and register both host installations.
8. Restart or launch fresh host sessions and run discovery/activation smoke
   checks.
9. Run workspace-helper and updater status checks.

If the canonical checkout or initial doctor check fails, the active
Superpowers directories are not removed. Once replacement begins, failures are
repaired forward to the canonical Aegis layout rather than by restoring a
second workflow pack.

## Verification

Neo source verification:

- one exact discovery test proving `$NEO_HOME/skills` is the sole implicit root
- one exact trust-input test proving `.agents/skills` is not a project trust
  input
- one exact config test proving `extra_skill_dirs` and `skill_path` still load
- one exact parser test using the current Aegis colon-space description
- the existing invalid-YAML test still rejects unrelated malformed metadata
- one explicit `neo-agent` binary build

Installation verification:

- Aegis doctor reports `ok: true`, `workspaceSupport: available`, and
  `configStatus: configured`
- every skill in both discovery roots links to the canonical current Aegis
  skills tree
- neither host discovery tree contains Superpowers
- Codex and Neo can explicitly activate `using-aegis`
- Aegis workspace helper passes an init/check smoke in a temporary target
- updater status reports both `codex` and `neo`
- Aegis workspace validation passes with every spec and plan indexed

## Post-Install Documentation Migration

After the host replacement completed, the user authorized a hard migration of
all tracked feature specs and plans from the legacy Superpowers documentation
tree into the Aegis workspace. The migration preserves document intent, updates
executable skill/path references, and retains no compatibility directory.

## Non-Goals

- Do not reinterpret or discard unfinished feature plans during path migration.
- Do not copy Aegis into two editable checkouts.
- Do not remove Neo's explicit extra skill directory configuration.
- Do not add a general plugin framework or speculative Aegis runtime adapter.
- Do not preserve Superpowers aliases, fallback paths, or mixed skill names.

## Self-Review

- No placeholders or unresolved choices remain.
- The discovery contract distinguishes implicit roots from explicit paths.
- The installation has one canonical owner and two generated host views.
- Failure handling prevents removing the working pack before Aegis validates.
- Verification covers discovery, resources, updater, workspace support, and
  the protected documentation boundary.
