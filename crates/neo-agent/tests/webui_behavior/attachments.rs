//! Attachment staging and the bootstrap model catalog: uploads are
//! type/size/count-bound at the boundary, messages carry staged ids that the
//! runtime projects per model capability (canonical history always keeps the
//! blob reference), and the model catalog exposes display fields only.

use base64::Engine as _;
use serde_json::{Value, json};

use super::http;
use super::provider::{Step, openai_response_sse};
use super::session_env::{
    TestEnv, snapshot, start_env, start_env_with_capabilities, start_env_with_config,
    wait_for_phase,
};

const PNG_BYTES: [u8; 4] = [0x89, b'P', b'N', b'G'];

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

async fn upload(test_env: &TestEnv, mime: &str, base64: &str) -> http::HttpResult {
    http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        "/api/attachments",
        &json!({ "mime": mime, "base64": base64 }),
    )
    .await
}

async fn create_with_attachments(test_env: &TestEnv, attachments: Value) -> http::HttpResult {
    http::post_json(
        test_env.webui.port,
        &test_env.cookie,
        "/api/sessions",
        &json!({ "message": "with media", "composer": null, "attachments": attachments }),
    )
    .await
}

/// `POST /api/attachments` whitelists image MIME types, bounds the decoded
/// bytes, and rejects over-cap or unknown ids on send — every rejection is a
/// boundary 4xx before any session or turn work happens.
#[tokio::test]
async fn attachment_upload_enforces_size_type_and_count_limits() {
    let (test_env, _provider) =
        start_env(tempfile::tempdir().expect("project tempdir"), vec![]).await;

    // MIME whitelist: images only.
    let rejected_mime = upload(&test_env, "text/plain", &b64(&PNG_BYTES)).await;
    assert_eq!(
        rejected_mime.status, 400,
        "non-image mime: {}",
        rejected_mime.body
    );

    // Well-formed base64 is required.
    let rejected_base64 = upload(&test_env, "image/png", "!!!not-base64!!!").await;
    assert_eq!(
        rejected_base64.status, 400,
        "bad base64: {}",
        rejected_base64.body
    );

    // A small image stages digest-addressed.
    let staged = upload(&test_env, "image/png", &b64(&PNG_BYTES)).await;
    assert_eq!(staged.status, 201, "staged: {}", staged.body);
    let ack: Value = serde_json::from_str(&staged.body).expect("ack json");
    let id = ack["id"].as_str().expect("attachment id");
    assert_eq!(id.len(), 64, "id is a full sha256 hex digest");
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(ack["mime"], "image/png");
    assert_eq!(ack["byte_len"], PNG_BYTES.len() as u64);

    // Decoded bytes beyond the 8 MiB cap are rejected as too large (the body
    // stays under the 12 MiB transport limit so the semantic cap decides).
    let oversized = vec![0xAB_u8; 8 * 1024 * 1024 + 1];
    let too_large = upload(&test_env, "image/png", &b64(&oversized)).await;
    assert_eq!(too_large.status, 413, "oversized: {}", too_large.body);

    // More than four ids on one message are rejected.
    let five: Vec<&str> = std::iter::repeat_n(id, 5).collect();
    let over_cap = create_with_attachments(&test_env, json!(five)).await;
    assert_eq!(over_cap.status, 400, "over cap: {}", over_cap.body);

    // A well-formed but unstaged digest is rejected.
    let unknown = "0".repeat(64);
    let unknown_id = create_with_attachments(&test_env, json!([unknown])).await;
    assert_eq!(unknown_id.status, 400, "unknown id: {}", unknown_id.body);

    // A malformed id never reaches a path join.
    let malformed = create_with_attachments(&test_env, json!(["../../etc/passwd"])).await;
    assert_eq!(malformed.status, 400, "malformed id: {}", malformed.body);

    // Nothing ran: no session was created by the rejected sends.
    let bootstrap = http::get(test_env.webui.port, &test_env.cookie, "/api/bootstrap").await;
    assert_eq!(bootstrap.status, 200);
    let catalog: Value = serde_json::from_str(&bootstrap.body).expect("bootstrap json");
    assert!(
        catalog["sessions"].as_array().is_none_or(Vec::is_empty),
        "rejected sends create no sessions: {}",
        bootstrap.body
    );
}

/// One image attachment flows through the boundary into canonical history as
/// `Content::Image` with a blob reference in both capability lanes; only the
/// provider-facing request copy is projected per capability — deterministic
/// digest-anchored text for an image-incapable model, in-place image for an
/// image-capable one.
#[tokio::test]
async fn message_with_attachments_projects_per_capability() {
    // Lane A: the default mock model has no image capability.
    let (test_env, provider) = start_env(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-1", "answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
    )
    .await;
    let staged = upload(&test_env, "image/png", &b64(&PNG_BYTES)).await;
    assert_eq!(staged.status, 201, "staged: {}", staged.body);
    let id: String = serde_json::from_str::<Value>(&staged.body).expect("ack json")["id"]
        .as_str()
        .expect("attachment id")
        .to_owned();
    let (session_id, _turn_id, _) = {
        let response = create_with_attachments(&test_env, json!([id])).await;
        assert_eq!(response.status, 201, "create: {}", response.body);
        let parsed: Value = serde_json::from_str(&response.body).expect("create json");
        (
            parsed["session_id"]
                .as_str()
                .expect("session id")
                .to_owned(),
            parsed["turn_id"].as_str().expect("turn id").to_owned(),
            parsed,
        )
    };
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    provider.wait_for_requests(1).await;
    let request = &provider.requests()[0].body;
    assert!(
        request.contains(&format!("[media not sent: image {id}")),
        "incapable model gets the deterministic digest-anchored replacement: {request}"
    );
    assert!(
        !request.contains("image_url"),
        "no image bytes leave toward an incapable model: {request}"
    );

    let view = snapshot(&test_env, &session_id).await;
    let serialized = serde_json::to_string(&view["history"]).expect("history json");
    assert!(
        serialized.contains(&format!("\"Blob\":\"{id}\"")),
        "canonical history keeps the blob reference: {serialized}"
    );
    assert!(
        !serialized.contains("media not sent"),
        "the request-lane replacement never enters canonical history: {serialized}"
    );

    // Lane B: the same model with the image capability sends it in place.
    let (test_env, provider) = start_env_with_capabilities(
        tempfile::tempdir().expect("project tempdir"),
        vec![
            Step::Respond(openai_response_sse("resp-2", "answer")),
            Step::Respond(openai_response_sse("resp-title", "Title")),
        ],
        "\"streaming\", \"tools\", \"images\"",
    )
    .await;
    let staged = upload(&test_env, "image/png", &b64(&PNG_BYTES)).await;
    assert_eq!(staged.status, 201, "staged: {}", staged.body);
    let id: String = serde_json::from_str::<Value>(&staged.body).expect("ack json")["id"]
        .as_str()
        .expect("attachment id")
        .to_owned();
    let response = create_with_attachments(&test_env, json!([id])).await;
    assert_eq!(response.status, 201, "create: {}", response.body);
    let session_id =
        serde_json::from_str::<Value>(&response.body).expect("create json")["session_id"]
            .as_str()
            .expect("session id")
            .to_owned();
    let _ = wait_for_phase(&test_env, &session_id, "idle").await;

    provider.wait_for_requests(1).await;
    let request = &provider.requests()[0].body;
    assert!(
        request.contains("image_url") && request.contains(&b64(&PNG_BYTES)),
        "capable model receives the image in place: {request}"
    );
    let view = snapshot(&test_env, &session_id).await;
    let serialized = serde_json::to_string(&view["history"]).expect("history json");
    assert!(
        serialized.contains(&format!("\"Blob\":\"{id}\"")),
        "the capable lane keeps the same canonical blob reference: {serialized}"
    );
}

/// The bootstrap model catalog carries display fields only — alias, provider
/// id, context window, capability tags — and never provider configuration
/// such as base URLs or API-key references.
#[tokio::test]
async fn bootstrap_models_catalog_has_display_fields_only() {
    let extra = r#"
[models."mock/vision-large"]
provider = "mock"
model = "vision-large"
capabilities = ["images", "streaming"]
max_context_tokens = 200000
"#;
    let (test_env, provider) =
        start_env_with_config(tempfile::tempdir().expect("project tempdir"), vec![], extra).await;

    let response = http::get(test_env.webui.port, &test_env.cookie, "/api/bootstrap").await;
    assert_eq!(response.status, 200, "bootstrap: {}", response.body);
    let bootstrap: Value = serde_json::from_str(&response.body).expect("bootstrap json");
    let models = bootstrap["models"].as_array().expect("models catalog");
    let vision = models
        .iter()
        .find(|model| model["alias"] == "mock/vision-large")
        .expect("vision model row");
    assert_eq!(vision["provider"], "mock");
    assert_eq!(vision["context_window"], 200_000);
    assert_eq!(vision["capabilities"], json!(["images", "streaming"]));
    let base = models
        .iter()
        .find(|model| model["alias"] == "mock/gpt-4.1")
        .expect("base model row");
    assert_eq!(base["provider"], "mock");
    assert_eq!(base["capabilities"], json!(["streaming", "tools"]));

    let serialized = response.body;
    assert!(
        !serialized.contains(&provider.url),
        "provider base URLs never appear in the catalog: {serialized}"
    );
    assert!(
        !serialized.contains("OPENAI_API_KEY") && !serialized.contains("api_key"),
        "key references never appear in the catalog: {serialized}"
    );
}
