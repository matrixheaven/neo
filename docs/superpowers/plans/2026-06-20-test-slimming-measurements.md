# Test Slimming Measurements

## Baseline

- Source test function count: 856
- Date: 2026-06-20
- Commit: 14dcc15d (plan)

## Final

- Source test function count: 826
- Date: 2026-06-20
- Net removed test functions: 30

## Completed Tasks

- Task 1: Consolidate Xtask Parity Stale-Claim Tests (xtask/src/main.rs: 55→34)
- Task 2: Consolidate Xtask Auth Token Leak Tests (xtask/src/main.rs: included above)
- Task 3: Consolidate Question Dialog TUI Tests (todo_question.rs: 16→12)
- Task 4: Consolidate Primitive ANSI/Wrap Tests (primitives.rs: 37→37; 1 added, 1 deleted)
- Task 5: Consolidate CLI Session ID Rejection Tests (cli_commands.rs: 39→38)
- Task 6: Consolidate RPC Session ID Rejection Tests (rpc_mode.rs: 18→15)
- Task 7: SKIPPED — no shared retry server helper existed
- Task 8: Consolidate Runtime Capability Rejection Tests (helpers only, same test count)
- Task 9: Consolidate Runtime Approval Decision Mirror Tests (helpers only, same test count)
- Task 10: Interactive Controller Test Triage Only (interactive.rs: 89→88)
- Task 11: Final Measurement And Verification

## Files Touched

- xtask/src/main.rs
- crates/neo-tui/tests/todo_question.rs
- crates/neo-tui/tests/primitives.rs
- crates/neo-agent/tests/cli_commands.rs
- crates/neo-agent/tests/rpc_mode.rs
- crates/neo-agent-core/tests/runtime_turn.rs
- crates/neo-agent/src/modes/interactive.rs