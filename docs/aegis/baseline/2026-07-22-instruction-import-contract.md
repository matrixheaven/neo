# Instruction Import Contract Baseline

Date: `2026-07-22`
Decision: `docs/aegis/adr/ADR-0002-non-blocking-instruction-import-cycles.md`

## Current State

- `InstructionResolver` is the sole owner of instruction entry discovery and
  recursive Markdown import expansion.
- The stored directory entry must be exactly `AGENTS.md`. Lowercase and
  mixed-case variants are ordinary files.
- Imported paths are canonicalized before traversal. Each canonical source body
  expands at its first occurrence only; repeated edges and cycle back-edges do
  not insert another body and do not block the bundle.
- New unique sources beyond depth, count, per-source byte, or graph-byte limits
  remain blocking structural errors. Missing, unreadable, invalid UTF-8,
  untrusted, and unstable sources also preserve atomic bundle failure.
- Successful new or changed scopes retain the existing instruction epoch and
  one-time tool-batch defer contract.

## Compatibility

- Historical JSONL events containing `include_cycle` or
  `ambiguous_agents_file` failure kinds remain readable.
- New resolution never produces those two failure kinds.
- Shell command strings remain opaque to instruction discovery; Bash and
  Terminal use their typed `cwd` argument or the primary workspace.

## Verification Owners

- `crates/neo-agent-core/tests/instruction_registry.rs` owns exact-name,
  source-once cycle, and structural-limit regressions.
- `crates/neo-agent-core/tests/session_jsonl.rs` owns historical session wire
  compatibility.
- `crates/neo-agent/src/trust.rs` owns project trust-input discovery behavior.

## Known Constraint

The historical failure-kind enum values remain visible in generated schemas and
presentation matches because existing session events may contain them. They are
compatibility carriers, not active resolver outcomes.
