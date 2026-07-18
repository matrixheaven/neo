# Interruptible MCP Startup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Neo's composer responsive during fresh/resumed-session MCP startup and let Esc cancel unfinished MCP connections without disconnecting servers that already connected.

**Architecture:** Reuse `McpConnectionManager`'s existing background tasks and snapshot polling. Add one terminal `Cancelled` status to the current manager/transcript models, move startup polling into the existing terminal loop, and route Esc to MCP cancellation only after active turn and shell interruption paths decline it.

**Tech Stack:** Rust 2024, Tokio tasks, cancellation through existing `JoinHandle` retirement, ratatui transcript state, exact Cargo binary/library tests.

## Global Constraints

- No new dependency, event channel, compatibility path, or duplicate MCP state model.
- Fresh sessions and command-line resume must use the same lifecycle implementation.
- Composer editing and Enter submission remain available during MCP startup.
- Esc interrupts an active turn or shell first; only idle Esc cancels MCP startup.
- Cancellation affects only `Pending` and `Reconnecting` servers; connected servers remain usable.
- All code must remain portable across Windows, Linux, and macOS.

---

### Task 1: Cancel Pending Manager Connections

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Test: `crates/neo-agent-core/src/tools/mcp_manager.rs`

**Interfaces:**
- Produces: `McpServerStatus::Cancelled`
- Produces: `McpConnectionManager::cancel_startup(&self) -> impl Future<Output = ()>`
- Preserves: connected entry clients, tools, resources, and status

- [ ] **Step 1: Write the failing manager test**

Add a Tokio unit test beside the existing manager lifecycle tests. Insert one connected entry and one pending entry whose `connect_task` never completes, call the wished-for API, then assert only the pending entry becomes cancelled:

```rust
#[tokio::test]
async fn cancel_startup_preserves_connected_servers() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    insert_entry(
        &manager,
        entry_for_status(McpServerStatus::Connected),
    )
    .await;

    let mut pending = entry_for_status(McpServerStatus::Pending);
    pending.config.id = "pending-server".to_owned();
    pending.client = None;
    pending.tools.clear();
    pending.resources.clear();
    pending.connect_task = Some(ManagedConnectTask {
        attempt_id: pending.attempt_id,
        expected_status: McpServerStatus::Pending,
        cleanup_handle: None,
        handle: tokio::spawn(std::future::pending()),
    });
    insert_entry(&manager, pending).await;

    manager.cancel_startup().await;

    assert_eq!(
        manager.snapshot("auth-server").await.unwrap().status,
        McpServerStatus::Connected
    );
    assert_eq!(
        manager.snapshot("pending-server").await.unwrap().status,
        McpServerStatus::Cancelled
    );
    assert!(manager.get_client("auth-server").await.is_ok());
}
```

- [ ] **Step 2: Run the exact test and verify RED**

Run:

```bash
rtk cargo test --package neo-agent-core --lib tools::mcp_manager::tests::cancel_startup_preserves_connected_servers -- --exact --nocapture
```

Expected: compilation fails because `Cancelled` and `cancel_startup` do not exist.

- [ ] **Step 3: Implement the minimal manager cancellation API**

Extend the current enum and stable string mapping:

```rust
pub enum McpServerStatus {
    Disabled,
    Pending,
    Connected,
    NeedsAuth,
    Failed,
    Reconnecting,
    Cancelled,
}

Self::Cancelled => "cancelled",
```

Add one method next to `shutdown` that retires only startup tasks:

```rust
pub async fn cancel_startup(&self) {
    let mut state = self.inner.write().await;
    let mut retirement = ConnectionRetirement::default();
    for entry in state.entries.values_mut().filter(|entry| {
        matches!(
            entry.status,
            McpServerStatus::Pending | McpServerStatus::Reconnecting
        )
    }) {
        retirement.collect_tasks(entry);
        entry.status = McpServerStatus::Cancelled;
        entry.error = None;
        entry.next_retry_ms = None;
    }
    let supervisor = state.supervisor.clone();
    drop(state);
    retirement.retire(&supervisor).await;
}
```

Update exhaustive manager/status matches so `Cancelled` is settled and unavailable, without treating it as connected or failed.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the Step 2 command. Expected: one test passes.

- [ ] **Step 5: Commit the manager behavior**

```bash
rtk git add crates/neo-agent-core/src/tools/mcp_manager.rs
rtk git commit -m "feat(mcp): cancel pending startup connections"
```

### Task 2: Render Cancelled Startup Rows

**Files:**
- Modify: `crates/neo-tui/src/transcript/entry/mod.rs`
- Modify: `crates/neo-tui/src/transcript/entry/render_mcp_startup.rs`
- Modify: `crates/neo-agent/src/mcp_ops.rs`
- Test: `crates/neo-tui/tests/transcript_pane.rs`

**Interfaces:**
- Consumes: `McpServerStatus::Cancelled`
- Produces: `McpStartupPhase::Cancelled`
- Produces copy: `MCP server "<id>" startup interrupted (<transport>)`

- [ ] **Step 1: Write the failing transcript test**

```rust
#[test]
fn mcp_startup_status_updates_pending_spinner_to_interrupted_row() {
    let mut transcript_pane = TranscriptPane::new(100, 12);
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Connecting,
    });
    transcript_pane.upsert_mcp_startup_status(McpStartupStatusData {
        id: "linear".to_owned(),
        transport: "http".to_owned(),
        phase: McpStartupPhase::Cancelled,
    });

    let rendered = plain_frame(&mut transcript_pane, 100, 12).join("\n");
    assert!(rendered.contains("MCP server \"linear\" startup interrupted (http)"));
    assert!(!rendered.contains("connecting..."));
}
```

- [ ] **Step 2: Run the exact test and verify RED**

```bash
rtk cargo test --package neo-tui --test transcript_pane mcp_startup_status_updates_pending_spinner_to_interrupted_row -- --exact --nocapture
```

Expected: compilation fails because `McpStartupPhase::Cancelled` does not exist.

- [ ] **Step 3: Implement the existing-model extension**

Add `Cancelled` to `McpStartupPhase`; map it in `McpStartupStatusData::message`, finalization, terminal-exit interruption, and renderer. Use `theme.status_warn`, no spinner, and no failure prefix:

```rust
McpStartupPhase::Cancelled => format!(
    "MCP server \"{}\" startup interrupted ({})",
    self.id, self.transport
),
```

Map manager status in `mcp_ops.rs`:

```rust
McpServerStatus::Cancelled => McpStartupPhase::Cancelled,
```

Treat cancelled discovery as `NotRequested`, and format CLI startup output as interrupted rather than failed.

- [ ] **Step 4: Run the exact test and verify GREEN**

Run the Step 2 command. Expected: one test passes.

- [ ] **Step 5: Commit the status presentation**

```bash
rtk git add crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/entry/render_mcp_startup.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-agent/src/mcp_ops.rs
rtk git commit -m "feat(tui): render interrupted MCP startup"
```

### Task 3: Move Startup Polling Into the Interactive Loop

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/input.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

**Interfaces:**
- Consumes: `McpConnectionManager::cancel_startup`
- Produces controller state: `mcp_startup_active: bool`
- Produces methods: `poll_mcp_startup(&mut self) -> bool`, `cancel_mcp_startup(&mut self) -> bool`

- [ ] **Step 1: Replace the blocking-startup test with a failing lifecycle regression**

Configure a deliberately missing stdio executable. Its existing reconnect backoff keeps startup active without shell commands, ports, or platform assumptions. Drive insert/backspace/Esc through the shared event source and use a short watchdog:

```rust
#[tokio::test]
async fn startup_mcp_keeps_composer_responsive_and_escape_interrupts() {
    struct ScriptedTerminalEvents(VecDeque<InputEvent>);

    impl TerminalEvents for ScriptedTerminalEvents {
        fn next_input_event(&mut self) -> Result<InputEvent> {
            self.0.pop_front().context("expected scripted input")
        }

        fn poll_input_event(&mut self, _timeout: Duration) -> Result<Option<InputEvent>> {
            Ok(self.0.pop_front())
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = test_config(temp.path(), temp.path().join(".neo/sessions"));
    config.mcp.servers.push(crate::config::McpServerConfig {
        id: "slow".to_owned(),
        enabled: true,
        transport: crate::config::McpTransport::Stdio,
        command: Some("neo-missing-mcp-server-for-test".into()),
        url: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        headers: BTreeMap::new(),
        cwd: None,
        enabled_tools: Vec::new(),
        disabled_tools: Vec::new(),
        startup_timeout_ms: Some(5_000),
        tool_timeout_ms: None,
    });
    let mut controller = InteractiveController::new_for_test(
        "neo",
        "test-session",
        "openai/gpt-4.1",
        temp.path(),
        |_request| async move { Ok(Vec::<AgentEvent>::new()) },
    );
    controller.local_config = Some(config.clone());
    let saw_text = Rc::new(Cell::new(false));
    let saw_text_on_render = Rc::clone(&saw_text);

    tokio::time::timeout(
        Duration::from_secs(1),
        run_tty_lifecycle_with_event_factory(
            &mut controller,
            &config,
            &StartupAction::None,
            |_keybindings| ScriptedTerminalEvents(VecDeque::from([
                InputEvent::Insert('x'),
                InputEvent::Backspace,
                InputEvent::Cancel,
                InputEvent::Interrupt,
                InputEvent::Interrupt,
            ])),
            move |tui, _| {
                saw_text_on_render.set(
                    saw_text_on_render.get() || tui.chrome().prompt().text == "x",
                );
                Ok(None)
            },
            || Ok(()),
        ),
    )
    .await
    .expect("MCP startup must not block terminal input")
    .expect("terminal lifecycle succeeds");

    assert!(saw_text.get());
    let snapshot = controller.mcp_manager.as_ref().unwrap().snapshot("slow").await.unwrap();
    assert_eq!(snapshot.status, McpServerStatus::Cancelled);
}
```

- [ ] **Step 2: Run the exact test and verify RED**

```bash
rtk cargo test --package neo-agent --bin neo modes::interactive::tests::startup_mcp_keeps_composer_responsive_and_escape_interrupts -- --exact --nocapture
```

Expected: watchdog timeout because the old lifecycle waits for MCP settlement before polling input.

- [ ] **Step 3: Implement non-blocking startup in the existing loop**

Add `mcp_startup_active: bool` to `InteractiveController` and initialize it to `false`. Simplify `connect_mcp_at_startup` so it inserts connecting rows, calls `reload_mcp_manager_from_config`, sets the flag when enabled servers exist, records configuration errors, and returns without polling.

Add one poll method using the existing snapshot mapping:

```rust
async fn poll_mcp_startup(&mut self) -> bool {
    if !self.mcp_startup_active {
        return false;
    }
    let (Some(config), Some(manager)) = (self.local_config.clone(), self.mcp_manager.clone())
    else {
        self.mcp_startup_active = false;
        return false;
    };
    let snapshots = manager.snapshots().await;
    let settled = snapshots.iter().all(|snapshot| {
        !matches!(
            snapshot.status,
            McpServerStatus::Pending | McpServerStatus::Reconnecting
        )
    });
    let mut changed = false;
    for snapshot in snapshots.iter().filter(|snapshot| {
        config
            .mcp
            .servers
            .iter()
            .any(|server| server.enabled && server.id == snapshot.id)
    }) {
        changed |= self.transcript_mut().upsert_mcp_startup_status(
            mcp_ops::mcp_startup_status_from_snapshot(snapshot),
        );
    }
    if settled {
        self.mcp_startup_active = false;
    }
    changed
}
```

Call it beside `poll_pending_mcp_probe()` in `run_terminal_loop_with_suspend`. Remove the old pre-loop polling/render loop rather than retaining two paths.

Add one cancellation method and call it after the existing active-turn/shell/overlay interrupt paths:

```rust
async fn cancel_mcp_startup(&mut self) -> bool {
    if !self.mcp_startup_active {
        return false;
    }
    if let Some(manager) = self.mcp_manager.clone() {
        manager.cancel_startup().await;
    }
    let _ = self.poll_mcp_startup().await;
    self.show_notice("MCP startup interrupted");
    true
}
```

`poll_mcp_startup()` observes the now-settled cancelled snapshots, updates every pending row,
and clears the active flag. In `handle_cancel_input` and `handle_interrupt_input`, return after
`interrupt_active_or_stale_turn()` succeeds; otherwise call `cancel_mcp_startup()` before idle
clear/exit handling.

- [ ] **Step 4: Run the exact lifecycle test and verify GREEN**

Run the Step 2 command. Expected: one test passes within the watchdog.

- [ ] **Step 5: Run the existing exact active-turn priority test**

```bash
rtk cargo test --package neo-agent --bin neo modes::interactive::tests::event_loop_escape_cancels_active_turn -- --exact --nocapture
```

Expected: one test passes, proving the existing turn-first path remains intact.

- [ ] **Step 6: Format and inspect the scoped diff**

```bash
rtk cargo fmt --all --check
rtk git diff --check
rtk git diff -- crates/neo-agent-core/src/tools/mcp_manager.rs crates/neo-tui/src/transcript/entry/mod.rs crates/neo-tui/src/transcript/entry/render_mcp_startup.rs crates/neo-tui/tests/transcript_pane.rs crates/neo-agent/src/mcp_ops.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
```

Expected: formatting and whitespace checks pass; diff contains only the planned MCP startup changes plus any pre-existing user edits already present in those files.

- [ ] **Step 7: Commit the interactive behavior**

```bash
rtk git add crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/input.rs crates/neo-agent/src/modes/interactive/tests.rs
rtk git commit -m "fix(tui): keep MCP startup interruptible"
```

### Task 4: Show the MCP Interrupt Hint in the Footer

**Files:**
- Modify: `crates/neo-tui/src/shell/state.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/tests.rs`
- Test: `crates/neo-tui/tests/app_shell.rs`

**Interfaces:**
- Produces: `NeoChromeState::set_mcp_startup_active(bool)`
- Produces: `NeoChromeState::mcp_startup_active() -> bool`
- Produces footer copy: `MCP connecting · esc to interrupt`
- Removes: duplicate `InteractiveController::mcp_startup_active`

- [ ] **Step 1: Write the failing footer test**

```rust
#[test]
fn app_shell_mcp_startup_shows_interrupt_hint() {
    let mut app = NeoChromeState::new("neo", "session-a", "openai/gpt-4.1", "/tmp/neo-ws");
    app.set_mcp_startup_active(true);

    assert_eq!(
        app.working_label().as_deref(),
        Some("MCP connecting · esc to interrupt")
    );
    assert!(render_app(100, &app).iter().any(|line| {
        line.contains("MCP connecting · esc to interrupt")
    }));
}
```

- [ ] **Step 2: Run the exact test and verify RED**

```bash
rtk cargo test --package neo-tui --test app_shell app_shell_mcp_startup_shows_interrupt_hint -- --exact --nocapture
```

Expected: compilation fails because `set_mcp_startup_active` does not exist.

- [ ] **Step 3: Move the single lifecycle flag into chrome state**

Add `mcp_startup_active: bool` to `NeoChromeState`, initialize it to `false`, and add the getter/setter. Extend the existing `working_label()` fallthrough after shell and streaming labels:

```rust
if self.mcp_startup_active {
    return Some("MCP connecting · esc to interrupt".to_owned());
}
```

Replace every `InteractiveController::mcp_startup_active` read/write with
`self.tui.chrome().mcp_startup_active()` or
`self.tui.chrome_mut().set_mcp_startup_active(...)`, then delete the controller field.

- [ ] **Step 4: Extend the lifecycle test before implementation**

In `startup_mcp_keeps_composer_responsive_and_escape_interrupts`, record whether the render
closure sees `working_label() == Some("MCP connecting · esc to interrupt")`, and assert it after
the loop. Run the exact lifecycle command from Task 3; expected RED before the controller wiring
and GREEN afterward.

- [ ] **Step 5: Run exact footer and lifecycle tests**

```bash
rtk cargo test --package neo-tui --test app_shell app_shell_mcp_startup_shows_interrupt_hint -- --exact --nocapture
rtk cargo test --package neo-agent --bin neo modes::interactive::tests::startup_mcp_keeps_composer_responsive_and_escape_interrupts -- --exact --nocapture
```

Expected: both tests pass.

- [ ] **Step 6: Commit the hint**

```bash
rtk git add crates/neo-tui/src/shell/state.rs crates/neo-tui/tests/app_shell.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/tests.rs
rtk git commit -m "fix(tui): show MCP startup interrupt hint"
```
