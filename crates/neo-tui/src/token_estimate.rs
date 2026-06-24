//! Lightweight token-count estimation for UI progress indicators.
//!
//! This is intentionally approximate: it is used to show the user that the
//! model is still producing output, not to bill or enforce context limits.

use unicode_segmentation::UnicodeSegmentation;

/// Estimate the number of tokens in `text`.
///
/// Heuristic: count Unicode words, then add one token for every four
/// non-word characters. This gives a reasonable order-of-magnitude estimate
/// for both English/code and CJK text without pulling in a full tokenizer.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let words = text.unicode_words().count();
    let non_word_chars = text.chars().count().saturating_sub(words);
    words + non_word_chars.div_ceil(4)
}

/// Format a token count for display, mirroring the compact style used in the
/// chrome context badge.
#[must_use]
pub fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{}m", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// Format an elapsed duration in seconds as `Xs` or `Xm Xs`.
#[must_use]
pub fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_code_is_reasonable() {
        let code = "pub fn main() { println!(\"hello\"); }";
        let tokens = estimate_tokens(code);
        assert!(tokens > 0 && tokens < code.len());
    }

    #[test]
    fn format_token_count_uses_compact_units() {
        assert_eq!(format_token_count(42), "42");
        assert_eq!(format_token_count(1_200), "1k");
        assert_eq!(format_token_count(1_200_000), "1m");
    }

    #[test]
    fn format_elapsed_seconds_and_minutes() {
        assert_eq!(format_elapsed(5), "5s");
        assert_eq!(format_elapsed(65), "1m 5s");
    }
}
