use super::compaction_rehydration::instruction_fixture;
use super::compaction_rehydration::reconcile_defer_epoch;
use super::fake_harness::DelayedHarness;
use super::fake_harness::DelayedStep;
use super::fake_harness::collect_turn_events;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, AgentToolCall,
    CompactionSettings, Content, InstructionContextBridge, StopReason, Tool, ToolContext,
    ToolExecutionMode, ToolFuture, ToolRegistry, ToolResult, harness::FakeHarness,
    instructions::InstructionEpochOutcome,
};
use neo_ai::{AiError, AiStreamEvent, MessagePhase};
use serde_json::json;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn runtime_can_compact_again_after_context_grows_past_threshold() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the first compaction
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_compact_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nFirst compaction.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "second answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Compaction summary call for the second compaction
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_compact_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nSecond compaction.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_3".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "third answer".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    let mut compactions = Vec::new();

    for prompt in [
        "first long prompt that seeds compaction",
        "second long prompt that triggers compaction",
        "third long prompt that should trigger compaction again",
    ] {
        let events = runtime
            .run_turn(&mut context, AgentMessage::user_text(prompt))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("turn should succeed");
        compactions.extend(events.into_iter().filter_map(|event| match event {
            AgentEvent::CompactionApplied { summary } => Some(summary),
            _ => None,
        }));
    }

    assert_eq!(
        compactions.len(),
        2,
        "context should compact again after later turns grow past the threshold. Messages: {:?}",
        context.messages().len()
    );
    // After the second compaction, the context should contain:
    // 1. The injected compaction summary system message
    // 2. The third user message
    // 3. The third assistant response
    assert_eq!(context.messages().len(), 3);
    assert!(matches!(
        context.messages().first(),
        Some(AgentMessage::System { .. })
    ));
    assert_eq!(context.compaction_summary(), compactions.last());
}

pub(crate) fn text_turn_events(id: &str, text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: id.to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

fn compaction_lifecycle(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count,
            } => Some(format!("start:{reason:?}:{tokens_before}:{message_count}")),
            AgentEvent::CompactionProgress { phase, percent } => {
                Some(format!("progress:{phase:?}:{percent}"))
            }
            AgentEvent::CompactionApplied { summary } => Some(format!(
                "applied:{}:{}",
                summary.first_kept_message_index, summary.tokens_before
            )),
            _ => None,
        })
        .collect()
}

fn assert_compaction_lifecycle(lifecycle: &[String]) {
    assert_eq!(lifecycle.first(), Some(&"start:Threshold:29:3".to_owned()));
    assert!(lifecycle.contains(&"progress:Estimating:0".to_owned()));
    assert!(lifecycle.contains(&"progress:SelectingBoundary:15".to_owned()));
    assert!(lifecycle.contains(&"progress:Summarizing:15".to_owned()));
    assert!(
        lifecycle
            .iter()
            .any(|e| e.starts_with("progress:Summarizing:") && e != "progress:Summarizing:15"),
        "Summarizing should make progress beyond its starting percent: {lifecycle:?}"
    );
    assert_eq!(
        lifecycle.iter().rfind(|e| e.starts_with("progress:")),
        Some(&"progress:Applying:100".to_owned()),
        "last progress should reach 100%: {lifecycle:?}"
    );
    assert!(lifecycle.contains(&"applied:2:29".to_owned()));

    let percents: Vec<u8> = lifecycle
        .iter()
        .filter_map(|e| {
            e.strip_prefix("progress:")
                .and_then(|rest| rest.split(':').next_back().and_then(|p| p.parse().ok()))
        })
        .collect();
    assert!(
        percents.windows(2).all(|w| w[0] <= w[1]),
        "progress percents should be monotonic: {percents:?}"
    );
}

#[tokio::test]
async fn runtime_emits_compaction_lifecycle_events_before_applying_summary() {
    let harness = FakeHarness::from_turns([
        text_turn_events("msg_1", "first answer"),
        text_turn_events(
            "msg_compact",
            "## Current Focus\nWorking on compaction test.",
        ),
        text_turn_events("msg_2", "second answer"),
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(4, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("first long prompt that seeds compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn should succeed");

    let events = runtime
        .run_turn(
            &mut context,
            AgentMessage::user_text("second long prompt that triggers compaction"),
        )
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn should succeed");

    // Verify the lifecycle starts at 0%, goes through the visible phases, and
    // finishes smoothly at 100% instead of jumping from ~80% to done.
    assert_compaction_lifecycle(&compaction_lifecycle(&events));
}

#[tokio::test]
async fn manual_compaction_streams_progress_before_summary_finishes() {
    let harness = DelayedHarness::new(vec![
        DelayedStep::Event(AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "summary".to_owned(),
        }),
        DelayedStep::Delay(Duration::from_secs(5)),
        DelayedStep::Event(AiStreamEvent::TextDelta {
            text: "summary".to_owned(),
        }),
        DelayedStep::Event(AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        }),
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_compaction(CompactionSettings::new(usize::MAX, 1)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("first"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("second")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    context.append_message(AgentMessage::user_text("third"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("fourth")],
        Vec::new(),
        StopReason::EndTurn,
    ));

    let mut stream = runtime.run_manual_compaction_turn(&mut context);
    assert!(matches!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("run start should stream")
            .expect("run start event")
            .expect("run start should be ok"),
        AgentEvent::RunStarted { .. }
    ));
    assert!(matches!(
        timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("compaction start should stream before the delayed summary")
            .expect("compaction start event")
            .expect("compaction start should be ok"),
        AgentEvent::CompactionStarted { .. }
    ));
}

#[tokio::test]
async fn runtime_context_window_events_share_budget_snapshot() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "done".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(4_000)));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(1_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 4));
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let update = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ContextWindowUpdated {
                used_tokens,
                projected_tokens,
                trigger_tokens,
                ..
            } => Some((*used_tokens, *projected_tokens, *trigger_tokens)),
            _ => None,
        })
        .expect("context update");
    assert!(update.0 > 0);
    assert!(update.1.is_some());
    assert!(update.2.is_some());
}

#[tokio::test]
async fn runtime_compacts_before_model_call_when_resume_exceeds_window() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "resumed".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history ".repeat(40_000)));
    context.append_message(AgentMessage::assistant(
        [Content::text("previous answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config =
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 1));
    config.model.capabilities.max_context_tokens = Some(32_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let assistant = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. }
                }
            )
        })
        .expect("assistant");
    assert!(compaction < assistant);
}

#[tokio::test]
async fn runtime_overflow_records_observed_window_and_retries_once() {
    let harness = FakeHarness::from_result_turns([
        vec![Err(AiError::ContextOverflow {
            message: "too many tokens".to_owned(),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
        vec![Err(AiError::RateLimit {
            message: "retry compacted request".to_owned(),
            retry_after: Some(Duration::ZERO),
        })],
        vec![
            Ok(AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "retry".to_owned(),
            }),
            Ok(AiStreamEvent::TextDelta {
                text: "recovered".to_owned(),
            }),
            Ok(AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            }),
        ],
    ]);
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("history"));
    context.append_message(AgentMessage::assistant(
        [Content::text("old answer")],
        Vec::new(),
        StopReason::EndTurn,
    ));
    let mut config = AgentConfig::for_model(harness.model())
        .with_system_prompt("system ".repeat(4_000))
        .with_compaction(CompactionSettings::new(usize::MAX, 1));
    config.max_retries = 1;
    config.model.capabilities.max_context_tokens = Some(200_000);

    let events = collect_turn_events(
        &harness,
        config,
        &mut context,
        AgentMessage::user_text("continue"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(
        serde_json::to_value(&requests[2]).expect("serialize compacted request"),
        serde_json::to_value(&requests[3]).expect("serialize retried compacted request")
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
    );
    let observed_max = events.iter().find_map(|event| match event {
        AgentEvent::ContextWindowUpdated {
            max_tokens: Some(max_tokens),
            source: Some(neo_agent_core::ContextWindowSource::ObservedOverflow),
            ..
        } => Some(*max_tokens),
        _ => None,
    });
    assert!(observed_max.is_some_and(|max| max > 1_000));
    assert!(events.contains(&AgentEvent::RetrySucceeded {
        turn: 1,
        retries_used: 1,
    }));
}

#[tokio::test]
async fn runtime_does_not_compact_mid_parallel_tool_group() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "a".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "a".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "b".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "b".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "c".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "c".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after tools".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = runtime_with_large_tool(&harness);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let last_tool_result = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::ToolResult { .. }
                }
            )
        })
        .expect("tool result");
    let first_compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    assert!(first_compaction > last_tool_result);
}

#[tokio::test]
async fn runtime_compacts_after_parallel_tool_group_before_followup() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "a".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "a".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "b".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "b".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::ToolCallStart {
                id: "c".to_owned(),
                name: "LargeTool".to_owned(),
            },
            AiStreamEvent::ToolCallEnd {
                id: "c".to_owned(),
                raw_arguments: "{}".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::ToolUse,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_2".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after compaction".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = runtime_with_large_tool(&harness);
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("use tools"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let compaction = events
        .iter()
        .position(|event| matches!(event, AgentEvent::CompactionApplied { .. }))
        .expect("compaction");
    let second_assistant = events
        .iter()
        .rposition(|event| {
            matches!(
                event,
                AgentEvent::MessageAppended {
                    message: AgentMessage::Assistant { .. }
                }
            )
        })
        .expect("assistant");
    assert!(compaction < second_assistant);
}

#[tokio::test]
async fn runtime_compaction_keeps_valid_tool_result_boundaries() {
    let harness = FakeHarness::from_turns([
        // Compaction summary call
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_compact".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "## Current Focus\nInspecting files.".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
        // Actual turn response
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_after_compaction".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "after compaction".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 3)),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("inspect"));
    context.append_message(AgentMessage::assistant(
        [],
        [
            AgentToolCall {
                id: "tool_1".into(),
                name: "Read".into(),
                raw_arguments: json!({ "path": "a.rs" }).to_string().into(),
            },
            AgentToolCall {
                id: "tool_2".into(),
                name: "List".into(),
                raw_arguments: json!({ "path": "src" }).to_string().into(),
            },
        ],
        StopReason::ToolUse,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_1",
        "Read",
        [Content::text("large content")],
        false,
    ));
    context.append_message(AgentMessage::tool_result(
        "tool_2",
        "List",
        [Content::text("file list")],
        false,
    ));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("continue"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(
        !matches!(
            request.messages.first(),
            Some(neo_ai::ChatMessage::ToolResult { .. })
        ),
        "compaction must not keep orphaned tool results at the start of replay"
    );
    // The first message is now either the compaction summary system message or
    // the user prompt — never an orphaned tool result.
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { .. } | neo_ai::ChatMessage::User { .. })
    ));
}

fn runtime_with_large_tool(harness: &FakeHarness) -> AgentRuntime {
    let mut registry = ToolRegistry::new();
    registry.register(LargeTool);
    let config = AgentConfig::for_model(harness.model())
        .with_tool_execution_mode(ToolExecutionMode::Parallel)
        .with_compaction(CompactionSettings::new(1, 1));
    AgentRuntime::with_tools(config, harness.client(), registry)
}

struct LargeTool;

impl Tool for LargeTool {
    fn name(&self) -> &'static str {
        "LargeTool"
    }

    fn description(&self) -> &'static str {
        "Returns a large payload."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    fn execute<'a>(&'a self, _ctx: &'a ToolContext, _input: serde_json::Value) -> ToolFuture<'a> {
        Box::pin(async { Ok(ToolResult::ok("tool output ".repeat(20_000))) })
    }
}

pub(crate) fn end_turn_events(text: &str) -> Vec<AiStreamEvent> {
    vec![
        AiStreamEvent::TextDelta {
            text: text.to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]
}

#[tokio::test]
async fn compacted_current_nested_scope_removal_emits_removed_epoch() {
    let fixture = instruction_fixture(&[("nested", "NESTED-RULES\n")], "ROOT-RULES\n");
    let nested = fixture.workspace.join("nested");
    let config = AgentConfig::for_model(neo_agent_core::harness::fake_model());
    let mut context = AgentContext::new();
    let (epoch, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![nested.clone()]).await;
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);
    InstructionContextBridge::rehydrate_after_compaction(&fixture.registry, &mut context)
        .await
        .expect("rehydration");
    assert_eq!(
        context.instruction_state().most_recent_scope.as_deref(),
        Some(nested.as_path())
    );
    assert!(!context.instruction_state().active_scopes.contains(&nested));
    std::fs::remove_file(nested.join("AGENTS.md")).expect("remove nested AGENTS.md");

    let (removed, fingerprint) =
        reconcile_defer_epoch(&fixture, &config, &context, vec![nested.clone()]).await;

    assert_eq!(removed.outcome, InstructionEpochOutcome::Removed);
    InstructionContextBridge::apply_epoch(&mut context, &removed, &fingerprint);
    assert!(
        !context
            .instruction_state()
            .visited_revisions
            .contains_key(&nested),
        "{removed:?}"
    );
}
