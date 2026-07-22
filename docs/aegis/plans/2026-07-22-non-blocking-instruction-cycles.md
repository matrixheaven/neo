# Non-Blocking Instruction Import Cycles Implementation Plan

Date: `2026-07-22`
Spec: `docs/aegis/specs/2026-07-22-non-blocking-instruction-cycles-design.md`
Execution: `inline`

## Scope

Implement exact `AGENTS.md` entry discovery and canonical-source-once import
expansion at the existing instruction resolver owner. Preserve only serialized
historical failure-kind values needed to rehydrate old sessions. Update current
documentation and leave unrelated runtime/workflow work untouched.

## Readiness

- Requirement Ready Check: `ready`; the user approved the recommendation and
  its written design without open behavior questions.
- Change Necessity: `code-change`; documentation cannot stop the deterministic
  false-positive scope and blocking cycle failure.
- Canonical owner: `crates/neo-agent-core/src/instructions/resolver.rs`.
- Compatibility: old JSONL failure kinds remain deserializable; no live state is
  rewritten.
- Retirement: delete case-folded discovery and cycle-error production; retain
  only the serialized historical enum values.
- TDD Route: mode `off`, decision `skipped`, posture `post-change regression`.

## File Map

- Modify `crates/neo-agent-core/src/instructions/resolver.rs`: exact entry
  selection, remove ambiguous/cycle error production and recursion stack.
- Modify `crates/neo-agent-core/src/instructions/types.rs`: remove retired
  runtime error variants while retaining historical serialized failure kinds.
- Modify `crates/neo-agent-core/tests/instruction_registry.rs`: exact-name and
  cyclic-graph regression coverage; retire obsolete failure assertions.
- Modify `crates/neo-agent-core/tests/session_jsonl.rs` only if existing epoch
  fixtures do not already anchor historical `include_cycle` deserialization.
- Modify `crates/neo-agent/src/trust.rs`: remove ambiguity handling/tests and
  assert exact-name trust discovery.
- Modify `AGENTS.md`, `docs/en/customization/agents.md`, and
  `docs/zh/customization/agents.md`: publish the new contract.

## Tasks

### 1. Change the resolver owner

1. Make `select_agents_file_name` select only the exact `AGENTS.md` directory
   entry and simplify its return type so it cannot report ambiguity.
2. Update `find_agents_file` and callers to the simplified selector.
3. Remove the import recursion stack and `IncludeCycle` error. Keep the existing
   canonical `visited` insertion as the single termination/deduplication rule.
4. Remove runtime-only `InstructionError` variants and their mappings. Keep
   `InstructionFailureKind::{IncludeCycle, AmbiguousAgentsFile}` for historical
   session decoding only.

### 2. Replace obsolete tests with regression evidence

1. Replace case-fold collision expectations with an exact-name fixture proving
   lowercase/mixed-case files are ignored and exact uppercase is selected.
2. Replace the cycle failure case with one indirect Markdown-link cycle that
   resolves successfully and contains every unique source body once.
3. Confirm registry reconciliation returns a successful epoch, then proceeds
   after applying it, proving the scope cannot remain policy-blocked.
4. Retain one historical JSON fixture only if needed to pin old failure-kind
   deserialization.

### 3. Update the public contract

1. Remove case-insensitive/ambiguous discovery wording from the agent guide and
   English/Chinese customization docs.
2. State that repeated and cyclic imports expand each canonical source once.
3. Remove cycle and ambiguous filename conditions from current failure tables;
   do not document historical wire-only variants as supported runtime failures.

### 4. Verify and commit

Run only the narrow targets that prove touched behavior:

```bash
cargo nextest run -p neo-agent-core --test instruction_registry <exact-name-filter>
cargo nextest run -p neo-agent-core --test instruction_registry <cycle-filter>
cargo nextest run -p neo-agent-core --test session_jsonl <historical-filter>
cargo test --package neo-agent --bin neo -- <exact-trust-test> --exact --nocapture --include-ignored
cargo fmt --all --check
```

If unrelated shared-worktree compilation blocks the `neo-agent` target, report
it and do not repair or revert that work. Run lingering-reference searches for
runtime production of `IncludeCycle`, `AmbiguousAgentsFile`, and case-folded
selection before committing.

## Stop Conditions

- Stop if exact entry casing cannot be observed portably from `read_dir`.
- Stop if removing an internal error variant breaks historical epoch decoding
  despite retaining the serialized failure-kind variant.
- Stop rather than weakening unrelated atomic import validation.

## Architecture Review Signal

After verification, evaluate whether the original path-scoped instruction ADR
or baseline needs an amendment. The implementation must not create a second
resolver, runtime fallback, session migration, or compatibility adapter.
