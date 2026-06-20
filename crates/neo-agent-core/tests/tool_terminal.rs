use neo_agent_core::{PermissionPolicy, ProcessSupervisor, ToolContext, ToolError, ToolRegistry};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_stop_returns_promptly_for_interactive_shell() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc"
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        registry.run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        ),
    )
    .await
    .expect("terminal stop should not block the async runtime")
    .expect("terminal stop should succeed");

    assert_eq!(supervisor.active_count().await, 0);
}

#[tokio::test]
async fn terminal_read_waits_briefly_for_fresh_running_output() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "sleep 0.02; printf settle-output; sleep 1"
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let read = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle, "max_output_bytes": 4096 }),
        )
        .await
        .expect("terminal read should succeed");

    assert!(
        read.content.contains("settle-output"),
        "terminal read should wait briefly for fresh running output: {read:?}"
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}

#[tokio::test]
async fn terminal_write_then_read_observes_interactive_shell_output() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 44,
                "rows": 9
            }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "printf terminal-event-ok\\n\n"
            }),
        )
        .await
        .expect("terminal write should succeed");

    let read = read_terminal_until(&registry, &context, &handle, "terminal-event-ok").await;
    assert!(
        read.contains("terminal-event-ok"),
        "terminal read should include interactive shell output: {read:?}"
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
}

#[tokio::test]
async fn terminal_tool_start_write_read_resize_and_stop_uses_real_pty() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "start",
                "command": "bash --noprofile --norc",
                "cols": 40,
                "rows": 8
            }),
        )
        .await
        .expect("terminal start should succeed");
    let details = started.details.as_ref().expect("start details");
    let handle = details["handle"].as_str().expect("terminal handle");
    assert_eq!(details["status"], "running");
    assert_eq!(details["cols"], 40);
    assert_eq!(details["rows"], 8);

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "printf 'pty:%s\\n' \"$COLUMNS:$LINES\"\n"
            }),
        )
        .await
        .expect("terminal write should succeed");

    let read = read_terminal_until(&registry, &context, handle, "pty:40:8").await;
    assert!(
        read.contains("pty:40:8"),
        "read output should include PTY-sized command output: {read:?}"
    );

    let resized = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "resize", "handle": handle, "cols": 72, "rows": 18 }),
        )
        .await
        .expect("terminal resize should succeed");
    assert_eq!(
        resized.details.as_ref().expect("resize details")["cols"],
        72
    );
    assert_eq!(
        resized.details.as_ref().expect("resize details")["rows"],
        18
    );

    registry
        .run(
            "Terminal",
            &context,
            json!({
                "mode": "write",
                "handle": handle,
                "input": "stty size\n"
            }),
        )
        .await
        .expect("terminal write after resize should succeed");
    let read = read_terminal_until(&registry, &context, handle, "18 72").await;
    assert!(
        read.contains("18 72"),
        "read output should include resized PTY dimensions: {read:?}"
    );

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle }),
        )
        .await
        .expect("terminal stop should succeed");
    assert_eq!(
        stopped.details.as_ref().expect("stop details")["status"],
        "stopped"
    );
    assert_eq!(supervisor.active_count().await, 0);

    let missing = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle }),
        )
        .await
        .expect_err("stopped terminal handle should be removed");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn process_supervisor_cleanup_stops_terminal_handles() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor.clone());

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "bash --noprofile --norc" }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();
    assert_eq!(supervisor.active_count().await, 1);

    supervisor.cleanup_all().await;
    assert_eq!(supervisor.active_count().await, 0);

    let missing = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": handle }),
        )
        .await
        .expect_err("supervisor cleanup should remove terminal handle");
    assert!(matches!(missing, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn terminal_read_details_do_not_leak_output_past_max_output_bytes() {
    let workspace = tempfile::tempdir().expect("workspace");
    let supervisor = ProcessSupervisor::default();
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all())
        .with_process_supervisor(supervisor);

    let started = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "start", "command": "printf 'keep-terminal-leak-tail'; sleep 1" }),
        )
        .await
        .expect("terminal start should succeed");
    let handle = started.details.as_ref().expect("start details")["handle"]
        .as_str()
        .expect("handle")
        .to_owned();

    let result = read_terminal_result_until_truncated_or_leaked(
        &registry,
        &context,
        &handle,
        "leak-tail",
        4,
    )
    .await;
    let serialized = serde_json::to_string(&result).expect("result serializes");

    assert!(result.content.contains("truncated: true"));
    assert!(!result.content.contains("terminal-leak-tail"));
    assert!(!serialized.contains("terminal-leak-tail"));
    let details = result.details.expect("read details");
    let output = details["output"].as_str().expect("details output");
    assert!(
        output.len() <= 4,
        "details output should be capped: {output:?}"
    );
    assert_eq!(details["output_truncated"], true);

    let stopped = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "stop", "handle": handle, "max_output_bytes": 4 }),
        )
        .await
        .expect("terminal stop should succeed");
    let stopped = serde_json::to_string(&stopped).expect("stop result serializes");
    assert!(!stopped.contains("terminal-leak-tail"));
}

#[tokio::test]
async fn terminal_description_contains_mode_guidance() {
    let registry = ToolRegistry::with_builtin_tools();
    let terminal = registry
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Terminal")
        .expect("Terminal tool spec");

    assert!(terminal.description.contains("start"));
    assert!(terminal.description.contains("write"));
    assert!(terminal.description.contains("read"));
    assert!(terminal.description.contains("resize"));
    assert!(terminal.description.contains("stop"));
    assert!(terminal.description.contains("Bash"));
}

#[test]
fn terminal_schema_includes_field_and_mode_descriptions() {
    let registry = ToolRegistry::with_builtin_tools();
    let terminal = registry
        .specs()
        .into_iter()
        .find(|spec| spec.name == "Terminal")
        .expect("Terminal tool spec");
    let schema = terminal
        .input_schema
        .get("schema")
        .unwrap_or(&terminal.input_schema);
    let required = schema["required"].as_array().expect("required array");
    let properties = schema["properties"].as_object().expect("schema properties");

    assert!(required.iter().any(|field| field == "mode"));
    assert!(!required.iter().any(|field| field == "command"));
    assert!(!required.iter().any(|field| field == "handle"));

    for field in [
        "mode",
        "command",
        "handle",
        "input",
        "cols",
        "rows",
        "max_output_bytes",
    ] {
        assert!(
            properties
                .get(field)
                .and_then(|property| property.get("description"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|description| !description.trim().is_empty()),
            "{field} should have a non-empty description"
        );
    }

    // Resolve the `mode` schema, which may be a `$ref` into `$defs`.
    let mode = &properties["mode"];
    let mode_schema = mode.get("schema").unwrap_or(mode);
    let mode_schema = if let Some(reference) = mode_schema["$ref"].as_str() {
        let key = reference.strip_prefix("#/$defs/").expect("$ref into $defs");
        &schema["$defs"][key]
    } else {
        mode_schema
    };
    let mode_any_of = mode_schema["anyOf"]
        .as_array()
        .or_else(|| mode_schema["oneOf"].as_array())
        .expect("mode should be an enum");
    let has_start_description = mode_any_of.iter().any(|variant| {
        variant["const"] == "start"
            && variant["description"]
                .as_str()
                .is_some_and(|d| !d.trim().is_empty())
    });
    assert!(
        has_start_description,
        "TerminalMode::Start should have a description in the schema"
    );
}

#[tokio::test]
async fn terminal_rejects_missing_mode() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let error = registry
        .run("Terminal", &context, json!({}))
        .await
        .expect_err("terminal should require mode");

    assert!(matches!(error, ToolError::InvalidInput { .. }));
}

#[tokio::test]
async fn terminal_rejects_unknown_handle() {
    let workspace = tempfile::tempdir().expect("workspace");
    let registry = ToolRegistry::with_builtin_tools();
    let context = ToolContext::new(workspace.path())
        .expect("context")
        .with_permission_policy(PermissionPolicy::allow_all());

    let error = registry
        .run(
            "Terminal",
            &context,
            json!({ "mode": "read", "handle": "no-such-handle" }),
        )
        .await
        .expect_err("terminal read should reject unknown handle");

    assert!(matches!(error, ToolError::InvalidInput { .. }));
}

async fn read_terminal_until(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    needle: &str,
) -> String {
    let mut latest = String::new();
    for _ in 0..50 {
        let read = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle, "max_output_bytes": 4096 }),
            )
            .await
            .expect("terminal read should succeed");
        latest.push_str(
            read.details
                .as_ref()
                .and_then(|details| details["output"].as_str())
                .unwrap_or_default(),
        );
        if latest.contains(needle) {
            return latest;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    latest
}

async fn read_terminal_result_until_truncated_or_leaked(
    registry: &ToolRegistry,
    context: &ToolContext,
    handle: &str,
    leak_tail: &str,
    max_output_bytes: usize,
) -> neo_agent_core::ToolResult {
    let mut latest = None;
    for _ in 0..50 {
        let read = registry
            .run(
                "Terminal",
                context,
                json!({ "mode": "read", "handle": handle, "max_output_bytes": max_output_bytes }),
            )
            .await
            .expect("terminal read should succeed");
        let serialized = serde_json::to_string(&read).expect("result serializes");
        let truncated = read
            .details
            .as_ref()
            .and_then(|details| details["output_truncated"].as_bool())
            .unwrap_or(false);
        if truncated || serialized.contains(leak_tail) {
            return read;
        }
        latest = Some(read);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    latest.expect("terminal read should have been attempted")
}
