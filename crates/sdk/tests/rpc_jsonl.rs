use neo_sdk::{
    JsonlCodec, RpcCodecError, RpcCommandKind, RpcCommandRecord, RpcCommandsResult, RpcError,
    RpcErrorCode, RpcMessage, RpcNotification, RpcRequest, RpcResponse, RpcSessionExportHtmlResult,
    RpcSessionGetResult, RpcSessionRecord, RpcSessionShareRecord, RpcSessionTreeRecord,
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
        summary_source: Some("local_extractive".to_owned()),
        summary_model: None,
        summary_updated_at: Some("125.0Z".to_owned()),
        parent_id: None,
        cloud_id: Some("cs_alpha".to_owned()),
        synced_at: Some("126.0Z".to_owned()),
        remote_parent_id: Some("cs_parent".to_owned()),
        children: vec!["alpha-fork-1".to_owned()],
        share_ids: vec!["sh_alpha".to_owned()],
        shares: vec![RpcSessionShareRecord {
            id: "sh_alpha".to_owned(),
            cloud_id: Some("cs_alpha".to_owned()),
            public: Some(true),
            html_url: Some("https://neo.example/v1/shares/sh_alpha.html".to_owned()),
            json_url: Some("https://neo.example/v1/shares/sh_alpha.json".to_owned()),
            created_at: Some("127.0Z".to_owned()),
        }],
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
    assert_eq!(value["record"]["summary_source"], "local_extractive");
    assert_eq!(value["record"]["summary_updated_at"], "125.0Z");
    assert!(value["record"]["parent_id"].is_null());
    assert_eq!(value["record"]["cloud_id"], "cs_alpha");
    assert_eq!(value["record"]["synced_at"], "126.0Z");
    assert_eq!(value["record"]["remote_parent_id"], "cs_parent");
    assert_eq!(value["record"]["children"], json!(["alpha-fork-1"]));
    assert_eq!(value["record"]["share_ids"], json!(["sh_alpha"]));
    assert_eq!(value["record"]["shares"][0]["id"], "sh_alpha");
    assert_eq!(value["record"]["shares"][0]["public"], true);
    assert_eq!(
        value["record"]["shares"][0]["html_url"],
        "https://neo.example/v1/shares/sh_alpha.html"
    );
    assert_eq!(
        serde_json::from_value::<RpcSessionTreeRecord>(value).expect("deserialize tree record"),
        tree_record
    );
}

#[test]
fn session_get_result_has_stable_json_shape() {
    let result = RpcSessionGetResult {
        record: RpcSessionRecord {
            id: "alpha".to_owned(),
            name: Some("Main thread".to_owned()),
            summary: Some("Local branch summary".to_owned()),
            summary_source: Some("local_extractive".to_owned()),
            summary_model: None,
            summary_updated_at: None,
            parent_id: None,
            cloud_id: None,
            synced_at: None,
            remote_parent_id: None,
            children: vec!["alpha-fork-1".to_owned()],
            share_ids: Vec::new(),
            shares: Vec::new(),
        },
        path: "/tmp/neo/.neo/sessions/alpha.jsonl".to_owned(),
        messages: vec![json!({
            "User": {
                "content": [
                    {
                        "Text": {
                            "text": "hello"
                        }
                    }
                ]
            }
        })],
    };

    let value = serde_json::to_value(&result).expect("serialize session get result");

    assert_eq!(value["id"], "alpha");
    assert_eq!(value["name"], "Main thread");
    assert_eq!(value["summary"], "Local branch summary");
    assert!(value["parent_id"].is_null());
    assert_eq!(value["children"], json!(["alpha-fork-1"]));
    assert_eq!(value["path"], "/tmp/neo/.neo/sessions/alpha.jsonl");
    assert_eq!(
        value["messages"][0]["User"]["content"][0]["Text"]["text"],
        "hello"
    );
    assert_eq!(
        serde_json::from_value::<RpcSessionGetResult>(value)
            .expect("deserialize session get result"),
        result
    );
}

#[test]
fn session_export_html_result_has_stable_json_shape() {
    let result = RpcSessionExportHtmlResult {
        session_id: "alpha".to_owned(),
        html: "<!doctype html><title>neo session alpha</title>".to_owned(),
    };

    let value = serde_json::to_value(&result).expect("serialize session export html result");

    assert_eq!(value["session_id"], "alpha");
    assert_eq!(
        value["html"],
        "<!doctype html><title>neo session alpha</title>"
    );
    assert_eq!(
        serde_json::from_value::<RpcSessionExportHtmlResult>(value)
            .expect("deserialize session export html result"),
        result
    );
}

#[test]
fn commands_result_has_stable_prompt_template_json_shape() {
    let result = RpcCommandsResult {
        commands: vec![RpcCommandRecord {
            name: "/review".to_owned(),
            kind: RpcCommandKind::PromptTemplate,
            template: "review".to_owned(),
            description: "Review a target".to_owned(),
            argument_hint: Some("<path>".to_owned()),
            location: "project".to_owned(),
            path: "/tmp/neo/.neo/prompts/review.md".to_owned(),
        }],
    };

    let value = serde_json::to_value(&result).expect("serialize commands result");

    assert_eq!(value["commands"][0]["name"], "/review");
    assert_eq!(value["commands"][0]["kind"], "prompt_template");
    assert_eq!(value["commands"][0]["template"], "review");
    assert_eq!(value["commands"][0]["description"], "Review a target");
    assert_eq!(value["commands"][0]["argument_hint"], "<path>");
    assert_eq!(value["commands"][0]["location"], "project");
    assert_eq!(
        serde_json::from_value::<RpcCommandsResult>(value).expect("deserialize commands result"),
        result
    );
}
