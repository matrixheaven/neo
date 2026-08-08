//! Interactive test fixtures: workflow approval invocation scaffolding (moved from `mod.rs`).

use super::super::*;

pub async fn spawn_workflow_approval_invocation(
    config: &AppConfig,
    session_id: &str,
) -> (
    neo_agent_core::workflow::WorkflowHandle,
    tokio::task::JoinHandle<
        Result<
            neo_agent_core::workflow::WorkflowInvocationOutcome,
            neo_agent_core::workflow::WorkflowError,
        >,
    >,
    PathBuf,
) {
    let session_directory = workspace_sessions_dir(config).join(session_id);
    let runtime = neo_agent_core::workflow::WorkflowRuntime::new(
        neo_agent_core::workflow::WorkflowLimits::default(),
    );
    let handle = runtime
        .create_run(
            &session_directory,
            neo_agent_core::workflow::WorkflowLaunchRequest {
                name: "approval-stop".to_owned(),
                description: "approval stop cleanup".to_owned(),
                phases: vec![neo_agent_core::workflow::WorkflowPhase {
                    id: "verify".to_owned(),
                    description: "verify".to_owned(),
                }],
                script: "neo.phase('verify')".to_owned(),
                args: serde_json::json!({}),
                launch_source: "test".to_owned(),
                output_schema: None,
                display_name: None,
                input_schema: None,
                definition_origin: None,
                inline_unsaved: false,
            },
        )
        .await
        .expect("create workflow");
    handle
        .enter_running_for_direct_execution()
        .await
        .expect("workflow must be running before approval-backed invoke");
    let harness = neo_agent_core::harness::FakeHarness::from_turns([]);
    let agent_config = neo_agent_core::AgentConfig::for_model(harness.model())
        .with_workspace_root(&config.project_dir)
        .expect("workspace root")
        .with_session_directory(&session_directory)
        .with_permission_mode(PermissionMode::Ask)
        .with_workflow_dispatch_resolver(config.workflow_dispatch_resolver.clone());
    let dispatch = neo_agent_core::runtime::WorkflowDispatchHandle {
        config: agent_config,
        model_client: harness.client(),
        registry: Arc::new(neo_agent_core::ToolRegistry::with_builtin_tools()),
        process_supervisor: neo_agent_core::ProcessSupervisor::default(),
        context: neo_agent_core::AgentContext::new(),
    };
    let invocation_handle = handle.clone();
    let invocation = tokio::spawn(async move {
        invocation_handle
            .invoke(
                0,
                neo_agent_core::workflow::WorkflowInvocationKind::VerifyCommand,
                serde_json::json!({"command": "sudo --version"}),
                false,
                move |context| async move {
                    dispatch
                        .run_one(
                            context,
                            "Bash",
                            serde_json::json!({"command": "sudo --version"}),
                        )
                        .await
                },
            )
            .await
    });
    let journal_path = session_directory
        .join("workflows")
        .join(&handle.run_id.0)
        .join("journal.jsonl");
    (handle, invocation, journal_path)
}

pub fn assert_cancelled_workflow_invocation_journal(journal_path: &Path) {
    let envelopes = neo_agent_core::workflow::collect_journal(
        journal_path,
        None,
        neo_agent_core::workflow::WorkflowLimits::default().journal_record_bytes,
        neo_agent_core::workflow::WorkflowLimits::default().journal_total_bytes,
    )
    .expect("read journal");
    assert!(envelopes.iter().any(|envelope| {
        matches!(
            &envelope.payload,
            neo_agent_core::workflow::JournalPayload::InvocationFinished { outcome, .. }
                if outcome.status == neo_agent_core::workflow::WorkflowOutcomeStatus::Cancelled
        )
    }));
}
