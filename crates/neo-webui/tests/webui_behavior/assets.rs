//! Embedded static resources: the exact-path allowlist serves the
//! compile-time embedded `web/dist` bytes to anonymous reads with the fixed
//! MIME types and the security headers, and every other path — SPA-looking
//! routes, traversal attempts, directory prefixes — gets the stable 404.

use super::http_server::{RawRequest, TestServer, raw_request};

// The same compile-time embedding the server uses; comparing response bytes
// against them proves the served content is the embedded build artifact.
const INDEX_HTML: &[u8] = include_bytes!("../../web/dist/index.html");
const APP_JS: &[u8] = include_bytes!("../../web/dist/assets/neo-webui.js");
const APP_CSS: &[u8] = include_bytes!("../../web/dist/assets/neo-webui.css");

#[tokio::test]
async fn embedded_assets_are_allowlisted_anonymous_and_non_fallback() {
    let server = TestServer::start().await;

    // Anonymous reads (no cookie) of the three embedded resources.
    for (path, content_type, bytes) in [
        ("/", "text/html; charset=utf-8", INDEX_HTML),
        ("/index.html", "text/html; charset=utf-8", INDEX_HTML),
        (
            "/assets/neo-webui.js",
            "text/javascript; charset=utf-8",
            APP_JS,
        ),
        ("/assets/neo-webui.css", "text/css; charset=utf-8", APP_CSS),
    ] {
        let response = raw_request(
            server.addr,
            RawRequest {
                path: path.to_string(),
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 200, "{path}: {}", response.body_str());
        assert_eq!(
            response.header("content-type"),
            Some(content_type),
            "{path}"
        );
        assert_eq!(response.body, bytes, "{path} serves the embedded bytes");
        assert!(
            response.header("content-security-policy").is_some(),
            "{path} carries the security headers"
        );
    }

    // Everything else is the stable 404: unknown names, SPA-looking routes
    // (no fallback), directory prefixes, traversal and encoded traversal.
    for path in [
        "/favicon.ico",
        "/app/sessions",
        "/assets/",
        "/assets/neo-webui.json",
        "/assets/../src/server.rs",
        "/assets/%2e%2e/secret",
        "/index.html/extra",
    ] {
        let response = raw_request(
            server.addr,
            RawRequest {
                path: path.to_string(),
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(response.status, 404, "{path}: {}", response.body_str());
        assert_eq!(response.error_code(), "not_found", "{path}");
    }

    // The exact host check still gates static reads.
    let bad_host = raw_request(
        server.addr,
        RawRequest {
            path: "/".to_string(),
            host: Some("example.com".to_string()),
            ..RawRequest::default()
        },
    )
    .await;
    assert_eq!(bad_host.status, 400, "{}", bad_host.body_str());
}
