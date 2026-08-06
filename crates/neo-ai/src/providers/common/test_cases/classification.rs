use super::*;
use crate::error::AiError;
use std::time::Duration;

enum ExpectedStatusError {
    Code(&'static str),
    RetryableTransport {
        message: &'static str,
    },
    Server {
        status: u16,
        retry_after: Option<Duration>,
    },
}

#[test]
fn http_status_codes_classify_into_typed_ai_errors() {
    // (case, status, body, retry_after, expected)
    let cases = [
        (
            "401 maps to auth",
            401,
            "Unauthorized",
            None,
            ExpectedStatusError::Code("provider.auth_error"),
        ),
        (
            "408 maps to retryable transport",
            408,
            "Request Timeout",
            Some(Duration::from_secs(2)),
            ExpectedStatusError::RetryableTransport {
                message: "Request Timeout",
            },
        ),
        (
            "413 with context pattern maps to context overflow",
            413,
            "Request too large: context_length exceeded",
            None,
            ExpectedStatusError::Code("provider.context_overflow"),
        ),
        (
            "413 without context pattern maps to protocol",
            413,
            "Payload Too Large",
            None,
            ExpectedStatusError::Code("provider.protocol_error"),
        ),
        (
            "429 maps to rate limit",
            429,
            "Too Many Requests",
            Some(Duration::from_secs(30)),
            ExpectedStatusError::Code("provider.rate_limit"),
        ),
        (
            "503 maps to server with retry_after",
            503,
            "Service Unavailable",
            Some(Duration::from_secs(7)),
            ExpectedStatusError::Server {
                status: 503,
                retry_after: Some(Duration::from_secs(7)),
            },
        ),
    ];

    for (name, status, body, retry_after, expected) in cases {
        let err = ProviderError::HttpStatus {
            status,
            body: Some(body.into()),
            retry_after,
        }
        .into_ai_error();
        let ok = match &expected {
            ExpectedStatusError::Code(code) => err.code() == *code,
            ExpectedStatusError::RetryableTransport { message } => {
                err.is_retryable()
                    && matches!(
                        &err,
                        AiError::Transport { message: actual } if actual == message
                    )
            }
            ExpectedStatusError::Server {
                status,
                retry_after,
            } => matches!(
                &err,
                AiError::Server {
                    status: actual_status,
                    retry_after: actual_retry_after,
                    ..
                } if *actual_status == *status && *actual_retry_after == *retry_after
            ),
        };
        assert!(ok, "case {name}: got {err:?}");
    }
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
