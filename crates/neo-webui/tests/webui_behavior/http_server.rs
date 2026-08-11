//! Shared fixture for `webui_behavior`: an in-memory fake `WebUiHost`, a
//! loopback test server on a random port, a raw HTTP/1.1 client (exact
//! Host/Origin/Cookie control), and a WebSocket client.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use neo_agent_core::session::ToolOutputRange;
use neo_agent_core::{AgentEvent, TodoEventData};
use neo_webui::Relay;
use neo_webui::protocol::{
    WebUiBootstrap, WebUiChangeStatus, WebUiCommand, WebUiCompletionItem, WebUiCompletions,
    WebUiCursor, WebUiError, WebUiErrorCode, WebUiEventBody, WebUiHistoryEntry, WebUiHost,
    WebUiModelInfo, WebUiPendingApproval, WebUiPendingQuestion, WebUiPhase, WebUiReply,
    WebUiSessionMetadata, WebUiSessionPage, WebUiSessionScope, WebUiSessionState,
    WebUiSessionSummary, WebUiSnapshot, WebUiSummaryState, WebUiWorkspaceChange,
    WebUiWorkspaceChangeDetail, WebUiWorkspaceChanges, WebUiWorkspaceGroup, WebUiWorkspaceSnapshot,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use futures::SinkExt;

// ── Fake host ────────────────────────────────────────────────────────────

struct FakeSession {
    /// Every published event with its relay sequence.
    #[allow(dead_code)]
    raw: Vec<(u64, AgentEvent)>,
    /// Projection after retraction (failed attempts removed).
    retained: Vec<(u64, AgentEvent)>,
    /// `retained.len()` at `RetryScheduled`; truncation point on resume.
    retry_boundary: Option<usize>,
    state: WebUiSessionState,
    metadata: WebUiSessionMetadata,
    pending_approval: Option<WebUiPendingApproval>,
    pending_questions: Vec<WebUiPendingQuestion>,
    todos: Vec<TodoEventData>,
}

impl Default for FakeSession {
    fn default() -> Self {
        Self {
            raw: Vec::new(),
            retained: Vec::new(),
            retry_boundary: None,
            state: WebUiSessionState {
                phase: WebUiPhase::Starting,
                waiting_approval: false,
                waiting_question: false,
                current_turn_id: Some("turn_1".to_string()),
                token_usage: None,
                context_window: None,
            },
            metadata: WebUiSessionMetadata {
                title: None,
                pinned: false,
                archived: false,
                updated_at: None,
            },
            pending_approval: None,
            pending_questions: Vec::new(),
            todos: Vec::new(),
        }
    }
}

/// In-memory `WebUiHost` driven by explicit publishes.
pub struct FakeHost {
    relay: Arc<Relay>,
    sessions: Mutex<HashMap<String, FakeSession>>,
}

impl FakeHost {
    #[must_use]
    pub fn new(relay: Arc<Relay>) -> Self {
        Self {
            relay,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Register a session without publishing anything.
    pub fn create_with_id(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("fake host poisoned")
            .entry(session_id.to_string())
            .or_default();
    }

    /// Publish one canonical event through the relay and update the fake
    /// projection (approvals, questions, todos, retraction, turn state).
    pub fn publish(&self, session_id: &str, event: AgentEvent) -> u64 {
        let sequence = self
            .relay
            .publisher(session_id)
            .publish(WebUiEventBody::SessionEvent {
                event: event.clone(),
                output: None,
            });
        let mut sessions = self.sessions.lock().expect("fake host poisoned");
        let session = sessions.entry(session_id.to_string()).or_default();
        session.raw.push((sequence, event.clone()));
        session.retained.push((sequence, event.clone()));
        match &event {
            AgentEvent::RetryScheduled { .. } => {
                session.retry_boundary = Some(session.retained.len());
            }
            AgentEvent::RetryResumed { .. } | AgentEvent::RetryExhausted { .. } => {
                if let Some(boundary) = session.retry_boundary.take() {
                    session.retained.truncate(boundary);
                }
            }
            AgentEvent::ApprovalRequested { request } => {
                session.pending_approval = Some(WebUiPendingApproval {
                    request_id: request.id.clone(),
                    turn_id: request.turn.to_string(),
                    presentation: request.presentation.clone(),
                    options: request.options.clone(),
                });
                session.state.waiting_approval = true;
            }
            AgentEvent::QuestionRequested { id, questions, .. } => {
                session.pending_questions.push(WebUiPendingQuestion {
                    id: id.clone(),
                    turn_id: "turn_1".to_string(),
                    questions: questions.clone(),
                });
                session.state.waiting_question = true;
            }
            AgentEvent::TodoUpdated { todos, .. } => session.todos.clone_from(todos),
            AgentEvent::TurnStarted { .. } => {
                session.state.phase = WebUiPhase::Running;
                session.state.current_turn_id = Some("turn_1".to_string());
            }
            AgentEvent::TurnFinished { .. } | AgentEvent::RunFinished { .. } => {
                session.state.phase = WebUiPhase::Idle;
                session.state.current_turn_id = None;
            }
            _ => {}
        }
        sequence
    }

    /// Publish the small workspace summary for one session, exactly like the
    /// real host does on every state or metadata change. Summaries never
    /// carry an `AgentEvent`.
    pub fn publish_summary(&self, session_id: &str) -> u64 {
        let sessions = self.sessions.lock().expect("fake host poisoned");
        let session = sessions.get(session_id).expect("session exists");
        self.relay
            .publish_summary(Self::summary_of(session_id, session))
    }

    fn summary_of(session_id: &str, session: &FakeSession) -> WebUiSessionSummary {
        WebUiSessionSummary {
            session_id: session_id.to_string(),
            title: session.metadata.title.clone(),
            updated_at: session.metadata.updated_at.clone(),
            pinned: session.metadata.pinned,
            archived: session.metadata.archived,
            state: match session.state.phase {
                WebUiPhase::Idle => WebUiSummaryState::Idle,
                WebUiPhase::Failed => WebUiSummaryState::Failed,
                _ if session.state.waiting_approval => WebUiSummaryState::WaitingApproval,
                _ if session.state.waiting_question => WebUiSummaryState::WaitingQuestion,
                _ => WebUiSummaryState::Running,
            },
            workspace_label: "sample-workspace".to_string(),
        }
    }
    fn snapshot_locked(
        session_id: &str,
        session: &FakeSession,
    ) -> Result<WebUiSnapshot, WebUiError> {
        Ok(WebUiSnapshot {
            stream_id: String::new(),
            session_id: session_id.to_string(),
            watermark: session.retained.last().map_or(0, |(sequence, _)| *sequence),
            session: session.state.clone(),
            metadata: session.metadata.clone(),
            history: session
                .retained
                .iter()
                .map(|(sequence, event)| WebUiHistoryEntry {
                    sequence: *sequence,
                    event: event.clone(),
                    output: None,
                })
                .collect(),
            pending_approval: session.pending_approval.clone(),
            pending_questions: session.pending_questions.clone(),
            todos: session.todos.clone(),
        })
    }
}

#[async_trait]
impl WebUiHost for FakeHost {
    async fn execute(&self, command: WebUiCommand) -> Result<WebUiReply, WebUiError> {
        match command {
            WebUiCommand::Bootstrap => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                let mut summaries = Vec::new();
                for (session_id, session) in sessions.iter() {
                    summaries.push(WebUiSessionSummary {
                        session_id: session_id.clone(),
                        title: session.metadata.title.clone(),
                        updated_at: session.metadata.updated_at.clone(),
                        pinned: session.metadata.pinned,
                        archived: session.metadata.archived,
                        state: match session.state.phase {
                            WebUiPhase::Idle => WebUiSummaryState::Idle,
                            WebUiPhase::Failed => WebUiSummaryState::Failed,
                            _ if session.state.waiting_approval => {
                                WebUiSummaryState::WaitingApproval
                            }
                            _ if session.state.waiting_question => {
                                WebUiSummaryState::WaitingQuestion
                            }
                            _ => WebUiSummaryState::Running,
                        },
                        workspace_label: "sample-workspace".to_string(),
                    });
                }
                Ok(WebUiReply::Bootstrap(WebUiBootstrap {
                    workspace_label: Some("sample-workspace".to_string()),
                    default_model: "fake-model".to_string(),
                    default_reasoning: neo_ai::ReasoningSelection::Off,
                    models: vec![WebUiModelInfo {
                        alias: "fake-model".to_string(),
                        provider: "fake-provider".to_string(),
                        display_name: None,
                        context_window: Some(128_000),
                        capabilities: vec!["streaming".to_string()],
                        reasoning: neo_ai::ReasoningCapability::None,
                    }],
                    permission_modes: vec![
                        neo_agent_core::PermissionMode::Ask,
                        neo_agent_core::PermissionMode::Auto,
                    ],
                    development_modes: vec![
                        neo_webui::protocol::WebUiDevelopmentMode::Normal,
                        neo_webui::protocol::WebUiDevelopmentMode::Plan,
                    ],
                    sessions: summaries,
                }))
            }
            WebUiCommand::CompleteInput { query } => {
                Ok(WebUiReply::Completions(WebUiCompletions {
                    items: vec![WebUiCompletionItem {
                        value: query.clone(),
                        label: query,
                        description: Some("fixture completion".to_string()),
                    }],
                }))
            }
            WebUiCommand::ListSessions {
                scope,
                query,
                cursor,
                limit,
            } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                let mut items: Vec<WebUiSessionSummary> = sessions
                    .iter()
                    .map(|(session_id, session)| WebUiSessionSummary {
                        session_id: session_id.clone(),
                        title: session.metadata.title.clone(),
                        updated_at: session.metadata.updated_at.clone(),
                        pinned: session.metadata.pinned,
                        archived: session.metadata.archived,
                        state: WebUiSummaryState::Idle,
                        workspace_label: "sample-workspace".to_string(),
                    })
                    .collect();
                match scope {
                    WebUiSessionScope::Active => items.retain(|item| !item.archived),
                    WebUiSessionScope::Archived => items.retain(|item| item.archived),
                }
                if let Some(query) = &query {
                    let query = query.to_lowercase();
                    items.retain(|item| {
                        item.title
                            .as_deref()
                            .is_some_and(|title| title.to_lowercase().contains(&query))
                    });
                }
                if let Some(cursor) = &cursor
                    && let Some(position) = items.iter().position(|item| &item.session_id == cursor)
                {
                    items = items.split_off(position + 1);
                }
                let limit = usize::try_from(limit.unwrap_or(100))
                    .unwrap_or(100)
                    .min(100);
                let has_more = items.len() > limit;
                items.truncate(limit);
                let next_cursor =
                    has_more.then(|| items.last().expect("non-empty").session_id.clone());
                Ok(WebUiReply::Sessions(WebUiSessionPage {
                    items,
                    next_cursor,
                }))
            }
            WebUiCommand::Snapshot { session_id } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                let Some(session) = sessions.get(&session_id) else {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                };
                Ok(WebUiReply::Snapshot(Self::snapshot_locked(
                    &session_id,
                    session,
                )?))
            }
            WebUiCommand::CreateSession { .. } => {
                let mut sessions = self.sessions.lock().expect("fake host poisoned");
                let session_id = format!("session_{:04}", sessions.len() + 1);
                sessions.insert(session_id.clone(), FakeSession::default());
                let state = WebUiSessionState {
                    phase: WebUiPhase::Starting,
                    waiting_approval: false,
                    waiting_question: false,
                    current_turn_id: Some("turn_1".to_string()),
                    token_usage: None,
                    context_window: None,
                };
                Ok(WebUiReply::SessionCreated {
                    session_id,
                    turn_id: "turn_1".to_string(),
                    state,
                })
            }
            WebUiCommand::StartTurn {
                session_id,
                message,
                ..
            } => {
                if message.trim().is_empty() {
                    return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
                }
                let mut sessions = self.sessions.lock().expect("fake host poisoned");
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                };
                session.state.phase = WebUiPhase::Starting;
                session.state.current_turn_id = Some("turn_1".to_string());
                Ok(WebUiReply::TurnStarted {
                    session_id,
                    turn_id: "turn_1".to_string(),
                    state: session.state.clone(),
                })
            }
            WebUiCommand::SendInput {
                session_id,
                turn_id,
                ..
            } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                if !sessions.contains_key(&session_id) {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                }
                Ok(WebUiReply::InputAccepted { turn_id })
            }
            WebUiCommand::CancelTurn {
                session_id,
                turn_id,
            } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                if !sessions.contains_key(&session_id) {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                }
                Ok(WebUiReply::Cancelling { turn_id })
            }
            WebUiCommand::ResolveApproval {
                session_id,
                turn_id,
                request_id,
                ..
            } => {
                let mut sessions = self.sessions.lock().expect("fake host poisoned");
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                };
                let Some(pending) = &session.pending_approval else {
                    return Err(WebUiError::new(WebUiErrorCode::StaleControl));
                };
                if pending.request_id != request_id || pending.turn_id != turn_id {
                    return Err(WebUiError::new(WebUiErrorCode::StaleControl));
                }
                session.pending_approval = None;
                session.state.waiting_approval = false;
                Ok(WebUiReply::Resolved)
            }
            WebUiCommand::ResolveQuestion {
                session_id,
                turn_id,
                question_id,
                ..
            } => {
                let mut sessions = self.sessions.lock().expect("fake host poisoned");
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                };
                let Some(index) = session
                    .pending_questions
                    .iter()
                    .position(|pending| pending.id == question_id && pending.turn_id == turn_id)
                else {
                    return Err(WebUiError::new(WebUiErrorCode::StaleControl));
                };
                session.pending_questions.remove(index);
                session.state.waiting_question = !session.pending_questions.is_empty();
                Ok(WebUiReply::Resolved)
            }
            WebUiCommand::UpdateMetadata {
                session_id,
                title,
                pinned,
                archived,
            } => {
                let mut sessions = self.sessions.lock().expect("fake host poisoned");
                let Some(session) = sessions.get_mut(&session_id) else {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                };
                if let Some(title) = title {
                    session.metadata.title = Some(title);
                }
                if let Some(pinned) = pinned {
                    session.metadata.pinned = pinned;
                }
                if let Some(archived) = archived {
                    session.metadata.archived = archived;
                }
                Ok(WebUiReply::MetadataUpdated(session.metadata.clone()))
            }
            WebUiCommand::ReadToolOutput {
                session_id,
                start_line,
                max_lines,
                ..
            } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                if !sessions.contains_key(&session_id) {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                }
                Ok(WebUiReply::ToolOutput(ToolOutputRange {
                    text: "fake output line\n".to_string(),
                    start_line,
                    next_line: start_line + u64::from(max_lines),
                    reached_end: true,
                }))
            }
            WebUiCommand::WorkspaceChanges => {
                Ok(WebUiReply::WorkspaceChanges(WebUiWorkspaceChanges {
                    branch: Some("fake-branch".to_string()),
                    dirty: true,
                    changes: vec![WebUiWorkspaceChange {
                        change_id: "fake-change".to_string(),
                        path: "src/fake.rs".to_string(),
                        status: WebUiChangeStatus::Modified,
                        added: 1,
                        deleted: 0,
                    }],
                }))
            }
            WebUiCommand::WorkspaceChangeDetail { change_id } => {
                if change_id != "fake-change" {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                }
                Ok(WebUiReply::WorkspaceChangeDetail(
                    WebUiWorkspaceChangeDetail {
                        change_id,
                        path: "src/fake.rs".to_string(),
                        status: WebUiChangeStatus::Modified,
                        diff: "@@ fake @@\n+fake\n".to_string(),
                        truncated: false,
                    },
                ))
            }
            WebUiCommand::UploadAttachment { mime, base64 } => {
                use base64::Engine as _;
                if !mime.starts_with("image/") {
                    return Err(WebUiError::new(WebUiErrorCode::InvalidRequest));
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(base64.as_bytes())
                    .map_err(|_| WebUiError::new(WebUiErrorCode::InvalidRequest))?;
                Ok(WebUiReply::AttachmentUploaded(
                    neo_webui::protocol::WebUiAttachmentAck {
                        id: "fake-attachment".to_string(),
                        mime,
                        byte_len: bytes.len() as u64,
                    },
                ))
            }
            WebUiCommand::AgentHistory {
                session_id,
                agent_id,
            } => {
                let sessions = self.sessions.lock().expect("fake host poisoned");
                if !sessions.contains_key(&session_id) {
                    return Err(WebUiError::new(WebUiErrorCode::NotFound));
                }
                Ok(WebUiReply::AgentHistory(
                    neo_webui::protocol::WebUiAgentHistory {
                        agent_id,
                        watermark: 0,
                        history: Vec::new(),
                    },
                ))
            }
        }
    }

    async fn session_exists(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("fake host poisoned")
            .contains_key(session_id)
    }

    async fn workspace_snapshot(&self) -> Result<WebUiWorkspaceSnapshot, WebUiError> {
        let sessions = self.sessions.lock().expect("fake host poisoned");
        let mut summaries: Vec<WebUiSessionSummary> = sessions
            .iter()
            .map(|(session_id, session)| Self::summary_of(session_id, session))
            .collect();
        summaries.sort_by(|left, right| left.session_id.cmp(&right.session_id));
        Ok(WebUiWorkspaceSnapshot {
            stream_id: String::new(),
            workspace_sequence: 0,
            workspaces: vec![WebUiWorkspaceGroup {
                label: "sample-workspace".to_string(),
                current: true,
                sessions: summaries,
            }],
        })
    }

    async fn subscribe(&self, session_id: &str) -> Result<WebUiSnapshot, WebUiError> {
        let sessions = self.sessions.lock().expect("fake host poisoned");
        let Some(session) = sessions.get(session_id) else {
            return Err(WebUiError::new(WebUiErrorCode::NotFound));
        };
        Self::snapshot_locked(session_id, session)
    }
}

// ── Loopback test server ─────────────────────────────────────────────────

pub struct TestServer {
    pub addr: SocketAddr,
    pub token: String,
    pub host: Arc<FakeHost>,
    pub relay: Arc<Relay>,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    pub async fn start() -> Self {
        let relay = Arc::new(Relay::new(format!(
            "test_stream_{}",
            uuid::Uuid::new_v4().simple()
        )));
        let host = Arc::new(FakeHost::new(relay.clone()));
        let running = neo_webui::start(host.clone(), relay.clone())
            .await
            .expect("loopback bind succeeds");
        let addr = running.local_addr;
        let token = running.access_token.clone();
        let task = tokio::spawn(async move {
            let _ = running.run().await;
        });
        Self {
            addr,
            token,
            host,
            relay,
            task,
        }
    }

    #[must_use]
    pub fn origin(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Claim the one-time token and return the full `Set-Cookie` value.
    pub async fn claim(&self) -> String {
        let body = format!(r#"{{"token":"{}"}}"#, self.token).into_bytes();
        let response = raw_request(
            self.addr,
            RawRequest {
                method: "POST".to_string(),
                path: "/api/auth/claim".to_string(),
                origin: Some(self.origin()),
                content_type: Some("application/json".to_string()),
                body,
                ..RawRequest::default()
            },
        )
        .await;
        assert_eq!(
            response.status,
            204,
            "claim failed: {}",
            response.body_str()
        );
        response
            .set_cookie()
            .expect("successful claim sets a cookie")
            .to_string()
    }

    /// Claim and return the `Cookie` header value to send on later requests.
    pub async fn claim_cookie(&self) -> String {
        cookie_pair(&self.claim().await)
    }
}

/// Extract `neo_webui_session=<value>` from a full `Set-Cookie` value.
#[must_use]
pub fn cookie_pair(set_cookie: &str) -> String {
    let name_value = set_cookie
        .split(';')
        .next()
        .expect("first cookie attribute is name=value");
    assert!(
        name_value.starts_with("neo_webui_session="),
        "unexpected cookie: {set_cookie}"
    );
    name_value.to_string()
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ── Raw HTTP/1.1 client (exact header control) ───────────────────────────

#[derive(Clone)]
pub struct RawRequest {
    pub method: String,
    pub path: String,
    /// Exact `Host` value; `None` writes the correct `127.0.0.1:<port>`.
    pub host: Option<String>,
    /// Omit the `Host` header entirely (protocol-level rejection).
    pub omit_host: bool,
    pub origin: Option<String>,
    pub cookie: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
    pub upgrade: bool,
}

impl Default for RawRequest {
    fn default() -> Self {
        Self {
            method: "GET".to_string(),
            path: "/".to_string(),
            host: None,
            omit_host: false,
            origin: None,
            cookie: None,
            content_type: None,
            body: Vec::new(),
            extra_headers: Vec::new(),
            upgrade: false,
        }
    }
}

pub struct RawResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RawResponse {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[must_use]
    pub fn set_cookie(&self) -> Option<&str> {
        self.header("set-cookie")
    }

    #[must_use]
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    #[must_use]
    pub fn error_code(&self) -> String {
        self.json()["code"].as_str().unwrap_or_default().to_string()
    }
}

pub async fn raw_request(addr: SocketAddr, request: RawRequest) -> RawResponse {
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect to loopback server");
    let host = request.host.unwrap_or_else(|| addr.to_string());
    let mut head = format!(
        "{} {} HTTP/1.1\r\nContent-Length: {}\r\n",
        request.method,
        request.path,
        request.body.len()
    );
    if !request.omit_host {
        head.push_str(&format!("Host: {host}\r\n"));
    }
    if !request.upgrade {
        head.push_str("Connection: close\r\n");
    }
    if let Some(origin) = &request.origin {
        head.push_str(&format!("Origin: {origin}\r\n"));
    }
    if let Some(cookie) = &request.cookie {
        head.push_str(&format!("Cookie: {cookie}\r\n"));
    }
    if let Some(content_type) = &request.content_type {
        head.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    for (name, value) in &request.extra_headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if request.upgrade {
        head.push_str(
            "Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n",
        );
    }
    head.push_str("\r\n");
    stream
        .write_all(head.as_bytes())
        .await
        .expect("write request head");
    if !request.body.is_empty() {
        stream
            .write_all(&request.body)
            .await
            .expect("write request body");
    }
    read_response(&mut stream).await
}

async fn read_response(stream: &mut TcpStream) -> RawResponse {
    let deadline = Duration::from_secs(10);
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        let count = tokio::time::timeout(deadline, stream.read(&mut chunk))
            .await
            .expect("read response head")
            .expect("read response head");
        if count == 0 {
            break None;
        }
        buffer.extend_from_slice(&chunk[..count]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break Some(position);
        }
    };
    let Some(header_end) = header_end else {
        return RawResponse {
            status: 0,
            headers: Vec::new(),
            body: buffer,
        };
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    let mut content_length = None;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                content_length = value.parse::<usize>().ok();
            }
            headers.push((name, value));
        }
    }
    let mut body = buffer[header_end + 4..].to_vec();
    if let Some(length) = content_length {
        while body.len() < length {
            let count = tokio::time::timeout(deadline, stream.read(&mut chunk))
                .await
                .expect("read response body")
                .expect("read response body");
            if count == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..count]);
        }
        body.truncate(length);
    } else {
        loop {
            let count = tokio::time::timeout(deadline, stream.read(&mut chunk))
                .await
                .expect("read response body")
                .expect("read response body");
            if count == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..count]);
        }
    }
    RawResponse {
        status,
        headers,
        body,
    }
}

// ── WebSocket client ─────────────────────────────────────────────────────

pub type TestWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub async fn ws_connect(
    addr: SocketAddr,
    cookie: &str,
    origin: &str,
) -> Result<TestWebSocket, Box<dyn std::error::Error>> {
    let url = format!("ws://{addr}/api/events");
    let request = http::Request::builder()
        .method("GET")
        .uri(&url)
        .header("Host", addr.to_string())
        .header("Origin", origin)
        .header("Cookie", cookie)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())?;
    let (socket, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(socket)
}

/// Send a `watch_session` frame.
pub async fn watch(ws: &mut TestWebSocket, session_id: &str, after: Option<WebUiCursor>) {
    let message = match after {
        None => format!(r#"{{"type":"watch_session","session_id":"{session_id}"}}"#),
        Some(cursor) => format!(
            r#"{{"type":"watch_session","session_id":"{session_id}","after":{{"stream_id":"{}","sequence":{}}}}}"#,
            cursor.stream_id, cursor.sequence
        ),
    };
    ws.send(Message::Text(message))
        .await
        .expect("send watch_session");
}

/// Send a `watch_workspace` frame.
pub async fn watch_workspace(ws: &mut TestWebSocket, after: Option<WebUiCursor>) {
    let message = match after {
        None => r#"{"type":"watch_workspace"}"#.to_string(),
        Some(cursor) => format!(
            r#"{{"type":"watch_workspace","after":{{"stream_id":"{}","sequence":{}}}}}"#,
            cursor.stream_id, cursor.sequence
        ),
    };
    ws.send(Message::Text(message))
        .await
        .expect("send watch_workspace");
}
