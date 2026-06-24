# MCP OAuth: Metadata Discovery + Dynamic Client Registration

> Goal: replace Neo's hardcoded provider list with Kimi Code-style MCP OAuth discovery and dynamic client registration, so any MCP HTTP/SSE server that speaks the MCP auth spec can authenticate without manual endpoint configuration.

## Context

Neo currently hardcodes one OAuth provider (Linear) and allows arbitrary providers only through manual `[oauth.providers.<id>]` config. This is not scalable: users must know each server's `auth_url`, `token_url`, and register a client_id out of band.

Kimi Code handles this by letting the MCP server declare its own OAuth metadata via RFC 9728 / RFC 8414 discovery, then dynamically registering an OAuth client (RFC 7591) at runtime. Neo should do the same.

## Decision

Use `rmcp`'s `auth` feature as the OAuth state machine. `rmcp` already implements:

- Protected Resource Metadata discovery (RFC 9728)
- Authorization Server Metadata discovery (RFC 8414)
- Dynamic Client Registration (RFC 7591)
- PKCE + authorization URL generation
- Token exchange and refresh
- Scope upgrade on 403 insufficient_scope

Neo provides the persistence, HTTP client wrapper, MCP state machine integration, and UI/CLI glue.

## Architecture

```text
┌─────────────────────────────────────────────────────────────────────┐
│                          User / Model                               │
└──────────────────┬────────────────────────────────┬─────────────────┘
                   │ invoke                         │ browser
                   ▼                                ▼
┌──────────────────────────┐              ┌──────────────────────┐
│ mcp__<id>__authenticate  │              │ Authorization Server │
│   (synthetic tool)       │              │  (discovered URL)    │
└──────────┬───────────────┘              └──────────┬───────────┘
           │                                          │
           │ McpOAuthManager                          │ redirect
           │                                          │
           ▼                                          ▼
┌──────────────────────────────────────────────────────────────┐
│  rmcp::transport::auth::AuthorizationManager                 │
│   - discover_metadata()                                       │
│   - register_client()                                         │
│   - get_authorization_url()                                   │
│   - complete_auth()                                           │
└──────┬───────────────────────┬───────────────────────────────┘
       │ CredentialStore       │ StateStore
       ▼                       ▼
┌─────────────────┐   ┌─────────────────┐
│ ~/.neo/oauth.json│   │ in-memory map   │
└─────────────────┘   └─────────────────┘
```

## Components

### 1. `McpOAuthManager` (`neo-agent-core/src/mcp/oauth.rs`)

One instance per MCP server identity (`server_id` + canonical `server_url`). Wraps `rmcp::transport::auth::AuthorizationManager` and exposes a higher-level API:

```rust
pub struct McpOAuthManager {
    inner: AuthorizationManager,
    server_id: String,
    server_url: Url,
}

impl McpOAuthManager {
    pub async fn new(server_id: String, server_url: Url) -> Result<Self, McpOAuthError>;

    /// Start interactive auth: discover, DCR, open browser, return auth URL.
    pub async fn begin_interactive_auth(
        &mut self,
        redirect_uri: Url,
    ) -> Result<OAuthUrl, McpOAuthError>;

    /// Finish interactive auth after browser callback.
    pub async fn complete_interactive_auth(
        &mut self,
        code: &str,
        state: &str,
    ) -> Result<(), McpOAuthError>;

    /// Return an access token, refreshing if needed.
    pub async fn access_token(&self) -> Result<String, McpOAuthError>;

    /// True if credentials exist and appear usable.
    pub async fn is_authorized(&self) -> bool;
}
```

### 2. `NeoOAuthCredentialStore`

Implements `rmcp::transport::auth::CredentialStore`. Persists `StoredCredentials` to `~/.neo/oauth.json` under key `mcp:<server_id>`.

Storage format migration:

```rust
// New format
pub struct NeoOAuthCredentials {
    pub client_id: String,
    pub access_token: String,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub granted_scopes: Vec<String>,
    pub token_received_at: Option<u64>,
}
```

On read, convert legacy `OAuthTokenSet` records into `NeoOAuthCredentials` with an empty `client_id` (legacy tokens were obtained with a hardcoded provider and have no DCR client). Legacy tokens continue to work until they expire.

### 3. `NeoOAuthStateStore`

Implements `rmcp::transport::auth::StateStore`. Keeps `StoredAuthorizationState` (PKCE verifier + csrf token) in an in-memory `HashMap` keyed by csrf token. Entries are removed when the flow completes, aborts, or times out.

### 4. `NeoOAuthHttpClient`

Implements `rmcp::transport::auth::OAuthHttpClient`. Wraps Neo's existing `reqwest::Client` so that discovery, DCR, and token requests share the same TLS/proxy/timeout configuration as the rest of the application.

### 5. MCP adapter 401 handling

`McpHttpToolAdapter` catches 401 responses and returns a distinct `McpError::NeedsAuth` variant. `McpConnectionManager` maps `McpError::NeedsAuth` to `McpServerStatus::NeedsAuth` instead of `Failed`.

### 6. Synthetic authenticate tool

When a server is in `needs-auth`, `ToolRegistry` (or a dedicated layer) replaces that server's real tools with one synthetic tool:

- Name: `mcp__<server_id>__authenticate`
- Description: explains the OAuth flow and asks the model to present the URL to the user.
- Parameters: empty object `{}`.
- Execution:
  1. Create/refresh `McpOAuthManager`.
  2. Start callback server on free port.
  3. Call `register_client()` if needed, then `get_authorization_url()`.
  4. Stream the URL to the UI as status / custom event.
  5. Wait for callback.
  6. Call `complete_interactive_auth()`.
  7. Drive `McpConnectionManager::reconnect(server_id)`.
  8. Return success; real tools replace the synthetic tool on next tool list refresh.

### 7. UI/CLI integration

- **TUI `/mcp`**: a server in `needs-auth` shows `auth required`. Pressing `O` triggers the synthetic authenticate flow. The authorization URL is shown in the status panel. After success the server reconnects and tool count updates.
- **CLI**: `neo mcp auth <server-id>` runs the same flow synchronously, prints the URL, blocks on the callback, and exits.
- **Model**: the model can call `mcp__<server_id>__authenticate` when it sees the tool.

## Data Flow

1. User adds an HTTP/SSE MCP server with only a `url`.
2. `McpConnectionManager` attempts to connect / list tools.
3. Server returns 401.
4. Adapter returns `McpError::NeedsAuth`.
5. Manager sets status to `McpServerStatus::NeedsAuth`.
6. Tool list is replaced by the synthetic `authenticate` tool.
7. Model or user invokes authenticate.
8. `McpOAuthManager` discovers metadata from `server_url`.
9. If the metadata has `registration_endpoint`, perform DCR; otherwise require a pre-configured client_id.
10. Generate authorization URL with PKCE and local callback URI.
11. Browser opens; user approves.
12. Callback server receives `code` + `state`.
13. Exchange code for tokens via `AuthorizationManager`.
14. `CredentialStore` persists tokens to `~/.neo/oauth.json`.
15. Manager reconnects the server with the new token.
16. Real tools become available.

## Error Handling

| Scenario | Behavior |
|---|---|
| Discovery fails (no metadata) | Report "server does not advertise MCP OAuth metadata"; suggest static bearer token or manual `[oauth.providers]` fallback. |
| DCR fails | Report registration error; if DCR unsupported, fall back to requiring a manually configured `client_id`. |
| User cancels browser flow | Synthetic tool returns error; server stays in `needs-auth`. |
| State mismatch | Refuse token exchange with clear CSRF error. |
| Token expires | `access_token()` refreshes automatically; if refresh fails, return to `needs-auth`. |
| 403 insufficient_scope | `AuthorizationManager` can request scope upgrade; synthetic tool may be re-invoked. |

## Security

- All tokens stay in `~/.neo/oauth.json` with `0o600` Unix permissions.
- PKCE verifier never leaves local memory except when exchanged over TLS.
- `state` parameter validated on callback.
- `redirect_uri` is always `127.0.0.1` with a random free port.
- No client secret is used; MCP OAuth clients are public clients with `token_endpoint_auth_method = "none"`.

## Migration

- Remove built-in `linear` provider from code.
- Keep `[oauth.providers.<id>]` config as an **override** for servers that do not support DCR or where the user wants a fixed client_id.
- On first read of `~/.neo/oauth.json`, migrate legacy `OAuthTokenSet` entries to `NeoOAuthCredentials`.
- Deprecate `provider_for_url()` and `OAuthProviderRegistry`; delete after one release cycle if unused.

## Testing Strategy

- **Unit tests**: mock `OAuthHttpClient` to verify discovery, DCR, authorization URL generation, token exchange, and refresh paths.
- **Integration tests**: spin up a local HTTP server that returns `WWW-Authenticate`, protected resource metadata, AS metadata, DCR response, and token response; run the full flow end-to-end.
- **TUI tests**: verify that a `needs-auth` server exposes only the synthetic authenticate tool and that `O` triggers it.
- **CLI tests**: verify `neo mcp auth <server-id>` prints the URL and completes after callback.
- **Regression tests**: ensure static bearer token (`headers.Authorization`) and stdio servers are unaffected.

## Dependencies

- Add `rmcp` with features `auth`, `reqwest` to `neo-agent-core`.
- `rmcp` pulls in `oauth2`, `reqwest`, `url` (already present).

## Out of Scope

- Client ID Metadata Documents (CIMD / SEP-991) — can be added later if `rmcp` supports it.
- Client Credentials flow (SEP-1046) — only authorization-code + PKCE + DCR for now.
- Hosted/cloud identity or token proxy.
- OS keychain encryption for token storage.
