# Neo Daily Audit Remediation Design

**Date:** 2026-07-10

**Status:** Approved by the user's request to plan and implement all 20 findings from the 2026-07-10 read-only audit.

## Goal

Fix all 20 audited defects without retaining compatibility branches or parallel implementations. Each repair must establish one authoritative ownership or data path, add a regression test that fails before the fix, and preserve Neo's local-only, Rust-native, cross-platform architecture.

## Scope

The work covers six boundaries:

1. Provider authentication and HTTP error normalization.
2. Goal, configuration, background-task, and multi-agent state ownership.
3. MCP request, reconnect, process, and stderr lifecycle.
4. TUI input, rendering, image, and logging behavior.
5. PTY I/O, Unicode decoding, and process-tree cleanup.
6. Workspace identity and atomic configuration persistence.

The following are deliberately out of scope:

- Changes under `.references/`.
- Hosted services or remote state.
- Compatibility readers for newly corrected transient representations.
- Broad refactors unrelated to the 20 findings.
- Product behavior changes beyond what is needed to make the audited contracts true.

## Architecture

### Provider Boundary

`neo-ai` will have one helper for consuming non-success HTTP responses. It will retain a bounded, sanitized response-body excerpt and `Retry-After`, allowing every provider to map authentication, rate-limit, server, and context-overflow errors consistently. Google authentication will use `x-goog-api-key`; the API key will never be placed in a URL or be overrideable by user-supplied extra headers.

### Runtime State Ownership

Reloadable disk configuration and live session services will no longer be conflated. `AppConfig::load` may construct initial service handles, but `InteractiveController::refresh_config` must reattach the existing `BackgroundTaskManager`, `MultiAgentRuntime`, permission state, workspace policy, and startup overrides before replacing its configuration snapshot. Goal creation will have two non-overlapping operations: `start` rejects an existing active goal, while `replace` atomically removes the previous durable goal and installs the new one.

### MCP Lifecycle

`RmcpClient` will separate a clonable request peer from exclusive shutdown ownership, so no network request holds the lifecycle mutex. Every managed connection task will carry the attempt generation captured when it was created; stale tasks cannot be installed or observed as current. Removing or replacing a stdio server will await its registered cleanup. Stdio stderr will be drained into a bounded byte tail owned by the client and attached to startup, timeout, and unexpected-close diagnostics.

### TUI And Streaming State

One `RawStdinEvents` instance will own stdin for the complete interactive lifetime, including startup trust. Cross-workspace resume will use `neo resume <session-id>` as a product-level operation that resolves the recorded workspace, rather than emitting shell-specific `cd` snippets. `TuiRenderer` will use a transactional terminal guard and return write/flush failures before committing cached frame state. Kitty payloads will be normalized to PNG before `f=100` is emitted.

Swarm text and tool progress will travel as bounded incremental updates. Full `SwarmSnapshot` values remain lifecycle/checkpoint values, not token-level events. Background progress updates will be serialized through one ordered path, eliminating per-delta spawned tasks and stale overwrites.

### PTY Boundary

The global terminal registry will only locate per-session handles. Potentially blocking writer operations run outside the registry lock and outside Tokio worker threads. UTF-8 decoding will preserve incomplete suffixes across chunks and pagination boundaries. Terminal ownership will include a portable process-tree abstraction: Unix process groups and Windows Job Objects, isolated behind `cfg` modules with the same terminate-and-wait contract.

### Paths, Persistence, And Diagnostics

An OS-native path hashing helper in `neo-agent-core` will hash Unix bytes and Windows UTF-16 code units; both session buckets and project keys will use it. Configuration mutations will pass through one locked read-modify-write function and one same-directory atomic writer. Core libraries will not write directly to process stderr: recoverable state failures become typed state, while diagnostics use `tracing`. TUI debug output will use one per-process bounded log with monotonic frame identifiers rather than timestamp-named files per render step.

## Error Handling

- Provider error-body reads are bounded and sanitised before entering `AiError`.
- Failed persistent approvals or background output writes remain visible in typed state; they are not silently reduced to stderr text.
- Renderer state changes commit only after terminal writes and flushes succeed.
- MCP cleanup and PTY process-tree cleanup are awaited and bounded.
- Atomic configuration update errors leave the previous file intact.
- No portable branch may use `panic!`, `todo!`, or a silent no-op as its unsupported-platform behavior.

## Testing Strategy

Every production change follows RED-GREEN-REFACTOR with one narrow test command. Tests must prove the externally relevant contract rather than derived traits, struct round-trips, or library behavior.

- Provider tests use local HTTP listeners and assert request headers plus error classification.
- State tests reproduce replacement, reload, stale-generation, and out-of-order update sequences.
- I/O tests use deterministic failing/blocking writers and split UTF-8 byte sequences.
- Process tests use platform-gated child trees with portable assertions and bounded waits.
- Filesystem tests use temporary directories, non-UTF paths where supported, concurrent writers, and injected atomic-write failures.
- Rendering tests inspect protocol bytes and inject write failures; no real terminal is required.

## Delivery Order

Provider and state fixes land first because they have small, stable boundaries. MCP lifecycle follows before TUI/PTY because terminal and background execution depend on correct resource ownership. Path, persistence, streaming, and logging tasks finish the migration. Each task is independently reviewed for specification compliance and code quality before the next begins.

## Acceptance Criteria

- All 20 audit findings have a mapped task and a passing regression test.
- No Google credential appears in a request URL or formatted transport error.
- Context-overflow responses from every provider reach `AiError::ContextOverflow`.
- Config reload preserves all live runtime handles and active task visibility.
- Goal, MCP, renderer, PTY, and swarm lifecycles cannot regress through stale state.
- Windows, Linux, and macOS use explicit, maintainable platform boundaries.
- Old duplicate helpers, incomplete diagnostic fields, per-frame log files, and direct core `eprintln!` calls are removed rather than retained as fallbacks.
- Verification uses only the narrow package/target/filter commands recorded in the implementation plan.

## Self-Review

- Coverage: all 20 audit findings map to an architecture section and acceptance criterion.
- Placeholders: none.
- Internal consistency: reloadable configuration is distinct from live state; MCP and PTY lifecycle ownership is explicit; no dual implementation is proposed.
- Scope: large but decomposable into independently testable tasks in one umbrella plan, as explicitly requested by the user.
