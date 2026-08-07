use std::collections::BTreeMap;

use neo_agent_core::workflow::{CompiledSchema, WorkflowChildKey, WorkflowStepKey};

use super::view::{
    TaskBrowserItem, TaskBrowserKind, TaskBrowserPendingUserRequest, TaskBrowserSnapshot,
    TaskBrowserStatus, TaskBrowserWorkflowChild, TaskBrowserWorkflowStep,
};
use super::{WorkflowAnswerControl, WorkflowAnswerForm};

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
    Tasks,
    Output,
    Steps,
    Agents,
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
    OpenTaskDetails,
    ToggleOutputFocus,
    OpenWorkflow,
    ToggleWorkflowFocus,
    OpenWorkflowChildDetails,
    TogglePauseResume,
    RequestStop,
    ConfirmStop,
    RequestSave,
    ToggleSaveDestination,
    SubmitSave,
    OpenAnswer,
    DismissAnswer,
    SelectPreviousAnswerField,
    SelectNextAnswerField,
    SubmitAnswer,
    RequestNextChildPage,
    RequestPrevChildPage,
    Cancel,
    Close,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TaskBrowserListIntent {
    pub active_only: bool,
    pub workflow_only: bool,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowChildPageIntent {
    pub task_id: String,
    pub step: Option<WorkflowStepKey>,
    pub cursor: Option<String>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowAnswerDraft {
    pub request_id: String,
    pub value: serde_json::Value,
    pub json_editor: String,
    pub field_errors: Vec<String>,
    pub form: WorkflowAnswerForm,
    pub selected_field: usize,
    pub(crate) branch_indices: BTreeMap<String, usize>,
    branch_drafts: BTreeMap<String, Vec<serde_json::Value>>,
    pub(crate) choice_indices: BTreeMap<String, usize>,
    field_inputs: BTreeMap<WorkflowAnswerInputKey, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct WorkflowAnswerInputKey {
    path: String,
    branch: Option<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowAnswerSubmission {
    pub task_id: String,
    pub request_id: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSaveDestination {
    Project,
    AllProjects,
}

impl WorkflowSaveDestination {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Project => "This project",
            Self::AllProjects => "All projects",
        }
    }

    const fn next(self) -> Self {
        match self {
            Self::Project => Self::AllProjects,
            Self::AllProjects => Self::Project,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSaveDraft {
    pub name: String,
    pub destination: WorkflowSaveDestination,
    pub replacement: Option<WorkflowSaveReplacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSaveReplacement {
    pub existing_display_name: String,
    pub new_display_name: String,
    pub target_location: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowSaveSubmission {
    pub task_id: String,
    pub name: String,
    pub destination: WorkflowSaveDestination,
    pub replace: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskBrowserState {
    filter: TaskBrowserFilter,
    snapshot: TaskBrowserSnapshot,
    selected_task_id: Option<String>,
    task_details_open: bool,
    output_scroll: usize,
    workflow_task_id: Option<String>,
    focus: TaskBrowserFocus,
    selected_step_key: Option<WorkflowStepKey>,
    selected_child_key: Option<WorkflowChildKey>,
    child_details_open: bool,
    child_cursor: Option<String>,
    child_prev_cursors: Vec<Option<String>>,
    child_refresh_requested: bool,
    stop_confirmation_task_id: Option<String>,
    save_draft: Option<WorkflowSaveDraft>,
    save_submission: Option<WorkflowSaveSubmission>,
    answer_draft: Option<WorkflowAnswerDraft>,
    dismissed_request_id: Option<String>,
    answer_submission: Option<WorkflowAnswerSubmission>,
    footer_message: Option<String>,
    list_cursor: Option<String>,
    list_next_cursor: Option<String>,
    list_has_more: bool,
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
            workflow_task_id: None,
            focus: TaskBrowserFocus::Tasks,
            task_details_open: false,
            output_scroll: 0,
            selected_step_key: None,
            selected_child_key: None,
            child_details_open: false,
            child_cursor: None,
            child_prev_cursors: Vec::new(),
            child_refresh_requested: false,
            stop_confirmation_task_id: None,
            save_draft: None,
            save_submission: None,
            answer_draft: None,
            dismissed_request_id: None,
            answer_submission: None,
            footer_message: None,
            list_cursor: None,
            list_next_cursor: None,
            list_has_more: false,
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
    pub fn snapshot(&self) -> &TaskBrowserSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub fn selected_task_id(&self) -> Option<&str> {
        self.selected_task_id.as_deref()
    }

    /// Select a workflow task and enter its workflow page.
    pub fn open_workflow_for_task(&mut self, task_id: &str) -> bool {
        let is_workflow = self
            .snapshot
            .items
            .iter()
            .any(|item| item.id == task_id && item.kind == TaskBrowserKind::Workflow);
        if !is_workflow {
            return false;
        }
        self.selected_task_id = Some(task_id.to_owned());
        self.open_workflow();
        true
    }

    #[must_use]
    pub fn workflow_item(&self) -> Option<&TaskBrowserItem> {
        let task_id = self.workflow_task_id.as_deref()?;
        self.snapshot.items.iter().find(|item| item.id == task_id)
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<&TaskBrowserItem> {
        let task_id = self.selected_task_id.as_deref()?;
        self.visible_items()
            .into_iter()
            .find(|item| item.id == task_id)
    }

    #[must_use]
    pub fn selected_workflow_step(&self) -> Option<&TaskBrowserWorkflowStep> {
        self.workflow_item()?
            .workflow
            .as_ref()?
            .steps
            .iter()
            .find(|step| Some(&step.key) == self.selected_step_key.as_ref())
    }

    #[must_use]
    pub fn selected_workflow_child(&self) -> Option<&TaskBrowserWorkflowChild> {
        self.workflow_item()?
            .workflow
            .as_ref()?
            .child_page
            .items
            .iter()
            .find(|child| Some(&child.key) == self.selected_child_key.as_ref())
    }

    #[must_use]
    pub const fn child_details_open(&self) -> bool {
        self.child_details_open
    }

    #[must_use]
    pub const fn task_details_open(&self) -> bool {
        self.task_details_open
    }

    #[must_use]
    pub const fn output_scroll(&self) -> usize {
        self.output_scroll
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
    pub fn save_draft(&self) -> Option<&WorkflowSaveDraft> {
        self.save_draft.as_ref()
    }

    pub fn set_save_name(&mut self, name: impl Into<String>) {
        if let Some(draft) = self.save_draft.as_mut() {
            draft.name = name.into();
        }
    }

    pub fn request_save_replace(
        &mut self,
        name: impl Into<String>,
        destination: WorkflowSaveDestination,
        existing_display_name: impl Into<String>,
        new_display_name: impl Into<String>,
        target_location: impl Into<String>,
    ) {
        self.save_draft = Some(WorkflowSaveDraft {
            name: name.into(),
            destination,
            replacement: Some(WorkflowSaveReplacement {
                existing_display_name: existing_display_name.into(),
                new_display_name: new_display_name.into(),
                target_location: target_location.into(),
            }),
        });
    }

    pub fn take_save_submission(&mut self) -> Option<WorkflowSaveSubmission> {
        self.save_submission.take()
    }

    #[must_use]
    pub fn answer_draft(&self) -> Option<&WorkflowAnswerDraft> {
        self.answer_draft.as_ref()
    }

    #[must_use]
    pub fn take_answer_submission(&mut self) -> Option<WorkflowAnswerSubmission> {
        self.answer_submission.take()
    }

    pub fn set_answer_json(&mut self, raw: impl Into<String>) {
        let raw = raw.into();
        let Some(draft) = self.answer_draft.as_mut() else {
            return;
        };
        draft.json_editor = raw.clone();
        match serde_json::from_str(&raw) {
            Ok(value) => {
                draft.value = value;
                draft.field_errors.clear();
            }
            Err(error) => draft.field_errors = vec![format!("JSON: {error}")],
        }
    }

    pub fn move_answer_field(&mut self, delta: isize) {
        let Some(draft) = self.answer_draft.as_mut() else {
            return;
        };
        let field_count = draft
            .form
            .visible_fields(&draft.value, &draft.choice_indices, &draft.branch_indices)
            .len();
        draft.selected_field = draft
            .selected_field
            .saturating_add_signed(delta)
            .min(field_count.saturating_sub(1));
    }

    #[must_use]
    pub fn selected_answer_field(&self) -> Option<&super::WorkflowAnswerField> {
        let draft = self.answer_draft.as_ref()?;
        draft
            .form
            .visible_fields(&draft.value, &draft.choice_indices, &draft.branch_indices)
            .get(draft.selected_field)
            .copied()
    }

    fn selected_answer_field_with_path(&self) -> Option<(super::WorkflowAnswerField, String)> {
        let draft = self.answer_draft.as_ref()?;
        let fields =
            draft
                .form
                .visible_fields(&draft.value, &draft.choice_indices, &draft.branch_indices);
        let field = (*fields.get(draft.selected_field)?).clone();
        let path = field.resolved_path(&draft.choice_indices);
        Some((field, path))
    }

    pub fn cycle_selected_answer_value(&mut self, delta: isize) -> bool {
        let Some((field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        let current = answer_value_at_path(&draft.value, &path).cloned();
        match &field.control {
            WorkflowAnswerControl::Boolean => set_answer_draft_value(
                draft,
                &path,
                serde_json::json!(!current.and_then(|value| value.as_bool()).unwrap_or(false)),
            ),
            WorkflowAnswerControl::Choice(options) => {
                let current = current
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .and_then(|value| options.iter().position(|option| option == &value))
                    .unwrap_or(0);
                let next = wrapped_index(current, delta, options.len());
                options.get(next).is_some_and(|value| {
                    set_answer_draft_value(draft, &path, serde_json::Value::String(value.clone()))
                })
            }
            WorkflowAnswerControl::MultipleChoice(options) => {
                let current = draft.choice_indices.get(&path).copied().unwrap_or(0);
                draft
                    .choice_indices
                    .insert(path, wrapped_index(current, delta, options.len()));
                true
            }
            WorkflowAnswerControl::BranchChoice(_) => {
                let current_index = draft.branch_indices.get(&path).copied().unwrap_or(0);
                if let Some(values) = draft.branch_drafts.get_mut(&path)
                    && let Some(current_value) = current
                    && let Some(slot) = values.get_mut(current_index)
                {
                    *slot = current_value;
                }
                let next = wrapped_index(current_index, delta, field.branches.len());
                let Some(value) = draft
                    .branch_drafts
                    .get(&path)
                    .and_then(|values| values.get(next))
                    .cloned()
                else {
                    return false;
                };
                draft.branch_indices.insert(path.clone(), next);
                set_answer_draft_value(draft, &path, value)
            }
            WorkflowAnswerControl::ObjectArray => {
                let Some(rows) = current.and_then(|value| value.as_array().cloned()) else {
                    return false;
                };
                if rows.is_empty() {
                    return false;
                }
                let current = draft.choice_indices.get(&path).copied().unwrap_or(0);
                draft
                    .choice_indices
                    .insert(path, wrapped_index(current, delta, rows.len()));
                true
            }
            _ => false,
        }
    }

    pub fn toggle_selected_answer_value(&mut self) -> bool {
        let Some((field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        if matches!(field.control, WorkflowAnswerControl::Boolean) {
            return self.cycle_selected_answer_value(1);
        }
        if matches!(field.control, WorkflowAnswerControl::BranchChoice(_)) {
            let Some(draft) = self.answer_draft.as_mut() else {
                return false;
            };
            let index = draft.branch_indices.get(&path).copied().unwrap_or(0);
            if field
                .branches
                .get(index)
                .and_then(|schema| schema.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("boolean")
            {
                let value = !answer_value_at_path(&draft.value, &path)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                return set_answer_draft_value(draft, &path, serde_json::json!(value));
            }
        }
        let WorkflowAnswerControl::MultipleChoice(options) = &field.control else {
            return false;
        };
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        let index = draft.choice_indices.get(&path).copied().unwrap_or(0);
        let Some(option) = options.get(index) else {
            return false;
        };
        let Some(values) = answer_value_at_path_mut(&mut draft.value, &path)
            .and_then(serde_json::Value::as_array_mut)
        else {
            return false;
        };
        if let Some(position) = values
            .iter()
            .position(|value| value.as_str() == Some(option))
        {
            values.remove(position);
        } else {
            values.push(serde_json::Value::String(option.clone()));
        }
        sync_answer_draft(draft);
        true
    }

    pub fn append_selected_answer_char(&mut self, ch: char) -> bool {
        let Some((mut field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        field.path = path;
        let input_key = answer_field_input_key(draft, &field);
        let mut raw = draft
            .field_inputs
            .get(&input_key)
            .cloned()
            .unwrap_or_default();
        raw.push(ch);
        update_answer_field_from_text(draft, &field, raw)
    }

    pub fn delete_selected_answer_char(&mut self) -> bool {
        let Some((mut field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        field.path = path;
        let input_key = answer_field_input_key(draft, &field);
        let mut raw = draft
            .field_inputs
            .get(&input_key)
            .cloned()
            .unwrap_or_else(|| {
                answer_value_at_path(&draft.value, &field.path)
                    .map(answer_value_as_input)
                    .unwrap_or_default()
            });
        raw.pop();
        update_answer_field_from_text(draft, &field, raw)
    }

    pub fn paste_selected_answer_value(&mut self, raw: &str) -> bool {
        let Some((mut field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        field.path = path;
        match field.control {
            WorkflowAnswerControl::Text | WorkflowAnswerControl::Number => {
                update_answer_field_from_text(draft, &field, raw.to_owned())
            }
            WorkflowAnswerControl::BranchChoice(_) => {
                let index = draft.branch_indices.get(&field.path).copied().unwrap_or(0);
                let schema = field.branches.get(index).unwrap_or(&field.schema);
                let value =
                    if schema.get("type").and_then(serde_json::Value::as_str) == Some("string") {
                        serde_json::Value::String(raw.to_owned())
                    } else {
                        match serde_json::from_str(raw) {
                            Ok(value) => value,
                            Err(error) => {
                                draft.field_errors =
                                    vec![format!("{}: {error}", answer_error_path(&field.path))];
                                return true;
                            }
                        }
                    };
                set_answer_draft_value(draft, &field.path, value)
            }
            WorkflowAnswerControl::ObjectArray => {
                match serde_json::from_str::<serde_json::Value>(raw) {
                    Ok(value @ serde_json::Value::Array(_)) => {
                        set_answer_draft_value(draft, &field.path, value)
                    }
                    Ok(_) => {
                        draft.field_errors = vec![format!(
                            "{}: expected a list",
                            answer_error_path(&field.path)
                        )];
                        true
                    }
                    Err(error) => {
                        draft.field_errors =
                            vec![format!("{}: {error}", answer_error_path(&field.path))];
                        true
                    }
                }
            }
            _ => false,
        }
    }

    pub fn append_selected_answer_object_row(&mut self) -> bool {
        let Some((field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        if !matches!(field.control, WorkflowAnswerControl::ObjectArray) {
            return false;
        }
        let Some(item_schema) = field.schema.get("items") else {
            return false;
        };
        let row = schema_default(item_schema);
        if !self.append_answer_object_row(&path, row) {
            return false;
        }
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        let selected = answer_value_at_path(&draft.value, &path)
            .and_then(serde_json::Value::as_array)
            .map_or(0, |rows| rows.len().saturating_sub(1));
        draft.choice_indices.insert(path, selected);
        true
    }

    pub fn remove_selected_answer_object_row(&mut self) -> bool {
        let Some((field, path)) = self.selected_answer_field_with_path() else {
            return false;
        };
        if !matches!(field.control, WorkflowAnswerControl::ObjectArray) {
            return false;
        }
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        let selected = draft.choice_indices.get(&path).copied().unwrap_or(0);
        let Some(rows) = answer_value_at_path_mut(&mut draft.value, &path)
            .and_then(serde_json::Value::as_array_mut)
        else {
            return false;
        };
        if rows.is_empty() {
            return false;
        }
        rows.remove(selected.min(rows.len() - 1));
        draft
            .choice_indices
            .insert(path.clone(), selected.min(rows.len().saturating_sub(1)));
        let row_prefix = format!("{path}/");
        draft
            .field_inputs
            .retain(|key, _| !key.path.starts_with(&row_prefix));
        let field_count = draft
            .form
            .visible_fields(&draft.value, &draft.choice_indices, &draft.branch_indices)
            .len();
        draft.selected_field = draft.selected_field.min(field_count.saturating_sub(1));
        sync_answer_draft(draft);
        true
    }

    pub fn set_answer_field_value(&mut self, path: &str, value: serde_json::Value) -> bool {
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        set_answer_draft_value(draft, path, value)
    }

    pub fn append_answer_object_row(&mut self, path: &str, row: serde_json::Value) -> bool {
        let Some(draft) = self.answer_draft.as_mut() else {
            return false;
        };
        let Some(rows) = draft
            .value
            .pointer_mut(path)
            .and_then(serde_json::Value::as_array_mut)
        else {
            return false;
        };
        rows.push(row);
        sync_answer_draft(draft);
        true
    }

    #[must_use]
    pub fn workflow_child_page_intent(&self) -> Option<WorkflowChildPageIntent> {
        self.workflow_item()?.workflow.as_ref()?;
        Some(WorkflowChildPageIntent {
            task_id: self.workflow_task_id.clone()?,
            step: self.selected_step_key.clone(),
            cursor: self.child_cursor.clone(),
            limit: self.page_limit,
        })
    }

    #[must_use]
    pub fn list_intent(&self) -> TaskBrowserListIntent {
        TaskBrowserListIntent {
            active_only: self.filter == TaskBrowserFilter::Active,
            workflow_only: self.filter == TaskBrowserFilter::Workflow,
            cursor: self.list_cursor.clone(),
            limit: self.page_limit,
        }
    }

    pub fn apply_snapshot(&mut self, snapshot: &TaskBrowserSnapshot) {
        self.snapshot = snapshot.clone();
        self.list_next_cursor.clone_from(&snapshot.next_cursor);
        self.list_has_more = snapshot.has_more;
        self.list_refresh_requested = false;
        self.reconcile_selection();
        self.open_pending_answer();
    }

    #[must_use]
    pub const fn list_refresh_requested(&self) -> bool {
        self.list_refresh_requested
    }

    pub fn take_child_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.child_refresh_requested)
    }

    #[must_use]
    pub fn visible_items(&self) -> Vec<&TaskBrowserItem> {
        self.snapshot
            .items
            .iter()
            .filter(|item| match self.filter {
                TaskBrowserFilter::All => true,
                TaskBrowserFilter::Active => item.status.is_active(),
                TaskBrowserFilter::Workflow => item.kind == TaskBrowserKind::Workflow,
            })
            .collect()
    }

    pub fn handle_action(&mut self, action: TaskBrowserAction) -> Option<String> {
        match action {
            TaskBrowserAction::SelectUp => self.move_selection(-1),
            TaskBrowserAction::SelectDown => self.move_selection(1),
            TaskBrowserAction::SelectFirst => self.select_at(0),
            TaskBrowserAction::SelectLast => {
                let len = self.current_len();
                if len > 0 {
                    self.select_at(len - 1);
                }
            }
            TaskBrowserAction::SelectPageUp => {
                if (self.workflow_task_id.is_none()
                    && self.task_details_open
                    && self.focus == TaskBrowserFocus::Output)
                    || (self.workflow_task_id.is_some() && self.child_details_open)
                {
                    self.move_output_scroll(-(PAGE_SIZE as isize));
                } else {
                    self.move_selection(-(PAGE_SIZE as isize));
                }
            }
            TaskBrowserAction::SelectPageDown => {
                if (self.workflow_task_id.is_none()
                    && self.task_details_open
                    && self.focus == TaskBrowserFocus::Output)
                    || (self.workflow_task_id.is_some() && self.child_details_open)
                {
                    self.move_output_scroll(PAGE_SIZE as isize);
                } else {
                    self.move_selection(PAGE_SIZE as isize);
                }
            }
            TaskBrowserAction::ToggleFilter if self.workflow_task_id.is_none() => {
                self.filter = self.filter.next();
                self.list_cursor = None;
                self.list_refresh_requested = true;
                self.reconcile_selection();
            }
            TaskBrowserAction::OpenTaskDetails if self.workflow_task_id.is_none() => {
                if self.selected_item().is_some() {
                    self.task_details_open = true;
                    self.focus = TaskBrowserFocus::Tasks;
                    self.output_scroll = 0;
                }
            }
            TaskBrowserAction::ToggleOutputFocus if self.workflow_task_id.is_none() => {
                if self.selected_item().is_some() {
                    self.task_details_open = true;
                    self.focus = match self.focus {
                        TaskBrowserFocus::Output => TaskBrowserFocus::Tasks,
                        _ => TaskBrowserFocus::Output,
                    };
                }
            }
            TaskBrowserAction::OpenWorkflow => self.open_workflow(),
            TaskBrowserAction::ToggleWorkflowFocus => {
                if self.workflow_task_id.is_some() {
                    self.focus = match self.focus {
                        TaskBrowserFocus::Steps => TaskBrowserFocus::Agents,
                        TaskBrowserFocus::Agents => TaskBrowserFocus::Steps,
                        TaskBrowserFocus::Tasks | TaskBrowserFocus::Output => {
                            TaskBrowserFocus::Steps
                        }
                    };
                }
            }
            TaskBrowserAction::OpenWorkflowChildDetails => {
                if self.focus == TaskBrowserFocus::Agents
                    && self.selected_workflow_child().is_some()
                {
                    self.child_details_open = true;
                    self.output_scroll = 0;
                }
            }
            TaskBrowserAction::TogglePauseResume => {
                let item = self.workflow_item()?;
                if matches!(
                    item.status,
                    TaskBrowserStatus::Running | TaskBrowserStatus::Paused
                ) {
                    return Some(item.id.clone());
                }
                self.footer_message =
                    Some("This workflow cannot be paused or resumed now.".to_owned());
            }
            TaskBrowserAction::RequestStop => {
                let item = self.workflow_item().or_else(|| self.selected_item())?;
                if item.can_stop {
                    self.stop_confirmation_task_id = Some(item.id.clone());
                }
            }
            TaskBrowserAction::ConfirmStop => return self.stop_confirmation_task_id.take(),
            TaskBrowserAction::RequestSave => {
                let workflow = self.workflow_item()?.workflow.as_ref()?;
                if workflow.inline_unsaved {
                    self.save_draft = Some(WorkflowSaveDraft {
                        name: workflow.display_name.clone(),
                        destination: WorkflowSaveDestination::Project,
                        replacement: None,
                    });
                }
            }
            TaskBrowserAction::ToggleSaveDestination => {
                if let Some(draft) = self.save_draft.as_mut() {
                    draft.destination = draft.destination.next();
                }
            }
            TaskBrowserAction::SubmitSave => self.submit_save(),
            TaskBrowserAction::OpenAnswer => self.open_answer(true),
            TaskBrowserAction::DismissAnswer => self.dismiss_answer(),
            TaskBrowserAction::SelectPreviousAnswerField => self.move_answer_field(-1),
            TaskBrowserAction::SelectNextAnswerField => self.move_answer_field(1),
            TaskBrowserAction::SubmitAnswer => self.submit_answer(),
            TaskBrowserAction::RequestNextChildPage => self.next_child_page(),
            TaskBrowserAction::RequestPrevChildPage => self.prev_child_page(),
            TaskBrowserAction::Cancel => {
                if self.stop_confirmation_task_id.take().is_some() {
                    self.footer_message = None;
                } else if self.save_draft.take().is_some() {
                } else if self.answer_draft.is_some() {
                    self.dismiss_answer();
                } else if self.child_details_open {
                    self.child_details_open = false;
                    self.output_scroll = 0;
                } else if self.workflow_task_id.take().is_some() {
                    self.focus = TaskBrowserFocus::Tasks;
                    self.selected_step_key = None;
                    self.selected_child_key = None;
                } else if self.task_details_open {
                    self.task_details_open = false;
                    self.focus = TaskBrowserFocus::Tasks;
                    self.output_scroll = 0;
                } else {
                    return Some(CLOSE_TASK_BROWSER.to_owned());
                }
            }
            TaskBrowserAction::Close => return Some(CLOSE_TASK_BROWSER.to_owned()),
            TaskBrowserAction::ToggleFilter
            | TaskBrowserAction::OpenTaskDetails
            | TaskBrowserAction::ToggleOutputFocus => {}
        }
        None
    }

    fn open_workflow(&mut self) {
        let Some((task_id, selected_step)) = self.selected_item().and_then(|item| {
            (item.kind == TaskBrowserKind::Workflow)
                .then(|| {
                    item.workflow.as_ref().map(|workflow| {
                        (
                            item.id.clone(),
                            workflow
                                .current_step_key
                                .clone()
                                .filter(|key| workflow.steps.iter().any(|step| step.key == *key))
                                .or_else(|| workflow.steps.first().map(|step| step.key.clone())),
                        )
                    })
                })
                .flatten()
        }) else {
            return;
        };
        self.workflow_task_id = Some(task_id);
        self.focus = TaskBrowserFocus::Steps;
        self.task_details_open = false;
        self.output_scroll = 0;
        self.child_details_open = false;
        self.child_cursor = None;
        self.child_prev_cursors.clear();
        self.selected_step_key = selected_step;
        self.reconcile_selection();
        self.child_refresh_requested = true;
    }

    fn open_pending_answer(&mut self) {
        let Some(request) = self
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .and_then(|workflow| workflow.pending_user.as_ref())
            .cloned()
        else {
            return;
        };
        if self.dismissed_request_id.as_deref() != Some(request.request_id.as_str()) {
            self.answer_draft = Some(answer_draft(&request));
        }
    }

    fn open_answer(&mut self, manual: bool) {
        let Some(request) = self
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .and_then(|workflow| workflow.pending_user.as_ref())
            .cloned()
        else {
            self.footer_message = Some("There is no pending answer.".to_owned());
            return;
        };
        if manual {
            self.dismissed_request_id = None;
        }
        self.answer_draft = Some(answer_draft(&request));
    }

    fn dismiss_answer(&mut self) {
        if let Some(draft) = self.answer_draft.take() {
            self.dismissed_request_id = Some(draft.request_id);
        }
    }

    fn submit_save(&mut self) {
        let Some(draft) = self.save_draft.as_ref() else {
            return;
        };
        let name = draft.name.trim();
        if name.is_empty() {
            self.footer_message = Some("A workflow name is required.".to_owned());
            return;
        }
        let Some(task_id) = self.workflow_task_id.clone() else {
            return;
        };
        self.save_submission = Some(WorkflowSaveSubmission {
            task_id,
            name: name.to_owned(),
            destination: draft.destination,
            replace: draft.replacement.is_some(),
        });
        self.save_draft = None;
    }

    fn submit_answer(&mut self) {
        let Some(request) = self
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .and_then(|workflow| workflow.pending_user.as_ref())
            .cloned()
        else {
            return;
        };
        let Some(value) = self
            .answer_draft
            .as_ref()
            .and_then(|draft| draft.field_errors.is_empty().then(|| draft.value.clone()))
        else {
            return;
        };
        match CompiledSchema::compile(&request.answer_schema)
            .and_then(|schema| schema.validate_instance(&value))
        {
            Ok(()) => {
                self.answer_submission = Some(WorkflowAnswerSubmission {
                    task_id: self.workflow_task_id.clone().unwrap_or_default(),
                    request_id: request.request_id.clone(),
                    value,
                });
                self.answer_draft = None;
            }
            Err(error) => {
                if let Some(draft) = self.answer_draft.as_mut() {
                    let path = if error.instance_path.is_empty() {
                        "/".to_owned()
                    } else {
                        error.instance_path
                    };
                    draft.field_errors = vec![format!("{path}: {}", error.message)];
                }
            }
        }
    }

    fn next_child_page(&mut self) {
        let Some(page) = self
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map(|workflow| &workflow.child_page)
        else {
            return;
        };
        let Some(cursor) = page.next_cursor.clone() else {
            return;
        };
        self.child_prev_cursors.push(self.child_cursor.clone());
        self.child_cursor = Some(cursor);
        self.child_refresh_requested = true;
    }

    fn prev_child_page(&mut self) {
        let Some(cursor) = self.child_prev_cursors.pop() else {
            return;
        };
        self.child_cursor = cursor;
        self.child_refresh_requested = true;
    }

    fn reconcile_selection(&mut self) {
        let visible = self.visible_items();
        if !self
            .selected_task_id
            .as_deref()
            .is_some_and(|id| visible.iter().any(|item| item.id == id))
        {
            self.selected_task_id = visible.first().map(|item| item.id.clone());
        }
        let Some((step_keys, child_keys)) = self
            .workflow_item()
            .and_then(|item| item.workflow.as_ref())
            .map(|workflow| {
                (
                    workflow
                        .steps
                        .iter()
                        .map(|step| step.key.clone())
                        .collect::<Vec<_>>(),
                    workflow
                        .child_page
                        .items
                        .iter()
                        .map(|child| child.key.clone())
                        .collect::<Vec<_>>(),
                )
            })
        else {
            return;
        };
        self.selected_step_key = self
            .selected_step_key
            .clone()
            .filter(|key| step_keys.contains(key))
            .or_else(|| step_keys.first().cloned());
        self.selected_child_key = self
            .selected_child_key
            .clone()
            .filter(|key| child_keys.contains(key))
            .or_else(|| child_keys.first().cloned());
    }

    fn move_selection(&mut self, delta: isize) {
        if self.workflow_task_id.is_none() {
            let ids = self
                .visible_items()
                .into_iter()
                .map(|item| item.id.clone())
                .collect::<Vec<_>>();
            move_keyed(&ids, &mut self.selected_task_id, delta, Clone::clone);
        } else if self.focus == TaskBrowserFocus::Steps {
            let steps = self
                .workflow_item()
                .and_then(|item| item.workflow.as_ref())
                .map(|workflow| workflow.steps.clone())
                .unwrap_or_default();
            move_keyed(&steps, &mut self.selected_step_key, delta, |step| {
                step.key.clone()
            });
            self.child_cursor = None;
            self.child_prev_cursors.clear();
            self.child_refresh_requested = true;
        } else {
            let children = self
                .workflow_item()
                .and_then(|item| item.workflow.as_ref())
                .map(|workflow| workflow.child_page.items.clone())
                .unwrap_or_default();
            move_keyed(&children, &mut self.selected_child_key, delta, |child| {
                child.key.clone()
            });
        }
    }

    fn move_output_scroll(&mut self, delta: isize) {
        if self.workflow_task_id.is_some() && self.child_details_open {
            // Workflow Agent Details has no preview lines to clamp against;
            // the renderer clamps the scroll to the wrapped activity rows.
            self.output_scroll = self.output_scroll.saturating_add_signed(delta);
            return;
        }
        let preview_len = self
            .selected_item()
            .map_or(0, |item| item.preview_lines.len());
        self.output_scroll = self
            .output_scroll
            .saturating_add_signed(delta)
            .min(preview_len.saturating_sub(1));
    }

    fn select_at(&mut self, index: usize) {
        let id = self.visible_items().get(index).map(|item| item.id.clone());
        if id.is_some() {
            self.selected_task_id = id;
        }
    }

    fn current_len(&self) -> usize {
        if self.workflow_task_id.is_none() {
            self.visible_items().len()
        } else if self.focus == TaskBrowserFocus::Steps {
            self.workflow_item()
                .and_then(|item| item.workflow.as_ref())
                .map_or(0, |workflow| workflow.steps.len())
        } else {
            self.workflow_item()
                .and_then(|item| item.workflow.as_ref())
                .map_or(0, |workflow| workflow.child_page.items.len())
        }
    }
}

fn answer_draft(request: &TaskBrowserPendingUserRequest) -> WorkflowAnswerDraft {
    let value = request
        .default
        .clone()
        .unwrap_or_else(|| schema_default(&request.answer_schema));
    let form = WorkflowAnswerForm::from_schema(
        &request.answer_schema,
        request.title.clone(),
        request.prompt.clone(),
    );
    let mut branch_indices = BTreeMap::new();
    let mut branch_drafts = BTreeMap::new();
    let mut choice_indices = BTreeMap::new();
    for field in &form.fields {
        choice_indices.insert(field.path.clone(), 0);
        if matches!(field.control, WorkflowAnswerControl::BranchChoice(_)) {
            let selected = field
                .branches
                .iter()
                .position(|schema| {
                    schema_accepts_value(
                        schema,
                        answer_value_at_path(&value, &field.path).unwrap_or(&value),
                    )
                })
                .unwrap_or(0);
            let mut values = field
                .branches
                .iter()
                .map(schema_default)
                .collect::<Vec<_>>();
            if let Some(slot) = values.get_mut(selected) {
                *slot = answer_value_at_path(&value, &field.path)
                    .cloned()
                    .unwrap_or_else(|| value.clone());
            }
            branch_indices.insert(field.path.clone(), selected);
            branch_drafts.insert(field.path.clone(), values);
        }
    }
    WorkflowAnswerDraft {
        request_id: request.request_id.clone(),
        json_editor: serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".to_owned()),
        value,
        field_errors: Vec::new(),
        form,
        selected_field: 0,
        branch_indices,
        branch_drafts,
        choice_indices,
        field_inputs: BTreeMap::new(),
    }
}

fn answer_value_at_path<'a>(
    value: &'a serde_json::Value,
    path: &str,
) -> Option<&'a serde_json::Value> {
    if path.is_empty() || path == "/" {
        Some(value)
    } else {
        value.pointer(path)
    }
}

fn answer_value_at_path_mut<'a>(
    value: &'a mut serde_json::Value,
    path: &str,
) -> Option<&'a mut serde_json::Value> {
    if path.is_empty() || path == "/" {
        Some(value)
    } else {
        value.pointer_mut(path)
    }
}

fn set_answer_draft_value(
    draft: &mut WorkflowAnswerDraft,
    path: &str,
    value: serde_json::Value,
) -> bool {
    let Some(target) = answer_value_at_path_mut(&mut draft.value, path) else {
        return false;
    };
    *target = value;
    if let Some(index) = draft.branch_indices.get(path).copied()
        && let Some(slot) = draft
            .branch_drafts
            .get_mut(path)
            .and_then(|values| values.get_mut(index))
    {
        *slot = target.clone();
    }
    sync_answer_draft(draft);
    true
}

fn sync_answer_draft(draft: &mut WorkflowAnswerDraft) {
    draft.json_editor = serde_json::to_string_pretty(&draft.value).unwrap_or_default();
    draft.field_errors.clear();
}

fn update_answer_field_from_text(
    draft: &mut WorkflowAnswerDraft,
    field: &super::WorkflowAnswerField,
    raw: String,
) -> bool {
    let schema = if matches!(field.control, WorkflowAnswerControl::BranchChoice(_)) {
        let index = draft.branch_indices.get(&field.path).copied().unwrap_or(0);
        field.branches.get(index).unwrap_or(&field.schema)
    } else {
        &field.schema
    };
    let kind = schema.get("type").and_then(serde_json::Value::as_str);
    draft
        .field_inputs
        .insert(answer_field_input_key(draft, field), raw.clone());
    match kind {
        Some("string") => {
            set_answer_draft_value(draft, &field.path, serde_json::Value::String(raw))
        }
        Some("integer") => match raw.parse::<i64>() {
            Ok(value) => set_answer_draft_value(draft, &field.path, serde_json::json!(value)),
            Err(_) => {
                draft.field_errors = vec![format!(
                    "{}: enter a whole number",
                    answer_error_path(&field.path)
                )];
                true
            }
        },
        Some("number") => match raw.parse::<serde_json::Number>() {
            Ok(value) => {
                set_answer_draft_value(draft, &field.path, serde_json::Value::Number(value))
            }
            Err(_) => {
                draft.field_errors = vec![format!(
                    "{}: enter a number",
                    answer_error_path(&field.path)
                )];
                true
            }
        },
        _ => false,
    }
}

fn answer_field_input_key(
    draft: &WorkflowAnswerDraft,
    field: &super::WorkflowAnswerField,
) -> WorkflowAnswerInputKey {
    let branch = if matches!(field.control, WorkflowAnswerControl::BranchChoice(_)) {
        Some((
            field.path.clone(),
            draft.branch_indices.get(&field.path).copied().unwrap_or(0),
        ))
    } else {
        field
            .branch_scope
            .as_ref()
            .map(|scope| (scope.parent_path.clone(), scope.branch_index))
    };
    WorkflowAnswerInputKey {
        path: field.path.clone(),
        branch,
    }
}

fn answer_value_as_input(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| value.to_string())
}

fn answer_error_path(path: &str) -> &str {
    if path.is_empty() { "/" } else { path }
}

fn wrapped_index(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let current = current % len;
    if delta.is_negative() {
        (current + len - 1) % len
    } else {
        (current + 1) % len
    }
}

fn schema_accepts_value(schema: &serde_json::Value, value: &serde_json::Value) -> bool {
    CompiledSchema::compile(schema)
        .and_then(|compiled| compiled.validate_instance(value))
        .is_ok()
}

fn schema_default(schema: &serde_json::Value) -> serde_json::Value {
    if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array) {
        return values.first().cloned().unwrap_or(serde_json::Value::Null);
    }
    for branch in ["oneOf", "anyOf"] {
        if let Some(value) = schema
            .get(branch)
            .and_then(serde_json::Value::as_array)
            .and_then(|values| values.first())
        {
            return schema_default(value);
        }
    }
    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("boolean") => serde_json::Value::Bool(false),
        Some("integer") | Some("number") => serde_json::json!(0),
        Some("array") => serde_json::Value::Array(Vec::new()),
        Some("object") => schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map_or_else(
                || serde_json::Value::Object(serde_json::Map::new()),
                |properties| {
                    serde_json::Value::Object(
                        properties
                            .iter()
                            .map(|(key, value)| (key.clone(), schema_default(value)))
                            .collect(),
                    )
                },
            ),
        _ => serde_json::Value::String(String::new()),
    }
}

fn move_keyed<T, K: Clone + PartialEq>(
    items: &[T],
    selected: &mut Option<K>,
    delta: isize,
    key: impl Fn(&T) -> K,
) {
    let Some(current) = selected.as_ref() else {
        *selected = items.first().map(&key);
        return;
    };
    let index = items
        .iter()
        .position(|item| key(item) == *current)
        .unwrap_or(0);
    let target = index
        .saturating_add_signed(delta)
        .min(items.len().saturating_sub(1));
    *selected = items.get(target).map(key);
}

impl Default for TaskBrowserState {
    fn default() -> Self {
        Self::new()
    }
}
