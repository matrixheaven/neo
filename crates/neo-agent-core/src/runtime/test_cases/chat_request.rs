//! Chat request behavior (moved from `chat_request.rs`).

use neo_ai::{ApiKind, ChatMessage, ContentPart, ModelCapabilities, ModelSpec, ProviderId};

use super::*;
use crate::Content;
use crate::tools::ToolRegistry;

fn tool_model() -> ModelSpec {
    ModelSpec {
        provider: ProviderId("test".to_owned()),
        model: "tool-model".to_owned(),
        api: ApiKind::Local,
        capabilities: ModelCapabilities::tool_chat(),
    }
}

fn system_texts(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::System { content } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_result_texts(request: &ChatRequest) -> String {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            ChatMessage::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flat_map(|content| content.iter())
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn chat_request_disabled_projection_keeps_tool_result_content() {
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::assistant(
        Vec::new(),
        vec![crate::AgentToolCall {
            id: "call".into(),
            name: "Read".into(),
            raw_arguments: "{}".into(),
        }],
        crate::StopReason::ToolUse,
    ));
    context.append_message(AgentMessage::tool_result(
        "call",
        "Read",
        vec![Content::text("x".repeat(8_000))],
        false,
    ));
    let config = AgentConfig::for_model(tool_model())
        .with_compaction(crate::CompactionSettings::new(usize::MAX, 4));

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");

    assert!(tool_result_texts(&request).contains(&"x".repeat(100)));
}

#[tokio::test]
async fn chat_request_sends_tools_without_duplicate_system_schema_catalog() {
    let tools = ToolRegistry::with_builtin_tools().specs();
    let config = AgentConfig::for_model(tool_model())
        .with_system_prompt("Base system")
        .with_tools(tools.clone());
    let context = AgentContext::new();

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");
    let system_text = system_texts(&request);

    assert!(
        !system_text.contains("<available_tools_schema>"),
        "{system_text}"
    );
    assert_eq!(request.tools, tools);
}

#[tokio::test]
async fn chat_request_omits_tool_schema_catalog_when_no_tools_are_available() {
    let config = AgentConfig::for_model(tool_model()).with_system_prompt("Base system");
    let context = AgentContext::new();

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");
    let system_text = system_texts(&request);

    assert!(
        !system_text.contains("<available_tools_schema>"),
        "{system_text}"
    );
    assert!(request.tools.is_empty());
}

#[tokio::test]
async fn chat_request_splits_session_id_and_lane_cache_key() {
    let config = AgentConfig::for_model(tool_model())
        .with_session_directory("/tmp/neo/session_00000000-0000-4000-8000-000000000123");
    let context = AgentContext::new();

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");

    // `session_id` keeps its session-correlation semantics: the plain session
    // directory name, unchanged by the lane key split.
    assert_eq!(
        request.options.session_id.as_deref(),
        Some("session_00000000-0000-4000-8000-000000000123")
    );
    // `prompt_cache_key` is the dedicated lane key: session + provider +
    // model + static projection shape (no history, no current input), with
    // each component length-prefixed so user-controlled ids can never alias
    // two different lanes.
    assert_eq!(
        request.options.prompt_cache_key.as_deref(),
        Some(
            "44:session_00000000-0000-4000-8000-000000000123\
             4:test\
             10:tool-model\
             121:proto=Local;user_image=description;user_video=description;\
             tool_image=description;tool_video=description;exchange=preserve"
        )
    );
}

#[tokio::test]
async fn chat_request_injects_runtime_context_without_live_mode_labels() {
    let temp = tempfile::tempdir().expect("temp workspace");
    let config = AgentConfig::for_model(tool_model())
        .with_system_prompt("Base system")
        .with_workspace_root(temp.path())
        .expect("workspace root")
        .with_permission_mode(crate::PermissionMode::Yolo);
    let context = AgentContext::new();

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");
    let system_text = system_texts(&request);

    assert!(system_text.contains("Runtime Context"), "{system_text}");
    assert!(!system_text.contains("permission mode:"), "{system_text}");
    assert!(
        !system_text.contains("tool execution mode:"),
        "{system_text}"
    );
    assert!(
        system_text.contains("write and shell tools are constrained by workspace permissions"),
        "{system_text}"
    );
    assert!(
        system_text.contains("Read may accept absolute paths"),
        "{system_text}"
    );
}

#[tokio::test]
async fn chat_request_keeps_todo_changes_out_of_dynamic_system_context() {
    let mut context = AgentContext::from_replay(
        [
            crate::AgentEvent::TodoUpdated {
                turn: 1,
                todos: vec![
                    crate::TodoEventData {
                        title: "Task 1".to_owned(),
                        status: "done".to_owned(),
                    },
                    crate::TodoEventData {
                        title: "Task 4".to_owned(),
                        status: "in_progress".to_owned(),
                    },
                    crate::TodoEventData {
                        title: "Task 12".to_owned(),
                        status: "pending".to_owned(),
                    },
                ],
            },
            crate::AgentEvent::MessageAppended {
                message: AgentMessage::user_text("Continue the task"),
            },
            crate::AgentEvent::CompactionApplied {
                summary: crate::CompactionSummary {
                    summary: "Continue from Task 4".to_owned(),
                    tokens_before: 10,
                    tokens_after: 5,
                    first_kept_message_index: 1,
                },
            },
        ]
        .iter(),
    );

    let request = chat_request(
        &AgentConfig::for_model(tool_model()),
        &context,
        MediaTransportCapabilities::default(),
    )
    .await
    .expect("chat request");
    let system_text = system_texts(&request);

    assert!(!system_text.contains("Runtime Todo State"), "{system_text}");
    assert!(!system_text.contains("<current_todos>"), "{system_text}");
    assert_eq!(system_text.matches("<todo_snapshot>").count(), 1);
    assert!(system_text.contains("Task 4"), "{system_text}");
    assert!(system_text.contains("in_progress"), "{system_text}");

    context.todos[1].status = "done".to_owned();
    let changed_todos_request = chat_request(
        &AgentConfig::for_model(tool_model()),
        &context,
        MediaTransportCapabilities::default(),
    )
    .await
    .expect("chat request");

    assert_eq!(changed_todos_request.messages, request.messages);

    context.append_message(AgentMessage::user_text("Newest request"));
    let appended_request = chat_request(
        &AgentConfig::for_model(tool_model()),
        &context,
        MediaTransportCapabilities::default(),
    )
    .await
    .expect("chat request");

    assert_eq!(
        &appended_request.messages[..request.messages.len()],
        request.messages.as_slice()
    );
    assert_eq!(appended_request.messages.len(), request.messages.len() + 1);
}

#[tokio::test]
async fn chat_request_does_not_add_review_mode_system_message() {
    let config = AgentConfig::for_model(tool_model()).with_system_prompt("Base system");
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("Please review this change"));

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");
    let system_text = system_texts(&request);

    assert!(!system_text.contains("Review Mode"), "{system_text}");
}

#[tokio::test]
async fn chat_request_does_not_project_unappended_workflow_injection() {
    let config = AgentConfig::for_model(tool_model())
        .with_turn_injection("Workflow guidance")
        .with_system_prompt("Base system");
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::user_text("Earlier request"));
    context.append_message(AgentMessage::assistant(
        vec![Content::text("Earlier answer")],
        Vec::new(),
        crate::StopReason::EndTurn,
    ));
    context.append_message(AgentMessage::user_text("Current request"));

    let request = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");
    assert!(!format!("{request:?}").contains("Workflow guidance"));
}

#[tokio::test]
async fn chat_request_preserves_prefix_when_instruction_update_appends() {
    let config = AgentConfig::for_model(tool_model()).with_system_prompt("Base system");
    let mut context = AgentContext::new();
    context.append_message(AgentMessage::injection_text(
            "<instruction_revision id=\"root\">root rules</instruction_revision>\n<instruction_active_state generation=\"1\" />",
            "instruction_epoch",
        ));
    context.append_message(AgentMessage::user_text("First request"));
    let first = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");

    context.append_message(AgentMessage::assistant(
        vec![Content::text("First answer")],
        Vec::new(),
        crate::StopReason::EndTurn,
    ));
    context.append_message(AgentMessage::injection_text(
        "<instruction_active_state generation=\"2\" />",
        "instruction_epoch",
    ));
    let second = chat_request(&config, &context, MediaTransportCapabilities::default())
        .await
        .expect("chat request");

    assert_eq!(first.tools, second.tools);
    assert_eq!(first.messages, second.messages[..first.messages.len()]);
    assert!(matches!(
        second.messages.last(),
        Some(ChatMessage::User { .. })
    ));
}
