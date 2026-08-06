use super::*;
use crate::error::AiError;
use std::time::Duration;

#[test]
fn http_status_429_maps_to_rate_limit() {
    let err = ProviderError::HttpStatus {
        status: 429,
        body: Some("Too Many Requests".into()),
        retry_after: Some(Duration::from_secs(30)),
    };
    let ai = err.into_ai_error();
    assert_eq!(ai.code(), "provider.rate_limit");
}

#[test]
fn permanent_quota_http_errors_are_terminal() {
    for (status, body) in [
        (402, "Payment Required"),
        (403, r#"{"error":{"code":"insufficient_quota"}}"#),
        (429, "Usage limit for this billing cycle"),
    ] {
        let error = ProviderError::HttpStatus {
            status,
            body: Some(body.into()),
            retry_after: None,
        }
        .into_ai_error();
        assert!(matches!(error, AiError::QuotaExhausted { .. }));
        assert!(!error.is_retryable());
    }

    assert!(matches!(
        ProviderError::HttpStatus {
            status: 403,
            body: Some("Forbidden".into()),
            retry_after: None,
        }
        .into_ai_error(),
        AiError::Auth { .. }
    ));
    assert!(matches!(
        ProviderError::HttpStatus {
            status: 429,
            body: Some("Too Many Requests".into()),
            retry_after: None,
        }
        .into_ai_error(),
        AiError::RateLimit { .. }
    ));
}

#[test]
fn permanent_quota_stream_codes_are_terminal() {
    for code in [
        "insufficient_quota",
        "insufficient_balance",
        "billing_limit_exceeded",
        "usage_limit_exceeded",
        "payment_required",
    ] {
        assert!(matches!(
            stream_failure(Some(code), "provider detail").into_ai_error(),
            AiError::QuotaExhausted { .. }
        ));
    }
    for code in ["quota_exceeded", "resource_exhausted"] {
        assert!(matches!(
            stream_failure(Some(code), "try later").into_ai_error(),
            AiError::RateLimit { .. }
        ));
    }
    for code in ["insufficient-quota", "payment required"] {
        assert!(matches!(
            stream_failure(Some(code), "try later").into_ai_error(),
            AiError::Protocol { .. }
        ));
    }
}

#[test]
fn http_status_401_maps_to_auth() {
    let err = ProviderError::HttpStatus {
        status: 401,
        body: Some("Unauthorized".into()),
        retry_after: None,
    };
    assert_eq!(err.into_ai_error().code(), "provider.auth_error");
}

#[test]
fn http_status_503_maps_to_server() {
    let err = ProviderError::HttpStatus {
        status: 503,
        body: Some("Service Unavailable".into()),
        retry_after: Some(Duration::from_secs(7)),
    };
    let ai = err.into_ai_error();
    assert!(matches!(
        ai,
        AiError::Server {
            status: 503,
            retry_after: Some(delay),
            ..
        } if delay == Duration::from_secs(7)
    ));
}

#[test]
fn http_status_408_maps_to_retryable_transport() {
    let err = ProviderError::HttpStatus {
        status: 408,
        body: Some("Request Timeout".into()),
        retry_after: Some(Duration::from_secs(2)),
    };
    let ai = err.into_ai_error();
    assert!(ai.is_retryable());
    assert!(matches!(
        ai,
        AiError::Transport { message } if message == "Request Timeout"
    ));
}

#[test]
fn streamed_status_408_maps_to_retryable_transport() {
    let ai = stream_failure(Some("408"), "request timeout").into_ai_error();
    assert!(matches!(
        ai,
        AiError::Transport { message } if message == "request timeout"
    ));
}

#[test]
fn stream_read_error_maps_to_retryable_transport() {
    let ai = stream_failure(Some("stream_read_error"), "stream_read_error").into_ai_error();
    assert!(matches!(
        ai,
        AiError::Transport { message } if message == "stream_read_error"
    ));
}

#[test]
fn transport_display_prefixes_underlying_message_once() {
    let transport = reqwest::Client::new()
        .get("://")
        .build()
        .expect_err("invalid URL must fail");
    let underlying = transport.to_string();
    let ai = ProviderError::Transport(transport).into_ai_error();

    assert_eq!(ai.to_string(), format!("transport error: {underlying}"));
}

#[test]
fn http_status_413_with_context_overflow_maps_to_context_overflow() {
    let err = ProviderError::HttpStatus {
        status: 413,
        body: Some("Request too large: context_length exceeded".into()),
        retry_after: None,
    };
    assert_eq!(err.into_ai_error().code(), "provider.context_overflow");
}

#[test]
fn http_status_413_without_context_pattern_maps_to_protocol() {
    let err = ProviderError::HttpStatus {
        status: 413,
        body: Some("Payload Too Large".into()),
        retry_after: None,
    };
    let ai = err.into_ai_error();
    assert_eq!(ai.code(), "provider.protocol_error");
}

#[test]
fn sanitize_extracts_title_from_html() {
    // Body starts with "413 " not "<", so starts_with('<') would miss this.
    // contains("<title>") detects HTML anywhere in the body.
    let html =
        "413 <html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n</html>\r\n";
    let result = sanitize_error_body(Some(html));
    assert_eq!(result, "413 Request Entity Too Large");
}

#[test]
fn sanitize_empty_title_falls_back_to_body() {
    let html = "<html><head><title>  </title></head><body>nginx</body></html>";
    let result = sanitize_error_body(Some(html));
    assert!(result.contains("nginx"));
}

#[test]
fn is_context_overflow_detects_patterns() {
    assert!(is_context_overflow("Request exceeds context_length limit"));
    assert!(is_context_overflow(
        "prompt is too long for maximum context"
    ));
    assert!(!is_context_overflow("Payload Too Large"));
}
