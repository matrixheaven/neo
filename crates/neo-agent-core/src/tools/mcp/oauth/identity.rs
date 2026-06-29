use sha2::{Digest, Sha256};

use super::McpOAuthError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthTransportKind {
    Http,
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpOAuthIdentity {
    pub server_id: String,
    pub canonical_resource_url: String,
    pub store_key: String,
    pub transport_kind: McpOAuthTransportKind,
}

impl McpOAuthIdentity {
    pub fn new(
        server_id: impl Into<String>,
        raw_url: impl AsRef<str>,
        transport_kind: McpOAuthTransportKind,
    ) -> Result<Self, McpOAuthError> {
        let server_id = server_id.into();
        let mut url = reqwest::Url::parse(raw_url.as_ref())
            .map_err(|err| McpOAuthError::InvalidIdentity(err.to_string()))?;

        match url.scheme() {
            "http" | "https" => {}
            scheme => {
                return Err(McpOAuthError::InvalidIdentity(format!(
                    "unsupported URL scheme `{scheme}`"
                )));
            }
        }

        url.set_fragment(None);
        let canonical_resource_url = url.to_string();
        let store_key = store_key(&server_id, &canonical_resource_url);

        Ok(Self {
            server_id,
            canonical_resource_url,
            store_key,
            transport_kind,
        })
    }
}

fn store_key(server_id: &str, canonical_resource_url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(server_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(canonical_resource_url.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");

    format!("{}-{}", sanitize_server_id(server_id), &hex[..24])
}

fn sanitize_server_id(server_id: &str) -> String {
    let sanitized: String = server_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "server".to_owned()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_url_removes_fragment_and_keeps_query() {
        let identity = McpOAuthIdentity::new(
            "linear",
            "https://mcp.example.com/sse?workspace=neo#ignored",
            McpOAuthTransportKind::Sse,
        )
        .unwrap();

        assert_eq!(
            identity.canonical_resource_url,
            "https://mcp.example.com/sse?workspace=neo"
        );
    }

    #[test]
    fn store_key_changes_when_url_changes() {
        let first = McpOAuthIdentity::new(
            "linear",
            "https://mcp.example.com/sse?workspace=neo",
            McpOAuthTransportKind::Http,
        )
        .unwrap();
        let second = McpOAuthIdentity::new(
            "linear",
            "https://mcp.example.com/sse?workspace=other",
            McpOAuthTransportKind::Http,
        )
        .unwrap();

        assert_ne!(first.store_key, second.store_key);
    }

    #[test]
    fn store_key_sanitizes_empty_server_id() {
        let identity = McpOAuthIdentity::new(
            "",
            "https://mcp.example.com/sse",
            McpOAuthTransportKind::Sse,
        )
        .unwrap();

        assert!(identity.store_key.starts_with("server-"));
    }

    #[test]
    fn rejects_non_http_urls() {
        let result =
            McpOAuthIdentity::new("linear", "file:///tmp/mcp", McpOAuthTransportKind::Http);

        assert!(result.is_err());
    }
}
