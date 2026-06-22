use crate::ansi::{Rect, clip_visible_to_width, display_width, next_sequence};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    chrome::{ApprovalModal, MAX_PROMPT_VISIBLE_LINES, NeoChromeState, OverlayKind, PromptState},
    widgets::{BtwPanel, TodoPanel},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    pub body: Rect,
    pub todo: Rect,
    pub status: Rect,
    pub approval: Rect,
    pub session_picker: Rect,
    pub overlay: Rect,
    pub btw: Rect,
    pub prompt: Rect,
    pub footer: Rect,
}

#[must_use]
pub fn chrome_layout(app: &mut NeoChromeState, area: Rect) -> ChromeLayout {
    let heights = ChromeLayoutHeights::new(app, area);
    let body_height = area.height.saturating_sub(heights.bottom_height());
    let body = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: body_height,
    };
    let mut stack = RectStack::new(area, body.y.saturating_add(body.height));
    let todo = stack.next(heights.todo);
    let status = stack.next(0);
    let approval = stack.next(heights.approval);
    let session_picker = stack.next(heights.session_picker);
    let overlay = stack.next(heights.overlay);
    let btw = stack.next(heights.btw);
    let prompt = stack.next(heights.prompt);
    let footer = stack.next(heights.footer_bar);

    ChromeLayout {
        body,
        todo,
        status,
        approval,
        session_picker,
        overlay,
        btw,
        prompt,
        footer,
    }
}

struct ChromeLayoutHeights {
    todo: u16,
    prompt: u16,
    footer_bar: u16,
    session_picker: u16,
    approval: u16,
    overlay: u16,
    btw: u16,
}

impl ChromeLayoutHeights {
    fn new(app: &mut NeoChromeState, area: Rect) -> Self {
        Self {
            todo: todo_height(app, area.width),
            prompt: prompt_panel_height(app, area.width),
            footer_bar: footer_bar_height(area.height),
            session_picker: session_picker_height(app, area.height),
            approval: approval_height(app, area),
            overlay: overlay_height(app, area.height),
            btw: btw_height(app, area),
        }
    }

    fn bottom_height(&self) -> u16 {
        self.todo
            .saturating_add(self.prompt)
            .saturating_add(self.footer_bar)
            .saturating_add(self.session_picker)
            .saturating_add(self.approval)
            .saturating_add(self.overlay)
            .saturating_add(self.btw)
    }
}

struct RectStack {
    x: u16,
    width: u16,
    y: u16,
}

impl RectStack {
    const fn new(area: Rect, y: u16) -> Self {
        Self {
            x: area.x,
            width: area.width,
            y,
        }
    }

    fn next(&mut self, height: u16) -> Rect {
        let rect = Rect {
            x: self.x,
            y: self.y,
            width: self.width,
            height,
        };
        self.y = self.y.saturating_add(height);
        rect
    }
}

fn prompt_panel_height(app: &NeoChromeState, width: u16) -> u16 {
    if app.focused_overlay_blocks_prompt() {
        0
    } else {
        prompt_height(app.prompt(), width)
    }
}

fn footer_bar_height(area_height: u16) -> u16 {
    u16::from(area_height >= 8)
}

fn session_picker_height(app: &NeoChromeState, area_height: u16) -> u16 {
    let height = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::SessionPicker(_)) => 16,
        _ => 0,
    };
    height.min(area_height.saturating_sub(3))
}

fn approval_height(app: &NeoChromeState, area: Rect) -> u16 {
    focused_approval_modal(app)
        .map_or(0, |modal| approval_panel_height(modal, area.width))
        .min(area.height.saturating_sub(3))
}

fn focused_approval_modal(app: &NeoChromeState) -> Option<&ApprovalModal> {
    match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::Approval(request)) => Some(&request.modal),
        _ => None,
    }
}

fn overlay_height(app: &NeoChromeState, area_height: u16) -> u16 {
    let height = app
        .focused_overlay()
        .map_or(0, |_| app.focused_overlay_height());
    height.min(area_height.saturating_sub(3))
}

fn todo_height(app: &NeoChromeState, width: u16) -> u16 {
    if app.has_todos() {
        TodoPanel::new(app.todo_items())
            .with_theme(app.theme())
            .height(width)
    } else {
        0
    }
}

fn btw_height(app: &mut NeoChromeState, area: Rect) -> u16 {
    let theme = app.theme();
    let Some(state) = app.btw_panel_state_mut() else {
        return 0;
    };
    u16::try_from(
        BtwPanel::new(state)
            .with_theme(theme)
            .render(usize::from(area.width), area.height)
            .len(),
    )
    .unwrap_or(area.height / 3)
}

#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = next_sequence(text, index) {
            index += sequence.len();
            continue;
        }

        let Some(grapheme) = text[index..].graphemes(true).next() else {
            break;
        };
        width += display_width(grapheme);
        index += grapheme.len();
    }
    width
}

#[must_use]
pub fn truncate_width(text: &str, max_width: usize, ellipsis: &str, pad: bool) -> String {
    if max_width == 0 {
        return String::new();
    }

    let text_width = visible_width(text);
    if text_width <= max_width {
        if pad {
            let mut padded = text.to_string();
            padded.push_str(&" ".repeat(max_width - text_width));
            return padded;
        }
        return text.to_string();
    }

    let ellipsis_width = visible_width(ellipsis);
    if ellipsis_width >= max_width {
        let clipped = clip_width(ellipsis, max_width);
        if pad {
            let clipped_width = visible_width(&clipped);
            return format!("{clipped}{}", " ".repeat(max_width - clipped_width));
        }
        return clipped;
    }

    let prefix_width = max_width - ellipsis_width;
    let prefix = clip_width(text, prefix_width);
    let mut truncated = format!("{prefix}{ellipsis}");
    if pad {
        let truncated_width = visible_width(&truncated);
        truncated.push_str(&" ".repeat(max_width - truncated_width));
    }
    truncated
}

#[must_use]
pub fn wrap_width(text: &str, max_width: usize) -> Vec<String> {
    wrap_width_with_indices(text, max_width)
        .into_iter()
        .map(|(_, line)| line)
        .collect()
}

/// Wrap `text` to `max_width` display columns and return each wrapped segment
/// with the char index in `text` where that segment starts.
#[must_use]
pub fn wrap_width_with_indices(text: &str, max_width: usize) -> Vec<(usize, String)> {
    if max_width == 0 {
        return vec![(0, String::new())];
    }

    let mut result = Vec::new();
    let mut char_index = 0;

    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            result.push((char_index, String::new()));
        } else {
            let mut current = String::new();
            let mut current_width = 0;
            let mut active_sgr = String::new();
            let mut byte_index = 0;
            let mut segment_start = char_index;

            while byte_index < logical_line.len() {
                if let Some(sequence) = next_sequence(logical_line, byte_index) {
                    current.push_str(sequence);
                    update_active_sgr(sequence, &mut active_sgr);
                    byte_index += sequence.len();
                    continue;
                }

                let Some(grapheme) = logical_line[byte_index..].graphemes(true).next() else {
                    break;
                };

                let grapheme_width = display_width(grapheme);
                if current_width > 0 && current_width + grapheme_width > max_width {
                    result.push((segment_start, std::mem::take(&mut current)));
                    segment_start = char_index;
                    current.push_str(&active_sgr);
                    current_width = 0;
                }

                current.push_str(grapheme);
                current_width += grapheme_width;
                byte_index += grapheme.len();
                char_index += grapheme.chars().count();
            }

            if !current.is_empty() {
                result.push((segment_start, current));
            }
        }
        char_index += 1; // for the '\n' separator
    }

    if result.is_empty() {
        result.push((0, String::new()));
    }
    result
}

pub(crate) fn update_active_sgr(sequence: &str, active_sgr: &mut String) {
    if !sequence.starts_with("\x1b[") || !sequence.ends_with('m') {
        return;
    }

    let action = sgr_style_action(sequence);
    if action.resets {
        active_sgr.clear();
    }
    if action.sets_style {
        active_sgr.push_str(sequence);
    }
}

struct SgrStyleAction {
    resets: bool,
    sets_style: bool,
}

fn sgr_style_action(sequence: &str) -> SgrStyleAction {
    let Some(parameters) = sequence
        .strip_prefix("\x1b[")
        .and_then(|sequence| sequence.strip_suffix('m'))
    else {
        return SgrStyleAction {
            resets: false,
            sets_style: false,
        };
    };

    let mut action = SgrStyleAction {
        resets: parameters.is_empty(),
        sets_style: false,
    };

    for parameter in parameters.split(';') {
        if parameter == "0" {
            action.resets = true;
        } else if !parameter.is_empty() {
            action.sets_style = true;
        }
    }

    action
}

fn approval_panel_height(modal: &ApprovalModal, width: u16) -> u16 {
    let content_width = usize::from(width.saturating_sub(4).max(1));
    let body_lines = wrap_width(&modal.body, content_width).len();
    let total = 2usize
        .saturating_add(1)
        .saturating_add(body_lines)
        .saturating_add(1)
        .saturating_add(1)
        .saturating_add(modal.options.len());
    u16::try_from(total.clamp(5, 10)).unwrap_or(10)
}

fn prompt_height(prompt: &PromptState, width: u16) -> u16 {
    let display_width = usize::from(width.saturating_sub(2).max(1));
    let lines = wrap_width(&format!("> {}", prompt.text), display_width)
        .len()
        .clamp(1, MAX_PROMPT_VISIBLE_LINES);
    u16::try_from(lines.saturating_add(2)).unwrap_or(8)
}

fn clip_width(text: &str, max_width: usize) -> String {
    clip_visible_to_width(text, max_width)
}
