# Neo Daily Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix all 20 high-confidence findings from the 2026-07-10 read-only audit with one authoritative implementation per boundary and focused cross-platform regression coverage.

**Architecture:** Normalize provider failures at one HTTP boundary, separate reloadable configuration from live runtime services, make MCP and terminal resources generation/lifecycle owned, and replace snapshot-heavy or lossy streaming paths with ordered incremental state. Consolidate path identity, config persistence, image normalization, and diagnostics rather than adding compatibility branches.

**Tech Stack:** Rust 2024, Tokio, reqwest, rmcp, portable-pty, crossterm, serde/JSONL, cargo-nextest, Windows Job Objects, Unix process groups.

---

## Design And Policy Notes

- Source design: `docs/superpowers/specs/2026-07-10-daily-audit-remediation-design.md`.
- Do not modify `.references/`; reference code is evidence only.
- Do not retain old helpers, fallback data paths, or dual event representations after migration.
- Subagents must not run any Git mutation. Root may run the recorded checkpoint commands only after explicit authorization for that command.
- Do not revert or overwrite the pre-existing Cargo manifest/lock changes or `docs/superpowers/specs/2026-07-08-google-cached-content-design.md`.
- Every task uses TDD: add one behavior test, run it and observe the expected failure, implement the minimum repair, rerun the same narrow test, then refactor.
- Verification evidence must name one package, one target selector, and at least one test filter. Do not run broad workspace/package tests.

## Finding Coverage

| Audit finding | Plan task |
|---|---|
| 1. Google API key in URL | Task 1 |
| 2. Responses/Google drop error bodies | Task 1 |
| 3. `StartGoal(replace=false)` replaces active goal | Task 2 |
| 4. Config refresh loses live runtimes | Task 3 |
| 5. MCP lifecycle mutex held across request await | Task 4 |
| 6. MCP reconnect task lacks generation | Task 5 |
| 7. Removed/reconfigured stdio MCP skips cleanup | Task 5 |
| 8. Startup trust creates a second stdin reader | Task 6 |
| 9. TUI state transition/render writes are non-transactional | Task 7 |
| 10. Terminal write blocks under global registry lock | Task 8 |
| 11. PTY output corrupts split UTF-8 | Task 9 |
| 12. Terminal stop leaves descendants | Task 10 |
| 13. Cross-workspace resume command is invalid | Task 6 |
| 14. Kitty labels JPEG/GIF/WebP as PNG | Task 11 |
| 15. Workspace key hashes lossy paths | Task 12 |
| 16. Swarm emits/clones full snapshots per delta | Task 13 |
| 17. MCP stderr diagnostics are disconnected | Task 14 |
| 18. Config writes are unlocked/non-atomic | Task 15 |
| 19. TUI debug mode creates colliding per-frame files | Task 16 |
| 20. Core `eprintln!` bypasses TUI logging | Task 16 |

## File Map

- `crates/neo-ai/src/providers/common/http.rs`: one bounded HTTP error-response consumer.
- `crates/neo-ai/src/providers/google.rs`: Google credential header and shared error mapping.
- `crates/neo-ai/src/providers/openai/responses.rs`: shared error mapping.
- `crates/neo-agent-core/src/goal/mod.rs`, `tools/goal.rs`: non-overlapping goal start/replace contracts.
- `crates/neo-agent/src/config/loader.rs`, `modes/interactive/mod.rs`: preserve live session handles during disk reload.
- `crates/neo-agent-core/src/tools/mcp/client.rs`: split request peer from shutdown ownership.
- `crates/neo-agent-core/src/tools/mcp_manager.rs`, `tools/process_supervisor.rs`: generation-owned tasks and awaited cleanup.
- `crates/neo-agent/src/modes/interactive/terminal_io.rs`, `interactive/mod.rs`, `interactive/sessions.rs`: one stdin owner and product-owned resume.
- `crates/neo-tui/src/screen_output/frame_differ.rs`: transactional terminal setup/rendering.
- `crates/neo-agent-core/src/tools/terminal.rs`: per-session I/O, streaming decoder, portable process-tree ownership.
- `crates/neo-tui/src/terminal_image/mod.rs`: normalized Kitty payload format.
- `crates/neo-agent-core/src/session/workspace.rs`: OS-native path identity helper.
- `crates/neo-agent/src/path_key.rs`: consume the shared path identity helper.
- `crates/neo-agent-core/src/multi_agent/runtime.rs`, `tools/delegate.rs`, `tools/background_tasks.rs`: bounded delta updates and ordered snapshots.
- `crates/neo-agent-core/src/tools/mcp/stdio.rs`, `tools/mcp/client.rs`, `tools/mcp_manager.rs`: bounded stderr tail.
- `crates/neo-agent/src/config/loader.rs`, `config/mutations.rs`: one locked atomic config update path.
- `crates/neo-tui/src/screen_output/debug_log.rs`, `neo-agent-core` diagnostics sites: bounded logs and tracing/typed diagnostics.

## Task 1: Secure Google Authentication And Normalize HTTP Error Bodies

**Files:**
- Modify: `crates/neo-ai/src/providers/common/http.rs`
- Modify: `crates/neo-ai/src/providers/google.rs`
- Modify: `crates/neo-ai/src/providers/openai/responses.rs`
- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`
- Modify: `crates/neo-ai/src/providers/anthropic.rs`
- Test: `crates/neo-ai/tests/real_provider_adapters.rs`

- [ ] **Step 1: Add a failing provider integration test**

Add a local HTTP listener test that records the request target and headers, then returns `413` with `{"error":{"message":"context_length exceeded"}}`:

```rust
#[tokio::test]
async fn google_uses_header_auth_and_maps_bounded_error_body() {
    let server = RecordedHttpServer::respond(
        413,
        r#"{"error":{"message":"context_length exceeded"}}"#,
    )
    .await;
    let client = GoogleGenerativeAiClient::new(server.base_url(), "secret-key", reqwest::Client::new());

    let error = first_error(client.stream_chat(minimal_request())).await;
    let request = server.single_request().await;

    assert_eq!(request.header("x-goog-api-key"), Some("secret-key"));
    assert!(!request.target.contains("secret-key"));
    assert_eq!(error.code(), "provider.context_overflow");
}
```

- [ ] **Step 2: Run the test and verify RED**

Run: `cargo nextest run -p neo-ai --test real_provider_adapters google_uses_header_auth_and_maps_bounded_error_body`

Expected: FAIL because the request target contains `key=secret-key`, lacks `x-goog-api-key`, and the error code is `provider.stream_error`.

- [ ] **Step 3: Add the single bounded error-response helper**

Implement in `providers/common/http.rs` and migrate all four adapters to it:

```rust
pub(crate) const ERROR_BODY_LIMIT: usize = 64 * 1024;

pub(crate) async fn into_http_status_error(mut response: reqwest::Response) -> ProviderError {
    let status = response.status().as_u16();
    let retry_after = response.headers().get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after);
    let mut bytes = Vec::new();
    while bytes.len() < ERROR_BODY_LIMIT {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = ERROR_BODY_LIMIT - bytes.len();
                bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let body = (!bytes.is_empty()).then(|| error_body_excerpt(&String::from_utf8_lossy(&bytes)));
    ProviderError::HttpStatus { status, body, retry_after }
}
```

Change Google URL construction to append only `alt=sse`; build headers by injecting extras first, then inserting a sensitive `x-goog-api-key` value so extras cannot override it.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-ai --test real_provider_adapters google_uses_header_auth_and_maps_bounded_error_body`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-ai/src/providers crates/neo-ai/tests/real_provider_adapters.rs
git commit -m "fix(ai): secure provider auth and preserve error bodies"
```

## Task 2: Enforce Goal Start Versus Replace Contracts

**Files:**
- Modify: `crates/neo-agent-core/src/goal/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/goal.rs`
- Test: `crates/neo-agent-core/tests/goals.rs`

- [ ] **Step 1: Add a restart-safe failing goal test**

```rust
#[tokio::test]
async fn start_rejects_an_active_goal_without_replacing_durable_state() {
    let dir = tempfile::tempdir().unwrap();
    let manager = GoalManager::load(dir.path().to_path_buf()).await.unwrap();
    let first = Goal::new("first");
    let first_id = first.id.clone();
    manager.start(first).await.unwrap();

    let error = manager.start(Goal::new("second")).await.unwrap_err();
    assert!(error.to_string().contains("active goal"));
    assert_eq!(manager.active().unwrap().id, first_id);

    let reloaded = GoalManager::load(dir.path().to_path_buf()).await.unwrap();
    assert_eq!(reloaded.active().unwrap().id, first_id);
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --test goals start_rejects_an_active_goal_without_replacing_durable_state`

Expected: FAIL because `start` succeeds and installs the second goal.

- [ ] **Step 3: Implement one start path and one replace path**

Change `GoalManager::start` to reject when `store.active().is_some()` before artifact creation. Keep deletion of the previous durable goal exclusively in `GoalManager::replace`. Update `StartGoalTool` so `replace=false` reports the domain error and never interprets `Some(previous)` as success.

```rust
pub async fn start(&self, mut goal: Goal) -> Result<()> {
    {
        let store = self.store.lock().map_err(|_| GoalError::Lock)?;
        if let Some(active) = store.active() {
            return Err(anyhow::anyhow!("active goal '{}' already exists", active.id));
        }
    }
    ensure_goal_artifacts(&self.session_dir, &mut goal).await?;
    self.store.lock().map_err(|_| GoalError::Lock)?.start(goal.clone());
    save_goal(&self.session_dir, &goal).await
}
```

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --test goals start_rejects_an_active_goal_without_replacing_durable_state`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/goal/mod.rs crates/neo-agent-core/src/tools/goal.rs crates/neo-agent-core/tests/goals.rs
git commit -m "fix(goal): enforce explicit goal replacement"
```

## Task 3: Preserve Live Runtime Handles Across Config Reload

**Files:**
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`

- [ ] **Step 1: Add a failing interactive reload test**

Create a background task and a delegate snapshot, rewrite an unrelated provider field, call `refresh_config`, and assert both handles are identical and their records remain visible:

```rust
#[tokio::test]
async fn refresh_config_preserves_live_task_and_multi_agent_state() {
    let mut controller = controller_with_file_config().await;
    let before = controller.local_config().unwrap().clone();
    before.background_tasks.start_question("q1".into(), "question".into()).await;
    before.multi_agent.register_agent(test_agent_snapshot("agent-1"));

    rewrite_default_model(controller.config_path().unwrap(), "other");
    controller.refresh_config();
    let after = controller.local_config().unwrap();

    assert!(after.background_tasks.snapshot("q1").await.is_ok());
    assert!(after.multi_agent.agent_snapshot("agent-1").is_some());
    assert!(Arc::ptr_eq(&before.live_permission_mode, &after.live_permission_mode));
    assert!(Arc::ptr_eq(&before.workspace_policy, &after.workspace_policy));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test --package neo-agent --bin neo -- modes::interactive::tests::refresh_config_preserves_live_task_and_multi_agent_state --exact --nocapture --include-ignored`

Expected: FAIL because the reloaded config contains empty managers and new `Arc` state.

- [ ] **Step 3: Reattach one canonical live-state bundle**

Add an internal method that moves only non-serializable live state from the current config into the newly loaded config:

```rust
impl AppConfig {
    pub(crate) fn inherit_live_state(&mut self, current: &Self) {
        self.background_tasks = current.background_tasks.clone();
        self.multi_agent = current.multi_agent.clone();
        self.live_permission_mode = Arc::clone(&current.live_permission_mode);
        self.workspace_policy = Arc::clone(&current.workspace_policy);
        self.permission_mode = current.permission_mode;
    }
}
```

In `refresh_config`, load disk state, call `inherit_live_state(current)`, then replace `local_config`. Remove hard-coded session override resets from this reload path.

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --package neo-agent --bin neo -- modes::interactive::tests::refresh_config_preserves_live_task_and_multi_agent_state --exact --nocapture --include-ignored`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent/src/config crates/neo-agent/src/modes/interactive
git commit -m "fix(config): preserve live state across reload"
```

## Task 4: Release MCP Lifecycle Locks Before Requests

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp/client.rs`
- Test: `crates/neo-agent-core/src/tools/mcp/client.rs`

- [ ] **Step 1: Add a failing concurrency test**

Build an rmcp service whose request remains pending, begin `call_tool`, then call `shutdown` and require shutdown to acquire ownership and finish within the configured timeout:

```rust
#[tokio::test]
async fn pending_request_does_not_hold_shutdown_ownership_lock() {
    let (client, request_seen) = hanging_test_client(Duration::from_millis(50)).await;
    let call = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.call_tool("hang", json!({})).await }
    });
    request_seen.await.unwrap();

    timeout(Duration::from_millis(200), client.shutdown()).await
        .expect("shutdown must not wait for the request mutex")
        .unwrap();
    assert!(call.await.unwrap().is_err());
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --lib pending_request_does_not_hold_shutdown_ownership_lock`

Expected: FAIL by timeout because the request holds `service` across `await`.

- [ ] **Step 3: Split peer and service ownership**

```rust
pub struct RmcpClient {
    peer: rmcp::service::Peer<RoleClient>,
    service: Mutex<Option<RunningService<RoleClient, ()>>>,
    request_timeout: Duration,
}

pub fn new(service: RunningService<RoleClient, ()>, timeout: Option<Duration>) -> Self {
    Self {
        peer: service.peer().clone(),
        service: Mutex::new(Some(service)),
        request_timeout: timeout.unwrap_or(Duration::from_secs(30)),
    }
}
```

All request methods call `self.peer.send_request(...)` without acquiring `service`; shutdown alone takes and cancels `RunningService`.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --lib pending_request_does_not_hold_shutdown_ownership_lock`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/mcp/client.rs
git commit -m "fix(mcp): separate requests from shutdown ownership"
```

## Task 5: Bind MCP Attempts To Generations And Await Cleanup

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Modify: `crates/neo-agent-core/src/tools/process_supervisor.rs`
- Modify: `crates/neo-agent-core/src/tools/mcp/stdio.rs`
- Test: `crates/neo-agent-core/src/tools/mcp_manager.rs`

- [ ] **Step 1: Add failing stale-attempt and cleanup tests**

```rust
#[tokio::test]
async fn stale_reconnect_cannot_install_into_a_new_generation() {
    let harness = ManagerHarness::new().await;
    harness.start_reconnect("server", "old-command").await;
    harness.apply_stdio_config("server", "new-command").await;
    harness.finish_old_reconnect().await;

    let snapshot = harness.manager.snapshot("server").await.unwrap();
    assert_ne!(snapshot.connected_command.as_deref(), Some("old-command"));
}

#[tokio::test]
async fn removing_stdio_server_awaits_registered_cleanup() {
    let (manager, cleanup_count) = manager_with_counted_cleanup().await;
    assert!(manager.remove_server("server").await);
    assert_eq!(cleanup_count.load(Ordering::SeqCst), 1);
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --lib stale_reconnect_cannot_install_into_a_new_generation`

Expected: FAIL because the old task is labeled with the current entry generation.

- [ ] **Step 3: Store generation with every task and add atomic cleanup APIs**

```rust
struct ManagedConnectTask {
    attempt_id: u64,
    handle: JoinHandle<Result<ConnectOutcome, McpError>>,
}

impl ProcessSupervisor {
    pub async fn remove_and_cleanup(&self, handle: &str) -> bool {
        let removed = self.processes.lock().await.remove(handle);
        if let Some(process) = removed {
            (process.cleanup)(handle.to_owned()).await;
            true
        } else {
            false
        }
    }
}
```

Capture `attempt_id` before spawning. Install a task only if the entry still has that generation and expected status. Poll using `task.attempt_id`, never `entry.attempt_id`. Refactor remove/reconfigure to collect cleanup handles under the manager lock, release it, then await cleanup.

- [ ] **Step 4: Verify both behaviors**

Run: `cargo nextest run -p neo-agent-core --lib stale_reconnect_cannot_install_into_a_new_generation`

Expected: PASS.

Run: `cargo nextest run -p neo-agent-core --lib removing_stdio_server_awaits_registered_cleanup`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/mcp_manager.rs crates/neo-agent-core/src/tools/process_supervisor.rs crates/neo-agent-core/src/tools/mcp/stdio.rs
git commit -m "fix(mcp): bind connection attempts to lifecycle generations"
```

## Task 6: Use One Stdin Reader And Product-Owned Resume Resolution

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/terminal_io.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/sessions.rs`
- Modify: `crates/neo-agent/src/modes/sessions.rs`
- Modify: `crates/neo-agent/src/main.rs`
- Test: `crates/neo-agent/src/modes/interactive/tests.rs`
- Test: `crates/neo-agent/tests/cli_commands.rs`

- [ ] **Step 1: Add failing ownership and resume tests**

```rust
#[tokio::test]
async fn startup_trust_and_main_loop_share_one_terminal_event_source() {
    let events = CountingTerminalEvents::new([trust_accept(), submit("hello")]);
    execute_tty_with_injected_events(config(), events.clone()).await.unwrap();
    assert_eq!(events.reader_count(), 1);
    assert_eq!(events.delivered_submissions(), vec!["hello"]);
}

#[tokio::test]
async fn cross_workspace_picker_emits_parseable_product_resume_command() {
    let mut controller = cross_workspace_controller();
    controller.load_selected_session().await.unwrap();
    assert_eq!(controller.last_status(), format!("neo resume {SESSION_A}"));
    assert!(Cli::try_parse_from(["neo", "resume", SESSION_A]).is_ok());
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test --package neo-agent --bin neo -- modes::interactive::tests::cross_workspace_picker_emits_parseable_product_resume_command --exact --nocapture --include-ignored`

Expected: FAIL with current `cd '...' && neo --resume '...'` text.

- [ ] **Step 3: Move stdin and workspace selection into product APIs**

Create `RawStdinEvents` once before the trust dialog and pass `&mut events` through trust and main-loop functions. Change cross-workspace picker output to `neo resume <id>`. Factor resume lookup so `Command::Resume` consults the global `SessionIndex`, returns the recorded work directory as a `PathBuf`, and calls `std::env::set_current_dir` before loading workspace-scoped configuration. Do not generate shell quoting or `cd` syntax.

```rust
let mut events = input_events(controller.keybindings.clone());
controller.resolve_trust_dialog_at_startup(data, &mut events, draw).await?;
controller.run_terminal_loop_with_suspend(draw, suspend, &mut events).await?;
```

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --package neo-agent --bin neo -- modes::interactive::tests::cross_workspace_picker_emits_parseable_product_resume_command --exact --nocapture --include-ignored`

Expected: PASS.

Run: `cargo nextest run -p neo-agent --test cli_commands resume_specific_session_uses_indexed_workspace`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent/src/modes/interactive crates/neo-agent/src/modes/sessions.rs crates/neo-agent/src/main.rs crates/neo-agent/tests/cli_commands.rs
git commit -m "fix(tui): unify stdin ownership and resume routing"
```

## Task 7: Make Terminal Setup And Frame Rendering Transactional

**Files:**
- Modify: `crates/neo-tui/src/screen_output/frame_differ.rs`
- Test: `crates/neo-tui/src/screen_output/frame_differ.rs`

- [ ] **Step 1: Add a writer-failure regression test**

```rust
#[test]
fn render_write_failure_does_not_commit_frame_state() {
    let mut renderer = test_renderer(vec!["old".to_owned()]);
    let before = renderer.snapshot_state_for_test();
    let mut output = FailingWriter::after_bytes(2);

    let error = renderer.render_to_with_size(&mut output, 80, 24, vec!["new".to_owned()], None)
        .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert_eq!(renderer.snapshot_state_for_test(), before);
}
```

Use a test-local writer; do not add test-only production APIs solely for inspection—assert existing fields from the module test.

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-tui --lib render_write_failure_does_not_commit_frame_state`

Expected: FAIL because render returns `Ok` and mutates cached lines/cursor state.

- [ ] **Step 3: Propagate I/O and guard terminal setup**

Change `finish_diff_render`, `render_deleted_tail`, `full_render`, and `position_hardware_cursor` to return `io::Result`. Use `?` for every write/flush in normal rendering. Compute next renderer state locally and assign it only after output succeeds. Introduce a private setup guard that disables raw mode and restores the Windows input mode if `enter` or `suspend_resume` exits early.

```rust
output.write_all(buffer.as_bytes())?;
output.flush()?;
self.previous_lines = new_lines;
self.hardware_cursor_row = next_cursor_row;
Ok(())
```

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-tui --lib render_write_failure_does_not_commit_frame_state`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-tui/src/screen_output/frame_differ.rs
git commit -m "fix(tui): make terminal rendering transactional"
```

## Task 8: Remove Blocking PTY Writes From The Global Registry Lock

**Files:**
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Test: `crates/neo-agent-core/tests/tool_terminal.rs`

- [ ] **Step 1: Add a failing cross-session concurrency test**

```rust
#[tokio::test]
async fn blocked_write_in_one_terminal_does_not_block_other_handles() {
    let blocked = start_terminal_with_blocking_writer().await;
    let healthy = start_echo_terminal().await;
    let write = tokio::spawn(write_large_input(blocked.handle.clone()));

    timeout(Duration::from_millis(250), read_terminal(healthy.handle.clone()))
        .await
        .expect("another handle must remain available")
        .unwrap();
    write.abort();
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --test tool_terminal blocked_write_in_one_terminal_does_not_block_other_handles`

Expected: FAIL by timeout because `TERMINALS` remains locked during synchronous `write_all`.

- [ ] **Step 3: Give every session an independently lockable writer**

Store `writer: Arc<StdMutex<Box<dyn Write + Send>>>`. Under `TERMINALS`, clone only the writer handle; release the map lock and call `task::spawn_blocking` for `write_all` plus `flush`. Stop removes the session from the map first, then owns cleanup independently.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --test tool_terminal blocked_write_in_one_terminal_does_not_block_other_handles`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/tests/tool_terminal.rs
git commit -m "fix(terminal): isolate blocking session writes"
```

## Task 9: Decode PTY Output Incrementally

**Files:**
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Test: `crates/neo-agent-core/src/tools/terminal.rs`

- [ ] **Step 1: Add failing split-sequence tests**

```rust
#[test]
fn terminal_decoder_preserves_utf8_split_across_chunks() {
    let mut decoder = TerminalUtf8Decoder::default();
    assert_eq!(decoder.push(&[0xE4, 0xBD]), "");
    assert_eq!(decoder.push(&[0xA0, b'!']), "你!");
    assert_eq!(decoder.finish(), "");
}

#[test]
fn limited_read_does_not_advance_past_incomplete_utf8() {
    let mut buffer = TerminalOutputBuffer::new(64);
    buffer.push("你".as_bytes());
    let first = buffer.read_since_limited(0, 2);
    assert_eq!(first.output, "");
    assert_eq!(first.next_offset, 0);
    assert_eq!(buffer.read_since_limited(first.next_offset, 3).output, "你");
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --lib terminal_decoder_preserves_utf8_split_across_chunks`

Expected: FAIL because each slice is decoded with `from_utf8_lossy` independently.

- [ ] **Step 3: Implement one incremental decoder contract**

Add a decoder that retains only the incomplete UTF-8 suffix, emits valid prefixes, replaces confirmed invalid sequences, and reports the number of source bytes consumed. Use it for stream callbacks and page reads; offsets advance only through consumed complete sequences. Remove direct per-chunk `from_utf8_lossy` calls.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --lib terminal_decoder_preserves_utf8_split_across_chunks`

Expected: PASS.

Run: `cargo nextest run -p neo-agent-core --lib limited_read_does_not_advance_past_incomplete_utf8`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/terminal.rs
git commit -m "fix(terminal): preserve split UTF-8 output"
```

## Task 10: Own And Terminate The PTY Process Tree

**Files:**
- Create: `crates/neo-agent-core/src/tools/terminal_process.rs`
- Modify: `crates/neo-agent-core/src/tools/mod.rs`
- Modify: `crates/neo-agent-core/src/tools/terminal.rs`
- Test: `crates/neo-agent-core/tests/tool_terminal.rs`

- [ ] **Step 1: Add a platform-gated descendant cleanup test**

```rust
#[cfg(unix)]
#[tokio::test]
async fn terminal_stop_terminates_descendant_processes() {
    let pid_file = tempfile::NamedTempFile::new().unwrap();
    let terminal = start_terminal(format!("sleep 60 & echo $! > {} ; wait", shell_quote(pid_file.path()))).await;
    let descendant = wait_for_pid(pid_file.path()).await;
    stop_terminal(terminal.handle).await.unwrap();
    assert_eventually_process_gone(descendant).await;
}

#[cfg(windows)]
#[tokio::test]
async fn terminal_stop_terminates_descendant_processes() {
    let terminal = start_terminal(windows_child_tree_fixture()).await;
    let descendant = terminal.reported_descendant_pid().await;
    stop_terminal(terminal.handle).await.unwrap();
    assert_eventually_process_gone(descendant).await;
}
```

- [ ] **Step 2: Verify RED on the host platform**

Run: `cargo nextest run -p neo-agent-core --test tool_terminal terminal_stop_terminates_descendant_processes`

Expected: FAIL because only the direct shell PID is killed.

- [ ] **Step 3: Add one portable process-tree interface**

```rust
pub(crate) trait TerminalProcessTree: Send {
    fn terminate(&mut self) -> io::Result<()>;
    fn wait(&mut self) -> io::Result<Option<i32>>;
}
```

Implement Unix ownership with a dedicated process group/session and TERM-to-KILL escalation. Implement Windows ownership with a Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. `TerminalSession` owns the platform implementation and `stop_session` calls the same `terminate`/`wait` contract. Unsupported targets use a portable direct-child implementation that returns explicit cleanup limitations, never panic/todo/silent success.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --test tool_terminal terminal_stop_terminates_descendant_processes`

Expected: PASS on the host platform; CI supplies the opposite-platform evidence.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/terminal_process.rs crates/neo-agent-core/src/tools/mod.rs crates/neo-agent-core/src/tools/terminal.rs crates/neo-agent-core/tests/tool_terminal.rs
git commit -m "fix(terminal): terminate owned process trees"
```

## Task 11: Normalize Kitty Payloads To PNG

**Files:**
- Modify: `crates/neo-tui/Cargo.toml`
- Modify: `crates/neo-tui/src/terminal_image/mod.rs`
- Modify: `crates/neo-tui/src/terminal_image/kitty.rs`
- Test: `crates/neo-tui/src/terminal_image/mod.rs`

- [ ] **Step 1: Add a failing non-PNG Kitty test**

```rust
#[test]
fn kitty_jpeg_payload_is_png_or_falls_back() {
    let jpeg = include_bytes!("../../../tests/fixtures/one-pixel.jpg");
    let rendered = render_bytes_for_kitty(jpeg, "image/jpeg");
    if let Some(sequence) = rendered.escape_sequence {
        let payload = decode_kitty_payload(&sequence);
        assert!(payload.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(sequence.contains("f=100"));
    } else {
        assert_eq!(rendered.protocol, NegotiatedImageProtocol::None);
    }
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-tui --lib kitty_jpeg_payload_is_png_or_falls_back`

Expected: FAIL because raw JPEG bytes are emitted with `f=100`.

- [ ] **Step 3: Add one normalization boundary**

Decode supported PNG/JPEG/GIF/WebP bytes once at `TerminalImageRenderer` input and encode PNG bytes for Kitty. If decoding or encoding fails, return the existing metadata fallback. Remove `kitty_format_for_mime`; Kitty accepts only normalized PNG from this caller.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-tui --lib kitty_jpeg_payload_is_png_or_falls_back`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-tui/Cargo.toml crates/neo-tui/src/terminal_image Cargo.lock
git commit -m "fix(tui): normalize kitty images to png"
```

## Task 12: Use OS-Native Path Identity Everywhere

**Files:**
- Modify: `crates/neo-agent-core/src/session/workspace.rs`
- Modify: `crates/neo-agent/src/path_key.rs`
- Test: `crates/neo-agent-core/src/session/workspace.rs`

- [ ] **Step 1: Add a failing non-UTF path identity test**

```rust
#[cfg(unix)]
#[test]
fn workspace_keys_distinguish_different_invalid_utf8_paths() {
    use std::os::unix::ffi::OsStringExt as _;
    let a = PathBuf::from(OsString::from_vec(b"/tmp/work-\xFE".to_vec()));
    let b = PathBuf::from(OsString::from_vec(b"/tmp/work-\xFF".to_vec()));
    assert_ne!(encode_workdir_key(&a), encode_workdir_key(&b));
}
```

Add a Windows unit test over an internal wide-unit hashing helper so unpaired surrogate sequences remain distinct without requiring impossible filesystem setup.

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --lib workspace_keys_distinguish_different_invalid_utf8_paths`

Expected: FAIL because both lossy strings contain the same replacement character.

- [ ] **Step 3: Move one native hash helper into core**

Expose an internal `hash_os_path_into(path, hasher)` with `OsStrExt::as_bytes` on Unix and `encode_wide` on Windows. `encode_workdir_key` and `neo-agent::path_key` both call the same core helper; delete the duplicate agent implementation.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --lib workspace_keys_distinguish_different_invalid_utf8_paths`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/session/workspace.rs crates/neo-agent/src/path_key.rs
git commit -m "fix(paths): hash workspace identity from os-native data"
```

## Task 13: Replace Per-Delta Swarm Snapshots With Ordered Deltas

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/multi_agent/runtime.rs`
- Modify: `crates/neo-agent-core/src/tools/delegate.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Modify: `crates/neo-agent/src/modes/interactive/event_handling.rs`
- Modify: `crates/neo-tui/src/transcript/child_activity.rs`
- Test: `crates/neo-agent-core/tests/multi_agent_background.rs`
- Test: `crates/neo-tui/tests/multi_agent_transcript.rs`

- [ ] **Step 1: Add a failing ordered-progress test**

```rust
#[tokio::test]
async fn swarm_text_deltas_are_bounded_and_background_updates_stay_ordered() {
    let harness = background_swarm_harness(2).await;
    harness.emit_text("agent-1", "a".repeat(20_000)).await;
    harness.emit_text("agent-1", "latest").await;
    harness.complete("agent-1").await;

    let events = harness.progress_events();
    assert!(events.iter().all(|event| event.serialized_len() < 8 * 1024));
    let snapshot = harness.background_snapshot().await;
    assert!(snapshot.agent("agent-1").latest_text.ends_with("latest"));
    assert!(snapshot.agent("agent-1").state.is_terminal());
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --test multi_agent_background swarm_text_deltas_are_bounded_and_background_updates_stay_ordered`

Expected: FAIL because each delta carries/clones a full accumulated snapshot and background updates are spawned independently.

- [ ] **Step 3: Introduce bounded delta events and one ordered updater**

Add an `AgentProgressDelta` enum carrying agent id plus bounded text/tool/lifecycle changes. `apply_child_event` mutates runtime state but emits only the delta for text/tool updates. Emit full `DelegateSwarmUpdated` snapshots at start, tool/lifecycle checkpoints, and finish. Replace `tokio::spawn` per update with one `mpsc` consumer or direct awaited update ordered by a monotonically increasing revision. Bound latest model text to a named constant and remove repeated `trim().to_owned()` of the full accumulated response.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --test multi_agent_background swarm_text_deltas_are_bounded_and_background_updates_stay_ordered`

Expected: PASS.

Run: `cargo nextest run -p neo-tui --test multi_agent_transcript swarm_progress_applies_text_delta`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/multi_agent crates/neo-agent-core/src/tools/delegate.rs crates/neo-agent-core/src/tools/background_tasks.rs crates/neo-agent/src/modes/interactive crates/neo-tui/src/transcript crates/neo-agent-core/tests/multi_agent_background.rs crates/neo-tui/tests/multi_agent_transcript.rs
git commit -m "fix(agent): stream bounded ordered swarm progress"
```

## Task 14: Attach Bounded Stdio Stderr To MCP Diagnostics

**Files:**
- Modify: `crates/neo-agent-core/src/tools/mcp/client.rs`
- Modify: `crates/neo-agent-core/src/tools/mcp/stdio.rs`
- Modify: `crates/neo-agent-core/src/tools/mcp_manager.rs`
- Test: `crates/neo-agent-core/src/tools/mcp/stdio.rs`

- [ ] **Step 1: Replace the non-behavioral command test with a failing diagnostic test**

```rust
#[tokio::test]
async fn failed_stdio_handshake_exposes_bounded_stderr_tail() {
    let server = test_stdio_server_that_writes_stderr("x".repeat(10_000));
    let error = build_stdio_client("broken", server.config(), &ProcessSupervisor::new())
        .await
        .unwrap_err();
    let tail = error.stderr_tail().expect("stderr tail");
    assert!(tail.len() <= MCP_STDERR_TAIL_CAPACITY);
    assert!(tail.ends_with('x'));
}
```

Delete `build_command_pipes_stderr`, which does not assert stderr behavior.

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-agent-core --lib failed_stdio_handshake_exposes_bounded_stderr_tail`

Expected: FAIL because stderr is discarded and `McpError` has no tail.

- [ ] **Step 3: Add a shared bounded byte tail**

Drain stderr into `Arc<Mutex<BoundedByteTail>>` with a 4 KiB cap. Attach the snapshot to handshake/timeout errors and expose it from the stdio client for unexpected-close diagnostics. Change `diagnostic_from_error` to derive `stderr_tail` from the error/client instead of accepting arbitrary `None` arguments. Decode only when rendering with `String::from_utf8_lossy` so drain remains byte-oriented.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-agent-core --lib failed_stdio_handshake_exposes_bounded_stderr_tail`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent-core/src/tools/mcp/client.rs crates/neo-agent-core/src/tools/mcp/stdio.rs crates/neo-agent-core/src/tools/mcp_manager.rs
git commit -m "fix(mcp): preserve bounded stdio diagnostics"
```

## Task 15: Serialize And Atomically Replace Config Updates

**Files:**
- Create: `crates/neo-agent/src/config/atomic_file.rs`
- Modify: `crates/neo-agent/src/config/mod.rs`
- Modify: `crates/neo-agent/src/config/loader.rs`
- Modify: `crates/neo-agent/src/config/mutations.rs`
- Test: `crates/neo-agent/src/config/mutations.rs`

- [ ] **Step 1: Add failing concurrent-update and failure-atomicity tests**

```rust
#[test]
fn concurrent_config_updates_preserve_both_mutations() {
    let file = temp_config();
    std::thread::scope(|scope| {
        scope.spawn(|| update_file_config(file.path(), |cfg| cfg.default_model = Some("a".into())).unwrap());
        scope.spawn(|| update_file_config(file.path(), |cfg| cfg.default_provider = Some("b".into())).unwrap());
    });
    let config = read_file_config(file.path()).unwrap();
    assert_eq!(config.default_model.as_deref(), Some("a"));
    assert_eq!(config.default_provider.as_deref(), Some("b"));
}

#[test]
fn failed_atomic_replace_leaves_previous_config_parseable() {
    let file = seeded_config();
    let result = update_file_config_with_writer(file.path(), |_| anyhow::bail!("injected"));
    assert!(result.is_err());
    assert!(read_file_config(file.path()).is_ok());
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo test --package neo-agent --bin neo -- config::mutations::tests::concurrent_config_updates_preserve_both_mutations --exact --nocapture --include-ignored`

Expected: FAIL through a lost update or because the centralized API does not exist.

- [ ] **Step 3: Create the only config mutation path**

Implement a process-local path-keyed mutex plus an OS advisory file lock covering the complete read-modify-write. Serialize to a unique same-directory temporary file, flush/sync it, atomically replace the destination with platform-specific implementation isolated in `atomic_file.rs`, then sync the directory where supported. Convert every mutation function to pass a closure to `update_file_config`; remove direct `read_file_config` + `write_file_config` sequences and direct `fs::write`.

```rust
pub(crate) fn update_file_config<T>(
    path: &Path,
    mutate: impl FnOnce(&mut FileConfig) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let _guard = config_lock(path)?;
    let mut config = read_file_config(path)?;
    let result = mutate(&mut config)?;
    atomic_file::write(path, toml::to_string_pretty(&config_with_default_compaction(&config))?.as_bytes())?;
    Ok(result)
}
```

- [ ] **Step 4: Verify GREEN**

Run: `cargo test --package neo-agent --bin neo -- config::mutations::tests::concurrent_config_updates_preserve_both_mutations --exact --nocapture --include-ignored`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-agent/src/config
git commit -m "fix(config): serialize atomic file updates"
```

## Task 16: Consolidate Diagnostics And Bound TUI Debug Logging

**Files:**
- Modify: `crates/neo-tui/src/screen_output/debug_log.rs`
- Modify: `crates/neo-tui/src/screen_output/frame_differ.rs`
- Modify: `crates/neo-agent-core/src/runtime/config.rs`
- Modify: `crates/neo-agent-core/src/runtime/tool_dispatch.rs`
- Modify: `crates/neo-agent-core/src/tools/bash.rs`
- Modify: `crates/neo-agent-core/src/tools/background_tasks.rs`
- Test: `crates/neo-tui/src/screen_output/debug_log.rs`
- Test: `crates/neo-agent/src/log_capture.rs`

- [ ] **Step 1: Add failing debug-log and captured-warning tests**

```rust
#[test]
fn debug_logger_uses_one_bounded_file_and_unique_frame_ids() {
    let dir = tempfile::tempdir().unwrap();
    let logger = DebugFrameLogger::new(dir.path(), 1024).unwrap();
    logger.record_frame(1, "render-start", "first").unwrap();
    logger.record_frame(1, "diff-output", "second").unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    let text = std::fs::read_to_string(logger.path()).unwrap();
    assert!(text.contains("frame=1 phase=render-start"));
    assert!(text.contains("frame=1 phase=diff-output"));
    assert!(text.len() <= 1024);
}

#[test]
fn core_warning_reaches_tui_capture_without_stderr_write() {
    let (capture, mut rx) = test_capture();
    let _guard = tracing_subscriber::registry().with(CapturingLayer::new(capture)).set_default();
    emit_repaired_tool_arguments_warning("Bash", "repaired");
    assert!(rx.blocking_recv().unwrap().message.contains("repaired"));
}
```

- [ ] **Step 2: Verify RED**

Run: `cargo nextest run -p neo-tui --lib debug_logger_uses_one_bounded_file_and_unique_frame_ids`

Expected: FAIL because each phase creates a timestamped file and same-millisecond names can truncate.

- [ ] **Step 3: Create one debug stream and remove direct core stderr ownership**

Replace `create_debug_log_file` calls with a lazily opened per-process `DebugFrameLogger` containing a monotonic frame id and a bounded/rotated file. Keep width-crash capture as an explicit one-shot artifact with a unique sequence id. Replace core `eprintln!` sites with structured `tracing::warn!`; persistence failures also update the relevant task/approval typed state so logs are not the sole signal. Delete obsolete formatted ring-buffer code in `neo-agent::log_capture` if no production reader remains; retain only the structured WARN/ERROR event channel.

- [ ] **Step 4: Verify GREEN**

Run: `cargo nextest run -p neo-tui --lib debug_logger_uses_one_bounded_file_and_unique_frame_ids`

Expected: PASS.

Run: `cargo test --package neo-agent --bin neo -- log_capture::tests::core_warning_reaches_tui_capture_without_stderr_write --exact --nocapture --include-ignored`

Expected: PASS.

- [ ] **Step 5: Root-only checkpoint after explicit authorization**

```bash
git add crates/neo-tui/src/screen_output crates/neo-agent-core/src/runtime crates/neo-agent-core/src/tools crates/neo-agent/src/log_capture.rs
git commit -m "fix(logging): unify bounded tui diagnostics"
```

## Final Verification And Review

- [ ] Run formatting: `cargo fmt --all --check`.
- [ ] Run one explicit target/filter for every task exactly as recorded above; do not replace them with a broad suite.
- [ ] Run targeted clippy for each touched library or binary target, for example `cargo clippy -p neo-agent-core --lib -- -D clippy::all` and `cargo clippy -p neo-tui --lib -- -D clippy::all`.
- [ ] Dispatch a fresh final reviewer with the complete 20-finding coverage matrix and the full diff.
- [ ] Confirm no direct production `eprintln!` remains under `neo-agent-core`.
- [ ] Confirm no production `to_string_lossy()` is used for workspace identity.
- [ ] Confirm no Google API key is added to a URL.
- [ ] Confirm no old full-snapshot-per-text-delta event path remains.
- [ ] Record any platform verification unavailable on the current host as explicit CI residual risk; do not claim Windows/Linux/macOS evidence that was not run.
