use neo_agent_core::rpc::{
    RpcError, RpcErrorCode, RpcMessage, RpcNotification, RpcRequest, RpcResponse,
    codec::{JsonlCodec, RpcCodecError},
};
use serde_json::json;

#[test]
fn jsonl_codec_encodes_and_decodes_rpc_messages() {
    let request = RpcMessage::Request(RpcRequest::new(
        "req-1",
        "extension.describe",
        json!({ "name": "alpha" }),
    ));
    let notification =
        RpcMessage::Notification(RpcNotification::new("log", json!({ "level": "info" })));
    let response = RpcMessage::Response(RpcResponse::success("req-1", json!({ "ok": true })));

    let encoded = JsonlCodec::encode_many([&request, &notification, &response]).unwrap();

    assert!(encoded.ends_with('\n'));
    assert_eq!(encoded.lines().count(), 3);
    assert_eq!(
        JsonlCodec::decode_stream(&encoded).unwrap(),
        vec![request, notification, response]
    );
}

#[test]
fn rpc_response_preserves_structured_error() {
    let response = RpcMessage::Response(RpcResponse::failure(
        "req-9",
        RpcError::new(
            RpcErrorCode::InvalidParams,
            "missing tool name",
            Some(json!({ "field": "name" })),
        ),
    ));

    let line = JsonlCodec::encode(&response).unwrap();
    let decoded = JsonlCodec::decode_line(&line).unwrap();

    assert_eq!(decoded, response);
    assert!(line.contains("\"code\":\"invalid_params\""));
}

#[test]
fn decoder_rejects_empty_or_trailing_content() {
    assert!(JsonlCodec::decode_line("\n").is_err());
    assert!(JsonlCodec::decode_line("{}\n{}").is_err());
}

#[test]
fn stream_decoder_reports_malformed_frame_line_number() {
    let err = JsonlCodec::decode_stream(concat!(
        "{\"type\":\"notification\",\"method\":\"ready\"}\n",
        "{\"type\":\"request\",\"id\":\"bad\",\"method\":\n"
    ))
    .unwrap_err();

    assert!(matches!(err, RpcCodecError::Line { line: 2, .. }));
    assert!(err.to_string().contains("line 2"));
}

#[test]
fn parse_error_can_be_returned_as_structured_rpc_failure() {
    let err = JsonlCodec::decode_line("{").unwrap_err();
    let response = err.to_response("bad-json");
    let line = JsonlCodec::encode(&RpcMessage::Response(response.clone())).unwrap();

    assert_eq!(
        response,
        RpcResponse::failure(
            "bad-json",
            RpcError::new(RpcErrorCode::ParseError, err.to_string(), None)
        )
    );
    assert!(line.contains("\"error\""));
    assert!(line.contains("\"code\":\"parse_error\""));
}
