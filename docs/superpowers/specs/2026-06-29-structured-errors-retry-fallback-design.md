# Spec A: Structured Errors + Retry Backoff + Model Fallback

**Date:** 2026-06-29
**Status:** Approved (design phase)
**Crates affected:** `neo-ai`, `neo-agent-core`, `neo-tui`, `neo-agent`

## Motivation

Neo's error handling has three gaps:

1. **Errors are opaque strings.** `AiError` has 4 variants carrying `String` messages. The TUI cannot distinguish a rate-limit error from an auth error from a context overflow — they all render as `Error: <text>`.
2. **Retries are immediate with no backoff.** The shared `open_response()` loop retries with zero delay, no jitter, no `Retry-After` header parsing. Default retry count is 0 (no retries at all).
3. **No model fallback.** When a provider fails permanently, the error propagates immediately. There is no automatic switch to a secondary model.

Reference: kimi-code has a 50+ error-code registry (`KIMI_ERROR_INFO`), exponential backoff (300ms–5s, factor 2, jitter, max 3 attempts via `loop/retry.ts`), and OAuth 401-refresh-once. Neo adopts the error taxonomy and backoff design, adds model fallback as an increment beyond what kimi-code offers.

## Design Decision

**Rust-idiomatic typed enums, not string-code discrimination.** kimi-code uses string-literal error codes because TypeScript has no sum types. Rust has real enums with exhaustive `match` — we use typed variants internally and map to stable string codes only for serialization (JSONL session logs).

### Error Type Upgrade

Upgrade `AiError` from 4 string variants to 8 typed variants:

```rust
// crates/neo-ai/src/error.rs

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("configuration error: {message}")]
    Configuration { message: String },

    #[error("rate limited: {message}")]
    RateLimit {
        message: String,
        retry_after: Option<Duration>,
    },

    #[error("authentication error: {message}")]
    Auth { message: String },

    #[error("context overflow: {message}")]
    ContextOverflow { message: String },

    #[error("server error ({status}): {message}")]
    Server { status: u16, message: String },

    #[error("stream error: {message}")]
    Stream { message: String },

    #[error("network error: {message}")]
    Network { message: String },

    #[error("request was cancelled")]
    Cancelled,
}
```

### Code + Metadata Registry

Each variant exposes a stable string code (`domain.reason` format) for JSONL serialization and a `NeoErrorInfo` with metadata for TUI rendering:

```rust
impl AiError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Configuration { .. } => "config.invalid",
            Self::RateLimit { .. } => "provider.rate_limit",
            Self::Auth { .. } => "provider.auth_error",
            Self::ContextOverflow { .. } => "provider.context_overflow",
            Self::Server { .. } => "provider.server_error",
            Self::Stream { .. } => "provider.stream_error",
            Self::Network { .. } => "provider.network_error",
            Self::Cancelled => "request.cancelled",
        }
    }

    pub const fn is_retryable(&self) -> bool {
        match self {
            Self::RateLimit { .. } | Self::Network { .. } | Self::Server { .. } => true,
            Self::Configuration { .. }
            | Self::Auth { .. }
            | Self::ContextOverflow { .. }
            | Self::Stream { .. }
            | Self::Cancelled => false,
        }
    }
}
```

```rust
// crates/neo-ai/src/error_info.rs  (NEW FILE)

pub struct NeoErrorInfo {
    pub title: &'static str,
    pub retryable: bool,
    pub action: Option<&'static str>,
}

pub fn error_info(code: &str) -> NeoErrorInfo {
    match code {
        "config.invalid" =>
            info("Configuration Error", false, Some("Check ~/.neo/config.toml")),
        "provider.rate_limit" =>
            info("Rate Limited", true, Some("Will auto-retry with backoff")),
        "provider.auth_error" =>
            info("Authentication Failed", false, Some("Check API key")),
        "provider.context_overflow" =>
            info("Context Overflow", false, Some("Run /compact")),
        "provider.server_error" =>
            info("Server Error", true, Some("Will auto-retry")),
        "provider.network_error" =>
            info("Network Error", true, Some("Check connection")),
        "provider.stream_error" =>
            info("Stream Error", false, None),
        "request.cancelled" =>
            info("Cancelled", false, None),
        // session.*, agent.*, tool.*, goal.*, mcp.*, skill.* codes added incrementally
        _ => info("Error", false, None),
    }
}

const fn info(title: &'static str, retryable: bool, action: Option<&'static str>) -> NeoErrorInfo {
    NeoErrorInfo { title, retryable, action }
}
```

### ProviderError → AiError Mapping

`ProviderError::HttpStatus` gains a `retry_after` field. The `into_ai_error()` method now branches by status code instead of collapsing everything to `AiError::Stream`:

```rust
pub(crate) enum ProviderError {
    HttpStatus {
        status: u16,
        body: Option<String>,
        retry_after: Option<Duration>,  // NEW
    },
    // ... other variants unchanged
}

impl ProviderError {
    pub(crate) fn into_ai_error(self) -> AiError {
        match self {
            Self::HttpStatus { status, body, .. } => {
                let excerpt = sanitize_error_body(body.as_deref());
                match status {
                    429 => AiError::RateLimit { message: excerpt, retry_after: None },
                    401 | 403 => AiError::Auth { message: excerpt },
                    400 | 413 | 422 if is_context_overflow(&excerpt) =>
                        AiError::ContextOverflow { message: excerpt },
                    s if s >= 500 => AiError::Server { status, message: excerpt },
                    _ => AiError::Stream {
                        message: format!("http status {status}: {excerpt}")
                    },
                }
            }
            Self::Transport(err) => AiError::Network { message: err.to_string() },
            Self::Header(msg) | Self::Stream(msg) | Self::Url(msg) | Self::Unsupported(msg) =>
                AiError::Stream { message: msg },
        }
    }
}
```

### HTML Error Body Sanitization

Ported from kimi-code's `sanitizeStatusErrorMessage`. When provider gateways (nginx, Cloudflare) return HTML error pages, extract the `<title>` tag as the human-readable message:

```rust
fn sanitize_error_body(body: Option<&str>) -> String {
    let raw = body.unwrap_or("").trim();
    if raw.starts_with('<') {
        if let Some(start) = raw.find("<title>") {
            let title_start = start + 7;
            if let Some(end) = raw[title_start..].find("</title>") {
                let title = raw[title_start..title_start + end].trim();
                if !title.is_empty() {
                    return title.replace('\r', "");
                }
            }
        }
    }
    // Strip CR, truncate to 4096 chars
    error_body_excerpt(&raw.replace('\r', ""))
}
```

### Context Overflow Detection

Ported from kimi-code's `CONTEXT_OVERFLOW_MESSAGE_PATTERNS`. Matches error messages from various providers to detect context-length issues regardless of HTTP status:

```rust
fn is_context_overflow(message: &str) -> bool {
    let lower = message.to_lowercase();
    const PATTERNS: &[&str] = &[
        "context_length",
        "context window",
        "maximum context",
        "exceed max tokens",
        "too many tokens",
        "prompt is too long",
        "token count exceeds",
        "token limit",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}
```

## Retry Backoff + Retry-After

### Upgraded `open_response()`

The shared retry loop gains exponential backoff with jitter, `Retry-After` header respect, and cancellation support:

```rust
const RETRY_MIN_MS: u64 = 300;
const RETRY_MAX_MS: u64 = 5_000;
const RETRY_FACTOR: f64 = 2.0;
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

pub(crate) async fn open_response<'a>(
    request: &'a ChatRequest,
    cancel_token: &CancellationToken,
    once: impl Fn(&'a ChatRequest) -> BoxFuture<'a, Result<reqwest::Response, ProviderError>>,
) -> Result<reqwest::Response, AiError> {
    let max_attempts = request.options.retries
        .map(|r| r.saturating_add(1))
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);

    let mut last_error = None;
    for attempt in 0..max_attempts {
        match once(request).await {
            Ok(response) => return Ok(response),
            Err(err) if attempt + 1 < max_attempts && err.is_retryable() => {
                let delay = compute_backoff_delay(&err, attempt);
                last_error = Some(err);
                if sleep_cancellable(delay, cancel_token).await.is_err() {
                    return Err(AiError::Cancelled);
                }
            }
            Err(err) => return Err(err.into_ai_error()),
        }
    }
    Err(last_error.map_or_else(
        || AiError::Stream("provider request failed without an error".into()),
        ProviderError::into_ai_error,
    ))
}
```

### Backoff Delay Calculation

```rust
fn compute_backoff_delay(err: &ProviderError, attempt: u32) -> Duration {
    // 1. Retry-After header takes precedence
    if let ProviderError::HttpStatus { retry_after: Some(ra), .. } = err {
        return (*ra).min(Duration::from_millis(RETRY_MAX_MS));
    }
    // 2. Exponential backoff: 300 * 2^attempt, capped at 5s
    let base = RETRY_MIN_MS as f64;
    let exp = RETRY_FACTOR.powi(attempt as i32);
    let raw = (base * exp).min(RETRY_MAX_MS as f64);
    // 3. Jitter: ±25%
    let jitter = rand::thread_rng().gen_range(0.75..1.25);
    Duration::from_millis((raw * jitter) as u64)
}
```

### Retry-After Header Parsing

Each provider wire client's error construction path parses the `Retry-After` response header:

```rust
fn parse_retry_after(value: &str) -> Option<Duration> {
    // HTTP spec: delta-seconds (integer)
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // HTTP spec: HTTP-date
    if let Ok(date) = httpdate::parse_http_date(value) {
        return date.duration_since(SystemTime::now()).ok();
    }
    None
}
```

Applied in all four provider clients (`anthropic.rs`, `openai/responses.rs`, `openai/compatible.rs`, `google.rs`) where `reqwest::Response` error status is captured.

### Cancellable Sleep

```rust
async fn sleep_cancellable(duration: Duration, token: &CancellationToken) -> Result<(), ()> {
    tokio::select! {
        _ = tokio::time::sleep(duration) => Ok(()),
        _ = token.cancelled() => Err(()),
    }
}
```

### Default Retries Changed

`RequestOptions::default()` changes from `retries: None` (= 0 retries) to `retries: Some(2)` (= 3 total attempts), aligning with kimi-code's `DEFAULT_MAX_RETRY_ATTEMPTS = 3`.

## Model Fallback

When a provider fails permanently after all retries, neo automatically switches to a configured secondary model. This is an increment beyond kimi-code (which has no model fallback).

### Configuration

```toml
# ~/.neo/config.toml — global fallback chain
[runtime]
fallback_models = ["claude-sonnet-4-5", "gpt-4.1"]
```

Per-model override:

```toml
[models.claude-opus-4]
fallback_models = ["claude-sonnet-4-5"]
```

### Fallback Decision Point

Model switching happens in the turn loop (not in `http.rs`), because switching models requires rebuilding the `ChatRequest` with a different `ModelSpec`:

```rust
// crates/neo-agent-core/src/runtime/turn_loop.rs

async fn run_model_turn_with_fallback(
    context: &AgentContext,
    config: &AgentConfig,
    emitter: &EventEmitter,
    cancel_token: &CancellationToken,
) -> Result<ModelTurnResult, AgentRuntimeError> {
    let mut models_to_try = vec![config.model.clone()];
    models_to_try.extend(config.fallback_models.iter().cloned());

    let mut last_error = None;
    for (idx, model) in models_to_try.iter().enumerate() {
        match run_model_turn(context, model, cancel_token).await {
            Ok(result) => return Ok(result),
            Err(e) if should_fallback(&e) && idx + 1 < models_to_try.len() => {
                emitter.send(AgentEvent::ModelFallback {
                    turn: emitter.context.turns,
                    from: models_to_try[idx].id.clone(),
                    to: models_to_try[idx + 1].id.clone(),
                    reason: e.code().to_owned(),
                });
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_error.unwrap())
}
```

### Fallback Conditions

Only provider-level failures trigger fallback. Context overflow and cancellation do not:

```rust
fn should_fallback(err: &AgentRuntimeError) -> bool {
    match err {
        AgentRuntimeError::Model(ai) => matches!(
            ai,
            AiError::RateLimit { .. } | AiError::Auth { .. }
                | AiError::Server { .. } | AiError::Network { .. }
        ),
        _ => false,
    }
}
```

- `ContextOverflow` → does NOT fallback (should trigger compaction instead)
- `Cancelled` → does NOT fallback
- `Stream` errors → do NOT fallback (likely a provider protocol issue, not a provider outage)

### New Event

```rust
// events.rs
AgentEvent::ModelFallback {
    turn: u32,
    from: String,
    to: String,
    reason: String,
},
```

## TUI Error Rendering

### AgentEvent::Error Extension

```rust
Error {
    turn: u32,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_after: Option<u64>,  // seconds
},
```

`#[serde(default)]` ensures backward compatibility with existing JSONL sessions.

### Rendering Logic

```rust
// transcript/event_handler.rs
AgentEvent::Error { message, code, retry_after } => {
    let info = code.as_deref()
        .map(neo_ai::error_info)
        .unwrap_or(NeoErrorInfo {
            title: "Error",
            retryable: false,
            action: None,
        });

    let severity = match code.as_deref() {
        Some("provider.rate_limit") | Some("provider.server_error")
        | Some("provider.network_error") => StatusSeverity::Warning,
        _ => StatusSeverity::Error,
    };

    let text = match (code.as_deref(), retry_after) {
        (Some("provider.rate_limit"), Some(secs)) =>
            format!("⚠ {} — retry in {}s", info.title, secs),
        (Some(_), _) if info.action.is_some() =>
            format!("✗ {} — {}", info.title, info.action.unwrap()),
        _ => format!("Error: {message}"),
    };

    transcript.push_status_with_severity(text, severity);
}
```

Rate-limit errors render as `Warning` (yellow) rather than `Error` (red), since they auto-retry.

## Migration Impact

### `crates/neo-ai/src/`

| File | Change |
|---|---|
| `error.rs` | `AiError` from 4 string variants → 8 typed variants; add `code()`, `is_retryable()` |
| `error_info.rs` | **New file**: `NeoErrorInfo` + `error_info()` registry |
| `providers/common/error.rs` | `ProviderError::HttpStatus` add `retry_after`; `into_ai_error()` branches by status; add `sanitize_error_body()`, `is_context_overflow()` |
| `providers/common/http.rs` | `open_response()` gains backoff + jitter + cancellable sleep; signature adds `&CancellationToken` |
| `providers/anthropic.rs` | Parse `Retry-After` header on error response |
| `providers/openai/responses.rs` | Same |
| `providers/openai/compatible.rs` | Same |
| `providers/google.rs` | Same |
| `lib.rs` | Export `error_info` module |

### `crates/neo-agent-core/src/`

| File | Change |
|---|---|
| `events.rs` | `AgentEvent::Error` add `code`, `retry_after`; add `ModelFallback` variant |
| `runtime/error.rs` | `AgentRuntimeError` add `code()` passthrough |
| `runtime/turn_loop.rs` | Wrap `run_model_turn` in `run_model_turn_with_fallback` |
| `runtime/config.rs` | `AgentConfig` add `fallback_models: Vec<ModelSpec>` |
| `runtime/stream_aggregator.rs` | Preserve `code` when mapping `AiStreamEvent::Error` |

### `crates/neo-tui/src/`

| File | Change |
|---|---|
| `transcript/event_handler.rs` | `Error` event renders by code/severity |
| `transcript/entry/render_status.rs` | `Warning` severity style (yellow for rate-limit etc.) |

### `crates/neo-agent/src/`

| File | Change |
|---|---|
| `config/mod.rs` | `RuntimeConfig` add `fallback_models: Vec<String>` |
| `config/types.rs` | `FileRuntimeConfig` add corresponding field |
| `modes/interactive/turn.rs` | Error event rendering path adaptation |
| `modes/run/output/json.rs` | JSON output include `code` field |

## Testing Strategy

### Error type tests

- Each variant's `code()` return value
- `is_retryable()` for each variant
- `sanitize_error_body()`: HTML `<title>` extraction, `\r` stripping, empty-title fallback
- `is_context_overflow()`: pattern matching across provider error message formats
- `parse_retry_after()`: integer-seconds and HTTP-date formats

### HTTP retry tests

- Backoff delay sequence (300→600→1200ms with factor 2)
- Jitter range (±25% of base)
- `Retry-After` header takes precedence over computed backoff
- Cancellable sleep interrupted by `CancellationToken`
- Non-retryable errors (4xx except 429) don't retry
- Default `retries = 2` → 3 total attempts

### Model fallback tests

- `RateLimit` after exhausting retries → fallback to next model
- `Auth` error → fallback to next model
- `ContextOverflow` → does NOT trigger fallback
- `Cancelled` → does NOT trigger fallback
- `ModelFallback` event emitted with correct `from`/`to`/`reason`
- No fallback models configured → error propagates normally

### Serialization tests

- `AgentEvent::Error` with `code` and `retry_after` serializes/deserializes
- Old format (no `code`) deserializes with `code: None` (backward compatible)

## Dependencies

- `rand` = "0.9" (already in workspace)
- `httpdate` crate for HTTP-date parsing (or manual parsing if dependency undesirable)
- `tokio-util` `CancellationToken` (already used)
