use neo_extensions::{ExtensionRunner, ExtensionTransport};
use neo_sdk::{JsonlCodec, RpcErrorCode, RpcMessage, RpcRequest, RpcResponse};
use serde_json::json;

#[tokio::test]
async fn stdio_runner_round_trips_jsonl_rpc() {
    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        script.path(),
        r#"
import sys, json
line = sys.stdin.readline()
message = json.loads(line)
assert message["type"] == "request"
print(json.dumps({
  "type": "response",
  "id": message["id"],
  "result": {
    "method": message["method"],
    "params": message["params"]
  }
}), flush=True)
"#,
    )
    .unwrap();

    let mut runner = ExtensionRunner::spawn(ExtensionTransport::Stdio {
        command: "python3".into(),
        args: vec![script.path().to_string_lossy().into_owned()],
        env: vec![],
    })
    .unwrap();

    let response = runner
        .request(RpcRequest::new(
            "call-1",
            "tool.echo",
            json!({ "value": 42 }),
        ))
        .await
        .unwrap();

    assert_eq!(
        response,
        RpcResponse::success(
            "call-1",
            json!({ "method": "tool.echo", "params": { "value": 42 } })
        )
    );
}

#[tokio::test]
async fn stdio_runner_rejects_mismatched_response_id() {
    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        script.path(),
        r#"
import json
print(json.dumps({"type":"response","id":"wrong","result":True}), flush=True)
"#,
    )
    .unwrap();

    let mut runner = ExtensionRunner::spawn(ExtensionTransport::Stdio {
        command: "python3".into(),
        args: vec![script.path().to_string_lossy().into_owned()],
        env: vec![],
    })
    .unwrap();

    let err = runner
        .request(RpcRequest::new("expected", "ping", json!({})))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("response id mismatch"));
}

#[tokio::test]
async fn stdio_runner_surfaces_structured_rpc_failures() {
    let script = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(
        script.path(),
        r#"
import json
message = json.loads(input())
print(json.dumps({
  "type": "response",
  "id": message["id"],
  "error": {
    "code": "method_not_found",
    "message": "unknown method",
    "data": {"method": message["method"]}
  }
}), flush=True)
"#,
    )
    .unwrap();

    let mut runner = ExtensionRunner::spawn(ExtensionTransport::Stdio {
        command: "python3".into(),
        args: vec![script.path().to_string_lossy().into_owned()],
        env: vec![],
    })
    .unwrap();

    let err = runner
        .request(RpcRequest::new("call-404", "missing.tool", json!({})))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("unknown method"));
    assert!(
        err.to_string()
            .contains(&format!("{:?}", RpcErrorCode::MethodNotFound))
    );
}

#[test]
fn runner_uses_sdk_jsonl_codec_contract() {
    let line = JsonlCodec::encode(&RpcMessage::Request(RpcRequest::new(
        "id",
        "method",
        json!({}),
    )))
    .unwrap();

    assert_eq!(
        JsonlCodec::decode_line(&line).unwrap(),
        RpcMessage::Request(RpcRequest::new("id", "method", json!({})))
    );
}
