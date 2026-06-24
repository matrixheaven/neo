# NEO-47 Local OAuth Authenticator

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a local OAuth 2.0 authenticator in Neo that can complete the authorization-code-with-PKCE flow for MCP servers (e.g. Linear's `https://mcp.linear.app/mcp`) and be reused later for OAuth-enabled model providers.

**Architecture:** Keep OAuth as a local-only, user-initiated flow. The authenticator starts a short-lived HTTP callback server on `localhost`, opens the user's browser to the provider's authorization URL, exchanges the returned code for tokens, and stores the tokens in `~/.neo/oauth.json`. A small, provider-agnostic core in `neo-agent-core` implements PKCE and token exchange; `neo-agent` provides the TUI/CLI glue and browser invocation.

**Tech Stack:** Rust 2024, `reqwest` for token exchange, `tiny_http` or `tokio::net::TcpListener` + `http` crate for local callback server, `sha2`/`rand`/`base64` for PKCE, serde JSON for token storage, `webbrowser` crate or `open` command for browser launch.

---

## Linear Context

- Linear: NEO-47
- Title: Local OAuth authenticator for MCP and providers
- Priority: Medium
- Project: CLI Commands / Runtime
- Team: Neo
- Label: Feature / Auth

## Relationship To Other Plans

- Depends on `docs/superpowers/plans/2026-06-24-neo-46-mcp-add-form.md` for the MCP add form; OAuth will be exposed as an additional action on HTTP/SSE server entries.
- Builds on NEO-17 MCP runtime work.
- Does **not** introduce hosted registries, marketplace, or cloud identity.

## User Request

The user wants to add MCP servers such as `https://mcp.linear.app/mcp` which require OAuth. Linear does not provide a static API token for headers, so OAuth is the only practical authentication path. The user notes that `docs/kimi-code` has a `/mcp-config` skill that can guide OAuth, but Neo itself needs the underlying OAuth machinery.

The authenticator should also be reusable for future OAuth-enabled model providers.

## Current State

Neo has no OAuth support. Remote HTTP/SSE MCP servers can only be authenticated via static `headers` (e.g. `Authorization=Bearer <token>`). This blocks servers that require token issuance through OAuth.

## Desired State

A reusable local OAuth authenticator with the following characteristics:

- Initiated explicitly by the user from the TUI (`/mcp` → select server → authenticate) or CLI (`neo mcp auth <server-id>`).
- Uses the OAuth 2.0 authorization-code flow with PKCE.
- Starts a local HTTP server on a free `localhost` port for the provider callback.
- Opens the user's default browser to the provider authorization URL.
- Waits for the callback, validates `state`, and exchanges the code for access/refresh tokens.
- Stores tokens locally in `~/.neo/oauth.json` (plain JSON, local-only).
- Provides tokens to MCP HTTP/SSE requests via the `Authorization` header or custom header configured per server.
- Extensible to provider OAuth by registering provider-specific client IDs, authorization endpoints, token endpoints, and scopes.

## Data Model

```rust
// Stored in ~/.neo/oauth.json
pub struct OAuthStore {
    pub entries: BTreeMap<String, OAuthTokenSet>, // key = "mcp:<server-id>" or "provider:<provider-id>"
}

pub struct OAuthTokenSet {
    pub access_token: String,
    pub token_type: String,          // e.g. "Bearer"
    pub refresh_token: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scopes: Vec<String>,
}

// Provider definition
pub struct OAuthProvider {
    pub id: String,
    pub client_id: String,
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub default_callback_port: u16,  // 0 = pick free port
}
```

## Tasks

### Task 1: Core OAuth PKCE flow in `neo-agent-core`

- [ ] Create `crates/neo-agent-core/src/oauth.rs`.
- [ ] Implement PKCE code verifier + challenge generation.
- [ ] Implement `build_authorization_url(provider, state, challenge) -> Url`.
- [ ] Implement `exchange_code_for_token(provider, code, verifier) -> Result<OAuthTokenSet>` using `reqwest`.
- [ ] Implement `refresh_access_token(provider, refresh_token) -> Result<OAuthTokenSet>`.
- [ ] Add unit tests for PKCE generation and URL building (without network).

### Task 2: Local callback server

- [ ] Implement a short-lived async HTTP server in `neo-agent-core/src/oauth/callback_server.rs`.
- [ ] Bind to `127.0.0.1:0` to pick a free port; report the actual port.
- [ ] Accept GET `/callback?code=...&state=...`.
- [ ] Validate `state` matches the value sent to the authorization URL.
- [ ] Return a simple HTML success/error page to the browser.
- [ ] Timeout after a configurable duration (e.g. 5 minutes) and return a clear error.

### Task 3: Token storage

- [ ] Add `OAuthStore` and persistence helpers in `neo-agent-core/src/oauth/store.rs`.
- [ ] Load from / save to `~/.neo/oauth.json`.
- [ ] Provide `get_token(key)`, `set_token(key, token_set)`, `remove_token(key)`.
- [ ] Ensure the file is created with user-only permissions (`0o600`) on Unix.

### Task 4: CLI and TUI integration

- [ ] Add `neo mcp auth <server-id>` CLI command in `crates/neo-agent/src/modes/run.rs`.
- [ ] Add `Auth` action in the TUI MCP manager overlay (`/mcp` → select HTTP/SSE server → `O` auth).
- [ ] In `interactive.rs`, implement `start_mcp_oauth_flow(server_id)`:
  - Look up server config.
  - Determine provider (initially hardcode Linear; later allow provider registry).
  - Generate PKCE params and state.
  - Start callback server.
  - Open browser via `webbrowser::open` or `open` command.
  - Exchange code for token.
  - Save to `OAuthStore`.
  - Update server `headers` or introduce a dedicated `oauth` field so MCP requests use the token.
- [ ] Show progress/status messages in the TUI.

### Task 5: Apply tokens to MCP HTTP/SSE requests

- [ ] Modify `McpHttpToolAdapter` / `McpHttpClient` to look up `OAuthStore` for the server.
- [ ] Inject `Authorization: Bearer <access_token>` header when an OAuth token exists and the server config does not already override it.
- [ ] Implement automatic refresh before a request if the token is expired and a refresh token exists.

### Task 6: Provider registry for OAuth

- [ ] Add built-in OAuth provider definitions (at minimum Linear).
- [ ] Allow custom providers via config `[oauth.providers.<id>]` with `client_id`, `auth_url`, `token_url`, `scopes`.
- [ ] Support per-provider `client_id` via environment variable (e.g. `NEO_OAUTH_LINEAR_CLIENT_ID`) so users can supply their own OAuth app.

### Task 7: Tests

- [ ] Unit test PKCE generation and URL parsing.
- [ ] Unit test token store load/save with a temp directory.
- [ ] Integration test the callback server with a mock HTTP request.
- [ ] Mock the token endpoint to test code exchange without network.

### Task 8: Docs

- [ ] Update `docs/mcp.md` with OAuth flow instructions.
- [ ] Document how to add a custom OAuth provider in `~/.neo/config.toml`.
- [ ] Document security model (local-only, plain JSON storage, user-only permissions).

## Testing

- `cargo run -p xtask -- test -p neo-agent-core oauth`
- `cargo run -p xtask -- test -p neo-agent mcp_auth`
- `cargo run -p xtask -- test -p neo-tui --lib`

## Notes / Constraints

- **Local-only.** No hosted OAuth proxy, no cloud identity, no marketplace.
- Token storage is plain JSON in `~/.neo/oauth.json`. Encrypting or using OS keychain is explicitly out of scope for the first version.
- The first built-in provider will be Linear. Additional providers can be added via config.
- Client secrets are not required for PKCE public clients. Users supply their own `client_id` via config or env var.
- Do not silently inject OAuth tokens into model context; tokens are runtime state used only for MCP/provider HTTP calls.
