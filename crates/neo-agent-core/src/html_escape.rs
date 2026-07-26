//! Crate-private HTML escaping for browser-facing markup.
//!
//! Use this for HTML documents served to or opened in a browser (session
//! export, OAuth callback pages). Escapes `&`, `<`, `>`, `"`, and `'`
//! (`&#39;`) so text is safe in element bodies and common attribute contexts.
//!
//! This is **not** the same as [`crate::xml_escape`], which is limited to
//! model/skill/shell pseudo-XML envelopes and does not escape quotes the same
//! way.

/// Escape text for inclusion in browser HTML (element content or attributes).
///
/// Matches historical session-export semantics: `&` `<` `>` `"` and `'`
/// (`&#39;`).
#[must_use]
pub(crate) fn escape_text(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for char in input.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(char),
        }
    }
    escaped
}
