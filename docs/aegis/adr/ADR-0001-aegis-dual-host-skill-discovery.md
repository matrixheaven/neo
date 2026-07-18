# ADR-0001 - Aegis dual-host skill discovery

Status: `recorded-from-plan`
Date: `2026-07-18`

## Source Evidence

- Approved design and plan plus fresh Neo activation smoke and updater/doctor checks
## Context

Codex and Neo need one complete Aegis method pack without editable-copy drift; Neo must have one implicit user root while retaining explicit skill paths; current Aegis metadata also exercises Codex-tolerated unquoted scalar prose.

## Decision

Keep one canonical checkout at ~/.codex/aegis; expose updater-managed direct-child symlink views at ~/.agents/skills/aegis and ~/.neo/skills/aegis; make NEO_HOME/skills Neo's sole implicit user root while retaining extra_skill_dirs and skill_path; keep strict YAML primary and apply one bounded Codex-compatible scalar repair after strict failure.

## Alternatives Considered

- Rejected two editable Aegis checkouts because they drift; rejected an umbrella symlink because updater canonicalization loses distinct host views; rejected editing canonical Aegis metadata because updates would overwrite it.
## Consequences

- Both hosts share one update source and Neo loads symlinked skills; generated discovery views and a bounded external-metadata parser repair become maintained compatibility surfaces.
## Compatibility Boundary

Explicit extra_skill_dirs and skill_path remain supported; strict YAML errors remain errors when bounded repair cannot parse; no Superpowers aliases or mixed loading remain.

## Retirement Impact

Removed both Superpowers discovery trees and Neo's implicit .agents/skills discovery/trust path; no backup or compatibility alias is retained.

## Baseline Sync

- Needed: needed
- Target: docs/aegis/baseline/2026-07-18-aegis-dual-host-install.md
- Action: create snapshot
- Reason: The decision changes canonical ownership, host discovery contract, compatibility boundary, and installed host status.

## Evidence References

- docs/aegis/specs/2026-07-18-aegis-dual-host-install-design.md
- docs/aegis/plans/2026-07-18-aegis-dual-host-install.md
## Boundary

This ADR is an advisory Aegis Method Pack record. It does not grant completion authority or replace project-authoritative architecture sources.
