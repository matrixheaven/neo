# Shell Admission Scheduler Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fail-fast shell capacity rejection with transparent, fair admission; remove default execution deadlines; expose truthful queue state in every transcript; and add a shell-free `Sleep` tool.

**Architecture:** `ShellRuntime` owns one in-process `Arc<ShellScheduler>`. The scheduler uses priority classes, per-owner FIFO queues, owner round-robin rings, one-shot grants, and an RAII permit retained by `GuardianClient`; runtime callbacks project queue/start transitions into events without changing model-visible Tool Results. Guardian supervision remains the execution boundary, with optional per-command deadlines and direct per-command resource budgets.

**Tech Stack:** Rust 2024, Tokio, `tokio_util::sync::CancellationToken`, Serde/Schemars, Neo `AgentEvent` JSONL, Ratatui transcript components, existing guardian IPC.

## Global Constraints

- Rust edition 2024; minimum Rust 1.96.1; no new crate dependency.
- The scheduler is process-local and in-memory; queued work is never replayed.
- Default total capacity is `8`; at most `3` permits may be admitted as `AgentBackground`.
- Priority is `User`, then `AgentForeground`, then eligible `AgentBackground`; no preemption.
- Agent queues are FIFO per owner and round-robin between owners within each class.
- Queueing has no item limit, TTL, admission timeout, ETA, task handle, or automatic background conversion.
- Permission completes before enqueue; hard launch boundaries are checked again after grant and before spawn.
- Omitted `timeout_secs` means no execution deadline; explicit values must be positive and queue time does not count.
- The schema recommendation for potentially long-running work is language-neutral and uses `7200` seconds.
- Old timeout/config names and `SetBackgroundDeadline` are deleted without aliases or fallback paths.
- Resource budgets are direct per command: parallelism `4`, descendants `32`, memory `25%`; capacity never divides them.
- Real process/memory violations and unavailable sampling remain hard guardian failures.
- Queue metadata is lifecycle/UI data only and never enters `ToolResult` or model context.
- `Sleep` accepts `duration_seconds: 1..=3600` and a non-empty, single-line reason of at most 160 characters; it never touches shell admission.
- Windows, Linux, and macOS behavior must remain equivalent; platform-specific process code stays behind existing `cfg` boundaries.
- Preserve unrelated dirty work. A commit step is valid only in an isolated/clean execution worktree or when every staged hunk is known to belong to this plan; otherwise skip the Git mutation and report it.
- Every test command names one package, one target selector, and one exact test filter. Do not widen to workspace-wide tests.
- Execute every shell command below through the repository-required `rtk` prefix; the code blocks show the underlying command for readability.

---

## File Map

**Create**

- `crates/neo-agent-core/src/tools/shell_guard/scheduler.rs`: admission queues, fairness, cancellation-safe grants, and RAII permits.
- `crates/neo-agent-core/src/tools/sleep.rs`: validation and cancellable time-based waiting.

**Modify: shell/config/runtime**

- `crates/neo-agent-core/src/tools/shell_guard/mod.rs`: canonical limits, scheduler ownership, exports, and removal of the atomic counter/static division.
- `crates/neo-agent-core/src/tools/shell_guard/client.rs`: accept an acquired permit and carry an optional timeout into Start.
- `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`: optional deadline encoding and deletion of `SetBackgroundDeadline`.
- `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`: optional Bash deadline and renamed direct resource limits.
- `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs`: optional Terminal deadline and renamed direct resource limits.
- `crates/neo-agent-core/src/tools/bash.rs`: `timeout_secs`, admission classes, post-grant validation, and actionable resource messages.
- `crates/neo-agent-core/src/tools/terminal.rs`: Start-only `timeout_secs`, background admission, and post-grant validation.
- `crates/neo-agent-core/src/tools/background_tasks.rs`: detach without deadline mutation and canonical resource messages.
- `crates/neo-agent-core/src/tools/mod.rs`: ToolContext admission callback, removal of default Bash timeout, Sleep registration/exports.
- `crates/neo-agent-core/src/runtime/tool_dispatch.rs`: permission-before-start ordering and admission-controlled start callbacks.
- `crates/neo-agent-core/src/runtime/events.rs`: queue/start event callback construction.
- `crates/neo-agent-core/src/runtime/permission.rs`: default approval for Sleep.
- `crates/neo-agent-core/src/events.rs`: durable queue transitions plus live-only queue updates.
- `crates/neo-agent-core/src/session/event_persistence.rs`: discard live queue updates and strip live queue metadata from every persisted Delegate/Swarm snapshot or progress event.
- `crates/neo-agent/src/config/types.rs`: canonical strict `[runtime.shell]` fields.
- `crates/neo-agent/src/config/loader.rs`: canonical field loading.
- `crates/neo-agent/src/config/mod.rs`: defaults and strict old-key rejection tests.
- `crates/neo-agent/src/modes/interactive/shell_command.rs`: user-shell admission and queued events.
- `crates/neo-agent/src/modes/interactive/mod.rs`: connect user-shell admission callbacks to its event channel.
- `crates/neo-agent/src/modes/interactive/tests.rs`: user-shell queue and config reload behavior.

**Modify: multi-agent/TUI**

- `crates/neo-agent-core/src/multi_agent/state.rs`: canonical queued child-tool phase.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`: fold queue events into the existing activity entry by tool-call ID.
- `crates/neo-agent-core/src/multi_agent/profile.rs`: make Sleep available to every role.
- `crates/neo-agent/src/modes/task_browser.rs`: format the new queued phase exhaustively.
- `crates/neo-tui/src/shell/stream.rs`: explicit `ToolStatusKind::Queued`.
- `crates/neo-tui/src/transcript/tool_call.rs`: live queue display state and in-place transitions.
- `crates/neo-tui/src/transcript/tool_renderers.rs`: distinguish Pending from Queued.
- `crates/neo-tui/src/transcript/shell_run.rs`: queued user-shell state.
- `crates/neo-tui/src/transcript/event_handler.rs`: queue event application.
- `crates/neo-tui/src/transcript/pane.rs`: queued tool upsert/replay liveness.
- `crates/neo-tui/src/transcript/presentation.rs`: treat queued cards as live presentation entries.
- `crates/neo-tui/src/transcript/store.rs`: queued entry lookup/finalization.
- `crates/neo-tui/src/transcript/tool_group.rs`: keep queued calls out of finalized grouping.
- `crates/neo-tui/src/transcript/child_activity.rs`: Delegate/Swarm queued row rendering.

**Modify: focused tests/docs**

- `crates/neo-agent/tests/shell_admission_runtime.rs`
- `crates/neo-agent-core/tests/runtime_turn.rs`
- `crates/neo-agent-core/tests/session_jsonl.rs`
- `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- `crates/neo-agent/tests/process_guard.rs`
- `crates/neo-agent/tests/process_guard_windows.rs`
- `crates/neo-agent/tests/tool_bash_guardian.rs`
- `crates/neo-agent/tests/tool_terminal_guardian.rs`
- `crates/neo-tui/tests/shell_events.rs`
- `crates/neo-tui/tests/tool_cards.rs`
- `crates/neo-tui/tests/multi_agent_transcript.rs`
- `docs/en/configuration/config-files.md`
- `docs/zh/configuration/config-files.md`
- `docs/en/configuration/permissions.md`
- `docs/zh/configuration/permissions.md`
- `docs/en/customization/agents.md`
- `docs/zh/customization/agents.md`
- `docs/en/reference/tools.md`
- `docs/zh/reference/tools.md`

---

### Task 1: Make Shell Limits Canonical and Per-Command

**Files:**

- Modify: `crates/neo-agent-core/src/tools/shell_guard/mod.rs:112-249`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/guardian.rs:560-590, 1028-1061`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs:475-505`
- Modify: `crates/neo-agent-core/src/tools/bash.rs:199-249`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs:1510-1630`
- Modify: `crates/neo-agent/src/config/types.rs:219-237`
- Modify: `crates/neo-agent/src/config/loader.rs:251-282`
- Modify: `crates/neo-agent/src/config/mod.rs:325-405`
- Modify: `crates/neo-agent/tests/process_guard.rs`
- Modify: `crates/neo-agent/tests/process_guard_windows.rs`

**Interfaces:**

- Produces the final `ShellLimits` and `GuardLimits` resource field names consumed by every later task.
- Leaves the temporary `ActiveCommands` admission cause in place only until Task 3 replaces fail-fast acquisition; Task 3 deletes it with its last caller.

- [ ] **Step 1: Write strict config and direct-budget RED tests**

Add these focused cases to the existing config/core test modules:

```rust
#[test]
fn runtime_shell_uses_canonical_per_command_limits() {
    let (_temp, config_path, project_dir) = temp_project_config(
        "[runtime.shell]\n\
         max_active_commands = 6\n\
         max_command_parallelism = 8\n\
         max_command_descendant_processes = 40\n\
         max_command_memory_percent = 30\n",
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 6);
    assert_eq!(config.runtime.shell.max_command_parallelism, 8);
    assert_eq!(config.runtime.shell.max_command_descendant_processes, 40);
    assert_eq!(config.runtime.shell.max_command_memory_percent, 30);
}

#[test]
fn runtime_shell_rejects_removed_limit_names() {
    for key in [
        "max_parallelism",
        "max_descendant_processes",
        "max_tree_memory_percent",
    ] {
        let input = format!("[runtime.shell]\n{key} = 1\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("removed key was accepted");
        let message = error.to_string();
        assert!(message.contains(key), "{error:#}");
        assert!(message.contains("max_active_commands"), "{error:#}");
        assert!(message.contains("max_command_parallelism"), "{error:#}");
    }
}

#[test]
fn limits_are_direct_per_command_budgets() {
    let limits = ShellLimits {
        max_active_commands: 8,
        max_command_descendant_processes: 32,
        max_command_memory_percent: 25,
        ..ShellLimits::default()
    };
    let runtime = ShellRuntime::for_tests(limits);
    let guard = runtime.guard_limits(Duration::from_secs(60), limits.max_output_bytes);
    assert_eq!(guard.max_command_descendant_processes, 32);
    assert_eq!(guard.max_command_memory_percent, 25);
}

#[test]
fn config_allows_capacity_larger_than_per_command_memory_percent() {
    let (_temp, config_path, project_dir) = temp_project_config(
        "[runtime.shell]\nmax_active_commands = 51\n",
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.runtime.shell.max_active_commands, 51);
    assert_eq!(config.runtime.shell.max_command_memory_percent, 25);
}

#[test]
fn resource_limit_messages_name_observation_and_next_step() {
    let process = ResourceLimitDetail {
        cause: ResourceLimitCause::ProcessCount,
        configured: Some(32),
        observed: Some(41),
    };
    let memory = ResourceLimitDetail {
        cause: ResourceLimitCause::TreeMemory,
        configured: Some(25),
        observed: Some(31),
    };
    let sampler = ResourceLimitDetail {
        cause: ResourceLimitCause::SamplerUnavailable,
        configured: None,
        observed: None,
    };
    assert!(format_resource_limit(Some(&process)).contains("41 > 32"));
    assert!(format_resource_limit(Some(&memory)).contains("31% > 25%"));
    let sampler = format_resource_limit(Some(&sampler));
    assert!(sampler.contains("monitoring unavailable"));
    assert!(sampler.contains("retry"));
}
```

Use the existing `temp_project_config` and `load_config` helpers shown above rather than adding a second TOML loader.
Update the existing `config_defaults_shell_limits` assertions to `4`, `4`, `32`, `25`, `65_536`, and `10_485_760` for the six canonical fields. Replace `config_rejects_shell_limits_that_round_static_memory_to_zero` with the capacity-independence test above.

- [ ] **Step 2: Run the config RED tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::tests::runtime_shell_uses_canonical_per_command_limits --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::tests::runtime_shell_rejects_removed_limit_names --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::tests::limits_are_direct_per_command_budgets --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::tests::config_defaults_shell_limits --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::tests::config_allows_capacity_larger_than_per_command_memory_percent --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::tests::resource_limit_messages_name_observation_and_next_step --exact --nocapture --include-ignored
```

Expected: all six fail because the canonical fields, defaults, strict rejection, decoupled validation, and actionable formatter do not exist.

- [ ] **Step 3: Replace the limit types without aliases**

Make these the complete resource fields:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardLimits {
    // Task 2 makes these deadline fields optional/canonical.
    pub timeout_ms: u64,
    pub background_timeout_ms: u64,
    pub max_command_parallelism: usize,
    pub max_command_descendant_processes: usize,
    pub max_command_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellLimits {
    // Removed with the deadline migration in Task 2.
    pub foreground_timeout_secs: u64,
    pub background_timeout_secs: u64,
    pub max_active_commands: usize,
    pub max_command_parallelism: usize,
    pub max_command_descendant_processes: usize,
    pub max_command_memory_percent: u8,
    pub max_output_bytes: usize,
    pub max_background_log_bytes: u64,
}

impl Default for ShellLimits {
    fn default() -> Self {
        Self {
            foreground_timeout_secs: 600,
            background_timeout_secs: 1_800,
            max_active_commands: 8,
            max_command_parallelism: 4,
            max_command_descendant_processes: 32,
            max_command_memory_percent: 25,
            max_output_bytes: 65_536,
            max_background_log_bytes: 10_485_760,
        }
    }
}
```

Validation requires every integer to be positive, memory to be in `1..=100`, and output to fit `u32`. Keep the timeout fields/validation until Task 2, but delete both `per_command_*` division helpers. Rename every guardian field and environment use; preserve an explicitly supplied environment variable and use `max_command_parallelism` only for an absent one.

Rename the real guardian causes' configured fields now. Keep `ActiveCommands` compiling until Task 3 removes `try_acquire`; do not use it from new code and do not add another capacity cause.

- [ ] **Step 4: Make file config strict and canonical**

Use exactly these fields and make unknown fields fail deserialization:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileRuntimeShellConfig {
    // Removed with the deadline migration in Task 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) foreground_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) background_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_active_commands: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_command_parallelism: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_command_descendant_processes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_command_memory_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_background_log_bytes: Option<u64>,
}
```

Update the resource fields in `runtime_shell_from_file` one-to-one and leave its two timeout assignments for Task 2. Rename JSON fixtures in process-guard tests. Do not accept the three removed resource names through `serde(alias)` or a manual migration map.

- [ ] **Step 5: Make real resource failures actionable**

Add one shared formatter in `tools/mod.rs` or `shell_guard/mod.rs` and use it in Bash, Terminal, TaskOutput/background snapshots, and TUI detail projection:

```rust
pub fn format_resource_limit(detail: Option<&ResourceLimitDetail>) -> String {
    match detail {
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::ProcessCount,
            configured: Some(limit),
            observed: Some(observed),
        }) => format!(
            "Resource limit exceeded: command descendants {observed} > {limit}. \
             Reduce per-command parallelism or raise \
             runtime.shell.max_command_descendant_processes."
        ),
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::TreeMemory,
            configured: Some(limit),
            observed: Some(observed),
        }) => format!(
            "Resource limit exceeded: command tree memory {observed}% > {limit}%. \
             Reduce the workload or raise runtime.shell.max_command_memory_percent."
        ),
        Some(ResourceLimitDetail {
            cause: ResourceLimitCause::SamplerUnavailable,
            ..
        }) => "Resource monitoring unavailable; the command was stopped because Neo \
               could not enforce shell limits. Check platform process monitoring and retry."
            .to_owned(),
        _ => "Resource limit exceeded.".to_owned(),
    }
}
```

Keep structured configured/observed details unchanged. Do not suggest Sleep for a resource violation.

- [ ] **Step 6: Run GREEN and commit Task 1**

Run the six exact commands from Step 2. Expected: PASS.

When the execution worktree is safe to commit:

```bash
git add crates/neo-agent-core/src/tools/shell_guard/mod.rs crates/neo-agent-core/src/tools/shell_guard/guardian.rs crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent/src/config/types.rs crates/neo-agent/src/config/loader.rs crates/neo-agent/src/config/mod.rs crates/neo-agent/tests/process_guard.rs crates/neo-agent/tests/process_guard_windows.rs
git commit -m "refactor(shell): decouple command limits from capacity"
```

---

### Task 2: Remove Default Deadlines and Canonicalize `timeout_secs`

**Files:**

- Modify: `crates/neo-agent-core/src/tools/shell_guard/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/protocol.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/client.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/guardian.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent/src/config/types.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/shell_command.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Modify: `crates/neo-agent-core/tests/tool_bash.rs`
- Modify: `crates/neo-agent-core/tests/runtime_turn.rs`
- Modify: `crates/neo-agent-core/tests/tool_permissions.rs`
- Modify: `crates/neo-agent/tests/tool_bash_guardian.rs`
- Modify: `crates/neo-agent/tests/tool_terminal_guardian.rs`

**Interfaces:**

- Consumes Task 1's final `GuardLimits` names.
- Produces `Option<Duration>` start signatures used by scheduler wiring.

- [ ] **Step 1: Write schema/protocol RED tests**

Add exact assertions for these contracts:

```rust
#[test]
fn bash_schema_uses_optional_timeout_secs_without_legacy_timeout() {
    let bash = ToolRegistry::with_builtin_tools()
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Bash")
        .expect("Bash spec");
    let schema = bash.input_schema.get("schema").unwrap_or(&bash.input_schema);
    let properties = schema["properties"].as_object().expect("properties");
    assert!(properties.contains_key("timeout_secs"));
    assert!(!properties.contains_key("timeout"));
    let text = properties["timeout_secs"].to_string();
    assert!(text.contains("7200"));
    assert!(!text.to_lowercase().contains("rust"));
    assert!(!text.to_lowercase().contains("cargo"));
}

#[test]
fn guard_start_round_trips_without_deadline() {
    let request = GuardRequest::Start {
        request_id: 1,
        request: StartRequest {
            task_id: "task-1".to_owned(),
            kind: GuardTaskKind::Bash,
            command: "printf ready".to_owned(),
            limits: GuardLimits {
                timeout_ms: None,
                max_command_parallelism: 4,
                max_command_descendant_processes: 32,
                max_command_memory_percent: 25,
                max_output_bytes: 65_536,
                max_background_log_bytes: 10_485_760,
            },
            status_dir: PathBuf::from("status"),
            cols: None,
            rows: None,
        },
    };
    let bytes = encode_request_for_test(&request).expect("encode");
    assert_eq!(decode_request_for_test(&bytes).expect("decode"), request);
    let GuardRequest::Start { request, .. } = request else {
        unreachable!("constructed Start request");
    };
    assert_eq!(request.limits.timeout_ms, None);
}

#[tokio::test]
async fn terminal_timeout_is_valid_only_for_start() {
    let schema = TerminalTool.input_schema();
    let schema = schema.get("schema").unwrap_or(&schema);
    let properties = schema["properties"].as_object().expect("properties");
    let timeout_schema = properties["timeout_secs"].to_string();
    assert!(timeout_schema.contains("7200"));
    assert!(!timeout_schema.to_lowercase().contains("rust"));
    assert!(!timeout_schema.to_lowercase().contains("cargo"));
    let temp = tempfile::tempdir().expect("tempdir");
    let context = ToolContext::new(temp.path()).expect("tool context");
    let error = TerminalTool
        .execute(
            &context,
            json!({"mode": "read", "handle": "missing", "timeout_secs": 5}),
        )
        .await
        .expect_err("non-start timeout was accepted");
    assert!(error.to_string().contains("timeout_secs is valid only for start"));
    let error = TerminalTool
        .execute(
            &context,
            json!({"mode": "start", "command": "printf ready", "timeout_secs": 0}),
        )
        .await
        .expect_err("zero start timeout was accepted");
    assert!(error.to_string().contains("timeout_secs must be positive"));
}

#[test]
fn runtime_shell_rejects_removed_timeout_keys() {
    for key in ["foreground_timeout_secs", "background_timeout_secs"] {
        let input = format!("[runtime.shell]\n{key} = 1\n");
        let (_temp, config_path, project_dir) = temp_project_config(&input);
        let error = AppConfig::load(ConfigOverrides {
            config_path: Some(config_path),
            yolo: false,
            auto: false,
            trust_store: None,
            project_dir: Some(project_dir),
        })
        .expect_err("removed timeout key was accepted");
        let message = error.to_string();
        assert!(message.contains(key), "{error:#}");
        assert!(message.contains("max_active_commands"), "{error:#}");
        assert!(message.contains("max_command_parallelism"), "{error:#}");
    }
}
```

Use the existing Tool trait invocation style in `terminal.rs`; the Bash and protocol tests above use the current registry and private codec helpers directly.

- [ ] **Step 2: Run RED**

```bash
cargo test --package neo-agent-core --test tool_bash -- bash_schema_uses_optional_timeout_secs_without_legacy_timeout --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::protocol::tests::guard_start_round_trips_without_deadline --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::terminal::tests::terminal_timeout_is_valid_only_for_start --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::tests::runtime_shell_rejects_removed_timeout_keys --exact --nocapture --include-ignored
```

Expected: FAIL on the legacy Bash field, mandatory numeric deadline, and missing Terminal field.

- [ ] **Step 3: Freeze the public and internal timeout shapes**

Use this schema text verbatim on Bash and Terminal Start:

```text
Optional execution timeout in seconds. Omit this field to allow the command to run until it finishes or is cancelled. For potentially long-running work, prefer omission; if a limit is necessary, do not set it below 7200 seconds. Use shorter values only for commands that are explicitly expected to finish quickly.
```

The relevant final fields/signatures are:

```rust
struct BashInput {
    command: String,
    cwd: Option<String>,
    #[schemars(range(min = 1))]
    timeout_secs: Option<u64>,
    run_in_background: Option<bool>,
    description: Option<String>,
    max_output_bytes: Option<usize>,
}

struct TerminalInput {
    mode: TerminalMode,
    command: Option<String>,
    handle: Option<String>,
    input: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
    #[schemars(range(min = 1))]
    timeout_secs: Option<u64>,
    max_output_bytes: Option<usize>,
}

pub struct ShellExecutionRequest {
    pub id: String,
    pub command: String,
    pub cwd: PathBuf,
    pub origin: ShellCommandOrigin,
    pub timeout: Option<Duration>,
    pub max_output_bytes: usize,
    pub cancel_token: CancellationToken,
    pub stream_update: Option<ToolUpdateCallback>,
    pub background_tasks: Option<BackgroundTaskManager>,
    pub shell_runtime: ShellRuntime,
}

pub(crate) async fn GuardianClient::start_bash(
    runtime: &ShellRuntime,
    task_id: String,
    command_text: String,
    cwd: &Path,
    status_dir: &Path,
    timeout: Option<Duration>,
    max_output_bytes: usize,
    stream_update: Option<ToolUpdateCallback>,
) -> Result<Self, ToolError>;

pub(crate) async fn GuardianClient::start_terminal(
    runtime: &ShellRuntime,
    task_id: String,
    command_text: String,
    cwd: &Path,
    status_dir: &Path,
    cols: u16,
    rows: u16,
    timeout: Option<Duration>,
) -> Result<Self, ToolError>;
```

During this task, keep the existing fail-fast permit acquisition inside the guardian start path; Task 3 replaces it with queued acquisition and adds the permit parameters. Reject explicit zero before constructing `Duration`. Delete `ToolContext.bash_timeout`, `with_bash_timeout`, the timeout assignment in `with_shell_runtime`, both timeout fields from `ShellLimits` and `FileRuntimeShellConfig`, and both timeout assignments in `runtime_shell_from_file`.

Rename only Bash input fixtures from `"timeout"` to `"timeout_secs"` in `tool_bash.rs`, `runtime_turn.rs`, and `tool_permissions.rs`; `TaskOutput.timeout` is a separate blocking-wait contract and remains unchanged. Remove the `with_bash_timeout` setup from permission tests. Replace `shell_mode_uses_spec_timeouts_for_user_commands` with `shell_mode_omits_execution_timeout_for_user_commands`, asserting `ShellExecutionRequest.timeout == None`.
Rename `bash_requires_permission_and_honors_timeout` to `bash_requires_permission_and_rejects_zero_timeout_secs`; preserve its permission/output assertions and change its final assertion to `ToolError::InvalidInput` for explicit zero.

- [ ] **Step 4: Delete deadline mutation from IPC and detach**

Make `GuardLimits.timeout_ms: Option<u64>` the sole deadline field. Delete:

- `GuardLimits.background_timeout_ms`;
- `GuardRequest::SetBackgroundDeadline`;
- its frame kind, encoder, decoder, guardian/Terminal handlers, and tests;
- `GuardianClient::set_background_deadline`; and
- the call from `BackgroundTaskManager::detach`.

Detach now only marks the already-running manager record detached and returns its snapshot. It preserves the timeout selected at process start.

- [ ] **Step 5: Use a real optional deadline in both guardians**

Do not emulate no-timeout with `u64::MAX`. Use one helper shape in each supervision module or one shared private helper:

```rust
fn command_deadline(timeout_ms: Option<u64>) -> Option<Pin<Box<tokio::time::Sleep>>> {
    timeout_ms.map(|millis| Box::pin(tokio::time::sleep(Duration::from_millis(millis))))
}

async fn wait_for_deadline(deadline: &mut Option<Pin<Box<tokio::time::Sleep>>>) {
    match deadline {
        Some(deadline) => deadline.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}
```

Select `wait_for_deadline(&mut deadline)` alongside control EOF, resource sampling, child exit, and Stop. Start the deadline after child spawn/containment succeeds, not before queueing or guardian launch.

- [ ] **Step 6: Update user shell and guardian behavior tests**

Replace `ShellRunRequest.foreground_timeout/background_timeout` with one `timeout: Option<Duration>` set to `None` by local `!` mode. Add one explicit short-deadline integration test and one no-deadline protocol assertion; do not add a minutes-long test.

Run:

```bash
cargo test --package neo-agent --test tool_bash_guardian -- explicit_timeout_starts_after_guardian_start_and_kills_tree --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_start_accepts_no_execution_deadline --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::shell_mode_omits_execution_timeout_for_user_commands --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test tool_permissions -- bash_requires_permission_and_rejects_zero_timeout_secs --exact --nocapture --include-ignored
```

Expected after implementation: PASS.

- [ ] **Step 7: Run Task 2 GREEN and commit**

Run all eight exact commands from Steps 2 and 6. Expected: PASS.

When safe to commit:

```bash
git add crates/neo-agent-core/src/tools/shell_guard/mod.rs crates/neo-agent-core/src/tools/shell_guard/protocol.rs crates/neo-agent-core/src/tools/shell_guard/client.rs crates/neo-agent-core/src/tools/shell_guard/guardian.rs crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent/src/config/types.rs crates/neo-agent/src/config/loader.rs crates/neo-agent/src/config/mod.rs crates/neo-agent/src/modes/interactive/shell_command.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent-core/tests/runtime_turn.rs crates/neo-agent-core/tests/tool_bash.rs crates/neo-agent-core/tests/tool_permissions.rs crates/neo-agent/tests/tool_bash_guardian.rs crates/neo-agent/tests/tool_terminal_guardian.rs
git commit -m "refactor(shell): make execution deadlines explicit"
```

---

### Task 3: Add the Fair, Cancellation-Safe Scheduler and Wire Every Launch

**Files:**

- Create: `crates/neo-agent-core/src/tools/shell_guard/scheduler.rs`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/mod.rs:1-30, 261-424`
- Modify: `crates/neo-agent-core/src/tools/shell_guard/client.rs:106-217, 364-473`
- Modify: `crates/neo-agent-core/src/tools/bash.rs:269-466, 555-590`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs:127-179`
- Modify: `crates/neo-agent/src/modes/interactive/shell_command.rs:90-145`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs:700-735`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs:8090-8175`
- Modify: `crates/neo-agent/tests/tool_bash_guardian.rs`
- Modify: `crates/neo-agent/tests/tool_terminal_guardian.rs`

**Interfaces:**

- Consumes Task 2's optional timeout start signatures.
- Produces the public admission metadata/callback types used by cross-crate shell requests and the crate-private `ShellCommandPermit` retained by the guardian.

- [ ] **Step 1: Write scheduler RED tests before production code**

Place the tests in the new scheduler module. Cover these exact observable sequences:

```rust
use ShellAdmissionClass::{AgentBackground, AgentForeground, User};

fn agent(owner: &str, class: ShellAdmissionClass) -> ShellAdmissionRequest {
    ShellAdmissionRequest {
        owner: owner.to_owned(),
        class,
    }
}

fn spawn_waiter(
    scheduler: &Arc<ShellScheduler>,
    owner: &'static str,
    class: ShellAdmissionClass,
    label: &'static str,
    order: Arc<Mutex<Vec<&'static str>>>,
) -> tokio::task::JoinHandle<ShellCommandPermit> {
    let scheduler = scheduler.clone();
    tokio::spawn(async move {
        let permit = scheduler.acquire(agent(owner, class), None).await;
        order.lock().expect("order lock").push(label);
        permit
    })
}

async fn wait_for_queued(scheduler: &ShellScheduler, expected: usize) {
    while scheduler.queued_count() != expected {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn immediate_admission_does_not_emit_queue_events() {
    let scheduler = ShellScheduler::new(1);
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let callback: ShellAdmissionCallback = Arc::new(move |event| {
        observed.lock().expect("events lock").push(event);
    });
    let permit = scheduler
        .acquire(agent("a", AgentForeground), Some(callback))
        .await;
    assert!(events.lock().expect("events lock").is_empty());
    drop(permit);
}

#[tokio::test]
async fn waits_at_capacity_and_grants_after_drop() {
    let scheduler = ShellScheduler::new(1);
    let first = scheduler.acquire(agent("a", AgentForeground), None).await;
    let second = tokio::spawn({
        let scheduler = scheduler.clone();
        async move { scheduler.acquire(agent("b", AgentForeground), None).await }
    });
    wait_for_queued(&scheduler, 1).await;
    assert!(!second.is_finished());
    drop(first);
    let permit = second.await.expect("waiter task");
    drop(permit);
    assert_eq!(scheduler.running_counts(), (0, 0));
}

#[tokio::test]
async fn user_then_foreground_then_background_and_owner_round_robin() {
    let scheduler = ShellScheduler::new(1);
    let held = scheduler.acquire(agent("hold", AgentForeground), None).await;
    let order = Arc::new(Mutex::new(Vec::new()));
    let bg_a1 = spawn_waiter(&scheduler, "a", AgentBackground, "bg-a1", order.clone());
    wait_for_queued(&scheduler, 1).await;
    let bg_a2 = spawn_waiter(&scheduler, "a", AgentBackground, "bg-a2", order.clone());
    wait_for_queued(&scheduler, 2).await;
    let bg_b1 = spawn_waiter(&scheduler, "b", AgentBackground, "bg-b1", order.clone());
    wait_for_queued(&scheduler, 3).await;
    let fg_a1 = spawn_waiter(&scheduler, "a", AgentForeground, "fg-a1", order.clone());
    wait_for_queued(&scheduler, 4).await;
    let fg_b1 = spawn_waiter(&scheduler, "b", AgentForeground, "fg-b1", order.clone());
    wait_for_queued(&scheduler, 5).await;
    let user = spawn_waiter(&scheduler, "user", User, "user", order.clone());
    wait_for_queued(&scheduler, 6).await;

    drop(held);
    drop(user.await.expect("user grant"));
    drop(fg_a1.await.expect("foreground a grant"));
    drop(fg_b1.await.expect("foreground b grant"));
    drop(bg_a1.await.expect("background a1 grant"));
    drop(bg_b1.await.expect("background b1 grant"));
    drop(bg_a2.await.expect("background a2 grant"));
    assert_eq!(
        *order.lock().expect("order lock"),
        ["user", "fg-a1", "fg-b1", "bg-a1", "bg-b1", "bg-a2"]
    );
}

#[tokio::test]
async fn fourth_background_waits_while_same_owner_foreground_uses_fourth_slot() {
    let scheduler = ShellScheduler::new(4);
    let bg1 = scheduler.acquire(agent("a", AgentBackground), None).await;
    let bg2 = scheduler.acquire(agent("b", AgentBackground), None).await;
    let bg3 = scheduler.acquire(agent("c", AgentBackground), None).await;
    let bg4 = spawn_waiter(
        &scheduler,
        "d",
        AgentBackground,
        "bg4",
        Arc::new(Mutex::new(Vec::new())),
    );
    wait_for_queued(&scheduler, 1).await;
    assert!(!bg4.is_finished());
    let foreground = scheduler.acquire(agent("d", AgentForeground), None).await;
    assert_eq!(scheduler.running_counts(), (4, 3));
    drop(foreground);
    assert!(!bg4.is_finished());
    drop(bg1);
    drop(bg4.await.expect("fourth background grant"));
    drop(bg2);
    drop(bg3);
    assert_eq!(scheduler.running_counts(), (0, 0));
}

#[tokio::test]
async fn dropping_waiter_during_grant_never_leaks_capacity() {
    let scheduler = ShellScheduler::new(1);
    for release_first in [false, true].into_iter().cycle().take(64) {
        let held = scheduler.acquire(agent("hold", AgentForeground), None).await;
        let waiter = spawn_waiter(
            &scheduler,
            "cancelled",
            AgentForeground,
            "cancelled",
            Arc::new(Mutex::new(Vec::new())),
        );
        wait_for_queued(&scheduler, 1).await;
        if release_first {
            drop(held);
            tokio::task::yield_now().await;
            waiter.abort();
        } else {
            waiter.abort();
            drop(held);
        }
        let _ = waiter.await;
        let probe = scheduler.acquire(agent("probe", AgentForeground), None).await;
        drop(probe);
        assert_eq!(scheduler.running_counts(), (0, 0));
    }
}

#[tokio::test]
async fn positions_follow_class_local_owner_round_robin() {
    let scheduler = ShellScheduler::new(1);
    let held = scheduler.acquire(agent("hold", AgentForeground), None).await;
    let positions = Arc::new(Mutex::new(HashMap::<&'static str, usize>::new()));
    let mut waiters = Vec::new();
    for (index, (owner, label)) in [("a", "a1"), ("b", "b1"), ("a", "a2")]
        .into_iter()
        .enumerate()
    {
        let observed = positions.clone();
        let callback: ShellAdmissionCallback = Arc::new(move |event| {
            if let ShellAdmissionEvent::Position { position, .. } = event {
                observed.lock().expect("positions lock").insert(label, position);
            }
        });
        let queued = scheduler.clone();
        waiters.push(tokio::spawn(async move {
            queued.acquire(agent(owner, AgentForeground), Some(callback)).await
        }));
        wait_for_queued(&scheduler, index + 1).await;
    }
    loop {
        let ready = positions.lock().expect("positions lock").len() == 3;
        if ready {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        *positions.lock().expect("positions lock"),
        HashMap::from([("a1", 1), ("b1", 2), ("a2", 3)])
    );
    drop(held);
    let first = waiters.remove(0).await.expect("first grant");
    loop {
        let ready = {
            let positions = positions.lock().expect("positions lock");
            positions.get("b1") == Some(&1) && positions.get("a2") == Some(&2)
        };
        if ready {
            break;
        }
        tokio::task::yield_now().await;
    }
    drop(first);
    for waiter in waiters {
        waiter.abort();
        let _ = waiter.await;
    }
    assert_eq!(scheduler.running_counts(), (0, 0));
}

#[tokio::test]
async fn shell_runtime_clones_share_scheduler() {
    let runtime = ShellRuntime::for_tests(ShellLimits {
        max_active_commands: 1,
        ..ShellLimits::default()
    });
    let held = runtime
        .acquire(agent("a", AgentForeground), None)
        .await;
    let clone = runtime.clone();
    let queued = tokio::spawn(async move {
        clone.acquire(agent("b", AgentForeground), None).await
    });
    while runtime.scheduler.queued_count() != 1 {
        tokio::task::yield_now().await;
    }
    assert!(!queued.is_finished());
    drop(held);
    drop(queued.await.expect("clone grant"));
    assert_eq!(runtime.scheduler.running_counts(), (0, 0));
}
```

Add `#[cfg(test)]` `running_counts() -> (usize, usize)` and `queued_count() -> usize` accessors that read state under the scheduler mutex. The state-based wait helper makes task enqueue deterministic without wall-clock sleeps; the final order vector remains the externally visible fairness assertion.

- [ ] **Step 2: Run scheduler RED**

```bash
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::waits_at_capacity_and_grants_after_drop --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::immediate_admission_does_not_emit_queue_events --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::user_then_foreground_then_background_and_owner_round_robin --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::fourth_background_waits_while_same_owner_foreground_uses_fourth_slot --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::dropping_waiter_during_grant_never_leaks_capacity --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::positions_follow_class_local_owner_round_robin --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::shell_guard::scheduler::tests::shell_runtime_clones_share_scheduler --exact --nocapture --include-ignored
```

Expected: FAIL because the module/API does not exist.

- [ ] **Step 3: Implement the frozen scheduler surface**

Use these exact visibility boundaries:

```rust
pub(crate) const MAX_AGENT_BACKGROUND_COMMANDS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAdmissionClass {
    User,
    AgentForeground,
    AgentBackground,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellAdmissionRequest {
    pub owner: String,
    pub class: ShellAdmissionClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellAdmissionEvent {
    Queued,
    Position { position: usize, waiting: Duration },
    Started,
}

pub type ShellAdmissionCallback =
    Arc<dyn Fn(ShellAdmissionEvent) + Send + Sync>;

pub(crate) struct ShellScheduler {
    capacity: usize,
    state: std::sync::Mutex<SchedulerState>,
}

pub(crate) struct ShellCommandPermit {
    scheduler: Arc<ShellScheduler>,
    class: ShellAdmissionClass,
}
```

`ShellScheduler::new(capacity) -> Arc<Self>` constructs the one shared object. State uses a user `VecDeque<Waiter>`, one `HashMap<String, VecDeque<Waiter>>` plus owner `VecDeque<String>` per agent class, total/background counters, and a monotonic waiter ID.

Export the four admission metadata/callback types through `tools/mod.rs` for local user shell mode in `neo-agent`. Keep `ShellScheduler`, waiter state, and `ShellCommandPermit` crate-private.

Always enqueue first, then dispatch under the mutex so a new arrival cannot jump an existing eligible waiter. Record whether the new waiter was granted by that same locked mutation. Dispatch repeatedly while total capacity exists, selecting User, foreground, then eligible background. Background is eligible only while `running_background < capacity.min(MAX_AGENT_BACKGROUND_COMMANDS)`. Increment counters before creating the permit. Send permits and invoke callbacks only after unlocking.

The await path owns a `WaitRegistration { id, Weak<ShellScheduler>, armed }`. Its Drop removes the waiter if still queued. If a permit send fails, drop that permit immediately and dispatch again. The permit is non-cloneable; its Drop decrements counters and dispatches.

If the same mutation granted the new waiter, emit neither Queued nor Position. Otherwise the acquire future emits `Queued` plus its first Position before awaiting its receiver, then marks the waiter ready under the mutex and recomputes its current rank, emitting a correction if the rank changed during initial notification. A concurrent release may grant it meanwhile, but `acquire` still delivers Queued/Position before returning the already-sent permit, so the caller's Started callback cannot overtake them. Compute class-local positions by traversing a copy of the current owner ring and FIFOs in round-robin order. Never run callbacks under the state mutex, and the scheduler itself never invokes Started.

- [ ] **Step 4: Replace `ShellRuntime::try_acquire` and the atomic counter**

`ShellRuntime` stores `Arc<ShellScheduler>` initialized with `limits.max_active_commands`. Its final API is:

```rust
pub(crate) async fn acquire(
    &self,
    request: ShellAdmissionRequest,
    callback: Option<ShellAdmissionCallback>,
) -> ShellCommandPermit {
    self.scheduler.acquire(request, callback).await
}
```

Delete `active: Arc<AtomicUsize>`, `try_acquire`, the old permit implementation, and the fail-fast test. Remove `ResourceLimitCause::ActiveCommands` from the enum, serializer name, exhaustive matches, `model_bash_error_result`, and tests; no capacity error replaces it. Re-export the scheduler types from `shell_guard` only as widely as their callers require.

- [ ] **Step 5: Acquire before guardian spawn and retain the permit**

Move permit acquisition out of `spawn_guardian_and_handshake`. Add `permit: ShellCommandPermit` as the final parameter of `GuardianClient::start_bash`, `GuardianClient::start_terminal`, their private `start`, and `GuardianStartArgs`. Move it into `ReaderTaskArgs`, which retains it until the guardian response task exits. A failed directory creation, spawn, pipe setup, Start write, or handshake returns normally and drops the permit.

Use these classes:

- local `!` command: `User`, owner `"user"`;
- model foreground Bash: `AgentForeground`, owner `ctx.agent_id` or `crate::session::MAIN_AGENT_ID`;
- model background Bash: `AgentBackground`, same owner rule;
- Terminal Start: `AgentBackground`, same owner rule.

Terminal Write/Read/Resize/Stop never acquire. A detached local shell retains its User permit and never changes the background counter.

Task 3 adds these fields to `ShellExecutionRequest`; all existing constructors set the class/owner above and use `None` until Task 4 installs event callbacks:

```rust
pub admission: ShellAdmissionRequest,
pub admission_callback: Option<ShellAdmissionCallback>,
```

- [ ] **Step 6: Make acquisition cancellation-safe at each caller**

Model tool execution is already selected against its turn cancellation token; dropping the tool future drops `WaitRegistration`. For local shell mode, explicitly select:

```rust
let permit = tokio::select! {
    permit = request.shell_runtime.acquire(request.admission.clone(), request.admission_callback.clone()) => permit,
    () = request.cancel_token.cancelled() => return Err(ToolError::Cancelled),
};
if request.cancel_token.is_cancelled() {
    drop(permit);
    return Err(ToolError::Cancelled);
}
```

After grant, perform the same cancellation recheck in model Bash/Terminal callers, re-run `ensure_shell_allowed` where a ToolContext exists, resolve/canonicalize cwd again, clamp output, and validate timeout conversion before invoking the callback's `Started` and spawning. If cancellation or revalidation fails, return that error and let the permit drop.

Rewrite `refresh_config_preserves_live_task_and_multi_agent_state` so it no longer calls deleted `try_acquire`: assert that refresh retains the current `runtime.shell` values and `runtime_root` even when the file changes `max_active_commands`. `shell_runtime_clones_share_scheduler` above is the focused proof that the retained clone shares live admission state.

- [ ] **Step 7: Add one real shared-runtime integration test**

In `tool_bash_guardian.rs`, construct one `ShellRuntime` with capacity one. Start a guardian command held by a deterministic stdin/channel fixture, start a second command with the same runtime, assert its guardian PID/status marker does not exist, release the first, then assert the second starts and completes. Name it:

```rust
queued_bash_does_not_spawn_guardian_before_permit
```

Run:

```bash
cargo test --package neo-agent --test tool_bash_guardian -- queued_bash_does_not_spawn_guardian_before_permit --exact --nocapture --include-ignored
```

Expected: PASS after implementation; no wall-clock sleep is used as the release mechanism.

- [ ] **Step 8: Run Task 3 GREEN and commit**

Run the seven scheduler commands and the integration command. Expected: PASS.

When safe to commit:

```bash
git add crates/neo-agent-core/src/tools/shell_guard/scheduler.rs crates/neo-agent-core/src/tools/shell_guard/mod.rs crates/neo-agent-core/src/tools/shell_guard/client.rs crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent/src/modes/interactive/shell_command.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs crates/neo-agent/tests/tool_bash_guardian.rs crates/neo-agent/tests/tool_terminal_guardian.rs
git commit -m "feat(shell): queue commands with fair admission"
```

---

### Task 4: Emit Truthful Queue/Start Events Without Polluting Tool Results

**Files:**

- Modify: `crates/neo-agent-core/src/events.rs:85-175`
- Modify: `crates/neo-agent-core/src/tools/mod.rs:198-360`
- Modify: `crates/neo-agent-core/src/runtime/events.rs:118-310`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs:274-680`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Modify: `crates/neo-agent-core/src/session/event_persistence.rs:14-90`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs:700-735`
- Modify: `crates/neo-agent/src/modes/interactive/shell_command.rs:90-140`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`

**Interfaces:**

- Consumes Task 3's callback and Started invocation.
- Produces the final `AgentEvent` queue contract consumed by the main and child TUIs.

- [ ] **Step 1: Write event-order and persistence RED tests**

Add an event-order test that holds the only scheduler permit, starts one approved Bash call, and observes events without allowing guardian spawn:

```rust
#[tokio::test]
async fn approved_bash_emits_queued_then_started_only_after_grant() {
    let workspace = tempfile::tempdir().expect("workspace");
    let runtime = ShellRuntime::new(
        ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        },
        PathBuf::from("missing-guardian"),
        workspace.path().join("runtime"),
    );
    let held = runtime
        .acquire(
            ShellAdmissionRequest {
                owner: "hold".to_owned(),
                class: ShellAdmissionClass::AgentForeground,
            },
            None,
        )
        .await;
    let config = AgentConfig::for_model(crate::harness::fake_model())
        .with_workspace_root(workspace.path())
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler(|_| PermissionApprovalDecision::AllowOnce)
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_shell_runtime(runtime);
    let model: Arc<dyn ModelClient> = Arc::new(
        neo_ai::providers::fake::FakeModelClient::new(Vec::new()),
    );
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let calls = [AgentToolCall {
        id: "call-1".into(),
        name: "Bash".into(),
        raw_arguments: r#"{"command":"printf ready"}"#.into(),
    }];
    let cancel = CancellationToken::new();
    let supervisor = ProcessSupervisor::default();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut emitter = EventEmitter::new(tx, AgentContext::new());
    let run = execute_tool_calls(
        &config,
        model,
        registry,
        None,
        1,
        &calls,
        &mut emitter,
        &cancel,
        &supervisor,
    );
    tokio::pin!(run);
    let mut approval_seen = false;
    loop {
        tokio::select! {
            event = rx.recv() => {
                let event = event.expect("event channel").expect("runtime event");
                if matches!(event, AgentEvent::ApprovalRequested { .. }) {
                    approval_seen = true;
                }
                if matches!(event, AgentEvent::ToolExecutionQueued { .. }) {
                    assert!(approval_seen, "Bash queued before approval completed");
                    break;
                }
                assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
            }
            result = &mut run => panic!("Bash returned before admission: {result:?}"),
        }
    }
    while let Ok(Ok(event)) = rx.try_recv() {
        assert!(!matches!(event, AgentEvent::ToolExecutionStarted { .. }));
    }
    drop(held);
    let results = run.await.expect("tool dispatch");
    let events = std::iter::from_fn(|| rx.try_recv().ok())
        .collect::<Result<Vec<_>, _>>()
        .expect("runtime events");
    assert!(events.iter().any(|event| matches!(event, AgentEvent::ToolExecutionStarted { .. })));
    let result = &results[0].1;
    let model_visible = format!("{} {:?}", result.content, result.details);
    assert!(!model_visible.contains("position"));
    assert!(!model_visible.contains("waiting_ms"));
}

#[test]
fn session_persists_queue_transition_but_not_live_queue_updates() {
    let mut persistence = SessionEventPersistence::default();
    let queued = AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "printf ready"}),
    };
    let update = AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "call-1".to_owned(),
        position: 2,
        waiting_ms: 18_000,
    };
    assert_eq!(persistence.persisted_events(&queued), vec![queued]);
    assert!(persistence.persisted_events(&update).is_empty());
}
```

Also assert the eventual Bash Tool Result contains neither `position` nor `waiting_ms`.

- [ ] **Step 2: Run RED**

```bash
cargo test --package neo-agent-core --lib -- runtime::tool_dispatch::tests::approved_bash_emits_queued_then_started_only_after_grant --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test session_jsonl -- session_persists_queue_transition_but_not_live_queue_updates --exact --nocapture --include-ignored
```

Expected: FAIL because queue variants do not exist and Started is emitted before permission.

- [ ] **Step 3: Add the exact event variants**

Add the four variants from the design:

```rust
ToolExecutionQueued {
    turn: u32,
    id: String,
    name: String,
    arguments: serde_json::Value,
},
ToolExecutionQueueUpdated {
    turn: u32,
    id: String,
    position: usize,
    waiting_ms: u64,
},
ShellCommandQueued {
    turn: u32,
    id: String,
    command: String,
    cwd: PathBuf,
    origin: ShellCommandOrigin,
},
ShellCommandQueueUpdated {
    turn: u32,
    id: String,
    position: usize,
    waiting_ms: u64,
},
```

`SessionEventPersistence` returns an empty vector for both `*QueueUpdated` variants and persists both Queued variants through its default branch. `apply_to_context` ignores all four; they are never messages.

- [ ] **Step 4: Attach one callback after permission**

Add a private optional `shell_admission_callback` to `ToolContext` with a builder. In `runtime/events.rs`, build one callback from the same `Arc<serde_json::Value>` used by the suspended tool future, so admission does not deep-copy command arguments into each waiter:

```rust
pub(super) fn make_shell_admission_callback(
    sink: EventSink,
    turn: u32,
    id: String,
    name: String,
    arguments: Arc<serde_json::Value>,
    bash_display_cwd: PathBuf,
) -> ShellAdmissionCallback {
    Arc::new(move |event| match event {
        ShellAdmissionEvent::Queued => {
            sink.emit_event(AgentEvent::ToolExecutionQueued {
                turn,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.as_ref().clone(),
            });
        }
        ShellAdmissionEvent::Position { position, waiting } => {
            sink.emit_event(AgentEvent::ToolExecutionQueueUpdated {
                turn,
                id: id.clone(),
                position,
                waiting_ms: u64::try_from(waiting.as_millis()).unwrap_or(u64::MAX),
            });
        }
        ShellAdmissionEvent::Started => {
            sink.emit_event(AgentEvent::ToolExecutionStarted {
                turn,
                id: id.clone(),
                name: name.clone(),
                arguments: arguments.as_ref().clone(),
            });
            if name == "Bash"
                && let Some(command) = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
            {
                sink.emit_event(AgentEvent::ShellCommandStarted {
                    turn,
                    id: id.clone(),
                    command: command.to_owned(),
                    cwd: bash_display_cwd.clone(),
                    origin: ShellCommandOrigin::ModelBashTool,
                });
            }
        }
    })
}
```

User shell mode builds the parallel callback using `ShellCommandQueued`, `ShellCommandQueueUpdated`, and `ShellCommandStarted` on its existing event channel.
Delete the unconditional `ShellCommandStarted` emitted by `InteractiveController::start_shell_command`; the callback is the sole owner of that transition.

The scheduler calls only Queued/Position. Bash, Terminal Start, and local shell callers invoke Started after their post-grant checks. An immediately admitted call therefore emits Started without first emitting or persisting a synthetic Queued event.

- [ ] **Step 5: Repair sequential and parallel dispatch ordering**

Delete the unconditional `ToolExecutionStarted` emissions at the top of both execution paths. The final ordering is:

1. parsed argument failure -> Finished only;
2. `before_tool_result` block -> Finished only;
3. permission deny/reject -> Finished only;
4. allowed non-admission tool -> Started immediately before `run_tool_with_cancel`;
5. allowed Bash or Terminal Start -> install admission callback; tool emits Queued/updates and later Started;
6. Terminal Write/Read/Resize/Stop -> normal immediate Started.

Use one helper:

```rust
fn uses_shell_admission(name: &str, arguments: &serde_json::Value) -> bool {
    name == "Bash"
        || (name == "Terminal"
            && arguments.get("mode").and_then(serde_json::Value::as_str) == Some("start"))
}
```

Both dispatch modes must call the same helper and callback builder. Do not duplicate a second event contract in Bash/Terminal Tool Results.

- [ ] **Step 6: Run GREEN and commit Task 4**

Run both commands from Step 2. Expected: PASS.

When safe to commit, note that `events.rs` and `event_persistence.rs` may contain unrelated work; stage only plan-owned hunks or skip the commit:

```bash
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/runtime/events.rs crates/neo-agent-core/src/runtime/tool_dispatch.rs crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/src/session/event_persistence.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/shell_command.rs crates/neo-agent-core/tests/session_jsonl.rs
git commit -m "feat(runtime): expose shell queue lifecycle"
```

---

### Task 5: Render Main Tool and User-Shell Queues In Place

**Files:**

- Modify: `crates/neo-tui/src/shell/stream.rs:56-65`
- Modify: `crates/neo-tui/src/transcript/tool_call.rs:26-248`
- Modify: `crates/neo-tui/src/transcript/tool_renderers.rs:30-125`
- Modify: `crates/neo-tui/src/transcript/shell_run.rs:12-256`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs:261-360, 627-760`
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs:520-590`
- Modify: `crates/neo-tui/src/transcript/pane.rs`
- Modify: `crates/neo-tui/src/transcript/presentation.rs`
- Modify: `crates/neo-tui/src/transcript/store.rs`
- Modify: `crates/neo-tui/src/transcript/tool_group.rs`
- Modify: `crates/neo-tui/tests/tool_cards.rs`
- Modify: `crates/neo-tui/tests/shell_events.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`

**Interfaces:**

- Consumes Task 4's events.
- Produces exact main-card text and replay finalization behavior; no core data-model changes.

- [ ] **Step 1: Write exact rendering RED tests**

```rust
fn apply_queued_bash(
    pane: &mut TranscriptPane,
    id: &str,
    command: &str,
    position: usize,
    waiting_ms: u64,
) {
    let arguments = json!({"command": command});
    pane.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: id.into(),
            name: "Bash".into(),
            raw_arguments: arguments.to_string().into(),
        },
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: id.to_owned(),
        name: "Bash".to_owned(),
        arguments,
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: id.to_owned(),
        position,
        waiting_ms,
    });
}

#[test]
fn bash_queue_event_renders_position_and_wait_in_original_card() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ToolCallStarted {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
    });
    pane.apply_agent_event(AgentEvent::ToolCallFinished {
        turn: 1,
        tool_call: AgentToolCall {
            id: "call-1".into(),
            name: "Bash".into(),
            raw_arguments: r#"{"command":"cargo test"}"#.into(),
        },
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueued {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "cargo test"}),
    });
    pane.apply_agent_event(AgentEvent::ToolExecutionQueueUpdated {
        turn: 1,
        id: "call-1".to_owned(),
        position: 2,
        waiting_ms: 18_000,
    });
    let rendered = rendered(&mut pane);
    assert!(rendered.contains("Queued Bash (cargo test) · #2 · waiting 18s"));
    assert_eq!(rendered.matches("Queued Bash").count(), 1);
}

#[test]
fn generic_pending_tool_is_not_called_queued() {
    let mut component = ToolCallComponent::new(ToolCallState {
        id: "call-1".to_owned(),
        name: "Read".to_owned(),
        arguments: None,
        result: None,
        details: None,
        status: ToolStatusKind::Pending,
        exit_code: None,
    });
    assert!(plain(component.render(80)).join("\n").contains("Preparing Read"));
}

#[test]
fn user_shell_queue_transitions_to_running_in_place() {
    let mut pane = TranscriptPane::new(80, 12);
    pane.apply_agent_event(AgentEvent::ShellCommandQueued {
        turn: 0,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    pane.apply_agent_event(AgentEvent::ShellCommandQueueUpdated {
        turn: 0,
        id: "shell-1".to_owned(),
        position: 1,
        waiting_ms: 3_000,
    });
    let queued = rendered(&mut pane);
    assert!(queued.contains("#1 · waiting 3s"));
    assert!(!queued.contains("ctrl+b to background"));
    pane.apply_agent_event(AgentEvent::ShellCommandStarted {
        turn: 0,
        id: "shell-1".to_owned(),
        command: "whoami".to_owned(),
        cwd: "/tmp".into(),
        origin: ShellCommandOrigin::UserShellMode,
    });
    let running = rendered(&mut pane);
    assert_eq!(running.matches("$ whoami").count(), 1);
    assert!(running.contains("ctrl+b to background"));
}

#[test]
fn queued_shell_card_keeps_relative_position_across_later_entries() {
    let mut pane = TranscriptPane::new(80, 20);
    apply_queued_bash(&mut pane, "call-1", "cargo test", 1, 4_000);
    pane.push_assistant_message("later assistant text");
    pane.apply_agent_event(AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "call-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: json!({"command": "cargo test"}),
    });
    let rendered = rendered(&mut pane);
    let tool = rendered.find("Bash (cargo test)").expect("tool row");
    let later = rendered.find("later assistant text").expect("later row");
    assert!(tool < later, "living tool card drifted after later content");
}

#[test]
fn replay_finalizes_dangling_shell_queue_without_restart() {
    let mut transcript = TranscriptPane::new(80, 12);
    let loaded = LoadedSessionTranscript::new("alpha", Vec::new(), Vec::new()).with_events([
        AgentEvent::ToolCallStarted {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
        },
        AgentEvent::ToolCallFinished {
            turn: 1,
            tool_call: AgentToolCall {
                id: "call-1".into(),
                name: "Bash".into(),
                raw_arguments: r#"{"command":"cargo test"}"#.into(),
            },
        },
        AgentEvent::ToolExecutionQueued {
            turn: 1,
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            arguments: json!({"command": "cargo test"}),
        },
    ]);
    replay_session_into_transcript(&mut transcript, &loaded);
    let rendered = transcript
        .render_frame(80, 12)
        .expect("render replay")
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Interrupted when terminal exited"), "{rendered}");
    assert!(!rendered.contains("Queued Bash"), "{rendered}");
}
```

- [ ] **Step 2: Run RED**

```bash
cargo test --package neo-tui --test tool_cards -- bash_queue_event_renders_position_and_wait_in_original_card --exact --nocapture --include-ignored
cargo test --package neo-tui --test tool_cards -- generic_pending_tool_is_not_called_queued --exact --nocapture --include-ignored
cargo test --package neo-tui --test shell_events -- user_shell_queue_transitions_to_running_in_place --exact --nocapture --include-ignored
cargo test --package neo-tui --test tool_cards -- queued_shell_card_keeps_relative_position_across_later_entries --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::replay_finalizes_dangling_shell_queue_without_restart --exact --nocapture --include-ignored
```

Expected: FAIL because Pending is overloaded as Queued and queue display state is absent.

- [ ] **Step 3: Add an explicit TUI queue state**

Add `Queued` to `ToolStatusKind`. Keep queue timing private to `ToolCallComponent` so the widely constructed `ToolCallState` does not gain transient clock fields:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct QueueDisplayState {
    position: usize,
    waiting_ms: u64,
    observed_at: Instant,
}

impl QueueDisplayState {
    fn elapsed_ms(&self) -> u64 {
        self.waiting_ms.saturating_add(
            u64::try_from(self.observed_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        )
    }
}
```

`ToolCallComponent::set_queued(position, waiting_ms)` sets status Queued and updates this state. Any transition to `ToolStatusKind::Running`, either terminal status, and result setters clear it. `has_visible_animation()` returns true for Queued so the existing transcript animation tick redraws elapsed wait even when rank does not change.

Update every exhaustive `ToolStatusKind` match in `entry`, `pane`, `presentation`, `store`, and `tool_group`: Queued is live, interruptible, and excluded from finalized grouping just like Running; it is not treated as Succeeded or Failed.

Render Pending as `Preparing`, Queued as `Queued`, and append:

```text
 · #<position> · waiting <format_elapsed(elapsed_seconds)>
```

after the existing `(command)` span. Keep width truncation through the existing line truncation path. Do not add a queue body or ETA.

- [ ] **Step 4: Add queued user-shell state**

Extend `ShellRunState` with:

```rust
Queued {
    position: Option<usize>,
    waiting_ms: u64,
    observed_at: Instant,
},
```

`queued` initializes `position: None`, zero wait, and the current Instant; `update_queue` supplies the first live rank/baseline. Add `start` and `has_visible_animation` methods. Queued rendering keeps `$ <command>` and shows `Queued` without a fabricated rank until the live update arrives, then shows the metadata on the next muted line; it does not show `ctrl+b to background`. Route queued ShellRun entries through `TranscriptEntry::has_visible_animation` so elapsed wait redraws. `ShellCommandStarted` calls `start` on an existing queued entry instead of pushing a second entry. `interrupt` already converts any live state to Cancelled, so replay's existing `finalize_interrupted_live_entries()` makes a dangling queued event historical rather than live.

- [ ] **Step 5: Apply all four queue events**

In `apply_tool_event`:

- `ToolExecutionQueued` upserts the existing tool by ID with arguments and Queued status;
- `ToolExecutionQueueUpdated` mutates only that tool's queue display;
- `ShellCommandQueued` creates/updates one shell run by ID; and
- `ShellCommandQueueUpdated` updates it by ID.

Started transitions the same entries. Finished behavior stays unchanged. Queue updates arriving after Started/Finished are ignored so delayed live notifications cannot regress a card.

- [ ] **Step 6: Run GREEN and commit Task 5**

Run the five commands from Step 2. Expected: PASS.

When safe to commit:

```bash
git add crates/neo-tui/src/shell/stream.rs crates/neo-tui/src/transcript/tool_call.rs crates/neo-tui/src/transcript/tool_renderers.rs crates/neo-tui/src/transcript/shell_run.rs crates/neo-tui/src/transcript/event_handler.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/presentation.rs crates/neo-tui/src/transcript/store.rs crates/neo-tui/src/transcript/tool_group.rs crates/neo-tui/tests/tool_cards.rs crates/neo-tui/tests/shell_events.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(tui): render shell admission queues in place"
```

---

### Task 6: Propagate the Canonical Queued Phase Through Delegate and Swarm

**Files:**

- Modify: `crates/neo-agent-core/src/multi_agent/state.rs:68-115, 190-220`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs:1415-1545, 2282-2360`
- Modify: `crates/neo-agent-core/src/session/event_persistence.rs`
- Modify: `crates/neo-agent/src/modes/task_browser.rs:230-275`
- Modify: `crates/neo-tui/src/transcript/child_activity.rs:24-185, 315-365`
- Modify: `crates/neo-tui/src/transcript/swarm_card.rs:530-600`
- Modify: `crates/neo-agent-core/tests/multi_agent_runtime.rs`
- Modify: `crates/neo-agent-core/tests/session_jsonl.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`

**Interfaces:**

- Consumes Task 4 queue events.
- Produces one canonical child activity entry that changes Queued -> Ongoing -> Done/Failed without losing summary/output.

- [ ] **Step 1: Write child lifecycle RED tests**

```rust
#[test]
fn child_shell_activity_keeps_command_and_output_with_or_without_queue() {
    for starts_queued in [false, true] {
        let runtime = MultiAgentRuntime::new();
        let child = runtime.start_foreground_delegate_for_test("run tests");
        let started_at = std::time::Instant::now();
        let mut events = Vec::new();
        if starts_queued {
            events.extend([
                AgentEvent::ToolExecutionQueued {
                    turn: 1,
                    id: "call-1".to_owned(),
                    name: "Bash".to_owned(),
                    arguments: json!({"command": "cargo test"}),
                },
                AgentEvent::ToolExecutionQueueUpdated {
                    turn: 1,
                    id: "call-1".to_owned(),
                    position: 2,
                    waiting_ms: 18_000,
                },
            ]);
        }
        events.extend([
            AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                arguments: json!({"command": "cargo test"}),
            },
            AgentEvent::ToolExecutionUpdate {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                partial_result: ToolResult::ok("test output"),
            },
            AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: "call-1".to_owned(),
                name: "Bash".to_owned(),
                result: ToolResult::ok("done"),
            },
        ]);
        for event in events {
            runtime.apply_child_event(&child.id, started_at, &event);
        }
        let snapshot = runtime.agent_snapshot(child.id.as_str()).expect("child snapshot");
        let tools = snapshot.activity.iter().filter_map(|entry| match &entry.kind {
            AgentActivityKind::Tool { summary, phase, output, .. } => {
                Some((summary, phase, output))
            }
            AgentActivityKind::Text { .. } => None,
        }).collect::<Vec<_>>();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0.as_deref(), Some("cargo test"));
        assert_eq!(*tools[0].1, AgentToolActivityPhase::Done);
        assert_eq!(tools[0].2.as_ref().map(|o| o.text.as_str()), Some("done"));
    }
}

#[test]
fn delegate_and_swarm_render_same_queued_shell_row() {
    let now_ms = 20_000;
    let mut snapshot = running_delegate();
    snapshot.activity = vec![AgentActivityEntry {
        kind: AgentActivityKind::Tool {
            id: "call-1".to_owned(),
            name: "Bash".to_owned(),
            summary: Some("cargo test".to_owned()),
            phase: AgentToolActivityPhase::Queued {
                position: Some(2),
                queued_at_ms: 2_000,
            },
            output: None,
        },
    }];
    let mut delegate_card = DelegateCardComponent::new(snapshot.clone());
    delegate_card.on_render_tick(now_ms);
    let delegate = plain(delegate_card.render_with_theme(120, &TuiTheme::default())).join("\n");
    let mut swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    swarm.children[0].agent = snapshot.clone();
    let mut swarm_card = SwarmCardComponent::new(swarm);
    swarm_card.set_expanded(true);
    swarm_card.on_render_tick(now_ms);
    let swarm = plain(swarm_card.render_with_theme(120, &TuiTheme::default())).join("\n");
    let expected = "Queued Bash (cargo test) · #2 · waiting 18s";
    assert!(delegate.contains(expected));
    assert!(swarm.contains(expected));

    let AgentActivityKind::Tool { phase, output, .. } = &mut snapshot.activity[0].kind else {
        panic!("expected tool activity");
    };
    *phase = AgentToolActivityPhase::Done;
    *output = Some(AgentToolOutputPreview {
        text: "done".to_owned(),
        is_error: false,
        truncated: false,
        tail: false,
    });
    let mut delegate_card = DelegateCardComponent::new(snapshot.clone());
    let delegate = plain(delegate_card.render_with_theme(120, &TuiTheme::default())).join("\n");
    let mut done_swarm = swarm_with_child_states(vec![AgentLifecycleState::Running]);
    done_swarm.children[0].agent = snapshot;
    let mut swarm_card = SwarmCardComponent::new(done_swarm);
    swarm_card.set_expanded(true);
    let swarm = plain(swarm_card.render_with_theme(120, &TuiTheme::default())).join("\n");
    for rendered in [delegate, swarm] {
        assert!(rendered.contains("Used Bash (cargo test)"), "{rendered}");
        assert!(rendered.contains("done"), "{rendered}");
    }
}
```

- [ ] **Step 2: Run RED**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- child_shell_activity_keeps_command_and_output_with_or_without_queue --exact --nocapture --include-ignored
cargo test --package neo-tui --test multi_agent_transcript -- delegate_and_swarm_render_same_queued_shell_row --exact --nocapture --include-ignored
```

Expected: FAIL because child activity has no queued phase or queue metadata.

- [ ] **Step 3: Extend the existing phase instead of adding another activity model**

Use this final enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum AgentToolActivityPhase {
    Queued {
        position: Option<usize>,
        queued_at_ms: u64,
    },
    Ongoing,
    Done,
    Failed,
}
```

`ToolExecutionQueued` upserts the existing tool-call ID with its summarized arguments and `Queued { position: None, queued_at_ms: now_ms() }`. `ToolExecutionQueueUpdated` finds that same ID and sets `position` plus `queued_at_ms = now_ms().saturating_sub(waiting_ms)`. Started changes it to Ongoing and keeps summary. Update keeps Ongoing and adds preview. Finished changes Done/Failed and keeps both summary and final preview.

Late queue updates may change only an existing Queued entry; they cannot regress Ongoing or terminal phases.

- [ ] **Step 4: Keep live queue metadata out of every parent JSONL event**

Add one `strip_live_queue_metadata` projection in `SessionEventPersistence`. Apply it before persisting all six parent event families: `DelegateStarted`/`DelegateFinished`, `DelegateProgressUpdated`, `DelegateSwarmStarted`/`DelegateSwarmFinished`, and `DelegateSwarmProgressUpdated`. For updated events, normalize before `PersistedAgentProgress::should_persist` and store only the normalized `last_progress`; a live position/wait change must not trigger a compact write. The helper normalizes queued phases in both `AgentSnapshot.activity` and `AgentProgressSnapshot.last_tool` to:

```rust
AgentToolActivityPhase::Queued {
    position: None,
    queued_at_ms: 0,
}
```

The durable Queued transition remains in the child event stream; position and elapsed data do not leak through compact progress or a terminal parent snapshot when cancellation finishes a still-queued child. The projection operates on clones and never mutates the live snapshots used by the TUI. Add an exact session test named:

```text
delegate_persistence_strips_live_shell_queue_metadata
```

Construct one queued `AgentSnapshot`, pass `DelegateFinished` and `DelegateSwarmFinished` through `persisted_events`, and assert every nested queued phase has `position == None` and `queued_at_ms == 0`. Then pass an equivalent `DelegateUpdated`, assert the same fields on the emitted `DelegateProgressUpdated`, change only its live position/wait baseline, and assert the next update emits nothing. This one test covers the shared projection and coalescing gate rather than duplicating a case for every event variant.

- [ ] **Step 5: Render the phase once for Delegate and Swarm**

Add one `child_tool_status_text(name, summary, phase, now_ms)` formatter in `child_activity.rs`. Use it from `render_child_tool_row` and from `swarm_card::child_activity_summary`, so expanded Delegate rows and collapsed/expanded Swarm rows cannot drift. The queued branch uses `Queued`, the existing `(summary)` formatting, and the exact suffix. For a Queued phase without a live position, render `Queued Bash (<command>)` without a fabricated number. Ongoing remains `Using`, Done remains `Used`, Failed remains `Failed`; output preview rendering is unchanged.

Update exhaustive task-browser formatting to label Queued as `queued` and never panic on the data-carrying variant.

- [ ] **Step 6: Verify core, persistence, and TUI GREEN**

```bash
cargo test --package neo-agent-core --test multi_agent_runtime -- child_shell_activity_keeps_command_and_output_with_or_without_queue --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test session_jsonl -- delegate_persistence_strips_live_shell_queue_metadata --exact --nocapture --include-ignored
cargo test --package neo-tui --test multi_agent_transcript -- delegate_and_swarm_render_same_queued_shell_row --exact --nocapture --include-ignored
```

Expected: PASS, with one child tool row throughout.

- [ ] **Step 7: Commit Task 6 when safe**

```bash
git add crates/neo-agent-core/src/multi_agent/state.rs crates/neo-agent-core/src/multi_agent/runtime.rs crates/neo-agent-core/src/session/event_persistence.rs crates/neo-agent/src/modes/task_browser.rs crates/neo-tui/src/transcript/child_activity.rs crates/neo-tui/src/transcript/swarm_card.rs crates/neo-agent-core/tests/multi_agent_runtime.rs crates/neo-agent-core/tests/session_jsonl.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "feat(multi-agent): show queued child shell work"
```

---

### Task 7: Add the Shell-Free `Sleep` Tool and Update User Documentation

**Files:**

- Create: `crates/neo-agent-core/src/tools/sleep.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs:1-150, 547-710`
- Modify: `crates/neo-agent-core/src/runtime/permission.rs:364-385`
- Modify: `crates/neo-agent-core/src/multi_agent/profile.rs:85-145`
- Modify: `crates/neo-agent-core/tests/tool_bash.rs`
- Modify: `docs/en/configuration/config-files.md`
- Modify: `docs/zh/configuration/config-files.md`
- Modify: `docs/en/configuration/permissions.md`
- Modify: `docs/zh/configuration/permissions.md`
- Modify: `docs/en/customization/agents.md`
- Modify: `docs/zh/customization/agents.md`
- Modify: `docs/en/reference/tools.md`
- Modify: `docs/zh/reference/tools.md`

**Interfaces:**

- Produces one built-in `Sleep` tool; it does not consume scheduler interfaces.
- Documents canonical config and schema semantics in English and Chinese.

- [ ] **Step 1: Write Sleep RED tests**

```rust
#[tokio::test]
async fn sleep_validates_bounds_reason_and_cancellation() {
    let tool = SleepTool;
    let spec = tool.spec();
    let schema = spec.input_schema.get("schema").unwrap_or(&spec.input_schema);
    let required = schema["required"].as_array().expect("required fields");
    assert!(required.iter().any(|field| field.as_str() == Some("duration_seconds")));
    assert!(required.iter().any(|field| field.as_str() == Some("reason")));
    assert!(spec.description.contains("WaitDelegate"));
    assert!(spec.description.contains("TaskOutput"));
    assert!(spec.description.contains("block=true"));
    let temp = tempfile::tempdir().expect("tempdir");
    let context = ToolContext::new(temp.path()).expect("tool context");
    for input in [
        json!({"duration_seconds": 0, "reason": "wait"}),
        json!({"duration_seconds": 3601, "reason": "wait"}),
        json!({"duration_seconds": 1, "reason": ""}),
        json!({"duration_seconds": 1, "reason": "line one\nline two"}),
        json!({"duration_seconds": 1, "reason": "x".repeat(161)}),
    ] {
        assert!(tool.execute(&context, input).await.is_err());
    }

    let cancelled = ToolContext::new(temp.path()).expect("cancelled context");
    cancelled.cancel_token.cancel();
    let error = tool
        .execute(&cancelled, json!({"duration_seconds": 60, "reason": "backoff"}))
        .await
        .expect_err("cancelled Sleep completed");
    assert!(matches!(error, ToolError::Cancelled));
}

#[tokio::test]
async fn sleep_does_not_consume_or_wait_for_shell_admission() {
    let temp = tempfile::tempdir().expect("tempdir");
    let runtime = ShellRuntime::new(
        ShellLimits {
            max_active_commands: 1,
            ..ShellLimits::default()
        },
        PathBuf::from("unused-guardian"),
        temp.path().join("runtime"),
    );
    let held = runtime
        .acquire(
            ShellAdmissionRequest {
                owner: "held".to_owned(),
                class: ShellAdmissionClass::AgentForeground,
            },
            None,
        )
        .await;
    let context = ToolContext::new(temp.path())
        .expect("tool context")
        .with_shell_runtime(runtime);
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        SleepTool.execute(
            &context,
            json!({"duration_seconds": 1, "reason": "timer backoff"}),
        ),
    )
    .await
    .expect("Sleep waited for shell admission")
    .expect("Sleep result");
    assert!(result.content.contains("Waited 1 seconds"));
    drop(held);
}

#[test]
fn sleep_is_available_to_every_role() {
    for role in AgentRole::ALL {
        assert!(AgentProfile::for_role(role).allowed_tools.contains("Sleep"));
    }
}

#[test]
fn sleep_is_default_approved() {
    let call = AgentToolCall {
        id: "call-sleep".into(),
        name: "Sleep".into(),
        raw_arguments: r#"{"duration_seconds":1,"reason":"wait"}"#.into(),
    };
    assert!(is_default_approved_tool(&call));
}
```

In the existing `builtin_tool_names_use_model_facing_kimi_style_casing` expected vector, add `"Sleep"` in sorted order before running RED.

- [ ] **Step 2: Run RED**

```bash
cargo test --package neo-agent-core --lib -- tools::sleep::tests::sleep_validates_bounds_reason_and_cancellation --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- tools::sleep::tests::sleep_does_not_consume_or_wait_for_shell_admission --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- multi_agent::profile::tests::sleep_is_available_to_every_role --exact --nocapture --include-ignored
cargo test --package neo-agent-core --lib -- runtime::permission::tests::sleep_is_default_approved --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test tool_bash -- builtin_tool_names_use_model_facing_kimi_style_casing --exact --nocapture --include-ignored
```

Expected: FAIL because Sleep is absent.

- [ ] **Step 3: Implement the minimal tool**

```rust
#[derive(Debug, Deserialize, JsonSchema)]
struct SleepInput {
    #[schemars(range(min = 1, max = 3600))]
    duration_seconds: u64,
    #[schemars(description = "Short single-line reason for waiting (maximum 160 characters).")]
    reason: String,
}

pub struct SleepTool;

fn invalid_sleep(message: &str) -> ToolError {
    ToolError::InvalidInput {
        tool: "Sleep".to_owned(),
        message: message.to_owned(),
    }
}

impl Tool for SleepTool {
    fn name(&self) -> &str { "Sleep" }

    fn description(&self) -> &str {
        "Pause this agent without starting a shell command. Use only for a \
         genuine time-based wait. Prefer WaitDelegate for a known agent or \
         swarm, and TaskOutput with block=true for a known background task. \
         The wait is cancellable and duration_seconds must be 1..=3600."
    }

    fn input_schema(&self) -> serde_json::Value { schema::<SleepInput>() }

    fn execute<'a>(&'a self, ctx: &'a ToolContext, input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async move {
            let input: SleepInput = parse_input(self.name(), input)?;
            let reason = input.reason.trim();
            if !(1..=3600).contains(&input.duration_seconds) {
                return Err(invalid_sleep("duration_seconds must be between 1 and 3600"));
            }
            if reason.is_empty()
                || reason.contains('\r')
                || reason.contains('\n')
                || reason.chars().count() > 160
            {
                return Err(invalid_sleep(
                    "reason must be a non-empty single line of at most 160 characters",
                ));
            }
            tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => Err(ToolError::Cancelled),
                () = tokio::time::sleep(Duration::from_secs(input.duration_seconds)) => {
                    Ok(ToolResult::ok(format!(
                        "Waited {} seconds: {reason}", input.duration_seconds
                    )))
                }
            }
        })
    }
}
```

Register Sleep in the built-in registry and built-in-name set, add it to all four role sets, include it in `is_default_approved_tool`, and add `Sleep` to the existing exact built-in inventory assertion in `tool_bash.rs`. It needs `ToolAccess::none()` through the existing default-approved path and no special plan-mode branch.

- [ ] **Step 4: Update English and Chinese docs in lockstep**

Add `[runtime.shell]` tables with the six canonical keys/defaults to both config files. Update both tool references to:

- describe omitted Bash/Terminal `timeout_secs` as no timeout;
- add Sleep and its condition-aware alternatives; and
- list Sleep in the built-in tool inventory.

Update both permissions pages' default-approved list and all role tables in both agent customization pages so every role includes Sleep. Do not mention a specific programming language in timeout guidance.

- [ ] **Step 5: Run GREEN and commit Task 7**

Run all five commands from Step 2. Expected: PASS.

When safe to commit:

```bash
git add crates/neo-agent-core/src/tools/sleep.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/runtime/permission.rs crates/neo-agent-core/src/multi_agent/profile.rs crates/neo-agent-core/tests/tool_bash.rs docs/en/configuration/config-files.md docs/zh/configuration/config-files.md docs/en/configuration/permissions.md docs/zh/configuration/permissions.md docs/en/customization/agents.md docs/zh/customization/agents.md docs/en/reference/tools.md docs/zh/reference/tools.md
git commit -m "feat(tools): add cancellable agent sleep"
```

---

### Task 8: Prove Ownership Cancellation, Queue Isolation, and Final Scope

**Files:**

- Create: `crates/neo-agent/tests/shell_admission_runtime.rs`
- Modify: `crates/neo-agent/tests/tool_bash_guardian.rs`
- Modify: `crates/neo-agent/tests/tool_terminal_guardian.rs`
- Modify: `crates/neo-tui/tests/multi_agent_transcript.rs`
- Modify only when a failing assertion identifies a defect: production files from Tasks 1-7.

**Interfaces:**

- Verifies the complete spec; introduces no new production abstraction.

- [ ] **Step 1: Add only the missing cross-boundary tests**

Add these exact tests. In the new `neo-agent` integration target, build a shared capacity-one `ShellRuntime` with `env!("CARGO_BIN_EXE_neo")`, start a Terminal through the public ToolRegistry to hold the slot, and retain its returned handle as the deterministic admission fixture. Cancel the turn/Agent/Swarm while its Bash call is queued, stop the Terminal, then run a probe Bash with the same runtime. This exercises the real cross-crate ownership path without exposing scheduler internals:

```text
turn_cancellation_removes_queued_bash_without_spawning
agent_cancellation_removes_queued_child_bash_without_spawning
swarm_cancellation_removes_all_queued_child_bash_without_spawning
terminal_session_holds_background_permit_until_process_exit
explicit_timeout_excludes_time_spent_in_admission_queue
queue_metadata_never_enters_tool_result_or_replayed_model_messages
```

Reuse the existing platform-aware guardian command helpers from `tool_bash_guardian.rs` and `tool_terminal_guardian.rs`; do not introduce `sh -c`, Unix signals, or fixed path separators in the new target. For cancellation tests, record the fixture Terminal's task files before queueing and assert that cancellation creates no additional guardian status/running marker; then prove a fresh request can consume the released slot. For timeout exclusion, hold admission longer than a one-second explicit timeout, grant it, then use a fixture-controlled command and assert it receives approximately one full second after start; use Tokio paused time when the tested boundary is in-process, otherwise use a generous monotonic range only around guardian execution. Synchronize on handles/status markers rather than arbitrary sleeps.

- [ ] **Step 2: Run each exact boundary test**

```bash
cargo test --package neo-agent --test shell_admission_runtime -- turn_cancellation_removes_queued_bash_without_spawning --exact --nocapture --include-ignored
cargo test --package neo-agent --test shell_admission_runtime -- agent_cancellation_removes_queued_child_bash_without_spawning --exact --nocapture --include-ignored
cargo test --package neo-agent --test shell_admission_runtime -- swarm_cancellation_removes_all_queued_child_bash_without_spawning --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_terminal_guardian -- terminal_session_holds_background_permit_until_process_exit --exact --nocapture --include-ignored
cargo test --package neo-agent --test tool_bash_guardian -- explicit_timeout_excludes_time_spent_in_admission_queue --exact --nocapture --include-ignored
cargo test --package neo-agent-core --test session_jsonl -- queue_metadata_never_enters_tool_result_or_replayed_model_messages --exact --nocapture --include-ignored
```

Expected: PASS. A failure is fixed at the owning boundary and only its exact test is rerun before continuing.

- [ ] **Step 3: Run narrow formatting and lint checks**

Format/check only touched Rust files in the execution worktree:

```bash
rustfmt --edition 2024 --check \
  crates/neo-agent-core/src/events.rs \
  crates/neo-agent-core/src/multi_agent/profile.rs \
  crates/neo-agent-core/src/multi_agent/runtime.rs \
  crates/neo-agent-core/src/multi_agent/state.rs \
  crates/neo-agent-core/src/runtime/events.rs \
  crates/neo-agent-core/src/runtime/permission.rs \
  crates/neo-agent-core/src/runtime/tool_dispatch.rs \
  crates/neo-agent-core/src/session/event_persistence.rs \
  crates/neo-agent-core/src/tools/background_tasks.rs \
  crates/neo-agent-core/src/tools/bash.rs \
  crates/neo-agent-core/src/tools/mod.rs \
  crates/neo-agent-core/src/tools/shell_guard/client.rs \
  crates/neo-agent-core/src/tools/shell_guard/guardian.rs \
  crates/neo-agent-core/src/tools/shell_guard/mod.rs \
  crates/neo-agent-core/src/tools/shell_guard/protocol.rs \
  crates/neo-agent-core/src/tools/shell_guard/scheduler.rs \
  crates/neo-agent-core/src/tools/shell_guard/terminal_guard.rs \
  crates/neo-agent-core/src/tools/sleep.rs \
  crates/neo-agent-core/src/tools/terminal.rs \
  crates/neo-agent-core/tests/multi_agent_runtime.rs \
  crates/neo-agent-core/tests/runtime_turn.rs \
  crates/neo-agent-core/tests/session_jsonl.rs \
  crates/neo-agent-core/tests/tool_bash.rs \
  crates/neo-agent-core/tests/tool_permissions.rs \
  crates/neo-agent/src/config/loader.rs \
  crates/neo-agent/src/config/mod.rs \
  crates/neo-agent/src/config/types.rs \
  crates/neo-agent/src/modes/interactive/mod.rs \
  crates/neo-agent/src/modes/interactive/shell_command.rs \
  crates/neo-agent/src/modes/interactive/tests.rs \
  crates/neo-agent/src/modes/task_browser.rs \
  crates/neo-agent/tests/process_guard.rs \
  crates/neo-agent/tests/process_guard_windows.rs \
  crates/neo-agent/tests/shell_admission_runtime.rs \
  crates/neo-agent/tests/tool_bash_guardian.rs \
  crates/neo-agent/tests/tool_terminal_guardian.rs \
  crates/neo-tui/src/shell/stream.rs \
  crates/neo-tui/src/transcript/child_activity.rs \
  crates/neo-tui/src/transcript/entry/mod.rs \
  crates/neo-tui/src/transcript/event_handler.rs \
  crates/neo-tui/src/transcript/pane.rs \
  crates/neo-tui/src/transcript/presentation.rs \
  crates/neo-tui/src/transcript/shell_run.rs \
  crates/neo-tui/src/transcript/store.rs \
  crates/neo-tui/src/transcript/swarm_card.rs \
  crates/neo-tui/src/transcript/tool_call.rs \
  crates/neo-tui/src/transcript/tool_group.rs \
  crates/neo-tui/src/transcript/tool_renderers.rs \
  crates/neo-tui/tests/multi_agent_transcript.rs \
  crates/neo-tui/tests/shell_events.rs \
  crates/neo-tui/tests/tool_cards.rs
cargo clippy --package neo-agent-core --lib -- -D clippy::all
cargo clippy --package neo-agent --bin neo -- -D clippy::all
cargo clippy --package neo-tui --lib -- -D clippy::all
```

Expected: exit 0. If unrelated dirty code causes a target lint failure, record the exact diagnostic and do not edit or revert that code.

- [ ] **Step 4: Prove canonical deletion and clean patch shape**

Run literal searches excluding the superseded historical design/plan documents:

```bash
rg -n "foreground_timeout_secs|background_timeout_secs|SetBackgroundDeadline|ResourceLimitCause::ActiveCommands" crates docs/en docs/zh
rg -n "max_parallelism|max_descendant_processes|max_tree_memory_percent" crates docs/en docs/zh
rg -n "struct BashInput|timeout_secs" crates/neo-agent-core/src/tools/bash.rs crates/neo-agent-core/tests/tool_bash.rs
git diff --check
git status --short
```

Expected: removed names appear only in negative rejection/deletion tests, never in production fields, codecs, handlers, or user docs. Historical documents under `docs/aegis` may retain them because the new spec explicitly supersedes those sections. The Bash inspection shows `timeout_secs` as its only schema field. `git diff --check` exits 0. Status contains no unplanned new file.

- [ ] **Step 5: Review spec coverage explicitly**

Before handoff, map each acceptance criterion in `docs/aegis/specs/2026-07-18-shell-admission-scheduler-design.md` to one passing exact test or one inspected deletion/config assertion. Confirm especially:

- no queue handle/result reaches the model;
- position rank is class-local round-robin order;
- user priority does not preempt running work;
- detached local shell retains User classification;
- queue updates are live-only in both direct and compact delegate persistence;
- queued replay finalizes through the existing interrupted-live mechanism; and
- Sleep is not suggested for resource-limit failures.

- [ ] **Step 6: Request focused review and commit final test fixes when safe**

Use `aegis:requesting-code-review` with the spec, this plan, the pre-work HEAD, the scoped diff, and the exact verification output. Fix every Critical/Important finding and rerun its one covering exact test.

If Task 8 changed files and the execution worktree is safe to commit:

```bash
git add crates/neo-agent/tests/shell_admission_runtime.rs crates/neo-agent-core/tests/session_jsonl.rs crates/neo-agent/tests/tool_bash_guardian.rs crates/neo-agent/tests/tool_terminal_guardian.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "test(shell): cover queued admission ownership"
```

Otherwise report that Git mutation was intentionally skipped to protect overlapping dirty work.
