# Local Workflow Platform Contract Baseline

Status: `recorded-from-adr`
Date: `2026-07-26`
ADR: `docs/aegis/adr/ADR-0006-local-workflow-platform.md`
Supersedes (historical): `docs/aegis/baseline/2026-07-23-runworkflow-runtime-contract.md` (substrate-only; kept as historical record)

This baseline records **only landed behavior** verified at platform closeout
(Tasks 1–25). It is not a proposal document.

## Product / Requirement Baseline

- Local-only workflow platform: no marketplace, profile sync, or hosted
  collaboration surface.
- Lua is the only workflow script engine. Dual-engine / Rhai / engine trait /
  factory designs are non-goals and are not present.
- Named `/workflow <name>` launch is host-direct and performs **zero model
  calls** before workflow execution. Launch adapters share one coordinator
  path.
- Definitions are paired `.lua` + `.workflow.toml` with exact SHA-256 revision
  framing. Precedence is `builtin < user < trusted project`. Same-scope name
  conflicts invalidate the name; invalid higher-scope content never silently
  falls back.
- Project discovery and project save reuse Neo workspace trust; untrusted
  project definitions are absent and cannot be saved.
- Final workflow result and every child require `output_schema`. Exactly one
  tools-disabled schema repair is allowed.
- `neo.tool` uses the canonical `ToolRegistry` with one deny classifier.
- Heterogeneous `neo.swarm` is supported. There is no hard-coded
  `MAX_SWARM_CHILDREN` or arbitrary total child cap; scale is bounded by host
  physical limits and backpressure.
- `AwaitingUser` is durable and independent of the worker loop.
- V1 runs are **read-only linked upgrade only** — no in-place migration or
  deletion of durable V1 state by this platform.
- Global admission uses **actual occupancy only**. Predictive `token_cap`,
  projected usage, and model-supplied machine limits are rejected.
- `/tasks` is extended for workflow dashboard filtering/pagination; it is not
  a second task system.
- Shell/Terminal admission remains pending while queued and unbounded without
  explicit timeout/cancel (owned by ShellRuntime; unchanged).

## Architecture / Runtime Boundary Baseline

- `WorkflowRuntime` is the **sole durable owner** of lifecycle, control,
  durable invocation identity, replay, recovery, and aggregate output.
- The definition registry owns **only** trusted definitions (cache is a
  rebuildable projection; never a durability or authorization source).
- The launch coordinator is **stateless**; durable creation and task
  registration consume the one-shot launch capability exactly once after
  durable materials exist.
- Immutable launch metadata lives under
  `<session_dir>/workflows/<run_id>/run.json`.
- Append-only `journal.jsonl` (V2 versioned envelopes) is the sole durable
  truth for current state, control transitions, invocation intent/outcome,
  child references, actual provider usage, artifacts membership, and recovery
  records.
- Torn-tail recovery: invalid non-newline EOF suffix is written to a
  content-hash-named file under `recovery-quarantine/` and synced before
  truncate. Quarantine failure leaves the journal byte-for-byte intact.
  Interior / newline-terminated corruption fails closed with no mutation.
- Incomplete external effects after host exit are reconciled as
  `interrupted(host_exit)` and are **never** auto-retried.
- Run-scoped artifacts are content-addressed under `artifacts/`; journal
  `ArtifactCommitted` owns membership; reads revalidate size/digest and reject
  symlink/reparse or non-regular files as typed corrupt/missing errors.
- Session JSONL and TUI workflow cards remain projections. Durable journal
  sequence is the ordering watermark.
- Delegate, DelegateGroup, DelegateSwarm, Bash, and Terminal tools and card
  designs are unchanged by this platform.

## Configuration And Compatibility

- Host limits live under `[runtime.workflow]` and resolve into validated
  `WorkflowLimits`. Scripts, definitions, and model tool inputs cannot set or
  raise them. Rejected predictive keys include `token_cap`, model
  `max_concurrency` as a workflow limit, and `projected_usage`.
- Persistence uses `Path` / `PathBuf`. Parent-directory sync is platform-gated
  with a portable non-Unix path. No bare `sh -c`, Unix-signal, or
  executable-bit assumptions in portable workflow paths.
- Definition files and artifact paths reject symlink/reparse escapes and parent
  escapes; only regular files with exact expected suffixes are accepted.

## Retirement Boundary

- Retired without fallback as active writers: `WorkflowHostRecorder`,
  `run_script`, `host_api`, `child_tools.run`, `mode=background` launch,
  model-owned concurrency/resource limits, dual engines / engine abstractions,
  hard-coded `MAX_SWARM_CHILDREN`, and predictive `token_cap` governance.
- Documentation may mention retired names only as rejected keys, non-goals, or
  historical notes.
- Existing session/workflow artifacts are not deleted or in-place migrated by
  this closeout.

## Cross-Platform Evidence (Task 25)

Base commit SHA: `b088a6dbf7ae9aaec35319654ab858c31dac5d8e` (docs/workflow close
commit) plus Task 25 platform test sources under:

- `crates/neo-agent-core/tests/workflow_registry.rs`
  (`registry_platform_path_and_link_semantics`)
- `crates/neo-agent-core/tests/workflow_journal_v2.rs`
  (`journal_platform_sync_and_quarantine_semantics`)
- `crates/neo-agent-core/tests/workflow_artifacts.rs`
  (`artifact_replace_and_integrity_are_platform_safe`)

| OS | Host | Result |
| --- | --- | --- |
| macOS (host) | Darwin aarch64, workspace `/Users/chenyuanhao/Workspace/neo` | All three nextest filters **passed** |
| Fedora Linux | Parallels VM `Fedora Linux`, kernel `7.0.8-100.fc43.aarch64`, guest tree `/root/neo-task25` | All three nextest filters **passed** |
| Windows 11 | Parallels VM `Windows 11`, `Microsoft Windows NT 10.0.26200.0` ARM64, guest tree `C:\Users\chenyuanhao\neo-task25` | All three nextest filters **passed** |

Exact commands (each OS):

```bash
cargo nextest run -p neo-agent-core --test workflow_registry registry_platform_path_and_link_semantics
cargo nextest run -p neo-agent-core --test workflow_journal_v2 journal_platform_sync_and_quarantine_semantics
cargo nextest run -p neo-agent-core --test workflow_artifacts artifact_replace_and_integrity_are_platform_safe
```

Both Parallels VMs were stopped after native runs (`prlctl list` showed both
`stopped`). Local deterministic proof is not a substitute for this native
matrix; CI remote proof remains separate.

## Verification Boundary

- Focused package/target/filter nextest evidence is the accepted contract proof
  for each task; workspace-wide `cargo test` is not required evidence.
- Stale-owner scan (Task 25) expects zero active old-owner writers for
  `WorkflowHostRecorder`, `run_script`, `host_api`, `child_tools.run`,
  `MAX_SWARM_CHILDREN`, model `token_cap` ownership, `read_journal(&guard.journal_path`,
  `mode=background`, `engine abstraction`, and `Rhai` as live code paths.
  Hits that only document rejection or historical non-goals are allowed.
- Provider-backed live multi-agent runs and visual TUI interaction remain
  residual surfaces outside this baseline’s deterministic closeout.

## Residual Risk

- Host crash can leave an external effect incomplete; recovery records
  interruption rather than guessing retry safety.
- Windows file-symlink creation may require Developer Mode / elevation; when
  unavailable, native proof still covers regular-file path containment, atomic
  write, integrity, and quarantine — and rejects symlinks when the host can
  create them.
- Append-only journal and definition revision frames are durable formats;
  future changes need explicit versioning.
- Unrelated dirty worktree files (for example `.gitignore` and concurrent TUI
  edits) must not be staged into the platform closeout commit.
