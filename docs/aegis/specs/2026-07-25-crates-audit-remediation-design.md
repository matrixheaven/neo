# Neo 2026-07-25 Crates Audit Remediation Design

Date: `2026-07-25`
Status: `approved by user on 2026-07-25`
ArchitectureReviewRequired: `yes`

## 1. Goal

Remediate the high-confidence findings from the 2026-07-25 read-only audit of
`neo-ai`, `neo-agent-core`, `neo-tui`, and `neo-agent`. Each repair must live in
the existing canonical owner, retire the duplicate or misleading path, and
preserve Neo's local-only, Rust-native, cross-platform design.

Success requires more than adding guards or tests. The incorrect owner must be
absent, state transitions and errors must remain observable, persisted history
must not be reconstructed from mutable live state, and Windows, Linux, and
macOS must each have an explicit supported behavior.

## 2. Requirement Basis

The requirement source is the 2026-07-25 crates audit plus three explicit user
decisions:

1. Delete the built-in Anthropic `ANTHROPIC_OAUTH_TOKEN` declaration. Keep
   API-key authentication only; do not add an OAuth compatibility path.
2. Make `ListDelegates` the sole delegate/swarm discovery owner. `TaskList`
   must stop synthesizing delegate and swarm entries.
3. For an old session without a persisted plan snapshot, show only the
   `ExitPlanMode` card/header. Never read the current workspace to fabricate a
   historical plan body.

Required authority:

- `AGENTS.md`: scope, canonical-owner, cross-platform, exact-test, subagent,
  and Git rules.
- `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md`: durable
  workflow ownership and recovery semantics.
- Existing Delegate/DelegateGroup/DelegateSwarm presentation contracts: card
  content, layout, and expansion behavior remain unchanged.
- Current source at the paths named below. `.references/` is comparison-only
  evidence and is never an implementation target.

Requirement Ready Check:

- Requirement source refs: complete.
- Canonical owners: identified for every included finding.
- User-facing choices: resolved.
- Acceptance criteria: defined in Section 13.
- Open blocker questions: none.
- Decision: `ready for design approval`.

## 3. Scope

### 3.1 Included findings

| ID | Finding | Canonical repair owner |
|---|---|---|
| F1 | Provider environment collection panics on an unrelated non-UTF-8 entry | `ProviderRegistry` environment snapshot |
| F2 | Built-in Anthropic advertises OAuth but always emits `x-api-key` | Built-in Anthropic `ProviderSpec` |
| F3 | Anthropic can emit tool args/end without a valid start | `StreamingToolCallAssembler` |
| F4 | Catalog HTTP errors bypass shared status classification and successful bodies are unbounded | Shared provider HTTP classifier plus catalog reader |
| F5 | Interactive catalog discovery launches each request twice | `pending_catalog_fetch` task owner |
| F6 | Session metadata mutators perform unlocked whole-file read-modify-write | `SessionMetadataStore` |
| F7 | Workflow/delegate/swarm worker panic can leave durable state `Running` | `WorkflowRuntime` and `MultiAgentRuntime` |
| F8 | One unrecoverable workflow journal prevents sibling runs from rehydrating | `WorkflowRuntime::rehydrate_run_entry` |
| F9 | `MessageDelegate` can report delivery after the live receiver was unregistered | `MultiAgentRuntime` live-steer registry |
| F10 | HTTP MCP authentication is inferred from error text containing `401` | HTTP/OAuth transport typed error boundary |
| F11 | `TaskList` reads historical logs and duplicates delegate/swarm discovery | `TaskList`, `TaskOutput`, and `ListDelegates` boundaries |
| F12 | Plan replay infers history from tool arguments and current workspace files | Persisted `ToolExecutionFinished.result.details` |
| F13 | Notification failures write directly to stderr and transient child failure disables future notifications | Notification runtime plus existing tracing capture |
| F14 | Windows VT input-mode query failure silently enters TUI without a valid input contract | Windows terminal-mode entry guard |
| F15 | Clipboard subprocess I/O blocks the interactive controller without a deadline | Interactive controller-owned clipboard task |
| F16 | Path completion recognizes only `/`, breaking native Windows input | `prompt_completion` path splitter |
| C1 | Dead inline-image render-cache side channel remains production-shaped | Main inline-image rendering path |
| C2 | Small forwarding abstractions, duplicate lexical helpers, escaping logic, and low-value tests remain | Existing local canonical helpers |

### 3.2 Explicit non-goals

- No real Anthropic OAuth implementation. It requires a separate design for
  login, refresh, credential kind, headers, request shaping, and lifecycle.
- No provider credential heuristic based on token prefix or environment name.
- No schema change to `AgentEvent`, `AgentMessage`, or `ToolResult`; no old
  session JSONL migration or rewrite.
- No compatibility branch for retired internal paths or duplicate state owner.
- No redesign of Delegate, DelegateGroup, or DelegateSwarm cards.
- No change to Bash/Terminal admission: queued calls stay pending. Commands
  without explicit timeout/cancel remain allowed to run indefinitely.
- No unified platform abstraction spanning catalog, clipboard, and completion;
  they have separate owners and failure contracts.
- No edits under `.references/`, dependency upgrade, broad lint cleanup, or
  unrelated dirty-worktree repair.
- No predictive token, cost, time, or agent governance.

## 4. Design Choice

The selected approach is `repair canonical owner; delete duplicate owner`.
Caller-side guards are rejected because they leave sibling paths inconsistent.
New compatibility adapters are rejected because the retired paths are internal
and no external consumer justifies two implementations.

Architecture invariants:

- Authentication kind is explicit; an API key is never relabeled as OAuth.
- Every durable mutation and terminal transition has one owner.
- Historical presentation comes only from persisted historical data.
- A task list is metadata; output content is fetched explicitly by ID.
- Platform-specific setup either establishes its contract or fails entry. It
  never silently enters a partially functional mode.
- Fixed byte/time bounds protect short-lived helpers and untrusted inputs only;
  they do not alter long-running agent or shell workload semantics.

## 5. Provider And Catalog Contracts

### 5.1 Loss-tolerant environment snapshot

`ProviderRegistry::resolver` collects with `std::env::vars_os()`. A pair enters
the resolver snapshot only when both key and value convert losslessly to UTF-8.
An unrelated non-UTF-8 entry is skipped; it must not panic or block resolution
of a valid target credential. If the requested credential is unavailable, the
existing typed configuration error remains authoritative.

A small private collection helper is permitted to make this deterministic to
test. It must not mutate the process environment or add lossy conversion.

### 5.2 Retire pseudo Anthropic OAuth

The built-in Anthropic provider lists only `ANTHROPIC_API_KEY`. Inline
`api_key` and explicit `api_key_env` retain API-key semantics and continue to
use `x-api-key` with the existing sensitive-header handling.

Delete `ANTHROPIC_OAUTH_TOKEN` from the built-in provider and update tests that
currently advertise it. Do not add `Authorization: Bearer`, token-prefix
sniffing, source-name inspection, an OAuth adapter, an alias, or fallback.

### 5.3 One streaming tool-call assembler

Delete Anthropic's manual `tool_args` and `block_tool_ids` lifecycle state.
Translate Anthropic content-block start and input-delta events into
`ToolCallChunk` values consumed by the existing `StreamingToolCallAssembler`;
finish through `finish_all()`.

A tool call without a name is a provider protocol error. It cannot emit
`ToolCallArgsDelta` or `ToolCallEnd` without a preceding valid start. Existing
shared buffering of arguments-before-name remains the only implementation and
must not be copied into Anthropic-specific code.

### 5.4 Catalog HTTP classification and body limit

Catalog non-success responses reuse the existing provider HTTP status
classifier, including typed authentication, rate-limit, server, and
`Retry-After` behavior. Catalog code must not reproduce status mapping.

A successful response has one fixed 16 MiB body limit. `Content-Length` may
reject early, but actual chunks must still be counted because the header may be
absent or false. Crossing the limit or failing JSON decode is a non-retryable
protocol error. Connection, timeout, and body transport failures retain their
transport classification for successful-response body reads. For a non-success
response, the known HTTP status remains authoritative even if its diagnostic
body cannot be read completely; existing 64 KiB best-effort error-body
truncation remains owned by the shared HTTP helper.

### 5.5 One interactive catalog request

At both catalog fetch call sites, delete the detached `_handle` spawn. Retain
only the task stored in `pending_catalog_fetch`. Do not introduce a fetcher
trait, a request counter, or another timeout; the catalog client already owns
connect and request deadlines.

## 6. Runtime And Persistence Contracts

### 6.1 Atomic session metadata mutation

`SessionMetadataStore` gains the sole internal `mutate_metadata` read-modify-
write operation. It acquires a stable sibling sidecar lock file, rereads the
current metadata while locked, applies one mutation, and publishes through the
existing atomic replacement path. The replaced metadata file itself is never
the lock target: Unix inode replacement would split lock ownership, while
Windows file locking could prevent replacement.

Every mutator, including summary, activity, title, and rename, routes through
this operation. The same sidecar lock covers the complete fork transaction:
ID allocation, directory copy/publication, metadata commit, and failure
rollback. Caller-owned mutexes and unlocked mutation paths are deleted. Lock,
read, parse, or write failure returns the existing typed session error and
leaves the previous metadata intact.

### 6.2 Supervised worker terminalization

Workflow execution is terminalized only by `WorkflowRuntime`. Delegate and
swarm execution is terminalized only by `MultiAgentRuntime`;
`BackgroundTaskManager` remains a projection/adapter, not a state owner.

Each runtime supervises its worker `JoinHandle`. A panic becomes `Failed` with
the stable reason `worker_panicked`; cancellation and normal errors remain
distinct. A panic does not retry or repeat an external effect. For a swarm,
every child that cannot continue is terminalized consistently so no child is
left `Running` without a worker. Existing journal failure handling remains
authoritative if recording the terminal state itself fails.

If a workflow panic leaves a durable `InvocationStarted` without a terminal
outcome, `WorkflowRuntime` first appends an interrupted invocation outcome with
reason `worker_panicked`, then clears `current_invocation`, then terminalizes
the run. Failure to persist that outcome follows the existing recovery-failure
path; it must not guess whether the effect ran or retry it.

### 6.3 Per-run recovery isolation

`WorkflowRuntime::rehydrate_run_entry` contains all errors attributable to one
run directory: metadata, journal parse/open, resolver, and recovery-record
failures produce an inspectable failed handle for that run and allow sibling
runs to continue.

Only failure to enumerate the workflows root or an internal runtime registry
invariant may fail session-wide rehydration. Do not create a fallback workflow,
retry effects, discard the bad run, or hide its failure.

### 6.4 Atomic delegate message delivery

Generation validation and enqueue happen in one operation while the live-steer
registry entry remains authoritative. That operation returns a typed outcome:
`Delivered`, `NotRunning`, or `Unknown`. Tool code does not perform a separate
snapshot precheck, and swarm broadcast reuses the same primitive.

The registry must never report `Delivered` unless the message was accepted by
the live receiver for the validated generation.

### 6.5 Typed MCP authentication

The HTTP/OAuth transport boundary maps HTTP authentication status and token
refresh/auth-required results to `McpErrorKind::NeedsAuth`. The manager switches
state and suppresses reconnect based only on that kind.

Delete matching of `"401"`, `"Unauthorized"`, or diagnostic text. Error text
remains presentation-only; a protocol or network error containing those
characters cannot become `NeedsAuth`.

### 6.6 Separate task and delegate discovery

`ListDelegates` is the sole delegate/swarm discovery API. Delete delegate and
swarm synthesis, deduplication, and corresponding positive synthesis tests from
`TaskList`.

`BackgroundTaskManager` provides a metadata-only enumeration path. `TaskList`
uses it to list only bash, question, and workflow task metadata, filtering any
manager records for delegate/swarm tasks before sorting and limiting. It does
not read any `<task>.log`. `TaskOutput` remains the sole API that hydrates task
output by ID. `BackgroundTaskManager` may still support notifications and
internal `TaskOutput`/`TaskStop` adaptation, but it is not a delegate state
owner.

## 7. Historical Plan Replay

`ToolExecutionFinished.result.details.{plan_content, plan_path}` is the only
durable plan replay truth. Core already persists this result in session JSONL;
the replay projection reads it without changing event or result schemas.

Delete `ReplayPlanSnapshot`, Write/Edit argument inference, batch-first-edit
special cases, and all reads of the current plan file during replay. A current
workspace file is mutable state and cannot prove historical content.

An old event without persisted snapshot details renders the existing
`ExitPlanMode` card/header with no plan body. No migration, placeholder body,
warning body, or compatibility fallback is added.

## 8. Notification And Terminal Entry

### 8.1 Notification failures

Notification error diagnostics use `tracing::warn!` and therefore the existing
log-capture path; they never write directly to process stderr and do not emit
an `AgentEvent::Error`. The Bell backend intentionally writes the BEL control
character to stderr and remains unchanged; that is notification output, not an
error diagnostic.

`neo-tui` may add the existing workspace `tracing` dependency as a direct
dependency for this call site. This adds no new third-party package or logging
owner; it connects the notification module to Neo's existing tracing capture.

Only permanent spawn/unsupported failures sticky-disable a notification
backend. A child that started but later exited unsuccessfully, or a transient
wait/thread failure, clears `in_flight` and permits the next notification to
retry. No notification failure may corrupt the active TUI frame.

### 8.2 Windows VT input contract

On Windows, failure to query the console input mode aborts TUI entry/resume with
an error. It must not return an inactive guard and continue with unknown input
semantics. Existing guard rollback restores any mode already changed.

Unix behavior is unchanged. A minimal private console-mode operation function
or equivalent test seam may inject the query failure without adding a public
trait or subsystem. Windows must run that deterministic failure test plus the
real enable/restore test; a macOS cross-compile alone is not runtime evidence.

## 9. Clipboard And Completion Portability

### 9.1 Controller-owned asynchronous clipboard task

Clipboard helpers use `tokio::process::Command` with `kill_on_drop(true)`.
One private fixed short deadline covers stdin write and child exit; it is not a
user configuration option. The interactive controller owns at most one
clipboard task. Starting a new copy cancels the prior task.

The internal copy buffer updates immediately. Helper success/failure updates
status only when the task completes, so a blocked helper cannot block input,
rendering, or shutdown. Timeout/cancel drops and kills the helper. Every
interactive copy entry point, including prompt copy, transcript copy, and
cross-workspace resume command copy, routes through this controller-owned task.
Delete the synchronous `ClipboardWriter` owner and all direct callers; a
private command specification or function hook may support deterministic tests,
but no public trait or second execution path is added.

Platform command selection remains macOS `pbcopy`, Windows `clip.exe`, and
Linux `wl-copy` then `xclip`. Do not reuse or modify ShellRuntime, Bash, or
Terminal timeout behavior; those commands retain their unbounded contract.

### 9.2 Native separator recognition

Path completion uses `std::path::is_separator` for user-input segmentation.
Windows accepts both `/` and `\\`; Unix treats only `/` as a separator so a
backslash remains a valid filename character.

Insertion preserves the separator style already used by the user, preventing
mixed paths such as `src\\dir/`. An internally normalized display path may
continue to use `/`; parsing user text and rendering a canonical label are
separate concerns.

## 10. Deletion And Simplification

### 10.1 Inline-image side channel

Delete `InlineImageRenderCache` and the unused `inline_image_renders` /
`inline_image_sequences` side channel, including exports, methods, and tests
that only prove the dead cache. Preserve the production inline-image rendering
path and its protocol/capability behavior.

### 10.2 Exact cleanup map

Cleanup is limited to the following proven duplicates. It may proceed only
after a zero-reference/call-path check confirms the named owner still covers
all callers.

| Retired path | Canonical replacement | Test policy |
|---|---|---|
| `neo-tui::shell::dialog_dispatch` input forwarding traits, impls, and `handle_input_ref` / `handle_input_owned` | Keep the existing `OverlayKind` match and call each concrete state's inherent `handle_input` directly | Keep existing overlay/dialog behavior tests |
| Trust dialog's private `dialog_sgr*`, `named_dialog_sgr`, and `DialogSgrLayer` copies | Reuse `dialogs::choice_picker::dialog_sgr_fg` / `dialog_sgr_bg` | Keep trust rendering and choice-picker color tests |
| `mode::plan_mode_guard::normalize` and `tools::normalize_path` | Make `workspace_policy::normalize_path` crate-private and call it; keep `paths_match` as the policy-facing wrapper | Keep all three modules' path/authorization tests |
| `WorkflowDispatchResolver::replace` | Use `WorkflowDispatchResolver::refresh`; update callers and the test name that advertises `replace` | Keep behavior tests; do not add an alias |
| Four XML escaping implementations in messages, skill context, skills, and instruction resolver | One crate-private `xml_escape::{escape_text, escape_attribute}` owner preserving current byte semantics | Keep existing skill/shell/instruction consumer coverage; no helper-only duplicate suite |
| `ansi_escape` tests `rgb_foreground` / `named_colors` and `todo_panel::selector_types_are_exported_from_widgets_surface` | Existing stronger ANSI behavior tests and compile-time export checking | Delete only these three low-value tests |
| Forwarders `box_draw::side_bordered_line`, `tool_renderers::format_tool_token_count`, `templates::load_user_prompt_templates`, and `models_cli::configured_model_is_default` | Direct calls to `content_line`, `token_estimate::format_token_count`, `load_prompt_templates_from_tree`, and `runtime::model_config_matches_default` | Keep existing caller coverage |

Delete the dead inline-image cache described in Section 10.1 in the same
deletion-focused phase. Do not delete `WorkflowSnapshot` or
`WorkflowStepRecord`; both remain in active runtime and presentation paths
despite legacy-looking names. No new dependency, generic platform layer,
interface, factory, or configuration knob is justified. The estimated net
reduction of roughly 244-251 lines is supporting evidence, not an acceptance
target.

## 11. Cross-Platform Contract

| Area | Windows | Linux | macOS |
|---|---|---|---|
| Environment | Skip non-Unicode `OsString` pairs without panic | Same lossless rule | Same lossless rule |
| Session metadata lock | Cross-process lock protects one RMW owner | Same | Same |
| Notification | Windows toast backend errors use tracing; transient child failures retry later | `notify-send` errors use tracing | `osascript` errors use tracing |
| Clipboard | `clip.exe`, async, bounded helper lifetime | `wl-copy` then `xclip`, async, bounded helper lifetime | `pbcopy`, async, bounded helper lifetime |
| Completion | `/` and `\\` are separators; preserve input style | Only `/` is a separator | Only `/` is a separator |
| VT input | Mode-query failure aborts TUI entry | Existing terminal entry unchanged | Existing terminal entry unchanged |
| Paths | No lossy conversion added | Backslash may be a filename character | Backslash may be a filename character |

Platform-specific code remains isolated behind existing `cfg` boundaries. No
unsupported platform branch may `panic!`, `todo!`, or silently disable required
input behavior. Native VM/machine verification is required only for behavior
that cannot be proven on the macOS host; any Parallels VM booted for that proof
must be shut down after use.

## 12. Implementation And Verification Policy

Implementation must use subagent-driven development with at least three
independent slices. A root agent owns integration, contract review, final stale-
owner scans, exact verification, and commits. Subagents must receive the same
scope, dirty-worktree, `.references/`, Git mutation, card UI, and ShellRuntime
constraints as the root agent.

TDD route: `off/skipped`. Each non-trivial repair adds the smallest regression
that fails on the old behavior and proves the canonical owner. Pure deletion
such as duplicate catalog spawns needs no synthetic test when diff and stale-
reference scans prove it.

Every test command names exactly one package, one target selector, and at least
one precise test filter. Work proceeds in logical slices; each verified slice
is one conventional commit. Unrelated failures are reported, not repaired.
No `.references/` edit, broad `cargo test`, fallback path, or worktree cleanup
is authorized.

## 13. Acceptance Criteria

1. A non-UTF-8 unrelated environment entry cannot panic provider resolution;
   valid target credentials still resolve.
2. Built-in Anthropic exposes only `ANTHROPIC_API_KEY`; Neo production and
   provider-test paths contain no `ANTHROPIC_OAUTH_TOKEN` credential source,
   heuristic, branch, or positive support test. Documentation of this retired
   decision and comparison-only `.references/` are excluded from the stale
   scan.
3. Anthropic tool streams cannot emit args/end without a valid start and use
   only `StreamingToolCallAssembler` for lifecycle assembly.
4. Catalog 401/429/5xx responses use the shared typed classifier, including
   `Retry-After`; chunked success bodies exceeding 16 MiB fail as protocol
   errors without unbounded allocation.
5. Each interactive catalog action owns exactly one stored fetch task.
6. Concurrent session metadata mutations preserve both updates, and a failed
   mutation preserves the previous file.
7. Workflow, delegate, and swarm worker panics reach inspectable `Failed`
   states with `worker_panicked`; no effect is retried and no child remains
   spuriously `Running`.
8. One unrecoverable workflow run becomes a failed handle while a valid sibling
   remains rehydrated and usable.
9. Delegate message delivery returns `Delivered` only after atomic generation
   validation and enqueue; unregister races return a non-delivered outcome.
10. Typed HTTP authentication errors become `NeedsAuth`; ordinary errors whose
    text contains `401` do not.
11. `TaskList` excludes delegate/swarm entries, reads no output logs, and lists
    only bash/question/workflow metadata. `ListDelegates` remains canonical.
12. Plan replay uses persisted result details after the live plan file changes;
    events without details render no plan body and perform no disk read.
13. Notification errors never write directly to stderr. Transient child failure
    does not disable later notifications; permanent unsupported/spawn failure
    does.
14. Windows console input-mode query failure aborts TUI entry and rolls back
    any already-applied mode changes.
15. A blocked clipboard helper cannot block subsequent interactive input;
    timeout preserves the internal copy buffer, reports failure, and terminates
    the helper.
16. Windows completion accepts both separators and preserves style; Unix keeps
    backslash as a filename character.
17. `InlineImageRenderCache` and all dead side-channel owners/tests are absent,
    while the main inline-image render path remains covered.
18. Focused stale-reference scans prove all retired owners and text heuristics
    are absent. `WorkflowSnapshot` and `WorkflowStepRecord` remain intact.
19. Delegate-family card presentation and Bash/Terminal admission/unbounded
    execution contracts are unchanged.
20. Native Windows/Linux/macOS evidence is collected where behavior differs;
    any Parallels VM used for verification is stopped afterward.

## 14. Compatibility And Retirement

Anti-Entropy Declaration:

- Deletion class: internal code retirement and durable-lifecycle repair.
- Retired paths: pseudo Anthropic OAuth declaration, Anthropic-local tool
  assembly, duplicate catalog spawns, unlocked metadata RMW, unsupervised
  worker terminalization, manager-level MCP text classification, TaskList
  delegate synthesis/log reads, live-disk plan replay, notification stderr,
  synchronous clipboard helpers, slash-only Windows splitting, dead inline-
  image cache, and proven duplicate cleanup helpers.
- Canonical owners: enumerated in Section 3.1.
- Durable data impact: no event schema change, migration, rewrite, or deletion.
  Old events without plan snapshots lose only an unrecoverable fabricated body.
- External behavior intentionally removed: the invalid built-in
  `ANTHROPIC_OAUTH_TOKEN` claim and delegate discovery through `TaskList`.
- User confirmation: explicitly granted for both removals and legacy plan-body
  omission.

Retirement decision: `delete-first`. No internal compatibility exception is
authorized. Real OAuth, if ever required, is a separate product design rather
than a resurrection of this path.

## 15. Self-Review

- Placeholder scan: no TODO, unresolved choice, or unspecified owner.
- Decision readback: all three user decisions appear as normative contracts.
- Coverage: every included finding maps to an owner and acceptance criterion.
- Minimality: existing assemblers, HTTP classifiers, persistence helpers,
  tracing capture, and platform commands are reused; only the existing
  workspace `tracing` package may become a direct `neo-tui` dependency.
- Cross-platform: Windows input/path behavior is explicit; Linux/macOS
  separator behavior remains native; clipboard helper bounds do not leak into
  ShellRuntime.
- Historical integrity: replay never reads mutable live files, and durable
  JSONL is not migrated.
- UI integrity: Delegate-family cards are out of scope and unchanged.
- Destructive boundary: only internal code/tests are retired; user data and
  `.references/` are untouched.

## 16. Approval Record

The user approved this design on 2026-07-25 after separately confirming the
Anthropic OAuth retirement, `ListDelegates` ownership, and legacy plan replay
behavior. The implementation plan must map every acceptance criterion to exact
files, test names, stale-owner scans, subagent slices, commit boundaries, and
platform verification.
