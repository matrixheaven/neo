//! Task 16: generic neo.tool policy — deny set, default-open eligibility,
//! child tool_allow ceilings, and typed workflow provenance.

use std::sync::{Arc, Mutex};

use neo_agent_core::harness::FakeHarness;
use neo_agent_core::skills::builtin::builtin_skills;
use neo_agent_core::tools::{
    Tool, ToolContext, ToolFuture, ToolRegistry, ToolResult, intersect_child_tool_allow,
    is_workflow_tool_denied, is_workflow_tool_eligible,
};
use neo_agent_core::workflow::{WorkflowExecutionOrigin, WorkflowId, WorkflowInvocationContext};
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, ApprovalAction, ApprovalResponse, PermissionMode,
    ProcessSupervisor, WorkflowDispatchHandle,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

/// Ordinary registered tools stay open for workflow dispatch by default.
#[test]
fn ordinary_registered_tools_are_workflow_eligible_by_default() {
    let mut registry = ToolRegistry::with_builtin_tools();
    // Newly registered ordinary tool — no allowlist entry required.
    struct Probe;
    impl Tool for Probe {
        fn name(&self) -> &'static str {
            "ProbeTool"
        }
        fn description(&self) -> &'static str {
            "probe"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object", "additionalProperties": false})
        }
        fn execute<'a>(
            &'a self,
            _ctx: &'a ToolContext,
            _input: serde_json::Value,
        ) -> ToolFuture<'a> {
            Box::pin(async { Ok(ToolResult::ok("ok")) })
        }
    }
    registry.register(Probe);

    for name in [
        "Read",
        "List",
        "Grep",
        "Find",
        "Glob",
        "Write",
        "Edit",
        "Bash",
        "Terminal",
        "Sleep",
        "TaskList",
        "TaskOutput",
        "ProbeTool",
    ] {
        assert!(registry.contains(name), "expected {name} to be registered");
        assert!(
            !is_workflow_tool_denied(name),
            "{name} must not be in the deny set"
        );
        assert!(
            is_workflow_tool_eligible(&registry, name),
            "{name} must be workflow-eligible by default"
        );
        assert!(
            registry.is_workflow_eligible(name),
            "{name} registry eligibility"
        );
    }

    // Unregistered names are never eligible (exact registry lookup).
    assert!(!is_workflow_tool_eligible(&registry, "NotARealTool"));
    assert!(!registry.is_workflow_eligible("NotARealTool"));
}

/// Workflow / dialog / goal / plan / child-control tools share one deny classifier.
#[test]
fn workflow_dialog_goal_plan_and_child_tools_are_denied() {
    let registry = ToolRegistry::with_builtin_tools();

    let denied = [
        "Workflow",
        "Delegate",
        "DelegateSwarm",
        "TaskPause",
        "TaskResume",
        "TaskStop",
        "TaskAnswer",
        "AskUserQuestion",
        "EnterPlanMode",
        "ExitPlanMode",
        "StartGoal",
        "ExitGoalMode",
        "UpdateGoalStatus",
        "GetGoalStatus",
        "Todo",
        "TodoList",
        "ListDelegates",
        "WaitDelegate",
        "InterruptDelegate",
        "MessageDelegate",
    ];
    for name in denied {
        assert!(
            is_workflow_tool_denied(name),
            "{name} must be denied by the centralized classifier"
        );
        // Even if registered, eligibility is false.
        if registry.contains(name) {
            assert!(
                !is_workflow_tool_eligible(&registry, name),
                "registered {name} must not be workflow-eligible"
            );
        }
    }

    // No fuzzy matching: nearby ordinary names stay open.
    assert!(!is_workflow_tool_denied("TaskList"));
    assert!(!is_workflow_tool_denied("TaskOutput"));
    assert!(!is_workflow_tool_denied("Read"));
    assert!(is_workflow_tool_eligible(&registry, "TaskList"));
}

/// Child tool_allow may only reduce parent capability.
#[test]
fn child_tool_allow_only_reduces_parent_capability() {
    let parent = ToolRegistry::with_builtin_tools().workflow_eligible_subset();
    assert!(parent.contains("Read"));
    assert!(parent.contains("Grep"));
    assert!(parent.contains("Bash"));
    // Denied control tools never appear in parent eligibility.
    assert!(!parent.contains("Delegate"));
    assert!(!parent.contains("Workflow"));
    assert!(!parent.contains("TodoList"));

    // Ceiling reduces to exact names only.
    let reduced =
        intersect_child_tool_allow(&parent, Some(&["Read".to_owned(), "Grep".to_owned()]));
    assert!(reduced.contains("Read"));
    assert!(reduced.contains("Grep"));
    assert!(!reduced.contains("Bash"));
    assert!(!reduced.contains("Write"));

    // Ceiling cannot restore a denied/control tool that parent lacks.
    let cannot_elevate = intersect_child_tool_allow(
        &parent,
        Some(&[
            "Read".to_owned(),
            "Delegate".to_owned(),
            "Workflow".to_owned(),
        ]),
    );
    assert!(cannot_elevate.contains("Read"));
    assert!(!cannot_elevate.contains("Delegate"));
    assert!(!cannot_elevate.contains("Workflow"));

    // Exact match only — wrong case does not grant tools.
    let exact = intersect_child_tool_allow(&parent, Some(&["read".to_owned()]));
    assert!(!exact.contains("Read"));
    assert!(exact.names().is_empty());

    // None preserves full parent authority.
    let full = parent.for_workflow_child(None);
    assert_eq!(full.names(), parent.names());

    // for_workflow_child applies eligibility then ceiling in one step.
    let combined = ToolRegistry::with_builtin_tools()
        .for_workflow_child(Some(&["Read".to_owned(), "Delegate".to_owned()]));
    assert!(combined.contains("Read"));
    assert!(!combined.contains("Delegate"));
}

/// `Workflow` and `TaskAnswer` are root-only: child, restricted, and schema-repair
/// tool sets never receive them, no model-visible `RunWorkflow` remains, and the
/// model-visible description uses self-contained mutation actions with no
/// mandatory routing or CLI prerequisite.
#[test]
fn workflow_tool_is_root_only_and_description_has_no_choreography() {
    let root = ToolRegistry::with_builtin_tools();
    assert!(root.contains("Workflow"), "root registry owns Workflow");
    assert!(
        !root.contains("RunWorkflow"),
        "retired model tool must stay absent from the root registry"
    );
    assert!(root.contains("TaskAnswer"));

    let child = ToolRegistry::with_builtin_child_tools();
    assert!(!child.contains("Workflow"));
    assert!(!child.contains("TaskAnswer"));
    assert!(!child.contains("RunWorkflow"));

    // Schema-repair turns do not execute tools: no workflow launch surface.
    let repair = ToolRegistry::new();
    assert!(!repair.contains("Workflow"));

    // The canonical deny classifier keeps workflow launch unreachable from
    // workflow scripts (neo.tool), even though the root registers it.
    assert!(is_workflow_tool_denied("Workflow"));
    assert!(!is_workflow_tool_eligible(&root, "Workflow"));
    assert!(!root.is_workflow_eligible("Workflow"));

    // The model-visible description routes existing-workflow use to discovery
    // and authoring intent to the authoring skill.
    let description = neo_agent_core::tools::WorkflowTool.description();
    assert!(description.contains("create-workflow"));
    assert!(description.contains("TaskOutput"));
    assert!(description.contains("no slash capability"));
    assert!(description.contains("explicit action"));
    assert!(description.contains("both input_schema and output_schema"));
    assert!(description.contains("without resending definition fields"));
    assert!(description.contains("task IDs"));
    assert!(description.contains("list/show"));
    assert!(description.contains("run_saved"));
    assert!(!description.contains("inspect"));
    assert!(
        !description.contains("MUST call Workflow(validate_inline), then Workflow(run_inline)")
    );
    assert!(!description.contains("REQUIRED FIRST ACTION"));

    let schema = neo_agent_core::tools::WorkflowTool.input_schema();
    assert!(schema["oneOf"].as_array().is_some_and(|branches| {
        branches
            .iter()
            .any(|branch| branch["properties"]["action"]["const"] == "validate_inline")
    }));
    assert!(!schema.to_string().contains("start with validate_inline"));
}

/// Workflow provenance is typed on approvals and tool events.
#[tokio::test]
async fn workflow_provenance_is_typed_on_approval_and_events() {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = dir.path().canonicalize().expect("workspace");
    let harness = FakeHarness::from_turns([]);
    let events = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
    let captured = Arc::clone(&events);

    let config = AgentConfig::for_model(harness.model())
        .with_workspace_root(&workspace)
        .expect("workspace root")
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler(move |request| {
            assert!(
                request.workflow_origin.is_some(),
                "approval must carry typed workflow origin"
            );
            let origin = request.workflow_origin.as_ref().unwrap();
            assert!(!origin.run_id.as_str().is_empty());
            assert!(!origin.definition_name.is_empty());
            assert!(origin.invocation_id.is_some());
            ApprovalResponse::Selected {
                request_id: request.id.clone(),
                action: ApprovalAction::Reject,
                feedback: None,
            }
        });

    let handle = WorkflowDispatchHandle {
        config: config.clone(),
        model_client: harness.client(),
        registry: Arc::new(ToolRegistry::with_builtin_tools()),
        process_supervisor: ProcessSupervisor::default(),
        context: AgentContext::default(),
    };

    let resolver = handle.resolver().expect("resolver");
    let (_lease, _drain) = resolver
        .lease_event_route(
            handle.config.session_directory.as_deref(),
            0,
            Arc::new(move |event| {
                captured.lock().expect("events").push(event);
            }),
        )
        .expect("event route");

    let origin = WorkflowExecutionOrigin {
        run_id: WorkflowId::generate(),
        human_handle: Some("review-1".to_owned()),
        definition_name: "code-review".to_owned(),
        definition_revision: Some("abc".to_owned()),
        phase_id: Some("phase-1".to_owned()),
        invocation_id: None,
        swarm_item_id: None,
    };
    let expected_run_id = origin.run_id.clone();

    let outcome = handle
        .run_one_with_origin(
            WorkflowInvocationContext {
                invocation_id: "inv_tool_1".to_owned(),
                cancel_token: CancellationToken::new(),
            },
            "Bash",
            json!({"command": "sudo true"}),
            Some(origin),
        )
        .await;

    // Rejected by permission — not executed, but provenance was required.
    assert!(
        !outcome.is_completed(),
        "rejected bash should not be ok: {outcome:?}"
    );

    let events = events.lock().expect("events");
    let approval = events.iter().find_map(|event| match event {
        AgentEvent::ApprovalRequested { request } => Some(request),
        _ => None,
    });
    let request = approval.expect("expected ApprovalRequested event");
    let origin = request
        .workflow_origin
        .as_ref()
        .expect("approval event carries origin");
    assert_eq!(origin.run_id, expected_run_id);
    assert_eq!(origin.definition_name, "code-review");
    assert_eq!(origin.invocation_id.as_deref(), Some("inv_tool_1"));
    assert_eq!(origin.phase_id.as_deref(), Some("phase-1"));
    assert_eq!(origin.human_handle.as_deref(), Some("review-1"));

    // Tool lifecycle events that did emit also carry origin (when applicable).
    for event in events.iter() {
        match event {
            AgentEvent::ToolExecutionStarted {
                workflow_origin, ..
            }
            | AgentEvent::ToolExecutionQueued {
                workflow_origin, ..
            }
            | AgentEvent::ToolExecutionFinished {
                workflow_origin, ..
            } => {
                let o = workflow_origin
                    .as_ref()
                    .expect("tool event must carry workflow origin");
                assert_eq!(o.run_id, expected_run_id);
            }
            _ => {}
        }
    }
}

/// The create-workflow skill describes output schemas as optional best-effort
/// projections: a mismatch never fails a child or starts a repair turn,
/// `neo.verify(false, ...)` is completed data, and `status` is host execution
/// state while `verified`/`supported`/`partial` are Workflow-owned data.
#[test]
fn create_workflow_guidance_describes_optional_output_projection() {
    let skills = builtin_skills().expect("built-ins load");
    let create_workflow = skills
        .iter()
        .find(|skill| skill.name == "create-workflow")
        .expect("create-workflow built-in");
    let body = &create_workflow.body;

    // output_schema is optional projection metadata, not an execution gate.
    assert!(
        body.contains("optional projection metadata"),
        "output_schema must be optional projection metadata"
    );
    assert!(
        body.contains("best-effort structured projection"),
        "output_schema must enable a best-effort structured projection"
    );
    assert!(
        body.contains("projection mismatch never fails a child or the Workflow"),
        "a projection mismatch must never fail the child or the Workflow"
    );

    // A mismatch is data: no repair turn and no neo.fail for it.
    assert!(
        body.contains("never starts a repair turn"),
        "guidance must promise no repair turn"
    );
    assert!(
        body.contains("never for a missing projection or negative evidence"),
        "neo.fail must be reserved for real execution failure or explicit policy"
    );

    // verify(false, ...) is completed data; status is execution state.
    assert!(
        body.contains("details.verified"),
        "verify must expose details.verified"
    );
    assert!(
        body.contains("never aborts the script"),
        "verify must never abort the script"
    );
    assert!(
        body.contains("host execution state") && body.contains("Workflow-owned result data"),
        "status is host execution state; business fields are workflow-owned data"
    );

    // Retired strict/repair claims are gone from the guidance.
    for retired in [
        "Exactly one non-executing schema repair",
        "fail closed on evidence gates",
        "**required** for workflow-origin children",
        "Check every host outcome's `ok` field",
        "if not security_check.ok",
    ] {
        assert!(
            !body.contains(retired),
            "retired guidance must not remain: {retired:?}"
        );
    }
}
