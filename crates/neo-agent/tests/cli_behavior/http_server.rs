//! Shared CLI domain fixtures: isolated NEO_HOME setup, session
//! transcript writers, and the single converged mock SSE provider
//! (previously four per-file copies).
//!
//! Every consumer test binary links this whole module but uses only a
//! subset of the fixtures, so dead-code is expected here.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tempfile::TempDir;

pub(crate) const SESSION_A: &str = "session_00000000-0000-4000-8000-000000000201";

pub(crate) const SESSION_B: &str = "session_00000000-0000-4000-8000-000000000202";

pub(crate) const SESSION_CHILD: &str = "session_00000000-0000-4000-8000-000000000203";

pub(crate) fn neo() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_neo"));
    // Each test gets its own unique NEO_HOME so config writes (now under
    // ~/.neo, not the project .neo) don't collide between concurrent tests.
    command.env("NEO_HOME", neo_home_for_test());
    command
}

/// Unique per-test neo home directory. `NEO_HOME` is the single source of truth
/// for config, skills, prompts, themes, sessions — so each test isolates it.
/// `NEO_HOME` IS the neo root (equivalent to ~/.neo), so config lives at
/// `<NEO_HOME>/config.toml`, prompts at `<NEO_HOME>/prompts`, etc.
pub(crate) fn neo_home_for_test() -> PathBuf {
    thread_local! {
        static HOME: std::cell::OnceCell<PathBuf> = const { std::cell::OnceCell::new() };
    }
    HOME.with(|cell| {
        cell.get_or_init(|| {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time after epoch")
                .as_nanos();
            std::env::temp_dir().join(format!("neo-cli-home-{nanos}-{id}"))
        })
        .clone()
    })
}

/// Write the config.toml content into the test's isolated `NEO_HOME`.
pub(crate) fn write_home_config(content: &str) {
    let config_path = neo_home_for_test().join("config.toml");
    fs::create_dir_all(config_path.parent().expect("config parent")).expect("create neo home");
    fs::write(&config_path, content).expect("write home config");
}

pub(crate) fn index_session(session_id: &str, session_dir: &Path, workdir: &Path) {
    let index = neo_agent_core::session::SessionIndex::new(&neo_home_for_test());
    index
        .append(&neo_agent_core::session::SessionIndexEntry {
            session_id: session_id.to_owned(),
            session_dir: session_dir.to_path_buf(),
            workdir: workdir.to_path_buf(),
        })
        .expect("index session");
}

pub(crate) fn run(mut command: Command) -> String {
    let output = command.output().expect("neo command should run");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout should be utf8")
}

/// Find `.jsonl` files in the bucket directory that corresponds to the
/// given project directory. The bucket name is `wd_<slug>_<hash12>`.
pub(crate) fn find_jsonl_files_in_bucket(sessions_root: &Path, project_dir: &Path) -> Vec<PathBuf> {
    // Search all buckets that match the slug prefix and check which one has our
    // session. Since temp dirs have unique basenames, the slug is unique enough.
    let basename = project_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let slug: String = basename
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "workspace" } else { slug };

    let prefix = format!("wd_{slug}_");

    let Ok(entries) = fs::read_dir(sessions_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with(&prefix) {
                let mut results = Vec::new();
                find_jsonl_files_recursive(&path, &mut results);
                return results;
            }
        }
    }
    Vec::new()
}

pub(crate) fn find_jsonl_files_recursive(dir: &Path, results: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            find_jsonl_files_recursive(&path, results);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
            results.push(path);
        }
    }
}

pub(crate) fn sessions_metadata_json(entries: &[(&str, Value)]) -> String {
    let mut sessions = serde_json::Map::new();
    for (id, value) in entries {
        sessions.insert((*id).to_owned(), value.clone());
    }
    json!({ "sessions": sessions }).to_string()
}

pub(crate) fn session_bucket(project_dir: &Path) -> PathBuf {
    let sessions_root = neo_home_for_test().join("sessions");
    neo_agent_core::session::workspace_sessions_dir(&sessions_root, project_dir)
}

pub(crate) fn write_session_transcript(sessions: &Path, session_id: &str, content: &str) {
    let session_dir = sessions.join(session_id);
    let wire = neo_agent_core::session::main_agent_wire_path(&session_dir);
    fs::create_dir_all(wire.parent().expect("wire parent")).expect("create wire dir");
    fs::write(&wire, content).expect("write main wire");
    fs::write(
        neo_agent_core::session::session_state_path(&session_dir),
        "{\"schema_version\":1,\"agents\":{\"main\":{\"kind\":\"main\",\"record_dir\":\"agents/main\"}}}\n",
    )
    .expect("write session state");
}

pub(crate) fn canonical_project_dir(temp: &TempDir) -> PathBuf {
    temp.path().canonicalize().expect("canonicalize temp dir")
}

/// Bind an ephemeral TCP port and accept one connection, immediately closing the
/// socket so the remote HTTP probe fails deterministically without relying on
/// `127.0.0.1:1` behavior.
pub(crate) fn failure_server_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind failure server");
    let url = format!("http://{}/rpc", listener.local_addr().expect("local addr"));
    std::thread::spawn(move || {
        if let Ok((socket, _)) = listener.accept() {
            drop(socket);
        }
    });
    url
}

#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: BTreeMap<String, String>,
    pub(crate) body: Value,
}

pub(crate) struct MockSseServer {
    pub(crate) url: String,
    pub(crate) requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockSseServer {
    pub(crate) fn start(responses: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock provider");
        let url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured_requests = Arc::clone(&requests);

        std::thread::spawn(move || {
            for response in responses {
                let (mut socket, _) = listener.accept().expect("accept provider request");
                let request = read_http_request(&mut socket);
                captured_requests
                    .lock()
                    .expect("lock requests")
                    .push(request);
                socket
                    .write_all(response.as_bytes())
                    .expect("write provider response");
            }
        });

        Self { url, requests }
    }

    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("lock requests").clone()
    }
}

pub(crate) fn openai_response_sse(id: &str, text: &str) -> String {
    sse_response(&[
        json!({ "type": "response.created", "response": { "id": id } }),
        json!({ "type": "response.output_text.delta", "delta": text }),
        json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "usage": { "input_tokens": 7, "output_tokens": 3 }
            }
        }),
    ])
}

pub(crate) fn mock_responses_config(base_url: &str) -> String {
    format!(
        r#"
default_provider = "mock"
default_model = "gpt-4.1"

[providers.mock]
type = "openai_response"
base_url = "{base_url}"
api_key_env = "OPENAI_API_KEY"

[models."mock/gpt-4.1"]
provider = "mock"
model = "gpt-4.1"
capabilities = ["streaming", "tools"]
"#
    )
}

pub(crate) fn mcp_json_response(id: u64, result: &Value) -> String {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
    .to_string();
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// HTTP 202 response consumed by rmcp for the `notifications/initialized` POST.
/// rmcp's `expect_accepted_or_json` accepts either a 202 or any JSON body.
pub(crate) fn mcp_http_accept() -> String {
    "HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
}

pub(crate) fn sse_response(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        write!(&mut body, "data: {event}\n\n").expect("write SSE event");
    }
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

pub(crate) fn read_http_request(socket: &mut TcpStream) -> RecordedRequest {
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

pub(crate) fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

pub(crate) const MCP_STDIO_FIXTURE: &str = r#"
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request["method"]
    if method == "initialize":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "protocolVersion": "2024-11-05",
                "serverInfo": {"name": "fixture", "version": "0.1.0"},
                "capabilities": {"tools": {}},
            },
        }
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "tools": [
                    {
                        "name": "docs-search",
                        "description": "Search project docs",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"query": {"type": "string"}},
                            "required": ["query"],
                        },
                    }
                ]
            },
        }
    elif method == "tools/call":
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [{"type": "text", "text": "ok"}],
                "isError": False,
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "error": {"code": -32601, "message": f"unknown method {method}"},
        }
    print(json.dumps(response), flush=True)
"#;

// ---------------------------------------------------------------------------
// Task 19: headless workflow command family
// ---------------------------------------------------------------------------

pub(crate) fn workflow_pair_bytes(
    name: &str,
    display: &str,
    description: &str,
    script: &str,
) -> (String, String) {
    use sha2::{Digest, Sha256};
    let source_sha = format!("{:x}", Sha256::digest(script.as_bytes()));
    let toml = format!(
        r#"
name = "{name}"
display_name = "{display}"
description = "{description}"
source_sha256 = "{source_sha}"

[[phases]]
id = "run"
description = "execute"

[output_schema]
type = "object"
additionalProperties = false
required = ["ok"]

[output_schema.properties.ok]
type = "boolean"
"#
    );
    (toml, script.to_owned())
}

pub(crate) fn write_user_workflow(name: &str, display: &str, description: &str, script: &str) {
    let home = neo_home_for_test();
    let dir = home.join("workflows");
    fs::create_dir_all(&dir).expect("workflows dir");
    let (toml, lua) = workflow_pair_bytes(name, display, description, script);
    fs::write(dir.join(format!("{name}.lua")), lua).expect("write lua");
    fs::write(dir.join(format!("{name}.workflow.toml")), toml).expect("write toml");
}

pub(crate) fn run_workflow_args(temp: &TempDir, args: &[&str]) -> String {
    let mut command = neo();
    command.current_dir(temp.path()).args(args);
    run(command)
}
