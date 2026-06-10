use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
};

use neo_sdk::CloudClient;
use serde_json::{Value, json};

#[tokio::test]
async fn cloud_client_uses_hosted_session_share_api_with_bearer_auth() {
    let server = MockHttpServer::start(vec![
        json_response(&json!({
            "record": session_record("cs_alpha", Value::Null)
        })),
        json_response(&json!({
            "sessions": [session_record("cs_alpha", Value::Null)]
        })),
        json_response(&json!({
            "record": session_record("cs_alpha", Value::Null),
            "messages": [{"User": {"content": [{"Text": {"text": "hello"}}]}}]
        })),
        json_response(&json!({
            "record": session_record("cs_fork", json!("cs_alpha"))
        })),
        json_response(&json!({
            "record": {
                "id": "sh_alpha",
                "session_id": "cs_alpha",
                "public": true,
                "html_url": "/v1/shares/sh_alpha.html",
                "json_url": "/v1/shares/sh_alpha.json",
                "created_at": "1"
            },
            "html": "<!doctype html>",
            "json": {"format": "neo.cloud.session.share"}
        })),
        json_response(&json!({
            "record": {
                "id": "sh_alpha",
                "session_id": "cs_alpha",
                "public": true,
                "html_url": "/v1/shares/sh_alpha.html",
                "json_url": "/v1/shares/sh_alpha.json",
                "created_at": "1"
            },
            "html": "<!doctype html>",
            "json": {"format": "neo.cloud.session.share"}
        })),
    ]);
    let client = CloudClient::new(&server.url);
    let token = "neo_at_test";

    let imported = client
        .import_session(
            token,
            "alpha",
            "{}\n".to_owned(),
            Some("Main".to_owned()),
            None,
        )
        .await
        .expect("import");
    assert_eq!(imported.id, "cs_alpha");
    assert_eq!(client.list_sessions(token).await.expect("list").len(), 1);
    assert_eq!(
        client
            .get_session(token, "cs_alpha")
            .await
            .expect("get")
            .record
            .id,
        "cs_alpha"
    );
    assert_eq!(
        client
            .fork_session(token, "cs_alpha")
            .await
            .expect("fork")
            .remote_parent_id
            .as_deref(),
        Some("cs_alpha")
    );
    assert_eq!(
        client
            .create_share(token, "cs_alpha", true)
            .await
            .expect("share")
            .record
            .id,
        "sh_alpha"
    );
    assert_eq!(
        client
            .get_share("sh_alpha")
            .await
            .expect("public share")
            .record
            .id,
        "sh_alpha"
    );

    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("POST", "/v1/sessions/import"),
            ("GET", "/v1/sessions"),
            ("GET", "/v1/sessions/cs_alpha"),
            ("POST", "/v1/sessions/cs_alpha/fork"),
            ("POST", "/v1/sessions/cs_alpha/shares"),
            ("GET", "/v1/shares/sh_alpha"),
        ]
    );
    for request in &requests[..5] {
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer neo_at_test")
        );
    }
    assert_eq!(requests[0].body["local_session_id"], "alpha");
    assert_eq!(requests[0].body["name"], "Main");
    assert_eq!(requests[4].body["public"], true);
    assert!(!requests[5].headers.contains_key("authorization"));
}

fn session_record(id: &str, remote_parent_id: Value) -> Value {
    json!({
        "id": id,
        "local_session_id": "alpha",
        "name": "Main",
        "summary": null,
        "remote_parent_id": remote_parent_id,
        "share_ids": [],
        "message_count": 1,
        "created_at": "1",
        "updated_at": "1"
    })
}

#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

struct MockHttpServer {
    url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockHttpServer {
    fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        std::thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().expect("accept request");
                captured_requests
                    .lock()
                    .expect("lock requests")
                    .push(read_http_request(&mut socket));
                socket
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });

        Self { url, requests }
    }

    fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

fn json_response(body: &Value) -> String {
    let body = body.to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn read_http_request(socket: &mut TcpStream) -> RecordedRequest {
    let mut buffer = Vec::new();
    let mut temp = [0_u8; 1024];
    let header_end;

    loop {
        let read = socket.read(&mut temp).expect("read request");
        assert_ne!(read, 0, "client closed before sending headers");
        buffer.extend_from_slice(&temp[..read]);
        if let Some(index) = find_header_end(&buffer) {
            header_end = index;
            break;
        }
    }

    let headers_raw = String::from_utf8(buffer[..header_end].to_vec()).expect("utf8 headers");
    let mut lines = headers_raw.split("\r\n");
    let request_line = lines.next().expect("request line");
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().expect("method").to_owned();
    let path = request_parts.next().expect("path").to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_owned()))
        .collect::<BTreeMap<_, _>>();
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = socket.read(&mut temp).expect("read body");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&temp[..read]);
    }
    let body_bytes = &buffer[body_start..body_start + content_length];
    let body = if body_bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body_bytes).expect("json body")
    };

    RecordedRequest {
        method,
        path,
        headers,
        body,
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
