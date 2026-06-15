use crate::core::{Finalization, Line};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(String),
    User(String),
    Assistant {
        thinking: Option<String>,
        content: String,
        finalized: bool,
    },
    ToolCallRunning {
        name: String,
        detail: String,
    },
    ToolCallFinished {
        name: String,
        detail: String,
    },
    Notice(String),
}

impl TranscriptEntry {
    #[must_use]
    pub fn banner(title: impl Into<String>) -> Self {
        Self::Banner(title.into())
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::User(content.into())
    }

    #[must_use]
    pub fn assistant_live(content: impl Into<String>) -> Self {
        Self::Assistant {
            thinking: None,
            content: content.into(),
            finalized: false,
        }
    }

    #[must_use]
    pub fn assistant_final(content: impl Into<String>) -> Self {
        Self::Assistant {
            thinking: None,
            content: content.into(),
            finalized: true,
        }
    }

    #[must_use]
    pub fn tool_call_running(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallRunning {
            name: name.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn tool_call_finished(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::ToolCallFinished {
            name: name.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn notice(content: impl Into<String>) -> Self {
        Self::Notice(content.into())
    }

    #[must_use]
    pub fn finalization(&self) -> Finalization {
        match self {
            Self::Banner(_) | Self::User(_) | Self::Notice(_) | Self::ToolCallFinished { .. } => {
                Finalization::Finalized
            }
            Self::Assistant { finalized, .. } if *finalized => Finalization::Finalized,
            Self::Assistant { .. } | Self::ToolCallRunning { .. } => Finalization::Live,
        }
    }

    #[must_use]
    pub fn render(&self, _width: usize) -> Vec<Line> {
        match self {
            Self::Banner(title) => {
                vec![Line::raw(title.clone())]
            }
            Self::User(content) => {
                let mut rows = Vec::new();
                rows.push(Line::raw("You"));
                rows.push(Line::raw(content.clone()));
                rows
            }
            Self::Notice(content) => {
                vec![Line::raw(content.clone())]
            }
            Self::Assistant {
                thinking,
                content,
                finalized,
            } => {
                let mut rows = Vec::new();
                if *finalized
                    && content.is_empty()
                    && thinking.as_ref().map_or(true, |t| t.is_empty())
                {
                    return rows;
                }
                if let Some(thinking) = thinking.as_ref().filter(|value| !value.is_empty()) {
                    rows.push(Line::raw(format!("● {thinking}")));
                }
                if !content.is_empty() {
                    if *finalized {
                        rows.push(Line::raw("Assistant"));
                    }
                    rows.push(Line::raw(content.clone()));
                }
                rows
            }
            Self::ToolCallRunning { name, detail } => {
                vec![Line::raw(format!("● Using {name} ({detail})"))]
            }
            Self::ToolCallFinished { name, detail } => {
                vec![Line::raw(format!("✓ Used {name} ({detail})"))]
            }
        }
    }
}
