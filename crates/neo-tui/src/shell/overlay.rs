use super::approval::ApprovalRequestModal;
use super::command_palette::CommandPaletteState;
use super::dialog_dispatch::handle_dialog_selection;
use super::pickers::{ModelPickerState, PromptCompletionState};
use super::session_picker::SessionPickerState;
use crate::primitive::theme::TuiTheme;

use crate::dialogs::QuestionStateMachine;
use crate::dialogs::{
    ApiKeyInputState, ChoicePickerState, CustomRegistryImportState, HelpPanelState,
    McpAddFormState, McpManagerState, ModelSelectorState, ProviderManagerState,
    TabbedModelSelectorState, TextInputState, TrustDialogState,
};
use crate::input::KeybindingAction;
use crate::tasks_browser::{TaskBrowserRenderer, TaskBrowserState};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OverlayId(u64);

impl OverlayId {
    #[must_use]
    pub(super) const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub id: OverlayId,
    pub title: String,
    pub kind: OverlayKind,
}

impl Overlay {
    #[must_use]
    pub fn new(title: impl Into<String>, kind: OverlayKind) -> Self {
        Self {
            id: OverlayId::default(),
            title: title.into(),
            kind,
        }
    }

    pub fn move_selection_down(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_down();
            return;
        }
        match &mut self.kind {
            OverlayKind::Approval(request) => request.move_down(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_down(),
            kind => handle_dialog_selection(kind, KeybindingAction::SelectDown),
        }
    }

    pub fn move_selection_up(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_up();
            return;
        }
        match &mut self.kind {
            OverlayKind::Approval(request) => request.move_up(),
            OverlayKind::QuestionDialog(state) => state.move_cursor_up(),
            kind => handle_dialog_selection(kind, KeybindingAction::SelectUp),
        }
    }

    pub fn move_selection_page_down(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_page_down();
        } else {
            handle_dialog_selection(&mut self.kind, KeybindingAction::SelectPageDown);
        }
    }

    pub fn move_selection_page_up(&mut self) {
        if let Some(list) = self.kind.list_selection_mut() {
            list.select_page_up();
        } else {
            handle_dialog_selection(&mut self.kind, KeybindingAction::SelectPageUp);
        }
    }

    #[must_use]
    pub(super) fn render_standalone_lines(
        &self,
        width: usize,
        theme: &TuiTheme,
    ) -> Option<Vec<String>> {
        self.kind
            .session_picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width))
    }

    #[must_use]
    pub(super) fn render_full_screen_lines(
        &self,
        width: usize,
        height: usize,
        theme: &TuiTheme,
    ) -> Option<Vec<String>> {
        self.kind.full_screen_lines(width, height, theme)
    }

    #[must_use]
    pub(super) fn render_lines(&self, width: usize, theme: &TuiTheme) -> Vec<String> {
        self.kind
            .picker_lines(width, theme)
            .or_else(|| self.kind.rich_dialog_lines(width))
            .or_else(|| self.kind.message_lines())
            .unwrap_or_default()
    }

    #[must_use]
    pub(super) fn height(&self) -> u16 {
        self.kind
            .compact_height()
            .or_else(|| self.kind.input_dialog_height())
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum OverlayKind {
    CommandPalette(CommandPaletteState),
    SessionPicker(SessionPickerState),
    ModelPicker(ModelPickerState),
    PromptCompletion(PromptCompletionState),
    Approval(ApprovalRequestModal),
    QuestionDialog(QuestionStateMachine),
    Message(String),
    // Neo rich dialogs
    ModelSelector(ModelSelectorState),
    TabbedModelSelector(TabbedModelSelectorState),
    ProviderManager(ProviderManagerState),
    McpManager(McpManagerState),
    McpAddForm(McpAddFormState),
    ChoicePicker(ChoicePickerState),
    ApiKeyInput(ApiKeyInputState),
    TextInput(TextInputState),
    CustomRegistryImport(CustomRegistryImportState),
    TrustDialog(TrustDialogState),
    HelpPanel(HelpPanelState),
    TaskBrowser(TaskBrowserState),
}

impl OverlayKind {
    fn list_selection_mut(&mut self) -> Option<OverlayListSelection<'_>> {
        match self {
            Self::CommandPalette(state) => Some(OverlayListSelection::CommandPalette(state)),
            Self::SessionPicker(state) => Some(OverlayListSelection::SessionPicker(state)),
            _ => self.secondary_list_selection_mut(),
        }
    }

    fn secondary_list_selection_mut(&mut self) -> Option<OverlayListSelection<'_>> {
        match self {
            Self::ModelPicker(state) => Some(OverlayListSelection::ModelPicker(state)),
            Self::PromptCompletion(state) => Some(OverlayListSelection::PromptCompletion(state)),
            _ => None,
        }
    }

    #[must_use]
    fn session_picker_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<String>> {
        let Self::SessionPicker(picker) = self else {
            return None;
        };
        Some(picker.render_lines(width, theme))
    }

    #[must_use]
    fn picker_lines(&self, width: usize, theme: &TuiTheme) -> Option<Vec<String>> {
        match self {
            Self::CommandPalette(palette) => Some(palette.render_lines(width)),
            Self::SessionPicker(_) => self.session_picker_lines(width, theme),
            Self::ModelPicker(picker) => Some(picker.render_lines(width)),
            Self::PromptCompletion(completions) => Some(completions.render_lines(width)),
            _ => None,
        }
    }

    #[must_use]
    fn rich_dialog_lines(&self, width: usize) -> Option<Vec<String>> {
        match self {
            Self::ModelSelector(state) => Some(state.render_lines(width)),
            Self::TabbedModelSelector(state) => Some(state.render_lines(width)),
            Self::ProviderManager(state) => Some(state.render_lines(width)),
            Self::McpManager(state) => Some(state.render_lines(width)),
            Self::HelpPanel(state) => Some(state.render_lines(width)),
            _ => self.input_dialog_lines(width),
        }
    }

    #[must_use]
    fn input_dialog_lines(&self, width: usize) -> Option<Vec<String>> {
        match self {
            Self::ChoicePicker(state) => Some(state.render_lines(width)),
            Self::ApiKeyInput(state) => Some(state.render_lines(width)),
            Self::TextInput(state) => Some(state.render_lines(width)),
            Self::CustomRegistryImport(state) => Some(state.render_lines(width)),
            Self::McpAddForm(state) => Some(state.render_lines(width)),
            Self::TrustDialog(state) => Some(state.render_lines(width)),
            _ => None,
        }
    }

    #[must_use]
    fn full_screen_lines(
        &self,
        width: usize,
        height: usize,
        theme: &TuiTheme,
    ) -> Option<Vec<String>> {
        let Self::TaskBrowser(state) = self else {
            return None;
        };
        Some(TaskBrowserRenderer::new(state, *theme).render(width, height))
    }

    #[must_use]
    fn message_lines(&self) -> Option<Vec<String>> {
        let Self::Message(text) = self else {
            return None;
        };
        Some(vec![text.clone()])
    }

    #[must_use]
    fn compact_height(&self) -> Option<u16> {
        match self {
            Self::CommandPalette(_) => Some(12),
            Self::PromptCompletion(_) | Self::Approval(_) => Some(8),
            Self::Message(_) => Some(3),
            _ => None,
        }
    }

    #[must_use]
    fn input_dialog_height(&self) -> Option<u16> {
        match self {
            Self::ApiKeyInput(_) | Self::TextInput(_) | Self::CustomRegistryImport(_) => Some(10),
            Self::SessionPicker(_)
            | Self::ModelPicker(_)
            | Self::QuestionDialog(_)
            | Self::ModelSelector(_)
            | Self::TabbedModelSelector(_)
            | Self::ProviderManager(_)
            | Self::McpManager(_)
            | Self::McpAddForm(_)
            | Self::ChoicePicker(_)
            | Self::TrustDialog(_)
            | Self::HelpPanel(_) => Some(16),
            Self::TaskBrowser(_) => Some(0),
            _ => None,
        }
    }
}

enum OverlayListSelection<'a> {
    CommandPalette(&'a mut CommandPaletteState),
    SessionPicker(&'a mut SessionPickerState),
    ModelPicker(&'a mut ModelPickerState),
    PromptCompletion(&'a mut PromptCompletionState),
}

impl OverlayListSelection<'_> {
    fn select_up(self) {
        match self {
            Self::CommandPalette(state) => state.move_up(),
            Self::SessionPicker(state) => state.move_up(),
            Self::ModelPicker(state) => state.move_up(),
            Self::PromptCompletion(state) => state.move_up(),
        }
    }

    fn select_down(self) {
        match self {
            Self::CommandPalette(state) => state.move_down(),
            Self::SessionPicker(state) => state.move_down(),
            Self::ModelPicker(state) => state.move_down(),
            Self::PromptCompletion(state) => state.move_down(),
        }
    }

    fn select_page_up(self) {
        match self {
            Self::CommandPalette(state) => state.page_up(),
            Self::SessionPicker(state) => state.page_up(),
            Self::ModelPicker(state) => state.page_up(),
            Self::PromptCompletion(state) => state.page_up(),
        }
    }

    fn select_page_down(self) {
        match self {
            Self::CommandPalette(state) => state.page_down(),
            Self::SessionPicker(state) => state.page_down(),
            Self::ModelPicker(state) => state.page_down(),
            Self::PromptCompletion(state) => state.page_down(),
        }
    }
}
