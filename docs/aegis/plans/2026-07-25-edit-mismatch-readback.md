# Edit Match-Mismatch Readback Implementation Plan

## Basis And Boundary

Implement the approved
`docs/aegis/specs/2026-07-25-edit-mismatch-readback-brief.md` in the existing
Read and Edit owners. Do not add a second reader, fuzzy recovery, automatic
retry, schema changes, or presentation changes.

Code necessity: the current Edit result discards comparison text already held
by `edit.rs`; only a source change can expose that evidence to the model.
Decision: code-change.

Architecture review required: yes, limited to the model-visible error contract.
`edit.rs` remains the mismatch owner and `read.rs` remains the bounded text
rendering owner.

## TDD Route

- Mode: off.
- Decision: skipped.
- Strict authority: not applicable.
- Test posture: focused post-change regression.

## Tasks

1. Add a crate-private in-memory snapshot renderer to `tools/read.rs` that
   reuses Read's current caps, formatting, and safety checks.
2. Append that comparison snapshot and explicit continuation guidance only to
   match-count failures in `tools/edit.rs`.
3. Extend `tests/tool_files.rs` with one focused large same-file mismatch case
   covering staged evidence, exact remaining lines, next offset, and zero disk
   writes.

## Verification

```bash
cargo test --package neo-agent-core --test tool_files edit_match_mismatch_returns_bounded_comparison_snapshot -- --exact --nocapture
rustfmt --check --edition 2024 crates/neo-agent-core/src/tools/read.rs crates/neo-agent-core/src/tools/edit.rs crates/neo-agent-core/tests/tool_files.rs
git diff --check
```

Stop after the focused contract is proven. Do not widen into Read UI or Edit
card redesign.
