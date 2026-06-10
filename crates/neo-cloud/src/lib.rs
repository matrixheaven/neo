//! Self-hosted Neo cloud server.

use std::{
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, bail};
use axum::{
    Extension, Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use neo_cloud_protocol::{
    BootstrapRequest, BootstrapResponse, CloudProfile, DeviceTokenLoginRequest, ErrorResponse,
    HealthResponse, ProfilePullResponse, ProfilePushRequest, ProfileStatusResponse,
};
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
            .layer(middleware::from_fn_with_state(
                self.store.clone(),
                authenticate,
            ));
        Router::new()
            .route("/v1/health", get(health))
            .route("/v1/auth/bootstrap", post(bootstrap))
            .route("/v1/auth/device-token", post(device_token_login))
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
}

impl From<anyhow::Error> for CloudError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

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
