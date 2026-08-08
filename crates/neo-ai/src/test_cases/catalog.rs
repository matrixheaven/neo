//! Catalog behavior (moved from `catalog.rs`).

use super::*;
use crate::{ReasoningBudget, ReasoningCapability, ReasoningEffort};
use tokio::io::AsyncWriteExt;

#[tokio::test(start_paused = true)]
async fn stalled_catalog_response_hits_request_deadline() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local catalog server");
    let address = listener.local_addr().expect("catalog server address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept catalog request");
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n",
            )
            .await
            .expect("write stalled catalog response headers");
        std::future::pending::<()>().await;
    });

    let error = tokio::time::timeout(
        CATALOG_REQUEST_TIMEOUT + Duration::from_secs(1),
        fetch_catalog_from(&format!("http://{address}/catalog")),
    )
    .await
    .expect("catalog client deadline must beat the test guard")
    .expect_err("stalled catalog response must time out");

    assert!(matches!(error, crate::error::AiError::Transport { .. }));
    server.abort();
}

#[tokio::test]
async fn catalog_http_errors_use_shared_status_classification() {
    async fn serve_status(status_line: &str, extra_headers: &str, body: &str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local catalog server");
        let address = listener.local_addr().expect("catalog server address");
        let response = format!(
            "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra_headers}\r\n{body}",
            body.len(),
        );
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept catalog request");
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write catalog error response");
        });
        format!("http://{address}/catalog")
    }

    let auth_url = serve_status(
        "HTTP/1.1 401 Unauthorized",
        "",
        r#"{"error":"invalid api key"}"#,
    )
    .await;
    let auth_err = fetch_catalog_from(&auth_url)
        .await
        .expect_err("401 must classify as Auth");
    assert!(
        matches!(auth_err, crate::error::AiError::Auth { .. }),
        "expected Auth, got {auth_err:?}"
    );
    assert!(!auth_err.is_retryable());

    let rate_url = serve_status(
        "HTTP/1.1 429 Too Many Requests",
        "retry-after: 7\r\n",
        r#"{"error":"rate limited"}"#,
    )
    .await;
    let rate_err = fetch_catalog_from(&rate_url)
        .await
        .expect_err("429 must classify as RateLimit");
    match &rate_err {
        crate::error::AiError::RateLimit {
            retry_after: Some(delay),
            ..
        } => assert_eq!(*delay, Duration::from_secs(7)),
        other => panic!("expected RateLimit with Retry-After, got {other:?}"),
    }
    assert!(rate_err.is_retryable());

    let server_url = serve_status(
        "HTTP/1.1 503 Service Unavailable",
        "",
        r#"{"error":"backend down"}"#,
    )
    .await;
    let server_err = fetch_catalog_from(&server_url)
        .await
        .expect_err("503 must classify as Server");
    match &server_err {
        crate::error::AiError::Server { status: 503, .. } => {}
        other => panic!("expected Server 503, got {other:?}"),
    }
    assert!(server_err.is_retryable());
}

#[tokio::test]
async fn oversized_chunked_catalog_response_is_rejected() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local catalog server");
    let address = listener.local_addr().expect("catalog server address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept catalog request");
        let mut buf = [0u8; 1024];
        let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
        // Chunked success body with no Content-Length; stream past the 16 MiB cap.
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n",
            )
            .await
            .expect("write chunked headers");
        let chunk = vec![b'x'; 64 * 1024];
        let header = format!("{:x}\r\n", chunk.len());
        let mut sent = 0usize;
        while sent <= CATALOG_BODY_LIMIT_BYTES {
            socket
                .write_all(header.as_bytes())
                .await
                .expect("write chunk size");
            socket.write_all(&chunk).await.expect("write chunk body");
            socket.write_all(b"\r\n").await.expect("write chunk CRLF");
            sent = sent.saturating_add(chunk.len());
        }
        // Do not send the terminating 0-chunk; client must reject mid-stream.
        std::future::pending::<()>().await;
    });

    let error = fetch_catalog_from(&format!("http://{address}/catalog"))
        .await
        .expect_err("oversized chunked catalog body must be rejected");
    assert!(
        matches!(error, crate::error::AiError::Protocol { .. }),
        "oversize is Protocol, got {error:?}"
    );
    assert!(!error.is_retryable());
    server.abort();
}

#[test]
fn infer_api_type_maps_npm_package_and_explicit_type() {
    // (case, id, npm package, explicit type, expected wire type)
    let cases = [
        (
            "anthropic npm package",
            "anthropic",
            Some("@ai-sdk/anthropic"),
            None,
            ApiType::Anthropic,
        ),
        (
            "openai npm package",
            "openai",
            Some("@ai-sdk/openai"),
            None,
            ApiType::OpenAi,
        ),
        (
            "explicit type wins without npm",
            "custom",
            None,
            Some("openai_response"),
            ApiType::OpenAiResponse,
        ),
    ];
    for (name, id, npm, explicit_type, expected) in cases {
        let entry = CatalogEntry {
            id: id.to_owned(),
            name: None,
            api: None,
            env: vec![],
            npm: npm.map(str::to_owned),
            explicit_type: explicit_type.map(str::to_owned),
            models: BTreeMap::new(),
        };
        assert_eq!(infer_api_type(&entry), Some(expected), "case {name}");
    }
}

#[test]
fn catalog_model_capabilities_defaults_to_streaming_and_tools() {
    let model = CatalogModel {
        id: "chat".to_owned(),
        name: None,
        family: None,
        limit: None,
        tool_call: None,
        reasoning: None,
        reasoning_options: Vec::new(),
        interleaved: None,
        modalities: None,
    };

    assert_eq!(catalog_model_capabilities(&model), ["streaming", "tools"]);
}

#[test]
fn catalog_model_capabilities_respects_disabled_tools_and_optional_features() {
    let model = CatalogModel {
        id: "vision-reasoning".to_owned(),
        name: None,
        family: None,
        limit: None,
        tool_call: Some(false),
        reasoning: Some(true),
        reasoning_options: Vec::new(),
        interleaved: None,
        modalities: Some(CatalogModalities {
            input: vec!["text".to_owned(), "image".to_owned()],
            output: vec!["text".to_owned()],
        }),
    };

    assert_eq!(
        catalog_model_capabilities(&model),
        ["streaming", "reasoning", "images"]
    );
}

#[test]
fn catalog_model_capability_reads_effort_reasoning_options() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "gpt-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "effort", "values": ["none", "minimal", "low", "medium", "high", "xhigh", "max", "UltraMax"] }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Effort {
            values: vec![
                ReasoningEffort::minimal(),
                ReasoningEffort::low(),
                ReasoningEffort::medium(),
                ReasoningEffort::high(),
                ReasoningEffort::xhigh(),
                ReasoningEffort::max(),
                ReasoningEffort::try_from("UltraMax").expect("custom effort"),
            ],
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_allows_disable_when_toggle_accompanies_effort() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "toggle-effort-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "toggle" },
            { "type": "effort", "values": ["low", "high"] }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Combined {
            toggle: true,
            effort: vec![ReasoningEffort::low(), ReasoningEffort::high()],
            budget: None,
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_reads_budget_reasoning_options() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "gemini-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "budget_tokens", "min": 0, "max": 24576 }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::BudgetTokens {
            min: Some(0),
            max: Some(24_576),
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_preserves_effort_and_budget_reasoning_options() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "combined-test",
        "reasoning": true,
        "reasoning_options": [
            { "type": "toggle" },
            { "type": "effort", "values": ["low", "high"] },
            { "type": "budget_tokens", "min": 128, "max": 24576 }
        ]
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Combined {
            toggle: true,
            effort: vec![ReasoningEffort::low(), ReasoningEffort::high()],
            budget: Some(ReasoningBudget {
                min: Some(128),
                max: Some(24_576),
            }),
            disable_supported: true,
        }
    );
}

#[test]
fn catalog_model_capability_falls_back_for_unknown_reasoning_metadata() {
    let model: CatalogModel = serde_json::from_value(serde_json::json!({
        "id": "unknown-reasoner",
        "reasoning": true
    }))
    .expect("catalog model");

    assert_eq!(
        catalog_model_reasoning(&model),
        ReasoningCapability::Toggle {
            disable_supported: true,
        }
    );
}
