//! `OpenAI` Responses stream error classification and terminal-state handling.

use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ApiKind, ModelClient,
    providers::openai::responses::OpenAiResponsesClient,
};
use serde_json::json;

use super::http_server::{MockServer, request, sse_response, truncated_sse_response};

#[tokio::test]
async fn openai_responses_client_returns_protocol_error_for_failed_streams() {
    let body = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-failed" } }),
        json!({
            "type": "response.failed",
            "response": { "status": "failed" }
        })
    );
    let server = MockServer::start(vec![truncated_sse_response(&body)]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.iter().filter(|event| event.is_err()).count(),
        1,
        "classified provider failure must emit exactly one error: {events:?}"
    );
    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("classified provider failure must emit an error");

    assert_eq!(error.code(), "provider.protocol_error");
    assert!(error.to_string().contains("status failed"));
}

#[tokio::test]
async fn openai_responses_stream_server_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": { "code": "server_error", "message": "upstream busy" }
        }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::Server {
            status: 500,
            retry_after: None,
            message
        } if message == "upstream busy"
    ));
}

#[tokio::test]
async fn openai_responses_stream_rate_limit_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": { "code": "rate_limit_exceeded", "message": "slow down" }
        }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::RateLimit {
            retry_after: None,
            message
        } if message == "slow down"
    ));
}

#[tokio::test]
async fn openai_responses_top_level_stream_rate_limit_error_is_retryable() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "error",
        "code": "rate_limit_exceeded",
        "message": "slow down"
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(
        error,
        AiError::RateLimit {
            retry_after: None,
            message
        } if message == "slow down"
    ));
}

#[tokio::test]
async fn openai_responses_client_returns_protocol_error_for_incomplete_streams() {
    let server = MockServer::start(vec![sse_response(&[
        json!({ "type": "response.created", "response": { "id": "resp-incomplete" } }),
        json!({
            "type": "response.incomplete",
            "response": { "status": "incomplete" }
        }),
    ])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert_eq!(error.code(), "provider.protocol_error");
    assert!(error.to_string().contains("status incomplete"));
}

#[tokio::test]
async fn openai_responses_body_error_respects_terminal_state() {
    let terminal = format!(
        "data: {}\n\ndata: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-terminal" } }),
        json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        })
    );
    let incomplete = format!(
        "data: {}\n\n",
        json!({ "type": "response.created", "response": { "id": "resp-incomplete" } })
    );
    let server = MockServer::start(vec![
        truncated_sse_response(&terminal),
        truncated_sse_response(&incomplete),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let completed = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .expect("terminal marker must survive the body error");
    assert!(matches!(
        completed.last(),
        Some(AiStreamEvent::MessageEnd { .. })
    ));

    let incomplete_events = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        incomplete_events
            .iter()
            .filter(|event| event.is_err())
            .count(),
        1,
        "incomplete stream must emit exactly one error: {incomplete_events:?}"
    );
    let error = incomplete_events
        .into_iter()
        .find_map(Result::err)
        .expect("incomplete body must remain an error");
    assert!(matches!(
        error,
        AiError::Transport { message } if !message.starts_with("transport error:")
    ));
}

#[tokio::test]
async fn openai_responses_numeric_429_and_503_errors_are_retryable() {
    let server = MockServer::start(vec![
        sse_response(&[json!({
            "type": "error",
            "code": 429,
            "message": "slow down"
        })]),
        sse_response(&[json!({
            "type": "error",
            "status": 503,
            "message": "unavailable"
        })]),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let rate_limit = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();
    let unavailable = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(rate_limit, AiError::RateLimit { .. }));
    assert!(matches!(unavailable, AiError::Server { status: 503, .. }));
}

#[tokio::test]
async fn openai_responses_nested_rate_limit_and_overload_errors_are_retryable() {
    let server = MockServer::start(vec![
        sse_response(&[json!({
            "type": "error",
            "error": { "type": "rate_limit_error", "message": "slow down" }
        })]),
        sse_response(&[json!({
            "type": "error",
            "error": { "type": "overloaded_error", "message": "busy" }
        })]),
    ]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let rate_limit = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();
    let overloaded = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(rate_limit, AiError::RateLimit { .. }));
    assert!(matches!(overloaded, AiError::Server { status: 529, .. }));
}

#[tokio::test]
async fn openai_responses_unknown_error_is_protocol() {
    let server = MockServer::start(vec![sse_response(&[json!({
        "type": "error",
        "error": { "type": "mystery_error", "message": "unknown failure" }
    })])]);
    let client = OpenAiResponsesClient::new(server.url.clone(), "test-key");

    let error = client
        .stream_chat(request(ApiKind::OpenAiResponse))
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap_err();

    assert!(matches!(error, AiError::Protocol { .. }));
}
