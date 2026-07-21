# Proof Bundle - 2026-07-21-release-0-1-1-rc1

## Method Pack Boundary

This proof bundle is an advisory Aegis Method Pack record. It does not determine evidence sufficiency, produce authoritative `GateDecision`, or grant `completion authority`.

## Task Intent

- Requested outcome: Run full CI, publish a timestamped RC1 tag, and create a clear GitHub Release covering changes since v0.1.0.
- Scope: Release records and notes, CI failures required to unblock release, scoped commits, main push, RC tag, and GitHub Release verification.

## Impact

- Compatibility boundary: Cargo crate version remains 0.1.1; release identity is conveyed by the RC tag and GitHub prerelease flag.
- Non-goals:
- Do not redesign release automation or alter unrelated user changes.

## Evidence Bundle Refs

- docs/aegis/work/2026-07-21-release-0-1-1-rc1/evidence-bundle-draft-ci-failure-diagnosis.json
- docs/aegis/work/2026-07-21-release-0-1-1-rc1/evidence-bundle-draft-final-local-ci.json
- docs/aegis/work/2026-07-21-release-0-1-1-rc1/evidence-bundle-draft-final-main-ci.json
- docs/aegis/work/2026-07-21-release-0-1-1-rc1/evidence-bundle-draft-rc2-release-complete.json

## Drift Check

- Scope status: Release, CI repairs, and durable evidence only; the user .gitignore edit remains untouched.
- Compatibility status: Cargo version remains 0.1.1; RC identity is tag metadata and GitHub prerelease state.
- Retirement status: The incompatible xpty path remains removed; portable-pty is the sole PTY owner.
- Advisory decision: continue
