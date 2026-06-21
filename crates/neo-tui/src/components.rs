use crate::ansi::{Rect, clip_visible_to_width, display_width, next_sequence};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    chrome::{ApprovalModal, NeoChromeState, OverlayKind, PromptState},
    widgets::TodoPanel,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeLayout {
    pub body: Rect,
    pub todo: Rect,
    pub status: Rect,
    pub approval: Rect,
    pub session_picker: Rect,
    pub overlay: Rect,
    pub prompt: Rect,
    pub footer: Rect,
}

#[must_use]
pub fn chrome_layout(app: &NeoChromeState, area: Rect) -> ChromeLayout {
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
    let prompt = stack.next(heights.prompt);
    let footer = stack.next(heights.footer_bar);

    ChromeLayout {
        body,
        todo,
        status,
        approval,
        session_picker,
        overlay,
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
}

impl ChromeLayoutHeights {
    fn new(app: &NeoChromeState, area: Rect) -> Self {
        Self {
            todo: todo_height(app, area.width),
            prompt: prompt_panel_height(app, area.width),
            footer_bar: footer_bar_height(area.height),
            session_picker: session_picker_height(app, area.height),
            approval: approval_height(app, area),
            overlay: overlay_height(app, area.height),
        }
    }

    fn bottom_height(&self) -> u16 {
        self.todo
            .saturating_add(self.prompt)
            .saturating_add(self.footer_bar)
            .saturating_add(self.session_picker)
            .saturating_add(self.approval)
            .saturating_add(self.overlay)
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
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for logical_line in text.split('\n') {
        if logical_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        wrap_single_line(logical_line, max_width, &mut lines);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn wrap_single_line(text: &str, max_width: usize, lines: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_width = 0;
    let mut active_sgr = String::new();
    let mut index = 0;

    while index < text.len() {
        if let Some(sequence) = next_sequence(text, index) {
            current.push_str(sequence);
            update_active_sgr(sequence, &mut active_sgr);
            index += sequence.len();
            continue;
        }

        let Some(grapheme) = text[index..].graphemes(true).next() else {
            break;
        };

        let grapheme_width = display_width(grapheme);
        if current_width > 0 && current_width + grapheme_width > max_width {
            lines.push(std::mem::take(&mut current));
            current.push_str(&active_sgr);
            current_width = 0;
        }

        current.push_str(grapheme);
        current_width += grapheme_width;
        index += grapheme.len();
    }

    if !current.is_empty() {
        lines.push(current);
    }
}

fn update_active_sgr(sequence: &str, active_sgr: &mut String) {
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
        .clamp(1, 6);
    u16::try_from(lines.saturating_add(2)).unwrap_or(8)
}

fn clip_width(text: &str, max_width: usize) -> String {
    clip_visible_to_width(text, max_width)
}
