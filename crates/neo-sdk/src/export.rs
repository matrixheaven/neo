use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, html};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportConversation {
    pub title: String,
    pub messages: Vec<ExportMessage>,
}

impl ExportConversation {
    #[must_use]
    pub fn new(title: impl Into<String>, messages: Vec<ExportMessage>) -> Self {
        Self {
            title: title.into(),
            messages,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExportMessage {
    pub role: String,
    pub content: String,
}

impl ExportMessage {
    #[must_use]
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlExportOptions {
    pub include_default_css: bool,
}

impl Default for HtmlExportOptions {
    fn default() -> Self {
        Self {
            include_default_css: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("message role {0:?} is not a safe CSS class token")]
    UnsafeRole(String),
}

pub fn export_html(
    conversation: &ExportConversation,
    options: &HtmlExportOptions,
) -> Result<String, ExportError> {
    let mut output =
        String::from("<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    output.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    output.push_str("<title>");
    output.push_str(&escape_html_text(&conversation.title));
    output.push_str("</title>\n");
    if options.include_default_css {
        output.push_str(DEFAULT_CSS);
    }
    output.push_str("</head>\n<body>\n<main class=\"conversation\">\n<h1>");
    output.push_str(&escape_html_text(&conversation.title));
    output.push_str("</h1>\n");

    for message in &conversation.messages {
        if !is_safe_role(&message.role) {
            return Err(ExportError::UnsafeRole(message.role.clone()));
        }
        output.push_str("<article class=\"message message-");
        output.push_str(&message.role);
        output.push_str("\">\n<header>");
        output.push_str(&escape_html_text(&message.role));
        output.push_str("</header>\n<div class=\"message-body\">");
        output.push_str(&render_safe_markdown(&message.content));
        output.push_str("</div>\n</article>\n");
    }

    output.push_str("</main>\n</body>\n</html>\n");
    Ok(output)
}

const DEFAULT_CSS: &str = r#"<style>
:root { color-scheme: light dark; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }
body { margin: 0; background: Canvas; color: CanvasText; }
.conversation { max-width: 860px; margin: 0 auto; padding: 32px 20px; }
.message { border-top: 1px solid color-mix(in srgb, CanvasText 18%, transparent); padding: 18px 0; }
.message header { font-size: 0.82rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0; opacity: 0.72; }
.message-body { line-height: 1.6; }
pre, code { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
pre { overflow-x: auto; padding: 12px; background: color-mix(in srgb, CanvasText 8%, transparent); }
</style>
"#;

fn render_safe_markdown(markdown: &str) -> String {
    let normalized_markdown = strip_ascii_controls(markdown);
    let parser = Parser::new_ext(
        &normalized_markdown,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    );
    let mut unsafe_link_depth = 0usize;
    let safe_events = parser.filter_map(|event| match event {
        Event::Html(html) | Event::InlineHtml(html) => Some(Event::Html(
            escape_html_text(&sanitize_text_markdown_links(&html)).into(),
        )),
        Event::Text(text) => Some(Event::Html(
            escape_html_text(&sanitize_text_markdown_links(&text)).into(),
        )),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            if let Some(dest_url) = sanitize_markdown_url(&dest_url) {
                Some(Event::Start(Tag::Link {
                    link_type,
                    dest_url: dest_url.into(),
                    title,
                    id,
                }))
            } else {
                unsafe_link_depth += 1;
                None
            }
        }
        Event::End(TagEnd::Link) if unsafe_link_depth > 0 => {
            unsafe_link_depth -= 1;
            None
        }
        Event::Start(Tag::Image {
            dest_url,
            title,
            id,
            ..
        }) => {
            let alt = format!("image: {dest_url} {title} {id}");
            Some(Event::Text(alt.into()))
        }
        Event::End(TagEnd::Image) => Some(Event::Text(String::new().into())),
        other => Some(other),
    });
    let mut html_output = String::new();
    html::push_html(&mut html_output, safe_events);
    html_output
}

fn is_safe_role(role: &str) -> bool {
    !role.is_empty()
        && role
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn escape_html_text(input: &str) -> String {
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

fn sanitize_markdown_url(value: &str) -> Option<String> {
    let href = strip_ascii_controls(value.trim());
    if href.is_empty() {
        return Some(href);
    }

    if let Some(colon_index) = href.find(':') {
        let scheme = strip_percent_encoded_controls(&href[..colon_index]);
        let has_scheme = scheme
            .chars()
            .next()
            .is_some_and(|char| char.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|char| char.is_ascii_alphanumeric() || matches!(char, '+' | '.' | '-'));
        if !has_scheme
            || !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto" | "tel" | "ftp"
            )
        {
            return None;
        }
    }

    Some(href)
}

fn strip_ascii_controls(value: &str) -> String {
    value
        .chars()
        .filter(|char| !char.is_ascii_control() && *char != '\u{7f}')
        .collect()
}

fn sanitize_text_markdown_links(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open_bracket) = rest.find('[') {
        let Some(close_bracket_offset) = rest[open_bracket + 1..].find("](") else {
            break;
        };
        let close_bracket = open_bracket + 1 + close_bracket_offset;
        let dest_start = close_bracket + 2;
        let Some(close_paren_offset) = rest[dest_start..].find(')') else {
            break;
        };
        let close_paren = dest_start + close_paren_offset;
        let label = &rest[open_bracket + 1..close_bracket];
        let destination = &rest[dest_start..close_paren];

        output.push_str(&rest[..open_bracket]);
        if sanitize_markdown_url(destination).is_some() {
            output.push_str(&rest[open_bracket..=close_paren]);
        } else {
            output.push_str(label);
        }
        rest = &rest[close_paren + 1..];
    }

    output.push_str(rest);
    output
}

fn strip_percent_encoded_controls(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            let decoded = high * 16 + low;
            if decoded <= 0x1f || decoded == 0x7f {
                index += 3;
                continue;
            }
        }
        output.push(bytes[index] as char);
        index += 1;
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
