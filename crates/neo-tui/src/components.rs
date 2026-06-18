use crate::ansi::Rect;
use unicode_width::UnicodeWidthChar;

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
    let prompt_height = prompt_height(app.prompt(), area.width);
    let footer_bar_height = if area.height >= 12 {
        2
    } else {
        u16::from(area.height >= 8)
    };
    let session_picker_height = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::SessionPicker(_)) => 16,
        _ => 0,
    }
    .min(area.height.saturating_sub(3));
    let approval_overlay = match app.focused_overlay().map(|overlay| &overlay.kind) {
        Some(OverlayKind::Approval(request)) => Some(request),
        _ => None,
    };
    let approval_height = approval_overlay
        .map_or(0, |request| {
            approval_panel_height(&request.modal, area.width)
        })
        .min(area.height.saturating_sub(3));
    // Rich dialog overlays (model selector, provider manager, choice picker, etc.)
    let overlay_height = if app.focused_overlay().is_some() {
        app.focused_overlay_height()
    } else {
        0
    }
    .min(area.height.saturating_sub(3));
    let todo_height = if app.has_todos() {
        TodoPanel::new(app.todo_items())
            .with_theme(app.theme())
            .height(area.width)
    } else {
        0
    };
    let bottom_height = todo_height
        .saturating_add(prompt_height)
        .saturating_add(footer_bar_height)
        .saturating_add(session_picker_height)
        .saturating_add(approval_height)
        .saturating_add(overlay_height);
    let body_y = area.y;
    let body_height = area.height.saturating_sub(bottom_height);
    let body = Rect {
        x: area.x,
        y: body_y,
        width: area.width,
        height: body_height,
    };
    let todo = Rect {
        x: area.x,
        y: body.y.saturating_add(body.height),
        width: area.width,
        height: todo_height,
    };
    let status = Rect {
        x: area.x,
        y: todo.y.saturating_add(todo.height),
        width: area.width,
        height: 0,
    };
    let approval = Rect {
        x: area.x,
        y: status.y.saturating_add(status.height),
        width: area.width,
        height: approval_height,
    };
    let session_picker = Rect {
        x: area.x,
        y: approval.y.saturating_add(approval.height),
        width: area.width,
        height: session_picker_height,
    };
    let overlay = Rect {
        x: area.x,
        y: session_picker.y.saturating_add(session_picker.height),
        width: area.width,
        height: overlay_height,
    };
    let prompt = Rect {
        x: area.x,
        y: overlay.y.saturating_add(overlay.height),
        width: area.width,
        height: prompt_height,
    };
    let footer = Rect {
        x: area.x,
        y: prompt.y.saturating_add(prompt.height),
        width: area.width,
        height: footer_bar_height,
    };

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

#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut index = 0;
    while index < text.len() {
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };
        width += character.width().unwrap_or(0);
        index += character.len_utf8();
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
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            current.push_str(sequence);
            update_active_sgr(sequence, &mut active_sgr);
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };

        let character_width = character.width().unwrap_or(0);
        if current_width > 0 && current_width + character_width > max_width {
            lines.push(std::mem::take(&mut current));
            current.push_str(&active_sgr);
            current_width = 0;
        }

        current.push(character);
        current_width += character_width;
        index += character.len_utf8();
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
    let mut clipped = String::new();
    let mut width = 0;
    let mut index = 0;

    while index < text.len() {
        if let Some(sequence) = ansi_escape_sequence(text, index) {
            clipped.push_str(sequence);
            index += sequence.len();
            continue;
        }

        let Some(character) = text[index..].chars().next() else {
            break;
        };

        let character_width = character.width().unwrap_or(0);
        if width + character_width > max_width {
            break;
        }
        clipped.push(character);
        width += character_width;
        index += character.len_utf8();
    }

    clipped
}

fn ansi_escape_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.get(start).copied()? != 0x1b {
        return None;
    }
    let introducer = *bytes.get(start + 1)?;
    match introducer {
        b'[' => csi_sequence(text, start),
        b']' | b'P' | b'_' | b'^' | b'X' => string_escape_sequence(text, start),
        b'(' | b')' | b'*' | b'+' | b'-' | b'.' | b'/' => fixed_escape_sequence(text, start, 3),
        0x40..=0x5f => fixed_escape_sequence(text, start, 2),
        _ => None,
    }
}

fn csi_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut index = start + 2;
    while index < bytes.len() {
        if (0x40..=0x7e).contains(&bytes[index]) {
            return text.get(start..index + 1);
        }
        index += 1;
    }
    None
}

fn string_escape_sequence(text: &str, start: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut index = start + 2;
    while index < bytes.len() {
        match bytes[index] {
            0x07 => return text.get(start..index + 1),
            0x1b if bytes.get(index + 1).copied() == Some(b'\\') => {
                return text.get(start..index + 2);
            }
            _ => index += 1,
        }
    }
    None
}

fn fixed_escape_sequence(text: &str, start: usize, byte_len: usize) -> Option<&str> {
    text.get(start..start + byte_len)
}
