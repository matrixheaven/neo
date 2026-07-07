pub mod api_key_input;
pub mod choice_picker;
pub mod confirm_dialog;
pub mod custom_registry_import;
pub mod help_panel;
pub mod mcp_add_form;
pub mod mcp_manager;
pub mod model_selector;
pub mod provider_manager;
pub mod question_dialog;
pub mod tabbed_model_selector;
pub mod text_input;
pub mod trust;
pub mod workspace_manager;

pub use api_key_input::{ApiKeyInputOptions, ApiKeyInputResult, ApiKeyInputState};
pub use choice_picker::{ChoiceItem, ChoicePickerOptions, ChoicePickerState, ChoiceResult};
pub use confirm_dialog::{ConfirmDialogOptions, ConfirmDialogResult, ConfirmDialogState};
pub use custom_registry_import::{
    CustomRegistryImportOptions, CustomRegistryImportResult, CustomRegistryImportState,
    CustomRegistrySource,
};
pub use help_panel::{HelpPanelCommand, HelpPanelOptions, HelpPanelState};
pub use mcp_add_form::{McpAddFormData, McpAddFormOptions, McpAddFormResult, McpAddFormState};
pub use mcp_manager::{
    McpManagerAction, McpManagerOptions, McpManagerState, McpServerRow, McpToolStatus,
};
pub use model_selector::{
    ModelEntry, ModelSelection, ModelSelectorOptions, ModelSelectorResult, ModelSelectorState,
};
pub use provider_manager::{
    ProviderManagerAction, ProviderManagerOptions, ProviderManagerState, ProviderSource,
    ProviderSourceKind,
};
pub use question_dialog::{
    QuestionDialogAction, QuestionDisplayData, QuestionDisplayOption, QuestionOptionState,
    QuestionResult, QuestionState, QuestionStateMachine,
};
pub use tabbed_model_selector::{TabbedModelSelectorOptions, TabbedModelSelectorState};
pub use text_input::{TextInputOptions, TextInputResult, TextInputState};
pub use trust::{
    TrustDialogChoice, TrustDialogData, TrustDialogInput, TrustDialogInputKind, TrustDialogResult,
    TrustDialogState,
};
pub use workspace_manager::{
    WorkspaceManagerAction, WorkspaceManagerOptions, WorkspaceManagerState, WorkspaceRow,
};
// Compatibility alias used by the neo-agent trust module.
pub use trust::TrustDialogInputKind as TrustInputKind;
