//! Shared utility functions reused across provider wire clients.

use crate::types::{ContentPart, TokenUsage};
use serde_json::Value;

use super::error::ProviderError;

/// Round an `f64` to six decimal places.
pub(crate) fn rounded_f64(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

/// Extract [`TokenUsage`] from a JSON object using provider-specific keys.
///
/// `value` should already point at the inner usage object (e.g. `value.get("usage")`).
pub(crate) fn token_usage_from(value: &Value, input_key: &str, output_key: &str) -> Option<TokenUsage> {
    Some(TokenUsage {
        input_tokens: u32::try_from(value.get(input_key)?.as_u64()?).ok()?,
        output_tokens: u32::try_from(value.get(output_key)?.as_u64()?).ok()?,
    })
}

/// Reject image content parts in non-user messages.
///
/// `provider` is display name inserted into the error message (e.g. `"Anthropic"`).
pub(crate) fn reject_images(
    content: &[ContentPart],
    provider: &str,
    role: &str,
) -> Result<(), ProviderError> {
    if content
        .iter()
        .any(|part| matches!(part, ContentPart::Image { .. }))
    {
        return Err(ProviderError::Unsupported(format!(
            "{provider} image content is only supported in user messages, not {role} messages"
        )));
    }
    Ok(())
}
