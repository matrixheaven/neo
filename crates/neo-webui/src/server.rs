//! Axum HTTP service: loopback listener, request guard (Host/Origin/Cookie +
//! security headers), stable error responses, command routes, the web long
//! connection with snapshot-plus-resume, and the compile-time embedded static
//! assets (exact-path allowlist, anonymous GET, no SPA fallback).

use std::collections::HashMap;
use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::{Body, to_bytes};
use axum::extract::ws::{CloseCode, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, Request, State};
use axum::http::{Method, StatusCode, Uri, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use http::HeaderMap;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::AuthState;
use crate::auth::{
    apply_security_headers, clear_cookie_header, decode_urlsafe_32, host_matches, origin_matches,
    session_cookie_header, session_cookie_value,
};
use crate::protocol::{
    WebUiAddWorkspaceBody, WebUiApprovalBody, WebUiAttachmentBody, WebUiCancelBody,
    WebUiCancelling, WebUiClaimRequest, WebUiCommand, WebUiCreateSessionBody, WebUiError,
    WebUiErrorBody, WebUiErrorCode, WebUiHost, WebUiInputAccepted, WebUiInputBody,
    WebUiMetadataBody, WebUiQuestionBody, WebUiReply, WebUiServerMessage, WebUiSessionScope,
    WebUiSessionStarted, WebUiStartTurnBody, WebUiUpdateWorkspaceBody, WebUiWatchRequest,
};
use crate::relay::{
    ATTACHMENT_BODY_LIMIT_BYTES, COMMAND_BODY_LIMIT_BYTES, FIRST_SUBSCRIBE_DEADLINE, ObserverQueue,
    OutboundMessage, Relay, SESSION_PAGE_LIMIT, SubscribeMode, SubscriptionLayer,
    TOOL_OUTPUT_MAX_LINES, WS_FRAME_LIMIT_BYTES,
};

/// Shared service state (all fields cheaply cloneable for handlers).
#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthState>,
    pub host: Arc<dyn WebUiHost>,
    pub relay: Arc<Relay>,
    pub port: u16,
    pub next_conn_id: Arc<AtomicU64>,
}

/// A running loopback service. The access token is held in memory only and
/// is meant for the startup address fragment, never for logs.
pub struct RunningWebUi {
    pub local_addr: std::net::SocketAddr,
    pub access_token: String,
    server: Pin<Box<dyn Future<Output = Result<(), std::io::Error>> + Send>>,
}

impl RunningWebUi {
    /// Full startup address: `http://127.0.0.1:<port>/#access=<token>`.
    #[must_use]
    pub fn access_url(&self) -> String {
        format!("http://{}/#access={}", self.local_addr, self.access_token)
    }

    /// Serve until the listener fails or the process exits.
    pub async fn run(self) -> Result<(), std::io::Error> {
        self.server.await
    }
}

/// Bind `127.0.0.1:0` (system-assigned port), mint the one-time token and
/// build the fully guarded router. The web service identity (`stream_id`)
/// belongs to the relay passed in by the caller.
pub async fn start(
    host: Arc<dyn WebUiHost>,
    relay: Arc<Relay>,
) -> Result<RunningWebUi, std::io::Error> {
    let listener =
        tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let local_addr = listener.local_addr()?;
    let auth = Arc::new(AuthState::new());
    let access_token = auth.access_token().unwrap_or_default();
    let app = build_router(AppState {
        auth,
        host,
        relay,
        port: local_addr.port(),
        next_conn_id: Arc::new(AtomicU64::new(1)),
    });
    let server = Box::pin(axum::serve(listener, app).into_future());
    Ok(RunningWebUi {
        local_addr,
        access_token,
        server,
    })
}

fn build_router(app: AppState) -> Router {
    Router::new()
        .route("/api/auth/claim", post(claim))
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/completions", get(completions))
        .route("/api/attachments", post(upload_attachment))
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route("/api/workspaces", post(add_workspace))
        .route(
            "/api/workspaces/{workspace_id}/reveal",
            post(reveal_workspace),
        )
        .route("/api/workspaces/{workspace_id}", patch(update_workspace))
        .route("/api/sessions/{session_id}/snapshot", get(snapshot))
        .route(
            "/api/sessions/{session_id}/agents/{agent_id}/history",
            get(agent_history),
        )
        .route("/api/sessions/{session_id}/turns", post(start_turn))
        .route("/api/sessions/{session_id}/input", post(send_input))
        .route("/api/sessions/{session_id}/cancel", post(cancel_turn))
        .route(
            "/api/sessions/{session_id}/approval",
            post(resolve_approval),
        )
        .route(
            "/api/sessions/{session_id}/question",
            post(resolve_question),
        )
        .route("/api/sessions/{session_id}", patch(update_metadata))
        .route(
            "/api/sessions/{session_id}/tool-output/{output_ref}",
            get(read_tool_output),
        )
        .route("/api/workspace/changes", get(workspace_changes))
        .route(
            "/api/workspace/changes/{change_id}",
            get(workspace_change_detail),
        )
        .route("/api/events", get(events_ws))
        .fallback(fallback)
        .layer(middleware::from_fn_with_state(app.clone(), guard))
        .with_state(app)
}

// ── Request guard ────────────────────────────────────────────────────────

/// One middleware path for Host, Origin, Cookie, and security headers.
async fn guard(State(app): State<AppState>, request: Request, next: Next) -> Response {
    if !host_matches(request.headers(), app.port) {
        tracing::debug!("web request rejected: bad host");
        return finish(error_response(
            StatusCode::BAD_REQUEST,
            WebUiErrorCode::InvalidRequest,
        ));
    }
    let path = request.uri().path();
    let is_claim = path == "/api/auth/claim";
    let is_long_connection = path == "/api/events";
    let is_write = matches!(request.method(), &Method::POST | &Method::PATCH);

    if is_claim {
        // Anonymous, but origin-checked like every other write.
        if !origin_matches(request.headers(), app.port) {
            tracing::debug!("web request rejected: claim origin");
            return finish(error_response(
                StatusCode::BAD_REQUEST,
                WebUiErrorCode::InvalidRequest,
            ));
        }
        return finish(next.run(request).await);
    }
    if !path.starts_with("/api/") && request.method() == Method::GET {
        // Anonymous static-resource reads (embedded assets land here once
        // delivered); they never carry session data. Read requests do not
        // require an Origin.
        return finish(next.run(request).await);
    }

    // Authenticated surface.
    let cookie_ok = session_cookie_value(request.headers())
        .is_some_and(|value| app.auth.verify_credential(value));
    if !cookie_ok {
        tracing::debug!("web request rejected: bad cookie");
        let mut response = error_response(StatusCode::UNAUTHORIZED, WebUiErrorCode::Unauthorized);
        response
            .headers_mut()
            .insert(header::SET_COOKIE, clear_cookie_header());
        return finish(response);
    }
    if (is_write || is_long_connection) && !origin_matches(request.headers(), app.port) {
        tracing::debug!("web request rejected: origin");
        return finish(error_response(
            StatusCode::BAD_REQUEST,
            WebUiErrorCode::InvalidRequest,
        ));
    }
    finish(next.run(request).await)
}

fn finish(response: Response) -> Response {
    let mut response = response;
    apply_security_headers(&mut response);
    response
}

// ── Stable error helpers ─────────────────────────────────────────────────

/// Whitelisted query keys for `GET /api/sessions`.
const SESSION_LIST_QUERY_KEYS: &[&str] = &["scope", "query", "cursor", "limit"];
/// Whitelisted query keys for `GET /api/completions`.
const COMPLETION_QUERY_KEYS: &[&str] = &["query"];
const COMPLETION_QUERY_MAX_CHARS: usize = 256;
/// Whitelisted query keys for `GET .../tool-output/...`.
const TOOL_OUTPUT_QUERY_KEYS: &[&str] = &["start_line", "max_lines"];

fn error_response(status: StatusCode, code: WebUiErrorCode) -> Response {
    (status, Json(WebUiErrorBody { code })).into_response()
}

fn host_error_response(error: WebUiError) -> Response {
    let status =
        StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    error_response(status, error.code)
}

fn invalid_request() -> Response {
    error_response(StatusCode::BAD_REQUEST, WebUiErrorCode::InvalidRequest)
}

fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    (status, Json(value)).into_response()
}

async fn read_limited(body: Body) -> Result<axum::body::Bytes, Response> {
    read_limited_with(body, COMMAND_BODY_LIMIT_BYTES).await
}

async fn read_limited_with(body: Body, limit: usize) -> Result<axum::body::Bytes, Response> {
    match to_bytes(body, limit).await {
        Ok(bytes) => Ok(bytes),
        Err(_) => Err(error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            WebUiErrorCode::TooLarge,
        )),
    }
}

async fn parse_body<T: DeserializeOwned>(body: Body) -> Result<T, Response> {
    let bytes = read_limited(body).await?;
    serde_json::from_slice(&bytes).map_err(|_| invalid_request())
}

async fn fallback(method: Method, uri: Uri) -> Response {
    // Embedded static assets: exact-path allowlist for anonymous GET reads
    // (the guard already applied the host check and security headers apply
    // to this response too). No SPA fallback, no traversal resolution.
    if method == Method::GET
        && let Some(asset) = crate::assets::asset_for_path(uri.path())
    {
        return asset.into_response();
    }
    error_response(StatusCode::NOT_FOUND, WebUiErrorCode::NotFound)
}

fn reply_response(app: &AppState, reply: WebUiReply) -> Response {
    match reply {
        WebUiReply::Bootstrap(value) => json(StatusCode::OK, &value),
        WebUiReply::Completions(value) => json(StatusCode::OK, &value),
        WebUiReply::Sessions(value) => json(StatusCode::OK, &value),
        WebUiReply::Snapshot(mut value) => {
            value.stream_id = app.relay.stream_id().to_string();
            value.watermark = value
                .watermark
                .min(app.relay.current_sequence(&value.session_id));
            json(StatusCode::OK, &value)
        }
        WebUiReply::SessionCreated {
            session_id,
            turn_id,
            state,
        } => json(
            StatusCode::CREATED,
            &started_body(app, session_id, turn_id, state),
        ),
        WebUiReply::TurnStarted {
            session_id,
            turn_id,
            state,
        } => json(
            StatusCode::ACCEPTED,
            &started_body(app, session_id, turn_id, state),
        ),
        WebUiReply::InputAccepted { turn_id } => {
            json(StatusCode::ACCEPTED, &WebUiInputAccepted { turn_id })
        }
        WebUiReply::Cancelling { turn_id } => {
            json(StatusCode::ACCEPTED, &WebUiCancelling { turn_id })
        }
        WebUiReply::Resolved => StatusCode::NO_CONTENT.into_response(),
        WebUiReply::MetadataUpdated(value) => json(StatusCode::OK, &value),
        WebUiReply::ToolOutput(value) => json(StatusCode::OK, &value),
        WebUiReply::WorkspaceChanges(value) => json(StatusCode::OK, &value),
        WebUiReply::WorkspaceChangeDetail(value) => json(StatusCode::OK, &value),
        WebUiReply::WorkspaceAdded(value) => json(StatusCode::CREATED, &value),
        WebUiReply::AttachmentUploaded(value) => json(StatusCode::CREATED, &value),
        WebUiReply::AgentHistory(value) => json(StatusCode::OK, &value),
    }
}

fn started_body(
    app: &AppState,
    session_id: String,
    turn_id: String,
    state: crate::protocol::WebUiSessionState,
) -> WebUiSessionStarted {
    WebUiSessionStarted {
        stream_id: app.relay.stream_id().to_string(),
        sequence: app.relay.current_sequence(&session_id),
        session_id,
        turn_id,
        state,
    }
}

// ── Authentication ───────────────────────────────────────────────────────

/// `POST /api/auth/claim` — compare-and-consume the one-time token and issue
/// a strict in-memory session cookie. Never echoes the token.
async fn claim(State(app): State<AppState>, headers: HeaderMap, body: Body) -> Response {
    let mut content_types = headers.get_all(header::CONTENT_TYPE).iter();
    match (content_types.next(), content_types.next()) {
        (Some(value), None) if value.as_bytes() == b"application/json" => {}
        _ => return invalid_request(),
    }
    let parsed: WebUiClaimRequest = match parse_body(body).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    // Wrong length, invalid base64, wrong bytes, and consumed tokens all get
    // the same generic 401 without any `Set-Cookie`.
    let Some(token) = decode_urlsafe_32(&parsed.token) else {
        return error_response(StatusCode::UNAUTHORIZED, WebUiErrorCode::Unauthorized);
    };
    match app.auth.claim(token) {
        Some(credential) => {
            let mut response = StatusCode::NO_CONTENT.into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, session_cookie_header(&credential));
            response
        }
        None => error_response(StatusCode::UNAUTHORIZED, WebUiErrorCode::Unauthorized),
    }
}

// ── Read routes ──────────────────────────────────────────────────────────

async fn bootstrap(State(app): State<AppState>) -> Response {
    match app.host.execute(WebUiCommand::Bootstrap).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn completions(
    State(app): State<AppState>,
    params: Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let params = match params {
        Ok(Query(params)) => params,
        Err(_) => return invalid_request(),
    };
    if params
        .keys()
        .any(|key| !COMPLETION_QUERY_KEYS.contains(&key.as_str()))
    {
        return invalid_request();
    }
    let Some(query) = params.get("query") else {
        return invalid_request();
    };
    if query.chars().count() > COMPLETION_QUERY_MAX_CHARS
        || !matches!(query.chars().next(), Some('/' | '@'))
    {
        return invalid_request();
    }
    match app
        .host
        .execute(WebUiCommand::CompleteInput {
            query: query.clone(),
        })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn list_sessions(
    State(app): State<AppState>,
    params: Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let params = match params {
        Ok(Query(params)) => params,
        Err(_) => return invalid_request(),
    };
    // Query parameters are a fixed whitelist: misspelled keys are rejected
    // instead of silently ignored.
    if params
        .keys()
        .any(|key| !SESSION_LIST_QUERY_KEYS.contains(&key.as_str()))
    {
        return invalid_request();
    }
    let scope = match params.get("scope").map(String::as_str).unwrap_or("active") {
        "active" => WebUiSessionScope::Active,
        "archived" => WebUiSessionScope::Archived,
        _ => return invalid_request(),
    };
    let query = params
        .get("query")
        .filter(|value| !value.is_empty())
        .cloned();
    let cursor = params
        .get("cursor")
        .filter(|value| !value.is_empty())
        .cloned();
    let limit = match params.get("limit") {
        None => None,
        Some(raw) => match raw.parse::<u32>() {
            Ok(limit) => Some(limit.min(SESSION_PAGE_LIMIT as u32)),
            Err(_) => return invalid_request(),
        },
    };
    let command = WebUiCommand::ListSessions {
        scope,
        query,
        cursor,
        limit,
    };
    match app.host.execute(command).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn snapshot(State(app): State<AppState>, Path(session_id): Path<String>) -> Response {
    match app
        .host
        .execute(WebUiCommand::Snapshot { session_id })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

/// `GET /api/sessions/<id>/agents/<agent_id>/history` — lazy child-agent
/// history replay. Unknown sessions, unknown or cross-session agent ids and
/// malformed ids all get the same 404 `not_found`.
async fn agent_history(
    State(app): State<AppState>,
    Path((session_id, agent_id)): Path<(String, String)>,
) -> Response {
    match app
        .host
        .execute(WebUiCommand::AgentHistory {
            session_id,
            agent_id,
        })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

/// `POST /api/attachments` — one base64 media payload staged digest-addressed.
/// The body limit is the attachment-specific cap (base64 inflation of the
/// per-attachment decoded cap), not the small command limit.
async fn upload_attachment(State(app): State<AppState>, body: Body) -> Response {
    let bytes = match read_limited_with(body, ATTACHMENT_BODY_LIMIT_BYTES).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    let parsed: WebUiAttachmentBody = match serde_json::from_slice(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => return invalid_request(),
    };
    let command = WebUiCommand::UploadAttachment {
        mime: parsed.mime,
        base64: parsed.base64,
    };
    match app.host.execute(command).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn read_tool_output(
    State(app): State<AppState>,
    Path((session_id, output_ref)): Path<(String, String)>,
    params: Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let params = match params {
        Ok(Query(params)) => params,
        Err(_) => return invalid_request(),
    };
    if params
        .keys()
        .any(|key| !TOOL_OUTPUT_QUERY_KEYS.contains(&key.as_str()))
    {
        return invalid_request();
    }
    let start_line = match params.get("start_line") {
        None => 0,
        Some(raw) => match raw.parse::<u64>() {
            Ok(value) => value,
            Err(_) => return invalid_request(),
        },
    };
    let max_lines = match params.get("max_lines") {
        None => TOOL_OUTPUT_MAX_LINES,
        Some(raw) => match raw.parse::<u32>() {
            Ok(value) if (1..=TOOL_OUTPUT_MAX_LINES).contains(&value) => value,
            _ => return invalid_request(),
        },
    };
    let command = WebUiCommand::ReadToolOutput {
        session_id,
        output_ref,
        start_line,
        max_lines,
    };
    match app.host.execute(command).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

// ── Workspace change routes ──────────────────────────────────────────────

/// Reject any query parameter: the workspace change routes accept none, and
/// misspelled keys are rejected instead of silently ignored. Returns the
/// rejection response when the query is non-empty or malformed.
fn query_rejection(
    params: &Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Option<Response> {
    match params {
        Ok(Query(params)) if params.is_empty() => None,
        _ => Some(invalid_request()),
    }
}

/// `GET /api/workspace/changes` — branch label plus the structured change
/// summary, read on demand (no git polling).
async fn workspace_changes(
    State(app): State<AppState>,
    params: Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if let Some(response) = query_rejection(&params) {
        return response;
    }
    match app.host.execute(WebUiCommand::WorkspaceChanges).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

/// `GET /api/workspace/changes/<change_id>` — bounded unified-diff preview
/// for one opaque change reference. The reference is validated host-side;
/// forged, outside, absolute or stale references all get the same 404.
async fn workspace_change_detail(
    State(app): State<AppState>,
    Path(change_id): Path<String>,
    params: Result<Query<HashMap<String, String>>, axum::extract::rejection::QueryRejection>,
) -> Response {
    if let Some(response) = query_rejection(&params) {
        return response;
    }
    let command = WebUiCommand::WorkspaceChangeDetail { change_id };
    match app.host.execute(command).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

// ── Write routes ─────────────────────────────────────────────────────────

async fn create_session(State(app): State<AppState>, body: Body) -> Response {
    let parsed: WebUiCreateSessionBody = match parse_body(body).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let command = WebUiCommand::CreateSession {
        workspace_id: parsed.workspace_id,
        message: parsed.message,
        composer: parsed.composer,
        attachments: parsed.attachments,
    };
    match app.host.execute(command).await {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn add_workspace(State(app): State<AppState>, body: Body) -> Response {
    let parsed: WebUiAddWorkspaceBody = match parse_body(body).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    match app
        .host
        .execute(WebUiCommand::AddWorkspace { path: parsed.path })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn reveal_workspace(
    State(app): State<AppState>,
    Path(workspace_id): Path<String>,
) -> Response {
    match app
        .host
        .execute(WebUiCommand::RevealWorkspace { workspace_id })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

async fn update_workspace(
    State(app): State<AppState>,
    Path(workspace_id): Path<String>,
    body: Body,
) -> Response {
    let parsed: WebUiUpdateWorkspaceBody = match parse_body(body).await {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    match app
        .host
        .execute(WebUiCommand::UpdateWorkspace {
            workspace_id,
            label: parsed.label,
            pinned: parsed.pinned,
            removed: parsed.removed,
            mark_read: parsed.mark_read,
            read_session_id: parsed.read_session_id,
        })
        .await
    {
        Ok(reply) => reply_response(&app, reply),
        Err(error) => host_error_response(error),
    }
}

macro_rules! path_body_handler {
    ($handler:ident, $body:ty, $build:expr) => {
        async fn $handler(
            State(app): State<AppState>,
            Path(session_id): Path<String>,
            body: Body,
        ) -> Response {
            let parsed: $body = match parse_body(body).await {
                Ok(parsed) => parsed,
                Err(response) => return response,
            };
            let command = ($build)(session_id, parsed);
            match app.host.execute(command).await {
                Ok(reply) => reply_response(&app, reply),
                Err(error) => host_error_response(error),
            }
        }
    };
}

path_body_handler!(
    start_turn,
    WebUiStartTurnBody,
    |session_id, body: WebUiStartTurnBody| {
        WebUiCommand::StartTurn {
            session_id,
            message: body.message,
            composer: body.composer,
            attachments: body.attachments,
        }
    }
);
path_body_handler!(
    send_input,
    WebUiInputBody,
    |session_id, body: WebUiInputBody| {
        WebUiCommand::SendInput {
            session_id,
            turn_id: body.turn_id,
            delivery: body.delivery,
            message: body.message,
            attachments: body.attachments,
        }
    }
);
path_body_handler!(
    cancel_turn,
    WebUiCancelBody,
    |session_id, body: WebUiCancelBody| {
        WebUiCommand::CancelTurn {
            session_id,
            turn_id: body.turn_id,
        }
    }
);
path_body_handler!(
    resolve_approval,
    WebUiApprovalBody,
    |session_id, body: WebUiApprovalBody| {
        WebUiCommand::ResolveApproval {
            session_id,
            turn_id: body.turn_id,
            request_id: body.request_id,
            action: body.action,
            feedback: body.feedback,
        }
    }
);
path_body_handler!(
    resolve_question,
    WebUiQuestionBody,
    |session_id, body: WebUiQuestionBody| {
        WebUiCommand::ResolveQuestion {
            session_id,
            turn_id: body.turn_id,
            question_id: body.question_id,
            answer: body.answer,
        }
    }
);
path_body_handler!(
    update_metadata,
    WebUiMetadataBody,
    |session_id, body: WebUiMetadataBody| {
        WebUiCommand::UpdateMetadata {
            session_id,
            title: body.title,
            pinned: body.pinned,
            archived: body.archived,
        }
    }
);

// ── Web long connection ──────────────────────────────────────────────────

async fn events_ws(State(app): State<AppState>, ws: WebSocketUpgrade) -> Response {
    // Both the single-frame and the whole-message limits are the fixed
    // 64 KiB bound, so fragmented messages cannot bypass the frame limit.
    ws.max_frame_size(WS_FRAME_LIMIT_BYTES)
        .max_message_size(WS_FRAME_LIMIT_BYTES)
        .on_upgrade(move |socket| handle_events_socket(app, socket))
}

/// One task owns the socket: it drains the connection's bounded relay queue
/// and reads inbound frames. The first frame must be a `watch_workspace` or
/// `watch_session` within the admission deadline; afterwards there are no
/// timeouts. The connection holds one bounded queue shared by its workspace
/// summary subscription and one full-session subscription.
async fn handle_events_socket(app: AppState, mut socket: WebSocket) {
    let conn_id = app.next_conn_id.fetch_add(1, Ordering::Relaxed);
    tracing::debug!(conn_id, "web long connection opened");

    let mut next_frame = match tokio::time::timeout(FIRST_SUBSCRIBE_DEADLINE, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => Some(text.to_string()),
        _ => {
            tracing::debug!(conn_id, "closing empty web connection before subscribe");
            return;
        }
    };
    let mut queue: Option<Arc<ObserverQueue>> = None;

    loop {
        // Drain every sendable outbound message first. A close frame (the
        // `1013` slow-consumer close) is always sent before the connection
        // is torn down and the observer deregistered.
        if let Some(queue) = &queue {
            let mut close_sent = false;
            for message in queue.drain_sendable() {
                close_sent |= matches!(message, OutboundMessage::Close { .. });
                if !send_outbound(&mut socket, message).await {
                    app.relay.deregister(conn_id);
                    return;
                }
            }
            if close_sent || queue.is_closed() {
                app.relay.deregister(conn_id);
                return;
            }
        }

        // Handle the pending inbound frame (first watch or a re-watch), or
        // wait for the next frame / queue activity.
        let Some(frame) = next_frame.take() else {
            let received = match &queue {
                Some(queue) => {
                    tokio::select! {
                        message = socket.recv() => message,
                        _ = queue.notified() => continue,
                    }
                }
                None => socket.recv().await,
            };
            match handle_inbound(&mut socket, received).await {
                Some(text) => next_frame = Some(text),
                None => {
                    app.relay.deregister(conn_id);
                    return;
                }
            }
            continue;
        };

        // Subscribe (atomic observer + watermark under the relay lock).
        let watch: WebUiWatchRequest = match serde_json::from_str(&frame) {
            Ok(watch) => watch,
            Err(_) => {
                send_error(&mut socket, WebUiErrorCode::InvalidRequest).await;
                app.relay.deregister(conn_id);
                return;
            }
        };
        match watch {
            WebUiWatchRequest::WatchWorkspace { after } => {
                let outcome = app.relay.subscribe_workspace(conn_id, after);
                if queue.is_none() {
                    queue = Some(outcome.queue.clone());
                }
                if outcome.mode == SubscribeMode::NeedsSnapshot
                    && !deliver_workspace_snapshot(&app, &mut socket, conn_id, outcome.watermark)
                        .await
                {
                    app.relay.deregister(conn_id);
                    return;
                }
            }
            WebUiWatchRequest::WatchSession { session_id, after } => {
                // Existence is confirmed with the host before the relay
                // registers anything: an unknown session gets `not_found`,
                // never an empty replay; a persisted but never-cached
                // session is forced down the snapshot path by the relay.
                if !app.host.session_exists(&session_id).await {
                    send_error(&mut socket, WebUiErrorCode::NotFound).await;
                    app.relay.deregister(conn_id);
                    return;
                }
                let outcome = app.relay.subscribe_session(conn_id, &session_id, after);
                if queue.is_none() {
                    queue = Some(outcome.queue.clone());
                }
                if outcome.mode == SubscribeMode::NeedsSnapshot
                    && !deliver_session_snapshot(&app, &mut socket, conn_id, &session_id).await
                {
                    app.relay.deregister(conn_id);
                    return;
                }
            }
        }
    }
}

/// Build and queue the workspace summary snapshot. Returns `false` when the
/// connection must be torn down (host error or impossible serialization).
async fn deliver_workspace_snapshot(
    app: &AppState,
    socket: &mut WebSocket,
    conn_id: u64,
    workspace_sequence: u64,
) -> bool {
    let snapshot = match app.host.workspace_snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => {
            send_error(socket, error.code).await;
            return false;
        }
    };
    let message = WebUiServerMessage::WorkspaceSnapshot {
        stream_id: app.relay.stream_id().to_string(),
        workspace_sequence,
        workspaces: snapshot.workspaces,
    };
    match serde_json::to_string(&message) {
        Ok(json) => {
            app.relay
                .deliver_snapshot(conn_id, SubscriptionLayer::Workspace, Arc::from(json));
            true
        }
        Err(_) => {
            send_error(socket, WebUiErrorCode::Internal).await;
            false
        }
    }
}

/// Build and queue one session's full snapshot; mirrors
/// [`deliver_workspace_snapshot`].
async fn deliver_session_snapshot(
    app: &AppState,
    socket: &mut WebSocket,
    conn_id: u64,
    session_id: &str,
) -> bool {
    let snapshot = match app.host.subscribe(session_id).await {
        Ok(mut snapshot) => {
            snapshot.stream_id = app.relay.stream_id().to_string();
            snapshot.watermark = snapshot
                .watermark
                .min(app.relay.current_sequence(session_id));
            snapshot
        }
        Err(error) => {
            tracing::debug!(conn_id, session_id, code = ?error.code, "watch rejected");
            send_error(socket, error.code).await;
            return false;
        }
    };
    let message = WebUiServerMessage::SessionSnapshot {
        snapshot: Box::new(snapshot),
    };
    match serde_json::to_string(&message) {
        Ok(json) => {
            app.relay
                .deliver_snapshot(conn_id, SubscriptionLayer::Session, Arc::from(json));
            true
        }
        Err(_) => {
            send_error(socket, WebUiErrorCode::Internal).await;
            false
        }
    }
}

async fn handle_inbound(
    socket: &mut WebSocket,
    message: Option<Result<Message, axum::Error>>,
) -> Option<String> {
    match message {
        Some(Ok(Message::Text(text))) => Some(text.to_string()),
        Some(Ok(Message::Ping(payload))) => {
            let _ = socket.send(Message::Pong(payload)).await;
            None
        }
        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => None,
        Some(Ok(_)) => {
            send_error(socket, WebUiErrorCode::InvalidRequest).await;
            None
        }
    }
}

async fn send_outbound(socket: &mut WebSocket, message: OutboundMessage) -> bool {
    match message {
        OutboundMessage::SessionJson(json) | OutboundMessage::WorkspaceJson(json) => socket
            .send(Message::Text(json.to_string().into()))
            .await
            .is_ok(),
        OutboundMessage::Close { code } => {
            let frame = CloseFrame {
                code: CloseCode::from(code),
                reason: "".into(),
            };
            socket.send(Message::Close(Some(frame))).await.is_ok()
        }
    }
}

async fn send_error(socket: &mut WebSocket, code: WebUiErrorCode) {
    let body = WebUiErrorBody { code };
    if let Ok(json) = serde_json::to_string(&body) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}
