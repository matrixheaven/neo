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
    let mut chars = chars.peekable();
    while let Some(ch) = chars.next() {
        if escaped {
            push_json_escape(&mut value, ch, &mut chars);
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

fn push_json_escape(
    value: &mut String,
    escaped: char,
    chars: &mut std::iter::Peekable<impl Iterator<Item = char>>,
) {
    match escaped {
        '"' => value.push('"'),
        '\\' => value.push('\\'),
        '/' => value.push('/'),
        'b' => value.push('\u{0008}'),
        'f' => value.push('\u{000c}'),
        'n' => value.push('\n'),
        'r' => value.push('\r'),
        't' => value.push('\t'),
        'u' => push_unicode_escape(value, chars),
        other => value.push(other),
    }
}

fn push_unicode_escape(
    value: &mut String,
    chars: &mut std::iter::Peekable<impl Iterator<Item = char>>,
) {
    let mut hex = String::new();
    for _ in 0..4 {
        let Some(ch) = chars.next() else {
            value.push_str("\\u");
            value.push_str(&hex);
            return;
        };
        if !ch.is_ascii_hexdigit() {
            value.push_str("\\u");
            value.push_str(&hex);
            value.push(ch);
            return;
        }
        hex.push(ch);
    }
    let Some(codepoint) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) else {
        value.push_str("\\u");
        value.push_str(&hex);
        return;
    };
    value.push(codepoint);
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

    #[test]
    fn decodes_common_json_string_escapes() {
        let partial = r#"{"content":"line 1\n\tline 2\\tail"}"#;
        assert_eq!(
            extract_partial_string_field(partial, "content"),
            Some("line 1\n\tline 2\\tail".to_owned())
        );
    }
}
