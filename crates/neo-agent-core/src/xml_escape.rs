//! Crate-private XML escaping for model-facing envelopes.
//!
//! Text nodes and attribute values share the same base escapes (`&`, `<`, `>`).
//! Attribute values also escape `"` so quoted attributes stay well-formed.

/// Escape text that appears between XML tags (not inside attributes).
#[must_use]
pub(crate) fn escape_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escape a value that will be placed inside a double-quoted XML attribute.
#[must_use]
pub(crate) fn escape_attribute(value: &str) -> String {
    escape_text(value).replace('"', "&quot;")
}
