use crate::primitive::theme::format_token_count;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextWindow {
    pub used_tokens: Option<u32>,
    pub max_tokens: u32,
}

impl ContextWindow {
    #[must_use]
    pub const fn new(max_tokens: u32) -> Self {
        Self {
            used_tokens: None,
            max_tokens,
        }
    }

    #[must_use]
    pub const fn with_used_tokens(mut self, used_tokens: u32) -> Self {
        self.used_tokens = Some(used_tokens);
        self
    }

    #[must_use]
    pub fn label(self) -> String {
        let used = self
            .used_tokens
            .map_or_else(|| "--".to_owned(), format_token_count);
        format!("ctx {used}/{}", format_token_count(self.max_tokens))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MainAgentTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub input_cache_read_tokens: u64,
    pub input_cache_write_tokens: u64,
}

impl MainAgentTokenUsage {
    pub fn add(&mut self, usage: neo_agent_core::AgentTokenUsage) {
        self.input_tokens = self
            .input_tokens
            .saturating_add(u64::from(usage.input_tokens));
        self.output_tokens = self
            .output_tokens
            .saturating_add(u64::from(usage.output_tokens));
        self.input_cache_read_tokens = self
            .input_cache_read_tokens
            .saturating_add(u64::from(usage.input_cache_read_tokens));
        self.input_cache_write_tokens = self
            .input_cache_write_tokens
            .saturating_add(u64::from(usage.input_cache_write_tokens));
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.input_cache_read_tokens == 0
            && self.input_cache_write_tokens == 0
    }

    #[must_use]
    pub fn label(self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut parts = vec![format!(
            "↑{} ↓{}",
            format_usage_token_count(self.input_tokens),
            format_usage_token_count(self.output_tokens)
        )];
        if let Some(cache) =
            format_cache_usage(self.input_cache_read_tokens, self.input_cache_write_tokens)
        {
            parts.push(cache);
        }
        Some(parts.join(" · "))
    }
}

#[must_use]
fn format_cache_usage(read: u64, write: u64) -> Option<String> {
    match (read, write) {
        (0, 0) => None,
        (read, 0) => Some(format!("cache {} read", format_usage_token_count(read))),
        (0, write) => Some(format!("cache {} write", format_usage_token_count(write))),
        (read, write) => Some(format!(
            "cache {} read / {} write",
            format_usage_token_count(read),
            format_usage_token_count(write)
        )),
    }
}

#[must_use]
fn format_usage_token_count(tokens: u64) -> String {
    if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}
