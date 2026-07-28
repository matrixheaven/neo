//! Task 15: heterogeneous swarms lower into canonical ChildPlan; no total-count cap.

use std::sync::Arc;

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::multi_agent::{
    AgentLifecycleState, AgentRole, AgentRunMode, ChildPlan, ChildWorktreePolicy, DelegateContext,
    DelegateSwarmItem, DelegateSwarmRequest, MultiAgentRuntime, SwarmResourceLimits,
    child_plans_from_delegate_swarm, child_plans_serialized_bytes,
};
use neo_agent_core::tools::{ToolContext, ToolRegistry};
use neo_agent_core::{AgentConfig, PermissionMode, ToolExecutionMode};
use neo_ai::{AiStreamEvent, StopReason};
use serde_json::json;
use tempfile::tempdir;

fn registry_with_children() -> (ToolRegistry, ToolContext, tempfile::TempDir) {
    let turn_done = vec![
        AiStreamEvent::MessageStart {
            id: "msg_x".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            stop_reason: StopReason::EndTurn,
            usage: None,
        },
    ];
    let harness = FakeHarness::from_turns((0..20).map(|_| turn_done.clone()));
    let dir = tempdir().unwrap();
    let ctx = ToolContext::new(dir.path()).unwrap().with_child_runtime(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_tool_execution_mode(ToolExecutionMode::Sequential),
        harness.client(),
        Arc::new(ToolRegistry::new()),
        1,
    );
    (ToolRegistry::with_builtin_tools(), ctx, dir)
}

fn sample_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": { "ok": { "type": "boolean" } },
        "required": ["ok"],
        "additionalProperties": false
    })
}

fn heterogeneous_plans() -> Vec<ChildPlan> {
    vec![
        ChildPlan {
            item_id: "item-0".to_owned(),
            item_label: "research".to_owned(),
            task: "research the API surface".to_owned(),
            title: Some("research".to_owned()),
            resume: None,
            role: Some(AgentRole::Explorer),
            model: Some("fast".to_owned()),
            provider: None,
            context: DelegateContext::None,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: Some(vec!["read".to_owned(), "grep".to_owned()]),
            output_schema: Some(sample_schema()),
        },
        ChildPlan {
            item_id: "item-1".to_owned(),
            item_label: "implement".to_owned(),
            task: "implement the fix".to_owned(),
            title: Some("implement".to_owned()),
            resume: None,
            role: Some(AgentRole::Coder),
            model: None,
            provider: Some("openai".to_owned()),
            context: DelegateContext::Summary,
            worktree: ChildWorktreePolicy::Isolated,
            tool_allow: None,
            output_schema: Some(sample_schema()),
        },
        ChildPlan {
            item_id: "item-2".to_owned(),
            item_label: "review".to_owned(),
            task: "review the patch".to_owned(),
            title: Some("review".to_owned()),
            resume: None,
            role: Some(AgentRole::Reviewer),
            model: None,
            provider: None,
            context: DelegateContext::None,
            worktree: ChildWorktreePolicy::Shared,
            tool_allow: Some(vec!["read".to_owned()]),
            output_schema: Some(sample_schema()),
        },
    ]
}

#[test]
fn heterogeneous_child_specs_reach_one_childplan_owner() {
    let runtime = MultiAgentRuntime::new();
    let plans = heterogeneous_plans();

    // Adapter path: DelegateSwarm template form also lowers to ChildPlan.
    let request = DelegateSwarmRequest {
        description: "template swarm".to_owned(),
        items: vec![
            DelegateSwarmItem {
                title: "a".to_owned(),
                value: "alpha".to_owned(),
            },
            DelegateSwarmItem {
                title: "b".to_owned(),
                value: "beta".to_owned(),
            },
        ],
        prompt_template: Some("work on {{item}} for {{description}}".to_owned()),
        resume_agent_ids: Default::default(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        max_concurrency: Some(2),
    };
    let from_adapter = child_plans_from_delegate_swarm(&request).expect("lower adapter");
    assert_eq!(from_adapter.len(), 2);
    assert!(from_adapter[0].task.contains("alpha"));
    assert_eq!(from_adapter[0].role, Some(AgentRole::Coder));
    assert_eq!(from_adapter[0].item_label, "alpha");

    // Both heterogeneous neo.swarm plans and the adapter plans share prepare_swarm_batch.
    let swarm_id = runtime.new_swarm_id();
    let snapshot = runtime
        .prepare_swarm_batch(
            &swarm_id,
            "heterogeneous research",
            AgentRole::Coder,
            AgentRunMode::Foreground,
            Some(3),
            &plans,
        )
        .expect("prepare heterogeneous batch");

    assert_eq!(snapshot.children.len(), 3);
    assert_eq!(snapshot.children[0].agent.role, AgentRole::Explorer);
    assert_eq!(snapshot.children[1].agent.role, AgentRole::Coder);
    assert_eq!(snapshot.children[2].agent.role, AgentRole::Reviewer);
    assert_eq!(snapshot.children[0].agent.task, "research the API surface");
    assert_eq!(snapshot.children[1].agent.context, DelegateContext::Summary);
    assert_eq!(snapshot.children[0].item, "research");
    assert_eq!(snapshot.children[1].item, "implement");
    // Result order follows input order.
    assert_eq!(
        snapshot
            .children
            .iter()
            .map(|c| c.item_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    // Adapter-lowered plans also hit the same owner.
    let swarm_id_2 = runtime.new_swarm_id();
    let adapter_snapshot = runtime
        .prepare_swarm_batch(
            &swarm_id_2,
            &request.description,
            request.role,
            request.mode,
            request.max_concurrency,
            &from_adapter,
        )
        .expect("prepare adapter batch");
    assert_eq!(adapter_snapshot.children.len(), 2);
    assert!(adapter_snapshot.children[0].agent.task.contains("alpha"));
}

#[tokio::test]
async fn swarm_arrays_larger_than_eight_validate_within_resource_limits() {
    // Build 12 homogeneous items — exceeds the retired MAX_SWARM_CHILDREN=8.
    let items: Vec<DelegateSwarmItem> = (0..12)
        .map(|i| DelegateSwarmItem {
            title: format!("item-{i}"),
            value: format!("value-{i}"),
        })
        .collect();
    let request = DelegateSwarmRequest {
        description: "large swarm".to_owned(),
        items,
        prompt_template: Some("do {{item}}".to_owned()),
        resume_agent_ids: Default::default(),
        role: AgentRole::Coder,
        mode: AgentRunMode::Foreground,
        max_concurrency: Some(4),
    };
    let plans = child_plans_from_delegate_swarm(&request).expect("lower");
    assert_eq!(plans.len(), 12);

    let limits = SwarmResourceLimits::default();
    let bytes = child_plans_serialized_bytes(&request.description, &plans).expect("bytes");
    assert!(
        bytes < limits.max_request_bytes,
        "12 small items must fit default byte budget: {bytes}"
    );

    // Integration: DelegateSwarm tool validation accepts >8 children.
    let (registry, ctx, _dir) = registry_with_children();
    let items_json: Vec<_> = (0..12)
        .map(|i| json!({"title": format!("item-{i}"), "value": format!("v{i}")}))
        .collect();
    let result = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "large swarm",
                "items": items_json,
                "prompt_template": "do {{item}}",
                "max_concurrency": 2,
                "mode": "background",
            }),
        )
        .await
        .expect("tool returns");
    assert!(
        !result.is_error,
        "arrays larger than eight must validate within resource limits: {}",
        result.content
    );
    let details = result.details.as_ref().expect("details");
    assert_eq!(
        details["swarm"]["aggregate"]["total"], 12,
        "expected 12 children registered: content={} details={details}",
        result.content
    );

    // Oversized per-item field still fails (byte ceiling, not count).
    let huge_value = "x".repeat(limits.max_item_field_bytes + 1);
    let oversized = registry
        .run(
            "DelegateSwarm",
            &ctx,
            json!({
                "description": "oversized",
                "items": [{"title": "big", "value": huge_value}],
                "prompt_template": "do {{item}}",
                "mode": "background",
            }),
        )
        .await
        .expect_err("oversized item field must fail resource validation");
    let message = oversized.to_string();
    assert!(
        message.contains("resource limit") || message.contains("exceeds"),
        "unexpected error: {message}"
    );
}

#[test]
fn completed_items_are_not_replayed_after_sibling_failure() {
    let runtime = MultiAgentRuntime::new();
    let swarm_id = runtime.create_swarm_for_test(vec![
        ("done", AgentLifecycleState::Completed),
        ("failed", AgentLifecycleState::Failed),
        ("queued", AgentLifecycleState::Queued),
    ]);

    // Completed is never in the resumable set after a sibling failure.
    let resumable = runtime.resumable_swarm_items(&swarm_id);
    assert_eq!(
        resumable,
        vec![1, 2],
        "only failed+queued resume; completed skipped"
    );

    let snapshot = runtime.swarm_snapshot(&swarm_id).expect("swarm");
    let completed = &snapshot.children[0];
    assert_eq!(completed.agent.state, AgentLifecycleState::Completed);
    let completed_run_count = completed.agent.run_count;

    // Re-dispatch path used by swarm execution skips terminal children.
    // Emulate by checking agent remains terminal and run_count is unchanged.
    if let Some(current) = runtime.agent_snapshot(completed.agent.id.as_str()) {
        assert!(current.state.is_terminal());
        assert_eq!(current.run_count, completed_run_count);
        assert_eq!(current.state, AgentLifecycleState::Completed);
    }

    // prepare_swarm_batch of a fresh batch with one completed-style resume is
    // not used for finished items — durable finished items stay finished.
    let plans = vec![ChildPlan {
        item_id: "item-0".to_owned(),
        item_label: "queued-only".to_owned(),
        task: "only unfinished work".to_owned(),
        title: Some("queued-only".to_owned()),
        resume: None,
        role: Some(AgentRole::Coder),
        model: None,
        provider: None,
        context: DelegateContext::None,
        worktree: ChildWorktreePolicy::Shared,
        tool_allow: None,
        output_schema: None,
    }];
    let swarm_id_2 = runtime.new_swarm_id();
    let prepared = runtime
        .prepare_swarm_batch(
            &swarm_id_2,
            "partial",
            AgentRole::Coder,
            AgentRunMode::Foreground,
            Some(1),
            &plans,
        )
        .expect("prepare");
    assert_eq!(prepared.children.len(), 1);
    assert_eq!(
        prepared.children[0].agent.state,
        AgentLifecycleState::Queued
    );
}
