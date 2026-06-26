use super::theme::format_token_count;

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
