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
pub(crate) fn token_usage_from(
    value: &Value,
    input_key: &str,
    output_key: &str,
) -> Option<TokenUsage> {
    token_count(value.get(input_key))?;
    token_count(value.get(output_key))?;
    merge_token_usage(None, value, input_key, output_key)
}

pub(crate) fn merge_token_usage(
    current: Option<TokenUsage>,
    value: &Value,
    input_key: &str,
    output_key: &str,
) -> Option<TokenUsage> {
    let input_tokens = token_count(value.get(input_key));
    let output_tokens = token_count(value.get(output_key));
    if input_tokens.is_none() && output_tokens.is_none() {
        return current;
    }

    let mut usage = current.unwrap_or(TokenUsage {
        input_tokens: 0,
        output_tokens: 0,
        input_cache_read_tokens: 0,
        input_cache_write_tokens: 0,
    });
    if let Some(input_tokens) = input_tokens {
        usage.input_tokens = input_tokens;
        usage.input_cache_read_tokens = token_count_from_any(
            value,
            &[
                "cache_read_input_tokens",
                "input_cache_read",
                "input_cache_read_tokens",
                "cache_read_tokens",
            ],
            &[
                ("input_tokens_details", "cached_tokens"),
                ("input_tokens_details", "cache_read_tokens"),
                ("prompt_tokens_details", "cached_tokens"),
                ("prompt_tokens_details", "cache_read_tokens"),
            ],
        );
        usage.input_cache_write_tokens = token_count_from_any(
            value,
            &[
                "cache_creation_input_tokens",
                "input_cache_creation",
                "input_cache_write_tokens",
                "cache_write_tokens",
            ],
            &[
                ("input_tokens_details", "cache_creation_tokens"),
                ("prompt_tokens_details", "cache_creation_tokens"),
            ],
        );
    }
    if let Some(output_tokens) = output_tokens {
        usage.output_tokens = output_tokens;
    }
    Some(usage)
}

fn token_count_from_any(value: &Value, direct_keys: &[&str], nested_keys: &[(&str, &str)]) -> u32 {
    direct_keys
        .iter()
        .find_map(|key| token_count(value.get(*key)))
        .or_else(|| {
            nested_keys.iter().find_map(|(parent, key)| {
                value
                    .get(*parent)
                    .and_then(|details| token_count(details.get(*key)))
            })
        })
        .unwrap_or(0)
}

fn token_count(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()
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
