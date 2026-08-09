//! Raw HTTP/1.1 client for the loopback web service: full control over Host,
//! Origin and Cookie headers (reqwest refuses to override Host). Responses are
//! parsed from status line, headers and content-length body.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug, Clone)]
pub(crate) struct HttpResult {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl HttpResult {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// One request against `127.0.0.1:<port>`. `origin` adds the exact loopback
/// `Origin` header required for claims, writes and the long connection;
/// `cookie` adds the session cookie when present.
pub(crate) async fn http_request(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    cookie: Option<&str>,
    origin: bool,
) -> HttpResult {
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    request.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    if origin {
        request.push_str(&format!("Origin: http://127.0.0.1:{port}\r\n"));
    }
    if let Some(cookie) = cookie {
        request.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    match body {
        Some(body) => {
            request.push_str("Content-Type: application/json\r\n");
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        None => {
            request.push_str("Content-Length: 0\r\n");
        }
    }
    request.push_str("Connection: close\r\n\r\n");
    if let Some(body) = body {
        request.push_str(body);
    }

    // The service prints its address before the accept loop starts; retry the
    // connect briefly so a freshly printed address cannot race the listener.
    let mut connected = None;
    for _ in 0..50 {
        match TcpStream::connect(("127.0.0.1", port)).await {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let mut socket = match connected {
        Some(socket) => socket,
        None => {
            return HttpResult {
                status: 0,
                headers: Vec::new(),
                body: "connect refused".to_owned(),
            };
        }
    };
    socket
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut buffer = Vec::new();
    let mut temp = [0_u8; 8192];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut temp))
            .await
            .expect("read response")
            .expect("read");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    parse_response(&buffer)
}

fn parse_response(buffer: &[u8]) -> HttpResult {
    let header_end = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(buffer.len());
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_owned(), value.trim().to_owned()))
        .collect();
    let body = if header_end < buffer.len() {
        String::from_utf8_lossy(&buffer[header_end + 4..]).to_string()
    } else {
        String::new()
    };
    HttpResult {
        status,
        headers,
        body,
    }
}

/// Exchange the one-time token for the strict session cookie.
pub(crate) async fn claim_token(port: u16, token: &str) -> Result<String, String> {
    let body = serde_json::json!({ "token": token }).to_string();
    let response = http_request(port, "POST", "/api/auth/claim", Some(&body), None, true).await;
    if response.status != 204 {
        return Err(format!("claim status {}: {response:?}", response.status));
    }
    let cookie = response
        .header("set-cookie")
        .ok_or_else(|| "claim set no cookie".to_owned())?
        .split(';')
        .next()
        .ok_or_else(|| "empty cookie".to_owned())?
        .to_owned();
    Ok(cookie)
}

/// POST one JSON body and return the parsed JSON response.
pub(crate) async fn post_json(
    port: u16,
    cookie: &str,
    path: &str,
    body: &serde_json::Value,
) -> HttpResult {
    http_request(
        port,
        "POST",
        path,
        Some(&body.to_string()),
        Some(cookie),
        true,
    )
    .await
}

/// PATCH one JSON body and return the parsed JSON response.
pub(crate) async fn patch_json(
    port: u16,
    cookie: &str,
    path: &str,
    body: &serde_json::Value,
) -> HttpResult {
    http_request(
        port,
        "PATCH",
        path,
        Some(&body.to_string()),
        Some(cookie),
        true,
    )
    .await
}

/// GET with the session cookie (reads do not require an Origin).
pub(crate) async fn get(port: u16, cookie: &str, path: &str) -> HttpResult {
    http_request(port, "GET", path, None, Some(cookie), false).await
}

/// Poll an async check until it returns `Some` or the deadline passes,
/// returning `None` on timeout so callers can dump diagnostics.
pub(crate) async fn poll_until_async<T, F, Check>(
    mut check: F,
    deadline: Duration,
    what: &str,
) -> Option<T>
where
    F: FnMut() -> Check,
    Check: std::future::Future<Output = Option<T>>,
{
    let start = std::time::Instant::now();
    loop {
        if let Some(value) = check().await {
            return Some(value);
        }
        if start.elapsed() >= deadline {
            eprintln!("timed out waiting for {what}");
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
