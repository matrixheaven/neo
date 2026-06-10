use std::{
    collections::BTreeMap,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use neo_sdk::CloudClient;
use serde_json::{Value, json};

#[tokio::test]
async fn cloud_client_uses_self_hosted_collaboration_api_with_bearer_auth() {
    let server = MockHttpServer::start(self_hosted_collaboration_api_responses());
    let client = CloudClient::new(&server.url);
    let token = "neo_at_test";

    assert_eq!(
        client
            .update_branch(
                token,
                "cs_alpha",
                Some("Feature branch".to_owned()),
                Some("Local branch summary".to_owned()),
                None,
            )
            .await
            .expect("update branch")
            .summary
            .as_deref(),
        Some("Local branch summary")
    );
    assert_eq!(
        client
            .continue_session(
                token,
                "cs_alpha",
                Some("remote-child".to_owned()),
                Some("Child branch".to_owned()),
            )
            .await
            .expect("continue session")
            .record
            .remote_parent_id
            .as_deref(),
        Some("cs_alpha")
    );
    assert_eq!(
        client.session_tree(token).await.expect("session tree")[0].children,
        vec!["cs_child".to_owned()]
    );
    assert_eq!(
        client
            .list_share_records(token, "cs_child")
            .await
            .expect("share records")[0]
            .id,
        "sh_child"
    );
    assert_eq!(
        client
            .get_share_record(token, "sh_child")
            .await
            .expect("share record")
            .session_id,
        "cs_child"
    );
    let settings = client.pull_settings(token).await.expect("pull settings");
    assert_eq!(
        settings.settings.default_model.as_deref(),
        Some("deepseek-v4-pro")
    );
    assert_eq!(
        client
            .push_settings(token, settings.settings)
            .await
            .expect("push settings")
            .revision,
        3
    );
    let catalog = client.command_catalog(token).await.expect("catalog");
    assert_eq!(catalog.commands[0].name, "sessions.tree");
    assert!(catalog.commands[0].available);

    let requests = server.requests();
    assert_self_hosted_collaboration_requests(&requests);
}

#[tokio::test]
async fn cloud_client_uses_hosted_session_share_api_with_bearer_auth() {
    let server = MockHttpServer::start(hosted_session_share_api_responses());
    let client = CloudClient::new(&server.url);
    let token = "neo_at_test";

    let imported = client
        .import_session(
            token,
            "alpha",
            "{}\n".to_owned(),
            Some("Main".to_owned()),
            Some("Branch summary".to_owned()),
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
    assert_hosted_session_requests(&requests);
}

fn hosted_session_share_api_responses() -> Vec<String> {
    vec![
        json_response(&json!({
            "record": session_record("cs_alpha", None)
        })),
        json_response(&json!({
            "sessions": [session_record("cs_alpha", None)]
        })),
        json_response(&json!({
            "record": session_record("cs_alpha", None),
            "messages": [{"User": {"content": [{"Text": {"text": "hello"}}]}}]
        })),
        json_response(&json!({
            "record": session_record("cs_fork", Some("cs_alpha"))
        })),
        json_response(&share_payload_response()),
        json_response(&share_payload_response()),
    ]
}

fn share_payload_response() -> Value {
    json!({
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
    })
}

fn assert_hosted_session_requests(requests: &[RecordedRequest]) {
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
    assert_eq!(requests[0].body["summary"], "Branch summary");
    assert_eq!(requests[4].body["public"], true);
    assert!(!requests[5].headers.contains_key("authorization"));
}

fn self_hosted_collaboration_api_responses() -> Vec<String> {
    vec![
        json_response(&json!({
            "record": session_record_with_summary("cs_alpha", None, "Local branch summary")
        })),
        json_response(&json!({
            "record": session_record("cs_child", Some("cs_alpha")),
            "messages": [{"User": {"content": [{"Text": {"text": "hello"}}]}}]
        })),
        json_response(&json!({
            "tree": [
                {
                    "depth": 0,
                    "record": session_record("cs_alpha", None),
                    "children": ["cs_child"]
                },
                {
                    "depth": 1,
                    "record": session_record("cs_child", Some("cs_alpha")),
                    "children": []
                }
            ]
        })),
        json_response(&json!({
            "shares": [share_record_response()]
        })),
        json_response(&json!({
            "record": share_record_response()
        })),
        json_response(&json!({
            "settings": {
                "default_provider": "anthropic",
                "default_model": "deepseek-v4-pro"
            },
            "revision": 2,
            "updated_at": "2"
        })),
        json_response(&json!({
            "revision": 3,
            "updated_at": "3"
        })),
        json_response(&json!({
            "commands": [
                {
                    "name": "sessions.tree",
                    "description": "List self-hosted cloud sessions as a branch tree",
                    "method": "GET",
                    "path": "/v1/sessions/tree",
                    "available": true
                }
            ]
        })),
    ]
}

fn assert_self_hosted_collaboration_requests(requests: &[RecordedRequest]) {
    assert_eq!(
        requests
            .iter()
            .map(|request| (request.method.as_str(), request.path.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("PUT", "/v1/sessions/cs_alpha/branch"),
            ("POST", "/v1/sessions/cs_alpha/continue"),
            ("GET", "/v1/sessions/tree"),
            ("GET", "/v1/sessions/cs_child/shares"),
            ("GET", "/v1/share-records/sh_child"),
            ("GET", "/v1/settings"),
            ("PUT", "/v1/settings"),
            ("GET", "/v1/commands"),
        ]
    );
    for request in requests {
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer neo_at_test")
        );
    }
    assert_eq!(requests[0].body["name"], "Feature branch");
    assert_eq!(requests[0].body["summary"], "Local branch summary");
    assert_eq!(requests[1].body["local_session_id"], "remote-child");
    assert_eq!(requests[1].body["name"], "Child branch");
    assert_eq!(
        requests[6].body["settings"]["default_model"],
        "deepseek-v4-pro"
    );
}

fn session_record(id: &str, remote_parent_id: Option<&str>) -> Value {
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

fn session_record_with_summary(id: &str, remote_parent_id: Option<&str>, summary: &str) -> Value {
    let mut record = session_record(id, remote_parent_id);
    record["summary"] = Value::String(summary.to_owned());
    record
}

fn share_record_response() -> Value {
    json!({
        "id": "sh_child",
        "session_id": "cs_child",
        "public": true,
        "html_url": "/v1/shares/sh_child.html",
        "json_url": "/v1/shares/sh_child.json",
        "created_at": "2"
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
        listener
            .set_nonblocking(true)
            .expect("set mock server nonblocking");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        std::thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = accept_request(&listener);
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("set read timeout");
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .expect("set write timeout");
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

fn accept_request(listener: &TcpListener) -> (TcpStream, std::net::SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match listener.accept() {
            Ok(accepted) => return accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept request: {error}"),
        }
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
