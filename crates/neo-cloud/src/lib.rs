//! Self-hosted Neo cloud server.

use std::{
    net::TcpListener,
    path::Path,
    sync::OnceLock,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use axum::{
    Extension, Json, Router,
    extract::{Path as AxumPath, Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use neo_agent_core::{AgentEvent, AgentMessage, Content, session::replay_messages};
use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudCreateShareRequest, CloudForkSessionResponse,
    CloudImportSessionRequest, CloudImportSessionResponse, CloudProfile, CloudSessionListResponse,
    CloudSessionPayload, CloudSessionRecord, CloudSharePayload, CloudShareRecord,
    DeviceTokenLoginRequest, ErrorResponse, HealthResponse, ProfilePullResponse,
    ProfilePushRequest, ProfileStatusResponse,
};
use neo_sdk::{ExportConversation, ExportMessage, HtmlExportOptions, export_html};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone)]
struct AuthenticatedUser {
    user_id: String,
}

impl Store {
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create database directory {}", parent.display())
                })?;
            }
            let connection = Connection::open(&path)
                .with_context(|| format!("failed to open database {}", path.display()))?;
            migrate(&connection)?;
            Ok::<_, anyhow::Error>(Self {
                inner: Arc::new(Mutex::new(connection)),
            })
        })
        .await
        .context("failed to initialize cloud store")?
    }

    fn bootstrap_device(&self, device_name: &str) -> anyhow::Result<BootstrapResponse> {
        let device_name = device_name.trim();
        if device_name.is_empty() {
            bail!("device_name is required");
        }
        let user_id = Uuid::new_v4().to_string();
        let device_id = Uuid::new_v4().to_string();
        let access_token = new_token("neo_at");
        let device_token = new_token("neo_dt");
        let now = now_string();
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO users (id, created_at) VALUES (?1, ?2)",
            params![user_id, now],
        )?;
        connection.execute(
            "INSERT INTO devices (id, user_id, name, device_token_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                device_id,
                user_id,
                device_name,
                token_hash(&device_token),
                now
            ],
        )?;
        connection.execute(
            "INSERT INTO access_tokens (token_hash, user_id, device_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![token_hash(&access_token), user_id, device_id, now],
        )?;
        Ok(BootstrapResponse {
            user_id,
            device_id,
            access_token,
            device_token,
            token_type: "Bearer".to_owned(),
        })
    }

    fn login_with_device_token(
        &self,
        request: &DeviceTokenLoginRequest,
    ) -> anyhow::Result<Option<BootstrapResponse>> {
        let new_access_token = new_token("neo_at");
        let new_device_token = new_token("neo_dt");
        let now = now_string();
        let connection = self.connection()?;
        let user_id = connection
            .query_row(
                "SELECT user_id FROM devices WHERE id = ?1 AND device_token_hash = ?2",
                params![request.device_id, token_hash(&request.device_token)],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(user_id) = user_id else {
            return Ok(None);
        };
        connection.execute(
            "UPDATE devices SET device_token_hash = ?1 WHERE id = ?2",
            params![token_hash(&new_device_token), request.device_id],
        )?;
        connection.execute(
            "INSERT INTO access_tokens (token_hash, user_id, device_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                token_hash(&new_access_token),
                user_id,
                request.device_id,
                now
            ],
        )?;
        Ok(Some(BootstrapResponse {
            user_id,
            device_id: request.device_id.clone(),
            access_token: new_access_token,
            device_token: new_device_token,
            token_type: "Bearer".to_owned(),
        }))
    }

    fn user_for_access_token(&self, token: &str) -> anyhow::Result<Option<AuthenticatedUser>> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT user_id FROM access_tokens WHERE token_hash = ?1",
                params![token_hash(token)],
                |row| {
                    Ok(AuthenticatedUser {
                        user_id: row.get(0)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn push_profile(&self, user_id: &str, profile: &CloudProfile) -> anyhow::Result<i64> {
        let profile_json = serde_json::to_string(profile)?;
        let now = now_string();
        let connection = self.connection()?;
        let revision = connection
            .query_row(
                "SELECT revision FROM profiles WHERE user_id = ?1",
                params![user_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            + 1;
        connection.execute(
            "INSERT INTO profiles (user_id, profile_json, revision, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET
               profile_json = excluded.profile_json,
               revision = excluded.revision,
               updated_at = excluded.updated_at",
            params![user_id, profile_json, revision, now],
        )?;
        Ok(revision)
    }

    fn pull_profile(&self, user_id: &str) -> anyhow::Result<ProfilePullResponse> {
        let connection = self.connection()?;
        let Some((profile_json, revision, updated_at)) = connection
            .query_row(
                "SELECT profile_json, revision, updated_at FROM profiles WHERE user_id = ?1",
                params![user_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?
        else {
            return Ok(ProfilePullResponse {
                profile: CloudProfile::default(),
                revision: 0,
                updated_at: String::new(),
            });
        };
        Ok(ProfilePullResponse {
            profile: serde_json::from_str(&profile_json)?,
            revision,
            updated_at,
        })
    }

    fn profile_status(&self, user_id: &str) -> anyhow::Result<ProfileStatusResponse> {
        let pulled = self.pull_profile(user_id)?;
        Ok(ProfileStatusResponse {
            revision: pulled.revision,
            updated_at: pulled.updated_at,
        })
    }

    fn import_session(
        &self,
        user_id: &str,
        request: &CloudImportSessionRequest,
    ) -> anyhow::Result<CloudSessionRecord> {
        let messages = sanitize_messages(replay_jsonl_messages(&request.jsonl)?);
        let now = now_string();
        let session_id = format!("cs_{}", Uuid::new_v4().simple());
        let messages_json = serde_json::to_string(&messages)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO sessions (
                id, user_id, local_session_id, name, summary, remote_parent_id,
                messages_json, message_count, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session_id,
                user_id,
                request.local_session_id,
                request.name,
                Option::<String>::None,
                request.remote_parent_id,
                messages_json,
                i64::try_from(messages.len()).unwrap_or(i64::MAX),
                now,
                now,
            ],
        )?;
        drop(connection);
        self.session_record(user_id, &session_id)?
            .ok_or_else(|| anyhow::anyhow!("imported cloud session was not persisted"))
    }

    fn list_sessions(&self, user_id: &str) -> anyhow::Result<Vec<CloudSessionRecord>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, local_session_id, name, summary, remote_parent_id, message_count, created_at, updated_at
             FROM sessions
             WHERE user_id = ?1
             ORDER BY created_at ASC, id ASC",
        )?;
        let records = statement
            .query_map(params![user_id], |row| {
                Ok(CloudSessionRecord {
                    id: row.get(0)?,
                    local_session_id: row.get(1)?,
                    name: row.get(2)?,
                    summary: row.get(3)?,
                    remote_parent_id: row.get(4)?,
                    share_ids: Vec::new(),
                    message_count: row.get::<_, i64>(5)?.try_into().unwrap_or(0),
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        drop(connection);
        records
            .into_iter()
            .map(|mut record| {
                record.share_ids = self.share_ids_for_session(user_id, &record.id)?;
                Ok(record)
            })
            .collect()
    }

    fn get_session(&self, user_id: &str, session_id: &str) -> anyhow::Result<CloudSessionPayload> {
        let connection = self.connection()?;
        let Some((messages_json,)) = connection
            .query_row(
                "SELECT messages_json FROM sessions WHERE user_id = ?1 AND id = ?2",
                params![user_id, session_id],
                |row| Ok((row.get::<_, String>(0)?,)),
            )
            .optional()?
        else {
            return Err(CloudError::not_found("session not found").into());
        };
        drop(connection);
        let record = self
            .session_record(user_id, session_id)?
            .ok_or_else(|| CloudError::not_found("session not found"))?;
        Ok(CloudSessionPayload {
            record,
            messages: serde_json::from_str(&messages_json)?,
        })
    }

    fn fork_session(&self, user_id: &str, session_id: &str) -> anyhow::Result<CloudSessionRecord> {
        let parent = self.get_session(user_id, session_id)?;
        let now = now_string();
        let fork_id = format!("cs_{}", Uuid::new_v4().simple());
        let messages_json = serde_json::to_string(&parent.messages)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO sessions (
                id, user_id, local_session_id, name, summary, remote_parent_id,
                messages_json, message_count, created_at, updated_at
             ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                fork_id,
                user_id,
                parent.record.name,
                parent.record.summary,
                parent.record.id,
                messages_json,
                i64::try_from(parent.messages.len()).unwrap_or(i64::MAX),
                now,
                now,
            ],
        )?;
        drop(connection);
        self.session_record(user_id, &fork_id)?
            .ok_or_else(|| anyhow::anyhow!("forked cloud session was not persisted"))
    }

    fn create_share(
        &self,
        user_id: &str,
        session_id: &str,
        public: bool,
    ) -> anyhow::Result<CloudSharePayload> {
        let session = self.get_session(user_id, session_id)?;
        let share_id = format!("sh_{}", Uuid::new_v4().simple());
        let html = render_messages_html(
            format!("neo session {}", session.record.id),
            &session.messages,
        )?;
        let json = session_share_json(&session.record, &session.messages);
        let now = now_string();
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO shares (id, user_id, session_id, public, html, json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                share_id,
                user_id,
                session.record.id,
                public,
                html,
                serde_json::to_string(&json)?,
                now,
            ],
        )?;
        connection.execute(
            "UPDATE sessions SET updated_at = ?1 WHERE user_id = ?2 AND id = ?3",
            params![now, user_id, session_id],
        )?;
        drop(connection);
        self.get_share_for_user(user_id, &share_id)
    }

    fn get_share_for_user(
        &self,
        user_id: &str,
        share_id: &str,
    ) -> anyhow::Result<CloudSharePayload> {
        self.read_share(
            "SELECT id, session_id, public, html, json, created_at
             FROM shares
             WHERE user_id = ?1 AND id = ?2",
            params![user_id, share_id],
        )
    }

    fn get_public_share(&self, share_id: &str) -> anyhow::Result<CloudSharePayload> {
        self.read_share(
            "SELECT id, session_id, public, html, json, created_at
             FROM shares
             WHERE id = ?1 AND public = 1",
            params![share_id],
        )
    }

    fn read_share<P>(&self, query: &str, params: P) -> anyhow::Result<CloudSharePayload>
    where
        P: rusqlite::Params,
    {
        let connection = self.connection()?;
        let Some((id, session_id, public, html, json, created_at)) = connection
            .query_row(query, params, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .optional()?
        else {
            return Err(CloudError::not_found("share not found").into());
        };
        Ok(CloudSharePayload {
            record: CloudShareRecord {
                id: id.clone(),
                session_id,
                public,
                html_url: format!("/v1/shares/{id}.html"),
                json_url: format!("/v1/shares/{id}.json"),
                created_at,
            },
            html,
            json: serde_json::from_str(&json)?,
        })
    }

    fn session_record(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> anyhow::Result<Option<CloudSessionRecord>> {
        let connection = self.connection()?;
        let record = connection
            .query_row(
                "SELECT id, local_session_id, name, summary, remote_parent_id, message_count, created_at, updated_at
                 FROM sessions
                 WHERE user_id = ?1 AND id = ?2",
                params![user_id, session_id],
                |row| {
                    Ok(CloudSessionRecord {
                        id: row.get(0)?,
                        local_session_id: row.get(1)?,
                        name: row.get(2)?,
                        summary: row.get(3)?,
                        remote_parent_id: row.get(4)?,
                        share_ids: Vec::new(),
                        message_count: row.get::<_, i64>(5)?.try_into().unwrap_or(0),
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()?;
        drop(connection);
        record
            .map(|mut record| {
                record.share_ids = self.share_ids_for_session(user_id, &record.id)?;
                Ok(record)
            })
            .transpose()
    }

    fn share_ids_for_session(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> anyhow::Result<Vec<String>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id FROM shares WHERE user_id = ?1 AND session_id = ?2 ORDER BY created_at ASC, id ASC",
        )?;
        statement
            .query_map(params![user_id, session_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    fn connection(&self) -> anyhow::Result<std::sync::MutexGuard<'_, Connection>> {
        self.inner
            .lock()
            .map_err(|_| anyhow::anyhow!("cloud store lock was poisoned"))
    }
}

#[derive(Clone)]
pub struct CloudServer {
    store: Store,
}

impl CloudServer {
    #[must_use]
    pub const fn new(store: Store) -> Self {
        Self { store }
    }

    pub async fn serve(self, listener: TcpListener) -> anyhow::Result<()> {
        listener
            .set_nonblocking(true)
            .context("failed to set cloud listener nonblocking")?;
        let listener = tokio::net::TcpListener::from_std(listener)
            .context("failed to create Tokio listener")?;
        axum::serve(listener, self.router())
            .await
            .context("cloud server failed")
    }

    fn router(self) -> Router {
        let protected = Router::new()
            .route("/v1/profile", get(pull_profile).put(push_profile))
            .route("/v1/profile/status", get(profile_status))
            .route("/v1/sessions/import", post(import_session))
            .route("/v1/sessions", get(list_sessions))
            .route("/v1/sessions/{session_id}", get(get_session))
            .route("/v1/sessions/{session_id}/fork", post(fork_session))
            .route("/v1/sessions/{session_id}/shares", post(create_share))
            .layer(middleware::from_fn_with_state(
                self.store.clone(),
                authenticate,
            ));
        Router::new()
            .route("/v1/health", get(health))
            .route("/v1/auth/bootstrap", post(bootstrap))
            .route("/v1/auth/device-token", post(device_token_login))
            .route("/v1/shares/{*share_path}", get(get_public_share))
            .merge(protected)
            .with_state(self.store)
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
    })
}

fn migrate(connection: &Connection) -> anyhow::Result<()> {
    connection.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            device_token_hash TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS access_tokens (
            token_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            device_id TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS profiles (
            user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
            profile_json TEXT NOT NULL,
            revision INTEGER NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            local_session_id TEXT,
            name TEXT,
            summary TEXT,
            remote_parent_id TEXT,
            messages_json TEXT NOT NULL,
            message_count INTEGER NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS shares (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            public INTEGER NOT NULL,
            html TEXT NOT NULL,
            json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id, created_at, id);
        CREATE INDEX IF NOT EXISTS idx_shares_user_session ON shares(user_id, session_id, created_at, id);
        ",
    )?;
    Ok(())
}

async fn bootstrap(
    State(store): State<Store>,
    Json(request): Json<BootstrapRequest>,
) -> Result<Json<BootstrapResponse>, CloudError> {
    Ok(Json(store.bootstrap_device(&request.device_name)?))
}

async fn device_token_login(
    State(store): State<Store>,
    Json(request): Json<DeviceTokenLoginRequest>,
) -> Result<Json<BootstrapResponse>, CloudError> {
    store
        .login_with_device_token(&request)?
        .map(Json)
        .ok_or_else(CloudError::unauthorized)
}

async fn pull_profile(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
) -> Result<Json<ProfilePullResponse>, CloudError> {
    Ok(Json(store.pull_profile(&user.user_id)?))
}

async fn push_profile(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
    Json(request): Json<ProfilePushRequest>,
) -> Result<Json<ProfileStatusResponse>, CloudError> {
    let revision = store.push_profile(&user.user_id, &request.profile)?;
    Ok(Json(ProfileStatusResponse {
        revision,
        updated_at: store.profile_status(&user.user_id)?.updated_at,
    }))
}

async fn profile_status(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
) -> Result<Json<ProfileStatusResponse>, CloudError> {
    Ok(Json(store.profile_status(&user.user_id)?))
}

async fn import_session(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
    Json(request): Json<CloudImportSessionRequest>,
) -> Result<Json<CloudImportSessionResponse>, CloudError> {
    let record = store.import_session(&user.user_id, &request)?;
    Ok(Json(CloudImportSessionResponse { record }))
}

async fn list_sessions(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
) -> Result<Json<CloudSessionListResponse>, CloudError> {
    Ok(Json(CloudSessionListResponse {
        sessions: store.list_sessions(&user.user_id)?,
    }))
}

async fn get_session(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<CloudSessionPayload>, CloudError> {
    Ok(Json(store.get_session(&user.user_id, &session_id)?))
}

async fn fork_session(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
    AxumPath(session_id): AxumPath<String>,
) -> Result<Json<CloudForkSessionResponse>, CloudError> {
    let record = store.fork_session(&user.user_id, &session_id)?;
    Ok(Json(CloudForkSessionResponse { record }))
}

async fn create_share(
    Extension(user): Extension<AuthenticatedUser>,
    State(store): State<Store>,
    AxumPath(session_id): AxumPath<String>,
    Json(request): Json<CloudCreateShareRequest>,
) -> Result<Json<CloudSharePayload>, CloudError> {
    Ok(Json(store.create_share(
        &user.user_id,
        &session_id,
        request.public,
    )?))
}

async fn get_public_share(
    State(store): State<Store>,
    AxumPath(share_path): AxumPath<String>,
) -> Result<Response, CloudError> {
    if let Some(share_id) = share_path.strip_suffix(".html") {
        return Ok(Html(store.get_public_share(share_id)?.html).into_response());
    }
    if let Some(share_id) = share_path.strip_suffix(".json") {
        return Ok(Json(store.get_public_share(share_id)?.json).into_response());
    }
    Ok(Json(store.get_public_share(&share_path)?).into_response())
}

async fn authenticate(
    State(store): State<Store>,
    mut request: Request,
    next: Next,
) -> Result<Response, CloudError> {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(CloudError::unauthorized)?;
    let user = store
        .user_for_access_token(token)?
        .ok_or_else(CloudError::unauthorized)?;
    request.extensions_mut().insert(user);
    Ok(next.run(request).await)
}

#[derive(Debug)]
struct CloudError {
    status: StatusCode,
    message: String,
}

impl CloudError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: "unauthorized".to_owned(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for CloudError {
    fn from(error: anyhow::Error) -> Self {
        match error.downcast::<CloudError>() {
            Ok(cloud_error) => return cloud_error,
            Err(error) => {
                return Self {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    message: error.to_string(),
                };
            }
        }
    }
}

impl std::fmt::Display for CloudError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CloudError {}

impl IntoResponse for CloudError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

fn new_token(prefix: &str) -> String {
    format!("{prefix}_{}_{}", Uuid::new_v4(), Uuid::new_v4())
}

fn token_hash(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hash = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut hash, "{byte:02x}").expect("write hash");
    }
    hash
}

fn now_string() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    seconds.to_string()
}

fn replay_jsonl_messages(jsonl: &str) -> anyhow::Result<Vec<AgentMessage>> {
    let mut events = Vec::new();
    for (index, line) in jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str::<serde_json::Value>(line)
            .with_context(|| format!("invalid session JSONL on line {}", index + 1))?;
        if value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "session_metadata")
        {
            continue;
        }
        events.push(
            serde_json::from_value::<AgentEvent>(value)
                .with_context(|| format!("invalid session event on line {}", index + 1))?,
        );
    }
    Ok(replay_messages(events.iter()))
}

fn sanitize_messages(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    messages.into_iter().map(sanitize_message).collect()
}

fn sanitize_message(message: AgentMessage) -> AgentMessage {
    match message {
        AgentMessage::System { content } => AgentMessage::System {
            content: sanitize_content(content),
        },
        AgentMessage::User { content } => AgentMessage::User {
            content: sanitize_content(content),
        },
        AgentMessage::Assistant {
            content,
            tool_calls,
            stop_reason,
        } => AgentMessage::Assistant {
            content: sanitize_content(content),
            tool_calls: tool_calls
                .into_iter()
                .map(|mut call| {
                    call.arguments = sanitize_json_value(call.arguments);
                    call
                })
                .collect(),
            stop_reason,
        },
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            is_error,
        } => AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content: sanitize_content(content),
            is_error,
        },
    }
}

fn sanitize_content(content: Vec<Content>) -> Vec<Content> {
    content
        .into_iter()
        .map(|part| match part {
            Content::Text { text } => Content::Text {
                text: sanitize_text(&text),
            },
            Content::Thinking {
                text,
                signature,
                redacted,
            } => Content::Thinking {
                text: sanitize_text(&text),
                signature: signature.map(|value| sanitize_text(&value)),
                redacted,
            },
            Content::Image { mime_type, data } => Content::Image { mime_type, data },
        })
        .collect()
}

fn sanitize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => serde_json::Value::String(sanitize_text(&value)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(sanitize_json_value).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .into_iter()
                .map(|(key, value)| {
                    if sensitive_key(&key) {
                        (key, serde_json::Value::String("[REDACTED]".to_owned()))
                    } else {
                        (key, sanitize_json_value(value))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

fn sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token") || key.contains("api_key") || key.contains("apikey")
}

fn sanitize_text(text: &str) -> String {
    let redacted_paths = jsonl_path_regex().replace_all(text, "[REDACTED_PATH]");
    api_secret_regex()
        .replace_all(&redacted_paths, "[REDACTED]")
        .into_owned()
}

fn jsonl_path_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"(?:[A-Za-z]:)?/[^\s"'<>]+\.jsonl"#).expect("path regex"))
}

fn api_secret_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:sk|api|token|key)[-_A-Za-z0-9]{8,}\b"#).expect("secret regex")
    })
}

fn render_messages_html(title: String, messages: &[AgentMessage]) -> anyhow::Result<String> {
    let export_messages = messages
        .iter()
        .map(|message| ExportMessage::new(message_role(message), message_text(message)))
        .collect();
    let conversation = ExportConversation::new(title, export_messages);
    export_html(&conversation, &HtmlExportOptions::default()).map_err(Into::into)
}

fn session_share_json(record: &CloudSessionRecord, messages: &[AgentMessage]) -> serde_json::Value {
    serde_json::json!({
        "format": "neo.cloud.session.share",
        "schema_version": 1,
        "metadata": {
            "id": record.id,
            "name": record.name,
            "summary": record.summary,
            "remote_parent_id": record.remote_parent_id,
            "share_ids": record.share_ids,
            "message_count": messages.len(),
        },
        "messages": messages,
    })
}

fn message_role(message: &AgentMessage) -> &'static str {
    match message {
        AgentMessage::System { .. } => "system",
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant { .. } => "assistant",
        AgentMessage::ToolResult { .. } => "tool",
    }
}

fn message_text(message: &AgentMessage) -> String {
    let content = match message {
        AgentMessage::System { content }
        | AgentMessage::User { content }
        | AgentMessage::Assistant { content, .. }
        | AgentMessage::ToolResult { content, .. } => content,
    };
    content
        .iter()
        .filter_map(Content::as_text)
        .collect::<Vec<_>>()
        .join("")
}
