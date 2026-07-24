# Clippy Complexity Governance Design

Status: Approved
Date: 2026-07-24

## Goal

Remove every local `#[allow(clippy::too_many_lines)]` and
`#[allow(clippy::too_many_arguments)]`, fix the four known CI failures, and make
both lint exemptions impossible to reintroduce.

## Baseline

- The workspace contains 68 matching attributes across 32 files: 48
  `too_many_lines` hits and 26 `too_many_arguments` hits, with 6 combined.
- CI currently runs Clippy with `-A clippy::pedantic`, so `too_many_lines` is
  globally disabled; item-level `allow` overrides the `clippy::all` setting for
  `too_many_arguments`.
- Command-line `-F` for both lints rejects every local override with `E0453`.
- The approved scope is all production and maintained test code. No justified
  local exception remains.

## Approved Approach

1. Repair the four known failures at their canonical owners before refactoring.
2. Remove the lint attributes in owner-based batches.
3. Group arguments only when they already form one semantic input/state unit.
4. Extract only cohesive phases from long functions. Do not create generic
   context bags, traits, factories, compatibility wrappers, or helper layers
   whose only purpose is moving lines elsewhere.
5. Keep scenario tests readable by extracting repeated setup or named phases,
   not by scattering one scenario across unrelated fixtures.
6. After all attributes are gone, set both workspace lints to `forbid`.
7. Keep the remaining pedantic CI policy unchanged; this task does not expand
   into a whole-pedantic cleanup.

## Four Failure Repairs

### Session writer timeouts

The two `neo-agent` tests keep a `JsonlSessionWriter` alive after `flush()` and
then try to open the same session again. The writer lifetime intentionally owns
the session lock. Drop the seed writer in each fixture; do not weaken locking or
change `flush()` semantics.

### Goal revision event assertion

`ToolExecutionFinished` is emitted correctly. A helper extracted in
`30cba1a2` constructs `User requested Goal. Goal mode remains active revisions`
instead of `User requested revisions. Goal mode remains active.` Fix the shared
revision message helper and preserve typed approval propagation.

### Workflow event forwarding assertion

The failing test still invokes the retired synchronous
`RunWorkflow {title, script}` contract. Current production dispatch routes
nested events correctly and current durable-background tests cover active route,
stream close, drain, idle routing, and projection. Delete the obsolete test and
its single-use probe rather than restoring the old contract.

## Ownership And Compatibility

- Session locking remains owned by `JsonlSessionWriter`.
- Approval response validation remains owned by the canonical approval and
  permission pipeline.
- `WorkflowRuntime` remains the durable workflow owner; session/TUI events stay
  projections.
- Delegate, DelegateGroup, DelegateSwarm, Bash, Terminal, transcript cards,
  provider schemas, persistence formats, and public CLI behavior do not change.
- Refactors must remain portable Rust with no new OS-specific assumptions.

## Anti-Entropy Declaration

- Deletion class: `code-retirement`.
- Old path: obsolete synchronous RunWorkflow test and its private probe.
- Canonical owner: durable background workflow dispatch and its current tests.
- Preserved behavior: nested workflow tool events route to the active stream and
  the stream closes after dispatch.
- Retired behavior: direct synchronous `{title, script}` workflow execution.
- External boundary touched: no.
- Source-of-truth data risk: none.
- User confirmation required: no.
- Decision: `delete-first`; no compatibility exception.

## Acceptance

- Zero matching local attributes remain under `crates/`.
- Both lints are workspace `forbid` rules inherited by all four crates.
- The two timeout tests complete without weakening the session lock contract.
- Goal reject/revise behavior emits the expected terminal result text.
- Obsolete workflow test/probe are absent and current routing regressions pass.
- Target-specific Clippy, formatting, binary build, and focused regressions pass.
- No unrelated cleanup, dependency addition, public contract change, or test
  timeout increase is introduced.

