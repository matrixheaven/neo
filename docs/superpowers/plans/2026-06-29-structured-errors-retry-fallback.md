# Structured Errors + Retry Backoff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Upgrade neo's error handling from opaque strings to typed variants with stable codes, add exponential backoff with jitter and `Retry-After` header parsing, and make the retry sleep cancellable.

**Architecture:** `AiError` evolves from 4 string variants to 8 typed variants with `code()`/`is_retryable()` methods. `ProviderError::HttpStatus` gains a `retry_after` field. The shared `open_response()` retry loop gains backoff, jitter, and cancellable sleep (reading the `CancellationToken` from `RequestOptions`, not from a new parameter). `AgentEvent::Error` gains `code`/`retry_after` fields. `AgentRuntimeError` gains a `code()` passthrough. TUI renders errors by code/severity.

> **Scope note — model fallback deferred:** Automatic model fallback (switching to a secondary model after the primary fails) requires `ProviderResolver` access inside the turn loop, which is a larger architectural change. It is intentionally **not** included in this plan. This plan delivers error classification, backoff, `Retry-After`, and TUI rendering only. Model fallback will be specified separately.

**Tech Stack:** Rust, thiserror, tokio, tokio-util (CancellationToken), rand 0.9, httpdate 1.0

**Spec:** `docs/superpowers/specs/2026-06-29-structured-errors-retry-fallback-design.md`

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/neo-ai/src/error.rs` | `AiError` enum with typed variants, `code()`, `is_retryable()` |
| `crates/neo-ai/src/error_info.rs` | **NEW** — `NeoErrorInfo` struct + `error_info()` registry |
| `crates/neo-ai/src/providers/common/error.rs` | `ProviderError` with `retry_after`, `into_ai_error()` status-based mapping, `sanitize_error_body()`, `is_context_overflow()`, `parse_retry_after()`; **deletes** `format_http_status_error()` |
| `crates/neo-ai/src/providers/common/http.rs` | `open_response()` with backoff + jitter + cancellable sleep (reads cancel token from `RequestOptions`) |
| `crates/neo-ai/src/options.rs` | `RequestOptions` gains `cancel_token: Option<Arc<CancellationToken>>` |
| `crates/neo-ai/src/lib.rs` | Export `error_info` module |
| `crates/neo-ai/Cargo.toml` | Add `rand`, `httpdate` deps |
| `crates/neo-agent-core/src/events.rs` | `AgentEvent::Error` gains `code`/`retry_after` fields |
| `crates/neo-agent-core/src/runtime/error.rs` | `AgentRuntimeError` gains `code()` passthrough |
| `crates/neo-agent-core/src/runtime/stream_aggregator.rs` | Thread `code: None` into `AgentEvent::Error` from `AiStreamEvent::Error` |
| `crates/neo-agent/src/modes/run/output/json.rs` | Update `AgentEvent::Error` destructuring for new fields |
| `crates/neo-agent/src/modes/btw.rs` | Update `AgentEvent::Error` pattern matches for new fields |
| `crates/neo-tui/src/transcript/event_handler.rs` | Error rendering by code/severity |
| `crates/neo-tui/src/transcript/pane.rs` | `push_status_with_severity()` method |

---

## Task 1: Upgrade `AiError` to typed variants

**Files:**
- Modify: `crates/neo-ai/src/error.rs`
- Test: `crates/neo-ai/src/error.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add at the bottom of `crates/neo-ai/src/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn code_returns_domain_dot_reason() {
        assert_eq!(AiError::Configuration { message: "x".into() }.code(), "config.invalid");
        assert_eq!(AiError::RateLimit { message: "x".into(), retry_after: None }.code(), "provider.rate_limit");
        assert_eq!(AiError::Auth { message: "x".into() }.code(), "provider.auth_error");
        assert_eq!(AiError::ContextOverflow { message: "x".into() }.code(), "provider.context_overflow");
        assert_eq!(AiError::Server { status: 500, message: "x".into() }.code(), "provider.server_error");
        assert_eq!(AiError::Stream { message: "x".into() }.code(), "provider.stream_error");
        assert_eq!(AiError::Network { message: "x".into() }.code(), "provider.network_error");
        assert_eq!(AiError::Cancelled.code(), "request.cancelled");
    }

    #[test]
    fn is_retryable_for_each_variant() {
        assert!(AiError::RateLimit { message: "".into(), retry_after: Some(Duration::from_secs(5)) }.is_retryable());
        assert!(AiError::Network { message: "".into() }.is_retryable());
        assert!(AiError::Server { status: 503, message: "".into() }.is_retryable());
        assert!(!AiError::Configuration { message: "".into() }.is_retryable());
        assert!(!AiError::Auth { message: "".into() }.is_retryable());
        assert!(!AiError::ContextOverflow { message: "".into() }.is_retryable());
        assert!(!AiError::Stream { message: "".into() }.is_retryable());
        assert!(!AiError::Cancelled.is_retryable());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-ai error`
Expected: FAIL — `AiError` variants don't have named fields, `code()` method doesn't exist.

- [ ] **Step 3: Replace `AiError` enum and add methods**

Replace the entire content of `crates/neo-ai/src/error.rs`:

```rust
use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
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

impl AiError {
    /// Stable string code for JSONL serialization — `domain.reason` format.
    #[must_use]
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

    /// Whether a failed request is worth retrying.
    #[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn code_returns_domain_dot_reason() {
        assert_eq!(AiError::Configuration { message: "x".into() }.code(), "config.invalid");
        assert_eq!(AiError::RateLimit { message: "x".into(), retry_after: None }.code(), "provider.rate_limit");
        assert_eq!(AiError::Auth { message: "x".into() }.code(), "provider.auth_error");
        assert_eq!(AiError::ContextOverflow { message: "x".into() }.code(), "provider.context_overflow");
        assert_eq!(AiError::Server { status: 500, message: "x".into() }.code(), "provider.server_error");
        assert_eq!(AiError::Stream { message: "x".into() }.code(), "provider.stream_error");
        assert_eq!(AiError::Network { message: "x".into() }.code(), "provider.network_error");
        assert_eq!(AiError::Cancelled.code(), "request.cancelled");
    }

    #[test]
    fn is_retryable_for_each_variant() {
        assert!(AiError::RateLimit { message: "".into(), retry_after: Some(Duration::from_secs(5)) }.is_retryable());
        assert!(AiError::Network { message: "".into() }.is_retryable());
        assert!(AiError::Server { status: 503, message: "".into() }.is_retryable());
        assert!(!AiError::Configuration { message: "".into() }.is_retryable());
        assert!(!AiError::Auth { message: "".into() }.is_retryable());
        assert!(!AiError::ContextOverflow { message: "".into() }.is_retryable());
        assert!(!AiError::Stream { message: "".into() }.is_retryable());
        assert!(!AiError::Cancelled.is_retryable());
    }
}
```

- [ ] **Step 4: Fix all compilation errors across the workspace**

The variant change from `Configuration(String)` to `Configuration { message: String }` etc. will break every site that constructs or matches `AiError`. Run the build and fix each error:

Run: `cargo build -p neo-ai 2>&1 | head -50`

Common fixes needed:
- `AiError::Configuration(msg)` → `AiError::Configuration { message: msg }`
- `AiError::Stream(msg)` → `AiError::Stream { message: msg }`
- `AiError::Network(msg)` → `AiError::Network { message: msg }`
- Pattern matches `AiError::Stream(msg) =>` → `AiError::Stream { message: msg } =>`

Search for all construction/match sites:
```bash
grep -rn 'AiError::' crates/neo-ai/src/ crates/neo-agent-core/src/ crates/neo-agent/src/ crates/neo-tui/src/ --include='*.rs' | grep -v 'test' | grep -v '#\[error'
```

Fix every match arm and construction site to use named fields.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-ai`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-ai/src/error.rs
git add -u  # pick up all the pattern fix sites
git commit -m "refactor(neo-ai): upgrade AiError to typed variants with code() and is_retryable()"
```

---

## Task 2: Add `NeoErrorInfo` registry

**Files:**
- Create: `crates/neo-ai/src/error_info.rs`
- Modify: `crates/neo-ai/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/neo-ai/src/error_info.rs` with tests first:

```rust
//! Metadata registry for error codes — provides title, retryability hint,
//! and user-facing remediation action for each [`crate::AiError::code`].

/// Metadata for a specific error code.
#[derive(Debug, Clone)]
pub struct NeoErrorInfo {
    /// Short human-readable title (e.g. "Rate Limited").
    pub title: &'static str,
    /// Whether this error category is retryable.
    pub retryable: bool,
    /// Optional user-facing remediation hint (e.g. "Check API key").
    pub action: Option<&'static str>,
}

impl NeoErrorInfo {
    /// Fallback info for unknown error codes.
    #[must_use]
    pub const fn fallback() -> Self {
        Self {
            title: "Error",
            retryable: false,
            action: None,
        }
    }
}

/// Look up metadata for a stable error code string.
///
/// See [`crate::AiError::code`] for the code format (`domain.reason`).
#[must_use]
pub fn error_info(code: &str) -> NeoErrorInfo {
    match code {
        "config.invalid" => info("Configuration Error", false, Some("Check ~/.neo/config.toml")),
        "provider.rate_limit" => info("Rate Limited", true, Some("Will auto-retry with backoff")),
        "provider.auth_error" => info("Authentication Failed", false, Some("Check API key")),
        "provider.context_overflow" => info("Context Overflow", false, Some("Run /compact")),
        "provider.server_error" => info("Server Error", true, Some("Will auto-retry")),
        "provider.network_error" => info("Network Error", true, Some("Check connection")),
        "provider.stream_error" => info("Stream Error", false, None),
        "request.cancelled" => info("Cancelled", false, None),
        _ => NeoErrorInfo::fallback(),
    }
}

const fn info(
    title: &'static str,
    retryable: bool,
    action: Option<&'static str>,
) -> NeoErrorInfo {
    NeoErrorInfo {
        title,
        retryable,
        action,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_return_specific_info() {
        let info = error_info("provider.rate_limit");
        assert_eq!(info.title, "Rate Limited");
        assert!(info.retryable);
        assert_eq!(info.action, Some("Will auto-retry with backoff"));
    }

    #[test]
    fn unknown_code_returns_fallback() {
        let info = error_info("something.unknown");
        assert_eq!(info.title, "Error");
        assert!(!info.retryable);
        assert_eq!(info.action, None);
    }

    #[test]
    fn context_overflow_has_compact_action() {
        let info = error_info("provider.context_overflow");
        assert_eq!(info.action, Some("Run /compact"));
    }
}
```

- [ ] **Step 2: Register the module in `lib.rs`**

In `crates/neo-ai/src/lib.rs`, add after `pub mod error;` (line 4):

```rust
pub mod error_info;
```

And at the bottom with other re-exports (after `pub use error::AiError;`):

```rust
pub use error_info::{NeoErrorInfo, error_info};
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-ai error_info`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/neo-ai/src/error_info.rs crates/neo-ai/src/lib.rs
git commit -m "feat(neo-ai): add NeoErrorInfo registry for error code metadata"
```

---

## Task 3: Upgrade `ProviderError` with `retry_after` + status-based mapping

**Files:**
- Modify: `crates/neo-ai/src/providers/common/error.rs`
- Modify: `crates/neo-ai/src/providers/anthropic.rs`
- Modify: `crates/neo-ai/src/providers/openai/responses.rs`
- Modify: `crates/neo-ai/src/providers/openai/compatible.rs`
- Modify: `crates/neo-ai/src/providers/google.rs`
- Modify: `crates/neo-ai/Cargo.toml`

- [ ] **Step 1: Write the failing tests**

Add a test module at the bottom of `crates/neo-ai/src/providers/common/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn http_status_429_maps_to_rate_limit() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: Some("Too Many Requests".into()),
            retry_after: Some(Duration::from_secs(30)),
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.rate_limit");
    }

    #[test]
    fn http_status_401_maps_to_auth() {
        let err = ProviderError::HttpStatus {
            status: 401,
            body: Some("Unauthorized".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.auth_error");
    }

    #[test]
    fn http_status_503_maps_to_server() {
        let err = ProviderError::HttpStatus {
            status: 503,
            body: Some("Service Unavailable".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.server_error");
    }

    #[test]
    fn http_status_413_with_context_overflow_maps_to_context_overflow() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Request too large: context_length exceeded".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.context_overflow");
    }

    #[test]
    fn http_status_413_without_context_pattern_maps_to_stream() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Payload Too Large".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.stream_error");
    }

    #[test]
    fn sanitize_extracts_title_from_html() {
        // Body starts with "413 " not "<", so starts_with('<') would miss this.
        // contains("<title>") detects HTML anywhere in the body.
        let html = "413 <html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n</html>\r\n";
        let result = sanitize_error_body(Some(html));
        assert_eq!(result, "413 Request Entity Too Large");
    }

    #[test]
    fn sanitize_strips_carriage_returns() {
        let result = sanitize_error_body(Some("line1\r\nline2\r"));
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn sanitize_empty_title_falls_back_to_body() {
        let html = "<html><head><title>  </title></head><body>nginx</body></html>";
        let result = sanitize_error_body(Some(html));
        assert!(result.contains("nginx"));
    }

    #[test]
    fn sanitize_plain_text_unchanged() {
        let result = sanitize_error_body(Some("just text"));
        assert_eq!(result, "just text");
    }

    #[test]
    fn sanitize_none_body_returns_empty() {
        let result = sanitize_error_body(None);
        assert_eq!(result, "");
    }

    #[test]
    fn is_context_overflow_detects_patterns() {
        assert!(is_context_overflow("Request exceeds context_length limit"));
        assert!(is_context_overflow("prompt is too long for maximum context"));
        assert!(!is_context_overflow("Payload Too Large"));
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_invalid_returns_none() {
        assert_eq!(parse_retry_after("not a number"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-ai tests`
Expected: FAIL — `ProviderError::HttpStatus` doesn't have `retry_after` field; `sanitize_error_body`, `is_context_overflow`, and `parse_retry_after` don't exist.

- [ ] **Step 3: Add `httpdate` dependency**

In `crates/neo-ai/Cargo.toml`, add `httpdate` directly (it is not in workspace deps):

```toml
[dependencies]
async-trait.workspace = true
futures.workspace = true
httpdate = "1.0"
reqwest.workspace = true
schemars.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio-util.workspace = true
```

- [ ] **Step 4: Implement the upgraded `ProviderError`**

Replace the entire content of `crates/neo-ai/src/providers/common/error.rs`:

```rust
//! Unified error type shared across provider wire clients.

use std::time::Duration;

use crate::error::AiError;

/// Maximum number of characters retained from an HTTP error response body.
const MAX_HTTP_ERROR_BODY_CHARS: usize = 4096;

/// Truncate `body` to [`MAX_HTTP_ERROR_BODY_CHARS`] characters, appending `...`
/// if truncation occurred. Leading/trailing whitespace is trimmed first.
pub(crate) fn error_body_excerpt(body: &str) -> String {
    let trimmed = body.trim();
    let mut chars = trimmed.chars();
    let excerpt = chars
        .by_ref()
        .take(MAX_HTTP_ERROR_BODY_CHARS)
        .collect::<String>();
    if chars.next().is_some() {
        format!("{excerpt}...")
    } else {
        excerpt
    }
}

/// Sanitize an HTTP error response body into a human-readable message.
///
/// If the body contains a `<title>` tag (common for nginx/proxy error pages),
/// extract its text. Carriage returns are stripped. The result is truncated
/// to 4096 chars via [`error_body_excerpt`].
///
/// Note: detection uses `contains("<title>")` rather than `starts_with('<')`
/// because some bodies are prefixed with a status code (e.g. `"413 <html>..."`).
pub(crate) fn sanitize_error_body(body: Option<&str>) -> String {
    let raw = body.unwrap_or("").trim();
    if raw.contains("<title>") {
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
    error_body_excerpt(&raw.replace('\r', ""))
}

/// Detect whether an error message indicates a context-length issue.
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

/// Parse an HTTP `Retry-After` header value into a `Duration`.
///
/// Supports both delta-seconds (integer) and HTTP-date formats per RFC 7231.
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    // Try integer seconds first (most common)
    if let Ok(secs) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    // Try HTTP-date format
    if let Ok(date) = httpdate::parse_http_date(value.trim()) {
        return date.duration_since(std::time::SystemTime::now()).ok();
    }
    None
}

/// Unified error type for all provider wire clients.
///
/// Variant set is the union of the four legacy private `ProviderError` enums.
/// `HttpStatus` carries an optional response body excerpt and an optional
/// `Retry-After` duration parsed from the response headers.
#[derive(Debug)]
pub(crate) enum ProviderError {
    Header(String),
    HttpStatus {
        status: u16,
        body: Option<String>,
        retry_after: Option<Duration>,
    },
    Transport(reqwest::Error),
    Stream(String),
    Url(String),
    Unsupported(String),
}

impl ProviderError {
    /// Whether a failed request is worth retrying.
    pub(crate) const fn is_retryable(&self) -> bool {
        match self {
            Self::HttpStatus { status, .. } => *status == 429 || *status >= 500,
            Self::Transport(_) => true,
            Self::Header(_) | Self::Stream(_) | Self::Url(_) | Self::Unsupported(_) => false,
        }
    }

    /// Convert into the public [`AiError`] type, branching by HTTP status.
    pub(crate) fn into_ai_error(self) -> AiError {
        match self {
            Self::HttpStatus {
                status,
                body,
                retry_after,
            } => {
                let excerpt = sanitize_error_body(body.as_deref());
                match status {
                    429 => AiError::RateLimit {
                        message: excerpt,
                        retry_after,
                    },
                    401 | 403 => AiError::Auth { message: excerpt },
                    400 | 413 | 422 if is_context_overflow(&excerpt) => {
                        AiError::ContextOverflow { message: excerpt }
                    }
                    s if s >= 500 => AiError::Server {
                        status,
                        message: excerpt,
                    },
                    _ => AiError::Stream {
                        message: format!("http status {status}: {excerpt}"),
                    },
                }
            }
            Self::Transport(err) => AiError::Network {
                message: format!("transport error: {err}"),
            },
            Self::Header(message)
            | Self::Stream(message)
            | Self::Url(message)
            | Self::Unsupported(message) => AiError::Stream { message },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn http_status_429_maps_to_rate_limit() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: Some("Too Many Requests".into()),
            retry_after: Some(Duration::from_secs(30)),
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.rate_limit");
    }

    #[test]
    fn http_status_401_maps_to_auth() {
        let err = ProviderError::HttpStatus {
            status: 401,
            body: Some("Unauthorized".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.auth_error");
    }

    #[test]
    fn http_status_503_maps_to_server() {
        let err = ProviderError::HttpStatus {
            status: 503,
            body: Some("Service Unavailable".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.server_error");
    }

    #[test]
    fn http_status_413_with_context_overflow_maps_to_context_overflow() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Request too large: context_length exceeded".into()),
            retry_after: None,
        };
        assert_eq!(err.into_ai_error().code(), "provider.context_overflow");
    }

    #[test]
    fn http_status_413_without_context_pattern_maps_to_stream() {
        let err = ProviderError::HttpStatus {
            status: 413,
            body: Some("Payload Too Large".into()),
            retry_after: None,
        };
        let ai = err.into_ai_error();
        assert_eq!(ai.code(), "provider.stream_error");
    }

    #[test]
    fn sanitize_extracts_title_from_html() {
        // Body starts with "413 " not "<", so starts_with('<') would miss this.
        // contains("<title>") detects HTML anywhere in the body.
        let html = "413 <html>\r\n<head><title>413 Request Entity Too Large</title></head>\r\n</html>\r\n";
        let result = sanitize_error_body(Some(html));
        assert_eq!(result, "413 Request Entity Too Large");
    }

    #[test]
    fn sanitize_strips_carriage_returns() {
        let result = sanitize_error_body(Some("line1\r\nline2\r"));
        assert_eq!(result, "line1\nline2");
    }

    #[test]
    fn sanitize_empty_title_falls_back_to_body() {
        let html = "<html><head><title>  </title></head><body>nginx</body></html>";
        let result = sanitize_error_body(Some(html));
        assert!(result.contains("nginx"));
    }

    #[test]
    fn sanitize_plain_text_unchanged() {
        let result = sanitize_error_body(Some("just text"));
        assert_eq!(result, "just text");
    }

    #[test]
    fn sanitize_none_body_returns_empty() {
        let result = sanitize_error_body(None);
        assert_eq!(result, "");
    }

    #[test]
    fn is_context_overflow_detects_patterns() {
        assert!(is_context_overflow("Request exceeds context_length limit"));
        assert!(is_context_overflow("prompt is too long for maximum context"));
        assert!(!is_context_overflow("Payload Too Large"));
    }

    #[test]
    fn parse_retry_after_seconds() {
        assert_eq!(parse_retry_after("30"), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("  5  "), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_invalid_returns_none() {
        assert_eq!(parse_retry_after("not a number"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
```

> **What was deleted:** The old `format_http_status_error()` function (lines 24–32 of the original file) is removed entirely. It was only used by the old `into_ai_error()` which produced opaque `AiError::Stream(format_http_status_error(status, &body))` strings. The new `into_ai_error()` branches by status code and uses `sanitize_error_body()` instead.

- [ ] **Step 5: Fix all `ProviderError::HttpStatus` construction sites**

The `HttpStatus` variant now has a `retry_after` field. There are 4 construction sites — one per provider wire client. Each currently looks like:

```rust
return Err(ProviderError::HttpStatus {
    status,
    body: Some(error_body_excerpt(&body)),
});
```

At each site, parse the `Retry-After` header before constructing the error. The `parse_retry_after` function is in `common::error`, so update the import.

**`crates/neo-ai/src/providers/anthropic.rs`** — in `open_response_once`, around line 55–64, replace:

```rust
        let response = builder.send().await.map_err(ProviderError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let status = status.as_u16();
            let body = response
                .text()
                .await
                .unwrap_or_else(|err| format!("failed to read error body: {err}"));
            return Err(ProviderError::HttpStatus {
                status,
                body: Some(error_body_excerpt(&body)),
            });
        }
```

with:

```rust
        let response = builder.send().await.map_err(ProviderError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let status = status.as_u16();
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(super::common::error::parse_retry_after);
            let body = response
                .text()
                .await
                .unwrap_or_else(|err| format!("failed to read error body: {err}"));
            return Err(ProviderError::HttpStatus {
                status,
                body: Some(error_body_excerpt(&body)),
                retry_after,
            });
        }
```

Apply the same change to **`crates/neo-ai/src/providers/openai/responses.rs`**, **`crates/neo-ai/src/providers/openai/compatible.rs`**, and **`crates/neo-ai/src/providers/google.rs`**. At each site:
1. Parse `retry_after` from `response.headers().get("retry-after")` **before** calling `response.text()` (headers are consumed by `.text()`).
2. Add `retry_after` to the `ProviderError::HttpStatus { ... }` struct literal.

> **Import note:** `parse_retry_after` lives in `common::error`. Each provider file already imports from `common::error` (e.g. `use super::common::error::{ProviderError, error_body_excerpt};`). Add `parse_retry_after` to that import list. For `google.rs` which uses `super::common::error::...`, adjust accordingly.

- [ ] **Step 6: Run tests and fix compilation**

Run: `cargo run -p xtask -- test -p neo-ai`
Expected: PASS — all provider compilation fixed and tests green.

- [ ] **Step 7: Commit**

```bash
git add crates/neo-ai/src/providers/common/error.rs crates/neo-ai/src/providers/ crates/neo-ai/Cargo.toml
git commit -m "feat(neo-ai): upgrade ProviderError with retry_after, status-based mapping, HTML sanitize"
```

---

## Task 4: Add backoff + jitter + cancellable sleep to `open_response`

**Files:**
- Modify: `crates/neo-ai/Cargo.toml`
- Modify: `crates/neo-ai/src/options.rs`
- Modify: `crates/neo-ai/src/providers/common/http.rs`

- [ ] **Step 1: Add `rand` and `tokio` dependencies to `neo-ai`**

In `crates/neo-ai/Cargo.toml`:

```toml
[dependencies]
async-trait.workspace = true
futures.workspace = true
httpdate = "1.0"
rand.workspace = true
reqwest.workspace = true
schemars.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tokio.workspace = true
tokio-util.workspace = true

[dev-dependencies]
tokio.workspace = true
```

(`rand` is in the workspace at version 0.9; `tokio` is needed for `tokio::time::sleep` and `tokio::select!`.)

- [ ] **Step 2: Add `cancel_token` to `RequestOptions` (and remove `PartialEq`)**

In `crates/neo-ai/src/options.rs`, first remove `PartialEq` from the derive list (line 73). `CancellationToken` does not implement `PartialEq`, and a grep confirms `RequestOptions` is never compared with `==` anywhere in the codebase:

```bash
grep -rn 'RequestOptions' crates/ --include='*.rs' | grep '=='
# Expected: no matches
```

Change the derive from:
```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
```
to:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
```

Then add a field to the `RequestOptions` struct (after `metadata`):

```rust
    /// Cancellation token for the HTTP retry loop's backoff sleep.
    /// Set by the runtime so retries abort promptly on user cancellation.
    #[serde(skip)]
    #[schemars(skip)]
    pub cancel_token: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
```

In `Default for RequestOptions`, add:

```rust
            cancel_token: None,
```

The complete updated `RequestOptions` and its `Default`:

```rust
// NOTE: `PartialEq` removed — `CancellationToken` does not implement it,
// and `RequestOptions` is never compared for equality in the codebase.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestOptions {
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub headers: BTreeMap<String, String>,
    #[schemars(skip)]
    pub timeout: Option<Duration>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub replay_reasoning: bool,
    pub retries: Option<u32>,
    pub cache: CacheRetention,
    pub session_id: Option<String>,
    pub metadata: RequestMetadata,
    #[serde(skip)]
    #[schemars(skip)]
    pub cancel_token: Option<std::sync::Arc<tokio_util::sync::CancellationToken>>,
}

impl Default for RequestOptions {
    fn default() -> Self {
        Self {
            temperature: None,
            max_tokens: None,
            headers: BTreeMap::new(),
            timeout: None,
            reasoning_effort: None,
            replay_reasoning: true,
            retries: Some(0),
            cache: CacheRetention::Short,
            session_id: None,
            metadata: RequestMetadata::default(),
            cancel_token: None,
        }
    }
}
```

> **Design note:** The `ModelClient::stream_chat` trait signature (`fn stream_chat(&self, request: ChatRequest)`) is **not changed**. The cancel token is threaded via `RequestOptions`, which is already part of `ChatRequest`. The runtime sets `request.options.cancel_token` before calling `stream_chat`, and the shared `open_response` reads it from there.

- [ ] **Step 3: Write the failing tests**

Add tests at the bottom of `crates/neo-ai/src/providers/common/http.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_grows_exponentially() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d0 = compute_backoff_delay(&err, 0).as_millis();
        let d2 = compute_backoff_delay(&err, 2).as_millis();
        // d0 should be roughly 300ms (225–375ms with ±25% jitter)
        assert!(d0 >= 200 && d0 <= 400, "d0={d0}");
        // d2 should be roughly 1200ms (900–1500ms with ±25% jitter)
        assert!(d2 >= 800 && d2 <= 1600, "d2={d2}");
    }

    #[test]
    fn backoff_delay_capped_at_max() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d = compute_backoff_delay(&err, 10).as_millis();
        // Even at attempt 10, should be capped at 5s (±25%: 3750–6250ms)
        assert!(d <= 6500, "d={d}");
    }

    #[test]
    fn retry_after_takes_precedence() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: Some(std::time::Duration::from_secs(10)),
        };
        let d = compute_backoff_delay(&err, 0);
        // Retry-After of 10s should be capped at RETRY_MAX_MS (5s)
        assert_eq!(d, std::time::Duration::from_millis(5_000));
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-ai backoff`
Expected: FAIL — `compute_backoff_delay` doesn't exist.

- [ ] **Step 5: Implement the upgraded `open_response`**

Replace the entire content of `crates/neo-ai/src/providers/common/http.rs`:

```rust
//! Shared HTTP helpers: retry loop with exponential backoff, and header injection.
//!
//! These are byte-for-byte identical across the four provider wire clients
//! (`openai::responses`, `anthropic`, `openai::compatible`, `google`), so they
//! live here to avoid duplication.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::future::BoxFuture;
use rand::Rng;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use super::error::ProviderError;
use crate::{AiError, ChatRequest};

/// Minimum backoff delay (first retry attempt).
const RETRY_MIN_MS: u64 = 300;
/// Maximum backoff delay cap.
const RETRY_MAX_MS: u64 = 5_000;
/// Exponential growth factor.
const RETRY_FACTOR: f64 = 2.0;
/// Default total attempts when `request.options.retries` is unset.
const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Compute the backoff delay for a given attempt number.
///
/// 1. If the error carries a `Retry-After` value, use it (capped at `RETRY_MAX_MS`).
/// 2. Otherwise use exponential backoff: `RETRY_MIN_MS * 2^attempt`, capped at `RETRY_MAX_MS`.
/// 3. Apply ±25% jitter to the computed value.
fn compute_backoff_delay(err: &ProviderError, attempt: u32) -> Duration {
    // 1. Retry-After header takes precedence (no jitter — server told us to wait)
    if let ProviderError::HttpStatus {
        retry_after: Some(ra),
        ..
    } = err
    {
        return (*ra).min(Duration::from_millis(RETRY_MAX_MS));
    }
    // 2. Exponential backoff
    let base = RETRY_MIN_MS as f64;
    let exp = RETRY_FACTOR.powi(attempt as i32);
    let raw = (base * exp).min(RETRY_MAX_MS as f64);
    // 3. Jitter: ±25% (rand 0.9 API: rand::rng())
    let jitter = rand::rng().gen_range(0.75..1.25);
    Duration::from_millis((raw * jitter) as u64)
}

/// Sleep for `duration`, but abort early if `cancel_token` is cancelled.
async fn sleep_cancellable(duration: Duration, token: &CancellationToken) -> Result<(), ()> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(()),
        _ = tokio::time::sleep(duration) => Ok(()),
    }
}

/// Retry loop shared by all provider wire clients.
///
/// Calls `once` up to `request.options.retries + 1` times (or
/// `DEFAULT_MAX_ATTEMPTS` if unset), retrying on
/// [`ProviderError::is_retryable`] errors with exponential backoff + jitter.
///
/// The backoff sleep is cancellable: if `request.options.cancel_token` is set
/// and the token is cancelled during a backoff sleep, the loop aborts early
/// with [`AiError::Cancelled`].
pub(crate) async fn open_response<'a>(
    request: &'a ChatRequest,
    once: impl Fn(&'a ChatRequest) -> BoxFuture<'a, Result<reqwest::Response, ProviderError>>,
) -> Result<reqwest::Response, AiError> {
    let max_attempts = request
        .options
        .retries
        .map(|r| r.saturating_add(1))
        .unwrap_or(DEFAULT_MAX_ATTEMPTS);

    // Extract cancel token from RequestOptions (set by the runtime).
    // If unset, create a standalone token that never fires.
    let cancel_token = request
        .options
        .cancel_token
        .as_deref()
        .cloned()
        .unwrap_or_default();

    let mut last_error = None;

    for attempt in 0..max_attempts {
        match once(request).await {
            Ok(response) => return Ok(response),
            Err(err) if attempt + 1 < max_attempts && err.is_retryable() => {
                let delay = compute_backoff_delay(&err, attempt);
                last_error = Some(err);
                if sleep_cancellable(delay, &cancel_token).await.is_err() {
                    return Err(AiError::Cancelled);
                }
            }
            Err(err) => return Err(err.into_ai_error()),
        }
    }

    Err(last_error.map_or_else(
        || AiError::Stream {
            message: "provider request failed without an error".to_owned(),
        },
        ProviderError::into_ai_error,
    ))
}

/// Insert each `(name, value)` pair from `extra` into `headers`.
///
/// Returns [`ProviderError::Header`] on an invalid header name or value.
pub(crate) fn inject_extra_headers(
    headers: &mut HeaderMap,
    extra: &BTreeMap<String, String>,
) -> Result<(), ProviderError> {
    for (name, value) in extra {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| ProviderError::Header(format!("invalid header name {name}: {err}")))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| ProviderError::Header(format!("invalid header value {name}: {err}")))?;
        headers.insert(name, value);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_delay_grows_exponentially() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d0 = compute_backoff_delay(&err, 0).as_millis();
        let d2 = compute_backoff_delay(&err, 2).as_millis();
        assert!(d0 >= 200 && d0 <= 400, "d0={d0}");
        assert!(d2 >= 800 && d2 <= 1600, "d2={d2}");
    }

    #[test]
    fn backoff_delay_capped_at_max() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: None,
        };
        let d = compute_backoff_delay(&err, 10).as_millis();
        assert!(d <= 6500, "d={d}");
    }

    #[test]
    fn retry_after_takes_precedence() {
        let err = ProviderError::HttpStatus {
            status: 429,
            body: None,
            retry_after: Some(std::time::Duration::from_secs(10)),
        };
        let d = compute_backoff_delay(&err, 0);
        assert_eq!(d, std::time::Duration::from_millis(5_000));
    }
}
```

> **Key changes from old plan:**
> - `rand::thread_rng()` → `rand::rng()` (rand 0.9 API).
> - `open_response` signature is **unchanged** — it reads the cancel token from `request.options.cancel_token` instead of taking a separate parameter. No call sites need updating.
> - `parse_retry_after` lives in `error.rs` (Task 3), not `http.rs`. The `http.rs` module does not import it.

- [ ] **Step 6: Run tests and fix compilation**

Run: `cargo run -p xtask -- test -p neo-ai`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/neo-ai/Cargo.toml crates/neo-ai/src/options.rs crates/neo-ai/src/providers/common/http.rs
git commit -m "feat(neo-ai): add exponential backoff with jitter and cancellable sleep to retry loop"
```

---

## Task 5: Upgrade `AgentEvent::Error` with `code` + `retry_after`

**Files:**
- Modify: `crates/neo-agent-core/src/events.rs`
- Modify: `crates/neo-agent-core/src/runtime/stream_aggregator.rs`
- Modify: `crates/neo-agent/src/modes/run/output/json.rs`
- Modify: `crates/neo-agent/src/modes/btw.rs`
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`

- [ ] **Step 1: Write the failing test**

Add to the existing test module in `crates/neo-agent-core/src/events.rs`:

```rust
    #[test]
    fn error_with_code_serializes() {
        let event = AgentEvent::Error {
            turn: 1,
            message: "rate limited".into(),
            code: Some("provider.rate_limit".into()),
            retry_after: Some(30),
        };
        let json = serde_json::to_string(&event).expect("serialize");
        assert!(json.contains("\"code\":\"provider.rate_limit\""));
        assert!(json.contains("\"retry_after\":30"));
        let back: AgentEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(event, back);
    }

    #[test]
    fn error_without_code_backward_compatible() {
        // Old JSONL format without code/retry_after
        let json = r#"{"Error":{"turn":1,"message":"old format"}}"#;
        let event: AgentEvent = serde_json::from_str(json).expect("deserialize");
        match event {
            AgentEvent::Error { turn, message, code, retry_after } => {
                assert_eq!(turn, 1);
                assert_eq!(message, "old format");
                assert_eq!(code, None);
                assert_eq!(retry_after, None);
            }
            _ => panic!("expected Error variant"),
        }
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core error_with_code`
Expected: FAIL — `AgentEvent::Error` doesn't have `code` or `retry_after` fields.

- [ ] **Step 3: Add fields to `AgentEvent::Error`**

In `crates/neo-agent-core/src/events.rs`, replace the `Error` variant (around line 238):

```rust
    Error {
        turn: u32,
        message: String,
        /// Stable error code (e.g. `"provider.rate_limit"`). `None` for old sessions.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Retry-After hint in seconds, if the provider included one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after: Option<u64>,
    },
```

- [ ] **Step 4: Update `stream_aggregator.rs` construction site**

In `crates/neo-agent-core/src/runtime/stream_aggregator.rs`, line 100–106, the `AiStreamEvent::Error` handler currently emits `AgentEvent::Error` with only `turn` and `message`. Update it to include the new fields:

```rust
            AiStreamEvent::Error { message } => {
                emitter.emit(AgentEvent::Error {
                    turn,
                    message: message.clone(),
                    code: None,
                    retry_after: None,
                });
                self.finish_current_message(turn, StopReason::Error, emitter);
            }
```

> **Note:** `AiStreamEvent::Error` currently carries only `{ message: String }`. Threading the structured `code`/`retry_after` through the stream event is a deeper change that would require upgrading every provider's SSE parser. For now, `None` is correct — the TUI falls back to the message text when `code` is absent. The structured error info reaches the caller via `AgentRuntimeError::Model(AiError)` (see Task 6), not via this event.

- [ ] **Step 5: Fix `json.rs` destructuring (BLOCKER)**

In `crates/neo-agent/src/modes/run/output/json.rs`, line 173, the `AgentEvent::Error` match arm destructures `{ turn, message }`. This will fail to compile after adding `code`/`retry_after`. Update it:

```rust
            AgentEvent::Error { turn, message, .. } => vec![json!({
                "type": "error",
                "turn": turn,
                "message": message,
            })],
```

- [ ] **Step 6: Fix `btw.rs` pattern matches (BLOCKER)**

In `crates/neo-agent/src/modes/btw.rs`, there are two `AgentEvent::Error` pattern matches that must be updated.

**Line 265** — change:
```rust
        Ok(AgentEvent::Error { message, .. }) => {
```
to:
```rust
        Ok(AgentEvent::Error { message, .. }) => {
```
(This already uses `..` so it compiles as-is. Verify.)

**Line 521** — the test constructs `AgentEvent::Error` explicitly. Change:
```rust
            Ok(AgentEvent::Error {
                turn: 1,
                message: "boom".to_owned(),
            })
```
to:
```rust
            Ok(AgentEvent::Error {
                turn: 1,
                message: "boom".to_owned(),
                code: None,
                retry_after: None,
            })
```

- [ ] **Step 7: Fix `event_handler.rs` pattern match**

In `crates/neo-tui/src/transcript/event_handler.rs`, line 214, update the destructuring:

```rust
            AgentEvent::Error { message, .. } => {
                self.push_status(format!("Error: {message}"));
                true
            }
```

(This uses `..` so it compiles. Task 7 will replace this with code-aware rendering.)

- [ ] **Step 8: Search for any remaining construction/match sites**

```bash
grep -rn 'AgentEvent::Error' crates/ --include='*.rs' | grep -v '#\[cfg(test)\]'
```

Fix any remaining sites that explicitly list fields. Use `..` in match arms; add `code: None, retry_after: None` in construction sites.

Also check `crates/neo-agent/src/modes/interactive/` for any `AgentEvent::Error` construction:
```bash
grep -rn 'AgentEvent::Error' crates/neo-agent/src/ --include='*.rs'
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-agent-core error`
Expected: PASS

Run: `cargo build -p neo-agent -p neo-tui`
Expected: PASS (no compilation errors)

- [ ] **Step 10: Commit**

```bash
git add crates/neo-agent-core/src/events.rs crates/neo-agent-core/src/runtime/stream_aggregator.rs
git add -u
git commit -m "feat(neo-agent-core): add code and retry_after to AgentEvent::Error"
```

---

## Task 6: Add `AgentRuntimeError::code()` passthrough

**Files:**
- Modify: `crates/neo-agent-core/src/runtime/error.rs`
- Test: `crates/neo-agent-core/src/runtime/error.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/neo-agent-core/src/runtime/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_error_code_delegates_to_ai_error() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::RateLimit {
            message: "too many requests".into(),
            retry_after: None,
        });
        assert_eq!(err.code(), Some("provider.rate_limit"));
    }

    #[test]
    fn model_error_code_for_network() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::Network {
            message: "timeout".into(),
        });
        assert_eq!(err.code(), Some("provider.network_error"));
    }

    #[test]
    fn non_model_error_returns_none() {
        assert_eq!(AgentRuntimeError::Cancelled.code(), None);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-agent-core model_error_code`
Expected: FAIL — `code()` method doesn't exist on `AgentRuntimeError`.

- [ ] **Step 3: Implement `code()` on `AgentRuntimeError`**

Add an `impl` block to `crates/neo-agent-core/src/runtime/error.rs`:

```rust
use thiserror::Error;

use crate::{ToolError, compaction};

#[derive(Debug, Error)]
pub enum AgentRuntimeError {
    #[error("model stream failed: {0}")]
    Model(#[from] neo_ai::AiError),
    #[error("tool execution failed: {0}")]
    Tool(#[from] ToolError),
    #[error("runtime I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("compaction failed: {0}")]
    Compaction(#[from] compaction::CompactionError),
    #[error("turn cancelled")]
    Cancelled,
}

impl AgentRuntimeError {
    /// Return the stable error code if this is a model-level error.
    ///
    /// Delegates to [`neo_ai::AiError::code`] for the [`Self::Model`] variant.
    /// Returns `None` for all other variants (tool errors, I/O, compaction,
    /// cancellation) which don't have provider-level codes.
    #[must_use]
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::Model(ai) => Some(ai.code()),
            Self::Tool(_)
            | Self::Io(_)
            | Self::Compaction(_)
            | Self::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_error_code_delegates_to_ai_error() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::RateLimit {
            message: "too many requests".into(),
            retry_after: None,
        });
        assert_eq!(err.code(), Some("provider.rate_limit"));
    }

    #[test]
    fn model_error_code_for_network() {
        let err = AgentRuntimeError::Model(neo_ai::AiError::Network {
            message: "timeout".into(),
        });
        assert_eq!(err.code(), Some("provider.network_error"));
    }

    #[test]
    fn non_model_error_returns_none() {
        assert_eq!(AgentRuntimeError::Cancelled.code(), None);
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-agent-core model_error_code`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/neo-agent-core/src/runtime/error.rs
git commit -m "feat(neo-agent-core): add AgentRuntimeError::code() passthrough to AiError"
```

---

## Task 7: TUI error rendering by code/severity

**Files:**
- Modify: `crates/neo-agent-core/src/lib.rs` (re-export `error_info`)
- Modify: `crates/neo-tui/src/transcript/pane.rs` (add `push_status_with_severity`)
- Modify: `crates/neo-tui/src/transcript/event_handler.rs`

- [ ] **Step 1: Re-export `error_info` from `neo-agent-core`**

The TUI crate depends on `neo-agent-core`, not `neo-ai` directly. Re-export `error_info` so the TUI can access it.

In `crates/neo-agent-core/src/lib.rs`, find the existing re-export section and add:

```rust
pub use neo_ai::error_info;
pub use neo_ai::NeoErrorInfo;
```

Verify the exact location by checking existing re-exports:
```bash
grep -n 'pub use neo_ai' crates/neo-agent-core/src/lib.rs
```

Add the re-exports alongside the existing `pub use neo_ai::...` lines.

- [ ] **Step 2: Add `push_status_with_severity` to `TranscriptPane`**

In `crates/neo-tui/src/transcript/pane.rs`, the existing `push_status` method (around line 187) creates a `TranscriptEntry::Status` with `severity: None`. Add a variant that accepts an explicit severity.

The `TranscriptEntry::Status` variant is:
```rust
Status {
    text: String,
    severity: Option<StatusSeverity>,
}
```

Add this method immediately after the existing `push_status`:

```rust
    /// Push a status entry with explicit severity.
    pub fn push_status_with_severity(
        &mut self,
        content: impl Into<String>,
        severity: crate::transcript::entry::StatusSeverity,
    ) {
        self.push_transcript(TranscriptEntry::Status {
            text: content.into(),
            severity: Some(severity),
        });
    }
```

> **Important:** The `TranscriptEntry::Status` variant uses named fields `{ text, severity }`, not a struct with a `kind` field. The constructor must match exactly: `TranscriptEntry::Status { text: ..., severity: Some(...) }`.

You also need to import `StatusSeverity` at the top of `pane.rs` or use the full path. Check existing imports:
```bash
grep -n 'use.*StatusSeverity\|use.*TranscriptEntry' crates/neo-tui/src/transcript/pane.rs
```

If `TranscriptEntry` is already imported (it is, via `use super::TranscriptEntry` or similar), add `StatusSeverity` to the import:

```rust
use super::entry::StatusSeverity;
```

Or use the full path `crate::transcript::entry::StatusSeverity` inline as shown above.

- [ ] **Step 3: Write the rendering upgrade**

In `crates/neo-tui/src/transcript/event_handler.rs`, replace the `AgentEvent::Error` handler (around line 214):

Current:
```rust
            AgentEvent::Error { message, .. } => {
                self.push_status(format!("Error: {message}"));
                true
            }
```

Replace with:
```rust
            AgentEvent::Error {
                message,
                code,
                retry_after,
                ..
            } => {
                use crate::transcript::entry::StatusSeverity;

                let severity = match code.as_deref() {
                    Some("provider.rate_limit")
                    | Some("provider.server_error")
                    | Some("provider.network_error") => StatusSeverity::Warning,
                    _ => StatusSeverity::Error,
                };

                let text = match (code.as_deref(), retry_after) {
                    (Some("provider.rate_limit"), Some(secs)) => {
                        format!("⚠ Rate Limited — retry in {secs}s")
                    }
                    (Some(c), _) => {
                        let info = neo_agent_core::error_info(c);
                        if let Some(action) = info.action {
                            format!("✗ {} — {}", info.title, action)
                        } else {
                            format!("Error: {message}")
                        }
                    }
                    _ => format!("Error: {message}"),
                };

                self.push_status_with_severity(text, severity);
                true
            }
```

> **Function name:** Use `neo_agent_core::error_info(c)` — NOT `error_code_info`. The function was defined as `error_info` in Task 2 and re-exported in Step 1 above.

- [ ] **Step 4: Run the build and fix**

Run: `cargo build -p neo-tui`
Expected: PASS

- [ ] **Step 5: Run focused tests**

Run: `cargo run -p xtask -- test -p neo-tui`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add crates/neo-agent-core/src/lib.rs crates/neo-tui/src/transcript/pane.rs crates/neo-tui/src/transcript/event_handler.rs
git commit -m "feat(neo-tui): render errors by code with severity classification"
```

---

## Task 8: Change default retries to 2

**Files:**
- Modify: `crates/neo-ai/src/options.rs`
- Test: `crates/neo-ai/src/options.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Add at the bottom of `crates/neo-ai/src/options.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_retries_is_two() {
        let opts = RequestOptions::default();
        assert_eq!(opts.retries, Some(2));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo run -p xtask -- test -p neo-ai default_retries`
Expected: FAIL — current default is `Some(0)`, test expects `Some(2)`.

- [ ] **Step 3: Change default from `Some(0)` to `Some(2)`**

In the `Default for RequestOptions` implementation in `crates/neo-ai/src/options.rs`, change:

```rust
            retries: Some(0),
```

to:

```rust
            retries: Some(2),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo run -p xtask -- test -p neo-ai`
Expected: PASS

- [ ] **Step 5: Run workspace tests to check for regressions**

Some tests may assert specific retry counts or rely on the old default. Run a broader check:

Run: `cargo run -p xtask -- test -p neo-ai -p neo-agent-core`
Expected: PASS

If any test fails because it assumed `retries == Some(0)`, update the test to explicitly set `retries: Some(0)` in its `RequestOptions` construction rather than relying on the default.

- [ ] **Step 6: Commit**

```bash
git add crates/neo-ai/src/options.rs
git commit -m "feat(neo-ai): default retries from 0 to 2 (3 total attempts)"
```

---

## Verification Summary

After all 8 tasks are complete, run the full verification gate:

```bash
cargo run -p xtask -- check --workspace
cargo run -p xtask -- test -p neo-ai -p neo-agent-core -p neo-tui -p neo-agent
```

Expected: All checks and tests pass.

### What this plan delivers:
1. **Typed error codes** — `AiError` has 8 variants with stable `code()` strings (`domain.reason` format).
2. **Status-based mapping** — HTTP 429 → `RateLimit`, 401/403 → `Auth`, 5xx → `Server`, context-overflow patterns → `ContextOverflow`, transport → `Network`.
3. **Retry-After parsing** — Both integer-seconds and HTTP-date formats, parsed from response headers.
4. **Exponential backoff + jitter** — 300ms base, 2× growth, 5s cap, ±25% jitter.
5. **Cancellable backoff sleep** — Via `RequestOptions.cancel_token`, no trait signature changes.
6. **Error metadata registry** — `NeoErrorInfo` with title, retryability, and remediation action.
7. **TUI severity rendering** — Rate-limit/server/network errors shown as warnings; others as errors.
8. **Default 2 retries** — 3 total attempts per provider request.

### What this plan does NOT deliver (deferred):
- **Model fallback** — switching to a secondary model after the primary fails. Requires `ProviderResolver` access in the turn loop. Will be specified separately.
- **`AiStreamEvent::Error` code threading** — mid-stream errors still emit `AgentEvent::Error` with `code: None`. Upgrading the SSE parsers to carry structured codes is a deeper change.
