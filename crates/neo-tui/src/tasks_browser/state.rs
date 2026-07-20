use super::view::{TaskBrowserItem, TaskBrowserSnapshot};

const CLOSE_TASK_BROWSER: &str = "__close__";
const PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserFilter {
    All,
    Active,
}

impl TaskBrowserFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Active => "ACTIVE",
        }
    }

    #[must_use]
    pub const fn pane_label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserFocus {
    List,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserAction {
    SelectUp,
    SelectDown,
    SelectFirst,
    SelectLast,
    SelectPageUp,
    SelectPageDown,
    ToggleFilter,
    ToggleOutputFocus,
    RequestStop,
    ConfirmStop,
    Refresh,
    Cancel,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserState {
    filter: TaskBrowserFilter,
    snapshot: TaskBrowserSnapshot,
    selected_task_id: Option<String>,
    output_scroll: usize,
    focus: TaskBrowserFocus,
    stop_confirmation_task_id: Option<String>,
    footer_message: Option<String>,
}

impl TaskBrowserState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            filter: TaskBrowserFilter::All,
            snapshot: TaskBrowserSnapshot::new(Vec::new()),
            selected_task_id: None,
            output_scroll: 0,
            focus: TaskBrowserFocus::List,
            stop_confirmation_task_id: None,
            footer_message: None,
        }
    }

    #[must_use]
    pub const fn filter(&self) -> TaskBrowserFilter {
        self.filter
    }

    #[must_use]
    pub const fn focus(&self) -> TaskBrowserFocus {
        self.focus
    }

    #[must_use]
    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task_id.as_deref()
    }

    #[must_use]
    pub fn stop_confirmation_task_id(&self) -> Option<&str> {
        self.stop_confirmation_task_id.as_deref()
    }

    #[must_use]
    pub fn footer_message(&self) -> Option<&str> {
        self.footer_message.as_deref()
    }

    pub fn set_footer_message(&mut self, message: impl Into<String>) {
        self.footer_message = Some(message.into());
    }

    pub fn clear_footer_message(&mut self) {
        self.footer_message = None;
    }

    #[must_use]
    pub const fn output_scroll(&self) -> usize {
        self.output_scroll
    }

    pub fn apply_snapshot(&mut self, snapshot: &TaskBrowserSnapshot) {
        self.snapshot = snapshot.clone();
        self.reconcile_selection();
    }

    #[must_use]
    pub const fn snapshot(&self) -> &TaskBrowserSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn visible_items(&self) -> Vec<&TaskBrowserItem> {
        self.snapshot
            .items()
            .iter()
            .filter(|item| self.filter == TaskBrowserFilter::All || item.status.is_active())
            .collect()
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&TaskBrowserItem> {
        let selected_task_id = self.selected_task_id.as_deref()?;
        self.visible_items()
            .into_iter()
            .find(|item| item.id == selected_task_id)
    }

    pub fn handle_action(&mut self, action: TaskBrowserAction) -> Option<String> {
        match action {
            TaskBrowserAction::SelectUp => self.move_selection(-1),
            TaskBrowserAction::SelectDown => self.move_selection(1),
            TaskBrowserAction::SelectFirst => self.select_at(0),
            TaskBrowserAction::SelectLast => {
                let len = self.visible_items().len();
                if len > 0 {
                    self.select_at(len - 1);
                }
            }
            TaskBrowserAction::SelectPageUp => {
                if self.focus == TaskBrowserFocus::Output {
                    self.move_output_scroll(-PAGE_SIZE.cast_signed());
                } else {
                    self.move_selection(-PAGE_SIZE.cast_signed());
                }
            }
            TaskBrowserAction::SelectPageDown => {
                if self.focus == TaskBrowserFocus::Output {
                    self.move_output_scroll(PAGE_SIZE.cast_signed());
                } else {
                    self.move_selection(PAGE_SIZE.cast_signed());
                }
            }
            TaskBrowserAction::ToggleFilter => {
                self.filter = match self.filter {
                    TaskBrowserFilter::All => TaskBrowserFilter::Active,
                    TaskBrowserFilter::Active => TaskBrowserFilter::All,
                };
                self.reconcile_selection();
            }
            TaskBrowserAction::ToggleOutputFocus => {
                self.focus = match self.focus {
                    TaskBrowserFocus::List => TaskBrowserFocus::Output,
                    TaskBrowserFocus::Output => TaskBrowserFocus::List,
                };
            }
            TaskBrowserAction::RequestStop => {
                let item = self.selected_item()?;
                if !item.can_stop {
                    self.footer_message = Some("Task already finished.".to_owned());
                    return None;
                }
                let task_id = item.id.clone();
                self.stop_confirmation_task_id = Some(task_id.clone());
                return Some(task_id);
            }
            TaskBrowserAction::ConfirmStop => return self.stop_confirmation_task_id.take(),
            TaskBrowserAction::Refresh => {}
            TaskBrowserAction::Cancel => {
                if self.stop_confirmation_task_id.take().is_none() {
                    return Some(CLOSE_TASK_BROWSER.to_owned());
                }
            }
            TaskBrowserAction::Close => return Some(CLOSE_TASK_BROWSER.to_owned()),
        }
        None
    }

    fn reconcile_selection(&mut self) {
        let visible_items = self.visible_items();
        let selected_still_visible = self
            .selected_task_id
            .as_deref()
            .is_some_and(|selected_id| visible_items.iter().any(|item| item.id == selected_id));

        if !selected_still_visible {
            self.selected_task_id = visible_items.first().map(|item| item.id.clone());
        }

        self.output_scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        let visible_items = self.visible_items();
        if visible_items.is_empty() {
            self.selected_task_id = None;
            return;
        }

        let current = self.selected_index_in(&visible_items).unwrap_or(0);
        let next = current
            .saturating_add_signed(delta)
            .min(visible_items.len() - 1);
        self.selected_task_id = Some(visible_items[next].id.clone());
        self.output_scroll = 0;
    }

    fn select_at(&mut self, index: usize) {
        let visible_items = self.visible_items();
        if let Some(item) = visible_items.get(index) {
            self.selected_task_id = Some(item.id.clone());
            self.output_scroll = 0;
        }
    }

    fn selected_index_in(&self, items: &[&TaskBrowserItem]) -> Option<usize> {
        let selected_task_id = self.selected_task_id.as_deref()?;
        items.iter().position(|item| item.id == selected_task_id)
    }

    fn move_output_scroll(&mut self, delta: isize) {
        let line_count = self
            .selected_item()
            .map_or(0, |item| item.preview_lines.len());
        if line_count == 0 {
            self.output_scroll = 0;
            return;
        }
        self.output_scroll = self
            .output_scroll
            .saturating_add_signed(delta)
            .min(line_count - 1);
    }
}

impl Default for TaskBrowserState {
    fn default() -> Self {
        Self::new()
    }
}
