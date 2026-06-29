# Neo MCP OAuth Lifecycle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete Neo's remote MCP OAuth lifecycle so it matches the accepted Kimi-style behavior: durable OAuth client/token/discovery state, startup `NeedsAuth` semantics, model-callable authentication, `/mcp` manual authentication, built-in `mcp-config`, and transcript status rows before the first chat turn.

**Architecture:** Replace the short-lived rmcp `AuthorizationManager` runtime path with a Neo-owned `McpOAuthService` inside `McpConnectionManager`. The service owns per-server/resource OAuth identity, durable credential files, refresh, authorization flow, and invalidation. HTTP/SSE transports ask the service for tokens. Manager state maps auth-required connection errors to `NeedsAuth`, registers one synthetic `mcp__<server>__authenticate` tool, and reconnects after successful auth. CLI, TUI `/mcp`, and built-in `mcp-config` all call the same service path.

**Tech Stack:** Rust 2024, `tokio`, `serde`, `serde_json`, `url`, `sha2`, `rmcp`, `reqwest`, `tempfile`, `wiremock` or an in-crate hyper test server, `cargo run -p xtask -- test`.

---

## Constraints

- Follow `/Users/chenyuanhao/Workspace/neo/AGENTS.md`.
- Start every execution session with `icm recall-context "Neo MCP OAuth lifecycle implementation" --limit 5`.
- Use CodeGraph before grep/read when locating code in this indexed repository.
- Use `rtk` command wrappers for shell exploration.
- Do not run bare `cargo test`; verification uses `cargo run -p xtask -- test`.
- Do not use git mutations unless the user explicitly authorizes that exact command. This plan contains optional commit checkpoints, but each one is gated by explicit per-command authorization.
- Do not preserve the old MCP OAuth runtime model as a second live path. The new code may read old `~/.neo/oauth.json` once for migration, but it writes only the new MCP credential store.

## Current Code Touchpoints

- `crates/neo-agent-core/src/tools/mcp/oauth.rs`
  - Current file builds a fresh `AuthorizationManager` with `FileCredentialStore` and `InMemoryStateStore`.
  - This is the root of the expired-token bug because the rebuilt manager lacks persisted OAuth client/discovery state.
- `crates/neo-agent-core/src/tools/mcp/http.rs`
  - `HttpConfig` currently holds `auth_manager: Option<Arc<Mutex<AuthorizationManager>>>`.
  - `OAuthStreamableHttpClient::auth_header()` calls `get_access_token()` directly on rmcp manager.
- `crates/neo-agent-core/src/tools/mcp_manager.rs`
  - `McpServerStatus` has `Disabled`, `Pending`, `Connected`, `Failed`, `Reconnecting`.
  - `ManagedMcpEntry` stores `auth_manager`.
  - `build_client_for_config()` creates the fresh manager for HTTP/SSE.
  - `register_connected_tools_into()` only registers real connected tools.
- `crates/neo-agent/src/mcp_ops.rs`
  - Loads `~/.neo/oauth.json` and wires it into the manager.
  - `authenticate_mcp_server_oauth()` uses the short-lived manager path.
- `crates/neo-agent/src/modes/run/runtime/agent.rs`
  - `wait_for_mcp_manager_probe()` already waits until enabled MCP servers stop being `Pending` or `Reconnecting`.
  - It imports `build_http_client_with_oauth` for the standalone fallback path.
- `crates/neo-agent/src/modes/interactive/mcp_manager.rs`
  - `/mcp` already exposes `McpManagerAction::Auth`.
- `crates/neo-tui/src/dialogs/mcp_manager.rs`
  - Dialog has `McpToolStatus` without `NeedsAuth`.
- `crates/neo-agent-core/src/skills/builtin/mcp-config.md`
  - File exists but is not in `BUILTIN_SOURCES`.
  - Existing manifest has `disableModelInvocation: false`.
- `crates/neo-agent-core/src/events.rs`
  - No MCP startup status event exists.

## Desired End State

- Enabled MCP servers settle before the first model turn.
- `Connected` servers register real MCP tools.
- OAuth-required HTTP/SSE servers settle as `NeedsAuth` and register exactly one synthetic tool named `mcp__<server_id>__authenticate`.
- Non-auth failures settle as `Failed` and register no tools.
- `/mcp` still supports manual OAuth through the `O auth` action.
- Built-in `mcp-config` is loaded and manually invokable.
- Transcript status rows include:

```text
MCP server "linear" connected · 38 tools (http)
MCP server "github" needs OAuth - run /mcp-config login github
MCP server "docs" failed: connection refused (http)
```

- Runtime writes MCP OAuth credentials only under:

```text
~/.neo/credentials/mcp/<store_key>/client.json
~/.neo/credentials/mcp/<store_key>/tokens.json
~/.neo/credentials/mcp/<store_key>/discovery.json
```

## File Structure

Created files:

- `crates/neo-agent-core/src/tools/mcp/oauth/mod.rs` owns OAuth module exports.
- `crates/neo-agent-core/src/tools/mcp/oauth/identity.rs` owns server/resource identity and store key canonicalization.
- `crates/neo-agent-core/src/tools/mcp/oauth/store.rs` owns durable `client.json`, `tokens.json`, and `discovery.json` file IO.
- `crates/neo-agent-core/src/tools/mcp/oauth/error.rs` owns OAuth-specific error and invalidation enums.
- `crates/neo-agent-core/src/tools/mcp/oauth/flow.rs` owns active interactive OAuth flow state and transient CSRF state storage.
- `crates/neo-agent-core/src/tools/mcp/oauth/service.rs` owns token lookup, refresh, flow creation, persistence, and migration entrypoints.
- `crates/neo-agent-core/src/tools/mcp/oauth/migration.rs` is created only if migration code would make `service.rs` hard to read.

Modified files:

- `crates/neo-agent-core/src/tools/mcp/oauth.rs` is deleted after its remaining useful `InMemoryStateStore` code is moved to `oauth/flow.rs`.
- `crates/neo-agent-core/src/tools/mcp/http.rs` stops owning rmcp `AuthorizationManager` and asks `McpOAuthService` for tokens.
- `crates/neo-agent-core/src/tools/mcp/mod.rs` exports the new OAuth service types and removes old standalone OAuth exports.
- `crates/neo-agent-core/src/tools/mcp_manager.rs` owns `NeedsAuth`, synthetic authenticate tools, OAuth service wiring, and reconnect after auth.
- `crates/neo-agent-core/src/events.rs` adds serializable MCP startup status events.
- `crates/neo-agent-core/src/skills/builtin/mod.rs` registers built-in `mcp-config`.
- `crates/neo-agent-core/src/skills/builtin/mcp-config.md` is rewritten for Neo MCP config and OAuth lifecycle.
- `crates/neo-agent/src/mcp_ops.rs` routes CLI and manager OAuth through `McpOAuthService`.
- `crates/neo-agent/src/modes/run/runtime/agent.rs` emits startup MCP status and removes standalone HTTP MCP fallback.
- `crates/neo-agent/src/modes/run/output/json.rs` maps MCP startup status events to JSON lifecycle output.
- `crates/neo-agent/src/modes/interactive/mcp_manager.rs` keeps manual `/mcp` OAuth and refreshes rows after auth.
- `crates/neo-tui/src/dialogs/mcp_manager.rs` renders `NeedsAuth`.
- `crates/neo-tui/src/transcript/event_handler.rs` renders startup transcript rows.

Task tracking:

- [ ] Phase 1: Add OAuth identity, store, and errors.
- [ ] Phase 2: Add OAuth service, flow restoration, and migration reader.
- [ ] Phase 3: Integrate HTTP/SSE transport with Neo OAuth service.
- [ ] Phase 4: Add manager `NeedsAuth`, synthetic authenticate tool, and reconnect.
- [ ] Phase 5: Update CLI, `/mcp`, and `mcp_ops`.
- [ ] Phase 6: Add startup transcript status events and renderers.
- [ ] Phase 7: Register and rewrite built-in `mcp-config`.
- [ ] Phase 8: Remove old runtime OAuth path.
- [ ] Phase 9: Add integration tests.
- [ ] Phase 10: Run focused verification.

## Phase 1: OAuth Identity And Store

### Task 1.1: Split `mcp/oauth.rs` Into A Module Directory

- [ ] Create the new OAuth module directory and files listed below.
- [ ] Move the existing transient state-store implementation into `oauth/flow.rs`.
- [ ] Delete `crates/neo-agent-core/src/tools/mcp/oauth.rs` after the module directory compiles.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core oauth` and expect the OAuth module tests to compile or fail only on tests that later tasks intentionally add.

Create these files:

- `crates/neo-agent-core/src/tools/mcp/oauth/mod.rs`
- `crates/neo-agent-core/src/tools/mcp/oauth/identity.rs`
- `crates/neo-agent-core/src/tools/mcp/oauth/store.rs`
- `crates/neo-agent-core/src/tools/mcp/oauth/error.rs`
- `crates/neo-agent-core/src/tools/mcp/oauth/flow.rs`
- `crates/neo-agent-core/src/tools/mcp/oauth/service.rs`

Delete `crates/neo-agent-core/src/tools/mcp/oauth.rs` after moving the still-needed `InMemoryStateStore` into `flow.rs`.

`mod.rs` must export the public API used outside the module:

```rust
mod error;
mod flow;
mod identity;
mod service;
mod store;

pub use error::{InvalidateScope, McpOAuthError};
pub use flow::{InMemoryStateStore, McpOAuthFlow};
pub use identity::{McpOAuthIdentity, McpOAuthTransportKind};
pub use service::{McpOAuthService, McpOAuthServiceConfig};
pub use store::{McpOAuthClientRecord, McpOAuthDiscoveryRecord, McpOAuthStore, McpOAuthTokenRecord};
```

### Task 1.2: Add OAuth Identity Canonicalization

- [ ] Write the `McpOAuthIdentity` implementation in `crates/neo-agent-core/src/tools/mcp/oauth/identity.rs`.
- [ ] Add the three unit tests shown in this task.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core canonical_url_removes_fragment_and_keeps_query`.
- [ ] Expected result: PASS for the identity canonicalization test.

Implement `identity.rs`:

```rust
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpOAuthTransportKind {
    Http,
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthIdentity {
    pub server_id: String,
    pub canonical_resource_url: Url,
    pub store_key: String,
    pub transport_kind: McpOAuthTransportKind,
}

#[derive(Debug, Error)]
pub enum McpOAuthIdentityError {
    #[error("MCP OAuth requires an http or https URL, got {scheme}")]
    UnsupportedScheme { scheme: String },
    #[error("invalid MCP OAuth URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
}

impl McpOAuthIdentity {
    pub fn new(
        server_id: &str,
        raw_url: &str,
        transport_kind: McpOAuthTransportKind,
    ) -> Result<Self, McpOAuthIdentityError> {
        let mut url = Url::parse(raw_url)?;
        match url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(McpOAuthIdentityError::UnsupportedScheme {
                    scheme: scheme.to_owned(),
                });
            }
        }
        url.set_fragment(None);

        let hash_input = format!("{server_id}\0{url}");
        let digest = Sha256::digest(hash_input.as_bytes());
        let digest_hex = format!("{digest:x}");
        let safe_server_id = sanitize_store_key_segment(server_id);
        let store_key = format!("{safe_server_id}-{}", &digest_hex[..24]);

        Ok(Self {
            server_id: server_id.to_owned(),
            canonical_resource_url: url,
            store_key,
            transport_kind,
        })
    }
}

fn sanitize_store_key_segment(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if out.is_empty() {
        out.push_str("server");
    }
    out
}
```

Add unit tests in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_url_removes_fragment_and_keeps_query() {
        let identity = McpOAuthIdentity::new(
            "linear",
            "https://mcp.linear.app/sse?team=a#token",
            McpOAuthTransportKind::Sse,
        )
        .unwrap();

        assert_eq!(
            identity.canonical_resource_url.as_str(),
            "https://mcp.linear.app/sse?team=a"
        );
        assert!(identity.store_key.starts_with("linear-"));
        assert_eq!(identity.store_key.len(), "linear-".len() + 24);
    }

    #[test]
    fn store_key_changes_when_url_changes() {
        let first = McpOAuthIdentity::new(
            "linear",
            "https://mcp.linear.app/sse",
            McpOAuthTransportKind::Sse,
        )
        .unwrap();
        let second = McpOAuthIdentity::new(
            "linear",
            "https://mcp.linear.app/other",
            McpOAuthTransportKind::Sse,
        )
        .unwrap();

        assert_ne!(first.store_key, second.store_key);
    }

    #[test]
    fn rejects_non_http_urls() {
        let err = McpOAuthIdentity::new(
            "local",
            "file:///tmp/server",
            McpOAuthTransportKind::Http,
        )
        .unwrap_err();

        assert!(err.to_string().contains("http or https"));
    }
}
```

### Task 1.3: Add Store Records And Atomic File Store

- [ ] Write the durable store record structs and `McpOAuthStore` in `crates/neo-agent-core/src/tools/mcp/oauth/store.rs`.
- [ ] Add the `round_trips_tokens` and `clear_tokens_is_idempotent` tests.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core round_trips_tokens`.
- [ ] Expected result: PASS and a tempdir-backed `tokens.json` round trip.

Implement `store.rs` with these records:

```rust
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::McpOAuthIdentity;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthClientRecord {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: Option<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthTokenRecord {
    pub access_token: String,
    pub token_type: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub token_received_at: u64,
    pub granted_scopes: Vec<String>,
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpOAuthDiscoveryRecord {
    pub resource_metadata: Value,
    pub authorization_server_metadata: Value,
    pub discovered_at: String,
}

#[derive(Debug, Clone)]
pub struct McpOAuthStore {
    root: PathBuf,
}

impl McpOAuthStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn server_dir(&self, identity: &McpOAuthIdentity) -> PathBuf {
        self.root.join(&identity.store_key)
    }

    pub fn load_client(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthClientRecord>> {
        read_json_optional(&self.server_dir(identity).join("client.json"))
    }

    pub fn save_client(
        &self,
        identity: &McpOAuthIdentity,
        record: &McpOAuthClientRecord,
    ) -> io::Result<()> {
        write_json_atomic(&self.server_dir(identity).join("client.json"), record)
    }

    pub fn load_tokens(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthTokenRecord>> {
        read_json_optional(&self.server_dir(identity).join("tokens.json"))
    }

    pub fn save_tokens(
        &self,
        identity: &McpOAuthIdentity,
        record: &McpOAuthTokenRecord,
    ) -> io::Result<()> {
        write_json_atomic(&self.server_dir(identity).join("tokens.json"), record)
    }

    pub fn clear_tokens(&self, identity: &McpOAuthIdentity) -> io::Result<()> {
        remove_optional(&self.server_dir(identity).join("tokens.json"))
    }

    pub fn load_discovery(
        &self,
        identity: &McpOAuthIdentity,
    ) -> io::Result<Option<McpOAuthDiscoveryRecord>> {
        read_json_optional(&self.server_dir(identity).join("discovery.json"))
    }

    pub fn save_discovery(
        &self,
        identity: &McpOAuthIdentity,
        record: &McpOAuthDiscoveryRecord,
    ) -> io::Result<()> {
        write_json_atomic(&self.server_dir(identity).join("discovery.json"), record)
    }
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<Option<T>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(invalid_data),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "OAuth store path has no parent")
    })?;
    fs::create_dir_all(parent)?;
    set_private_dir_permissions(parent)?;

    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        set_private_file_permissions(&tmp)?;
        serde_json::to_writer_pretty(&mut file, value).map_err(invalid_data)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn remove_optional(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn invalid_data(err: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}
```

Add tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::mcp::oauth::McpOAuthTransportKind;

    fn identity() -> McpOAuthIdentity {
        McpOAuthIdentity::new(
            "linear",
            "https://mcp.linear.app/sse",
            McpOAuthTransportKind::Sse,
        )
        .unwrap()
    }

    #[test]
    fn round_trips_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(temp.path().join("credentials").join("mcp"));
        let identity = identity();
        let tokens = McpOAuthTokenRecord {
            access_token: "access".to_owned(),
            token_type: Some("Bearer".to_owned()),
            refresh_token: Some("refresh".to_owned()),
            expires_in: Some(3600),
            token_received_at: 1782600000,
            granted_scopes: vec!["read".to_owned()],
            raw: serde_json::json!({"provider": "test"}),
        };

        store.save_tokens(&identity, &tokens).unwrap();
        assert_eq!(store.load_tokens(&identity).unwrap(), Some(tokens));
    }

    #[test]
    fn clear_tokens_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let store = McpOAuthStore::new(temp.path().join("credentials").join("mcp"));
        let identity = identity();

        store.clear_tokens(&identity).unwrap();
        assert_eq!(store.load_tokens(&identity).unwrap(), None);
    }
}
```

### Task 1.4: Add OAuth Error Types

- [ ] Write `InvalidateScope` and `McpOAuthError` in `crates/neo-agent-core/src/tools/mcp/oauth/error.rs`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core oauth`.
- [ ] Expected result: PASS for identity, store, and state-store tests added so far.

Implement `error.rs`:

```rust
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidateScope {
    TokensOnly,
    AllCredentials,
}

#[derive(Debug, Error)]
pub enum McpOAuthError {
    #[error("MCP server requires OAuth")]
    MissingTokens,
    #[error("MCP server requires OAuth reauthentication: {0}")]
    NeedsAuth(String),
    #[error("OAuth is not supported for this MCP transport")]
    UnsupportedTransport,
    #[error("invalid OAuth identity: {0}")]
    InvalidIdentity(String),
    #[error("OAuth store error: {0}")]
    Store(String),
    #[error("OAuth flow error: {0}")]
    Flow(String),
}

impl McpOAuthError {
    pub fn is_needs_auth(&self) -> bool {
        matches!(self, Self::MissingTokens | Self::NeedsAuth(_))
    }
}
```

## Phase 2: Service And rmcp Flow Restoration

### Task 2.1: Implement `McpOAuthServiceConfig` And Store Root Resolution

- [ ] Write the `McpOAuthServiceConfig` and `McpOAuthService` shell in `crates/neo-agent-core/src/tools/mcp/oauth/service.rs`.
- [ ] Prefer `neo_home` passed from `neo-agent`; add a crate dependency only if no existing helper can provide the default Neo home.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_oauth`.
- [ ] Expected result: service compiles and existing OAuth tests still pass.

Implement the top of `service.rs`:

```rust
use std::{path::PathBuf, sync::Arc, time::{SystemTime, UNIX_EPOCH}};

use tokio::sync::Mutex;

use super::{
    InvalidateScope, McpOAuthError, McpOAuthFlow, McpOAuthIdentity, McpOAuthStore,
    McpOAuthTokenRecord,
};

#[derive(Debug, Clone)]
pub struct McpOAuthServiceConfig {
    pub neo_home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct McpOAuthService {
    store: McpOAuthStore,
    flow_lock: Arc<Mutex<()>>,
}

impl McpOAuthService {
    pub fn new(config: McpOAuthServiceConfig) -> Self {
        let home = config.neo_home.unwrap_or_else(default_neo_home);
        Self {
            store: McpOAuthStore::new(home.join("credentials").join("mcp")),
            flow_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn from_store(store: McpOAuthStore) -> Self {
        Self {
            store,
            flow_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn store(&self) -> &McpOAuthStore {
        &self.store
    }
}

fn default_neo_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".neo")
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
```

If `dirs` is not already in `neo-agent-core` dependencies, add it to `crates/neo-agent-core/Cargo.toml` only if another crate-local home helper is unavailable. Prefer the existing config-provided `neo_home` path when manager is created from `neo-agent`.

### Task 2.2: Implement Token Lookup And Expiry Detection

- [ ] Add `has_tokens()`, `access_token()`, `refresh()`, `invalidate()`, and expiry helpers to `service.rs`.
- [ ] Add freshness tests for no expiry, future expiry, and expiry within 60 seconds.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core token_is_fresh`.
- [ ] Expected result: PASS for all token freshness cases.

Continue `service.rs`:

```rust
impl McpOAuthService {
    pub async fn has_tokens(&self, identity: &McpOAuthIdentity) -> bool {
        self.store.load_tokens(identity).ok().flatten().is_some()
    }

    pub async fn access_token(
        &self,
        identity: &McpOAuthIdentity,
    ) -> Result<Option<String>, McpOAuthError> {
        let Some(tokens) = self
            .store
            .load_tokens(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
        else {
            return Ok(None);
        };

        if token_is_fresh(&tokens) {
            return Ok(Some(tokens.access_token));
        }

        match self.refresh(identity, &tokens).await {
            Ok(refreshed) => Ok(Some(refreshed.access_token)),
            Err(err) if err.is_needs_auth() => Err(err),
            Err(err) => Err(McpOAuthError::NeedsAuth(err.to_string())),
        }
    }

    async fn refresh(
        &self,
        identity: &McpOAuthIdentity,
        tokens: &McpOAuthTokenRecord,
    ) -> Result<McpOAuthTokenRecord, McpOAuthError> {
        let Some(refresh_token) = tokens.refresh_token.as_deref() else {
            return Err(McpOAuthError::NeedsAuth(
                "access token expired and no refresh token is available".to_owned(),
            ));
        };
        let Some(_client) = self
            .store
            .load_client(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
        else {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth client registration is missing".to_owned(),
            ));
        };
        let Some(_discovery) = self
            .store
            .load_discovery(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?
        else {
            return Err(McpOAuthError::NeedsAuth(
                "OAuth discovery metadata is missing".to_owned(),
            ));
        };

        refresh_with_rmcp(identity, refresh_token).await
    }

    pub async fn invalidate(
        &self,
        identity: &McpOAuthIdentity,
        scope: InvalidateScope,
    ) -> Result<(), McpOAuthError> {
        self.store
            .clear_tokens(identity)
            .map_err(|err| McpOAuthError::Store(err.to_string()))?;
        if matches!(scope, InvalidateScope::AllCredentials) {
            clear_all_credentials(self.store.server_dir(identity))
                .map_err(|err| McpOAuthError::Store(err.to_string()))?;
        }
        Ok(())
    }
}

fn token_is_fresh(tokens: &McpOAuthTokenRecord) -> bool {
    let Some(expires_in) = tokens.expires_in else {
        return true;
    };
    let expires_at = tokens.token_received_at.saturating_add(expires_in);
    unix_now_secs().saturating_add(60) < expires_at
}
```

Implement `refresh_with_rmcp()` by reconstructing the rmcp OAuth client from persisted `client.json` and `discovery.json`. The final code must not call `AuthorizationManager::new(base_url)` and then use it without restoring the registered client. If the rmcp API does not expose a direct client restore method, implement Neo's token refresh with `reqwest` against the stored token endpoint from `discovery.json`, using the stored `client_id`, optional `client_secret`, and `refresh_token`.

Add a unit test for `token_is_fresh()` covering:

- no expiry means fresh;
- future expiry means fresh;
- expiry within 60 seconds means stale.

### Task 2.3: Preserve `InMemoryStateStore` Only For Active Flows

- [ ] Move `InMemoryStateStore` from the deleted `oauth.rs` file to `oauth/flow.rs`.
- [ ] Keep save/load/delete behavior exactly as the existing tests expect.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core InMemoryStateStore`.
- [ ] Expected result: PASS for transient state save, load, and delete behavior.

Move the existing `InMemoryStateStore` from old `oauth.rs` into `flow.rs` unchanged in behavior. Keep its tests for save/load/delete CSRF state.

### Task 2.4: Implement `begin_authorization()` And Flow Completion

- [ ] Add `McpOAuthFlow` to `oauth/flow.rs`.
- [ ] Add `begin_authorization()` to `McpOAuthService`.
- [ ] Persist client, token, and discovery records when the flow completes.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_oauth`.
- [ ] Expected result: OAuth service tests compile without printing secrets.

`McpOAuthFlow` must be the only path that opens the browser/local callback server. It must persist:

- `client.json`;
- `tokens.json`;
- `discovery.json`.

The public API:

```rust
pub struct McpOAuthFlow {
    authorization_url: url::Url,
    identity: McpOAuthIdentity,
    service: McpOAuthService,
    manager: Arc<tokio::sync::Mutex<rmcp::transport::auth::AuthorizationManager>>,
}

impl McpOAuthFlow {
    pub fn authorization_url(&self) -> &url::Url {
        &self.authorization_url
    }

    pub fn identity(&self) -> &McpOAuthIdentity {
        &self.identity
    }

    pub async fn complete(self, timeout: std::time::Duration) -> Result<(), McpOAuthError> {
        let _timeout = timeout;
        let mut manager = self.manager.lock().await;
        manager
            .start_authorization()
            .await
            .map_err(|err| McpOAuthError::Flow(err.to_string()))?;
        let credentials = manager
            .get_credentials()
            .await
            .map_err(|err| McpOAuthError::Flow(err.to_string()))?;
        self.service
            .persist_rmcp_credentials(&self.identity, credentials)
            .await
    }
}
```

Adjust the exact rmcp calls to match the crate API. The contract is strict: `complete()` returns only after tokens are persisted or the flow failed.

`McpOAuthService::begin_authorization()` must:

```rust
impl McpOAuthService {
    pub async fn begin_authorization(
        &self,
        identity: McpOAuthIdentity,
    ) -> Result<McpOAuthFlow, McpOAuthError> {
        let _guard = self.flow_lock.lock().await;
        let mut manager = rmcp::transport::auth::AuthorizationManager::new(
            identity.canonical_resource_url.as_str(),
        )
        .await
        .map_err(|err| McpOAuthError::Flow(err.to_string()))?;

        manager.set_state_store(super::InMemoryStateStore::new());
        configure_manager_from_store(&mut manager, self, &identity).await?;

        let authorization_url = manager
            .get_authorization_url()
            .await
            .map_err(|err| McpOAuthError::Flow(err.to_string()))?;

        Ok(McpOAuthFlow {
            authorization_url,
            identity,
            service: self.clone(),
            manager: Arc::new(Mutex::new(manager)),
        })
    }
}
```

If rmcp exposes a different method name for generating the authorization URL, use that method, but keep the service API and persistence contract unchanged.

### Task 2.5: Add Migration Reader From Old `oauth.json`

- [ ] Add migration code in `service.rs` or `oauth/migration.rs`.
- [ ] Add tests for missing old file, migrated old token response, and expired migrated token without client metadata.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core migration`.
- [ ] Expected result: migration writes only new `tokens.json` and does not keep old runtime store reads alive.

Add `crates/neo-agent-core/src/tools/mcp/oauth/migration.rs` only if the logic is too large for `service.rs`; otherwise keep it private in `service.rs`.

Behavior:

- Input: old oauth file path, configured server id, new `McpOAuthIdentity`.
- Read key `mcp:<server_id>` from old `OAuthStore`.
- If token response can be converted to `McpOAuthTokenRecord`, write `tokens.json`.
- Do not fabricate `client.json` or `discovery.json`.
- Return:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthMigrationOutcome {
    NotFound,
    TokensMigrated,
    Unusable,
}
```

Tests:

- missing old file returns `NotFound`;
- old token response writes `tokens.json`;
- missing client/discovery still causes expired-token access to return `NeedsAuth`.

## Phase 3: HTTP/SSE Transport Integration

### Task 3.1: Replace `auth_manager` With Neo OAuth Config

- [ ] Modify `crates/neo-agent-core/src/tools/mcp/http.rs` to add `HttpOAuthConfig`.
- [ ] Replace `HttpConfig.auth_manager` with `HttpConfig.oauth`.
- [ ] Update `Debug` output to show only whether OAuth is configured.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core http_config_holds_values`.
- [ ] Expected result: test is updated to assert `oauth: false` behavior and no token values are rendered.

Modify `crates/neo-agent-core/src/tools/mcp/http.rs`:

```rust
use super::oauth::{McpOAuthIdentity, McpOAuthService};

#[derive(Clone)]
pub struct HttpOAuthConfig {
    pub service: McpOAuthService,
    pub identity: McpOAuthIdentity,
}

#[derive(Clone, serde::Deserialize, Default)]
pub struct HttpConfig {
    pub url: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub startup_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
    #[serde(skip)]
    pub oauth: Option<HttpOAuthConfig>,
}
```

Update `Debug` to print `oauth: true/false` and never print token values.

Rename `OAuthStreamableHttpClient` to `NeoStreamableHttpClient` or keep the name if a broad rename causes churn. The struct must hold:

```rust
#[derive(Clone)]
pub struct NeoStreamableHttpClient {
    client: reqwest::Client,
    oauth: Option<HttpOAuthConfig>,
}
```

### Task 3.2: Map Token Absence And 401 To Auth-Required Errors

- [ ] Update `auth_header()` to call `McpOAuthService::access_token()`.
- [ ] Add `OAuthHttpError::NeedsAuth`.
- [ ] Map rmcp auth-required initialization errors to `McpError::needs_auth()`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core friendly_http_init_error`.
- [ ] Expected result: auth-required strings map to `McpErrorKind::NeedsAuth`.

Update `auth_header()`:

```rust
async fn auth_header(
    &self,
    custom_headers: &HashMap<HeaderName, HeaderValue>,
) -> Result<Option<String>, OAuthHttpError> {
    if custom_headers.contains_key(&http::header::AUTHORIZATION) {
        return Ok(None);
    }
    let Some(oauth) = &self.oauth else {
        return Ok(None);
    };
    oauth
        .service
        .access_token(&oauth.identity)
        .await
        .map_err(|err| {
            if err.is_needs_auth() {
                OAuthHttpError::NeedsAuth(err.to_string())
            } else {
                OAuthHttpError::Auth(err.to_string())
            }
        })
}
```

Update `OAuthHttpError`:

```rust
#[derive(Debug, Error)]
pub enum OAuthHttpError {
    #[error("reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("OAuth required: {0}")]
    NeedsAuth(String),
    #[error("OAuth error: {0}")]
    Auth(String),
}
```

Update `friendly_http_init_error()` so any auth-required rmcp initialization error returns `McpError::needs_auth()` instead of `McpError::protocol()`:

```rust
if display.contains("AuthRequired")
    || display.contains("AuthRequiredError")
    || display.contains("auth_required")
    || display.contains("401")
    || display.contains("Unauthorized")
{
    return McpError::needs_auth(
        "Server requires OAuth authorization. Run /mcp-config login <server_id> to authenticate.",
    );
}
```

### Task 3.3: Add `McpErrorKind`

- [ ] Add `McpErrorKind` and preserve `McpError::protocol()` callers.
- [ ] Add `McpError::needs_auth()`, `kind()`, and `is_needs_auth()`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_error`.
- [ ] Expected result: existing MCP tests still compile and new kind tests pass.

Modify `crates/neo-agent-core/src/tools/mcp/mod.rs` or wherever `McpError` is defined:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpErrorKind {
    Protocol,
    NeedsAuth,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct McpError {
    kind: McpErrorKind,
    message: String,
}

impl McpError {
    pub fn protocol(message: impl Into<String>) -> Self {
        Self {
            kind: McpErrorKind::Protocol,
            message: message.into(),
        }
    }

    pub fn needs_auth(message: impl Into<String>) -> Self {
        Self {
            kind: McpErrorKind::NeedsAuth,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> McpErrorKind {
        self.kind
    }

    pub fn is_needs_auth(&self) -> bool {
        matches!(self.kind, McpErrorKind::NeedsAuth)
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}
```

Keep existing `McpError::protocol()` callers compiling.

Tests:

- `friendly_http_init_error()` maps an auth-required string to `NeedsAuth`.
- non-auth connection errors remain `Protocol`.

## Phase 4: Manager Status, Synthetic Tool, And Reconnect

### Task 4.1: Extend `McpServerStatus`

- [ ] Add `McpServerStatus::NeedsAuth`.
- [ ] Update `as_str()` and all exhaustive matches.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_manager`.
- [ ] Expected result: compile errors identify every match that still needs explicit `NeedsAuth` handling.

In `crates/neo-agent-core/src/tools/mcp_manager.rs` add:

```rust
pub enum McpServerStatus {
    Disabled,
    Pending,
    Connected,
    NeedsAuth,
    Failed,
    Reconnecting,
}
```

Update `as_str()`:

```rust
Self::NeedsAuth => "needs_auth",
```

### Task 4.2: Replace Manager OAuth Fields

- [ ] Replace `auth_manager` with `oauth_identity` in `ManagedMcpEntry`.
- [ ] Replace old OAuth store fields with `oauth_service` in `McpConnectionManagerState`.
- [ ] Update constructors and remove old `with_oauth_store()` and `set_oauth_store()` after callers are migrated.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_manager`.
- [ ] Expected result: manager compiles after callers are updated in the same phase.

Change `ManagedMcpEntry`:

```rust
oauth_identity: Option<McpOAuthIdentity>,
```

Remove:

```rust
auth_manager: Option<Arc<Mutex<AuthorizationManager>>>,
```

Change `McpConnectionManagerState`:

```rust
oauth_service: McpOAuthService,
```

Remove:

```rust
oauth_store: Arc<RwLock<OAuthStore>>,
oauth_store_path: Option<PathBuf>,
```

Update constructors:

```rust
impl McpConnectionManager {
    pub fn new(supervisor: ProcessSupervisor) -> Self {
        Self::with_oauth_service(
            supervisor,
            McpOAuthService::new(McpOAuthServiceConfig { neo_home: None }),
        )
    }

    pub fn with_oauth_service(supervisor: ProcessSupervisor, oauth_service: McpOAuthService) -> Self {
        Self {
            inner: Arc::new(RwLock::new(McpConnectionManagerState {
                supervisor,
                entries: BTreeMap::new(),
                next_attempt_id: 1,
                oauth_service,
            })),
        }
    }

    pub async fn set_oauth_service(&self, oauth_service: McpOAuthService) {
        self.inner.write().await.oauth_service = oauth_service;
    }
}
```

Remove `with_oauth_store()` and `set_oauth_store()` after updating all callers.

### Task 4.3: Build HTTP/SSE Clients With Identity

- [ ] Pass `McpOAuthService` through `spawn_connect()`, `connect_one()`, and `build_client_for_config()`.
- [ ] Create OAuth identity for HTTP and SSE transports.
- [ ] Return `BuiltClient` with `oauth_identity`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_manager`.
- [ ] Expected result: HTTP/SSE manager code no longer references `AuthorizationManager`.

Change `spawn_connect()` and `connect_one()` to pass `McpOAuthService` instead of old OAuth store pieces.

`build_client_for_config()` should return:

```rust
struct BuiltClient {
    client: Arc<dyn McpClient>,
    oauth_identity: Option<McpOAuthIdentity>,
}
```

HTTP/SSE branch:

```rust
let transport_kind = match &config.transport {
    ManagedMcpTransport::Http { .. } => McpOAuthTransportKind::Http,
    ManagedMcpTransport::Sse { .. } => McpOAuthTransportKind::Sse,
    ManagedMcpTransport::Stdio { .. } => unreachable!(),
};
let identity = McpOAuthIdentity::new(&config.id, url, transport_kind)
    .map_err(|err| McpError::protocol(err.to_string()))?;
let client = http::build_http_client(HttpConfig {
    url: url.clone(),
    headers: headers.clone(),
    startup_timeout_ms: config.startup_timeout_ms,
    request_timeout_ms: config.tool_timeout_ms,
    oauth: Some(http::HttpOAuthConfig {
        service: oauth_service.clone(),
        identity: identity.clone(),
    }),
})
.await?;
Ok(BuiltClient {
    client,
    oauth_identity: Some(identity),
})
```

### Task 4.4: Map `NeedsAuth` Without Reconnect Backoff

- [ ] Add `set_needs_auth()`.
- [ ] Use it in connect and refresh error handling when `err.is_needs_auth()`.
- [ ] Confirm `NeedsAuth` does not increment reconnect attempts or schedule backoff.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core needs_auth`.
- [ ] Expected result: `NeedsAuth` is a settled state.

Add helper:

```rust
fn set_needs_auth(entry: &mut ManagedMcpEntry, diagnostic: McpDiagnostic) {
    entry.status = McpServerStatus::NeedsAuth;
    entry.error = Some(diagnostic);
    entry.client = None;
    entry.tools.clear();
    entry.resources.clear();
    entry.next_retry_ms = None;
}
```

In `poll_finished_connections()`, when `err.is_needs_auth()`:

```rust
let diagnostic = diagnostic_from_error(&err, &entry.config, None);
set_needs_auth(entry, diagnostic);
```

Do not call `set_failed()` and do not schedule reconnect for `NeedsAuth`.

Update `refresh_tools()`:

- If discovery fails with `NeedsAuth`, set `NeedsAuth`.
- Return the snapshot without scheduling reconnect.

### Task 4.5: Register Synthetic Authenticate Tool

- [ ] Add `McpAuthenticateTool`.
- [ ] Add `McpConnectionManager::authenticate_oauth()` or a core flow-returning equivalent if browser opening remains in `neo-agent`.
- [ ] Register exactly one authenticate tool for `NeedsAuth`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core needs_auth_registers_authenticate_tool_only`.
- [ ] Expected result: the registry contains `mcp__linear__authenticate` and no real tools for the needs-auth server.

Add `McpAuthenticateTool` near `ManagedMcpTool`:

```rust
struct McpAuthenticateTool {
    server_id: String,
    exposed_name: String,
    manager: McpConnectionManager,
}

impl super::Tool for McpAuthenticateTool {
    fn name(&self) -> &str {
        &self.exposed_name
    }

    fn description(&self) -> &str {
        "Authenticate this MCP server with OAuth, then reconnect it."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn execute<'a>(
        &'a self,
        _ctx: &'a super::ToolContext,
        _input: serde_json::Value,
    ) -> super::ToolFuture<'a> {
        let server_id = self.server_id.clone();
        let manager = self.manager.clone();
        Box::pin(async move {
            manager.authenticate_oauth(&server_id).await.map_err(|err| {
                super::ToolError::Mcp {
                    server_id: server_id.clone(),
                    tool_name: "authenticate".to_owned(),
                    message: err.to_string(),
                }
            })
        })
    }
}
```

Add manager method:

```rust
pub async fn authenticate_oauth(&self, server_id: &str) -> anyhow::Result<super::ToolResult> {
    let (identity, service) = {
        let state = self.inner.read().await;
        let entry = state
            .entries
            .get(server_id)
            .with_context(|| format!("MCP server '{server_id}' not found"))?;
        let identity = oauth_identity_for_config(&entry.config)?;
        (identity, state.oauth_service.clone())
    };

    let flow = service.begin_authorization(identity).await?;
    let auth_url = flow.authorization_url().to_string();
    open_authorization_url(&auth_url);
    flow.complete(Duration::from_secs(300)).await?;
    let snapshot = self.reconnect_now(server_id).await?;

    Ok(super::ToolResult::ok(format!(
        "Authenticated MCP server \"{}\" and reconnected with {} tools.",
        snapshot.id, snapshot.tool_count
    )))
}
```

If opening the browser belongs in `neo-agent` rather than `neo-agent-core`, make the core method return an `McpOAuthFlow` and keep browser opening in CLI/TUI. The final architecture must still have one shared service path and one shared reconnect path.

Update `register_connected_tools_into()`:

```rust
if matches!(entry.status, McpServerStatus::NeedsAuth) {
    let exposed_name = namespaced_tool_name(&entry.config.id, "authenticate");
    registry.register(McpAuthenticateTool {
        server_id: entry.config.id.clone(),
        exposed_name,
        manager: self.clone(),
    });
    if let Some(error) = &entry.error {
        diagnostics.push(error.clone());
    }
    continue;
}
```

Tests:

- A `NeedsAuth` entry registers `mcp__linear__authenticate`.
- `Failed` entry registers no synthetic tool.
- `Connected` entry registers real tools and not authenticate.

## Phase 5: CLI, `/mcp`, And `mcp_ops`

### Task 5.1: Replace Old OAuth Store Wiring

- [ ] Modify `mcp_ops.rs` to create `McpOAuthService` from `AppConfig`.
- [ ] Remove runtime loading of `~/.neo/oauth.json` into manager state.
- [ ] Keep only explicit one-time migration reads.
- [ ] Run `cargo run -p xtask -- test -p neo-agent mcp_ops`.
- [ ] Expected result: manager setup compiles without old OAuth store wiring.

Modify `crates/neo-agent/src/mcp_ops.rs`:

- Stop loading `~/.neo/oauth.json` into `OAuthStore` for manager runtime.
- Create `McpOAuthService::new(McpOAuthServiceConfig { neo_home })`.
- Call `manager.set_oauth_service(service).await`.
- Keep a one-time migration call before applying config.

New helper:

```rust
fn mcp_oauth_service_for_config(config: &AppConfig) -> McpOAuthService {
    McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: config.neo_home.clone(),
    })
}
```

### Task 5.2: Update `authenticate_mcp_server_oauth()`

- [ ] Replace `build_authorization_manager()` with `McpOAuthService::begin_authorization()`.
- [ ] Open the returned authorization URL.
- [ ] Complete the flow and persist credentials through the service.
- [ ] Run `cargo run -p xtask -- test -p neo-agent authenticate_mcp_server_oauth`.
- [ ] Expected result: tests use fake flow/server state and do not open a real browser.

Current function calls `build_authorization_manager()`. Replace it with service flow:

```rust
pub async fn authenticate_mcp_server_oauth(
    server_id: &str,
    server: &ConfiguredMcpServer,
    neo_home: &Path,
) -> anyhow::Result<String> {
    let (url, transport_kind) = oauth_url_and_transport(server)
        .with_context(|| format!("MCP server '{server_id}' does not use HTTP/SSE OAuth"))?;
    let identity = McpOAuthIdentity::new(server_id, url, transport_kind)?;
    let service = McpOAuthService::new(McpOAuthServiceConfig {
        neo_home: Some(neo_home.to_path_buf()),
    });
    let flow = service.begin_authorization(identity).await?;
    let auth_url = flow.authorization_url().to_string();
    open::that(&auth_url).context("failed to open OAuth authorization URL")?;
    flow.complete(Duration::from_secs(300)).await?;
    Ok(auth_url)
}
```

Return value can remain the opened URL if existing callers display it. Do not print tokens.

### Task 5.3: Add `NeedsAuth` To Discovery Mapping

- [ ] Add `McpToolDiscovery::NeedsAuth(String)`.
- [ ] Map `McpServerStatus::NeedsAuth` to that discovery value.
- [ ] Ensure wait loops still wait only on `Pending` and `Reconnecting`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent mcp_tool_discovery`.
- [ ] Expected result: `NeedsAuth` is visible to CLI/TUI and never blocks startup wait loops.

Update `McpToolDiscovery`:

```rust
pub enum McpToolDiscovery {
    NotRequested,
    Success(Vec<String>),
    NeedsAuth(String),
    Failed(String),
    SkippedDisabled,
}
```

Map snapshots:

```rust
McpServerStatus::NeedsAuth => McpToolDiscovery::NeedsAuth(
    snapshot
        .error
        .as_ref()
        .map(|error| error.message.clone())
        .unwrap_or_else(|| "OAuth required".to_owned()),
),
```

Update wait loops in `probe_mcp_servers()`, `list_mcp_resources()`, and `read_mcp_resource()` so `NeedsAuth` is settled:

```rust
matches!(snapshot.status, McpServerStatus::Pending | McpServerStatus::Reconnecting)
```

This expression already excludes `NeedsAuth`; add tests to lock that in.

### Task 5.4: Update Interactive `/mcp`

- [ ] Add `McpToolStatus::NeedsAuth`.
- [ ] Map `McpToolDiscovery::NeedsAuth` to the new TUI status.
- [ ] After manual auth succeeds, reconnect and refresh rows.
- [ ] Run `cargo run -p xtask -- test -p neo-tui mcp_manager`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent interactive_mcp`.
- [ ] Expected result: `/mcp` renders OAuth-required rows and preserves `O auth`.

Modify `crates/neo-agent/src/modes/interactive/mcp_manager.rs`:

- Map `McpToolDiscovery::NeedsAuth(reason)` to `McpToolStatus::NeedsAuth(reason)`.
- After successful manual auth, call manager reconnect for that server and refresh rows.
- Preserve the existing `O auth` action for remote HTTP/SSE.

Modify `crates/neo-tui/src/dialogs/mcp_manager.rs`:

```rust
pub enum McpToolStatus {
    NotDiscovered,
    Discovering,
    Discovered(Vec<String>),
    NeedsAuth(String),
    Failed(String),
    SkippedDisabled,
}

impl McpToolStatus {
    fn summary(&self) -> String {
        match self {
            Self::NeedsAuth(reason) => format!("tools: OAuth required - {reason}"),
            // existing arms
        }
    }
}
```

Rendering should use the same error style for `NeedsAuth` as `Failed`, but the text must say OAuth required rather than failed.

Tests:

- `/mcp` row renders `OAuth required`.
- `O auth` still works for HTTP and SSE rows.
- `O auth` remains ignored for stdio rows.

## Phase 6: Startup Transcript Status Rows

### Task 6.1: Add Serializable MCP Startup Event

- [ ] Add `McpStartupStatus`, `McpStartupStatusEvent`, and `AgentEvent::McpServerStatus`.
- [ ] Add serde round-trip test.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_server_status_event_round_trips`.
- [ ] Expected result: event serializes and deserializes unchanged.

Modify `crates/neo-agent-core/src/events.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum McpStartupStatus {
    Connected,
    NeedsAuth,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpStartupStatusEvent {
    pub server_id: String,
    pub transport: String,
    pub status: McpStartupStatus,
    pub tool_count: usize,
    pub message: Option<String>,
}
```

Add variant:

```rust
AgentEvent::McpServerStatus {
    status: McpStartupStatusEvent,
}
```

Add serde round-trip tests next to existing event tests:

```rust
#[test]
fn mcp_server_status_event_round_trips() {
    let event = AgentEvent::McpServerStatus {
        status: McpStartupStatusEvent {
            server_id: "linear".to_owned(),
            transport: "http".to_owned(),
            status: McpStartupStatus::Connected,
            tool_count: 38,
            message: None,
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let back: AgentEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event, back);
}
```

### Task 6.2: Emit Rows After Startup Settle

- [ ] Emit one MCP startup status event for each enabled server after the manager has settled.
- [ ] Keep `NeedsAuth` settled and non-blocking.
- [ ] Pass `None` for call sites that cannot display startup events.
- [ ] Run `cargo run -p xtask -- test -p neo-agent mcp_startup`.
- [ ] Expected result: startup events are emitted before the first model turn in interactive/run paths.

Modify `crates/neo-agent/src/modes/run/runtime/agent.rs`.

After `wait_for_mcp_manager_probe(manager_ref, config).await`, collect snapshots and emit one event per enabled server before registry registration. If `tool_registry_for_config()` does not currently have an event sink, change its signature to accept an optional startup event callback:

```rust
pub async fn tool_registry_for_config(
    config: &AppConfig,
    todos: Arc<TodoStore>,
    mcp_manager: Option<McpConnectionManager>,
    startup_events: Option<Arc<dyn Fn(AgentEvent) + Send + Sync>>,
) -> anyhow::Result<ToolRegistry>
```

For call sites that do not have an event channel, pass `None`.

Emission helper:

```rust
fn mcp_startup_event(snapshot: &McpServerSnapshot) -> Option<AgentEvent> {
    let status = match snapshot.status {
        McpServerStatus::Connected => McpStartupStatus::Connected,
        McpServerStatus::NeedsAuth => McpStartupStatus::NeedsAuth,
        McpServerStatus::Failed => McpStartupStatus::Failed,
        McpServerStatus::Disabled | McpServerStatus::Pending | McpServerStatus::Reconnecting => {
            return None;
        }
    };
    Some(AgentEvent::McpServerStatus {
        status: McpStartupStatusEvent {
            server_id: snapshot.id.clone(),
            transport: snapshot.transport.clone(),
            status,
            tool_count: snapshot.tool_count,
            message: snapshot.error.as_ref().map(|error| error.message.clone()),
        },
    })
}
```

If changing `tool_registry_for_config()` would cascade too far, emit the events in the interactive controller immediately after it calls the registry builder and snapshots the manager. The accepted behavior requires transcript rows before the first model turn, not a specific function boundary.

### Task 6.3: Render Transcript Rows

- [ ] Render connected, needs-auth, and failed MCP startup rows in transcript.
- [ ] Add JSON lifecycle output mapping.
- [ ] Run `cargo run -p xtask -- test -p neo-tui mcp_server_status`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent json`.
- [ ] Expected result: status rows and JSON event fields match this plan.

Modify `crates/neo-tui/src/transcript/event_handler.rs` to map `AgentEvent::McpServerStatus`:

```rust
AgentEvent::McpServerStatus { status } => {
    let text = match status.status {
        McpStartupStatus::Connected => format!(
            "MCP server \"{}\" connected · {} tools ({})",
            status.server_id, status.tool_count, status.transport
        ),
        McpStartupStatus::NeedsAuth => format!(
            "MCP server \"{}\" needs OAuth - run /mcp-config login {}",
            status.server_id, status.server_id
        ),
        McpStartupStatus::Failed => format!(
            "MCP server \"{}\" failed: {} ({})",
            status.server_id,
            status.message.as_deref().unwrap_or("unknown error"),
            status.transport
        ),
    };
    self.push_status(text);
}
```

Update JSON output mapping in `crates/neo-agent/src/modes/run/output/json.rs` so `McpServerStatus` is included in lifecycle output with fields:

```json
{
  "type": "mcp.server.status",
  "server_id": "linear",
  "transport": "http",
  "status": "connected",
  "tool_count": 38,
  "message": null
}
```

Tests:

- transcript event handler renders the exact connected row;
- renders exact needs-auth row;
- JSON mapper emits `mcp.server.status`.

## Phase 7: Built-In `mcp-config` Skill

### Task 7.1: Rewrite Skill Manifest

- [ ] Replace `crates/neo-agent-core/src/skills/builtin/mcp-config.md` with the manifest and body below.
- [ ] Confirm the body never asks the model to print secrets.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp_config`.
- [ ] Expected result: skill parses with `disableModelInvocation: true` after Task 7.2 registers it.

Modify `crates/neo-agent-core/src/skills/builtin/mcp-config.md`:

```markdown
---
name: mcp-config
description: Configure and authenticate MCP servers in Neo.
disableModelInvocation: true
---

# mcp-config

Use this skill when the user asks to list, add, remove, enable, disable, refresh, or authenticate MCP servers.

## Commands

- Use `/mcp` for the interactive MCP manager.
- Use `/mcp-config login <server>` when an HTTP or SSE MCP server needs OAuth.
- Use `mcp__<server>__authenticate` when the model-visible authenticate tool is available for a server in the `needs_auth` state.
- Use `neo mcp auth <server>` for non-interactive command-line authentication.

## OAuth Lifecycle

Remote HTTP and SSE MCP servers can settle as `needs_auth` during startup. That is not a connection failure. Authenticate the server, then refresh or reconnect it so Neo replaces the authenticate tool with real MCP tools.

Never print OAuth access tokens, refresh tokens, client secrets, or authorization codes.
```

### Task 7.2: Register Skill

- [ ] Add `MCP_CONFIG` to built-in skill sources.
- [ ] Add tests for presence, manual invocation, and auto-invocation exclusion.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core builtin_skill_names`.
- [ ] Expected result: `mcp-config` is loaded and not auto-invokable.

Modify `crates/neo-agent-core/src/skills/builtin/mod.rs`:

```rust
const MCP_CONFIG: &str = include_str!("mcp-config.md");
const BUILTIN_SOURCES: &[&str] = &[SUB_SKILL, SELF_EVO, MCP_CONFIG];
```

Tests:

- `builtin_skill_names()` contains `mcp-config`.
- Loaded skill manifest has `disable_model_invocation == true`.
- Auto-invokable skill list excludes `mcp-config`.

## Phase 8: Remove Old Runtime OAuth Path

### Task 8.1: Remove Old Exports

- [ ] Remove old OAuth manager exports from `mcp/mod.rs`.
- [ ] Export the new OAuth service types.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core mcp`.
- [ ] Expected result: no code imports `build_authorization_manager` from production runtime.

Modify `crates/neo-agent-core/src/tools/mcp/mod.rs`:

- Remove `pub use oauth::build_authorization_manager`.
- Remove `build_http_client_with_oauth`.
- Export the new OAuth service types needed by `neo-agent`.

Expected public exports:

```rust
pub use http::{HttpConfig, HttpOAuthConfig, build_http_client};
pub use oauth::{
    InvalidateScope, McpOAuthError, McpOAuthIdentity, McpOAuthService, McpOAuthServiceConfig,
    McpOAuthTransportKind,
};
```

### Task 8.2: Remove Standalone HTTP Client Fallback

- [ ] Remove `build_http_client_with_oauth` import and usage from run runtime code.
- [ ] Route configured MCP startup through `McpConnectionManager`.
- [ ] Replace fallback-dependent tests with manager-based tests.
- [ ] Run `cargo run -p xtask -- test -p neo-agent runtime`.
- [ ] Expected result: run mode compiles without standalone HTTP OAuth fallback.

Modify `crates/neo-agent/src/modes/run/runtime/agent.rs`:

- Remove import of `build_http_client_with_oauth`.
- Remove fallback code that directly builds HTTP MCP clients outside `McpConnectionManager`.
- Route all configured MCP startup through the manager.

If a test depends on standalone fallback construction, replace it with a manager-based test.

### Task 8.3: Remove Old Store Dependence From MCP Runtime

- [ ] Search for old runtime symbols.
- [ ] Remove production references.
- [ ] Keep old `OAuthStore` only in migration code/tests if still needed.
- [ ] Run the verification search command below.
- [ ] Expected result: no production runtime references to the old short-lived OAuth path remain.

Search for these symbols and remove production references:

```text
build_authorization_manager
FileCredentialStore
key_for_server
auth_manager
OAuthStore
oauth_store_path
~/.neo/oauth.json
```

Allowed remaining references:

- `InMemoryStateStore` in active flow code;
- `OAuthStore` only inside migration tests or migration reader;
- documentation that states old MCP OAuth store is migration-only.

Verification command:

```bash
rtk rg -n "build_authorization_manager|FileCredentialStore|key_for_server|auth_manager|oauth_store_path|mcp:<server_id>" crates/neo-agent-core/src crates/neo-agent/src
```

The command must show no production runtime references.

## Phase 9: Integration Tests

### Task 9.1: Manager `NeedsAuth` Test

- [ ] Add a test-only needs-auth entry helper or construct `ManagedMcpEntry` directly in the module test.
- [ ] Add the registry assertion shown below.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core needs_auth_registers_authenticate_tool_only`.
- [ ] Expected result: only `mcp__linear__authenticate` is registered.

Add or extend tests in `crates/neo-agent-core/src/tools/mcp_manager.rs` using a mock connect path if available. If connect path is not injectable, add a small private helper test for state transitions.

Required assertion:

```rust
#[tokio::test]
async fn needs_auth_registers_authenticate_tool_only() {
    let manager = McpConnectionManager::new(ProcessSupervisor::default());
    manager
        .insert_test_entry_needs_auth("linear", ManagedMcpTransport::Http {
            url: "https://mcp.linear.app/sse".to_owned(),
            headers: BTreeMap::new(),
        })
        .await;

    let mut registry = ToolRegistry::default();
    let diagnostics = manager.register_connected_tools_into(&mut registry).await;
    let names = registry
        .specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["mcp__linear__authenticate"]);
    assert_eq!(diagnostics.len(), 1);
}
```

If adding `insert_test_entry_needs_auth()` is too invasive, construct `ManagedMcpEntry` directly inside the module's test block.

### Task 9.2: HTTP 401 Maps To `NeedsAuth`

- [ ] Add an async HTTP initialization test in `http.rs`.
- [ ] Return auth-required initialize response or HTTP 401 from a local test server.
- [ ] Assert `McpErrorKind::NeedsAuth`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core http_401_maps_to_needs_auth`.
- [ ] Expected result: auth-required startup is classified as `NeedsAuth`.

Add a focused async test in `http.rs`:

- Start a local test server.
- Return an auth-required initialize response or HTTP 401.
- Call `build_http_client()` with OAuth service and no tokens.
- Assert error kind is `McpErrorKind::NeedsAuth`.

### Task 9.3: Expired Token With Missing Client Returns `NeedsAuth`

- [ ] Write stale `tokens.json` without `client.json` or `discovery.json`.
- [ ] Call `McpOAuthService::access_token()`.
- [ ] Assert `McpOAuthError::NeedsAuth`.
- [ ] Run `cargo run -p xtask -- test -p neo-agent-core expired_token_with_missing_client_returns_needs_auth`.
- [ ] Expected result: the old `OAuth client not configured` condition is represented as Neo `NeedsAuth`.

Add test in `service.rs`:

- Write stale `tokens.json` only.
- Call `access_token()`.
- Assert `McpOAuthError::NeedsAuth`.

### Task 9.4: Manual Auth Reconnect Hook Test

- [ ] Add a controller-level test using fake service or fake manager behavior.
- [ ] Trigger `McpManagerAction::Auth("linear")`.
- [ ] Assert authentication and row refresh were requested.
- [ ] Run `cargo run -p xtask -- test -p neo-agent manual_auth_reconnect`.
- [ ] Expected result: manual `/mcp` auth path reconnects without requiring a real browser.

Add a test in `crates/neo-agent/src/modes/interactive/mcp_manager.rs` or `mcp_ops.rs`:

- Use a fake service or fake manager method if available.
- Trigger `McpManagerAction::Auth("linear")`.
- Assert the path calls authentication and refreshes rows.

Keep this test at the controller boundary; do not require opening a browser.

## Phase 10: Verification

Run focused tests first:

```bash
cargo run -p xtask -- test -p neo-agent-core mcp_oauth
cargo run -p xtask -- test -p neo-agent-core mcp_manager
cargo run -p xtask -- test -p neo-agent-core mcp::http
cargo run -p xtask -- test -p neo-tui mcp_manager
cargo run -p xtask -- test -p neo-agent mcp_ops
```

Then run crate-level focused checks:

```bash
cargo run -p xtask -- test -p neo-agent-core
cargo run -p xtask -- test -p neo-agent
cargo run -p xtask -- test -p neo-tui
```

Run formatting and lint gate:

```bash
cargo run -p xtask -- check
```

If the change touches shared runtime code beyond MCP, run:

```bash
cargo run -p xtask -- test --workspace --all-features
```

Do not broaden verification to `ci` unless focused tests reveal a cross-crate risk that cannot be bounded.

## Acceptance Checklist

- [ ] Runtime no longer uses a fresh rmcp `AuthorizationManager` as the MCP token refresh authority.
- [ ] MCP OAuth credentials are written only to `~/.neo/credentials/mcp/<store_key>/`.
- [ ] Existing MCP entries from `~/.neo/oauth.json` are read only by migration code.
- [ ] Expired token plus missing client metadata returns `NeedsAuth`, not `OAuth client not configured`.
- [ ] HTTP/SSE auth-required startup settles as `McpServerStatus::NeedsAuth`.
- [ ] `NeedsAuth` does not schedule reconnect backoff.
- [ ] Startup gate proceeds after `Connected`, `NeedsAuth`, or `Failed`.
- [ ] `NeedsAuth` registers exactly one synthetic authenticate tool.
- [ ] Manual `/mcp` OAuth action remains available.
- [ ] Built-in `mcp-config` is registered, manually invokable, and not model auto-invokable.
- [ ] Startup transcript rows match the accepted strings.
- [ ] Tokens, refresh tokens, client secrets, auth codes, and Authorization headers never appear in transcript, diagnostics, model-visible tool output, or logs.

## Optional Commit Checkpoints

These checkpoints are for a human or executing agent after explicit user authorization for each git mutation command:

1. `oauth-store-service`: identity, store, service, migration, and unit tests.
2. `mcp-manager-needs-auth`: HTTP integration, manager status, synthetic authenticate tool, and manager tests.
3. `mcp-ui-skill-startup`: CLI/TUI paths, transcript event, JSON output, built-in skill, and focused tests.

Do not run `git add`, `git commit`, `git switch`, `git checkout`, or any other git mutation unless the user explicitly authorizes that exact command.
