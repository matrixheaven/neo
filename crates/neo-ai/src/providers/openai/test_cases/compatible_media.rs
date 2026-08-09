//! OpenAI Chat Completions request-body fixed points: media projection
//! contract, dedicated cache lane key, and historical tool-exchange replay.

use super::*;
use crate::{ApiKind, ImageData, ModelCapabilities, ModelSpec, ProviderId, ToolCall};

fn request(model: &str, messages: Vec<ChatMessage>) -> ChatRequest {
    ChatRequest {
        model: ModelSpec {
            provider: ProviderId("openai".to_owned()),
            model: model.to_owned(),
            api: ApiKind::OpenAi,
            capabilities: ModelCapabilities::chat(),
        },
        messages,
        tools: vec![],
        options: crate::RequestOptions::default(),
    }
}

#[test]
fn request_body_maps_dedicated_prompt_cache_key_not_session_id() {
    let mut request = request(
        "gpt-test",
        vec![ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hi".to_owned(),
            }],
        }],
    );
    request.options = crate::RequestOptions {
        session_id: Some("session-1".to_owned()),
        prompt_cache_key: Some("lane-1".to_owned()),
        ..crate::RequestOptions::default()
    };

    let body = request_body(&request).expect("body");
    assert_eq!(
        body["prompt_cache_key"], "lane-1",
        "the dedicated field maps to prompt_cache_key"
    );
}

#[test]
fn request_body_omits_prompt_cache_key_when_only_session_id_is_set() {
    let mut request = request(
        "gpt-test",
        vec![ChatMessage::User {
            content: vec![ContentPart::Text {
                text: "hi".to_owned(),
            }],
        }],
    );
    request.options = crate::RequestOptions {
        session_id: Some("session-1".to_owned()),
        ..crate::RequestOptions::default()
    };

    let body = request_body(&request).expect("body");
    assert!(
        body.get("prompt_cache_key").is_none(),
        "session_id keeps its correlation semantics and is not reused as the cache lane"
    );
}

#[test]
fn request_body_serializes_user_image_base64_as_data_uri() {
    let request = request(
        "gpt-test",
        vec![ChatMessage::User {
            content: vec![
                ContentPart::Text {
                    text: "look".to_owned(),
                },
                ContentPart::Image {
                    mime_type: "image/png".to_owned(),
                    data: ImageData::Base64("aGVsbG8=".to_owned()),
                },
            ],
        }],
    );

    let body = request_body(&request).expect("body");
    assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
    assert_eq!(
        body["messages"][0]["content"][1]["image_url"]["url"],
        "data:image/png;base64,aGVsbG8="
    );
}

#[test]
fn request_body_rejects_video_in_user_message() {
    let request = request(
        "gpt-test",
        vec![ChatMessage::User {
            content: vec![ContentPart::Video {
                mime_type: "video/mp4".to_owned(),
                data: ImageData::Base64("dmlkZW8=".to_owned()),
            }],
        }],
    );

    let error = request_body(&request).expect_err("video must fail closed, never become text");
    assert!(matches!(error, ProviderError::Unsupported(_)));
}

#[test]
fn request_body_rejects_tool_result_media() {
    let request = request(
        "gpt-test",
        vec![ChatMessage::ToolResult {
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
        }],
    );

    let error = request_body(&request).expect_err("tool-result media must fail closed");
    assert!(matches!(error, ProviderError::Unsupported(_)));
}

#[test]
fn request_body_replays_read_media_file_exchange_without_tool_table_entry() {
    let request = request(
        "gpt-test",
        vec![
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
        ],
    );

    let body = request_body(&request).expect("historical exchange replays");
    assert!(body.get("tools").is_none(), "the tool table is empty");
    assert_eq!(body["messages"][0]["role"], "assistant");
    assert_eq!(
        body["messages"][0]["tool_calls"][0]["function"]["name"],
        "ReadMediaFile"
    );
    assert_eq!(body["messages"][1]["role"], "tool");
    assert_eq!(body["messages"][1]["tool_call_id"], "call_1");
    assert_eq!(
        body["messages"][1]["content"], "captured the clip",
        "the full exchange replays with call id and result text intact"
    );
}
