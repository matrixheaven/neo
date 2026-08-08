use futures::StreamExt;
use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{AgentRunMode, MultiAgentRuntime};
use neo_agent_core::tools::{ToolContext, ToolRegistry, ToolResult};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, PermissionMode,
    ToolExecutionMode,
};
use neo_ai::{AiStreamEvent, ChatMessage, ContentPart, StopReason};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn foreground_delegate_runs_child_model_turn_and_reports_child_summary() {
    let harness = foreground_delegate_harness();
    let events = run_harness_turn(&harness, "delegate a real task").await;

    let requests = harness.requests();
    assert!(
        requests.len() >= 2,
        "parent and child model turns should run"
    );
    let child_request = requests
        .iter()
        .find(|request| format!("{:?}", request.messages).contains("inspect queue"))
        .expect("child model request");
    let child_text = format!("{:?}", child_request.messages);
    assert!(child_text.contains("inspect queue"), "{child_text}");
    assert!(child_text.contains("Context mode: inherit"), "{child_text}");
    assert!(child_text.contains("Reviewer"), "{child_text}");
    assert!(child_text.contains("git add"), "{child_text}");
    assert!(child_text.contains("git commit"), "{child_text}");

    let delegate_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { result, .. }
                if serde_json::from_str::<serde_json::Value>(&result.content)
                    .ok()
                    .and_then(|content| content.get("kind").cloned())
                    == Some(serde_json::json!("delegate_result")) =>
            {
                Some(result)
            }
            _ => None,
        })
        .expect("delegate tool result");
    assert!(delegate_result.content.contains("queue is safe"));
    assert!(
        !delegate_result
            .content
            .contains("Foreground delegate completed.")
    );

    let finished_agent = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::DelegateFinished { turn, agent, .. } => Some((*turn, agent)),
            _ => None,
        })
        .expect("delegate finished event");
    assert_eq!(finished_agent.0, 1);
    assert_eq!(finished_agent.1.tool_count, 0);
    assert_eq!(finished_agent.1.token_count, 18);
    assert_eq!(finished_agent.1.cache_read_token_count, 9);
    assert_eq!(finished_agent.1.cache_write_token_count, 2);
    assert_eq!(
        finished_agent.1.latest_text.as_deref(),
        Some("queue is safe")
    );
    assert_eq!(
        finished_agent
            .1
            .outcome
            .as_ref()
            .map(|outcome| outcome.summary.as_str()),
        Some("queue is safe")
    );
}

#[tokio::test]
async fn delegate_tools_reject_empty_tasks_bad_context_and_zero_concurrency() {
    let harness = FakeHarness::from_turns([]);
    let registry = std::sync::Arc::new(ToolRegistry::with_builtin_tools());
    let ctx = neo_agent_core::tools::ToolContext::new(tempfile::tempdir().unwrap().path())
        .unwrap()
        .with_child_runtime(
            AgentConfig::for_model(harness.model())
                .with_tool_execution_mode(ToolExecutionMode::Sequential)
                .with_permission_mode(PermissionMode::Yolo),
            harness.client(),
            registry.clone(),
            1,
        );

    let empty_delegate = registry
        .run("Delegate", &ctx, json!({ "task": "" }))
        .await
        .expect("empty task should return validation result");
    assert!(empty_delegate.is_error);
    assert!(empty_delegate.content.contains("task must not be empty"));

    let bad_context = registry
        .run(
            "Delegate",
            &ctx,
            json!({ "task": "x", "context": "garbage" }),
        )
        .await
        .expect_err("bad context should be rejected");
    assert!(bad_context.to_string().contains("unknown variant"));

    let zero_concurrency = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "bad concurrency",
                "items": [{"title": "a", "value": "a"}],
                "prompt_template": "{{item}}",
                "max_concurrency": 0
            }),
        )
        .await
        .expect_err("zero concurrency should be rejected");
    assert!(zero_concurrency.to_string().contains("max_concurrency"));

    let legacy_template = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "legacy placeholder",
                "items": [{"title": "a", "value": "a"}],
                "prompt_template": "Review {task}"
            }),
        )
        .await
        .expect_err("legacy placeholder should be rejected");
    assert!(
        legacy_template
            .to_string()
            .contains("prompt_template must include {{item}}")
    );
}

#[tokio::test]
async fn swarm_result_shape_matches_between_foreground_wait_and_task_output() {
    let (registry, ctx) = registry_with_multi_agent();
    let foreground = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "shape check",
                "items": [
                    {"title": "a", "value": "a"},
                    {"title": "b", "value": "b"}
                ],
                "prompt_template": "Inspect {{item}}",
                "mode": "foreground"
            }),
        )
        .await
        .expect("foreground swarm should complete");
    let swarm_id = foreground.details.as_ref().unwrap()["swarm_id"]
        .as_str()
        .expect("swarm id")
        .to_owned();

    let waited = registry
        .run(
            "WaitDelegate",
            &ctx,
            serde_json::json!({ "ids": [swarm_id] }),
        )
        .await
        .expect("wait should read completed swarm");
    let output = registry
        .run(
            "TaskOutput",
            &ctx,
            serde_json::json!({ "task_id": swarm_id, "view": "result" }),
        )
        .await
        .expect("task output should read completed swarm");

    let foreground_details = foreground.details.as_ref().unwrap();
    let waited_details = waited.details.as_ref().unwrap();
    let output_details = output.details.as_ref().unwrap();
    let foreground_content: serde_json::Value =
        serde_json::from_str(&foreground.content).expect("foreground result JSON");
    let waited_content: serde_json::Value =
        serde_json::from_str(&waited.content).expect("wait result JSON");
    let output_content: serde_json::Value =
        serde_json::from_str(&output.content).expect("TaskOutput result JSON");
    assert_eq!(foreground_content["kind"], "delegate_swarm_result");
    assert_eq!(
        foreground_content["items"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(waited_content["kind"], "delegate_wait");
    assert_eq!(waited_content["items"][0]["kind"], "delegate_swarm_result");
    assert_eq!(output_content["kind"], "delegate_swarm_result");
    assert_eq!(output_content["items"].as_array().map(Vec::len), Some(2));
    assert_eq!(waited_details["kind"], "delegate_wait");
    assert_eq!(waited_details["outcome"], "all_terminal");
    assert_eq!(waited_details["aggregate"]["total"], 1);
    let waited_swarm = &waited_details["items"][0];

    for details in [foreground_details, waited_swarm, output_details] {
        assert_eq!(details["kind"], "delegate_swarm");
        assert_eq!(details["summary_scope"], "swarm_items");
        assert!(
            details["aggregate"]["total"].as_u64().is_some(),
            "{details}"
        );
        assert!(details["items"][0]["name"].as_str().is_some(), "{details}");
        assert!(
            details["items"][0]["elapsed_ms"].as_u64().is_some(),
            "{details}"
        );
        assert!(
            details["items"][0]["tool_count"].as_u64().is_some(),
            "{details}"
        );
        assert!(
            details["items"][0]["token_count"].as_u64().is_some(),
            "{details}"
        );
    }
}

#[tokio::test]
async fn delegate_swarm_rejects_unknown_template_placeholder() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": [{"title": "one", "value": "one"}],
                "prompt_template": "Audit {{task}} and {{item}}"
            }),
        )
        .await;

    let result = result.unwrap_or_else(|err| ToolResult::error(err.to_string()));
    assert!(result.is_error);
    assert!(
        result
            .content
            .contains("only {{item}} and {{description}} are supported"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn delegate_swarm_rejects_duplicate_expanded_prompts() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "audit",
                "items": [
                    {"title": "same", "value": "same"},
                    {"title": "same", "value": "same"}
                ],
                "prompt_template": "Audit {{item}}"
            }),
        )
        .await;

    let result = result.unwrap_or_else(|err| ToolResult::error(err.to_string()));
    assert!(result.is_error);
    assert!(
        result.content.contains("duplicate expanded child prompt"),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn child_runtime_shares_registry_but_not_parent_visibility() {
    let fixture = instruction_fixture();
    let (parent, nested_revision) = seed_parent_and_assert_compaction(&fixture).await;
    assert_child_instruction_modes(&fixture, &parent, &nested_revision).await;
    assert_runtime_child_inherits_and_persists_epoch(&fixture, parent).await;
}

#[tokio::test]
async fn concurrent_children_singleflight_the_same_source_read() {
    use neo_agent_core::instructions::{
        FilesystemSourceIo, InstructionRegistry, InstructionRegistryConfig, SourceIo,
        SourceMetadata,
    };
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest};
    use std::io;
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct CountingSourceIo {
        byte_reads: Arc<AtomicUsize>,
    }

    impl SourceIo for CountingSourceIo {
        fn read_metadata(&self, path: &Path) -> io::Result<SourceMetadata> {
            FilesystemSourceIo.read_metadata(path)
        }

        fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.byte_reads.fetch_add(1, Ordering::SeqCst);
            FilesystemSourceIo.read_bytes(path)
        }
    }

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp
        .path()
        .canonicalize()
        .expect("canonical tempdir")
        .join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    std::fs::write(workspace.join("AGENTS.md"), "root rules\n").expect("root agents");

    // Instrument the registry file reader; two child baselines read one
    // source once.
    let byte_reads = Arc::new(AtomicUsize::new(0));
    let registry = Arc::new(
        InstructionRegistry::with_source_io(
            InstructionRegistryConfig {
                primary_workspace: workspace,
                neo_home: None,
                project_trusted: true,
            },
            None,
            Arc::new(CountingSourceIo {
                byte_reads: Arc::clone(&byte_reads),
            }),
        )
        .expect("registry"),
    );

    let harness = FakeHarness::from_turns([
        child_text_turn("first child done"),
        child_text_turn("second child done"),
    ]);
    let mut config = AgentConfig::for_model(harness.model());
    config.instruction_registry = Some(Arc::clone(&registry));
    let deps = ChildRuntimeDeps::new(config, harness.client(), Arc::new(ToolRegistry::new()));
    let runtime = MultiAgentRuntime::new();
    let request = |task: &str| DelegateRequest {
        task: task.to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::None,
        output_schema: None,
    };
    let first_request = request("first child");
    let second_request = request("second child");
    let (first, second) = tokio::join!(
        runtime.run_child_turn(deps.clone(), &first_request, AgentRunMode::Foreground),
        runtime.run_child_turn(deps, &second_request, AgentRunMode::Foreground),
    );
    first.expect("first child run");
    second.expect("second child run");

    assert_eq!(
        byte_reads.load(Ordering::SeqCst),
        1,
        "two concurrent child baselines share the session source cache"
    );
}

/// Golden card contract for DelegateSwarm tool results / details projection.
/// Layout and field names must remain stable for TUI cards (Task 15).
#[tokio::test]
async fn delegate_swarm_golden_card_contract_is_unchanged() {
    let (registry, ctx) = registry_with_multi_agent();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            serde_json::json!({
                "description": "golden card swarm",
                "items": [
                    {"title": "alpha", "value": "A"},
                    {"title": "beta", "value": "B"}
                ],
                "prompt_template": "process {{item}}",
                "max_concurrency": 2
            }),
        )
        .await
        .expect("swarm runs");
    assert!(!result.is_error, "{}", result.content);
    let details = result.details.expect("details present");
    assert_eq!(details["kind"], "delegate_swarm");
    assert_eq!(details["summary_scope"], "swarm_items");
    assert!(details.get("swarm_id").and_then(|v| v.as_str()).is_some());
    assert!(details.get("id").and_then(|v| v.as_str()).is_some());
    assert_eq!(details["mode"], "foreground");
    assert!(details.get("role").is_some());
    assert_eq!(details["description"], "golden card swarm");
    assert!(details.get("aggregate").is_some());
    assert!(details.get("items").and_then(|v| v.as_array()).is_some());
    assert!(
        details
            .get("resume_hint")
            .and_then(|v| v.as_str())
            .is_some()
    );
    let items = details["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    for (index, item) in items.iter().enumerate() {
        assert_eq!(item["index"], index);
        assert!(item.get("item").is_some(), "{item}");
        assert!(item.get("agent_id").is_some(), "{item}");
        assert!(item.get("name").is_some(), "{item}");
        assert!(item.get("status").is_some(), "{item}");
        assert!(item.get("title").is_some(), "{item}");
        assert!(item.get("elapsed_ms").is_some(), "{item}");
        assert!(item.get("tool_count").is_some(), "{item}");
        assert!(item.get("token_count").is_some(), "{item}");
        // summary may be null or string — key must exist for card layout
        assert!(item.as_object().unwrap().contains_key("summary"), "{item}");
    }
    // Ordered by input index
    assert_eq!(items[0]["item"], "A");
    assert_eq!(items[1]["item"], "B");
}

fn foreground_delegate_harness() -> FakeHarness {
    FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "parent_msg".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "tool_delegate".to_owned(),
                name: "Delegate".to_owned(),
            },
            AiStreamEvent::ToolCallArgsDelta {
                id: "tool_delegate".to_owned(),
                json_fragment:
                    r#"{"task":"inspect queue","role":"reviewer","context":"parent facts"}"#
                        .to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "tool_delegate".to_owned(),
                raw_arguments: json!({
                    "task": "inspect queue",
                    "role": "reviewer",
                    "context": "inherit"
                })
                .to_string(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: neo_ai::MessagePhase::Unknown,
                id: "child_msg".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "queue is safe".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: neo_ai::MessagePhase::Unknown,
                stop_reason: StopReason::EndTurn,
                usage: Some(neo_ai::TokenUsage {
                    input_tokens: 13,
                    output_tokens: 5,
                    input_cache_read_tokens: 9,
                    input_cache_write_tokens: 2,
                }),
            },
        ],
    ])
}

async fn run_harness_turn(harness: &FakeHarness, prompt: &str) -> Vec<AgentEvent> {
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_tool_execution_mode(ToolExecutionMode::Sequential)
            .with_permission_mode(PermissionMode::Yolo),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text(prompt))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed")
}

fn child_text_turn(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: format!("msg_{text}"),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ]
}

fn request_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .flat_map(|message| match message {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. }
            | ChatMessage::ToolResult { content, .. } => content.iter(),
        })
        .filter_map(|part| match part {
            ContentPart::Text { text } | ContentPart::Thinking { text, .. } => Some(text.as_str()),
            ContentPart::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn registry_with_multi_agent() -> (ToolRegistry, ToolContext) {
    let turn_done = vec![
        AiStreamEvent::MessageStart {
            phase: neo_ai::MessagePhase::Unknown,
            id: "msg_x".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: neo_ai::MessagePhase::Unknown,
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ];
    let harness = FakeHarness::from_turns((0..10).map(|_| turn_done.clone()));
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap().with_child_runtime(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        harness.client(),
        std::sync::Arc::new(ToolRegistry::new()),
        1,
    );
    (ToolRegistry::with_builtin_tools(), ctx)
}

/// Combined text of every instruction injection in a context.
fn instruction_message_text(context: &AgentContext) -> String {
    context
        .messages()
        .iter()
        .filter(|message| message.is_injection_variant("instruction_epoch"))
        .map(AgentMessage::text)
        .collect()
}

struct InstructionFixture {
    _temp: tempfile::TempDir,
    workspace: std::path::PathBuf,
    nested: std::path::PathBuf,
    registry: Arc<neo_agent_core::instructions::InstructionRegistry>,
    budget: neo_agent_core::instructions::InstructionBudget,
}

fn instruction_fixture() -> InstructionFixture {
    use neo_agent_core::instructions::{InstructionRegistry, InstructionRegistryConfig};

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp
        .path()
        .canonicalize()
        .expect("canonical tempdir")
        .join("workspace");
    let nested = workspace.join("nested");
    std::fs::create_dir_all(&nested).expect("nested dir");
    std::fs::write(workspace.join("AGENTS.md"), "root rules\n").expect("root agents");
    std::fs::write(nested.join("AGENTS.md"), "nested rules\n").expect("nested agents");
    let registry = Arc::new(
        InstructionRegistry::new(InstructionRegistryConfig {
            primary_workspace: workspace.clone(),
            neo_home: None,
            project_trusted: true,
        })
        .expect("registry"),
    );

    InstructionFixture {
        _temp: temp,
        workspace,
        nested,
        registry,
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
    }
}

async fn seed_parent_and_assert_compaction(
    fixture: &InstructionFixture,
) -> (neo_agent_core::instructions::AgentInstructionState, String) {
    use neo_agent_core::instructions::{
        AgentInstructionState, InstructionInheritance, InstructionPreflightDecision,
        InstructionReconcileKind, InstructionReconcileRequest,
    };
    use neo_agent_core::multi_agent::seed_child_instruction_baseline;

    let mut parent = AgentInstructionState::default();
    let parent_request = InstructionReconcileRequest {
        agent_id: "main".to_owned(),
        kind: InstructionReconcileKind::Baseline,
        target_directories: vec![fixture.nested.clone()],
        budget: fixture.budget,
        deferred_tool_ids: Vec::new(),
    };
    let InstructionPreflightDecision::Defer { epoch, fingerprint } =
        fixture.registry.reconcile(parent_request, &parent).await
    else {
        panic!("parent baseline should defer with an epoch");
    };
    parent.apply_epoch(&epoch, &fingerprint);
    let nested_revision = parent
        .visible_revisions
        .get(&fixture.nested)
        .cloned()
        .expect("parent sees the nested revision");

    let mut compacted_parent = AgentContext::new();
    neo_agent_core::runtime::InstructionContextBridge::apply_epoch(
        &mut compacted_parent,
        &epoch,
        &fingerprint,
    );
    neo_agent_core::runtime::InstructionContextBridge::rehydrate_after_compaction(
        &fixture.registry,
        &mut compacted_parent,
    )
    .await
    .expect("parent rehydration");
    assert_eq!(
        compacted_parent
            .instruction_state()
            .most_recent_scope
            .as_deref(),
        Some(fixture.nested.as_path())
    );
    assert!(
        !compacted_parent
            .instruction_state()
            .active_scopes
            .contains(&fixture.nested)
    );

    let mut child_config = AgentConfig::for_model(neo_agent_core::harness::fake_model());
    child_config.instruction_registry = Some(Arc::clone(&fixture.registry));
    child_config.instruction_inheritance = InstructionInheritance::FullContext;
    let mut child = AgentContext::new();
    seed_child_instruction_baseline(
        &mut child,
        &child_config,
        Some(compacted_parent.instruction_state()),
        "agent_after_parent_compaction",
    )
    .await
    .expect("compacted-parent inherit baseline");
    assert!(
        child
            .instruction_state()
            .visible_revisions
            .contains_key(&fixture.nested),
        "full-context child must inherit the parent's most-recent pinned nested scope"
    );

    (parent, nested_revision)
}

async fn assert_summary_rehydration(
    fixture: &InstructionFixture,
    summary_child: &mut AgentContext,
) {
    neo_agent_core::runtime::InstructionContextBridge::rehydrate_after_compaction(
        &fixture.registry,
        summary_child,
    )
    .await
    .expect("summary child rehydration");
    assert!(
        !summary_child
            .instruction_state()
            .visible_revisions
            .contains_key(&fixture.nested),
        "session-shared cache metadata must not become summary-child visibility"
    );
    assert!(
        !summary_child
            .instruction_state()
            .visited_revisions
            .contains_key(&fixture.nested),
        "session-shared cache metadata must not become summary-child visited history"
    );
}

async fn assert_child_instruction_modes(
    fixture: &InstructionFixture,
    parent: &neo_agent_core::instructions::AgentInstructionState,
    nested_revision: &str,
) {
    use neo_agent_core::instructions::{InstructionEpochOutcome, InstructionInheritance};
    use neo_agent_core::multi_agent::seed_child_instruction_baseline;

    let mut inherit_config = AgentConfig::for_model(neo_agent_core::harness::fake_model());
    inherit_config.instruction_registry = Some(Arc::clone(&fixture.registry));
    inherit_config.instruction_inheritance = InstructionInheritance::FullContext;
    let mut inherit_child = AgentContext::new();
    let inherit_epoch = seed_child_instruction_baseline(
        &mut inherit_child,
        &inherit_config,
        Some(parent),
        "agent_inherit",
    )
    .await
    .expect("inherit baseline emits an epoch");

    let mut summary_config = AgentConfig::for_model(neo_agent_core::harness::fake_model());
    summary_config.instruction_registry = Some(Arc::clone(&fixture.registry));
    summary_config.instruction_inheritance = InstructionInheritance::Summary;
    let mut summary_child = AgentContext::new();
    let summary_epoch = seed_child_instruction_baseline(
        &mut summary_child,
        &summary_config,
        Some(parent),
        "agent_summary",
    )
    .await
    .expect("summary baseline emits an epoch");

    let inherit_registry = inherit_child
        .instruction_registry()
        .expect("inherit child registry");
    let summary_registry = summary_child
        .instruction_registry()
        .expect("summary child registry");
    assert!(Arc::ptr_eq(&inherit_registry, &fixture.registry));
    assert!(Arc::ptr_eq(&summary_registry, &fixture.registry));
    assert!(Arc::ptr_eq(&inherit_registry, &summary_registry));
    assert_eq!(inherit_epoch.agent_id, "agent_inherit");
    assert_eq!(summary_epoch.agent_id, "agent_summary");
    assert_eq!(inherit_epoch.outcome, InstructionEpochOutcome::Ready);
    assert_eq!(summary_epoch.outcome, InstructionEpochOutcome::Ready);
    assert_eq!(
        inherit_child
            .instruction_state()
            .visible_revisions
            .get(&fixture.nested)
            .map(String::as_str),
        Some(nested_revision),
        "full-context inheritance explicitly copies the parent's visible nested revision"
    );
    assert!(
        !summary_child
            .instruction_state()
            .visible_revisions
            .contains_key(&fixture.nested),
        "summary inheritance must not infer nested visibility from the parent"
    );
    assert!(
        summary_child
            .instruction_state()
            .visible_revisions
            .contains_key(&fixture.workspace),
        "summary baseline still loads the workspace root scope"
    );
    let inherit_text = instruction_message_text(&inherit_child);
    assert!(inherit_text.contains("nested rules"), "{inherit_text}");
    let summary_text = instruction_message_text(&summary_child);
    assert!(!summary_text.contains("nested rules"), "{summary_text}");
    assert!(summary_text.contains("root rules"), "{summary_text}");

    assert_summary_rehydration(fixture, &mut summary_child).await;
}

async fn assert_runtime_child_inherits_and_persists_epoch(
    fixture: &InstructionFixture,
    parent: neo_agent_core::instructions::AgentInstructionState,
) {
    use neo_agent_core::multi_agent::{ChildRuntimeDeps, DelegateContext, DelegateRequest};
    use neo_agent_core::session::{
        JsonlSessionReader, SessionState, SessionStateStore, agent_wire_path,
    };

    let session_temp = tempfile::tempdir().expect("session tempdir");
    let session_dir = session_temp.path();
    let mut session_state = SessionState::new();
    session_state.ensure_main_agent();
    SessionStateStore::new(session_dir)
        .write(&session_state)
        .expect("state");

    let runtime = MultiAgentRuntime::new().with_session_directory(session_dir.to_path_buf());
    let harness = FakeHarness::from_turns([child_text_turn("inherit child done")]);
    let mut child_config = AgentConfig::for_model(harness.model());
    child_config.instruction_registry = Some(Arc::clone(&fixture.registry));
    let deps = ChildRuntimeDeps::new(
        child_config,
        harness.client(),
        Arc::new(ToolRegistry::new()),
    )
    .with_parent_instruction_state(parent);
    let request = DelegateRequest {
        task: "inherit task".to_owned(),
        resume: None,
        title: None,
        role: None,
        mode: AgentRunMode::Foreground,
        context: DelegateContext::Inherit,
        output_schema: None,
    };
    let output = runtime
        .run_child_turn(deps, &request, AgentRunMode::Foreground)
        .await
        .expect("inherit child run");
    let child_id = output.snapshot.id.as_str().to_owned();

    let wire = agent_wire_path(session_dir, &child_id);
    let replayed = JsonlSessionReader::read_all(&wire)
        .await
        .expect("read child wire");
    let baseline = replayed
        .iter()
        .find_map(|event| match event {
            AgentEvent::InstructionEpoch { epoch } => Some(epoch),
            _ => None,
        })
        .expect("child baseline epoch event");
    assert_eq!(baseline.agent_id, child_id);
    assert!(
        baseline
            .selected_bundles
            .iter()
            .any(|bundle| bundle.display_path == fixture.nested),
        "inherit child baseline includes the nested scope: {baseline:#?}"
    );

    let requests = harness.requests();
    assert_eq!(requests.len(), 1, "{requests:#?}");
    let sent = request_text(&requests[0].messages);
    assert!(sent.contains("nested rules"), "{sent}");
    assert!(sent.contains("root rules"), "{sent}");

    assert!(
        replayed.iter().any(|event| matches!(
            event,
            AgentEvent::InstructionEpoch { epoch } if epoch.agent_id == child_id
        )),
        "child epochs go to the child's wire JSONL: {replayed:#?}"
    );
}
