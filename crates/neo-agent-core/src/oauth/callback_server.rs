//! Short-lived local HTTP callback server for OAuth 2.0 authorization-code flows.
//!
//! Binds to `127.0.0.1:0`, accepts a single GET `/callback` request, validates
//! the `state` parameter, and returns the authorization `code`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot::{Receiver, Sender};

use super::OAuthError;

/// Authorization code received from the OAuth callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackCode {
    /// Authorization code to exchange for tokens.
    pub code: String,
    /// State parameter echoed by the authorization server.
    pub state: String,
}

type ResultSender = Sender<Result<CallbackCode, OAuthError>>;
type SharedResultSender = Arc<tokio::sync::Mutex<Option<ResultSender>>>;

/// A short-lived local HTTP server that waits for the OAuth callback request.
#[derive(Debug)]
pub struct CallbackServer {
    /// The actual loopback port the server bound to.
    pub local_port: u16,
    shutdown: Option<Sender<()>>,
    result_rx: Receiver<Result<CallbackCode, OAuthError>>,
    timeout: Duration,
}

impl CallbackServer {
    /// Start a new callback server that validates the given `state` and waits up
    /// to `timeout` for a callback.
    ///
    /// The server binds to `127.0.0.1:0` and reports the chosen port in
    /// [`Self::local_port`].
    pub async fn start(expected_state: String, timeout: Duration) -> Result<Self, OAuthError> {
        Self::start_inner(Some(expected_state), timeout).await
    }

    /// Start a callback server that accepts any `state` parameter.
    ///
    /// This is useful when the caller validates the state itself (e.g. rmcp's
    /// `AuthorizationManager` validates state during token exchange).
    pub async fn start_unvalidated(timeout: Duration) -> Result<Self, OAuthError> {
        Self::start_inner(None, timeout).await
    }

    async fn start_inner(
        expected_state: Option<String>,
        timeout: Duration,
    ) -> Result<Self, OAuthError> {
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|err| OAuthError::CallbackServer(err.to_string()))?;
        let local_port = listener
            .local_addr()
            .map_err(|err| OAuthError::CallbackServer(err.to_string()))?
            .port();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(run_server(
            listener,
            local_port,
            expected_state,
            result_tx,
            shutdown_rx,
        ));

        Ok(Self {
            local_port,
            shutdown: Some(shutdown_tx),
            result_rx,
            timeout,
        })
    }

    /// Wait for the callback request, or until the configured timeout elapses.
    ///
    /// On success, returns the validated [`CallbackCode`]. On timeout, the
    /// server is shut down and [`OAuthError::CallbackTimeout`] is returned.
    pub async fn wait_for_code(mut self) -> Result<CallbackCode, OAuthError> {
        match tokio::time::timeout(self.timeout, self.result_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(OAuthError::CallbackServer(
                "callback server result channel closed".to_string(),
            )),
            Err(_) => {
                if let Some(tx) = self.shutdown.take() {
                    let _ = tx.send(());
                }
                Err(OAuthError::CallbackTimeout(self.timeout))
            }
        }
    }
}

async fn run_server(
    listener: TcpListener,
    local_port: u16,
    expected_state: Option<String>,
    result_tx: ResultSender,
    mut shutdown_rx: Receiver<()>,
) {
    let result_tx: SharedResultSender = Arc::new(tokio::sync::Mutex::new(Some(result_tx)));

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => break,
            accept_result = listener.accept() => match accept_result {
                Ok((stream, _)) => {
                    let expected_state = expected_state.clone();
                    let result_tx_for_handle = Arc::clone(&result_tx);
                    let mut handle = tokio::spawn(handle_connection(
                        stream,
                        local_port,
                        expected_state,
                        result_tx_for_handle,
                    ));
                    tokio::select! {
                        biased;
                        _ = &mut shutdown_rx => {
                            handle.abort();
                            break;
                        }
                        _ = &mut handle => {
                            if result_tx.lock().await.is_none() {
                                break;
                            }
                        }
                    }
                }
                Err(err) => {
                    send_result(
                        &result_tx,
                        Err(OAuthError::CallbackServer(err.to_string())),
                    )
                    .await;
                    break;
                }
            },
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    local_port: u16,
    expected_state: Option<String>,
    result_tx: SharedResultSender,
) {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let Some(first_line) = read_request_line(&mut reader).await else {
        let _ = writer.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
        return;
    };

    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "GET" {
        let body = error_html("Only GET /callback is supported.");
        let _ = writer
            .write_all(response_bytes(400, "Bad Request", &body).as_bytes())
            .await;
        send_result(
            &result_tx,
            Err(OAuthError::CallbackServer(
                "invalid callback request".to_string(),
            )),
        )
        .await;
        return;
    }

    let path = parts[1];
    if !path.starts_with("/callback") {
        let body = error_html("Not found.");
        let _ = writer
            .write_all(response_bytes(404, "Not Found", &body).as_bytes())
            .await;
        return;
    }

    let (code, state) = match parse_callback_params(path, local_port) {
        Ok(params) => params,
        Err(err) => {
            let body = error_html(&format!("Invalid callback URL: {err}"));
            let _ = writer
                .write_all(response_bytes(400, "Bad Request", &body).as_bytes())
                .await;
            send_result(
                &result_tx,
                Err(OAuthError::CallbackServer(format!(
                    "invalid callback URL: {err}"
                ))),
            )
            .await;
            return;
        }
    };

    let got_state = state.unwrap_or_default();
    if let Some(ref expected) = expected_state
        && got_state != *expected
    {
        let reason = format!("State mismatch: expected {expected}, got {got_state}.");
        let body = error_html(&reason);
        let _ = writer
            .write_all(response_bytes(400, "Bad Request", &body).as_bytes())
            .await;
        send_result(
            &result_tx,
            Err(OAuthError::CallbackStateMismatch {
                expected: expected.clone(),
                got: got_state,
            }),
        )
        .await;
        return;
    }

    let Some(code) = code else {
        let body = error_html("Missing authorization code.");
        let _ = writer
            .write_all(response_bytes(400, "Bad Request", &body).as_bytes())
            .await;
        send_result(&result_tx, Err(OAuthError::CallbackMissingCode)).await;
        return;
    };

    let body = success_html();
    let _ = writer
        .write_all(response_bytes(200, "OK", &body).as_bytes())
        .await;
    send_result(
        &result_tx,
        Ok(CallbackCode {
            code,
            state: got_state,
        }),
    )
    .await;
}

async fn read_request_line(
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Option<String> {
    let mut first_line = String::new();
    if reader.read_line(&mut first_line).await.is_err() {
        return None;
    }

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
        }
    }

    Some(first_line)
}

fn parse_callback_params(
    path: &str,
    local_port: u16,
) -> Result<(Option<String>, Option<String>), String> {
    let base = format!("http://127.0.0.1:{local_port}{path}");
    let url = reqwest::Url::parse(&base).map_err(|err| err.to_string())?;

    let mut code = None;
    let mut state = None;
    for (key, value) in url.query_pairs() {
        if key == "code" {
            code = Some(value.into_owned());
        } else if key == "state" {
            state = Some(value.into_owned());
        }
    }

    Ok((code, state))
}

async fn send_result(result_tx: &SharedResultSender, result: Result<CallbackCode, OAuthError>) {
    if let Some(tx) = result_tx.lock().await.take() {
        let _ = tx.send(result);
    }
}

fn success_html() -> String {
    "<!DOCTYPE html>\
     <html>\
     <head><title>Authorization successful</title></head>\
     <body>\
     <h1>Authorization successful</h1>\
     <p>You can close this tab.</p>\
     </body>\
     </html>"
        .to_string()
}

fn error_html(reason: &str) -> String {
    format!(
        "<!DOCTYPE html>\
         <html>\
         <head><title>Authorization failed</title></head>\
         <body>\
         <h1>Authorization failed</h1>\
         <p>{}</p>\
         </body>\
         </html>",
        html_escape(reason)
    )
}

fn response_bytes(status: u16, status_text: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    )
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            ch => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn callback_server_returns_code_on_valid_request() {
        let server = CallbackServer::start("expected-state".to_string(), Duration::from_secs(5))
            .await
            .unwrap();
        let port = server.local_port;
        let url = format!("http://127.0.0.1:{port}/callback?code=abc123&state=expected-state");
        let response = reqwest::get(&url).await.unwrap();
        assert_eq!(response.status(), 200);
        let body = response.text().await.unwrap();
        assert!(body.contains("Authorization successful"));

        let result = server.wait_for_code().await.unwrap();
        assert_eq!(result.code, "abc123");
        assert_eq!(result.state, "expected-state");
    }

    #[tokio::test]
    async fn callback_server_rejects_wrong_state() {
        let server = CallbackServer::start("expected-state".to_string(), Duration::from_secs(5))
            .await
            .unwrap();
        let port = server.local_port;
        let url = format!("http://127.0.0.1:{port}/callback?code=abc123&state=wrong-state");
        let response = reqwest::get(&url).await.unwrap();
        assert_eq!(response.status(), 400);
        let body = response.text().await.unwrap();
        assert!(body.contains("Authorization failed"));

        let err = server.wait_for_code().await.unwrap_err();
        match err {
            OAuthError::CallbackStateMismatch { expected, got } => {
                assert_eq!(expected, "expected-state");
                assert_eq!(got, "wrong-state");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn callback_server_rejects_missing_code() {
        let server = CallbackServer::start("expected-state".to_string(), Duration::from_secs(5))
            .await
            .unwrap();
        let port = server.local_port;
        let url = format!("http://127.0.0.1:{port}/callback?state=expected-state");
        let response = reqwest::get(&url).await.unwrap();
        assert_eq!(response.status(), 400);
        let body = response.text().await.unwrap();
        assert!(body.contains("Authorization failed"));

        let err = server.wait_for_code().await.unwrap_err();
        assert!(matches!(err, OAuthError::CallbackMissingCode));
    }

    #[tokio::test]
    async fn callback_server_times_out() {
        let server = CallbackServer::start("expected-state".to_string(), Duration::from_millis(50))
            .await
            .unwrap();
        let err = server.wait_for_code().await.unwrap_err();
        assert!(matches!(err, OAuthError::CallbackTimeout(_)));
    }
}
