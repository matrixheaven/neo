use super::compaction::end_turn_events;
use super::compaction_rehydration::instruction_fixture;
use super::compaction_rehydration::reconcile_defer_epoch;
use super::fake_harness::collect_turn_events;
use super::fake_harness::final_done_turn;
use super::fake_harness::run_turn_collect;
use super::fake_harness::tool_call_turn;
use futures::StreamExt;
use neo_agent_core::{
    AgentConfig, AgentContext, AgentEvent, AgentMessage, AgentRuntime, InstructionContextBridge,
    PermissionMode, ToolRegistry, ToolResult,
    harness::FakeHarness,
    instructions::{InstructionEpochData, InstructionEpochOutcome},
    skills::{SkillStore, SkillStoreHandle},
};
use neo_ai::{AiStreamEvent, ChatMessage, ChatRequest, ContentPart, MessagePhase, ToolSpec};
use serde_json::json;

#[tokio::test]
async fn unchanged_session_keeps_cache_prefix_and_new_context_appends() {
    let harness = FakeHarness::from_turns([
        vec![
            AiStreamEvent::MessageStart {
                phase: MessagePhase::Unknown,
                id: "msg_1".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "first reply".to_owned(),
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
                text: "second reply".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                phase: MessagePhase::Unknown,
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ],
    ]);
    let config = AgentConfig::for_model(harness.model());
    let mut context = AgentContext::new();

    collect_turn_events(
        &harness,
        config.clone(),
        &mut context,
        AgentMessage::user_text("first prompt"),
    )
    .await;
    collect_turn_events(
        &harness,
        config.clone(),
        &mut context,
        AgentMessage::user_text("second prompt"),
    )
    .await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2, "one request per turn");
    let first = &requests[0];
    let second = &requests[1];

    // Context integrity invariant: the unchanged session keeps a byte-identical
    // cache prefix — every message of the first request reappears in the second
    // request as the same leading bytes, serialized exactly as the provider
    // sees them.
    let first_messages = first
        .messages
        .iter()
        .map(|message| serde_json::to_vec(message).expect("serialize message"))
        .collect::<Vec<_>>();
    let second_messages = second
        .messages
        .iter()
        .map(|message| serde_json::to_vec(message).expect("serialize message"))
        .collect::<Vec<_>>();
    assert!(
        second_messages.len() >= first_messages.len()
            && second_messages[..first_messages.len()] == first_messages[..],
        "cache prefix changed\nfirst: {}\nsecond: {}",
        String::from_utf8_lossy(&serde_json::to_vec(&first.messages).expect("serialize")),
        String::from_utf8_lossy(&serde_json::to_vec(&second.messages).expect("serialize"))
    );

    // New canonical messages append after the prefix in event order: the
    // assistant reply to the first prompt, then the second user prompt.
    assert_eq!(second.messages.len(), first.messages.len() + 2);
    let appended = &second.messages[first.messages.len()..];
    assert_eq!(
        appended,
        [
            ChatMessage::Assistant {
                content: vec![ContentPart::Text {
                    text: "first reply".to_owned(),
                }],
                tool_calls: Vec::new(),
            },
            ChatMessage::User {
                content: vec![ContentPart::Text {
                    text: "second prompt".to_owned(),
                }],
            },
        ]
    );
}

#[tokio::test]
async fn runtime_injects_workspace_context_into_model_request() {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "ok".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_workspace_root(workspace.path())
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    runtime
        .run_turn(&mut context, AgentMessage::user_text("where am I?"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let request = harness.requests().pop().expect("model request");
    assert!(matches!(
        request.messages.first(),
        Some(neo_ai::ChatMessage::System { content })
            if content.iter().any(|part| matches!(
                part,
                neo_ai::ContentPart::Text { text }
                    if text.contains("Runtime Context")
                        && text.contains("- cwd:")
                        && text.contains(&workspace_root.display().to_string())
                        && text.contains("Do not prefix shell commands with `cd")
            ))
    ));
}

#[tokio::test]
async fn runtime_context_window_estimate_includes_effective_request_messages() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace_root = temp.path().join("workspace");
    std::fs::create_dir(&workspace_root).expect("workspace dir");
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model())
            .with_system_prompt("system prompt that must count toward context")
            .with_workspace_root(workspace_root)
            .expect("workspace root"),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("short"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::ContextWindowUpdated {
                turn: 1,
                used_tokens,
                ..
            } if *used_tokens > 20
        )),
        "context estimate should include system/workspace request messages, not only the user buffer"
    );
}

#[tokio::test]
async fn runtime_context_window_estimate_includes_tool_schemas() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let tool = ToolSpec {
        name: "LargeSchemaTool".to_owned(),
        description: "tool description that must count toward context".repeat(8),
        input_schema: json!({
            "type": "object",
            "properties": {
                "payload": {
                    "type": "string",
                    "description": "schema description that must count toward context".repeat(16),
                },
            },
            "required": ["payload"],
            "additionalProperties": false,
        }),
    };
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_tools(vec![tool]),
        harness.client(),
    );
    let mut context = AgentContext::new();

    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("x"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    let used_tokens = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ContextWindowUpdated { used_tokens, .. } => Some(*used_tokens),
            _ => None,
        })
        .expect("context window update");

    assert!(
        used_tokens > 100,
        "context estimate should include tool name, description, and input schema; got {used_tokens}"
    );
}

#[tokio::test]
async fn runtime_applies_context_append_transform_before_model_request() {
    let harness = FakeHarness::from_events([
        AiStreamEvent::MessageStart {
            phase: MessagePhase::Unknown,
            id: "msg_1".to_owned(),
        },
        AiStreamEvent::TextDelta {
            text: "trimmed".to_owned(),
        },
        AiStreamEvent::MessageEnd {
            phase: MessagePhase::Unknown,
            stop_reason: neo_ai::StopReason::EndTurn,
            usage: None,
        },
    ]);
    let runtime = AgentRuntime::new(
        AgentConfig::for_model(harness.model()).with_context_append_transform(|messages| {
            vec![AgentMessage::system_reminder(format!(
                "append-only transform saw {} messages",
                messages.len()
            ))]
        }),
        harness.client(),
    );
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("drop"));

    runtime
        .run_turn(&mut context, AgentMessage::user_text("keep"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    assert_eq!(harness.requests()[0].messages.len(), 3);
    assert!(matches!(
        &harness.requests()[0].messages[0],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "drop"
        )
    ));
    assert!(matches!(
        &harness.requests()[0].messages[1],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text == "keep"
        )
    ));
    assert!(matches!(
        &harness.requests()[0].messages[2],
        neo_ai::ChatMessage::User { content } if matches!(
            content.first(),
            Some(neo_ai::ContentPart::Text { text }) if text.contains("append-only transform saw")
        )
    ));
    assert_eq!(context.messages()[0], AgentMessage::user_text("drop"));
    assert_eq!(context.messages()[1], AgentMessage::user_text("keep"));
}

#[tokio::test]
async fn runtime_appends_available_skills_snapshot_only_when_changed() {
    let skills_dir = tempfile::tempdir().expect("skills dir");
    for (name, description) in [("zeta", "Zeta skill"), ("alpha", "Alpha skill")] {
        let dir = skills_dir.path().join(name);
        std::fs::create_dir_all(&dir).expect("create skill dir");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nBody.\n"),
        )
        .expect("write skill");
    }
    let initial = SkillStore::load(&[], &[skills_dir.path().to_path_buf()], Vec::new());
    let handle = SkillStoreHandle::new(initial);
    let harness =
        FakeHarness::from_turns([final_done_turn(), final_done_turn(), final_done_turn()]);
    let runtime = AgentRuntime::with_tools_and_skill_handle(
        AgentConfig::for_model(harness.model()),
        harness.client(),
        ToolRegistry::new(),
        handle.clone(),
    );
    let mut context = AgentContext::new();

    let first = runtime
        .run_turn(&mut context, AgentMessage::user_text("first"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("first turn");

    std::fs::remove_dir_all(skills_dir.path().join("zeta")).expect("remove zeta");
    std::fs::write(
        skills_dir.path().join("alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Updated alpha skill\n---\nBody.\n",
    )
    .expect("update alpha");
    let beta = skills_dir.path().join("beta");
    std::fs::create_dir_all(&beta).expect("create beta");
    std::fs::write(
        beta.join("SKILL.md"),
        "---\nname: beta\ndescription: Beta skill\n---\nBody.\n",
    )
    .expect("write beta");
    handle.replace(SkillStore::load(
        &[],
        &[skills_dir.path().to_path_buf()],
        Vec::new(),
    ));

    let second = runtime
        .run_turn(&mut context, AgentMessage::user_text("second"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("second turn");
    let third = runtime
        .run_turn(&mut context, AgentMessage::user_text("third"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("third turn");

    let snapshot_count = |events: &[AgentEvent]| {
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::MessageAppended {
                        message: AgentMessage::User { origin, .. },
                    } if origin.is_injection_variant("available_skills")
                )
            })
            .count()
    };
    assert_eq!(snapshot_count(&first), 1);
    assert_eq!(snapshot_count(&second), 1);
    assert_eq!(snapshot_count(&third), 0);

    let snapshots = context
        .messages()
        .iter()
        .filter(|message| {
            matches!(
                message,
                AgentMessage::User { origin, .. }
                    if origin.is_injection_variant("available_skills")
            )
        })
        .map(AgentMessage::text)
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), 2);
    assert!(
        snapshots[0].find("- alpha:").expect("alpha listing")
            < snapshots[0].find("- zeta:").expect("zeta listing")
    );
    assert!(snapshots[1].contains("DISREGARD any earlier skill listings"));
    assert!(snapshots[1].contains("- alpha: Updated alpha skill"));
    assert!(snapshots[1].contains("- beta: Beta skill"));
    assert!(!snapshots[1].contains("- zeta:"));
}

#[tokio::test]
async fn adjacent_requests_keep_the_complete_previous_message_prefix() {
    let fixture = instruction_fixture(&[("nested", "nested rules\n")], "root rules\n");
    let harness =
        FakeHarness::from_turns([end_turn_events("reply one"), end_turn_events("reply two")]);
    let tool = ToolSpec {
        name: "Read".to_owned(),
        description: "read a file".to_owned(),
        input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
    };
    let config = AgentConfig::for_model(harness.model())
        .with_system_prompt("BASE SYSTEM PROMPT")
        .with_tools(vec![tool])
        .with_workspace_root(&fixture.workspace)
        .expect("workspace root")
        .with_session_directory(
            fixture
                .workspace
                .join("session_00000000-0000-4000-8000-0000000000aa"),
        );
    let runtime = AgentRuntime::new(config.clone(), harness.client());
    let mut context = AgentContext::new();

    run_turn_collect(&runtime, &mut context, "first request").await;

    // Activate the nested scope between the two provider requests.
    let (epoch, fingerprint) = reconcile_defer_epoch(
        &fixture,
        &config,
        &context,
        vec![fixture.workspace.join("nested")],
    )
    .await;
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Activated);
    InstructionContextBridge::apply_epoch(&mut context, &epoch, &fingerprint);

    run_turn_collect(&runtime, &mut context, "second request").await;

    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    let second = &requests[1];

    // The complete earlier message sequence is the exact prefix of the next
    // pre-compaction request; the epoch only appends.
    assert!(
        first.messages.len() < second.messages.len(),
        "the epoch and the follow-up exchange must append messages"
    );
    assert_eq!(
        first.messages.as_slice(),
        &second.messages[..first.messages.len()],
        "request N messages must be the exact prefix of request N+1"
    );

    // Stable system prompt bytes, tool ordering, reasoning settings, and the
    // session cache key across scope activation.
    let system_text = |request: &ChatRequest| match request.messages.first() {
        Some(neo_ai::ChatMessage::System { content }) => content
            .iter()
            .filter_map(|part| match part {
                neo_ai::ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>(),
        other => panic!("expected leading system message, got {other:?}"),
    };
    assert_eq!(system_text(first), system_text(second));
    assert_eq!(system_text(first), "BASE SYSTEM PROMPT");
    assert_eq!(first.tools, second.tools);
    assert_eq!(first.options.reasoning, second.options.reasoning);
    assert_eq!(first.options.session_id, second.options.session_id);
    assert_eq!(
        first.options.session_id.as_deref(),
        Some("session_00000000-0000-4000-8000-0000000000aa")
    );
}

pub(crate) fn instruction_epochs(events: &[AgentEvent]) -> Vec<&InstructionEpochData> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::InstructionEpoch { epoch } => Some(epoch),
            _ => None,
        })
        .collect()
}

pub(crate) fn finished_tool_results<'a>(events: &'a [AgentEvent], id: &str) -> Vec<&'a ToolResult> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolExecutionFinished {
                id: event_id,
                result,
                ..
            } if event_id == id => Some(result),
            _ => None,
        })
        .collect()
}

pub(crate) fn event_index(
    events: &[AgentEvent],
    predicate: impl Fn(&AgentEvent) -> bool,
) -> Option<usize> {
    events.iter().position(predicate)
}

fn collect_request_artifact_leaks(
    value: &serde_json::Value,
    keys: &mut Vec<String>,
    strings: &mut Vec<String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if matches!(
                    key.as_str(),
                    "output_ref" | "byte_len" | "line_count" | "complete" | "tool_output"
                ) {
                    keys.push(key.clone());
                }
                collect_request_artifact_leaks(child, keys, strings);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_request_artifact_leaks(item, keys, strings);
            }
        }
        serde_json::Value::String(text)
            if text.contains("agents/") || text.contains(".log") || text.contains(".idx") =>
        {
            strings.push(text.clone());
        }
        _ => {}
    }
}

#[tokio::test]
async fn display_output_never_enters_model_context() {
    let workspace = tempfile::tempdir().expect("workspace");
    let harness = FakeHarness::from_turns([
        tool_call_turn(&[(
            "tool_1",
            "Bash",
            json!({"command": "printf 'SECRET_DISPLAY_9f2a'"}),
        )]),
        end_turn_events("done"),
    ]);
    let runtime = AgentRuntime::with_tools(
        AgentConfig::for_model(harness.model())
            .with_permission_mode(PermissionMode::Yolo)
            .with_workspace_root(workspace.path())
            .expect("workspace config")
            .with_session_directory(workspace.path().join("session")),
        harness.client(),
        ToolRegistry::with_builtin_tools(),
    );
    let mut context = AgentContext::new();
    let events = runtime
        .run_turn(&mut context, AgentMessage::user_text("run bash"))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("turn should succeed");

    // The finished events carry one typed reference shared by the
    // `ToolExecutionFinished` and `ShellCommandFinished` projections.
    let finished_ref = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolExecutionFinished { id, output_ref, .. } if id == "tool_1" => {
                output_ref.as_ref()
            }
            _ => None,
        })
        .expect("finished bash event must carry the captured reference");
    let shell_ref = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ShellCommandFinished { id, output_ref, .. } if id == "tool_1" => {
                output_ref.as_ref()
            }
            _ => None,
        })
        .expect("shell finished event must carry the captured reference");
    assert_eq!(finished_ref, shell_ref, "one artifact per bash execution");
    assert!(finished_ref.complete, "{finished_ref:?}");
    assert!(finished_ref.byte_len > 0, "{finished_ref:?}");

    // Every request the model saw is byte-level free of the reference, its
    // metadata, and the artifact path. Cache-prefix input derives from these
    // same canonical messages, so it is covered by the same assertion.
    let requests = harness.requests();
    assert!(!requests.is_empty(), "model requests were recorded");
    let mut leaks_keys = Vec::new();
    let mut leaks_strings = Vec::new();
    for request in &requests {
        let serialized = serde_json::to_value(request).expect("serialize request");
        collect_request_artifact_leaks(&serialized, &mut leaks_keys, &mut leaks_strings);
    }
    assert!(
        leaks_keys.is_empty(),
        "request JSON must not carry output-reference keys: {leaks_keys:?}"
    );
    assert!(
        leaks_strings.is_empty(),
        "request JSON must not carry artifact paths: {leaks_strings:?}"
    );

    // The complete display text may only ride inside the bounded tool-result
    // preview, never in user/assistant/system text or tool arguments.
    let mut tool_result_texts = Vec::new();
    let mut other_texts = Vec::new();
    for request in &requests {
        for message in &request.messages {
            match message {
                neo_ai::ChatMessage::ToolResult { content, .. } => {
                    for part in content {
                        if let neo_ai::ContentPart::Text { text } = part {
                            tool_result_texts.push(text);
                        }
                    }
                }
                neo_ai::ChatMessage::User { content, .. }
                | neo_ai::ChatMessage::Assistant { content, .. } => {
                    for part in content {
                        if let neo_ai::ContentPart::Text { text } = part {
                            other_texts.push(text);
                        }
                    }
                }
                neo_ai::ChatMessage::System { .. } => {}
            }
        }
    }
    assert!(
        tool_result_texts
            .iter()
            .any(|text| text.contains("SECRET_DISPLAY_9f2a")),
        "the bounded preview must reach the model as the tool result"
    );
    assert!(
        !other_texts
            .iter()
            .any(|text| text.contains("SECRET_DISPLAY_9f2a")),
        "complete display text must not leak into any other message part"
    );
}
