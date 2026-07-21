# Neo 0.1.1 RC1 release - Intent

## TaskIntentDraft

- Requested outcome: Run full CI, publish a timestamped RC1 tag, and create a clear GitHub Release covering changes since v0.1.0.
- Goal: Run full CI, publish a timestamped RC1 tag, and create a clear GitHub Release covering changes since v0.1.0.
- Success evidence:
- Local CI succeeds; main push CI succeeds; timestamped RC tag release workflow succeeds; GitHub Release notes and assets are verified.
- Stop condition: Done when the GitHub Release and assets are verified; otherwise stop as blocked, needs-verification, or scope-exceeded with a resumable checkpoint.
- Non-goals:
- Do not redesign release automation or alter unrelated user changes.
- Scope: Release records and notes, CI failures required to unblock release, scoped commits, main push, RC tag, and GitHub Release verification.
- Change kinds:
- release
- Risk hints:
- Tagging is irreversible without destructive cleanup; current remote CI is failing; shared worktree contains an unrelated .gitignore edit.

## BaselineReadSetHint

- .github/workflows/ci.yml
- .github/workflows/release.yml

## BaselineUsageDraft

- Required baseline refs:
- .github/workflows/ci.yml
- .github/workflows/release.yml
- Acknowledged before plan:
- none
- Cited in plan:
- none
- Missing refs:
- .github/workflows/ci.yml
- .github/workflows/release.yml
- Advisory decision: needs-baseline-readback

## ImpactStatementDraft

- Compatibility boundary: Cargo crate version remains 0.1.1; release identity is conveyed by the RC tag and GitHub prerelease flag.
- Affected layers:
- CI and release distribution
- Owners:
- .github/workflows/ci.yml and .github/workflows/release.yml
- Invariants:
- No tag before CI is green; preserve unrelated worktree changes; release remains marked prerelease.
- Non-goals:
- Do not redesign release automation or alter unrelated user changes.

These records are Method Pack drafts / hints, not authoritative runtime decisions.

## BaselineUsageDraft

- Required baseline refs:
- .github/workflows/ci.yml
- .github/workflows/release.yml
- Delivered context refs:
- none
- Acknowledged before plan:
- .github/workflows/ci.yml
- .github/workflows/release.yml
- Cited in plan:
- .github/workflows/ci.yml
- .github/workflows/release.yml
- Missing refs:
- none
- Advisory decision: continue
