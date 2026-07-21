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
