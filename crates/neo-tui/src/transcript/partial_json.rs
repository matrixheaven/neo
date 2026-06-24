//! Best-effort extraction of string fields from a partial JSON object.
//!
//! Model providers stream tool-call arguments as raw JSON fragments; this module
//! provides a lightweight scanner that can pull out a string value before the
//! object is complete. It is intentionally forgiving: if extraction fails, the
//! caller simply falls back to not showing a preview.

/// Try to extract the value of a string field from a partial JSON object.
///
/// The scanner looks for `"<field>":` followed by a JSON string literal and
/// returns the raw contents, handling simple escaped quotes and backslashes.
/// It stops at the first closing quote that is not escaped.
#[must_use]
pub fn extract_partial_string_field(partial_json: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\":");
    let start = partial_json.find(&key)? + key.len();
    let rest = &partial_json[start..];
    let rest = rest.trim_start();
    let mut chars = rest.chars();
    if chars.next() != Some('"') {
        return None;
    }

    let mut value = String::new();
    let mut escaped = false;
    for ch in chars {
        if escaped {
            value.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(value),
            _ => value.push(ch),
        }
    }
    // String not closed yet: return what we have so far.
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_complete_string() {
        let partial = r#"{"path":"src/foo.rs","content":"hello"}"#;
        assert_eq!(
            extract_partial_string_field(partial, "content"),
            Some("hello".to_owned())
        );
    }

    #[test]
    fn extracts_partial_string() {
        let partial = r#"{"path":"src/foo.rs","content":"hello world"#;
        assert_eq!(
            extract_partial_string_field(partial, "content"),
            Some("hello world".to_owned())
        );
    }

    #[test]
    fn handles_escaped_quotes() {
        let partial = r#"{"content":"say \"hi\""}"#;
        assert_eq!(
            extract_partial_string_field(partial, "content"),
            Some(r#"say "hi""#.to_owned())
        );
    }
}
