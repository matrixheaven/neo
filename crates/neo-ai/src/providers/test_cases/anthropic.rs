//! Anthropic request-body fixed points: media projection contract and
//! historical tool-exchange replay.

use super::*;
use crate::{ApiKind, ImageData, ModelCapabilities, ModelSpec, ProviderId, ToolCall};

fn request(messages: Vec<ChatMessage>) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("anthropic".to_owned()),
            model: "claude-test".to_owned(),
            api: ApiKind::AnthropicMessages,
            capabilities: ModelCapabilities::chat(),
        },
        messages,
        tools: vec![],
        options: crate::RequestOptions::default(),
    }
}

#[test]
fn request_body_serializes_user_image_as_base64_block() {
    let request = request(vec![ChatMessage::User {
        content: vec![
            ContentPart::Text {
                text: "look".to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: ImageData::Base64("aGVsbG8=".to_owned()),
            },
        ],
    }]);

    let body = request_body(&request).expect("body");
    assert_eq!(body["messages"][0]["content"][1]["type"], "image");
    assert_eq!(
        body["messages"][0]["content"][1]["source"],
        json!({
            "type": "base64",
            "media_type": "image/png",
            "data": "aGVsbG8=",
        })
    );
}

#[test]
fn request_body_rejects_video_in_user_message() {
    let request = request(vec![ChatMessage::User {
        content: vec![ContentPart::Video {
            mime_type: "video/mp4".to_owned(),
            data: ImageData::Base64("dmlkZW8=".to_owned()),
        }],
    }]);

    let error = request_body(&request).expect_err("video must fail closed, never become text");
    assert!(matches!(error, ProviderError::Unsupported(_)));
}

#[test]
fn request_body_rejects_tool_result_media() {
    let request = request(vec![ChatMessage::ToolResult {
        tool_call_id: "call_1".to_owned(),
        content: vec![
            ContentPart::Text {
                text: "captured".to_owned(),
            },
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: ImageData::Base64("aGVsbG8=".to_owned()),
            },
        ],
        is_error: false,
    }]);

    let error = request_body(&request).expect_err("tool-result media must fail closed");
    assert!(matches!(error, ProviderError::Unsupported(_)));
}

#[test]
fn request_body_replays_read_media_file_exchange_without_tool_table_entry() {
    let request = request(vec![
        ChatMessage::Assistant {
            content: vec![ContentPart::Text {
                text: "calling read".to_owned(),
            }],
            tool_calls: vec![ToolCall {
                id: "call_1".to_owned(),
                name: "ReadMediaFile".to_owned(),
                raw_arguments: r#"{"path":"clip.mp4"}"#.to_owned(),
            }],
        },
        ChatMessage::ToolResult {
            tool_call_id: "call_1".to_owned(),
            content: vec![ContentPart::Text {
                text: "captured the clip".to_owned(),
            }],
            is_error: false,
        },
    ]);

    let body = request_body(&request).expect("historical exchange replays");
    assert!(body.get("tools").is_none(), "the tool table is empty");
    assert_eq!(body["messages"][0]["role"], "assistant");
    assert_eq!(body["messages"][0]["content"][1]["type"], "tool_use");
    assert_eq!(body["messages"][0]["content"][1]["id"], "call_1");
    assert_eq!(body["messages"][0]["content"][1]["name"], "ReadMediaFile");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"][0]["type"], "tool_result");
    assert_eq!(
        body["messages"][1]["content"][0]["content"],
        "captured the clip"
    );
}

#[test]
fn request_body_never_uses_prompt_cache_key_as_user_identity() {
    let mut request = request(vec![ChatMessage::User {
        content: vec![ContentPart::Text {
            text: "hi".to_owned(),
        }],
    }]);
    request.options = crate::RequestOptions {
        session_id: Some("session-1".to_owned()),
        prompt_cache_key: Some("lane-1".to_owned()),
        ..crate::RequestOptions::default()
    };

    let body = request_body(&request).expect("body");
    assert!(
        body.get("prompt_cache_key").is_none(),
        "Anthropic has no prompt-cache key field; nothing may be mapped onto it"
    );
    assert_eq!(
        body["metadata"]["user_id"], "session-1",
        "metadata.user_id keeps the session-correlation identity"
    );
}
