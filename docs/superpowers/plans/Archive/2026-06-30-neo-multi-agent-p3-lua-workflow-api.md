# Neo Multi-Agent P3 Lua Workflow API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Lua workflow APIs match the tool contracts and return useful structured data instead of recorder/demo handles.

**Architecture:** Keep `LuaWorkflowRunner` as the sandbox host, but replace ad-hoc userdata-only return values with table-safe handles that can convert themselves to JSON. `neo.verify` becomes a pure assertion API, and shell verification moves to `neo.verify_command`. Reports are stored as structured values and surfaced in `RunWorkflow` output content and details.

**Tech Stack:** Rust 2024, `mlua`, `serde_json`, `ToolRegistry`, `RunWorkflowTool`, `cargo nextest run`.

---

## Source Spec

Use `/Users/chenyuanhao/Workspace/neo/docs/superpowers/specs/2026-06-30-neo-multi-agent-hardening-design.md`.

This plan covers:

- Section 15 Lua Workflow API.
- Section 20 workflow error message contract.
- Acceptance criteria under Lua.

P1 and P2 must be complete first.

## Constraints

- Start implementation with `icm recall-context "Neo multi-agent P3 Lua workflow API" --limit 5`.
- Use CodeGraph before grep/read for symbol discovery in this repo.
- Do not run bare `cargo test`; use `cargo nextest run ...`.
- Do not mutate git unless the user explicitly authorizes that exact command.
- Do not leave `neo.verify(shell_command)` semantics in place.
- Do not expose Rust source paths in user-facing workflow errors.
- Do not make `description` optional for `neo.swarm`.

## Current Code Touchpoints

- `crates/neo-agent-core/src/workflow/lua.rs`
  - Installs `neo.delegate`, `neo.swarm`, `neo.verify`, `neo.report`, `neo.fail`.
  - `DelegateHandle` and `SwarmHandle` are userdata.
  - `lua_return_to_json` rejects returned userdata.
  - `neo.verify` runs Bash.
- `crates/neo-agent-core/src/workflow/host_api.rs`
  - `WorkflowHostRecorder` stores steps and reports.
- `crates/neo-agent-core/src/tools/workflow.rs`
  - Formats `RunWorkflow` output.
- `crates/neo-agent-core/tests/workflow_lua.rs`
  - Existing workflow smoke tests.

## File Structure

Modify:

- `crates/neo-agent-core/src/workflow/lua.rs`
- `crates/neo-agent-core/src/workflow/host_api.rs`
- `crates/neo-agent-core/src/tools/workflow.rs`
- `crates/neo-agent-core/tests/workflow_lua.rs`

## Desired End State

- `return neo.delegate({...})` serializes to JSON/table.
- `return neo.swarm({...})` serializes to JSON/table.
- `neo.swarm(...):items()` and `:results()` return per-child structured tables.
- Lua `neo.swarm` uses the same required `description` and template validation as `DelegateSwarm`.
- `neo.verify(true, "msg")` succeeds.
- `neo.verify(false, "msg")` fails with `msg`.
- `neo.verify_command(command, failure_message)` routes through Bash permission and reports denial as `verify_command denied by Bash permission policy`.
- `neo.report(value)` contents appear in `RunWorkflow` content and details.
- Workflow errors are sanitized and do not include `crates/neo-agent-core/src/...`.

## Task 1: Add Failing Tests For Table-Safe Handles

**Files:**

- Modify: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Add delegate return serialization test**

Append:

```rust
#[tokio::test]
async fn workflow_can_return_delegate_handle_as_table() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "delegate return",
                "script": "return neo.delegate({ task = 'inspect one file', mode = 'foreground' })"
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    let details = result.details.as_ref().expect("details");
    let returned = details.get("result").expect("result");
    assert_eq!(returned.get("kind").and_then(serde_json::Value::as_str), Some("delegate"));
    assert!(returned.get("agent_id").and_then(serde_json::Value::as_str).is_some());
    assert!(returned.get("summary").and_then(serde_json::Value::as_str).is_some());
}
```

- [ ] **Step 2: Add swarm item serialization test**

Append:

```rust
#[tokio::test]
async fn workflow_swarm_handle_exposes_items_and_serializes() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "swarm return",
                "script": r#"
                    local s = neo.swarm({
                        description = "audit",
                        items = { "core", "tui" },
                        prompt_template = "Audit {{item}}",
                        mode = "foreground"
                    })
                    local items = s:items()
                    return { id = s:id(), summary = s:summary(), items = items, table = s:to_table() }
                "#
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    let returned = result.details.as_ref().unwrap().get("result").unwrap();
    assert_eq!(
        returned.get("items").and_then(serde_json::Value::as_array).map(Vec::len),
        Some(2)
    );
    assert_eq!(
        returned.pointer("/table/kind").and_then(serde_json::Value::as_str),
        Some("swarm")
    );
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL with unsupported userdata or missing swarm item methods.

## Task 2: Implement Table-Safe Delegate And Swarm Handles

**Files:**

- Modify: `crates/neo-agent-core/src/workflow/lua.rs`

- [ ] **Step 1: Add serializable handle payloads**

In `workflow/lua.rs`, replace handle structs with:

```rust
#[derive(Debug, Clone, serde::Serialize)]
struct DelegateHandle {
    kind: &'static str,
    agent_id: Option<String>,
    name: Option<String>,
    status: String,
    summary: String,
    result: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SwarmHandle {
    kind: &'static str,
    swarm_id: Option<String>,
    status: String,
    summary: String,
    items: Vec<serde_json::Value>,
    has_failures: bool,
    result: serde_json::Value,
}
```

- [ ] **Step 2: Add `to_lua_table` helpers**

Add:

```rust
impl DelegateHandle {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("delegate handle serializes")
    }

    fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Value> {
        lua.to_value(&self.to_json())
    }
}

impl SwarmHandle {
    fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("swarm handle serializes")
    }

    fn to_lua_table(&self, lua: &Lua) -> mlua::Result<Value> {
        lua.to_value(&self.to_json())
    }
}
```

- [ ] **Step 3: Update userdata methods**

For `DelegateHandle`:

```rust
impl mlua::UserData for DelegateHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, _: ()| Ok(this.agent_id.clone()));
        methods.add_method("status", |_, this, _: ()| Ok(this.status.clone()));
        methods.add_method("summary", |_, this, _: ()| Ok(this.summary.clone()));
        methods.add_method("result", |lua, this, _: ()| lua.to_value(&this.result));
        methods.add_method("to_table", |lua, this, _: ()| this.to_lua_table(lua));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, _: ()| {
            Ok(this.agent_id.clone().unwrap_or_else(|| this.summary.clone()))
        });
    }
}
```

For `SwarmHandle`:

```rust
impl mlua::UserData for SwarmHandle {
    fn add_methods<M: mlua::UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("id", |_, this, _: ()| Ok(this.swarm_id.clone()));
        methods.add_method("status", |_, this, _: ()| Ok(this.status.clone()));
        methods.add_method("summary", |_, this, _: ()| Ok(this.summary.clone()));
        methods.add_method("items", |lua, this, _: ()| lua.to_value(&this.items));
        methods.add_method("results", |lua, this, _: ()| lua.to_value(&this.items));
        methods.add_method("has_failures", |_, this, _: ()| Ok(this.has_failures));
        methods.add_method("to_table", |lua, this, _: ()| this.to_lua_table(lua));
        methods.add_meta_method(mlua::MetaMethod::ToString, |_, this, _: ()| {
            Ok(this.swarm_id.clone().unwrap_or_else(|| this.summary.clone()))
        });
    }
}
```

- [ ] **Step 4: Convert returned userdata to JSON**

In `lua_return_to_json`, add downcast handling:

```rust
fn lua_return_to_json(lua: &Lua, value: Value) -> mlua::Result<Option<serde_json::Value>> {
    match value {
        Value::Nil => Ok(None),
        Value::UserData(userdata) => {
            if let Ok(handle) = userdata.borrow::<DelegateHandle>() {
                return Ok(Some(handle.to_json()));
            }
            if let Ok(handle) = userdata.borrow::<SwarmHandle>() {
                return Ok(Some(handle.to_json()));
            }
            Err(mlua::Error::external("unsupported workflow return userdata"))
        }
        other => lua.from_value(other).map(Some),
    }
}
```

- [ ] **Step 5: Populate handles from tool details**

Update `delegate_handle_from_result` to set:

```rust
DelegateHandle {
    kind: "delegate",
    agent_id: agent.map(|snapshot| snapshot.id.as_str().to_owned()),
    name: agent.map(|snapshot| snapshot.display_name.as_str().to_owned()),
    status: agent
        .map(|snapshot| snapshot.state.as_str().to_owned())
        .unwrap_or_else(|| if result.is_error { "failed" } else { "completed" }.to_owned()),
    summary: existing_summary,
    result: result.details.clone().unwrap_or_else(|| serde_json::json!({})),
}
```

Update swarm handle creation to set:

```rust
let items = result
    .details
    .as_ref()
    .and_then(|details| details.get("items"))
    .and_then(serde_json::Value::as_array)
    .cloned()
    .unwrap_or_default();
SwarmHandle {
    kind: "swarm",
    swarm_id: result
        .details
        .as_ref()
        .and_then(|details| details.get("swarm_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned),
    status: result
        .details
        .as_ref()
        .and_then(|details| details.get("status"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or(if has_failures { "failed" } else { "completed" })
        .to_owned(),
    summary,
    items,
    has_failures,
    result: result.details.clone().unwrap_or_else(|| serde_json::json!({})),
}
```

- [ ] **Step 6: Run handle tests**

Run:

```bash
```

Expected: PASS.

## Task 3: Split `neo.verify` And `neo.verify_command`

**Files:**

- Modify: `crates/neo-agent-core/src/workflow/lua.rs`
- Modify: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Add assertion verify tests**

Append:

```rust
#[tokio::test]
async fn workflow_verify_is_boolean_assertion() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let ok = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "verify assertion ok",
                "script": "neo.verify(true, 'should pass'); return 'ok'"
            }),
        )
        .await
        .expect("workflow should run");
    assert!(!ok.is_error, "{}", ok.content);

    let failed = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "verify assertion fail",
                "script": "neo.verify(false, 'expected three completed children')"
            }),
        )
        .await
        .expect("workflow returns failure result");
    assert!(failed.is_error);
    assert!(failed.content.contains("expected three completed children"), "{}", failed.content);
}
```

- [ ] **Step 2: Add `verify_command` permission wording test**

Append:

```rust
#[tokio::test]
async fn workflow_verify_command_reports_bash_permission_denial_clearly() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_denying_bash();

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "verify command denied",
                "script": "return neo.verify_command('printf denied', 'verify failed')"
            }),
        )
        .await
        .expect("workflow returns failure result");

    assert!(result.is_error);
    assert!(
        result.content.contains("verify_command denied by Bash permission policy"),
        "{}",
        result.content
    );
}
```

Add this helper beside the existing workflow context helpers:

```rust
fn workflow_tool_context_denying_bash() -> (ToolContext, Arc<FakeHarness>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let harness = Arc::new(FakeHarness::default());
    let registry = Arc::new(ToolRegistry::with_builtin_tools());
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Sequential)
        .with_permission_mode(PermissionMode::Yolo);
    let ctx = ToolContext::new(dir.path())
        .expect("tool context")
        .with_access(ToolAccess {
            file_read: true,
            file_write: false,
            shell: false,
            tool: true,
            user_question: false,
        })
        .with_child_runtime(config, harness.client(), registry, 1);
    (ctx, harness)
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
```

Expected: FAIL because `neo.verify` expects a string command and `verify_command` is missing.

- [ ] **Step 4: Replace `verify` with assertion function**

In `install_host_neo_table`, replace the current async `verify` with:

```rust
let recorder_verify = self.recorder.clone();
let verify_events = event_context.clone();
let verify_ctx = ctx.clone();
let verify = lua
    .create_function(move |_, (condition, message): (bool, String)| -> mlua::Result<()> {
        if condition {
            recorder_verify.push_step(workflow_step(
                "verify",
                WorkflowState::Completed,
                Some(message.clone()),
                Some(serde_json::json!({ "condition": true, "message": message })),
                None,
                None,
                None,
            ));
            emit_workflow_update(&verify_ctx, &verify_events, &recorder_verify);
            Ok(())
        } else {
            recorder_verify.push_step(workflow_step(
                "verify",
                WorkflowState::Failed,
                Some(message.clone()),
                Some(serde_json::json!({ "condition": false, "message": message.clone() })),
                None,
                None,
                None,
            ));
            emit_workflow_update(&verify_ctx, &verify_events, &recorder_verify);
            Err(mlua::Error::RuntimeError(message))
        }
    })
    .map_err(|e| WorkflowError::Host(e.to_string()))?;
neo.set("verify", verify)
    .map_err(|e| WorkflowError::Host(e.to_string()))?;
```

- [ ] **Step 5: Add `verify_command`**

Install:

```rust
let verify_command_ctx = ctx.clone();
let verify_command_recorder = self.recorder.clone();
let verify_command_events = event_context.clone();
let verify_command = lua
    .create_async_function(move |_, (command, failure_message): (String, Option<String>)| {
        let ctx = verify_command_ctx.clone();
        let recorder = verify_command_recorder.clone();
        let event_context = verify_command_events.clone();
        async move {
            recorder.record(format!("verify_command: {command}"));
            let result = run_tool(&ctx, "Bash", serde_json::json!({ "command": command })).await;
            match result {
                Ok(result) if !result.is_error => {
                    recorder.push_step(workflow_step(
                        "verify_command",
                        WorkflowState::Completed,
                        Some(result.content.clone()),
                        result.details,
                        None,
                        None,
                        None,
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Ok(true)
                }
                Ok(result) => {
                    let message = failure_message.unwrap_or(result.content);
                    recorder.push_step(workflow_step(
                        "verify_command",
                        WorkflowState::Failed,
                        Some(message.clone()),
                        result.details,
                        None,
                        None,
                        None,
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Err(mlua::Error::RuntimeError(message))
                }
                Err(err) => {
                    let raw = err.to_string();
                    let message = if raw.to_ascii_lowercase().contains("permission") {
                        "verify_command denied by Bash permission policy".to_owned()
                    } else {
                        failure_message.unwrap_or(raw)
                    };
                    recorder.push_step(workflow_step(
                        "verify_command",
                        WorkflowState::Failed,
                        Some(message.clone()),
                        None,
                        None,
                        None,
                        None,
                    ));
                    emit_workflow_update(&ctx, &event_context, &recorder);
                    Err(mlua::Error::RuntimeError(message))
                }
            }
        }
    })
    .map_err(|e| WorkflowError::Host(e.to_string()))?;
neo.set("verify_command", verify_command)
    .map_err(|e| WorkflowError::Host(e.to_string()))?;
```

- [ ] **Step 6: Run verify tests**

Run:

```bash
```

Expected: PASS.

## Task 4: Show Report Contents In RunWorkflow Output

**Files:**

- Modify: `crates/neo-agent-core/src/tools/workflow.rs`
- Modify: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Add failing report preview test**

Append:

```rust
#[tokio::test]
async fn run_workflow_output_includes_report_values() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "reports",
                "script": r#"
                    neo.report("first report")
                    neo.report({ completed = 3 })
                    return "done"
                "#
            }),
        )
        .await
        .expect("workflow should run");

    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("first report"), "{}", result.content);
    assert!(result.content.contains("completed"), "{}", result.content);
    let reports = result
        .details
        .as_ref()
        .and_then(|details| details.get("reports"))
        .and_then(serde_json::Value::as_array)
        .expect("reports array");
    assert_eq!(reports.len(), 2);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
```

Expected: FAIL if content only says `reports: 2`.

- [ ] **Step 3: Add report preview formatter**

In `tools/workflow.rs`:

```rust
fn format_report_preview(reports: &[serde_json::Value]) -> String {
    reports
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, value)| format!("report {}: {}", index + 1, compact_json(value)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<unserializable report>".to_owned()),
    }
}
```

- [ ] **Step 4: Include reports in content and details**

In `RunWorkflowTool::execute`, after runner completes:

```rust
let reports = runner.recorder().reports();
let report_preview = format_report_preview(&reports);
let content = if report_preview.is_empty() {
    format!("workflow: {title}\nstatus: completed\nsteps: {}", steps.len())
} else {
    format!(
        "workflow: {title}\nstatus: completed\nsteps: {}\nreports:\n{}",
        steps.len(),
        report_preview
    )
};
```

In details:

```rust
"reports": reports
    .iter()
    .enumerate()
    .map(|(index, value)| serde_json::json!({ "index": index + 1, "value": value }))
    .collect::<Vec<_>>(),
```

- [ ] **Step 5: Run report test**

Run:

```bash
```

Expected: PASS.

## Task 5: Sanitize Lua Errors

**Files:**

- Modify: `crates/neo-agent-core/src/workflow/lua.rs`
- Modify: `crates/neo-agent-core/tests/workflow_lua.rs`

- [ ] **Step 1: Add failing source-path sanitizer test**

Append:

```rust
#[tokio::test]
async fn workflow_lua_errors_do_not_expose_rust_source_paths() {
    let tool = neo_agent_core::tools::RunWorkflowTool;
    let (ctx, _model) = workflow_tool_context_with_fake_model();

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({
                "title": "bad lua",
                "script": "error('plain workflow failure')"
            }),
        )
        .await
        .expect("workflow returns failure result");

    assert!(result.is_error);
    assert!(result.content.contains("plain workflow failure"), "{}", result.content);
    assert!(
        !result.content.contains("crates/neo-agent-core/src"),
        "{}",
        result.content
    );
}
```

- [ ] **Step 2: Run test to verify it fails if paths leak**

Run:

```bash
```

Expected: PASS. Keep this regression test even if the current implementation already passes.

- [ ] **Step 3: Add sanitizer helper**

In `workflow/lua.rs`:

```rust
fn sanitize_lua_error(err: impl ToString) -> String {
    let raw = err.to_string();
    raw.lines()
        .filter(|line| !line.contains("crates/neo-agent-core/src/"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
```

Use it anywhere `WorkflowError::Lua(err.to_string())` is created:

```rust
.map_err(|err| WorkflowError::Lua(sanitize_lua_error(err)))?
```

- [ ] **Step 4: Run sanitizer test**

Run:

```bash
```

Expected: PASS.

## Task 6: P3 Verification And Commit Boundary

**Files:**

- Verify all files changed by this plan.

- [ ] **Step 1: Run all workflow Lua tests**

Run:

```bash
```

Expected: PASS.

- [ ] **Step 2: Scan for old verify semantics**

Run:

```bash
rg -n "neo\\.verify\\(\"|verify\\(\" crates/neo-agent-core/tests/workflow_lua.rs docs crates/neo-agent-core/src/workflow
```

Expected:

- Tests and docs use `neo.verify(true_or_false, "message")` for assertions.
- Shell command checks use `neo.verify_command("command", "message")`.

- [ ] **Step 3: Commit if authorized**

Only if the user has explicitly authorized git mutation in this session:

```bash
git add crates/neo-agent-core/src/workflow/lua.rs \
  crates/neo-agent-core/src/workflow/host_api.rs \
  crates/neo-agent-core/src/tools/workflow.rs \
  crates/neo-agent-core/tests/workflow_lua.rs
git commit -m "fix: harden lua workflow multi-agent api"
```

Expected: one logical commit for P3.
