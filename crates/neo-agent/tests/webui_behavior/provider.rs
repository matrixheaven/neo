//! Programmable mock SSE provider for `openai_response`-type requests.
//!
//! Each `Step` consumes exactly one connection: the full request (headers and
//! body) is read and recorded, then the step either responds immediately with
//! a fixed SSE body or holds the connection until [`Provider::release_next`]
//! is called (a barrier for tests that must observe a state before the model
//! answers). A held connection whose client cancels is tolerated.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;

/// One scripted provider step for one accepted connection.
pub(crate) enum Step {
    /// Respond immediately with this SSE body.
    Respond(String),
    /// Hold the connection until released, then respond with this SSE body.
    HoldThenRespond(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) body: String,
}

struct ProviderInner {
    requests: Mutex<Vec<RecordedRequest>>,
    holds: Mutex<VecDeque<Arc<tokio::sync::Notify>>>,
}

/// Programmable provider bound to a random loopback port.
pub(crate) struct Provider {
    pub(crate) url: String,
    pub(crate) port: u16,
    inner: Arc<ProviderInner>,
    task: JoinHandle<()>,
}

impl Provider {
    pub(crate) fn start(steps: Vec<Step>) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind provider");
        let port = listener.local_addr().expect("provider addr").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking provider");
        let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
        let inner = Arc::new(ProviderInner {
            requests: Mutex::new(Vec::new()),
            holds: Mutex::new(VecDeque::new()),
        });
        let task_inner = Arc::clone(&inner);
        let task = tokio::spawn(async move {
            for step in steps {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let task_inner = Arc::clone(&task_inner);
                tokio::spawn(async move {
                    let mut socket = socket;
                    let request = match read_http_request(&mut socket).await {
                        Some(request) => request,
                        None => return,
                    };
                    if let Ok(mut requests) = task_inner.requests.lock() {
                        requests.push(request);
                    }
                    let (hold, body) = match step {
                        Step::Respond(body) => (None, body),
                        Step::HoldThenRespond(body) => {
                            let notify = Arc::new(tokio::sync::Notify::new());
                            if let Ok(mut holds) = task_inner.holds.lock() {
                                holds.push_back(Arc::clone(&notify));
                            }
                            (Some(notify), body)
                        }
                    };
                    if let Some(notify) = hold {
                        notify.notified().await;
                    }
                    if socket.write_all(body.as_bytes()).await.is_err() {
                        // The client cancelled (e.g. the turn was cancelled while
                        // the request was held); the script continues.
                    }
                });
            }
        });
        Self {
            url: format!("http://127.0.0.1:{port}"),
            port,
            inner,
            task,
        }
    }

    /// Release the oldest held connection (readiness barrier).
    pub(crate) async fn release_next(&self) {
        let notify = loop {
            let next = self
                .inner
                .holds
                .lock()
                .expect("provider holds lock")
                .pop_front();
            match next {
                Some(notify) => break notify,
                None => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        };
        notify.notify_one();
    }

    /// Wait until at least `count` requests have been recorded (readiness
    /// signal; fails fast after the deadline so provider desyncs never hang).
    pub(crate) async fn wait_for_requests(&self, count: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let seen = self
                .inner
                .requests
                .lock()
                .expect("provider requests lock")
                .len();
            if seen >= count {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "provider timed out waiting for {count} requests (saw {seen})"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.inner
            .requests
            .lock()
            .expect("provider requests lock")
            .clone()
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Read one HTTP/1.1 request (headers plus content-length body).
async fn read_http_request(socket: &mut TcpStream) -> Option<RecordedRequest> {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 4096];
    let header_end;
    loop {
        let read = socket.read(&mut temp).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            header_end = index;
            break;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
    }
    let headers_raw = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let mut lines = headers_raw.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut temp).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body =
        String::from_utf8_lossy(&buffer[body_start..body_start + content_length]).to_string();
    Some(RecordedRequest { method, path, body })
}

/// SSE body for a completed `openai_response` stream with plain text.
pub(crate) fn openai_response_sse(id: &str, text: &str) -> String {
    sse_response(&[
        serde_json::json!({ "type": "response.created", "response": { "id": id } }),
        serde_json::json!({ "type": "response.output_text.delta", "delta": text }),
        serde_json::json!({
            "type": "response.completed",
            "response": { "status": "completed", "usage": { "input_tokens": 7, "output_tokens": 3 } }
        }),
    ])
}

/// SSE body for one function-call item followed by a completed response.
pub(crate) fn openai_tool_call_sse(id: &str, tool: &str, call_id: &str, arguments: &str) -> String {
    sse_response(&[
        serde_json::json!({ "type": "response.created", "response": { "id": id } }),
        serde_json::json!({
            "type": "response.output_item.added",
            "item": { "id": "item-1", "type": "function_call", "call_id": call_id, "name": tool, "arguments": "" }
        }),
        serde_json::json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "item-1",
            "delta": arguments,
        }),
        serde_json::json!({
            "type": "response.output_item.done",
            "item": { "id": "item-1", "type": "function_call", "call_id": call_id, "name": tool, "arguments": arguments }
        }),
        serde_json::json!({
            "type": "response.completed",
            "response": { "status": "completed" }
        }),
    ])
}

pub(crate) fn sse_response(events: &[serde_json::Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str(&format!("data: {event}\n\n"));
    }
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}
