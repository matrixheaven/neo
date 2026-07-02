use super::select_list::{SelectItem, SelectListState};
use crate::primitive::theme::TuiTheme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub id: String,
    pub label: String,
    pub description: Option<String>,
}

impl CommandSpec {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: Option<impl Into<String>>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.map(Into::into),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPaletteState {
    list: SelectListState,
}

impl CommandPaletteState {
    #[must_use]
    pub fn new(commands: impl IntoIterator<Item = CommandSpec>) -> Self {
        Self {
            list: SelectListState::new(commands.into_iter().map(select_from_command), 8),
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
    pub fn selected_command(&self) -> Option<CommandSpec> {
        self.list.selected_item().map(command_from_select_item)
    }

    #[must_use]
    pub fn confirm(&self) -> Option<CommandSpec> {
        self.selected_command()
    }

    #[must_use]
    pub fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        self.list.render_lines(width, theme)
    }
}

fn select_from_command(command: CommandSpec) -> SelectItem {
    SelectItem::new(command.id, command.label, command.description)
}

fn command_from_select_item(item: &SelectItem) -> CommandSpec {
    CommandSpec {
        id: item.value.clone(),
        label: item.label.clone(),
        description: item.description.clone(),
    }
}
