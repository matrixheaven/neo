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
