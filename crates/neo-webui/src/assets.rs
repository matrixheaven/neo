//! Compile-time embedded frontend assets. The allowlist is byte-for-byte the
//! delivered `web/dist` build artifact: exact path comparisons only, no
//! runtime disk reads, no directory enumeration, no SPA fallback and no path
//! traversal (paths are compared, never joined or resolved).

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

const INDEX_HTML: &[u8] = include_bytes!("../web/dist/index.html");
const APP_JS: &[u8] = include_bytes!("../web/dist/assets/neo-webui.js");
const APP_CSS: &[u8] = include_bytes!("../web/dist/assets/neo-webui.css");

/// One embedded static resource with its fixed MIME type.
pub struct EmbeddedAsset {
    bytes: &'static [u8],
    content_type: &'static str,
}

impl EmbeddedAsset {
    /// The embedded bytes (compile-time constant).
    #[must_use]
    pub fn bytes(&self) -> &'static [u8] {
        self.bytes
    }
}

impl IntoResponse for EmbeddedAsset {
    fn into_response(self) -> Response {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, self.content_type)],
            self.bytes,
        )
            .into_response()
    }
}

/// Exact-path allowlist for anonymous static reads. Every other path is
/// `None` and becomes the stable `404`; there is deliberately no single-page
/// route fallback.
#[must_use]
pub fn asset_for_path(path: &str) -> Option<EmbeddedAsset> {
    match path {
        "/" | "/index.html" => Some(EmbeddedAsset {
            bytes: INDEX_HTML,
            content_type: "text/html; charset=utf-8",
        }),
        "/assets/neo-webui.js" => Some(EmbeddedAsset {
            bytes: APP_JS,
            content_type: "text/javascript; charset=utf-8",
        }),
        "/assets/neo-webui.css" => Some(EmbeddedAsset {
            bytes: APP_CSS,
            content_type: "text/css; charset=utf-8",
        }),
        _ => None,
    }
}
