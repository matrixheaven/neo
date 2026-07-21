# Neo 0.1.1 RC1 release - Checkpoint

- Task ID: 2026-07-21-release-0-1-1-rc1
- Current todo: Diagnose current main CI failure and establish release notes scope.
- Active slice: Release baseline and CI diagnosis.
- Blocked on: none
- Next step: Extract all blocking Clippy errors and compare the existing def5b78f commit before editing.

## Checkpoint Update

- Current todo: Run full local CI and then push the CI fix plus release records.
- Active slice: Full CI verification before any push or tag.
- Completed todos:
- Diagnosed and fixed Rust 1.97 Clippy blockers; committed 5b8eaa43.
- Drafted docs/releases/v0.1.1-rc.1.md with v0.1.0 baseline and v0.1.1 RC changes.
- Evidence refs:
- 5b8eaa43; docs/releases/v0.1.1-rc.1.md; .github/workflows/ci.yml; .github/workflows/release.yml
- Blocked on: none
- Next step: Run fmt, workspace clippy, neo-agent build, and workspace nextest in workflow order; stop before tagging if any fails.

## DriftCheckDraft

- Scope status: Release docs, CI repair, and release gates only; unrelated .gitignore edit preserved.
- Compatibility status: Cargo package version remains 0.1.1; RC identity is tag/release metadata.
- Retirement status: No compatibility or legacy path added; no retirement work triggered.
- New risk signals:
- Remote CI was red before the fix; fresh local and remote evidence still required.
- Advisory decision: continue

## Checkpoint Update

- Current todo: Commit release records, push main, and wait for matching remote CI.
- Active slice: Remote main CI gate before tag creation.
- Completed todos:
- Fixed all local CI blockers and committed scoped repairs through 6422d663.
- Final serial local CI passed: fmt, workspace Clippy, neo-agent build, and 2767 nextest tests.
- Drafted timestamped RC1 release notes at docs/releases/v0.1.1-rc.1.md.
- Evidence refs:
- final-local-ci; commits 5b8eaa43 56ffcea9 44875b5e 6422d663
- Blocked on: none
- Next step: Commit release records, push main, wait for the exact head SHA CI to pass, then create and push the annotated RC tag.

## DriftCheckDraft

- Scope status: Release, CI blockers, and durable release records only; unrelated .gitignore edit remains untouched.
- Compatibility status: Package version remains 0.1.1; RC identity and timestamp remain tag metadata.
- Retirement status: Old single-file Write test fixtures and contradictory symlink drift test retired; no compatibility branch added.
- New risk signals:
- Remote CI and six-platform release jobs remain unverified.
- Advisory decision: continue
