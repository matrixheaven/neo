use crate::core::{Finalization, Line};
use crate::wrap_width;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEntry {
    Banner(String),
    User(String),
    Assistant {
        thinking: String,
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
            thinking: String::new(),
            content: content.into(),
            finalized: false,
        }
    }

    #[must_use]
    pub fn assistant_final(content: impl Into<String>) -> Self {
        Self::Assistant {
            thinking: String::new(),
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
    pub fn render(&self, width: usize) -> Vec<Line> {
        // Every `Line` returned here MUST map to exactly one terminal row:
        // content is split on `\n` and soft-wrapped to `width` so no line ever
        // carries an embedded newline. The renderer's diff/scroll math treats
        // each `Vec<String>` entry as one screen row, so an un-split long line
        // would corrupt the coordinate model and garble streaming output.
        let inner_width = width.max(1);
        match self {
            Self::Banner(title) => wrap_width(title, inner_width)
                .into_iter()
                .map(Line::raw)
                .collect(),
            Self::User(content) => {
                let mut rows = Vec::new();
                rows.push(Line::raw("You"));
                rows.extend(wrap_width(content, inner_width).into_iter().map(Line::raw));
                rows
            }
            Self::Notice(content) => wrap_width(content, inner_width)
                .into_iter()
                .map(Line::raw)
                .collect(),
            Self::Assistant {
                thinking,
                content,
                finalized,
            } => {
                let mut rows = Vec::new();
                if *finalized && content.is_empty() && thinking.is_empty() {
                    return rows;
                }
                if !thinking.is_empty() {
                    rows.extend(
                        wrap_width(&format!("● {thinking}"), inner_width)
                            .into_iter()
                            .map(Line::raw),
                    );
                }
                if !content.is_empty() {
                    if *finalized {
                        rows.push(Line::raw("Assistant"));
                    }
                    rows.extend(wrap_width(content, inner_width).into_iter().map(Line::raw));
                }
                rows
            }
            Self::ToolCallRunning { name, detail } => {
                wrap_width(&format!("● Using {name} ({detail})"), inner_width)
                    .into_iter()
                    .map(Line::raw)
                    .collect()
            }
            Self::ToolCallFinished { name, detail } => {
                wrap_width(&format!("✓ Used {name} ({detail})"), inner_width)
                    .into_iter()
                    .map(Line::raw)
                    .collect()
            }
        }
    }
}
