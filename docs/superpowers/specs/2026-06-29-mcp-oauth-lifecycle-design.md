# Neo MCP OAuth Lifecycle Design

> Goal: make Neo's remote MCP OAuth behavior match Kimi Code's complete lifecycle: startup connection status, needs-auth state, model-callable authentication, manual `/mcp` authentication, a built-in `mcp-config` skill, durable OAuth client/token/discovery storage, and automatic reconnect after login.

## Context

Neo already has a manager-backed MCP runtime. Enabled MCP servers are loaded during tool registry construction, the startup path waits for pending/reconnecting servers to settle, connected servers register real tools, and failed servers are isolated as diagnostics instead of aborting the agent. That startup shape is close to the desired behavior and should be preserved.

The current OAuth path is incomplete. `neo mcp auth <server>` can complete an rmcp discovery/DCR/browser flow and persist credentials, but later runtime connections rebuild a fresh `AuthorizationManager` with only the credential store attached. A non-expired access token can still work because it is read directly from the store. Once the token expires, refresh requires a configured OAuth client, and the rebuilt manager has none, producing `OAuth client not configured`.

Kimi Code solves this at the lifecycle level, not with a narrow refresh patch. It persists OAuth tokens, DCR client information, and discovery state per MCP server/resource identity; attaches an OAuth provider only after tokens exist; maps unauthorized remote servers to `needs-auth`; exposes a synthetic `mcp__<server>__authenticate` tool; and reconnects the server after auth completes.

Neo should adopt that full lifecycle and delete the misleading partial OAuth paths rather than preserving two models.

## Non-Goals

- No hosted OAuth proxy, hosted MCP registry, cloud identity, or marketplace.
- No OS keychain integration in this design; local JSON with user-only permissions remains acceptable.
- No project-local MCP config. Neo MCP config remains in global Neo config (`$NEO_HOME/config.toml` or `~/.neo/config.toml`).
- No compatibility branch that keeps writing MCP OAuth credentials to the old `~/.neo/oauth.json` shape.
- No rewrite of the existing startup gate if it already satisfies the settled-before-chat semantics.

## Desired Behavior

When Neo starts with enabled MCP servers:

1. Neo connects every enabled MCP server before the first model turn starts.
2. Each enabled server reaches a settled startup state:
   - `connected`: real MCP tools are available.
   - `needs-auth`: a synthetic `mcp__<server>__authenticate` tool is available.
   - `failed`: no tools are available, but the user can still chat.
   - `disabled`: ignored by the startup gate.
3. The transcript shows one concise status row per enabled server:
   - `MCP server "linear" connected · 38 tools (http)`
   - `MCP server "github" needs OAuth - run /mcp-config login github`
   - `MCP server "docs" failed: connection refused (http)`
4. The user can authenticate through any of three entry points:
   - The model calls `mcp__<server>__authenticate`.
   - The user presses `O Auth` in the `/mcp` manager.
   - The user invokes `/mcp-config login <server>` or `/skill:mcp-config`.
5. After OAuth succeeds, Neo reconnects that server and replaces the synthetic authenticate tool with the real MCP tools.

## Status Model

Extend `McpServerStatus` to:

```text
Disabled
Pending
Connected
NeedsAuth
Failed
Reconnecting
```

`NeedsAuth` is a settled state. It must not keep startup waiting. The startup gate should wait until enabled servers are no longer `Pending` or `Reconnecting`, then proceed.

Status meanings:

| Status | Meaning | Model-visible tools |
|---|---|---|
| `Disabled` | Configured but not started | none |
| `Pending` | Initial connect in progress | none |
| `Connected` | Server initialized and tools discovered | real MCP tools |
| `NeedsAuth` | Server requires OAuth login or token refresh failed in a reauth-required way | one synthetic authenticate tool |
| `Failed` | Non-auth startup/config/transport failure | none |
| `Reconnecting` | Backoff/reconnect in progress | prior tools are removed until settled |

## OAuth Identity

MCP OAuth credentials must be bound to both the configured server id and the canonical resource URL.

```text
McpOAuthIdentity
  server_id: String
  canonical_resource_url: Url
  store_key: String
  transport_kind: http | sse
```

Canonicalization rules:

- Parse as URL.
- Remove fragment.
- Preserve scheme, host, port, path, and query after URL parser normalization.
- Reject non-HTTP(S) URLs for OAuth identity creation.
- `store_key = safe_server_id + "-" + sha256(server_id + "\0" + canonical_resource_url)[0..24]`.

This prevents accidental reuse when a server id is kept but the URL changes.

## Persistent Store

Store MCP OAuth data under:

```text
~/.neo/credentials/mcp/<store_key>/
  client.json
  tokens.json
  discovery.json
```

Use `$NEO_HOME` when set; otherwise `~/.neo`.

Permissions:

- Parent directories: `0700` on Unix.
- JSON files: `0600` on Unix.
- Writes are atomic: write to a same-directory temp file, flush, then rename over the target. If any step fails, leave the previous file intact.

Files:

### `client.json`

Stores DCR client information needed to reconstruct the OAuth client after process restart:

```json
{
  "client_id": "...",
  "client_secret": null,
  "redirect_uris": ["http://127.0.0.1:12345/callback"],
  "token_endpoint_auth_method": "none",
  "raw": {}
}
```

`raw` preserves provider-specific returned fields that rmcp or future Neo code may need.

### `tokens.json`

Stores token response state:

```json
{
  "access_token": "...",
  "token_type": "Bearer",
  "refresh_token": "...",
  "expires_in": 3600,
  "token_received_at": 1782600000,
  "granted_scopes": ["..."],
  "raw": {}
}
```

`access_token` and `refresh_token` must never be printed in logs, diagnostics, transcript entries, or model-visible context.

### `discovery.json`

Stores discovered protected resource and authorization server metadata:

```json
{
  "resource_metadata": {},
  "authorization_server_metadata": {},
  "discovered_at": "2026-06-29T00:00:00Z"
}
```

The runtime can rediscover when this file is missing or incompatible. Stored discovery is an optimization and state restoration aid, not a source of user-facing truth when the server disagrees.

## Migration

The old MCP OAuth data in `~/.neo/oauth.json` is not the new canonical store.

Migration policy:

1. New code writes only to `~/.neo/credentials/mcp/<store_key>/`.
2. Existing `~/.neo/oauth.json` MCP entries are read only by a one-time migration path.
3. If an old entry can be safely matched to a configured server id and current URL, migrate usable token data into `tokens.json`.
4. If client information or canonical URL cannot be reconstructed, do not fabricate it. Mark that server `NeedsAuth` and ask the user to reauthenticate.
5. Do not keep a fallback reader that silently uses `mcp:<server_id>` forever.

If `~/.neo/oauth.json` remains useful for non-MCP OAuth in the future, this design does not delete the whole file. It removes MCP OAuth's dependence on it.

## Core Components

### `McpOAuthService`

Owned by `McpConnectionManager`.

Responsibilities:

- Build `McpOAuthIdentity` for remote OAuth-capable servers.
- Check whether tokens exist for an identity.
- Provide an access token, refreshing before expiry when possible.
- Start an interactive authorization flow.
- Persist client, tokens, and discovery state.
- Invalidate tokens or all credentials for an identity.

Sketch:

```rust
pub struct McpOAuthService { ... }

impl McpOAuthService {
    pub fn identity(server_id: &str, url: &str, transport: ManagedMcpTransportKind) -> Result<McpOAuthIdentity>;
    pub async fn has_tokens(&self, identity: &McpOAuthIdentity) -> bool;
    pub async fn access_token(&self, identity: &McpOAuthIdentity) -> Result<Option<String>, McpOAuthError>;
    pub async fn begin_authorization(&self, identity: McpOAuthIdentity) -> Result<McpOAuthFlow>;
    pub async fn invalidate(&self, identity: &McpOAuthIdentity, scope: InvalidateScope) -> Result<()>;
}
```

`access_token` returns `Ok(None)` only when there are no tokens. Expired/revoked tokens that require reauth should return a specific `NeedsAuth` error so the manager can transition state.

### OAuth Flow

`McpOAuthFlow` owns one local callback server and flow-scoped state:

```rust
pub struct McpOAuthFlow {
    pub authorization_url: Url,
    pub identity: McpOAuthIdentity,
    ...
}

impl McpOAuthFlow {
    pub async fn complete(self, timeout: Duration) -> Result<(), McpOAuthError>;
    pub async fn cancel(self);
}
```

Flow:

1. Start local callback server on `127.0.0.1:0`.
2. Discover protected resource metadata and authorization server metadata.
3. Restore `client.json` if compatible with the redirect URI strategy; otherwise dynamically register a public native client.
4. Generate authorization URL with PKCE and state.
5. Wait for callback.
6. Validate state.
7. Exchange code for tokens.
8. Persist `client.json`, `tokens.json`, and `discovery.json`.

### HTTP/SSE OAuth Integration

HTTP/SSE client construction must be explicit:

- If static `Authorization` header or configured static bearer token exists, do not attach OAuth. Auth failures become `Failed`.
- If no static auth and `McpOAuthService.has_tokens(identity)` is false, connect without OAuth provider. A 401/OAuth-required response maps to `NeedsAuth`.
- If tokens exist, attach OAuth token source/provider. Successful refresh updates `tokens.json`; refresh failure that semantically requires user action maps to `NeedsAuth`.

This avoids the current bug where a passive startup connection tries to drive OAuth without an active callback listener or restored client.

## Connection Manager Integration

`McpConnectionManager` remains the MCP lifecycle owner.

Changes:

- Add `NeedsAuth`.
- Hold `McpOAuthService` in manager state.
- For remote HTTP/SSE configs, derive `McpOAuthIdentity`.
- Map OAuth-required and reauth-required errors to `NeedsAuth`.
- Preserve diagnostics for `NeedsAuth`, including a concise message and suggested command.
- Register tools as:
  - `Connected`: real MCP tools.
  - `NeedsAuth`: synthetic authenticate tool.
  - others: none.
- `reconnect_now(server_id)` must work after OAuth writes credentials.

Do not duplicate connection lifecycle in TUI or CLI. `/mcp`, CLI status, transcript rows, and tool registry all consume manager snapshots.

## Synthetic Authenticate Tool

Name:

```text
mcp__<server_id>__authenticate
```

Arguments:

```json
{}
```

Description requirements:

- Explain that the server requires OAuth.
- Instruct the model to show the authorization URL verbatim.
- State that the tool waits for the browser callback.
- State that successful login reconnects the MCP server.

Execution:

1. Emit status: discovering OAuth metadata.
2. Start `McpOAuthService::begin_authorization`.
3. Emit custom/status update with the authorization URL.
4. Wait for `flow.complete(...)`.
5. Call `McpConnectionManager::reconnect_now(server_id)`.
6. Return success text.

Error behavior:

| Error | Result |
|---|---|
| Already authorized | Reconnect and return success |
| User timeout/cancel | Return tool error; stay `NeedsAuth` |
| State mismatch | Return tool error; stay `NeedsAuth`; diagnostic severity high |
| Discovery unsupported | Return tool error; transition or remain `Failed` if server cannot speak MCP OAuth |
| DCR failed | Return tool error; stay `NeedsAuth` if retryable, otherwise `Failed` |
| Reconnect failed after auth | Token remains stored; status becomes `Failed` with reconnect diagnostic |

## `/mcp` TUI Behavior

The `/mcp` manager remains the manual management surface.

Additions:

- Show `NeedsAuth` as a first-class row status:
  - `needs auth`
  - `OAuth required`
  - tool count displayed as `auth required` or `authenticate available`
- Keep `O Auth` for HTTP/SSE servers.
- `O Auth` runs the same OAuth flow as the synthetic tool.
- After success, call manager reconnect and refresh rows.
- Static-header HTTP/SSE servers should not show OAuth as the primary repair action when unauthorized; their failure is config/header related.

Manual reauth:

- If a connected OAuth server is selected and the user presses `O Auth`, Neo should allow explicit reauth.
- Reauth invalidates `tokens.json` for that identity before starting a new flow. It keeps compatible `client.json` and `discovery.json`; incompatible client/discovery data is overwritten by the new flow.

## Startup Transcript

Neo should show MCP startup results in the transcript, modeled after Kimi:

```text
MCP server "linear" connected · 38 tools (http)
MCP server "github" needs OAuth - run /mcp-config login github
MCP server "docs" failed: connection refused (http)
```

Rules:

- Emit one row for each enabled server after it reaches a settled startup state.
- Emit later rows for real state changes after startup.
- Deduplicate by `(server_id, status, transport, tool_count, diagnostic_message)`.
- Do not print tokens, URLs containing OAuth state, or raw auth headers.
- Keep these as transcript/status entries, not assistant/model messages.

If existing startup waiting already settles all enabled servers before chat, keep it. Only extend it to understand `NeedsAuth` and to surface transcript rows.

## Built-In `mcp-config` Skill

Neo already has a builtin skill slot for `mcp-config`; it should be replaced with Kimi-equivalent Neo semantics.

Manifest:

```yaml
name: mcp-config
description: Configure MCP servers and handle MCP OAuth login.
type: prompt
disableModelInvocation: true
```

Behavior:

- User can invoke with `/mcp-config` or `/skill:mcp-config`.
- The existing `crates/neo-agent-core/src/skills/builtin/mcp-config.md` file becomes part of `BUILTIN_SOURCES`; it is not merely a dormant file in the tree.
- The skill is listed as builtin but hidden from model auto-invocation listings.
- User-provided same-name skills may override the builtin skill through the existing skill precedence model.

Skill instructions:

- Login flow:
  - If `mcp__<server>__authenticate` exists and user asks to login/auth/sign in, call it.
  - If several authenticate tools exist and the user did not name one, ask which.
  - If user named a server without an authenticate tool, say that server is not currently waiting for OAuth and stop.
- Config flow:
  - Read/write Neo global config under `$NEO_HOME/config.toml` or `~/.neo/config.toml`.
  - Preserve unrelated config.
  - For OAuth HTTP/SSE servers, write only URL and MCP fields, not token/header secrets.
  - Tell the user that Neo connects MCP servers at startup; use a new session or `/mcp` reconnect/auth to refresh tools.
- Secrets:
  - Do not write bearer tokens or OAuth tokens as literals.
  - Static auth should use env references or existing header mechanisms, with warnings before inline secret writes.

## CLI Behavior

`neo mcp auth <server-id>` remains, but uses the same service and store as runtime.

Expected output:

- Prints or opens the authorization URL.
- Waits for callback.
- Saves credentials to the new MCP OAuth store.
- Reconnect behavior is only relevant in interactive/session contexts; the CLI command should report that the next session or reconnect will use the credentials.

`neo mcp status` should include `needs-auth` rows.

## Error Mapping

| Condition | Status |
|---|---|
| HTTP/SSE 401 without static auth and no tokens | `NeedsAuth` |
| OAuth metadata advertised / auth required during initialize | `NeedsAuth` |
| Token expired and refresh succeeds | stay or become `Connected` |
| Token expired and refresh token revoked/missing | `NeedsAuth` |
| 403 insufficient scope from OAuth-protected server | `NeedsAuth` with scope-upgrade diagnostic |
| Static Authorization header configured and server returns 401 | `Failed` |
| Server unreachable, DNS, connection refused, timeout | `Failed` |
| Invalid MCP protocol response | `Failed` |
| Disabled config | `Disabled` |

## Security

- OAuth tokens never enter model context.
- Authorization URLs may contain state and challenge data; the authenticate tool may show them to the user, but logs should avoid recording them at high verbosity.
- Callback binds only to `127.0.0.1`.
- State must be validated before token exchange.
- PKCE verifier is flow-local and not persisted.
- Token files are user-readable only.
- Static headers and bearer tokens keep existing redaction behavior.

## Testing Strategy

### Unit Tests

- OAuth identity canonicalization and store key stability.
- Safe server id sanitization.
- Store read/write roundtrip for `client.json`, `tokens.json`, `discovery.json`.
- Store file permissions on Unix.
- Corrupt JSON handling.
- `has_tokens` behavior for missing, present, and invalid token files.
- Error classification: OAuth required vs static-header unauthorized.

### Manager Tests

- HTTP OAuth-required startup becomes `NeedsAuth`.
- `NeedsAuth` registers only `mcp__<server>__authenticate`.
- Connected server registers real tools and not authenticate.
- Failed server registers no tools.
- Startup settle treats `NeedsAuth` as settled.
- Reconnect after auth replaces authenticate with real tools.

### Integration Tests

Use a local mock MCP/OAuth server that supports:

- MCP initialize/list tools.
- 401 OAuth challenge.
- Protected resource metadata.
- Authorization server metadata.
- Dynamic client registration.
- Token endpoint.
- Refresh endpoint.

Scenarios:

- No token -> `NeedsAuth` -> authenticate -> reconnect -> real tools.
- Expired token + refresh token -> refresh succeeds -> connected.
- Expired token + revoked refresh -> `NeedsAuth`.
- Static header 401 -> `Failed`, not `NeedsAuth`.

### TUI Tests

- `/mcp` renders `needs auth`.
- `O Auth` action is available for OAuth-capable HTTP/SSE servers.
- Startup transcript rows render connected/needs-auth/failed messages.
- Transcript rows deduplicate.

### Skill Tests

- Builtin `mcp-config` is loaded.
- It is listable to user-facing skill list.
- It does not appear in model auto skill listing.
- `/skill:mcp-config` activates it.
- User same-name skill can override builtin.

## Acceptance Criteria

- Linear MCP can be added as an HTTP MCP server with no static token.
- On startup, Linear reaches either `Connected` or `NeedsAuth` before the first model turn.
- If auth is required, the model sees `mcp__linear__authenticate`.
- `/mcp-config login linear` calls the authenticate tool and surfaces the URL unchanged.
- `/mcp` `O Auth` uses the same OAuth flow.
- After browser auth, Neo reconnects Linear and real tools appear without restarting the whole app.
- After access token expiry, refresh works across process restarts and no longer fails with `OAuth client not configured`.
- If refresh is impossible, the server becomes `NeedsAuth` rather than an opaque failed initialize request.
- Startup transcript includes Kimi-style MCP status rows.
- MCP OAuth no longer writes new credentials to `~/.neo/oauth.json`.

## Implementation Notes

- Prefer adapting rmcp's OAuth state machine where it fits, but do not expose raw `AuthorizationManager` as the persistence boundary.
- If rmcp cannot restore an OAuth client from stored DCR metadata directly, Neo should own enough metadata to call the relevant configure path before refresh.
- Keep `McpConnectionManager` as the single owner of server state; do not create a separate TUI-side manager.
- Keep verification focused. This is cross-cutting, so integration tests are valuable, but broad workspace tests are a final gate rather than the first proof.
