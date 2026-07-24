# Neo 2026-07-24 Crates Audit Remediation Design

Date: `2026-07-24`
Status: `approved by explicit request to write and commit the spec and plan`
ArchitectureReviewRequired: `yes`

## 1. Goal

Remediate every high-confidence finding from the 2026-07-24 four-crate audit
through the existing canonical owner for each behavior. The result must remove
credential ambiguity, make RPC and turn lifecycles observable and cancellable,
bound untrusted streams, make multi-agent preparation atomic, preserve terminal
stream semantics, and make persistence/path behavior genuinely portable across
Windows, Linux, and macOS.

Success is not "tests pass" in isolation. Success requires the old duplicate or
unsafe path to be absent, the intended behavior to be proved through its real
consumer, and the platform-specific contract to remain explicit.

## 2. Requirement Basis

The approved requirement source is the 2026-07-24 read-only audit and the
user's instruction to convert all reported problems into a committed design
and implementation plan.

Required authority:

- `AGENTS.md`: scope, cross-platform, exact-test, Git, and canonical-owner
  rules.
- `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`: durable
  workflow truth, actual-usage-only cost semantics, and unchanged existing
  Delegate/Bash/Terminal behavior.
- `docs/aegis/specs/2026-07-20-bash-terminal-tool-card-brief.md`: Bash and
  Terminal inspectability and the fixed Delegate/DelegateGroup/DelegateSwarm
  card boundary.
- Current code at the finding paths. `.references/` is comparison-only and is
  not an implementation target.

Requirement Ready Check:

- Requirement source refs: complete.
- Goals and scope refs: complete.
- User/scenario refs: local CLI, TUI, RPC clients, provider streams, workflow
  recovery, and multi-agent orchestration.
- Acceptance criteria: defined in Section 13.
- Open blocker questions: none.
- Decision: `ready`.

## 3. Scope

### 3.1 Included findings

| ID | Finding | Canonical repair owner |
|---|---|---|
| F1 | Provider switch can reuse the previous provider's key env | Provider-specific `ProviderSpec` and invocation resolver |
| F2 | JSONL RPC buffers until stdin EOF; prompt failures kill the server | RPC server I/O loop |
| F3 | Dropped `AgentEventStream` leaves its detached turn running | `AgentEventStream` lifecycle |
| F4 | `DelegateSwarm` preparation partially commits before validation | `MultiAgentRuntime` |
| F5 | Provider SSE framing has no pending-frame limit | Shared `providers::common::sse` framer |
| F6 | TUI partial chunks are treated as complete lines | Transcript incremental output state |
| F7 | ShellRun has a second, weaker ANSI sanitizer | Existing canonical ANSI parser |
| F8 | Workflow metadata duplicates atomic persistence and skips Windows directory sync | `session::atomic_file` |
| F9 | Safe-directory creation validates only the leaf | `session::atomic_file` directory walker |
| F10 | Windows launch barrier interpolates a path into `cmd.exe` text | Windows process-tree barrier |
| F11 | Native non-Unicode paths fail in Windows replacement and session-index JSON | Lossless native-path wire and Windows wide API boundary |
| F12 | Git footer follows special files and loses Unix path bytes | Git-status worker |
| F13 | Google URL construction bypasses HTTP(S)-only validation | Shared provider URL helper |
| F14 | Public `env_api_keys` duplicates credential resolution | `CredentialResolver` / `ProviderSpec` |
| C1 | Duplicate path predicates, wrappers, and unused dependencies | Existing canonical helpers and manifests |
| C2 | Duplicate/trivial tests and fixed-port failure fixture | Behavior-owning integration tests |

### 3.2 Explicit non-goals

- No product redesign of provider selection, RPC methods, workflow UX, or
  Delegate-family cards.
- No new compatibility layer, legacy credential alias, fallback parser, or
  second path representation.
- No edits under `.references/`.
- No predictive token/cost/time/agent budgeting. SSE and file byte limits are
  machine-safety bounds on untrusted input, never workload governance.
- No migration or deletion of existing user sessions, workflow journals, or
  other persistent source-of-truth data.
- No broad formatting, Clippy cleanup, test reorganization, or dependency
  upgrade outside the named findings.
- No attempt to absorb unrelated dirty worktree changes.

## 4. Design Choice

Three approaches were considered:

1. Add caller-side guards around each symptom. This is rejected because it
   preserves duplicate owners and misses sibling callers.
2. Introduce new adapter layers while keeping old behavior for compatibility.
   This is rejected because no active external dependency justifies parallel
   internal paths.
3. Repair each behavior in its existing owner and delete the obsolete path.
   This is selected because it minimizes code, makes contracts testable at one
   boundary, and matches Neo's canonical-only policy.

Architecture Integrity Lens:

- Invariant: every credential, lifecycle, stream, persistence, and path
  decision has exactly one authoritative owner.
- Responsibility overlap: present in credentials, ANSI sanitization, workflow
  atomic writes, path predicates, and low-value duplicate tests.
- Higher-level simplification: repair shared owner once, migrate all consumers,
  then delete the duplicate.
- Compatibility falsifier: retain an old path only if a documented external
  consumer is proven before deletion. No such evidence exists for the internal
  paths in scope.
- Verdict: `repair canonical owner; delete duplicate owner`.

## 5. Credentials And Provider Transport

### 5.1 Provider-specific credentials

The top-level `AppConfig::api_key_env` credential path is retired. Credentials
belong only to the selected provider's `ProviderSpec`; a model switch changes
the selected spec and can neither mutate nor inherit another provider's
environment-variable list. Existing documentation and examples migrate to
`[providers.<id>].api_key_env`; no top-level compatibility alias or fallback is
retained.

The public `env_api_keys` table and its re-exports are retired. All credential
lookups, including Windows case-insensitive environment matching and Google key
precedence, flow through `CredentialResolver` and `ProviderSpec`.

### 5.2 Shared provider URL validation

Google must build its model path through the same shared helper as the other
providers. The helper accepts only `http` or `https` with a host. Google then
adds `alt=sse` through `Url::query_pairs_mut`; invalid configuration remains a
non-retryable URL/protocol error.

### 5.3 Bounded SSE framing

`providers::common::sse` becomes the single owner of pending bytes, frame
boundary detection, and the maximum incomplete-frame size. Provider-specific
code owns only JSON/event interpretation.

The bound protects Neo from an endpoint that never terminates a frame. It is a
fixed machine-safety input limit, not a model workload limit. Crossing it emits
one non-retryable protocol error and terminates the stream. The implementation
must consume frames without repeated prefix `Vec::drain` copying.

## 6. RPC And Turn Lifecycle

### 6.1 Incremental JSONL RPC

The RPC loop owns both input and output. It decodes one request line, writes all
notifications and the terminal response for that request immediately, flushes,
then reads the next request. EOF only closes the server; it is not a response
barrier.

Malformed input and request-scoped execution failures produce a structured
`RpcResponse::failure` and leave the server available for the next request.
Fatal stdin/stdout I/O remains process-fatal because protocol progress is no
longer observable.

`prompt` must stream events as they are produced. It must not collect the whole
turn and replay it after completion.

### 6.2 Cancel-on-drop event streams

`AgentEventStream` owns a clone of the per-turn cancellation token and tracks
whether the stream reached normal completion. Dropping an incomplete stream
cancels the turn. Reaching `Ready(None)` disarms the guard. Send failure is not
treated as an invisible successful consumer.

This preserves the existing rule that Bash/Terminal admission waits and
commands without explicit timeout remain unbounded. Cancellation occurs only
because the owning consumer disappeared or explicitly cancelled.

## 7. Atomic Multi-Agent Preparation

`MultiAgentRuntime` is the sole owner of agent lifecycle mutation. A new
runtime method accepts all new child descriptions and resume IDs, validates the
complete batch against one locked snapshot, and commits the state transition
only after every item is valid.

Validation includes duplicate IDs, unknown IDs, illegal lifecycle states,
capacity, and name/identity collisions. On failure there are no new agents, no
incremented `run_count`, no cleared outcome/activity, and no swarm registration.
The tool-level `prepare_swarm_children` mutation path is deleted after callers
migrate.

This changes runtime preparation only. Delegate, DelegateGroup, and
DelegateSwarm card content, layout, expansion, and transcript semantics remain
exactly unchanged.

## 8. TUI Streaming And ANSI Ownership

ToolCall and ShellRun keep an incomplete trailing text fragment across partial
events. Only terminated logical lines enter the visible bounded deque; the
tail is finalized exactly once when the tool ends. Chunk boundaries never
create line boundaries.

The bounded collection uses `VecDeque` or an equivalent O(1) front eviction;
it does not allocate every line and repeatedly call `remove(0)`.

All shell output sanitization calls the existing canonical ANSI state machine.
The weaker `utils::shell_output::sanitize_shell_output` parser is removed or
reduced to a direct canonical call with no parsing logic of its own. DCS, APC,
PM, SOS, OSC, CSI, C1 forms, split control sequences, and final unterminated
text follow one behavior.

No card redesign is authorized. This slice changes only output byte-to-line
projection inside the existing cards.

## 9. Persistence And Safe Paths

### 9.1 One atomic-file owner

Workflow `run.json` creation reuses
`session::atomic_file::write_file_atomic_create_new`. The atomic helper owns
temporary-file creation, cleanup, create-new publication, and parent-directory
sync on Unix and Windows. Workflow code maps `AtomicWriteStatus` into workflow
errors without reimplementing filesystem mechanics.

`journal.jsonl` remains append-only and syncs each durable record before the
corresponding external-effect/state boundary, as required by the RunWorkflow
baseline. This work must not weaken journal durability or add batching.

### 9.2 Ancestor-safe directory creation

`ensure_safe_directory_tree` reuses the existing component-wise directory
creation logic. Every existing ancestor is checked with `symlink_metadata`;
Unix symlinks, Windows reparse points/junctions, and non-directories are
rejected before creating descendants. No fallback calls `create_dir_all` on an
unvalidated chain.

All duplicated reparse/symlink predicates migrate to this canonical owner and
are deleted.

### 9.3 Lossless native paths

Windows replacement uses a safe, audited wrapper that accepts native `Path` or
wide-character input without converting through UTF-8. The implementation may
upgrade or extend an existing safe dependency, but Neo itself remains
`unsafe_code = "forbid"`. If no suitable safe API is available, implementation
stops for an explicit dependency decision; it must not add an unsafe block or a
remove-then-rename fallback.

The append-only session index uses a versioned lossless native-path wire field:
Unix bytes and Windows UTF-16 code units are encoded into JSON-safe text with a
platform tag. The reader accepts the existing Unicode `PathBuf` JSON shape and
the new lossless shape, but all new writes use only the new shape. This is a
read-compatibility exception for durable user data, not a second runtime owner.
Existing index files are not rewritten or migrated.

## 10. Windows Process Barrier

The launch barrier must not interpolate a native path into `cmd.exe` source.
The preferred design passes a generated environment variable as a separate
process environment entry and references the fixed variable name from the
loop, with command extensions disabled. If that cannot represent all native
paths losslessly, use an inherited synchronization handle instead.

There is one implementation path. Do not add quoting branches for `%`, `!`,
quotes, or spaces one character at a time.

## 11. Git Footer Safety

The Git-status worker parses NUL-delimited Unix paths into `OsString` from raw
bytes. Windows may decode the Git output only when lossless; an undecodable
entry is counted as one uninspectable file rather than converted lossy.

Before opening an untracked path, the worker uses `symlink_metadata` and accepts
only an in-workspace regular non-symlink file. Reads are bounded so device-like,
procfs-like, sparse, or unexpectedly large inputs cannot monopolize the
background worker. FIFO, socket, device, symlink, and over-limit paths count as
one uninspectable untracked file. The footer must continue refreshing after
such an entry.

## 12. Simplification And Test Value

After behavior repairs:

- Delete the public `env_api_keys` module and tests.
- Delete duplicate reparse/symlink predicates and trivial forwarding wrappers.
- Remove direct dependencies proved unused by a fresh dependency scan and
  zero-reference search; update `Cargo.lock` only through Cargo.
- Delete duplicate trust-dialog tests when the integration test covers the
  same states through the real overlay.
- Delete tests that only prove serde derive round trips, field setters/defaults,
  or a cosmetic output flag already covered by a stronger test.
- Replace the fixed `127.0.0.1:1` failure assumption with a bound local mock
  that deterministically returns the intended failure.

Tests that protect wire JSON shape, provider-visible schema, lifecycle order,
rendered output, or platform behavior remain. Line-count reduction is evidence
of cleanup, not an acceptance target.

## 13. Acceptance Criteria

1. Switching from an OpenAI model to an Anthropic model resolves only the
   Anthropic provider's configured credentials; no top-level credential owner
   or cross-provider override remains.
2. No production export or call site for `env_api_keys` remains.
3. An RPC client receives and can parse the first response while stdin stays
   open; a failed prompt returns a matching failure and the next request works.
4. Dropping an incomplete `AgentEventStream` cancels its turn and prevents any
   later tool effect; normally draining a stream preserves normal completion.
5. A failing swarm batch leaves every preexisting and proposed agent unchanged.
6. Every provider rejects an oversized unterminated SSE frame with a protocol
   error and accepts frames split at every byte boundary.
7. Google rejects non-HTTP(S)/hostless URLs before transport retry logic.
8. Partial TUI chunks reconstruct exact logical lines, use bounded O(1)
   eviction, and sanitize all ANSI/control-string forms through one owner.
9. Delegate-family cards render identically before and after the TUI repair.
10. Workflow metadata creation uses the shared atomic helper and reports
    committed-unsynced state accurately; journal ordering/durability is
    unchanged.
11. Ancestor symlinks/reparse points are rejected before child creation.
12. Native non-Unicode path fixtures round-trip through Windows replacement,
    Unix session index, and platform-appropriate index decoding.
13. A Windows barrier path containing `%NAME%`, spaces, and `!` releases the
    intended child without command-text path interpolation.
14. FIFO/symlink/large/non-UTF untracked files cannot hang Git footer refresh or
    cause a lossy lookup.
15. Focused deletion scans show no duplicate sanitizer, credential table,
    workflow atomic writer, reparse predicate, or obsolete wrapper.
16. All verification follows the repository's narrow package/target/filter
    rule; native Windows and Linux checks are performed for platform-only code.

## 14. Compatibility And Retirement

Anti-Entropy Declaration:

- Deletion class: internal code retirement plus durable-contract repair.
- Old paths: synthesized global provider env, `env_api_keys`, buffered RPC
  output, tool-level partial swarm mutation, provider-local SSE buffer
  ownership, weak ANSI parser, workflow-local atomic metadata writer, duplicate
  path predicates, lossy path conversions, and low-value tests.
- New canonical owners: existing `ProviderSpec`/`CredentialResolver`, RPC loop,
  `AgentEventStream`, `MultiAgentRuntime`, common SSE/HTTP helpers, canonical
  ANSI parser, `session::atomic_file`, and native-path wire.
- External boundary touched: RPC behavior and durable session-index encoding.
- Source-of-truth data risk: existing session index data must remain readable;
  no data deletion or rewrite is authorized.
- User confirmation required for code retirement: no.

Retirement Decision:

- Path: `delete-first` for internal duplicate owners.
- Exception: `compat-exception` only for reading existing Unicode session-index
  records; new writes use one lossless format.
- Non-edits: no persistent-state deletion, no migration, no `.references`
  changes, and no fallback branches for malformed new data.

## 15. Planning Readback

TaskIntentDraft:

- Outcome: all audited boundaries are repaired without adding parallel owners.
- Success evidence: Section 13 plus exact platform tests and lingering-reference
  scans.
- Stop condition: all mapped tasks are implemented, reviewed, narrowly
  verified, and committed; unrelated failures are reported rather than fixed.

BaselineUsageDraft:

- Required refs: `AGENTS.md`, RunWorkflow runtime baseline, shell-card spec,
  this design.
- Missing refs: none.
- Decision: `continue`.

ImpactStatementDraft:

- Affected layers: `neo-ai`, `neo-agent-core`, `neo-tui`, `neo-agent`, crate
  manifests, focused tests, and durable session-index decoding.
- Canonical owners: enumerated in Section 3.1.
- Compatibility: existing RPC methods, session data, workflow journals, TUI
  cards, Bash/Terminal execution, and provider catalog configuration remain.
- ArchitectureReviewRequired: `yes`.
- ADR signal: `yes`; completion review should decide whether the lossless path
  wire and cancel-on-drop stream lifecycle warrant an ADR/baseline update after
  implementation evidence exists.

## 16. Self-Review

- Placeholder scan: no placeholders or unresolved choices.
- Coverage: every reported finding and cleanup category maps to an owner and an
  acceptance criterion.
- Internal consistency: machine-safety bounds do not become cost governance;
  RPC streaming does not change method semantics; journal durability is not
  weakened; card design remains fixed.
- Minimality: shared owners are reused; no dependency or adapter is proposed
  where existing code suffices.
- Destructive boundary: only internal code/tests are retired. Durable user data
  is read-compatible and never deleted or rewritten by this work.
