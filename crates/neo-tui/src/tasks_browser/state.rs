use super::view::{TaskBrowserItem, TaskBrowserKind, TaskBrowserSnapshot, TaskBrowserStatus};

const CLOSE_TASK_BROWSER: &str = "__close__";
const PAGE_SIZE: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBrowserFilter {
    All,
    Active,
    Workflow,
}

impl TaskBrowserFilter {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::All => "ALL",
            Self::Active => "ACTIVE",
            Self::Workflow => "WORKFLOW",
        }
    }

    #[must_use]
    pub const fn pane_label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Active => "active",
            Self::Workflow => "workflow",
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::All => Self::Active,
            Self::Active => Self::Workflow,
            Self::Workflow => Self::All,
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
    RequestPause,
    ConfirmPause,
    RequestResume,
    ConfirmResume,
    RequestStop,
    ConfirmStop,
    RequestAnswer,
    ConfirmAnswer,
    /// Request next list page from the host (query-bound cursor).
    RequestNextPage,
    /// Request previous list page from the host.
    RequestPrevPage,
    Refresh,
    Cancel,
    Close,
}

/// Host-visible list query intent derived from browser filter state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskBrowserListIntent {
    pub active_only: bool,
    pub workflow_only: bool,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserState {
    filter: TaskBrowserFilter,
    snapshot: TaskBrowserSnapshot,
    selected_task_id: Option<String>,
    output_scroll: usize,
    focus: TaskBrowserFocus,
    pause_confirmation_task_id: Option<String>,
    resume_confirmation_task_id: Option<String>,
    stop_confirmation_task_id: Option<String>,
    answer_confirmation_task_id: Option<String>,
    footer_message: Option<String>,
    /// Cursor used to fetch the current page (None = first page).
    list_cursor: Option<String>,
    /// Stack of previous page cursors for back navigation.
    list_prev_cursors: Vec<Option<String>>,
    /// Host-reported next cursor (query-bound).
    list_next_cursor: Option<String>,
    list_has_more: bool,
    list_query_hash: Option<String>,
    /// When true, host should re-fetch with [`Self::list_intent`].
    list_refresh_requested: bool,
    page_limit: usize,
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
            pause_confirmation_task_id: None,
            resume_confirmation_task_id: None,
            stop_confirmation_task_id: None,
            answer_confirmation_task_id: None,
            footer_message: None,
            list_cursor: None,
            list_prev_cursors: Vec::new(),
            list_next_cursor: None,
            list_has_more: false,
            list_query_hash: None,
            list_refresh_requested: false,
            page_limit: 50,
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
    pub fn pause_confirmation_task_id(&self) -> Option<&str> {
        self.pause_confirmation_task_id.as_deref()
    }

    #[must_use]
    pub fn resume_confirmation_task_id(&self) -> Option<&str> {
        self.resume_confirmation_task_id.as_deref()
    }

    #[must_use]
    pub fn answer_confirmation_task_id(&self) -> Option<&str> {
        self.answer_confirmation_task_id.as_deref()
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

    #[must_use]
    pub fn list_cursor(&self) -> Option<&str> {
        self.list_cursor.as_deref()
    }

    #[must_use]
    pub fn list_next_cursor(&self) -> Option<&str> {
        self.list_next_cursor.as_deref()
    }

    #[must_use]
    pub const fn list_has_more(&self) -> bool {
        self.list_has_more
    }

    #[must_use]
    pub fn list_query_hash(&self) -> Option<&str> {
        self.list_query_hash.as_deref()
    }

    #[must_use]
    pub const fn list_refresh_requested(&self) -> bool {
        self.list_refresh_requested
    }

    pub fn clear_list_refresh_requested(&mut self) {
        self.list_refresh_requested = false;
    }

    /// Build the host list intent from the current filter / cursor.
    #[must_use]
    pub fn list_intent(&self) -> TaskBrowserListIntent {
        TaskBrowserListIntent {
            active_only: matches!(self.filter, TaskBrowserFilter::Active),
            workflow_only: matches!(self.filter, TaskBrowserFilter::Workflow),
            cursor: self.list_cursor.clone(),
            limit: self.page_limit,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &TaskBrowserSnapshot) {
        if let Err(message) = self.apply_snapshot_checked(snapshot) {
            self.footer_message = Some(message);
        }
    }

    /// Apply a paged snapshot, enforcing query-bound cursor rules.
    pub fn apply_snapshot_checked(&mut self, snapshot: &TaskBrowserSnapshot) -> Result<(), String> {
        if let (Some(expected), Some(actual)) = (
            self.list_query_hash.as_deref(),
            snapshot.query_hash.as_deref(),
        ) {
            // Allow first paint (no prior hash) and same-hash pages.
            if expected != actual && self.list_cursor.is_some() {
                return Err(
                    "list cursor query/filter does not match the active browser query".to_owned(),
                );
            }
        }
        self.snapshot = snapshot.clone();
        self.list_next_cursor = snapshot.next_cursor.clone();
        self.list_has_more = snapshot.has_more;
        if let Some(hash) = &snapshot.query_hash {
            self.list_query_hash = Some(hash.clone());
        }
        self.list_refresh_requested = false;
        self.reconcile_selection();
        Ok(())
    }

    #[must_use]
    pub const fn snapshot(&self) -> &TaskBrowserSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn visible_items(&self) -> Vec<&TaskBrowserItem> {
        // Server-side filters already applied for Active/Workflow pages; still
        // defend client-side so offline snapshots remain consistent.
        self.snapshot
            .items()
            .iter()
            .filter(|item| match self.filter {
                TaskBrowserFilter::All => true,
                TaskBrowserFilter::Active => item.status.is_active(),
                TaskBrowserFilter::Workflow => item.kind == TaskBrowserKind::Workflow,
            })
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
                self.filter = self.filter.next();
                self.list_cursor = None;
                self.list_prev_cursors.clear();
                self.list_next_cursor = None;
                self.list_has_more = false;
                self.list_query_hash = None;
                self.list_refresh_requested = true;
                self.reconcile_selection();
            }
            TaskBrowserAction::ToggleOutputFocus => {
                self.focus = match self.focus {
                    TaskBrowserFocus::List => TaskBrowserFocus::Output,
                    TaskBrowserFocus::Output => TaskBrowserFocus::List,
                };
            }
            TaskBrowserAction::RequestPause => {
                let item = self.selected_item()?;
                if item.kind != TaskBrowserKind::Workflow {
                    self.footer_message = Some("Only workflow tasks can be paused.".to_owned());
                    return None;
                }
                if item.status != TaskBrowserStatus::Running {
                    self.footer_message = Some("Only running workflows can be paused.".to_owned());
                    return None;
                }
                let task_id = item.id.clone();
                let label = item.human_handle.clone().unwrap_or_else(|| task_id.clone());
                self.clear_confirmations();
                self.pause_confirmation_task_id = Some(task_id.clone());
                self.footer_message = Some(format!("Pause {label}? Enter confirm   Esc cancel"));
                return Some(task_id);
            }
            TaskBrowserAction::ConfirmPause => {
                self.footer_message = None;
                return self.pause_confirmation_task_id.take();
            }
            TaskBrowserAction::RequestResume => {
                let item = self.selected_item()?;
                if item.kind != TaskBrowserKind::Workflow {
                    self.footer_message = Some("Only workflow tasks can be resumed.".to_owned());
                    return None;
                }
                if item.status != TaskBrowserStatus::Paused {
                    self.footer_message = Some("Only paused workflows can be resumed.".to_owned());
                    return None;
                }
                let task_id = item.id.clone();
                let label = item.human_handle.clone().unwrap_or_else(|| task_id.clone());
                self.clear_confirmations();
                self.resume_confirmation_task_id = Some(task_id.clone());
                self.footer_message = Some(format!("Resume {label}? Enter confirm   Esc cancel"));
                return Some(task_id);
            }
            TaskBrowserAction::ConfirmResume => {
                self.footer_message = None;
                return self.resume_confirmation_task_id.take();
            }
            TaskBrowserAction::RequestStop => {
                let item = self.selected_item()?;
                if !item.can_stop {
                    self.footer_message = Some("Task already finished.".to_owned());
                    return None;
                }
                let task_id = item.id.clone();
                self.clear_confirmations();
                self.stop_confirmation_task_id = Some(task_id.clone());
                self.footer_message = None;
                return Some(task_id);
            }
            TaskBrowserAction::ConfirmStop => return self.stop_confirmation_task_id.take(),
            TaskBrowserAction::RequestAnswer => {
                let item = self.selected_item()?;
                if item.kind != TaskBrowserKind::Workflow {
                    self.footer_message = Some("Only workflow tasks can be answered.".to_owned());
                    return None;
                }
                if item.status != TaskBrowserStatus::Waiting {
                    self.footer_message =
                        Some("Only awaiting-user workflows can be answered.".to_owned());
                    return None;
                }
                let Some(request_id) = item
                    .workflow
                    .as_ref()
                    .and_then(|w| w.pending_request_id.clone())
                else {
                    self.footer_message =
                        Some("No pending user request on this workflow.".to_owned());
                    return None;
                };
                let task_id = item.id.clone();
                let label = item.human_handle.clone().unwrap_or_else(|| task_id.clone());
                self.clear_confirmations();
                self.answer_confirmation_task_id = Some(task_id.clone());
                self.footer_message = Some(format!(
                    "Answer {request_id} on {label}? Enter confirm   Esc cancel"
                ));
                return Some(task_id);
            }
            TaskBrowserAction::ConfirmAnswer => {
                self.footer_message = None;
                return self.answer_confirmation_task_id.take();
            }
            TaskBrowserAction::RequestNextPage => {
                if !self.list_has_more {
                    self.footer_message = Some("No more pages.".to_owned());
                    return None;
                }
                let Some(next) = self.list_next_cursor.clone() else {
                    self.footer_message = Some("No more pages.".to_owned());
                    return None;
                };
                self.list_prev_cursors.push(self.list_cursor.clone());
                self.list_cursor = Some(next);
                self.list_refresh_requested = true;
                self.footer_message = None;
            }
            TaskBrowserAction::RequestPrevPage => {
                let Some(prev) = self.list_prev_cursors.pop() else {
                    self.footer_message = Some("Already on the first page.".to_owned());
                    return None;
                };
                self.list_cursor = prev;
                self.list_refresh_requested = true;
                self.footer_message = None;
            }
            TaskBrowserAction::Refresh => {
                self.list_refresh_requested = true;
            }
            TaskBrowserAction::Cancel => {
                let cancelled_confirmation = self.clear_confirmations();
                if cancelled_confirmation {
                    self.footer_message = None;
                } else {
                    return Some(CLOSE_TASK_BROWSER.to_owned());
                }
            }
            TaskBrowserAction::Close => return Some(CLOSE_TASK_BROWSER.to_owned()),
        }
        None
    }

    fn clear_confirmations(&mut self) -> bool {
        self.pause_confirmation_task_id.take().is_some()
            | self.resume_confirmation_task_id.take().is_some()
            | self.stop_confirmation_task_id.take().is_some()
            | self.answer_confirmation_task_id.take().is_some()
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
