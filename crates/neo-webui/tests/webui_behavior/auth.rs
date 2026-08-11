//! Auth and request-guard behavior: one-time token claim, session cookie,
//! exact Host/Origin validation, security headers, body limits, and log
//! redaction. All servers bind `127.0.0.1:0`; all waits are response-driven
//! (no fixed sleeps).

use std::sync::{Arc, Mutex};

use base64::Engine as _;

use super::http_server::{RawRequest, RawResponse, TestServer, cookie_pair, raw_request};

fn error_code(response: &RawResponse) -> String {
    response.error_code()
}

fn claim_request(server: &TestServer, token: &str) -> RawRequest {
    RawRequest {
        method: "POST".to_string(),
        path: "/api/auth/claim".to_string(),
        origin: Some(server.origin()),
        content_type: Some("application/json".to_string()),
        body: format!(r#"{{"token":"{token}"}}"#).into_bytes(),
        ..RawRequest::default()
    }
}

#[tokio::test]
async fn authenticated_completion_query_returns_typed_candidates_and_rejects_bad_triggers() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    let response = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/completions?query=%2Fplan".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(response.status, 200);
    let body: serde_json::Value = serde_json::from_slice(&response.body).expect("completion json");
    assert_eq!(body["items"][0]["value"], "/plan");
    assert_eq!(body["items"][0]["description"], "fixture completion");

    let rejected = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/completions?query=plan".to_string(),
            cookie: Some(cookie),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(rejected.status, 400);
    assert_eq!(error_code(&rejected), "invalid_request");
}

#[tokio::test]
async fn claim_consumes_the_one_time_token_and_sets_a_strict_http_only_cookie() {
    let server = TestServer::start().await;
    let response = raw_request(server.addr, claim_request(&server, &server.token)).await;
    assert_eq!(response.status, 204);
    let set_cookie = response.set_cookie().expect("claim sets a session cookie");
    let attributes: Vec<&str> = set_cookie.split(';').map(str::trim).collect();
    assert!(
        attributes[0].starts_with("neo_webui_session="),
        "cookie name is fixed: {set_cookie}"
    );
    let credential = attributes[0].split_once('=').expect("cookie value").1;
    assert!(!credential.is_empty());
    assert!(attributes.contains(&"HttpOnly"));
    assert!(attributes.contains(&"SameSite=Strict"));
    assert!(attributes.contains(&"Path=/"));
    assert!(
        !set_cookie.contains("Secure"),
        "no fake Secure flag: {set_cookie}"
    );
    assert!(
        !set_cookie.contains("Max-Age"),
        "no persistence: {set_cookie}"
    );

    // The token is consumed: the same claim now fails generically and sets
    // no cookie.
    let second = raw_request(server.addr, claim_request(&server, &server.token)).await;
    assert_eq!(second.status, 401);
    assert_eq!(error_code(&second), "unauthorized");
    assert!(second.set_cookie().is_none(), "no cookie on failed claim");

    // The issued credential unlocks the authenticated surface.
    let read = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some(format!("neo_webui_session={credential}")),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(read.status, 200);
}

#[tokio::test]
async fn concurrent_claims_exactly_one_succeeds() {
    let server = TestServer::start().await;
    let request = claim_request(&server, &server.token);
    let (first, second) = tokio::join!(
        raw_request(server.addr, request.clone()),
        raw_request(server.addr, request)
    );
    let statuses = [first.status, second.status];
    assert_eq!(
        statuses.iter().filter(|status| **status == 204).count(),
        1,
        "exactly one concurrent claim wins"
    );
    assert_eq!(statuses.iter().filter(|status| **status == 401).count(), 1);
}

#[tokio::test]
async fn invalid_consumed_or_wrong_length_tokens_get_the_same_generic_401() {
    let server = TestServer::start().await;
    // Consume the real token first so it can serve as the "consumed" case.
    let consumed = raw_request(server.addr, claim_request(&server, &server.token)).await;
    assert_eq!(consumed.status, 204);
    assert!(consumed.set_cookie().is_some());

    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let cases = [
        server.token.clone(),               // consumed
        encode(&[0u8; 32]),                 // well-formed, wrong bytes
        encode(&[1u8; 31]),                 // too short
        encode(&[1u8; 33]),                 // too long
        format!("{}=", encode(&[2u8; 32])), // padded
        "!!!not-base64!!!".to_string(),     // invalid alphabet
        String::new(),                      // empty
    ];
    let mut bodies = Vec::new();
    for token in &cases {
        let response = raw_request(server.addr, claim_request(&server, token)).await;
        assert_eq!(response.status, 401, "case {token:?}");
        assert_eq!(error_code(&response), "unauthorized", "case {token:?}");
        assert!(response.set_cookie().is_none(), "case {token:?}");
        bodies.push(response.body);
    }
    // Every invalid/consumed claim shares one byte-identical generic body.
    for pair in bodies.windows(2) {
        assert_eq!(pair[0], pair[1], "generic 401 must not leak token state");
    }
}

#[tokio::test]
async fn unknown_json_or_query_fields_are_rejected() {
    let server = TestServer::start().await;

    // Claim with an unknown field: rejected before the token is touched, so
    // the token stays valid.
    let rejected_claim = raw_request(
        server.addr,
        RawRequest {
            method: "POST".to_string(),
            path: "/api/auth/claim".to_string(),
            origin: Some(server.origin()),
            content_type: Some("application/json".to_string()),
            body: format!(r#"{{"token":"{}","bogus":1}}"#, server.token).into_bytes(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(rejected_claim.status, 400);
    assert_eq!(error_code(&rejected_claim), "invalid_request");
    assert!(rejected_claim.set_cookie().is_none());

    let cookie = server.claim_cookie().await;

    // Positive control: a legal create passes.
    let created = raw_request(
        server.addr,
        RawRequest {
            method: "POST".to_string(),
            path: "/api/sessions".to_string(),
            origin: Some(server.origin()),
            cookie: Some(cookie.clone()),
            content_type: Some("application/json".to_string()),
            body: br#"{"message":"hello"}"#.to_vec(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(created.status, 201, "legal create still passes");
    let session_id = created.json()["session_id"]
        .as_str()
        .expect("session id")
        .to_string();

    // Every inbound JSON body rejects unknown fields, including nested
    // composer and question-answer objects.
    let post = |path: String, body: &'static [u8]| RawRequest {
        method: "POST".to_string(),
        path,
        origin: Some(server.origin()),
        cookie: Some(cookie.clone()),
        content_type: Some("application/json".to_string()),
        body: body.to_vec(),
        ..RawRequest::default()
    };
    let cases = [
        post(
            "/api/sessions".to_string(),
            br#"{"message":"hi","bogus":1}"#,
        ),
        post(
            "/api/sessions".to_string(),
            br#"{"message":"hi","composer":{"model":"m","bogus":1}}"#,
        ),
        post(
            format!("/api/sessions/{session_id}/turns"),
            br#"{"message":"hi","bogus":1}"#,
        ),
        post(
            format!("/api/sessions/{session_id}/input"),
            br#"{"turn_id":"t1","delivery":"follow_up","message":"hi","bogus":1}"#,
        ),
        post(
            format!("/api/sessions/{session_id}/question"),
            br#"{"turn_id":"t1","question_id":"q1","answer":{"selections":[],"bogus":1}}"#,
        ),
    ];
    for case in cases {
        let response = raw_request(server.addr, case.clone()).await;
        assert_eq!(response.status, 400, "{} {}", case.method, case.path);
        assert_eq!(error_code(&response), "invalid_request");
    }

    let metadata = raw_request(
        server.addr,
        RawRequest {
            method: "PATCH".to_string(),
            path: format!("/api/sessions/{session_id}"),
            origin: Some(server.origin()),
            cookie: Some(cookie.clone()),
            content_type: Some("application/json".to_string()),
            body: br#"{"title":"x","bogus":1}"#.to_vec(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(metadata.status, 400, "metadata unknown field");
    assert_eq!(error_code(&metadata), "invalid_request");

    // Query parameters are a fixed whitelist: misspelled keys are rejected
    // instead of silently ignored.
    for path in [
        "/api/sessions?bogus=1".to_string(),
        format!("/api/sessions/{session_id}/tool-output/ref_1?bogus=1"),
    ] {
        let response = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: path.clone(),
                cookie: Some(cookie.clone()),
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 400, "query whitelist {path}");
        assert_eq!(error_code(&response), "invalid_request", "{path}");
    }

    // Positive control: whitelisted query keys reach the handler.
    let listed = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/sessions?scope=active&limit=10".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(listed.status, 200, "legal list query still passes");
    let tool_output = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: format!("/api/sessions/{session_id}/tool-output/ref_1?start_line=1&max_lines=10"),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_ne!(
        tool_output.status, 400,
        "legal tool-output query keys pass the whitelist"
    );

    // Long-connection subscriptions reject unknown fields too.
    let mut ws = super::http_server::ws_connect(server.addr, &cookie, &server.origin())
        .await
        .expect("websocket upgrade");
    futures::SinkExt::send(
        &mut ws,
        tokio_tungstenite::tungstenite::Message::Text(
            r#"{"type":"watch_session","session_id":"s1","bogus":1}"#.into(),
        ),
    )
    .await
    .expect("send bogus watch");
    let reply = futures::StreamExt::next(&mut ws)
        .await
        .expect("watch error")
        .expect("watch error ok");
    let tokio_tungstenite::tungstenite::Message::Text(body) = reply else {
        panic!("expected a text watch error");
    };
    let body: neo_webui::protocol::WebUiErrorBody =
        serde_json::from_str(&body).expect("error shape");
    assert_eq!(body.code, neo_webui::WebUiErrorCode::InvalidRequest);
}

#[tokio::test]
async fn write_and_websocket_requests_reject_wrong_or_missing_origin() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    let good_origin = server.origin();
    let bad_origins = [
        None,                                    // missing
        Some("http://evil.example".to_string()), // different host
        Some("null".to_string()),                // null origin
        Some(format!("{good_origin}/")),         // trailing slash
        Some("http://127.0.0.1:9999".to_string()),
        Some("http://localhost:1234".to_string()),
    ];

    for origin in &bad_origins {
        let response = raw_request(
            server.addr,
            RawRequest {
                method: "POST".to_string(),
                path: "/api/sessions".to_string(),
                origin: origin.clone(),
                cookie: Some(cookie.clone()),
                content_type: Some("application/json".to_string()),
                body: br#"{"message":"hi"}"#.to_vec(),
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 400, "write origin {origin:?}");
        assert_eq!(
            error_code(&response),
            "invalid_request",
            "origin {origin:?}"
        );
    }

    // Multiple Origin values are rejected as well.
    let response = raw_request(
        server.addr,
        RawRequest {
            method: "POST".to_string(),
            path: "/api/sessions".to_string(),
            origin: Some(good_origin.clone()),
            cookie: Some(cookie.clone()),
            content_type: Some("application/json".to_string()),
            body: br#"{"message":"hi"}"#.to_vec(),
            extra_headers: vec![("Origin".to_string(), "http://evil.example".to_string())],
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(response.status, 400, "duplicate origin");

    // WebSocket upgrades with wrong or missing origins are rejected before
    // the upgrade.
    for origin in &bad_origins {
        let response = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: "/api/events".to_string(),
                origin: origin.clone(),
                cookie: Some(cookie.clone()),
                upgrade: true,
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 400, "ws origin {origin:?}");
        assert_eq!(
            error_code(&response),
            "invalid_request",
            "ws origin {origin:?}"
        );
    }

    // Positive controls: reads need no Origin; writes with the exact Origin
    // succeed.
    let read = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/sessions".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(read.status, 200, "reads do not require an origin");
    let write = raw_request(
        server.addr,
        RawRequest {
            method: "POST".to_string(),
            path: "/api/sessions".to_string(),
            origin: Some(good_origin),
            cookie: Some(cookie),
            content_type: Some("application/json".to_string()),
            body: br#"{"message":"hi"}"#.to_vec(),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(write.status, 201, "exact origin write succeeds");
}

#[tokio::test]
async fn lookalike_cookie_names_never_mask_the_real_session_cookie() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    // Another loopback origin can set any cookie name; a name that merely
    // starts with `neo_webui_session` must not invalidate the request and
    // must never be mistaken for the real session cookie.
    let response = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some(format!("neo_webui_sessionX=spoofed; {cookie}")),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(
        response.status, 200,
        "real cookie accepted despite a lookalike name: {:?}",
        response.body
    );
    // A lookalike alone is not a valid session cookie.
    let rejected = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some("neo_webui_sessionX=spoofed".to_string()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(rejected.status, 401, "lookalike name alone is unauthorized");
}

#[tokio::test]
async fn requests_with_wrong_host_are_rejected() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    let good = server.addr.to_string();
    let bad_hosts = [
        Some("localhost:1234".to_string()),
        Some(format!("127.0.0.1:{}", server.addr.port() + 1)),
        Some("127.0.0.1".to_string()),
        Some(format!("{good}.")), // trailing dot is not the exact value
        Some("EXAMPLE.COM".to_string()),
    ];
    for host in &bad_hosts {
        let response = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: "/api/bootstrap".to_string(),
                host: host.clone(),
                cookie: Some(cookie.clone()),
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 400, "host {host:?}");
        assert_eq!(error_code(&response), "invalid_request", "host {host:?}");
    }
    // A missing Host header is rejected as well (the HTTP layer itself
    // answers 400 before the guard can run).
    let missing = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            omit_host: true,
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(missing.status, 400, "missing Host is rejected");
    // The exact single Host value works.
    let ok = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            host: Some(good),
            cookie: Some(cookie),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(ok.status, 200);
}

#[tokio::test]
async fn websocket_upgrade_without_cookie_is_rejected() {
    let server = TestServer::start().await;
    let response = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/events".to_string(),
            origin: Some(server.origin()),
            upgrade: true,
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(response.status, 401);
    assert_eq!(error_code(&response), "unauthorized");
    let clear = response.set_cookie().expect("stale cookie is cleared");
    assert!(
        clear.contains("Max-Age=0"),
        "clearing expires the cookie: {clear}"
    );

    // A stale cookie on a regular read is rejected and cleared too.
    let stale = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some("stale-value".to_string()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(stale.status, 401);
    assert!(
        stale
            .set_cookie()
            .is_some_and(|value| value.contains("Max-Age=0"))
    );
}

#[tokio::test]
async fn oversized_request_bodies_are_rejected_with_too_large() {
    let server = TestServer::start().await;
    let cookie = server.claim_cookie().await;
    let request = |body: Vec<u8>| RawRequest {
        method: "POST".to_string(),
        path: "/api/sessions".to_string(),
        origin: Some(server.origin()),
        cookie: Some(cookie.clone()),
        content_type: Some("application/json".to_string()),
        body,
        ..RawRequest::default()
    };
    let huge = vec![b'x'; 300 * 1024];
    let response = raw_request(server.addr, request(huge)).await;
    assert_eq!(response.status, 413);
    assert_eq!(error_code(&response), "too_large");
    // At the exact 256 KiB boundary the size check passes and JSON parsing
    // rejects the non-JSON body with invalid_request instead.
    let boundary = vec![b'x'; 256 * 1024];
    let response = raw_request(server.addr, request(boundary)).await;
    assert_eq!(response.status, 400);
    assert_eq!(error_code(&response), "invalid_request");
}

#[test]
fn sensitive_material_never_reaches_logs() {
    use std::io::Write;

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl tracing_subscriber::fmt::MakeWriter<'_> for Capture {
        type Writer = Capture;
        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    let captured = Arc::new(Mutex::new(Vec::new()));
    let secrets = Arc::new(Mutex::new(Vec::<String>::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_writer(Capture(captured.clone()))
        .with_max_level(tracing::Level::DEBUG)
        .without_time()
        .finish();
    // One global subscriber per test process: thread-local defaults are
    // unreliable under tracing's global callsite interest cache when other
    // tests run in parallel.
    tracing::subscriber::set_global_default(subscriber)
        .expect("no other test installs a global subscriber");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime");

    runtime.block_on(async {
        let server = TestServer::start().await;
        let set_cookie = server.claim().await;
        let cookie_value = cookie_pair(&set_cookie);
        // Failed claim (wrong token), authenticated read, stale-cookie
        // read, and an unauth'd websocket upgrade attempt.
        let _ = raw_request(
            server.addr,
            RawRequest {
                method: "POST".to_string(),
                path: "/api/auth/claim".to_string(),
                origin: Some(server.origin()),
                content_type: Some("application/json".to_string()),
                body: br#"{"token":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#.to_vec(),
                ..RawRequest::default()
            },
        )
        .await;
        let _ = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: "/api/bootstrap".to_string(),
                cookie: Some(cookie_value.clone()),
                ..RawRequest::default()
            },
        )
        .await;
        let _ = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: "/api/bootstrap".to_string(),
                cookie: Some("bogus".to_string()),
                ..RawRequest::default()
            },
        )
        .await;
        let _ = raw_request(
            server.addr,
            RawRequest {
                method: "GET".to_string(),
                path: "/api/events".to_string(),
                origin: Some(server.origin()),
                upgrade: true,
                ..RawRequest::default()
            },
        )
        .await;
        secrets.lock().expect("secrets poisoned").extend([
            server.token.clone(),
            cookie_value,
            set_cookie,
            "neo_webui_session=".to_string(),
            r#"{"token":""#.to_string(),
        ]);
    });

    let captured_logs = captured.lock().expect("capture poisoned").clone();
    let logs = String::from_utf8_lossy(&captured_logs);
    assert!(
        logs.contains("web request rejected"),
        "capture works: {logs}"
    );
    for secret in secrets.lock().expect("secrets poisoned").iter() {
        assert!(
            !logs.contains(secret.as_str()),
            "sensitive material reached logs: {secret}"
        );
    }
}

#[tokio::test]
async fn responses_carry_security_headers_and_no_cross_origin_headers() {
    let server = TestServer::start().await;
    // The claim response itself carries the security headers and issues the
    // cookie; the token is consumed here, so the read below uses the cookie
    // from this response.
    let claim = raw_request(server.addr, claim_request(&server, &server.token)).await;
    assert_eq!(claim.status, 204);
    assert_eq!(claim.header("cache-control"), Some("no-store"));
    assert_eq!(claim.header("referrer-policy"), Some("no-referrer"));
    let cookie = cookie_pair(claim.set_cookie().expect("claim sets a cookie"));
    let response = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some(cookie.clone()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(response.status, 200);
    assert_eq!(response.header("cache-control"), Some("no-store"));
    assert_eq!(response.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(response.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        response.header("content-security-policy"),
        Some(neo_webui::CONTENT_SECURITY_POLICY)
    );
    for (name, _) in &response.headers {
        assert!(
            !name.starts_with("access-control-"),
            "no cross-origin headers, got {name}"
        );
    }
    // Error responses carry the headers too.
    let bad = raw_request(
        server.addr,
        RawRequest {
            method: "GET".to_string(),
            path: "/api/bootstrap".to_string(),
            cookie: Some("nope".to_string()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(bad.status, 401);
    assert_eq!(bad.header("cache-control"), Some("no-store"));
    assert_eq!(bad.header("referrer-policy"), Some("no-referrer"));
}
