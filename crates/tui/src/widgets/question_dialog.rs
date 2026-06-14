use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Maximum number of option lines shown per question page.
pub const MAX_OPTIONS_VISIBLE: usize = 6;

// ---------------------------------------------------------------------------
// Display data — mirrors `neo_agent_core::events` types but owned by the TUI
// so that the crate does not depend on event wire formats.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDisplayOption {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionDisplayData {
    pub question: String,
    pub header: Option<String>,
    pub body: Option<String>,
    pub options: Vec<QuestionDisplayOption>,
    pub multi_select: bool,
}

// ---------------------------------------------------------------------------
// Mutable state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOptionState {
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionState {
    pub question: String,
    pub header: Option<String>,
    pub body: Option<String>,
    pub options: Vec<QuestionOptionState>,
    pub multi_select: bool,
    /// One entry per option (not counting "Other").
    pub selected: Vec<bool>,
    pub other_selected: bool,
    pub other_text: String,
}

impl QuestionState {
    /// Total selectable items including the implicit "Other" option.
    #[must_use]
    pub fn total_options(&self) -> usize {
        self.options.len() + 1
    }

    /// Has at least one answer been provided?
    #[must_use]
    pub fn is_answered(&self) -> bool {
        self.other_selected || self.selected.iter().any(|&s| s)
    }

    /// Compile the answer string for this question.
    #[must_use]
    pub fn answer(&self) -> String {
        let mut parts: Vec<&str> = self
            .options
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected.get(*i).copied().unwrap_or(false))
            .map(|(_, o)| o.label.as_str())
            .collect();
        if self.other_selected {
            let text = if self.other_text.is_empty() {
                "Other"
            } else {
                self.other_text.as_str()
            };
            parts.push(text);
        }
        parts.join(", ")
    }
}

/// Result returned when the user submits the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionResult {
    pub id: String,
    /// One answer string per question, in order.
    pub answers: Vec<String>,
}

/// Action produced by key handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionDialogAction {
    /// Key was consumed but no terminal action produced.
    None,
    /// User submitted answers (only when all questions answered).
    Submit(QuestionResult),
    /// User cancelled (Esc).
    Cancel,
}

/// Full state for the question dialog overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionStateMachine {
    pub id: String,
    pub questions: Vec<QuestionState>,
    /// Active tab index: `0..questions.len()` = question tabs,
    /// `questions.len()` = submit tab.
    pub active_tab: usize,
    /// Cursor within the current question's option list (0-based, includes
    /// the implicit "Other" option).
    pub cursor: usize,
    /// Scroll offset for option pagination.
    pub scroll: usize,
    /// Inline-edit buffer for "Other".
    pub other_input: String,
    /// Whether "Other" inline-edit is active.
    pub other_editing: bool,
}

impl QuestionStateMachine {
    /// Build a new dialog from display data.
    #[must_use]
    pub fn new(id: impl Into<String>, questions: Vec<QuestionDisplayData>) -> Self {
        let question_states = questions
            .into_iter()
            .map(|q| {
                let option_count = q.options.len();
                QuestionState {
                    question: q.question,
                    header: q.header,
                    body: q.body,
                    options: q
                        .options
                        .into_iter()
                        .map(|o| QuestionOptionState {
                            label: o.label,
                            description: o.description,
                        })
                        .collect(),
                    multi_select: q.multi_select,
                    selected: vec![false; option_count],
                    other_selected: false,
                    other_text: String::new(),
                }
            })
            .collect();
        Self {
            id: id.into(),
            questions: question_states,
            active_tab: 0,
            cursor: 0,
            scroll: 0,
            other_input: String::new(),
            other_editing: false,
        }
    }

    /// Number of tabs including the submit tab.
    #[must_use]
    pub fn tab_count(&self) -> usize {
        self.questions.len() + 1
    }

    /// Is the active tab the submit tab?
    #[must_use]
    pub fn on_submit_tab(&self) -> bool {
        self.active_tab >= self.questions.len()
    }

    /// Has every question been answered?
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.questions.iter().all(QuestionState::is_answered)
    }

    /// Compile answers for all questions.
    #[must_use]
    pub fn compile_answers(&self) -> Vec<String> {
        self.questions.iter().map(QuestionState::answer).collect()
    }

    /// Number of options for the active question (including "Other"), or 0
    /// when on the submit tab.
    #[must_use]
    fn active_option_count(&self) -> usize {
        if self.on_submit_tab() {
            return 0;
        }
        self.questions[self.active_tab].total_options()
    }

    // -- Cursor / tab movement ------------------------------------------------

    pub fn move_cursor_up(&mut self) {
        if self.on_submit_tab() {
            return;
        }
        let count = self.active_option_count();
        if count == 0 {
            return;
        }
        if self.cursor == 0 {
            self.cursor = count - 1;
        } else {
            self.cursor -= 1;
        }
        self.sync_scroll();
    }

    pub fn move_cursor_down(&mut self) {
        if self.on_submit_tab() {
            return;
        }
        let count = self.active_option_count();
        if count == 0 {
            return;
        }
        self.cursor = (self.cursor + 1) % count;
        self.sync_scroll();
    }

    pub fn move_tab_left(&mut self) {
        if self.active_tab > 0 {
            self.active_tab -= 1;
            self.cursor = 0;
            self.scroll = 0;
            self.other_editing = false;
        }
    }

    pub fn move_tab_right(&mut self) {
        if self.active_tab + 1 < self.tab_count() {
            self.active_tab += 1;
            self.cursor = 0;
            self.scroll = 0;
            self.other_editing = false;
        }
    }

    /// Move to the next unanswered question tab, or to submit if all answered.
    fn advance_to_next_unanswered(&mut self) {
        let start = self.active_tab + 1;
        for i in start..self.questions.len() {
            if !self.questions[i].is_answered() {
                self.active_tab = i;
                self.cursor = 0;
                self.scroll = 0;
                self.other_editing = false;
                return;
            }
        }
        // All answered — go to submit.
        self.active_tab = self.questions.len();
        self.cursor = 0;
        self.scroll = 0;
        self.other_editing = false;
    }

    fn sync_scroll(&mut self) {
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + MAX_OPTIONS_VISIBLE {
            self.scroll = self.cursor.saturating_sub(MAX_OPTIONS_VISIBLE - 1);
        }
    }

    // -- Selection ------------------------------------------------------------

    /// Select the option at `cursor`. For single-select this clears all other
    /// selections and auto-advances. For multi-select it toggles.
    pub fn select_current(&mut self) {
        if self.on_submit_tab() {
            return;
        }
        let q = &mut self.questions[self.active_tab];
        let total = q.total_options();
        let other_index = q.options.len(); // "Other" is the last option
        if self.cursor >= total {
            return;
        }

        if self.cursor == other_index {
            // "Other"
            if q.multi_select {
                q.other_selected = !q.other_selected;
                if q.other_selected {
                    self.other_editing = true;
                    self.other_input = q.other_text.clone();
                } else {
                    self.other_editing = false;
                }
            } else {
                // Single-select: clear others, select "Other", enter edit mode.
                for s in &mut q.selected {
                    *s = false;
                }
                q.other_selected = true;
                self.other_editing = true;
                self.other_input = q.other_text.clone();
                // Stay on this question so user can type custom answer.
                // Auto-advance happens when they press Enter to confirm the text.
                return;
            }
        } else if q.multi_select {
            q.selected[self.cursor] = !q.selected[self.cursor];
        } else {
            // Single-select
            for (i, s) in q.selected.iter_mut().enumerate() {
                *s = i == self.cursor;
            }
            q.other_selected = false;
        }

        if !q.multi_select {
            self.advance_to_next_unanswered();
        }
    }

    /// Select option by 1-based number (key '1'..'9').
    pub fn select_by_number(&mut self, n: usize) {
        if n == 0 || self.on_submit_tab() {
            return;
        }
        let total = self.active_option_count();
        if n > total {
            return;
        }
        self.cursor = n - 1;
        self.select_current();
    }

    /// Toggle the current option (multi-select only).
    pub fn toggle_current(&mut self) {
        if self.on_submit_tab() {
            return;
        }
        let q = &mut self.questions[self.active_tab];
        if !q.multi_select {
            // In single-select, Space selects like Enter.
            self.select_current();
            return;
        }
        let total = q.total_options();
        if self.cursor >= total {
            return;
        }
        let other_index = q.options.len();
        if self.cursor == other_index {
            q.other_selected = !q.other_selected;
            if q.other_selected {
                self.other_editing = true;
                self.other_input = q.other_text.clone();
            } else {
                self.other_editing = false;
            }
        } else {
            q.selected[self.cursor] = !q.selected[self.cursor];
        }
    }

    // -- "Other" inline edit --------------------------------------------------

    pub fn insert_char(&mut self, c: char) {
        if self.other_editing {
            self.other_input.push(c);
            if !self.on_submit_tab() {
                self.questions[self.active_tab].other_text = self.other_input.clone();
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.other_editing {
            self.other_input.pop();
            if !self.on_submit_tab() {
                self.questions[self.active_tab].other_text = self.other_input.clone();
            }
        }
    }

    // -- Enter / submit -------------------------------------------------------

    /// Handle the Enter key contextually.
    #[must_use]
    pub fn handle_enter(&mut self) -> QuestionDialogAction {
        if self.on_submit_tab() {
            if self.is_complete() {
                return QuestionDialogAction::Submit(QuestionResult {
                    id: self.id.clone(),
                    answers: self.compile_answers(),
                });
            }
            return QuestionDialogAction::None;
        }
        self.select_current();
        QuestionDialogAction::None
    }

    /// Process a raw crossterm key event.
    #[must_use]
    pub fn handle_key(&mut self, event: KeyEvent) -> QuestionDialogAction {
        // While editing "Other", most keys go to text input.
        if self.other_editing {
            match event.code {
                KeyCode::Esc => {
                    self.other_editing = false;
                    return QuestionDialogAction::None;
                }
                KeyCode::Enter => {
                    self.other_editing = false;
                    if !self.on_submit_tab() && !self.questions[self.active_tab].multi_select {
                        self.advance_to_next_unanswered();
                    }
                    return QuestionDialogAction::None;
                }
                KeyCode::Backspace => {
                    self.backspace();
                    return QuestionDialogAction::None;
                }
                KeyCode::Char(c) => {
                    self.insert_char(c);
                    return QuestionDialogAction::None;
                }
                _ => {} // fall through to normal navigation
            }
        }

        match event.code {
            KeyCode::Up => {
                self.move_cursor_up();
                QuestionDialogAction::None
            }
            KeyCode::Down => {
                self.move_cursor_down();
                QuestionDialogAction::None
            }
            KeyCode::Left | KeyCode::BackTab => {
                self.move_tab_left();
                QuestionDialogAction::None
            }
            KeyCode::Right | KeyCode::Tab => {
                self.move_tab_right();
                QuestionDialogAction::None
            }
            KeyCode::Enter => self.handle_enter(),
            KeyCode::Char(' ') => {
                self.toggle_current();
                QuestionDialogAction::None
            }
            KeyCode::Esc => QuestionDialogAction::Cancel,
            KeyCode::Char(c)
                if c.is_ascii_digit() && !event.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(n) = c.to_digit(10) {
                    self.select_by_number(n as usize);
                }
                QuestionDialogAction::None
            }
            _ => QuestionDialogAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_questions() -> Vec<QuestionDisplayData> {
        vec![
            QuestionDisplayData {
                question: "Which language?".into(),
                header: Some("Lang".into()),
                body: None,
                options: vec![
                    QuestionDisplayOption {
                        label: "Rust".into(),
                        description: None,
                    },
                    QuestionDisplayOption {
                        label: "Go".into(),
                        description: None,
                    },
                ],
                multi_select: false,
            },
            QuestionDisplayData {
                question: "Which features?".into(),
                header: Some("Feat".into()),
                body: None,
                options: vec![
                    QuestionDisplayOption {
                        label: "A".into(),
                        description: None,
                    },
                    QuestionDisplayOption {
                        label: "B".into(),
                        description: None,
                    },
                ],
                multi_select: true,
            },
        ]
    }

    #[test]
    fn new_initialises_state() {
        let state = QuestionStateMachine::new("q-1", make_questions());
        assert_eq!(state.id, "q-1");
        assert_eq!(state.questions.len(), 2);
        assert_eq!(state.active_tab, 0);
        assert!(!state.is_complete());
    }

    #[test]
    fn single_select_auto_advances() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        // Select option 0 on question 0 → should advance to question 1
        state.cursor = 0;
        state.select_current();
        assert_eq!(state.active_tab, 1);
        assert!(state.questions[0].selected[0]);
    }

    #[test]
    fn multi_select_toggles() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        // Go to question 1 (multi-select)
        state.active_tab = 1;
        state.cursor = 0;
        state.toggle_current();
        assert!(state.questions[1].selected[0]);
        assert!(!state.is_complete());
        state.cursor = 1;
        state.toggle_current();
        assert!(state.questions[1].selected[1]);
    }

    #[test]
    fn tab_navigation() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        assert_eq!(state.active_tab, 0);
        state.move_tab_right();
        assert_eq!(state.active_tab, 1);
        state.move_tab_right();
        assert_eq!(state.active_tab, 2); // submit tab
        assert!(state.on_submit_tab());
        state.move_tab_right();
        assert_eq!(state.active_tab, 2); // can't go past submit
        state.move_tab_left();
        assert_eq!(state.active_tab, 1);
    }

    #[test]
    fn compile_answers() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        // Answer question 0
        state.cursor = 0;
        state.select_current(); // selects "Rust", advances to q1
        // Answer question 1 (multi-select)
        state.cursor = 0;
        state.toggle_current(); // selects "A"
        state.cursor = 1;
        state.toggle_current(); // selects "B"

        assert!(state.is_complete());
        let answers = state.compile_answers();
        assert_eq!(answers, vec!["Rust", "A, B"]);
    }

    #[test]
    fn select_by_number() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        state.select_by_number(2); // select "Go"
        assert!(state.questions[0].selected[1]);
        assert!(!state.questions[0].selected[0]);
        assert_eq!(state.active_tab, 1); // auto-advanced
    }

    #[test]
    fn handle_enter_on_submit_when_incomplete() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        state.active_tab = state.questions.len(); // submit tab
        let action = state.handle_enter();
        assert_eq!(action, QuestionDialogAction::None);
    }

    #[test]
    fn handle_enter_on_submit_when_complete() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        // Answer both questions
        state.cursor = 0;
        state.select_current(); // q0: Rust, advances
        state.cursor = 0;
        state.toggle_current(); // q1: A
        state.cursor = 1;
        state.toggle_current(); // q1: B

        state.active_tab = state.questions.len();
        let action = state.handle_enter();
        match action {
            QuestionDialogAction::Submit(result) => {
                assert_eq!(result.id, "q-1");
                assert_eq!(result.answers, vec!["Rust", "A, B"]);
            }
            _ => panic!("expected Submit"),
        }
    }

    #[test]
    fn other_option_single_select() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        // Move cursor to "Other" (index 2, the last option)
        state.cursor = 2;
        state.select_current();
        assert!(state.questions[0].other_selected);
        assert!(state.other_editing);
        // Type something
        state.insert_char('C');
        state.insert_char('+');
        assert_eq!(state.questions[0].other_text, "C+");
    }

    #[test]
    fn esc_cancels() {
        let mut state = QuestionStateMachine::new("q-1", make_questions());
        let action = state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, QuestionDialogAction::Cancel);
    }
}
