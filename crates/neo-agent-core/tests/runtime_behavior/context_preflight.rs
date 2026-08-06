use super::compaction_rehydration::PreflightFixture;
use super::compaction_rehydration::preflight_config;
use super::compaction_rehydration::preflight_context;
use super::compaction_rehydration::preflight_fixture;
use super::context::event_index;
use super::context::finished_tool_results;
use super::context::instruction_epochs;
use super::fake_harness::end_turn_events;
use super::fake_harness::run_turn_collect;
use super::fake_harness::tool_call_turn;
use super::permissions::permit_once;
use futures::StreamExt;
use neo_agent_core::{
    AgentContext, AgentEvent, AgentMessage, AgentRuntime, ApprovalAction, ApprovalResponse,
    Content, PermissionMode, StopReason, ToolRegistry,
    harness::FakeHarness,
    instructions::{InstructionEpochOutcome, InstructionFailureKind},
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::timeout;

pub(crate) fn preflight_runtime(fixture: &PreflightFixture, harness: &FakeHarness) -> AgentRuntime {
    AgentRuntime::with_tools(
        preflight_config(fixture, harness),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    )
}

fn has_tool_started(events: &[AgentEvent]) -> bool {
    event_index(events, |event| {
        matches!(event, AgentEvent::ToolExecutionStarted { .. })
    })
    .is_some()
}

#[tokio::test]
async fn first_nested_edit_defers_before_side_effect_and_retried_batch_executes_once() {
    let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
    let target = fixture.workspace.join("nested").join("target.txt");
    std::fs::write(&target, "alpha").expect("target file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "alpha",
        "new": "beta"
    });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("call_1", "Edit", edit_arguments.clone())]),
        tool_call_turn(&[("call_2", "Edit", edit_arguments)]),
        end_turn_events("edited"),
    ]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = AgentContext::new();

    let events = run_turn_collect(&runtime, &mut context, "edit the file").await;

    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus nested activation epochs: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["call_1".to_owned()]);

    // The deferred first call produced a provider-valid non-error result and
    // never touched the file; the retried call edited it exactly once.
    let deferred = finished_tool_results(&events, "call_1");
    assert_eq!(deferred.len(), 1, "call_1");
    assert!(!deferred[0].is_error, "deferred result must be non-error");
    let details = deferred[0].details.as_ref().expect("deferred details");
    assert_eq!(details["status"], "deferred");
    assert_eq!(details["reason"], "instruction_epoch");
    assert_eq!(details["side_effect_occurred"], false);
    assert_eq!(details["generation"], json!(epochs[1].generation));

    let retried = finished_tool_results(&events, "call_2");
    assert_eq!(retried.len(), 1, "call_2");
    assert!(
        !retried[0].is_error,
        "the retried edit must succeed exactly once: {}",
        retried[0].content
    );
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target"),
        "beta"
    );

    // The deferred result reaches the provider before the next request.
    let requests = harness.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1].messages.iter().any(|message| matches!(
            message,
            neo_ai::ChatMessage::ToolResult { tool_call_id, is_error, .. }
                if tool_call_id == "call_1" && !is_error
        )),
        "the deferred call must receive a non-error tool result before the next request"
    );

    // Event order proof: the activation epoch precedes every tool start; no
    // permission prompt ever appears before instruction preflight.
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.generation == epochs[1].generation)
    })
    .expect("activation epoch index");
    let started_index = event_index(&events, |event| {
        matches!(event, AgentEvent::ToolExecutionStarted { .. })
    });
    assert!(
        started_index.is_some_and(|index| index > epoch_index),
        "no tool may start before the instruction epoch: {events:?}"
    );
    assert!(
        event_index(&events, |event| matches!(
            event,
            AgentEvent::ApprovalRequested { .. }
        ))
        .is_none(),
        "no approval prompt may precede instruction preflight: {events:?}"
    );
}

#[tokio::test]
async fn one_new_scope_defers_every_call_in_a_parallel_mixed_batch() {
    let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
    std::fs::write(fixture.workspace.join("readme.txt"), "hello").expect("readme");
    let new_file = fixture.workspace.join("nested").join("new.txt");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            ("call_read", "Read", json!({ "path": "readme.txt" })),
            (
                "call_write",
                "Write",
                json!({ "path": "nested/new.txt", "content": "created" }),
            ),
            ("call_grep", "Grep", json!({ "pattern": "hello" })),
        ]),
        end_turn_events("done"),
    ]);
    let observed_approvals = Arc::new(Mutex::new(Vec::new()));
    let config = preflight_config(&fixture, &harness)
        .with_permission_mode(PermissionMode::Ask)
        .with_approval_handler({
            let observed_approvals = Arc::clone(&observed_approvals);
            move |request| {
                observed_approvals
                    .lock()
                    .expect("approvals lock")
                    .push(request.clone());
                permit_once(request)
            }
        });
    let runtime =
        AgentRuntime::with_tools(config, harness.client(), ToolRegistry::with_builtin_tools());
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "mixed batch").await;

    // Zero tools executed: the write left no file and no tool ever started.
    assert!(
        !new_file.exists(),
        "a deferred write must not create the file"
    );
    assert!(
        !has_tool_started(&events),
        "no tool may start in a deferred batch: {events:?}"
    );
    assert!(
        observed_approvals
            .lock()
            .expect("approvals lock")
            .is_empty(),
        "instruction preflight precedes every permission prompt"
    );
    assert!(
        event_index(&events, |event| matches!(
            event,
            AgentEvent::ApprovalRequested { .. }
        ))
        .is_none(),
        "no approval prompt may appear in a deferred batch: {events:?}"
    );

    // Every call in the batch received a matching non-error deferred result.
    for id in ["call_read", "call_write", "call_grep"] {
        let results = finished_tool_results(&events, id);
        assert_eq!(results.len(), 1, "{id}");
        assert!(
            !results[0].is_error,
            "{id} deferred result must be non-error"
        );
        let details = results[0].details.as_ref().expect("deferred details");
        assert_eq!(details["status"], "deferred", "{id}");
        assert_eq!(details["side_effect_occurred"], false, "{id}");
    }
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus one activation epoch: {events:?}"
    );
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Activated);
}

#[tokio::test]
async fn first_read_write_and_nested_cwd_shell_each_defer_before_execution() {
    let cases: Vec<(&str, serde_json::Value, Option<&str>)> = vec![
        ("Read", json!({ "path": "nested/data.txt" }), None),
        (
            "Write",
            json!({ "path": "nested/created.txt", "content": "created" }),
            Some("nested/created.txt"),
        ),
        (
            "Bash",
            json!({ "command": "printf changed > marker.txt", "cwd": "nested" }),
            Some("nested/marker.txt"),
        ),
        (
            "Terminal",
            json!({ "mode": "start", "command": "printf hi", "cwd": "nested" }),
            None,
        ),
    ];
    for (tool, arguments, marker) in cases {
        let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
        std::fs::write(fixture.workspace.join("nested").join("data.txt"), "body")
            .expect("data file");
        let harness = FakeHarness::from_turns([
            tool_call_turn(&[("call_1", tool, arguments)]),
            end_turn_events("done"),
        ]);
        let runtime = preflight_runtime(&fixture, &harness);
        let mut context = preflight_context(&fixture);

        let events = run_turn_collect(&runtime, &mut context, "first contact").await;

        let results = finished_tool_results(&events, "call_1");
        assert_eq!(results.len(), 1, "{tool}");
        assert!(
            !results[0].is_error,
            "{tool} deferred result must be non-error"
        );
        assert_eq!(
            results[0].details.as_ref().expect("deferred details")["status"],
            "deferred",
            "{tool}"
        );
        assert!(
            !has_tool_started(&events),
            "{tool} must not start in a deferred batch: {events:?}"
        );
        assert!(
            event_index(&events, |event| matches!(
                event,
                AgentEvent::ShellCommandStarted { .. }
            ))
            .is_none(),
            "{tool} shell side effect must not start: {events:?}"
        );
        if let Some(marker) = marker {
            assert!(
                !fixture.workspace.join(marker).exists(),
                "{tool} deferred side effect must not exist"
            );
        }
        let epochs = instruction_epochs(&events);
        assert_eq!(epochs.len(), 2, "{tool}: baseline plus activation epoch");
        assert_eq!(
            epochs[1].outcome,
            InstructionEpochOutcome::Activated,
            "{tool}"
        );
    }
}

fn assert_stale_approval_deferred(events: &[AgentEvent], new_file: &std::path::Path) {
    assert!(
        !new_file.exists(),
        "an approval taken against stale instructions must defer, not execute"
    );
    assert!(
        !has_tool_started(events),
        "the deferred write must not start: {events:?}"
    );
    let write_results = finished_tool_results(events, "write_1");
    assert_eq!(write_results.len(), 1, "write_1");
    assert!(
        !write_results[0].is_error,
        "the recheck defer is a non-error result"
    );
    assert_eq!(
        write_results[0].details.as_ref().expect("details")["status"],
        "deferred"
    );
    let epochs = instruction_epochs(events);
    assert_eq!(
        epochs.len(),
        1,
        "exactly one Updated epoch after the approval: {events:?}"
    );
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Updated);
    assert_eq!(epochs[0].deferred_tool_ids, vec!["write_1".to_owned()]);

    let approval_index = event_index(events, |event| {
        matches!(event, AgentEvent::ApprovalRequested { .. })
    })
    .expect("approval event");
    let epoch_index = event_index(events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("epoch event");
    assert!(
        approval_index < epoch_index,
        "preflight approved first, the changed source rechecked to defer: {events:?}"
    );
}

#[tokio::test]
async fn approval_wait_rechecks_instruction_fingerprint_before_execution() {
    let fixture = preflight_fixture(&[("nested", "nested rules v1\n")], "root rules\n");
    std::fs::write(fixture.workspace.join("nested").join("data.txt"), "body").expect("data");

    // Turn 1: activate the nested scope through a deferred-then-retried Read.
    let first_harness = FakeHarness::from_turns([
        tool_call_turn(&[("read_1", "Read", json!({ "path": "nested/data.txt" }))]),
        tool_call_turn(&[("read_2", "Read", json!({ "path": "nested/data.txt" }))]),
        end_turn_events("read done"),
    ]);
    let first_runtime = preflight_runtime(&fixture, &first_harness);
    let mut context = preflight_context(&fixture);
    let first_events = run_turn_collect(&first_runtime, &mut context, "read the file").await;
    assert_eq!(
        instruction_epochs(&first_events).len(),
        2,
        "baseline plus activation in turn 1: {first_events:?}"
    );

    // Turn 2: the Write needs approval in ask mode; the instruction source
    // changes while the dialog waits, so approval must defer, not execute.
    let new_file = fixture.workspace.join("nested").join("approved.txt");
    let second_harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "write_1",
            "Write",
            json!({ "path": "nested/approved.txt", "content": "approved" }),
        )]),
        end_turn_events("done"),
    ]);
    let (decision_sender, decision_receiver) = oneshot::channel::<ApprovalResponse>();
    let decision_receiver = Arc::new(Mutex::new(Some(decision_receiver)));
    let config = preflight_config(&fixture, &second_harness)
        .with_permission_mode(PermissionMode::Ask)
        .with_async_approval_handler(move |_request| {
            let receiver = decision_receiver
                .lock()
                .expect("decision receiver lock")
                .take()
                .expect("single approval decision receiver");
            async move { receiver.await.expect("approval decision") }
        });
    let second_runtime = AgentRuntime::with_tools(
        config,
        second_harness.client(),
        ToolRegistry::with_builtin_tools(),
    );

    let mut stream = second_runtime.run_turn(&mut context, AgentMessage::user_text("write a file"));
    let mut events = Vec::new();
    loop {
        let event = timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("approval dialog should be pending")
            .expect("stream open")
            .expect("event ok");
        let is_approval = matches!(event, AgentEvent::ApprovalRequested { .. });
        events.push(event);
        if is_approval {
            break;
        }
    }
    // The source changes after preflight but before approval completes.
    std::fs::write(
        fixture.workspace.join("nested").join("AGENTS.md"),
        "nested rules v2\n",
    )
    .expect("mutate AGENTS.md");
    decision_sender
        .send(ApprovalResponse::Selected {
            request_id: "write_1".to_owned(),
            action: ApprovalAction::PermitOnce,
            feedback: None,
        })
        .expect("send decision");
    while let Some(event) = stream.next().await {
        events.push(event.expect("event ok"));
    }

    // The epoch lands after the approval resolution, before the next request.
    assert_stale_approval_deferred(&events, &new_file);
}

#[tokio::test]
async fn blocked_scope_allows_read_only_diagnosis_but_blocks_mixed_mutation_batch() {
    let fixture = preflight_fixture(
        &[("nested", "@missing-rules.md\nnested body\n")],
        "root rules\n",
    );
    let target = fixture.workspace.join("nested").join("data.txt");
    std::fs::write(&target, "body").expect("data file");
    let edit_arguments = json!({
        "path": target.to_string_lossy(),
        "old": "body",
        "new": "changed"
    });
    let read_arguments = json!({ "path": target.to_string_lossy() });
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[("edit_1", "Edit", edit_arguments.clone())]),
        tool_call_turn(&[("read_1", "Read", read_arguments.clone())]),
        tool_call_turn(&[
            ("read_2", "Read", read_arguments),
            ("edit_2", "Edit", edit_arguments),
        ]),
        end_turn_events("done"),
    ]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "work in nested").await;

    // Exactly two epochs: the Ready baseline and one Blocked epoch. The same
    // failure is injected once even though three batches touched the scope.
    let epochs = instruction_epochs(&events);
    assert_eq!(
        epochs.len(),
        2,
        "baseline plus one blocked epoch: {events:?}"
    );
    assert_eq!(epochs[1].outcome, InstructionEpochOutcome::Blocked);
    let failure = epochs[1].failure.as_ref().expect("blocked failure");
    assert_eq!(failure.kind, InstructionFailureKind::MissingImport);
    assert_eq!(epochs[1].deferred_tool_ids, vec!["edit_1".to_owned()]);

    // The first Edit blocked with a structured result and no side effect.
    let blocked_first = finished_tool_results(&events, "edit_1");
    assert_eq!(blocked_first.len(), 1, "edit_1");
    assert!(blocked_first[0].is_error, "edit_1 must be blocked");
    let details = blocked_first[0].details.as_ref().expect("blocked details");
    assert_eq!(details["status"], "blocked");
    assert_eq!(details["reason"], "instruction_scope_blocked");
    assert_eq!(details["side_effect_occurred"], false);
    assert_eq!(details["failure"]["kind"], "missing import");

    // Read-only diagnosis proceeds once the failure epoch is visible.
    let read_results = finished_tool_results(&events, "read_1");
    assert_eq!(read_results.len(), 1, "read_1");
    assert!(
        !read_results[0].is_error,
        "read-only diagnosis must proceed: {}",
        read_results[0].content
    );
    assert!(read_results[0].content.contains("body"));

    // A mixed batch afterwards blocks as a whole without a third epoch.
    for id in ["read_2", "edit_2"] {
        let results = finished_tool_results(&events, id);
        assert_eq!(results.len(), 1, "{id}");
        assert!(results[0].is_error, "{id} must block in a mixed batch");
        assert_eq!(
            results[0].details.as_ref().expect("details")["status"],
            "blocked",
            "{id}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&target).expect("read target"),
        "body",
        "no blocked call may produce a side effect"
    );

    // Event order proof: no tool started before the blocked epoch; exactly
    // one tool (the diagnostic read) started after it.
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { epoch } if epoch.outcome == InstructionEpochOutcome::Blocked)
    })
    .expect("blocked epoch index");
    let started: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, AgentEvent::ToolExecutionStarted { .. }).then_some(index)
        })
        .collect();
    assert_eq!(
        started.len(),
        1,
        "only the diagnostic read may start: {events:?}"
    );
    assert!(started[0] > epoch_index);
}

#[tokio::test]
async fn baseline_epoch_precedes_first_user_message_for_new_and_legacy_sessions() {
    // New session: empty context establishes a Ready baseline first.
    let fixture = preflight_fixture(&[], "root rules\n");
    let harness = FakeHarness::from_turns([end_turn_events("hello")]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = preflight_context(&fixture);

    let events = run_turn_collect(&runtime, &mut context, "first prompt").await;

    let epochs = instruction_epochs(&events);
    assert_eq!(epochs.len(), 1, "one baseline epoch: {events:?}");
    assert_eq!(epochs[0].outcome, InstructionEpochOutcome::Ready);
    assert!(epochs[0].deferred_tool_ids.is_empty());
    let epoch_index = event_index(&events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch index");
    let user_index = event_index(&events, |event| {
        matches!(
            event,
            AgentEvent::MessageAppended {
                message: AgentMessage::User { .. }
            }
        )
    })
    .expect("user message index");
    assert!(
        epoch_index < user_index,
        "the baseline epoch must precede the first user message: {events:?}"
    );
    let instruction_position = context
        .messages()
        .iter()
        .position(|message| message.is_injection_variant("instruction_epoch"))
        .expect("pinned instruction message");
    let user_position = context
        .messages()
        .iter()
        .position(
            |message| matches!(message, AgentMessage::User { origin, .. } if origin.is_user()),
        )
        .expect("user message");
    assert!(
        instruction_position < user_position,
        "the pinned baseline must precede the user message in context"
    );

    // Legacy resume: replayed context without any epoch gets a fresh baseline
    // before the new user message, not a reconstructed legacy behavior.
    let legacy_fixture = preflight_fixture(&[], "root rules\n");
    let legacy_harness = FakeHarness::from_turns([end_turn_events("hi again")]);
    let legacy_runtime = preflight_runtime(&legacy_fixture, &legacy_harness);
    let mut legacy = AgentContext::from_replay(
        [
            AgentEvent::MessageAppended {
                message: AgentMessage::user_text("legacy prompt"),
            },
            AgentEvent::MessageAppended {
                message: AgentMessage::assistant(
                    [Content::text("legacy answer")],
                    Vec::new(),
                    StopReason::EndTurn,
                ),
            },
            AgentEvent::TurnFinished {
                turn: 1,
                stop_reason: StopReason::EndTurn,
            },
        ]
        .iter(),
    );
    assert_eq!(legacy.instruction_state().visible_generation, 0);
    let legacy_events = run_turn_collect(&legacy_runtime, &mut legacy, "next prompt").await;

    let legacy_epochs = instruction_epochs(&legacy_events);
    assert_eq!(
        legacy_epochs.len(),
        1,
        "exactly one fresh baseline for the legacy session: {legacy_events:?}"
    );
    assert_eq!(legacy_epochs[0].outcome, InstructionEpochOutcome::Ready);
    let legacy_epoch_index = event_index(&legacy_events, |event| {
        matches!(event, AgentEvent::InstructionEpoch { .. })
    })
    .expect("baseline epoch index");
    let new_user_index = legacy_events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::MessageAppended {
                message: AgentMessage::User { content, .. },
            } if content
                .iter()
                .filter_map(Content::as_text)
                .collect::<String>()
                == "next prompt" =>
            {
                Some(index)
            }
            _ => None,
        })
        .expect("new user message index");
    assert!(
        legacy_epoch_index < new_user_index,
        "the baseline must precede the new user message on legacy resume: {legacy_events:?}"
    );
}

#[tokio::test]
async fn instruction_preflight_defers_whole_multi_edit_call_batch_for_one_new_scope() {
    let fixture = preflight_fixture(&[("nested", "nested rules\n")], "root rules\n");
    let nested_file = fixture.workspace.join("nested").join("a.txt");
    let root_file = fixture.workspace.join("root.txt");
    std::fs::write(&nested_file, "nested\n").expect("nested");
    std::fs::write(&root_file, "root\n").expect("root");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[
            (
                "edit_root",
                "Edit",
                json!({
                    "path": root_file.to_string_lossy(),
                    "old": "root",
                    "new": "ROOT"
                }),
            ),
            (
                "edit_nested",
                "Edit",
                json!({
                    "path": nested_file.to_string_lossy(),
                    "old": "nested",
                    "new": "NESTED"
                }),
            ),
        ]),
        end_turn_events("done"),
    ]);
    let runtime = preflight_runtime(&fixture, &harness);
    let mut context = AgentContext::new();
    let events = run_turn_collect(&runtime, &mut context, "edit across scopes").await;

    for id in ["edit_root", "edit_nested"] {
        let finished = finished_tool_results(&events, id);
        assert_eq!(finished.len(), 1);
        assert!(!finished[0].is_error);
        let details = finished[0].details.as_ref().expect("details");
        assert_eq!(details["status"], "deferred");
        assert_eq!(details["side_effect_occurred"], false);
    }
    assert_eq!(
        std::fs::read_to_string(&nested_file).expect("nested"),
        "nested\n"
    );
    assert_eq!(std::fs::read_to_string(&root_file).expect("root"), "root\n");
}
