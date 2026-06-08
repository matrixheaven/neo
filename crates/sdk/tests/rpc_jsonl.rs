use neo_sdk::{
    JsonlCodec, RpcCodecError, RpcError, RpcErrorCode, RpcMessage, RpcNotification, RpcRequest,
    RpcResponse, RpcSessionRecord, RpcSessionTreeRecord,
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

#[test]
fn session_rpc_records_have_stable_json_shape() {
    let record = RpcSessionRecord {
        id: "alpha".to_owned(),
        name: Some("Main thread".to_owned()),
        summary: Some("Local branch summary".to_owned()),
        parent_id: None,
        children: vec!["alpha-fork-1".to_owned()],
    };
    let tree_record = RpcSessionTreeRecord {
        depth: 1,
        record: record.clone(),
    };

    let value = serde_json::to_value(&tree_record).expect("serialize tree record");

    assert_eq!(value["depth"], 1);
    assert_eq!(value["record"]["id"], "alpha");
    assert_eq!(value["record"]["name"], "Main thread");
    assert_eq!(value["record"]["summary"], "Local branch summary");
    assert!(value["record"]["parent_id"].is_null());
    assert_eq!(value["record"]["children"], json!(["alpha-fork-1"]));
    assert_eq!(
        serde_json::from_value::<RpcSessionTreeRecord>(value).expect("deserialize tree record"),
        tree_record
    );
}
