use neo_ai::{
    AiStreamEvent, ChatMessage, ContentPart, ImageData, StopReason, ToolCall, ToolSpec,
    collect_tool_arguments, schema_for,
};
use schemars::JsonSchema;
use serde_json::{Value, json};

#[derive(JsonSchema)]
#[allow(dead_code)]
struct CreateFileInput {
    path: String,
    contents: String,
}

#[test]
fn tool_spec_helpers_build_object_schema_from_rust_type() {
    let tool = ToolSpec::from_schema::<CreateFileInput>("create_file", "Create or replace a file");

    assert_eq!(tool.name, "create_file");
    assert_eq!(tool.description, "Create or replace a file");
    assert_eq!(tool.input_schema["type"], "object");
    assert!(tool.input_schema["properties"].get("path").is_some());
    assert!(tool.input_schema["properties"].get("contents").is_some());
    assert_eq!(tool.input_schema["required"], json!(["path", "contents"]));
}

#[test]
fn tool_spec_helpers_build_single_string_schema() {
    let tool = ToolSpec::string_arg("read_file", "Read a file", "path", "Path to read");

    assert_eq!(tool.input_schema["type"], "object");
    assert_eq!(tool.input_schema["properties"]["path"]["type"], "string");
    assert_eq!(
        tool.input_schema["properties"]["path"]["description"],
        "Path to read"
    );
    assert_eq!(tool.input_schema["required"], json!(["path"]));
}

#[test]
fn schema_for_returns_json_schema_for_types() {
    let schema = schema_for::<CreateFileInput>();

    assert_eq!(schema["type"], "object");
}

#[test]
fn collect_tool_arguments_prefers_final_tool_call_end_arguments() {
    let events = vec![
        AiStreamEvent::ToolCallArgsDelta {
            id: "call-1".to_owned(),
            json_fragment: "{\"path\":\"wrong\"}".to_owned(),
        },
        AiStreamEvent::ToolCallEnd {
            id: "call-1".to_owned(),
            arguments: json!({ "path": "final" }),
        },
    ];

    assert_eq!(
        collect_tool_arguments(&events, "call-1").expect("arguments should collect"),
        json!({ "path": "final" })
    );
}

#[test]
fn collect_tool_arguments_joins_fragments_and_ignores_other_tool_calls() {
    let events = vec![
        AiStreamEvent::ToolCallArgsDelta {
            id: "call-2".to_owned(),
            json_fragment: "{\"ignored\":true}".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "call-1".to_owned(),
            json_fragment: "{\"path\":".to_owned(),
        },
        AiStreamEvent::ToolCallArgsDelta {
            id: "call-1".to_owned(),
            json_fragment: "\"src/lib.rs\"}".to_owned(),
        },
    ];

    assert_eq!(
        collect_tool_arguments(&events, "call-1").expect("arguments should collect"),
        json!({ "path": "src/lib.rs" })
    );
}

#[test]
fn collect_tool_arguments_reports_missing_or_invalid_arguments() {
    let missing = collect_tool_arguments(&[], "call-1").expect_err("missing args should error");
    assert!(missing.to_string().contains("missing tool arguments"));

    let invalid = collect_tool_arguments(
        &[AiStreamEvent::ToolCallArgsDelta {
            id: "call-1".to_owned(),
            json_fragment: "{\"unterminated\"".to_owned(),
        }],
        "call-1",
    )
    .expect_err("invalid args should error");
    assert!(invalid.to_string().contains("invalid tool arguments"));
}

#[test]
fn chat_message_stream_event_and_tool_spec_serialize_stably() {
    let message = ChatMessage::Assistant {
        content: vec![
            ContentPart::Text {
                text: "hello".to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: ImageData::Url("https://example.com/image.png".to_owned()),
            },
        ],
        tool_calls: vec![ToolCall {
            id: "call-1".to_owned(),
            name: "read_file".to_owned(),
            arguments: json!({ "path": "Cargo.toml" }),
        }],
    };
    let event = AiStreamEvent::MessageEnd {
        stop_reason: StopReason::ToolUse,
        usage: None,
    };
    let tool = ToolSpec::string_arg("read_file", "Read a file", "path", "Path to read");

    assert_eq!(
        serde_json::from_value::<ChatMessage>(serde_json::to_value(&message).unwrap()).unwrap(),
        message
    );
    assert_eq!(
        serde_json::from_value::<AiStreamEvent>(serde_json::to_value(&event).unwrap()).unwrap(),
        event
    );
    assert_eq!(
        serde_json::from_value::<ToolSpec>(serde_json::to_value(&tool).unwrap()).unwrap(),
        tool
    );

    let serialized_tool: Value = serde_json::to_value(tool).unwrap();
    assert_eq!(
        serialized_tool["input_schema"]["properties"]["path"]["type"],
        "string"
    );
}
