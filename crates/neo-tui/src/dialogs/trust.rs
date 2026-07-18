//! Workspace Trust dialog — blocking security gate shown when project context
//! inputs are detected and no trust decision has been recorded.

use std::path::PathBuf;

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::InputResult;
use crate::primitive::theme::TuiTheme;
use crate::primitive::truncate_width;

/// Inputs shown in the trust dialog. Built from `neo_agent::trust` data but
/// kept independent so `neo-tui` does not depend on `neo-agent`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDialogData {
    pub current_dir: PathBuf,
    pub detected: Vec<TrustDialogInput>,
    pub parent_candidates: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDialogInput {
    pub path: PathBuf,
    pub kind: TrustDialogInputKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDialogInputKind {
    ContextFile,
    NeoDir,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustDialogResult {
    Trust { target: PathBuf },
    Untrusted { target: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustDialogChoice {
    TrustCurrent,
    TrustParent,
    ContinueUntrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustDialogStep {
    Main { selected: TrustDialogChoice },
    Parent { selected: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustDialogState {
    data: TrustDialogData,
    theme: TuiTheme,
    step: TrustDialogStep,
    result: Option<TrustDialogResult>,
}

impl TrustDialogState {
    #[must_use]
    pub fn new(data: TrustDialogData, theme: TuiTheme) -> Self {
        Self {
            data,
            theme,
            step: TrustDialogStep::Main {
                selected: TrustDialogChoice::ContinueUntrusted,
            },
            result: None,
        }
    }

    #[must_use]
    pub const fn data(&self) -> &TrustDialogData {
        &self.data
    }

    #[must_use]
    pub fn result(&self) -> Option<&TrustDialogResult> {
        self.result.as_ref()
    }

    #[must_use]
    pub fn take_result(&mut self) -> Option<TrustDialogResult> {
        self.result.take()
    }

    #[must_use]
    pub fn selected_choice(&self) -> TrustDialogChoice {
        match self.step {
            TrustDialogStep::Main { selected } => selected,
            TrustDialogStep::Parent { .. } => TrustDialogChoice::TrustParent,
        }
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if self.result.is_some() {
            return InputResult::Ignored;
        }

        match self.step {
            TrustDialogStep::Main { selected } => self.handle_main_input(selected, input),
            TrustDialogStep::Parent { selected } => self.handle_parent_input(selected, input),
        }
    }

    fn handle_main_input(
        &mut self,
        selected: TrustDialogChoice,
        input: &InputEvent,
    ) -> InputResult {
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                self.step = TrustDialogStep::Main {
                    selected: self.previous_main_choice(selected),
                };
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                self.step = TrustDialogStep::Main {
                    selected: self.next_main_choice(selected),
                };
                InputResult::Handled
            }
            InputEvent::Insert(character) => {
                if let Some(choice) = self.choice_for_digit(*character) {
                    self.step = TrustDialogStep::Main { selected: choice };
                    InputResult::Handled
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                self.confirm_main_choice(selected)
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                self.result = Some(TrustDialogResult::Untrusted {
                    target: self.data.current_dir.clone(),
                });
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }

    fn handle_parent_input(&mut self, selected: usize, input: &InputEvent) -> InputResult {
        let len = self.data.parent_candidates.len();
        match input {
            InputEvent::Action(KeybindingAction::SelectUp) => {
                let new_selected = if len == 0 {
                    0
                } else if selected == 0 {
                    len - 1
                } else {
                    selected - 1
                };
                self.step = TrustDialogStep::Parent {
                    selected: new_selected,
                };
                InputResult::Handled
            }
            InputEvent::Action(KeybindingAction::SelectDown) => {
                let new_selected = if len == 0 { 0 } else { (selected + 1) % len };
                self.step = TrustDialogStep::Parent {
                    selected: new_selected,
                };
                InputResult::Handled
            }
            InputEvent::Insert(character) => {
                if let Some(digit) = character.to_digit(10) {
                    let index = usize::try_from(digit).unwrap_or(0);
                    if index > 0 && index <= len {
                        self.step = TrustDialogStep::Parent {
                            selected: index - 1,
                        };
                        return InputResult::Handled;
                    }
                }
                InputResult::Ignored
            }
            InputEvent::Action(KeybindingAction::SelectConfirm) | InputEvent::Submit => {
                if let Some(target) = self.data.parent_candidates.get(selected).cloned() {
                    self.result = Some(TrustDialogResult::Trust { target });
                    InputResult::Submitted
                } else {
                    InputResult::Ignored
                }
            }
            InputEvent::Action(KeybindingAction::SelectCancel) | InputEvent::Cancel => {
                self.step = TrustDialogStep::Main {
                    selected: TrustDialogChoice::TrustParent,
                };
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn main_choices(&self) -> Vec<TrustDialogChoice> {
        let mut choices = vec![TrustDialogChoice::TrustCurrent];
        if !self.data.parent_candidates.is_empty() {
            choices.push(TrustDialogChoice::TrustParent);
        }
        choices.push(TrustDialogChoice::ContinueUntrusted);
        choices
    }

    fn previous_main_choice(&self, current: TrustDialogChoice) -> TrustDialogChoice {
        let choices = self.main_choices();
        let position = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0);
        let new_position = if position == 0 {
            choices.len() - 1
        } else {
            position - 1
        };
        choices[new_position]
    }

    fn next_main_choice(&self, current: TrustDialogChoice) -> TrustDialogChoice {
        let choices = self.main_choices();
        let position = choices
            .iter()
            .position(|choice| *choice == current)
            .unwrap_or(0);
        choices[(position + 1) % choices.len()]
    }

    fn choice_for_digit(&self, character: char) -> Option<TrustDialogChoice> {
        let choices = self.main_choices();
        let digit = character.to_digit(10)?;
        let index = usize::try_from(digit).unwrap_or(0);
        if index == 0 || index > choices.len() {
            return None;
        }
        Some(choices[index - 1])
    }

    fn confirm_main_choice(&mut self, choice: TrustDialogChoice) -> InputResult {
        match choice {
            TrustDialogChoice::TrustCurrent => {
                self.result = Some(TrustDialogResult::Trust {
                    target: self.data.current_dir.clone(),
                });
                InputResult::Submitted
            }
            TrustDialogChoice::TrustParent => match self.data.parent_candidates.len() {
                0 => InputResult::Ignored,
                1 => {
                    self.result = Some(TrustDialogResult::Trust {
                        target: self.data.parent_candidates[0].clone(),
                    });
                    InputResult::Submitted
                }
                _ => {
                    self.step = TrustDialogStep::Parent { selected: 0 };
                    InputResult::Handled
                }
            },
            TrustDialogChoice::ContinueUntrusted => {
                self.result = Some(TrustDialogResult::Untrusted {
                    target: self.data.current_dir.clone(),
                });
                InputResult::Submitted
            }
        }
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        match self.step {
            TrustDialogStep::Main { .. } => self.render_main(width),
            TrustDialogStep::Parent { .. } => self.render_parent_step(width),
        }
    }

    fn render_main(&self, width: usize) -> Vec<String> {
        let inner = width.saturating_sub(2).max(1);
        let mut lines = vec![
            border_top(width, "Workspace Trust", self.theme.overlay_border),
            content_line(
                "Neo found project context files before loading this workspace.",
                inner,
            ),
            blank_line(inner),
            content_line("Directory", inner),
        ];

        let dir_display = format!(
            "  {}",
            Self::truncated_path(&self.data.current_dir, inner.saturating_sub(2))
        );
        lines.push(content_line(&dir_display, inner));
        lines.push(blank_line(inner));

        lines.push(content_line("Detected", inner));
        if self.data.detected.is_empty() {
            lines.push(content_line("  (none)", inner));
        } else {
            for input in &self.data.detected {
                lines.push(content_line(
                    &format!("  {}", self.detected_label(input, inner.saturating_sub(2))),
                    inner,
                ));
            }
        }
        lines.push(blank_line(inner));

        lines.push(content_line(
            "Project instructions can change model behavior. Trust only workspaces",
            inner,
        ));
        lines.push(content_line("whose contents you understand.", inner));
        lines.push(blank_line(inner));

        let choices = self.main_choices();
        for (index, choice) in choices.iter().enumerate() {
            let number = index + 1;
            let is_selected = self.selected_choice() == *choice;
            let label = Self::main_choice_label(*choice);
            lines.push(self.choice_line(&format!("{number}. {label}"), is_selected, inner));

            if *choice == TrustDialogChoice::TrustParent && self.data.parent_candidates.len() == 1 {
                let parent_path =
                    Self::truncated_path(&self.data.parent_candidates[0], inner.saturating_sub(4));
                lines.push(content_line(&format!("     {parent_path}"), inner));
            }
        }
        lines.push(blank_line(inner));

        let hint = "↑/↓ select · 1/2/3 choose · Enter confirm";
        lines.push(muted_line(hint, inner, self.theme.text_muted));
        lines.push(border_bottom(width, self.theme.overlay_border));

        lines
    }

    fn render_parent_step(&self, width: usize) -> Vec<String> {
        let inner = width.saturating_sub(2).max(1);
        let mut lines = Vec::new();

        lines.push(border_top(
            width,
            "Trust Parent Directory",
            self.theme.overlay_border,
        ));
        lines.push(content_line(
            "Select the ancestor to trust. Trusting a parent also trusts child folders.",
            inner,
        ));
        lines.push(blank_line(inner));

        match self.step {
            TrustDialogStep::Parent { selected } => {
                for (index, candidate) in self.data.parent_candidates.iter().enumerate() {
                    let number = index + 1;
                    let is_selected = index == selected;
                    let label = format!(
                        "{number}. {}",
                        Self::truncated_path(candidate, inner.saturating_sub(4))
                    );
                    lines.push(self.choice_line(&label, is_selected, inner));
                }
            }
            TrustDialogStep::Main { .. } => unreachable!("parent step renderer called from main"),
        }

        lines.push(blank_line(inner));
        let hint = "↑/↓ select · Enter trust selected · Esc back";
        lines.push(muted_line(hint, inner, self.theme.text_muted));
        lines.push(border_bottom(width, self.theme.overlay_border));

        lines
    }

    fn main_choice_label(choice: TrustDialogChoice) -> &'static str {
        match choice {
            TrustDialogChoice::TrustCurrent => "Trust this directory",
            TrustDialogChoice::TrustParent => "Trust parent directory",
            TrustDialogChoice::ContinueUntrusted => "Continue untrusted",
        }
    }

    fn choice_line(&self, text: &str, is_selected: bool, inner: usize) -> String {
        let marker = if is_selected { "▸" } else { " " };
        let content = truncate_width(&format!("{marker} {text}"), inner, "", false);
        if is_selected {
            format!(
                "\x1b[{};{}m {} \x1b[0m",
                dialog_sgr_fg(self.theme.selected_fg),
                dialog_sgr_bg(self.theme.selected_bg),
                content,
            )
        } else {
            format!(
                "\x1b[38;2;{}m {} \x1b[0m",
                dialog_rgb(self.theme.text_primary),
                content,
            )
        }
    }

    fn detected_label(&self, input: &TrustDialogInput, max_width: usize) -> String {
        let relative = input.path.strip_prefix(&self.data.current_dir).map_or_else(
            |_| {
                input.path.file_name().map_or_else(
                    || input.path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            },
            |path| path.display().to_string(),
        );

        let with_slash = match input.kind {
            TrustDialogInputKind::ContextFile => relative,
            TrustDialogInputKind::NeoDir => {
                if relative.ends_with('/') || relative.is_empty() {
                    relative
                } else {
                    format!("{relative}/")
                }
            }
        };
        truncate_width(&with_slash, max_width, "", false)
    }

    fn truncated_path(path: &std::path::Path, max_width: usize) -> String {
        truncate_width(&path.display().to_string(), max_width, "…", false)
    }
}

fn border_top(width: usize, title: &str, border_color: crate::primitive::Color) -> String {
    let inner = width.saturating_sub(2).max(1);
    let title = format!(" {title} ");
    let title_len = title.chars().count().min(inner);
    let remaining = inner.saturating_sub(title_len);
    format!(
        "\x1b[38;2;{}m╭{}{}\x1b[0m",
        dialog_rgb(border_color),
        title,
        "─".repeat(remaining)
    )
}

fn border_bottom(width: usize, border_color: crate::primitive::Color) -> String {
    let inner = width.saturating_sub(2).max(1);
    format!(
        "\x1b[38;2;{}m╰{}\x1b[0m",
        dialog_rgb(border_color),
        "─".repeat(inner)
    )
}

fn content_line(text: &str, inner: usize) -> String {
    format!(" {} ", truncate_width(text, inner, "", false))
}

fn blank_line(inner: usize) -> String {
    format!(" {} ", " ".repeat(inner))
}

fn muted_line(text: &str, inner: usize, muted: crate::primitive::Color) -> String {
    format!(
        "\x1b[38;2;{}m {} \x1b[0m",
        dialog_rgb(muted),
        truncate_width(text, inner, "", false)
    )
}

fn dialog_rgb(color: crate::primitive::Color) -> String {
    match color {
        crate::primitive::Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
        _ => "255;255;255".into(),
    }
}

fn dialog_sgr_fg(color: crate::primitive::Color) -> String {
    dialog_sgr(color, DialogSgrLayer::Foreground)
}

fn dialog_sgr_bg(color: crate::primitive::Color) -> String {
    dialog_sgr(color, DialogSgrLayer::Background)
}

fn dialog_sgr(color: crate::primitive::Color, layer: DialogSgrLayer) -> String {
    match color {
        crate::primitive::Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", layer.rgb_prefix()),
        crate::primitive::Color::Indexed(i) => format!("{};{i}", layer.indexed_prefix()),
        _ => named_dialog_sgr(color, layer)
            .unwrap_or_default()
            .to_owned(),
    }
}

fn named_dialog_sgr(color: crate::primitive::Color, layer: DialogSgrLayer) -> Option<&'static str> {
    const FOREGROUND: &[(crate::primitive::Color, &str)] = &[
        (crate::primitive::Color::Black, "30"),
        (crate::primitive::Color::Red, "31"),
        (crate::primitive::Color::Green, "32"),
        (crate::primitive::Color::Yellow, "33"),
        (crate::primitive::Color::Blue, "34"),
        (crate::primitive::Color::Magenta, "35"),
        (crate::primitive::Color::Cyan, "36"),
        (crate::primitive::Color::White, "37"),
        (crate::primitive::Color::Gray, "90"),
        (crate::primitive::Color::DarkGray, "90"),
        (crate::primitive::Color::LightRed, "91"),
        (crate::primitive::Color::LightGreen, "92"),
        (crate::primitive::Color::LightYellow, "93"),
        (crate::primitive::Color::LightBlue, "94"),
        (crate::primitive::Color::LightMagenta, "95"),
        (crate::primitive::Color::LightCyan, "96"),
        (crate::primitive::Color::Reset, "39"),
    ];
    const BACKGROUND: &[(crate::primitive::Color, &str)] = &[
        (crate::primitive::Color::Black, "40"),
        (crate::primitive::Color::Red, "41"),
        (crate::primitive::Color::Green, "42"),
        (crate::primitive::Color::Yellow, "43"),
        (crate::primitive::Color::Blue, "44"),
        (crate::primitive::Color::Magenta, "45"),
        (crate::primitive::Color::Cyan, "46"),
        (crate::primitive::Color::White, "47"),
        (crate::primitive::Color::Gray, "100"),
        (crate::primitive::Color::DarkGray, "100"),
        (crate::primitive::Color::LightRed, "101"),
        (crate::primitive::Color::LightGreen, "102"),
        (crate::primitive::Color::LightYellow, "103"),
        (crate::primitive::Color::LightBlue, "104"),
        (crate::primitive::Color::LightMagenta, "105"),
        (crate::primitive::Color::LightCyan, "106"),
        (crate::primitive::Color::Reset, "49"),
    ];

    let table = match layer {
        DialogSgrLayer::Foreground => FOREGROUND,
        DialogSgrLayer::Background => BACKGROUND,
    };
    table
        .iter()
        .find_map(|(candidate, code)| (*candidate == color).then_some(*code))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogSgrLayer {
    Foreground,
    Background,
}

impl DialogSgrLayer {
    const fn rgb_prefix(self) -> &'static str {
        match self {
            Self::Foreground => "38",
            Self::Background => "48",
        }
    }

    const fn indexed_prefix(self) -> &'static str {
        match self {
            Self::Foreground => "5",
            Self::Background => "6",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_data() -> TrustDialogData {
        TrustDialogData {
            current_dir: PathBuf::from("/Users/me/src/acme/tools/cli"),
            detected: vec![
                TrustDialogInput {
                    path: PathBuf::from("/Users/me/src/acme/tools/cli/AGENTS.md"),
                    kind: TrustDialogInputKind::ContextFile,
                },
                TrustDialogInput {
                    path: PathBuf::from("/Users/me/src/acme/tools/cli/.neo"),
                    kind: TrustDialogInputKind::NeoDir,
                },
            ],
            parent_candidates: vec![
                PathBuf::from("/Users/me/src/acme/tools"),
                PathBuf::from("/Users/me/src/acme"),
            ],
        }
    }

    fn theme() -> TuiTheme {
        TuiTheme::default()
    }

    #[test]
    fn defaults_to_continue_untrusted() {
        let state = TrustDialogState::new(sample_data(), theme());
        assert_eq!(
            state.selected_choice(),
            TrustDialogChoice::ContinueUntrusted
        );
    }

    #[test]
    fn renders_detected_inputs_without_file_contents() {
        let state = TrustDialogState::new(sample_data(), theme());
        let lines = state.render_lines(80);
        let text: String = lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("AGENTS.md"), "{text}");
        assert!(text.contains(".neo/"), "{text}");
        assert!(
            !text.contains("project instruction contents"),
            "must not display file contents: {text}"
        );
    }

    #[test]
    fn parent_selection_step_renders_candidates() {
        let mut state = TrustDialogState::new(sample_data(), theme());
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectUp));
        // Now on TrustParent
        assert_eq!(state.selected_choice(), TrustDialogChoice::TrustParent);
        assert!(matches!(state.step, TrustDialogStep::Main { .. }));
        let result = state.handle_input(&InputEvent::Submit);
        assert_eq!(
            result,
            InputResult::Handled,
            "multiple parents should enter selection step"
        );

        let lines = state.render_lines(80);
        let text: String = lines
            .iter()
            .map(|line| crate::primitive::strip_ansi(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("Trust Parent Directory"), "{text}");
        assert!(text.contains("/Users/me/src/acme/tools"), "{text}");
        assert!(text.contains("/Users/me/src/acme"), "{text}");
    }

    #[test]
    fn single_parent_trusts_directly() {
        let data = TrustDialogData {
            current_dir: PathBuf::from("/Users/me/src/acme/tools/cli"),
            detected: vec![TrustDialogInput {
                path: PathBuf::from("/Users/me/src/acme/tools/cli/AGENTS.md"),
                kind: TrustDialogInputKind::ContextFile,
            }],
            parent_candidates: vec![PathBuf::from("/Users/me/src/acme")],
        };
        let mut state = TrustDialogState::new(data, theme());
        state.handle_input(&InputEvent::Insert('2'));
        let result = state.handle_input(&InputEvent::Submit);
        assert_eq!(result, InputResult::Submitted);
        match state.take_result() {
            Some(TrustDialogResult::Trust { target }) => {
                assert_eq!(target, PathBuf::from("/Users/me/src/acme"));
            }
            other => panic!("expected Trust result, got {other:?}"),
        }
    }

    #[test]
    fn cancel_chooses_untrusted() {
        let mut state = TrustDialogState::new(sample_data(), theme());
        let result = state.handle_input(&InputEvent::Cancel);
        assert_eq!(result, InputResult::Cancelled);
        match state.take_result() {
            Some(TrustDialogResult::Untrusted { target }) => {
                assert_eq!(target, PathBuf::from("/Users/me/src/acme/tools/cli"));
            }
            other => panic!("expected Untrusted result, got {other:?}"),
        }
    }
}
