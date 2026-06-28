pub mod anthropic;
pub mod fake;
pub mod google;
pub mod openai_compatible;
pub mod openai_images;
pub mod openai_responses;

mod common;

use crate::types::ContentPart;

/// Extract text from content parts, joining with newlines.
/// `include_thinking` controls whether reasoning text is included.
pub(crate) fn collect_text_content(content: &[ContentPart], include_thinking: bool) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::Thinking { text, .. } if include_thinking => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
