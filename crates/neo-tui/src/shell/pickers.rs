use super::select_list::{SelectItem, SelectListState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompletionPrefix {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCompletionState {
    prefix: PromptCompletionPrefix,
    picker: PickerState,
}

impl PromptCompletionState {
    #[must_use]
    pub fn new(
        prefix: PromptCompletionPrefix,
        items: impl IntoIterator<Item = PickerItem>,
    ) -> Self {
        Self {
            prefix,
            picker: PickerState::new(items),
        }
    }

    #[must_use]
    pub const fn prefix(&self) -> &PromptCompletionPrefix {
        &self.prefix
    }

    pub fn move_up(&mut self) {
        self.picker.move_up();
    }

    pub fn move_down(&mut self) {
        self.picker.move_down();
    }

    pub fn page_up(&mut self) {
        self.picker.page_up();
    }

    pub fn page_down(&mut self) {
        self.picker.page_down();
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<PickerItem> {
        self.picker.selected_item()
    }

    #[must_use]
    pub fn confirm(&self) -> Option<PickerItem> {
        self.picker.confirm()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.picker.render_lines(width)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

impl PickerItem {
    #[must_use]
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

impl From<PickerItem> for SelectItem {
    fn from(item: PickerItem) -> Self {
        Self::new(item.value, item.label, item.description)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    list: SelectListState,
}

impl PickerState {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = PickerItem>) -> Self {
        Self::new_with_visible(items, 8)
    }

    #[must_use]
    pub fn new_with_visible(
        items: impl IntoIterator<Item = PickerItem>,
        max_visible: usize,
    ) -> Self {
        Self {
            list: SelectListState::new(items.into_iter().map(SelectItem::from), max_visible),
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.list.set_filter(filter);
    }

    pub fn move_up(&mut self) {
        self.list.move_up();
    }

    pub fn move_down(&mut self) {
        self.list.move_down();
    }

    pub fn page_up(&mut self) {
        self.list.page_up();
    }

    pub fn page_down(&mut self) {
        self.list.page_down();
    }

    #[must_use]
    pub const fn list(&self) -> &SelectListState {
        &self.list
    }

    #[must_use]
    pub fn selected_item(&self) -> Option<PickerItem> {
        self.list.selected_item().map(picker_from_select_item)
    }

    #[must_use]
    pub fn selected_model(&self) -> Option<PickerItem> {
        self.selected_item()
    }

    #[must_use]
    pub fn confirm(&self) -> Option<PickerItem> {
        self.selected_item()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        self.list.render_lines(width)
    }
}

fn picker_from_select_item(item: &SelectItem) -> PickerItem {
    PickerItem {
        value: item.value.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
    }
}

pub type ModelPickerState = PickerState;
