# Neo 0.1.1 RC1 release - Evidence

No evidence has been recorded yet.

## EvidenceBundleDraft

- Artifact key: ci-failure-diagnosis
- Type: remote-log
- Source: gh run view 29799810016 --log-failed
- Summary: Remote CI stopped at Run clippy under Rust 1.97; blocking errors were needless_borrow in list.rs and items_after_test_module in guardian.rs.
- Verifier: Codex root-cause review

## EvidenceBundleDraft

- Artifact key: final-local-ci
- Type: command
- Source: cargo fmt --all --check; cargo clippy --workspace --all-targets --all-features -- -D clippy::all; cargo build -p neo-agent; cargo nextest run --workspace --all-features
- Summary: Final serial workflow-equivalent run exited 0: fmt passed; Clippy 0 errors; build 0 errors; nextest 2767 passed across 63 binaries with 0 failures.
- Verifier: Codex fresh local execution

## EvidenceBundleDraft

- Artifact key: rc1-release-failure
- Type: remote-log
- Source: GitHub Actions run 29805672416
- Summary: Linux ARM64 failed linking xpty's `close_range`; Windows x86_64 and ARM64 failed compiling neo-tui because base64 was only a dev dependency. The other three platform archives succeeded.
- Verifier: Codex failed-job log review

## EvidenceBundleDraft

- Artifact key: local-release-repair
- Type: command
- Source: Windows target cargo check; Linux ARM64 cargo zigbuild release; two exact nextest filters; fmt and targeted Clippy
- Summary: Windows target check exited 0; aarch64-unknown-linux-gnu release zigbuild exited 0; notification encoding and real PTY tests passed; fmt and both affected library Clippy checks exited 0.
- Verifier: Codex fresh local execution

## EvidenceBundleDraft

- Artifact key: builtin-skill-refresh-race
- Type: remote-log-and-test
- Source: main CI run 29807598057; exact btw follow-up test; concurrent built-in extraction test
- Summary: CI observed a truncated `.builtin/sub-skill/SKILL.md` during parallel refresh. The canonical writer now atomically creates or replaces each built-in file; both the original failing test and the new concurrency test pass.
- Verifier: Codex root-cause review and fresh local execution

## EvidenceBundleDraft

- Artifact key: rc2-release-complete
- Type: remote-workflow-and-release
- Source: GitHub Actions run 29808716841 and GitHub Release v0.1.1-rc.2+20260721.0634
- Summary: Release workflow completed successfully on all six target platforms. The public non-draft prerelease contains six non-empty archives, complete v0.1.0 baseline and v0.1.1 Added/Changed/Fixed notes, and explicitly supersedes incomplete RC1.
- Verifier: Codex fresh GitHub CLI verification

## EvidenceBundleDraft

- Artifact key: final-main-ci
- Type: remote-workflow
- Source: GitHub Actions run 29808374855 at ec47ee25265a62a7498b0a0f1d6975fbb9a75af1
- Summary: Main CI completed successfully: formatting, Clippy, release build, and 2773 tests all passed for the exact commit tagged as RC2.
- Verifier: Codex fresh GitHub CLI verification
