use neo_ai::{ApiType, ReasoningBudget, ReasoningCapability, ReasoningEffort};

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{InputResult, Style, paint, truncate_width};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomEndpointAuthDraft {
    EnvVar(String),
    InlineSecret(String),
    LocalPlaceholder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEndpointProviderDraft {
    pub display_name: String,
    pub provider_id: String,
    pub api_type: ApiType,
    pub base_url: String,
    pub auth: CustomEndpointAuthDraft,
    pub models: Vec<CustomEndpointModelDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEndpointModelDraft {
    pub source: CustomEndpointModelSource,
    pub model_id: String,
    pub alias: String,
    pub display_name: Option<String>,
    pub max_context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub streaming: bool,
    pub tools: bool,
    pub images: bool,
    pub embeddings: bool,
    pub reasoning: ReasoningCapability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomEndpointModelSource {
    Manual,
    Fetched {
        owned_by: Option<String>,
        created: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEndpointFetchedModel {
    pub id: String,
    pub owned_by: Option<String>,
    pub created: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomEndpointWizardAction {
    FetchModels,
    TestConnection(CustomEndpointProviderDraft),
    Save(CustomEndpointProviderDraft),
    Cancelled,
}

pub struct CustomEndpointWizardOptions {
    pub theme: TuiTheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardStep {
    Provider,
    ApiType,
    EndpointAuth,
    AuthSource,
    ModelSource,
    FetchSelect,
    ModelIdentity,
    ModelCapabilities,
    ReasoningType,
    ReasoningEffort,
    ReasoningBudget,
    ReasoningCombined,
    AddedModels,
    Review,
    TestResult,
    ValidationError,
    Saved,
}

const RENDERABLE_STEPS: [WizardStep; 17] = [
    WizardStep::Provider,
    WizardStep::ApiType,
    WizardStep::EndpointAuth,
    WizardStep::AuthSource,
    WizardStep::ModelSource,
    WizardStep::FetchSelect,
    WizardStep::ModelIdentity,
    WizardStep::ModelCapabilities,
    WizardStep::ReasoningType,
    WizardStep::ReasoningEffort,
    WizardStep::ReasoningBudget,
    WizardStep::ReasoningCombined,
    WizardStep::AddedModels,
    WizardStep::Review,
    WizardStep::TestResult,
    WizardStep::ValidationError,
    WizardStep::Saved,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomEndpointWizardState {
    theme: TuiTheme,
    step: WizardStep,
    selected: usize,
    display_name: String,
    provider_id: String,
    api_type: ApiType,
    base_url: String,
    auth_source: CustomEndpointAuthDraft,
    models: Vec<CustomEndpointModelDraft>,
    draft_model: CustomEndpointModelDraft,
    fetched_models: Vec<CustomEndpointFetchedModel>,
    fetched_selected: Vec<bool>,
    pending_fetched_review: Vec<usize>,
    action: Option<CustomEndpointWizardAction>,
    validation_error: Option<String>,
}

impl CustomEndpointWizardState {
    #[must_use]
    pub fn new(opts: CustomEndpointWizardOptions) -> Self {
        Self {
            theme: opts.theme,
            step: WizardStep::Provider,
            selected: 0,
            display_name: String::new(),
            provider_id: String::new(),
            api_type: ApiType::OpenAi,
            base_url: String::new(),
            auth_source: CustomEndpointAuthDraft::EnvVar(String::new()),
            models: Vec::new(),
            draft_model: Self::empty_model("", ""),
            fetched_models: Vec::new(),
            fetched_selected: Vec::new(),
            pending_fetched_review: Vec::new(),
            action: None,
            validation_error: None,
        }
    }

    fn empty_model(provider_id: &str, model_id: &str) -> CustomEndpointModelDraft {
        let alias = if provider_id.is_empty() || model_id.is_empty() {
            String::new()
        } else {
            format!("{provider_id}/{model_id}")
        };
        CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Manual,
            model_id: model_id.to_owned(),
            alias,
            display_name: None,
            max_context_tokens: None,
            max_output_tokens: None,
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::None,
        }
    }

    #[must_use]
    pub fn action(&self) -> Option<&CustomEndpointWizardAction> {
        self.action.as_ref()
    }

    pub fn take_action(&mut self) -> Option<CustomEndpointWizardAction> {
        self.action.take()
    }

    pub fn apply_fetched_models(&mut self, models: Vec<CustomEndpointFetchedModel>) {
        self.fetched_selected = vec![true; models.len()];
        self.fetched_models = models;
        self.pending_fetched_review.clear();
        self.step = WizardStep::FetchSelect;
        self.selected = 0;
    }

    pub fn apply_test_result(&mut self, result: Result<(), String>) {
        self.step = WizardStep::TestResult;
        self.validation_error = result.err();
        self.selected = 0;
    }

    pub fn queue_selected_fetched_models_for_review(&mut self) -> bool {
        let mut selected: Vec<usize> = self
            .fetched_selected
            .iter()
            .enumerate()
            .filter_map(|(index, selected)| selected.then_some(index))
            .filter(|index| self.fetched_models.get(*index).is_some())
            .collect();
        if selected.is_empty() {
            return false;
        }

        let first = selected.remove(0);
        self.pending_fetched_review = selected;
        self.load_fetched_model_for_review(first);
        self.step = WizardStep::ModelIdentity;
        self.selected = 0;
        true
    }

    fn load_fetched_model_for_review(&mut self, index: usize) {
        if let Some(model) = self.fetched_models.get(index) {
            let mut draft = Self::empty_model(&self.provider_id, &model.id);
            draft.source = CustomEndpointModelSource::Fetched {
                owned_by: model.owned_by.clone(),
                created: model.created,
            };
            self.draft_model = draft;
        }
    }

    fn provider_step_error(&self) -> Option<String> {
        if self.display_name.trim().is_empty() {
            return Some("Display name is required.".to_owned());
        }
        if !valid_provider_id(self.provider_id.trim()) {
            return Some(
                "Provider id must be lowercase ascii letters, digits, '_' or '-'.".to_owned(),
            );
        }
        None
    }

    fn endpoint_step_error(&self) -> Option<String> {
        if self.base_url.trim().is_empty() {
            return Some("Base URL is required.".to_owned());
        }
        match &self.auth_source {
            CustomEndpointAuthDraft::EnvVar(value) if value.trim().is_empty() => {
                Some("Environment variable name is required.".to_owned())
            }
            CustomEndpointAuthDraft::InlineSecret(value) if value.trim().is_empty() => {
                Some("Inline API key is required.".to_owned())
            }
            CustomEndpointAuthDraft::EnvVar(_)
            | CustomEndpointAuthDraft::InlineSecret(_)
            | CustomEndpointAuthDraft::LocalPlaceholder => None,
        }
    }

    fn model_step_error(model: &CustomEndpointModelDraft) -> Option<String> {
        if model.model_id.trim().is_empty() {
            return Some("Model id is required.".to_owned());
        }
        if model.alias.trim().is_empty() {
            return Some("Model alias is required.".to_owned());
        }
        if let ReasoningCapability::Effort { values, .. } = &model.reasoning
            && values.is_empty()
        {
            return Some("Effort reasoning needs at least one value.".to_owned());
        }
        None
    }

    fn draft(&self) -> CustomEndpointProviderDraft {
        CustomEndpointProviderDraft {
            display_name: self.display_name.trim().to_owned(),
            provider_id: self.provider_id.trim().to_owned(),
            api_type: self.api_type,
            base_url: self.base_url.trim().trim_end_matches('/').to_owned(),
            auth: self.auth_source.clone(),
            models: self.models.clone(),
        }
    }

    #[must_use]
    pub fn current_draft(&self) -> CustomEndpointProviderDraft {
        self.draft()
    }

    fn handle_text_input(&mut self, input: &InputEvent) -> Option<InputResult> {
        match input {
            InputEvent::Insert(ch) => self.edit_current_text(TextEdit::Insert(*ch)),
            InputEvent::Paste(text) => self.edit_current_text(TextEdit::Paste(text)),
            InputEvent::Backspace => self.edit_current_text(TextEdit::Backspace),
            _ => None,
        }
    }

    fn edit_current_text(&mut self, edit: TextEdit<'_>) -> Option<InputResult> {
        match self.step {
            WizardStep::Provider => match self.selected {
                0 => Some(apply_text_edit(&mut self.display_name, edit, CharSet::Text)),
                1 => Some(apply_text_edit(&mut self.provider_id, edit, CharSet::Graph)),
                _ => None,
            },
            WizardStep::EndpointAuth => match self.selected {
                0 => Some(apply_text_edit(&mut self.base_url, edit, CharSet::Graph)),
                2 => match &mut self.auth_source {
                    CustomEndpointAuthDraft::EnvVar(value)
                    | CustomEndpointAuthDraft::InlineSecret(value) => {
                        Some(apply_text_edit(value, edit, CharSet::Graph))
                    }
                    CustomEndpointAuthDraft::LocalPlaceholder => None,
                },
                _ => None,
            },
            WizardStep::ModelIdentity => match self.selected {
                0 => {
                    let previous_model_id = self.draft_model.model_id.clone();
                    let update_alias = self.draft_model.alias.is_empty()
                        || self.draft_model.alias
                            == auto_alias(&self.provider_id, &previous_model_id);
                    let result =
                        apply_text_edit(&mut self.draft_model.model_id, edit, CharSet::Graph);
                    if update_alias {
                        self.draft_model.alias =
                            auto_alias(&self.provider_id, &self.draft_model.model_id);
                    }
                    Some(result)
                }
                1 => Some(apply_text_edit(
                    &mut self.draft_model.alias,
                    edit,
                    CharSet::Graph,
                )),
                2 => {
                    let value = self
                        .draft_model
                        .display_name
                        .get_or_insert_with(String::new);
                    let result = apply_text_edit(value, edit, CharSet::Text);
                    if value.is_empty() {
                        self.draft_model.display_name = None;
                    }
                    Some(result)
                }
                3 => Some(apply_u32_edit(
                    &mut self.draft_model.max_context_tokens,
                    edit,
                )),
                4 => Some(apply_u32_edit(
                    &mut self.draft_model.max_output_tokens,
                    edit,
                )),
                _ => None,
            },
            WizardStep::ReasoningBudget => match self.selected {
                0 | 1 => Some(self.edit_reasoning_budget_value(edit)),
                _ => None,
            },
            _ => None,
        }
    }

    fn edit_reasoning_budget_value(&mut self, edit: TextEdit<'_>) -> InputResult {
        let selected = self.selected;
        match &mut self.draft_model.reasoning {
            ReasoningCapability::BudgetTokens { min, max, .. } => {
                apply_u32_edit(if selected == 0 { min } else { max }, edit)
            }
            ReasoningCapability::Combined { budget, .. } => {
                let budget = budget.get_or_insert(ReasoningBudget {
                    min: None,
                    max: None,
                });
                apply_u32_edit(
                    if selected == 0 {
                        &mut budget.min
                    } else {
                        &mut budget.max
                    },
                    edit,
                )
            }
            ReasoningCapability::None
            | ReasoningCapability::Toggle { .. }
            | ReasoningCapability::Effort { .. } => InputResult::Ignored,
        }
    }

    fn submit_review(&mut self) -> InputResult {
        if self.models.is_empty() {
            self.action = None;
            return self.validation_error("Add at least one model before saving.");
        }
        self.action = Some(CustomEndpointWizardAction::Save(self.draft()));
        InputResult::Submitted
    }

    fn validation_error(&mut self, message: impl Into<String>) -> InputResult {
        self.action = None;
        self.validation_error = Some(message.into());
        self.step = WizardStep::ValidationError;
        self.selected = 0;
        InputResult::Handled
    }

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        debug_assert!(RENDERABLE_STEPS.contains(&self.step));
        match self.step {
            WizardStep::Provider => self.render_provider(width),
            WizardStep::ApiType => self.render_api_type(width),
            WizardStep::EndpointAuth => self.render_endpoint_auth(width),
            WizardStep::AuthSource => self.render_auth_source(width),
            WizardStep::ModelSource => self.render_model_source(width),
            WizardStep::FetchSelect => self.render_fetch_select(width),
            WizardStep::ModelIdentity => self.render_model_identity(width),
            WizardStep::ModelCapabilities => self.render_model_capabilities(width),
            WizardStep::ReasoningType => self.render_reasoning_type(width),
            WizardStep::ReasoningEffort => self.render_reasoning_effort(width),
            WizardStep::ReasoningBudget => self.render_reasoning_budget(width),
            WizardStep::ReasoningCombined => self.render_reasoning_combined(width),
            WizardStep::AddedModels => self.render_added_models(width),
            WizardStep::Review => self.render_review(width),
            WizardStep::TestResult => self.render_test_result(width),
            WizardStep::ValidationError => self.render_validation_error(width),
            WizardStep::Saved => self.render_saved(width),
        }
    }

    fn selection_marker(&self, index: usize) -> &'static str {
        if index == self.selected { "▸" } else { " " }
    }

    fn field_label(&self, index: usize, label: &str) -> String {
        let selected = index == self.selected;
        let marker_style = Style::default().fg(if selected {
            self.theme.brand
        } else {
            self.theme.text_muted
        });
        let label_style = if selected {
            Style::default().fg(self.theme.brand).bold()
        } else {
            Style::default().fg(self.theme.text_primary)
        };
        format!(
            "{} {}",
            paint(self.selection_marker(index), marker_style),
            paint(label, label_style)
        )
    }

    fn render_provider(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "Custom Endpoint 1/4 · Provider",
            vec![
                section_line("Provider", self.theme),
                self.field_label(0, "Display name"),
                input_value_line(
                    &self.display_name,
                    "Name shown in Neo",
                    self.selected == 0,
                    self.theme,
                ),
                String::new(),
                self.field_label(1, "Provider id"),
                input_value_line(
                    &self.provider_id,
                    "lowercase-id-used-in-config",
                    self.selected == 1,
                    self.theme,
                ),
                String::new(),
                self.field_label(2, "API type"),
                format!("  {}  ›", api_type_label(self.api_type)),
                String::new(),
                hint_line(
                    "↑/↓ select · Tab field · Enter continue · Esc cancel",
                    self.theme,
                ),
            ],
        )
    }

    fn render_api_type(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "API Type",
            api_type_rows()
                .iter()
                .enumerate()
                .map(|(index, (label, value))| {
                    let marker = if index == self.selected { "▸" } else { " " };
                    format!("{marker} {label:<21} type = \"{value}\"")
                })
                .chain([
                    "".to_owned(),
                    hint_line("↑/↓ select · Enter choose · Esc back", self.theme),
                ])
                .collect(),
        )
    }

    fn render_endpoint_auth(&self, width: usize) -> Vec<String> {
        let auth_label = match self.auth_source {
            CustomEndpointAuthDraft::EnvVar(_) => "Environment variable",
            CustomEndpointAuthDraft::InlineSecret(_) => "Paste secret",
            CustomEndpointAuthDraft::LocalPlaceholder => "Local placeholder",
        };
        let auth_value_line = match &self.auth_source {
            CustomEndpointAuthDraft::EnvVar(value) => input_value_line(
                value,
                "ENV_VAR_WITH_API_KEY",
                self.selected == 2,
                self.theme,
            ),
            CustomEndpointAuthDraft::InlineSecret(value) => {
                if value.is_empty() {
                    input_value_line("", "Paste API key", self.selected == 2, self.theme)
                } else if self.selected == 2 {
                    format!("  ********{}", input_cursor(self.theme))
                } else {
                    "  ********".to_owned()
                }
            }
            CustomEndpointAuthDraft::LocalPlaceholder => "  local".to_owned(),
        };
        self.render_box(
            width,
            "Custom Endpoint 2/4 · Endpoint & Auth",
            vec![
                section_line("Endpoint", self.theme),
                self.field_label(0, "Base URL"),
                input_value_line(
                    &self.base_url,
                    "Endpoint base URL (usually ends with /v1)",
                    self.selected == 0,
                    self.theme,
                ),
                String::new(),
                section_line("Auth", self.theme),
                self.field_label(1, "API key source"),
                format!("  {auth_label}  ›"),
                String::new(),
                self.field_label(2, auth_field_label(&self.auth_source)),
                auth_value_line,
                String::new(),
                hint_line(
                    "↑/↓ select · Tab field · Enter continue · Esc back",
                    self.theme,
                ),
            ],
        )
    }

    fn render_auth_source(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "API Key Source",
            vec![
                format!(
                    "{} Environment variable     writes api_key_env",
                    self.selection_marker(0)
                ),
                format!(
                    "{} Paste secret             writes api_key",
                    self.selection_marker(1)
                ),
                format!(
                    "{} Local placeholder        writes api_key = \"local\"",
                    self.selection_marker(2)
                ),
                String::new(),
                hint_line("↑/↓ select · Enter choose · Esc back", self.theme),
            ],
        )
    }

    fn render_model_source(&self, width: usize) -> Vec<String> {
        let fetch_label = if is_openai_family(self.api_type) {
            "Fetch from /models     OpenAI-family model IDs"
        } else {
            "Fetch from /models     OpenAI-family only"
        };
        self.render_box(
            width,
            "Custom Endpoint 3/4 · Models",
            vec![
                section_line("How should Neo add models?", self.theme),
                String::new(),
                format!("{} {fetch_label}", self.selection_marker(0)),
                format!(
                    "{} Enter manually        Add model ID and capabilities",
                    self.selection_marker(1)
                ),
                String::new(),
                hint_line("↑/↓ select · Enter continue · Esc back", self.theme),
            ],
        )
    }

    fn render_fetch_select(&self, width: usize) -> Vec<String> {
        let mut content = vec![
            format!("{} models found", self.fetched_models.len()),
            String::new(),
        ];
        if self.fetched_models.is_empty() {
            content.push("No models fetched yet".to_owned());
        } else {
            for (index, model) in self.fetched_models.iter().enumerate().take(8) {
                let marker = if self.fetched_selected.get(index).copied().unwrap_or(false) {
                    "▣"
                } else {
                    "☐"
                };
                content.push(format!(
                    "{} {marker} {}",
                    self.selection_marker(index),
                    model.id
                ));
            }
        }
        content.extend([
            String::new(),
            "↑/↓ select · Space toggle · Enter review config".to_owned(),
            "/ filter · Esc back".to_owned(),
        ]);
        self.render_box(width, "Custom Endpoint 3/4 · Select Models", content)
    }

    fn render_model_identity(&self, width: usize) -> Vec<String> {
        let title = if matches!(
            self.draft_model.source,
            CustomEndpointModelSource::Fetched { .. }
        ) {
            "Custom Endpoint 3/4 · Review Model 1"
        } else {
            "Custom Endpoint 3/4 · Model 1"
        };
        let mut content = Vec::new();
        if let CustomEndpointModelSource::Fetched { owned_by, created } = &self.draft_model.source {
            content.push("Source: /models".to_owned());
            content.push(format!("  id = \"{}\"", self.draft_model.model_id));
            if let Some(owned_by) = owned_by {
                content.push(format!("  owned_by = \"{owned_by}\""));
            }
            if let Some(created) = created {
                content.push(format!("  created = {created}"));
            }
            content.push(String::new());
        }
        content.extend([
            section_line("Model", self.theme),
            self.field_label(0, "Model id"),
            input_value_line(
                &self.draft_model.model_id,
                "Model id from provider",
                self.selected == 0,
                self.theme,
            ),
            String::new(),
            self.field_label(1, "Alias"),
            input_value_line(
                &self.draft_model.alias,
                "provider-id/model-id",
                self.selected == 1,
                self.theme,
            ),
            String::new(),
            self.field_label(2, "Display name"),
            input_value_line(
                self.draft_model.display_name.as_deref().unwrap_or_default(),
                "Optional display name",
                self.selected == 2,
                self.theme,
            ),
            String::new(),
            section_line("Limits", self.theme),
            self.field_label(3, "Context tokens"),
            input_value_line(
                &optional_u32_input_value(self.draft_model.max_context_tokens),
                "Unset context limit",
                self.selected == 3,
                self.theme,
            ),
            String::new(),
            self.field_label(4, "Output tokens"),
            input_value_line(
                &optional_u32_input_value(self.draft_model.max_output_tokens),
                "Unset output limit",
                self.selected == 4,
                self.theme,
            ),
            String::new(),
            hint_line(
                "↑/↓ select · Tab field · Enter capabilities · Esc back",
                self.theme,
            ),
        ]);
        self.render_box(width, title, content)
    }

    fn render_model_capabilities(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "Custom Endpoint 3/4 · Model Capabilities",
            vec![
                value_or_placeholder(
                    &self.draft_model.alias,
                    "Model alias will appear here",
                    self.theme,
                ),
                String::new(),
                format!(
                    "{} [{}] streaming     Server can stream tokens",
                    self.selection_marker(0),
                    mark(self.draft_model.streaming)
                ),
                format!(
                    "{} [{}] tools         Model supports tool calls",
                    self.selection_marker(1),
                    mark(self.draft_model.tools)
                ),
                format!(
                    "{} [{}] images        Model accepts image input",
                    self.selection_marker(2),
                    mark(self.draft_model.images)
                ),
                format!(
                    "{} [{}] embeddings    Model is an embedding model",
                    self.selection_marker(3),
                    mark(self.draft_model.embeddings)
                ),
                String::new(),
                format!(
                    "{} Reasoning         {}  ›",
                    self.selection_marker(4),
                    reasoning_label(&self.draft_model.reasoning)
                ),
                String::new(),
                "↑/↓ select · Space toggle · Enter continue · Esc back".to_owned(),
            ],
        )
    }

    fn render_reasoning_type(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "Reasoning",
            vec![
                format!(
                    "{} None               No reasoning controls",
                    self.selection_marker(0)
                ),
                format!(
                    "{} Toggle             On/off only",
                    self.selection_marker(1)
                ),
                format!(
                    "{} Effort             minimal, low, medium, high, xhigh, max",
                    self.selection_marker(2)
                ),
                format!(
                    "{} Budget tokens      min/max token budget",
                    self.selection_marker(3)
                ),
                format!(
                    "{} Combined           toggle, effort, and budget",
                    self.selection_marker(4)
                ),
                String::new(),
                "↑/↓ select · Enter choose · Esc back".to_owned(),
            ],
        )
    }

    fn render_reasoning_effort(&self, width: usize) -> Vec<String> {
        let values = reasoning_effort_values(&self.draft_model.reasoning);
        self.render_box(
            width,
            "Reasoning Effort",
            all_reasoning_efforts()
                .iter()
                .enumerate()
                .map(|(index, effort)| {
                    format!(
                        "{} [{}] {}",
                        self.selection_marker(index),
                        mark(values.contains(effort)),
                        effort.as_str()
                    )
                })
                .chain([
                    String::new(),
                    "↑/↓ select · Space toggle · Enter choose · Esc back".to_owned(),
                ])
                .collect(),
        )
    }

    fn render_reasoning_budget(&self, width: usize) -> Vec<String> {
        let budget = reasoning_budget(&self.draft_model.reasoning);
        let disable_supported = reasoning_disable_supported(&self.draft_model.reasoning);
        self.render_box(
            width,
            "Reasoning Budget",
            vec![
                format!(
                    "{} Minimum tokens     {}",
                    self.selection_marker(0),
                    optional_u32_input_value(budget.as_ref().and_then(|budget| budget.min))
                ),
                format!(
                    "{} Maximum tokens     {}",
                    self.selection_marker(1),
                    optional_u32_input_value(budget.as_ref().and_then(|budget| budget.max))
                ),
                format!(
                    "{} Disable supported  {}",
                    self.selection_marker(2),
                    yes_no(disable_supported)
                ),
                String::new(),
                "↑/↓ select · Tab field · Enter choose · Esc back".to_owned(),
            ],
        )
    }

    fn render_reasoning_combined(&self, width: usize) -> Vec<String> {
        let (toggle, effort_enabled, budget_enabled, disable_supported) =
            match &self.draft_model.reasoning {
                ReasoningCapability::Combined {
                    toggle,
                    effort,
                    budget,
                    disable_supported,
                } => (
                    *toggle,
                    !effort.is_empty(),
                    budget.is_some(),
                    *disable_supported,
                ),
                _ => (true, false, false, true),
            };
        self.render_box(
            width,
            "Reasoning Combined",
            vec![
                format!("{} [{}] toggle", self.selection_marker(0), mark(toggle)),
                format!(
                    "{} [{}] effort values  ›",
                    self.selection_marker(1),
                    mark(effort_enabled)
                ),
                format!(
                    "{} [{}] budget range   ›",
                    self.selection_marker(2),
                    mark(budget_enabled)
                ),
                format!(
                    "{} Disable supported  {}",
                    self.selection_marker(3),
                    yes_no(disable_supported)
                ),
                String::new(),
                "↑/↓ select · Space toggle · Enter choose · Esc back".to_owned(),
            ],
        )
    }

    fn render_added_models(&self, width: usize) -> Vec<String> {
        let mut content = vec!["Added models".to_owned(), String::new()];
        if self.models.is_empty() {
            content.push("No models added yet".to_owned());
        } else {
            for (index, model) in self.models.iter().enumerate() {
                content.push(format!(
                    "{} {}    {}",
                    self.selection_marker(index),
                    model.alias,
                    model.model_id
                ));
            }
        }
        content.extend([
            String::new(),
            "↑/↓ select · Enter review · Esc back".to_owned(),
        ]);
        self.render_box(width, "Custom Endpoint 4/4 · Added Models", content)
    }

    fn render_review(&self, width: usize) -> Vec<String> {
        let mut content = vec![
            "Provider".to_owned(),
            format!(
                "  id       {}",
                value_or_placeholder(&self.provider_id, "not set", self.theme)
            ),
            format!("  type     {}", self.api_type.as_config_str()),
            format!(
                "  base_url {}",
                value_or_placeholder(&self.base_url, "not set", self.theme)
            ),
            String::new(),
            "Models".to_owned(),
        ];
        if self.models.is_empty() {
            content.push("  -".to_owned());
        } else {
            for model in &self.models {
                content.push(format!("  {} -> {}", model.alias, model.model_id));
            }
        }
        content.extend([
            String::new(),
            format!("{} Save provider", self.selection_marker(0)),
            format!("{} Test connection", self.selection_marker(1)),
            String::new(),
            "↑/↓ select · Enter choose · Esc back".to_owned(),
        ]);
        self.render_box(width, "Custom Endpoint 4/4 · Review", content)
    }

    fn render_test_result(&self, width: usize) -> Vec<String> {
        let content = if let Some(message) = &self.validation_error {
            vec![
                "Request failed".to_owned(),
                message.clone(),
                String::new(),
                format!("{} Edit auth", self.selection_marker(0)),
                format!("{} Save anyway", self.selection_marker(1)),
                format!("{} Back to review", self.selection_marker(2)),
                String::new(),
                "↑/↓ select · Enter choose · Esc back".to_owned(),
            ]
        } else {
            vec![
                "Request succeeded".to_owned(),
                String::new(),
                format!("{} Save provider", self.selection_marker(0)),
                format!("{} Back to review", self.selection_marker(1)),
                String::new(),
                "↑/↓ select · Enter choose · Esc back".to_owned(),
            ]
        };
        self.render_box(width, "Custom Endpoint · Test Connection", content)
    }

    fn render_validation_error(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "Custom Endpoint · Validation Error",
            vec![
                self.validation_error
                    .as_deref()
                    .unwrap_or("Validation failed")
                    .to_owned(),
                String::new(),
                "Enter edit · Esc back".to_owned(),
            ],
        )
    }

    fn render_saved(&self, width: usize) -> Vec<String> {
        self.render_box(
            width,
            "Custom Endpoint · Saved",
            vec![
                "Provider saved".to_owned(),
                format!("{} models configured", self.models.len()),
                String::new(),
                "Enter close".to_owned(),
            ],
        )
    }

    fn render_box(&self, width: usize, title: &str, content: Vec<String>) -> Vec<String> {
        if width == 0 {
            return vec![String::new(); content.len() + 2];
        }
        if width == 1 {
            let border_style = Style::default().fg(self.theme.overlay_border);
            let mut lines = Vec::with_capacity(content.len() + 2);
            lines.push(paint("╭", border_style));
            lines.extend((0..content.len()).map(|_| paint("│", border_style)));
            lines.push(paint("╰", border_style));
            return lines;
        }

        let inner_width = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let text_style = Style::default().fg(self.theme.text_primary);
        let title_text = if inner_width > 4 {
            truncate_width(title, inner_width.saturating_sub(3), "…", false)
        } else {
            String::new()
        };
        let top_prefix = if title_text.is_empty() {
            String::new()
        } else {
            format!(" {title_text} ")
        };
        let top_fill = inner_width.saturating_sub(crate::primitive::visible_width(&top_prefix));

        let mut lines = Vec::with_capacity(content.len() + 2);
        lines.push(paint(
            &format!("╭{top_prefix}{}╮", "─".repeat(top_fill)),
            border_style,
        ));
        for line in content {
            let padded = truncate_width(&line, inner_width, "…", true);
            lines.push(format!(
                "{}{}{}",
                paint("│", border_style),
                paint(&padded, text_style),
                paint("│", border_style)
            ));
        }
        lines.push(paint(
            &format!("╰{}╯", "─".repeat(inner_width)),
            border_style,
        ));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        if matches!(input, InputEvent::Insert('\t')) {
            return self.move_selection(1);
        }
        if let Some(result) = self.handle_text_input(input) {
            return result;
        }
        match input {
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => self.back(),
            InputEvent::Action(KeybindingAction::SelectUp) => self.move_selection(-1),
            InputEvent::Action(KeybindingAction::SelectDown) => self.move_selection(1),
            InputEvent::Insert(' ') => self.toggle_selected(),
            InputEvent::Submit | InputEvent::Action(KeybindingAction::SelectConfirm) => {
                self.submit_step()
            }
            _ => InputResult::Ignored,
        }
    }

    fn back(&mut self) -> InputResult {
        match self.step {
            WizardStep::Provider => {
                self.action = Some(CustomEndpointWizardAction::Cancelled);
                InputResult::Cancelled
            }
            WizardStep::ApiType => {
                self.step = WizardStep::Provider;
                self.selected = 2;
                InputResult::Handled
            }
            WizardStep::EndpointAuth => {
                self.step = WizardStep::Provider;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::AuthSource => {
                self.step = WizardStep::EndpointAuth;
                self.selected = 1;
                InputResult::Handled
            }
            WizardStep::ModelSource => {
                self.step = WizardStep::EndpointAuth;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::FetchSelect => {
                self.step = WizardStep::ModelSource;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::ModelIdentity => {
                if matches!(
                    self.draft_model.source,
                    CustomEndpointModelSource::Fetched { .. }
                ) {
                    self.step = WizardStep::FetchSelect;
                    self.selected = 0;
                } else {
                    self.step = WizardStep::ModelSource;
                    self.selected = 1;
                }
                InputResult::Handled
            }
            WizardStep::ModelCapabilities => {
                self.step = WizardStep::ModelIdentity;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::ReasoningType
            | WizardStep::ReasoningEffort
            | WizardStep::ReasoningBudget
            | WizardStep::ReasoningCombined => {
                self.step = WizardStep::ModelCapabilities;
                self.selected = 4;
                InputResult::Handled
            }
            WizardStep::AddedModels => {
                self.step = WizardStep::ModelCapabilities;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::Review | WizardStep::TestResult | WizardStep::Saved => {
                self.step = if matches!(self.step, WizardStep::Review) {
                    WizardStep::AddedModels
                } else {
                    WizardStep::Review
                };
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::ValidationError => {
                self.step = WizardStep::Provider;
                self.selected = 0;
                InputResult::Handled
            }
        }
    }

    fn move_selection(&mut self, delta: isize) -> InputResult {
        let count = self.selection_count();
        if count == 0 {
            return InputResult::Ignored;
        }
        self.selected = if delta.is_negative() {
            self.selected.saturating_sub(delta.unsigned_abs())
        } else {
            (self.selected + delta as usize).min(count - 1)
        };
        InputResult::Handled
    }

    fn selection_count(&self) -> usize {
        match self.step {
            WizardStep::Provider | WizardStep::EndpointAuth | WizardStep::AuthSource => 3,
            WizardStep::ApiType => 4,
            WizardStep::ModelSource | WizardStep::Review => 2,
            WizardStep::FetchSelect => self.fetched_models.len(),
            WizardStep::ReasoningBudget => 3,
            WizardStep::ModelIdentity => 5,
            WizardStep::ModelCapabilities | WizardStep::ReasoningType => 5,
            WizardStep::ReasoningEffort => 6,
            WizardStep::ReasoningCombined => 4,
            WizardStep::AddedModels => self.models.len().max(1),
            WizardStep::TestResult => {
                if self.validation_error.is_some() {
                    3
                } else {
                    2
                }
            }
            WizardStep::ValidationError | WizardStep::Saved => 0,
        }
    }

    fn toggle_selected(&mut self) -> InputResult {
        match self.step {
            WizardStep::FetchSelect => {
                if let Some(selected) = self.fetched_selected.get_mut(self.selected) {
                    *selected = !*selected;
                    InputResult::Handled
                } else {
                    InputResult::Ignored
                }
            }
            WizardStep::ReasoningEffort => self.toggle_reasoning_effort(),
            WizardStep::ReasoningBudget => {
                if self.selected == 2 {
                    self.toggle_reasoning_disable_supported();
                    InputResult::Handled
                } else {
                    InputResult::Ignored
                }
            }
            WizardStep::ReasoningCombined => self.toggle_reasoning_combined(),
            WizardStep::ModelCapabilities => {
                match self.selected {
                    0 => self.draft_model.streaming = !self.draft_model.streaming,
                    1 => self.draft_model.tools = !self.draft_model.tools,
                    2 => self.draft_model.images = !self.draft_model.images,
                    3 => self.draft_model.embeddings = !self.draft_model.embeddings,
                    4 => {
                        self.step = WizardStep::ReasoningType;
                        self.selected = 0;
                    }
                    _ => return InputResult::Ignored,
                }
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn submit_step(&mut self) -> InputResult {
        match self.step {
            WizardStep::Provider => self.submit_provider(),
            WizardStep::ApiType => self.submit_api_type(),
            WizardStep::EndpointAuth => self.submit_endpoint_auth(),
            WizardStep::AuthSource => self.submit_auth_source(),
            WizardStep::ModelSource => self.submit_model_source(),
            WizardStep::FetchSelect => self.submit_fetch_select(),
            WizardStep::ModelIdentity => self.submit_model_identity(),
            WizardStep::ModelCapabilities => self.submit_model_capabilities(),
            WizardStep::ReasoningType => self.submit_reasoning_type(),
            WizardStep::ReasoningEffort => self.submit_reasoning_effort(),
            WizardStep::ReasoningBudget => self.submit_reasoning_budget(),
            WizardStep::ReasoningCombined => self.submit_reasoning_combined(),
            WizardStep::AddedModels => {
                self.step = WizardStep::Review;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::Review => self.submit_review_choice(),
            WizardStep::ValidationError => {
                self.step = WizardStep::Provider;
                self.selected = 0;
                InputResult::Handled
            }
            WizardStep::TestResult => self.submit_test_result_choice(),
            WizardStep::Saved => InputResult::Handled,
        }
    }

    fn submit_provider(&mut self) -> InputResult {
        if self.selected == 2 {
            self.step = WizardStep::ApiType;
            self.selected = api_type_index(self.api_type);
            return InputResult::Handled;
        }
        if let Some(error) = self.provider_step_error() {
            return self.validation_error(error);
        }
        self.step = WizardStep::EndpointAuth;
        self.selected = 0;
        InputResult::Handled
    }

    fn submit_api_type(&mut self) -> InputResult {
        self.api_type = api_type_for_index(self.selected);
        self.step = WizardStep::Provider;
        self.selected = 2;
        InputResult::Handled
    }

    fn submit_endpoint_auth(&mut self) -> InputResult {
        if self.selected == 1 {
            self.step = WizardStep::AuthSource;
            self.selected = match self.auth_source {
                CustomEndpointAuthDraft::EnvVar(_) => 0,
                CustomEndpointAuthDraft::InlineSecret(_) => 1,
                CustomEndpointAuthDraft::LocalPlaceholder => 2,
            };
            return InputResult::Handled;
        }
        if let Some(error) = self.endpoint_step_error() {
            return self.validation_error(error);
        }
        self.step = WizardStep::ModelSource;
        self.selected = 0;
        InputResult::Handled
    }

    fn submit_auth_source(&mut self) -> InputResult {
        self.auth_source = match self.selected {
            0 => CustomEndpointAuthDraft::EnvVar(String::new()),
            1 => CustomEndpointAuthDraft::InlineSecret(String::new()),
            2 => CustomEndpointAuthDraft::LocalPlaceholder,
            _ => return InputResult::Ignored,
        };
        self.step = WizardStep::EndpointAuth;
        self.selected = 1;
        InputResult::Handled
    }

    fn submit_model_source(&mut self) -> InputResult {
        match self.selected {
            0 => {
                self.action = Some(CustomEndpointWizardAction::FetchModels);
                InputResult::Submitted
            }
            1 => {
                self.draft_model = Self::empty_model(&self.provider_id, "");
                self.step = WizardStep::ModelIdentity;
                self.selected = 0;
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }

    fn submit_fetch_select(&mut self) -> InputResult {
        if self.queue_selected_fetched_models_for_review() {
            InputResult::Handled
        } else {
            self.validation_error("Select at least one fetched model.")
        }
    }

    fn submit_model_identity(&mut self) -> InputResult {
        if let Some(error) = Self::model_step_error(&self.draft_model) {
            return self.validation_error(error);
        }
        self.step = WizardStep::ModelCapabilities;
        self.selected = 0;
        InputResult::Handled
    }

    fn submit_model_capabilities(&mut self) -> InputResult {
        if let Some(error) = Self::model_step_error(&self.draft_model) {
            return self.validation_error(error);
        }
        if let Some(existing) = self
            .models
            .iter_mut()
            .find(|model| model.alias == self.draft_model.alias)
        {
            *existing = self.draft_model.clone();
        } else {
            self.models.push(self.draft_model.clone());
        }
        if !self.pending_fetched_review.is_empty() {
            let next = self.pending_fetched_review.remove(0);
            self.load_fetched_model_for_review(next);
            self.step = WizardStep::ModelIdentity;
            self.selected = 0;
            return InputResult::Handled;
        }
        self.step = WizardStep::AddedModels;
        self.selected = 0;
        InputResult::Handled
    }

    fn submit_reasoning_type(&mut self) -> InputResult {
        match self.selected {
            0 => {
                self.draft_model.reasoning = ReasoningCapability::None;
                self.step = WizardStep::ModelCapabilities;
            }
            1 => {
                self.draft_model.reasoning = ReasoningCapability::Toggle {
                    disable_supported: true,
                };
                self.step = WizardStep::ModelCapabilities;
            }
            2 => {
                self.draft_model.reasoning = ReasoningCapability::Effort {
                    values: default_reasoning_efforts(),
                    disable_supported: true,
                };
                self.step = WizardStep::ReasoningEffort;
            }
            3 => {
                self.draft_model.reasoning = ReasoningCapability::BudgetTokens {
                    min: None,
                    max: None,
                    disable_supported: true,
                };
                self.step = WizardStep::ReasoningBudget;
            }
            4 => {
                self.draft_model.reasoning = ReasoningCapability::Combined {
                    toggle: true,
                    effort: default_reasoning_efforts(),
                    budget: None,
                    disable_supported: true,
                };
                self.step = WizardStep::ReasoningCombined;
            }
            _ => return InputResult::Ignored,
        }
        self.selected = 0;
        InputResult::Handled
    }

    fn submit_reasoning_effort(&mut self) -> InputResult {
        if let Some(error) = Self::model_step_error(&self.draft_model) {
            return self.validation_error(error);
        }
        self.return_from_reasoning_detail(WizardStep::ReasoningEffort)
    }

    fn submit_reasoning_budget(&mut self) -> InputResult {
        self.return_from_reasoning_detail(WizardStep::ReasoningBudget)
    }

    fn submit_reasoning_combined(&mut self) -> InputResult {
        match self.selected {
            1 => {
                self.ensure_combined_effort();
                self.step = WizardStep::ReasoningEffort;
                self.selected = 0;
            }
            2 => {
                self.ensure_combined_budget();
                self.step = WizardStep::ReasoningBudget;
                self.selected = 0;
            }
            _ => {
                self.step = WizardStep::ModelCapabilities;
                self.selected = 4;
            }
        }
        InputResult::Handled
    }

    fn return_from_reasoning_detail(&mut self, detail_step: WizardStep) -> InputResult {
        if matches!(
            self.draft_model.reasoning,
            ReasoningCapability::Combined { .. }
        ) {
            self.step = WizardStep::ReasoningCombined;
            self.selected = match detail_step {
                WizardStep::ReasoningEffort => 1,
                WizardStep::ReasoningBudget => 2,
                _ => 0,
            };
        } else {
            self.step = WizardStep::ModelCapabilities;
            self.selected = 4;
        }
        InputResult::Handled
    }

    fn toggle_reasoning_effort(&mut self) -> InputResult {
        let Some(effort) = all_reasoning_efforts().get(self.selected).copied() else {
            return InputResult::Ignored;
        };
        match &mut self.draft_model.reasoning {
            ReasoningCapability::Effort { values, .. }
            | ReasoningCapability::Combined { effort: values, .. } => {
                if let Some(index) = values.iter().position(|value| *value == effort) {
                    values.remove(index);
                } else {
                    values.push(effort);
                    sort_reasoning_efforts(values);
                }
                InputResult::Handled
            }
            ReasoningCapability::None
            | ReasoningCapability::Toggle { .. }
            | ReasoningCapability::BudgetTokens { .. } => InputResult::Ignored,
        }
    }

    fn toggle_reasoning_disable_supported(&mut self) {
        match &mut self.draft_model.reasoning {
            ReasoningCapability::Toggle { disable_supported }
            | ReasoningCapability::Effort {
                disable_supported, ..
            }
            | ReasoningCapability::BudgetTokens {
                disable_supported, ..
            }
            | ReasoningCapability::Combined {
                disable_supported, ..
            } => *disable_supported = !*disable_supported,
            ReasoningCapability::None => {}
        }
    }

    fn toggle_reasoning_combined(&mut self) -> InputResult {
        match &mut self.draft_model.reasoning {
            ReasoningCapability::Combined {
                toggle,
                effort,
                budget,
                disable_supported,
            } => {
                match self.selected {
                    0 => *toggle = !*toggle,
                    1 => {
                        if effort.is_empty() {
                            *effort = default_reasoning_efforts();
                        } else {
                            effort.clear();
                        }
                    }
                    2 => {
                        *budget = if budget.is_some() {
                            None
                        } else {
                            Some(ReasoningBudget {
                                min: None,
                                max: None,
                            })
                        };
                    }
                    3 => *disable_supported = !*disable_supported,
                    _ => return InputResult::Ignored,
                }
                InputResult::Handled
            }
            ReasoningCapability::None
            | ReasoningCapability::Toggle { .. }
            | ReasoningCapability::Effort { .. }
            | ReasoningCapability::BudgetTokens { .. } => InputResult::Ignored,
        }
    }

    fn ensure_combined_effort(&mut self) {
        if let ReasoningCapability::Combined { effort, .. } = &mut self.draft_model.reasoning
            && effort.is_empty()
        {
            *effort = default_reasoning_efforts();
        }
    }

    fn ensure_combined_budget(&mut self) {
        if let ReasoningCapability::Combined { budget, .. } = &mut self.draft_model.reasoning {
            budget.get_or_insert(ReasoningBudget {
                min: None,
                max: None,
            });
        }
    }

    fn submit_review_choice(&mut self) -> InputResult {
        match self.selected {
            0 => self.submit_review(),
            1 => {
                self.action = Some(CustomEndpointWizardAction::TestConnection(self.draft()));
                InputResult::Submitted
            }
            _ => InputResult::Ignored,
        }
    }

    fn submit_test_result_choice(&mut self) -> InputResult {
        match (self.validation_error.is_some(), self.selected) {
            (true, 0) => {
                self.step = WizardStep::EndpointAuth;
                self.selected = 1;
                InputResult::Handled
            }
            (true, 1) | (false, 0) => self.submit_review(),
            (true, 2) | (false, 1) => {
                self.step = WizardStep::Review;
                self.selected = 0;
                InputResult::Handled
            }
            _ => InputResult::Ignored,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharSet {
    Graph,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEdit<'a> {
    Insert(char),
    Paste(&'a str),
    Backspace,
}

fn apply_text_edit(value: &mut String, edit: TextEdit<'_>, char_set: CharSet) -> InputResult {
    match edit {
        TextEdit::Insert(ch) => {
            push_allowed_text_char(value, ch, char_set);
        }
        TextEdit::Paste(text) => {
            value.extend(text.chars().filter(|ch| allowed_text_char(*ch, char_set)));
        }
        TextEdit::Backspace => {
            value.pop();
        }
    }
    InputResult::Handled
}

fn apply_u32_edit(value: &mut Option<u32>, edit: TextEdit<'_>) -> InputResult {
    let mut text = optional_u32_input_value(*value);
    match edit {
        TextEdit::Insert(ch) => {
            if ch.is_ascii_digit() {
                text.push(ch);
            }
        }
        TextEdit::Paste(pasted) => {
            text.extend(pasted.chars().filter(char::is_ascii_digit));
        }
        TextEdit::Backspace => {
            text.pop();
        }
    }
    *value = if text.is_empty() {
        None
    } else {
        text.parse::<u32>().ok()
    };
    InputResult::Handled
}

fn push_allowed_text_char(value: &mut String, ch: char, char_set: CharSet) {
    if allowed_text_char(ch, char_set) {
        value.push(ch);
    }
}

fn allowed_text_char(ch: char, char_set: CharSet) -> bool {
    match char_set {
        CharSet::Graph => ch.is_ascii_graphic(),
        CharSet::Text => ch.is_ascii_graphic() || ch == ' ',
    }
}

fn auto_alias(provider_id: &str, model_id: &str) -> String {
    if provider_id.is_empty() || model_id.is_empty() {
        String::new()
    } else {
        format!("{provider_id}/{model_id}")
    }
}

fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

fn api_type_rows() -> [(&'static str, &'static str); 4] {
    [
        ("OpenAI-compatible", "openai"),
        ("OpenAI Responses", "openai_response"),
        ("Anthropic Messages", "anthropic"),
        ("Google Generative AI", "google"),
    ]
}

fn api_type_for_index(index: usize) -> ApiType {
    match index {
        0 => ApiType::OpenAi,
        1 => ApiType::OpenAiResponse,
        2 => ApiType::Anthropic,
        3 => ApiType::Google,
        _ => ApiType::OpenAi,
    }
}

fn api_type_index(api_type: ApiType) -> usize {
    match api_type {
        ApiType::OpenAi => 0,
        ApiType::OpenAiResponse => 1,
        ApiType::Anthropic => 2,
        ApiType::Google => 3,
    }
}

fn api_type_label(api_type: ApiType) -> &'static str {
    match api_type {
        ApiType::OpenAi => "OpenAI-compatible",
        ApiType::OpenAiResponse => "OpenAI Responses",
        ApiType::Anthropic => "Anthropic Messages",
        ApiType::Google => "Google Generative AI",
    }
}

fn is_openai_family(api_type: ApiType) -> bool {
    matches!(api_type, ApiType::OpenAi | ApiType::OpenAiResponse)
}

fn input_value_line(value: &str, placeholder: &str, show_cursor: bool, theme: TuiTheme) -> String {
    let cursor = if show_cursor {
        input_cursor(theme)
    } else {
        String::new()
    };
    if value.is_empty() {
        format!("  {}{}", muted_placeholder(placeholder, theme), cursor)
    } else {
        format!("  {value}{cursor}")
    }
}

fn value_or_placeholder(value: &str, placeholder: &str, theme: TuiTheme) -> String {
    if value.is_empty() {
        muted_placeholder(placeholder, theme)
    } else {
        value.to_owned()
    }
}

fn auth_field_label(auth_source: &CustomEndpointAuthDraft) -> &'static str {
    match auth_source {
        CustomEndpointAuthDraft::EnvVar(_) => "Env var name",
        CustomEndpointAuthDraft::InlineSecret(_) => "API key",
        CustomEndpointAuthDraft::LocalPlaceholder => "API key",
    }
}

fn section_line(label: &str, theme: TuiTheme) -> String {
    paint(label, Style::default().fg(theme.text_primary).bold())
}

fn hint_line(text: &str, theme: TuiTheme) -> String {
    paint(text, Style::default().fg(theme.text_muted))
}

fn input_cursor(theme: TuiTheme) -> String {
    paint("▏", Style::default().fg(theme.brand).bold())
}

fn muted_placeholder(placeholder: &str, theme: TuiTheme) -> String {
    paint(placeholder, Style::default().fg(theme.text_muted))
}

fn optional_u32_input_value(value: Option<u32>) -> String {
    value.map_or_else(String::new, |value| value.to_string())
}

fn mark(enabled: bool) -> &'static str {
    if enabled { "x" } else { " " }
}

fn reasoning_label(reasoning: &ReasoningCapability) -> &'static str {
    match reasoning {
        ReasoningCapability::None => "None",
        ReasoningCapability::Toggle { .. } => "Toggle",
        ReasoningCapability::Effort { .. } => "Effort",
        ReasoningCapability::BudgetTokens { .. } => "Budget tokens",
        ReasoningCapability::Combined { .. } => "Combined",
    }
}

fn all_reasoning_efforts() -> [ReasoningEffort; 6] {
    [
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::XHigh,
        ReasoningEffort::Max,
    ]
}

fn default_reasoning_efforts() -> Vec<ReasoningEffort> {
    vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ]
}

fn sort_reasoning_efforts(values: &mut Vec<ReasoningEffort>) {
    values.sort_by_key(|value| {
        all_reasoning_efforts()
            .iter()
            .position(|effort| effort == value)
            .unwrap_or(usize::MAX)
    });
}

fn reasoning_effort_values(reasoning: &ReasoningCapability) -> &[ReasoningEffort] {
    match reasoning {
        ReasoningCapability::Effort { values, .. } => values,
        ReasoningCapability::Combined { effort, .. } => effort,
        ReasoningCapability::None
        | ReasoningCapability::Toggle { .. }
        | ReasoningCapability::BudgetTokens { .. } => &[],
    }
}

fn reasoning_budget(reasoning: &ReasoningCapability) -> Option<ReasoningBudget> {
    match reasoning {
        ReasoningCapability::BudgetTokens { min, max, .. } => Some(ReasoningBudget {
            min: *min,
            max: *max,
        }),
        ReasoningCapability::Combined { budget, .. } => budget.clone(),
        ReasoningCapability::None
        | ReasoningCapability::Toggle { .. }
        | ReasoningCapability::Effort { .. } => None,
    }
}

fn reasoning_disable_supported(reasoning: &ReasoningCapability) -> bool {
    match reasoning {
        ReasoningCapability::None => false,
        ReasoningCapability::Toggle { disable_supported }
        | ReasoningCapability::Effort {
            disable_supported, ..
        }
        | ReasoningCapability::BudgetTokens {
            disable_supported, ..
        }
        | ReasoningCapability::Combined {
            disable_supported, ..
        } => *disable_supported,
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neo_ai::ReasoningEffort;

    fn state() -> CustomEndpointWizardState {
        CustomEndpointWizardState::new(CustomEndpointWizardOptions {
            theme: TuiTheme::default(),
        })
    }

    fn visible(state: &CustomEndpointWizardState) -> String {
        state
            .render_lines(72)
            .into_iter()
            .map(|line| crate::primitive::strip_ansi(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn provider_step_renders_empty_fields_as_muted_placeholders() {
        let state = state();

        let rendered = state.render_lines(72).join("\n");
        let visible = visible(&state);

        assert!(visible.contains("Name shown in Neo"), "{visible}");
        assert!(visible.contains("lowercase-id-used-in-config"), "{visible}");
        assert!(!visible.contains("Acme Gateway"), "{visible}");
        assert!(!visible.contains("\n  acme▏"), "{visible}");
        assert!(
            rendered.contains(&paint(
                "Name shown in Neo",
                Style::default().fg(TuiTheme::default().text_muted)
            )),
            "{rendered:?}"
        );
    }

    #[test]
    fn endpoint_and_model_steps_render_empty_fields_as_muted_placeholders() {
        let mut state = state();

        state.step = WizardStep::EndpointAuth;
        let endpoint = visible(&state);
        assert!(endpoint.contains("Endpoint base URL"), "{endpoint}");
        assert!(endpoint.contains("ENV_VAR_WITH_API_KEY"), "{endpoint}");
        assert!(!endpoint.contains("gateway.example"), "{endpoint}");
        assert!(!endpoint.contains("ACME_API_KEY"), "{endpoint}");

        state.step = WizardStep::ModelIdentity;
        let rendered = state.render_lines(72).join("\n");
        let model = visible(&state);
        assert!(model.contains("Model id from provider"), "{model}");
        assert!(model.contains("provider-id/model-id"), "{model}");
        assert!(model.contains("Optional display name"), "{model}");
        assert!(!model.contains("qwen2.5"), "{model}");
        assert!(!model.contains("acme/"), "{model}");
        assert!(
            rendered.contains(&paint(
                "Model id from provider",
                Style::default().fg(TuiTheme::default().text_muted)
            )),
            "{rendered:?}"
        );
    }

    #[test]
    fn model_identity_limits_are_selectable_and_editable() {
        let mut state = state();
        state.provider_id = "provider".to_owned();
        state.step = WizardStep::ModelIdentity;
        state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "model");
        state.selected = 2;

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected, 3);
        assert_eq!(
            state.handle_input(&InputEvent::Paste("131072".to_owned())),
            InputResult::Handled
        );
        assert_eq!(state.draft_model.max_context_tokens, Some(131_072));

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(state.selected, 4);
        assert_eq!(
            state.handle_input(&InputEvent::Paste("8192".to_owned())),
            InputResult::Handled
        );
        assert_eq!(state.draft_model.max_output_tokens, Some(8_192));

        let visible = visible(&state);
        assert!(visible.contains("▸ Output tokens"), "{visible}");
        assert!(visible.contains("Context tokens"), "{visible}");
    }

    #[test]
    fn selected_editable_field_renders_brand_cursor_and_hierarchy() {
        let mut state = state();
        state.step = WizardStep::ModelIdentity;
        state.selected = 0;

        let rendered = state.render_lines(72).join("\n");
        let visible = visible(&state);

        assert!(visible.contains("▸ Model id"), "{visible}");
        assert!(visible.contains("▏"), "{visible}");
        assert!(
            rendered.contains(&paint(
                "▏",
                Style::default().fg(TuiTheme::default().brand).bold()
            )),
            "{rendered:?}"
        );
        assert!(
            rendered.contains(&paint(
                "Model",
                Style::default().fg(TuiTheme::default().text_primary).bold()
            )),
            "{rendered:?}"
        );
    }

    #[test]
    fn test_result_render_offers_save_anyway_on_failure() {
        let mut state = state();
        state.apply_test_result(Err("401 Unauthorized".to_owned()));
        let visible = visible(&state);
        assert!(visible.contains("Request failed"), "{visible}");
        assert!(visible.contains("401 Unauthorized"), "{visible}");
        assert!(visible.contains("Save anyway"), "{visible}");
        assert!(visible.contains("↑/↓ select"), "{visible}");
    }

    #[test]
    fn api_type_render_includes_all_current_protocols() {
        let mut state = state();
        state.step = WizardStep::ApiType;
        let visible = visible(&state);
        assert!(visible.contains("OpenAI-compatible"), "{visible}");
        assert!(visible.contains("type = \"openai\""), "{visible}");
        assert!(visible.contains("OpenAI Responses"), "{visible}");
        assert!(visible.contains("type = \"openai_response\""), "{visible}");
        assert!(visible.contains("Anthropic Messages"), "{visible}");
        assert!(visible.contains("type = \"anthropic\""), "{visible}");
        assert!(visible.contains("Google Generative AI"), "{visible}");
        assert!(visible.contains("type = \"google\""), "{visible}");
        assert!(visible.contains("↑/↓ select"), "{visible}");
    }

    #[test]
    fn fetched_model_review_shows_blank_limits_as_review_points() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.step = WizardStep::ModelIdentity;
        state.draft_model = CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Fetched {
                owned_by: Some("acme".to_owned()),
                created: Some(1_700_000_000),
            },
            model_id: "qwen2.5-coder-32b-instruct".to_owned(),
            alias: "acme/qwen2.5-coder-32b-instruct".to_owned(),
            display_name: None,
            max_context_tokens: None,
            max_output_tokens: None,
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::None,
        };

        let visible = visible(&state);
        assert!(visible.contains("Source: /models"), "{visible}");
        assert!(visible.contains("owned_by = \"acme\""), "{visible}");
        assert!(visible.contains("Context tokens"), "{visible}");
        assert!(visible.contains("Unset context limit"), "{visible}");
        assert!(visible.contains("Output tokens"), "{visible}");
        assert!(visible.contains("Unset output limit"), "{visible}");
        assert!(visible.contains("↑/↓ select"), "{visible}");
    }

    #[test]
    fn queued_fetched_model_for_review_preserves_source_metadata() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.apply_fetched_models(vec![CustomEndpointFetchedModel {
            id: "qwen2.5-coder-32b-instruct".to_owned(),
            owned_by: Some("model-lab".to_owned()),
            created: Some(1_700_000_000),
        }]);

        state.queue_selected_fetched_models_for_review();

        let visible = visible(&state);
        assert!(visible.contains("Source: /models"), "{visible}");
        assert!(visible.contains("owned_by = \"model-lab\""), "{visible}");
        assert!(visible.contains("created = 1700000000"), "{visible}");
        assert!(visible.contains("Context tokens"), "{visible}");
        assert!(visible.contains("Unset context limit"), "{visible}");
        assert!(visible.contains("Output tokens"), "{visible}");
        assert!(visible.contains("Unset output limit"), "{visible}");
    }

    #[test]
    fn non_api_selectable_screens_render_marker_from_selected_index() {
        let mut state = state();
        state.step = WizardStep::AuthSource;
        state.selected = 1;

        let visible = visible(&state);

        assert!(visible.contains("  Environment variable"), "{visible}");
        assert!(visible.contains("▸ Paste secret"), "{visible}");
    }

    #[test]
    fn renderable_steps_fit_narrow_widths() {
        let mut state = state();
        state.apply_fetched_models(vec![CustomEndpointFetchedModel {
            id: "qwen2.5-coder-32b-instruct".to_owned(),
            owned_by: Some("acme".to_owned()),
            created: Some(1_700_000_000),
        }]);

        for step in [
            WizardStep::Provider,
            WizardStep::ApiType,
            WizardStep::EndpointAuth,
            WizardStep::AuthSource,
            WizardStep::ModelSource,
            WizardStep::FetchSelect,
            WizardStep::ModelIdentity,
            WizardStep::ModelCapabilities,
            WizardStep::ReasoningType,
            WizardStep::ReasoningEffort,
            WizardStep::ReasoningBudget,
            WizardStep::ReasoningCombined,
            WizardStep::AddedModels,
            WizardStep::Review,
            WizardStep::TestResult,
            WizardStep::ValidationError,
            WizardStep::Saved,
        ] {
            state.step = step;
            for width in [1, 8, 24] {
                for line in state.render_lines(width) {
                    let visible_width =
                        crate::primitive::visible_width(&crate::primitive::strip_ansi(&line));
                    assert!(
                        visible_width <= width,
                        "{step:?} at width {width} rendered width {visible_width}: {line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn all_selectable_screens_render_select_hint() {
        let mut state = state();
        state.apply_fetched_models(vec![CustomEndpointFetchedModel {
            id: "qwen2.5-coder-32b-instruct".to_owned(),
            owned_by: Some("acme".to_owned()),
            created: Some(1_700_000_000),
        }]);

        for step in [
            WizardStep::Provider,
            WizardStep::ApiType,
            WizardStep::EndpointAuth,
            WizardStep::AuthSource,
            WizardStep::ModelSource,
            WizardStep::FetchSelect,
            WizardStep::ModelCapabilities,
            WizardStep::ReasoningType,
            WizardStep::ReasoningEffort,
            WizardStep::ReasoningBudget,
            WizardStep::ReasoningCombined,
            WizardStep::AddedModels,
            WizardStep::Review,
        ] {
            state.step = step;
            let visible = visible(&state);
            assert!(
                visible.contains("↑/↓ select"),
                "missing select hint in {step:?}:\n{visible}"
            );
        }
    }

    #[test]
    fn provider_validation_rejects_uppercase_id() {
        let mut state = state();
        state.display_name = "Acme Gateway".to_owned();
        state.provider_id = "Acme".to_owned();

        assert!(state.provider_step_error().is_some());
        assert!(state.provider_step_error().unwrap().contains("lowercase"));
    }

    #[test]
    fn cancel_from_api_type_returns_to_provider_without_cancel_action() {
        let mut state = state();
        state.step = WizardStep::ApiType;
        state.selected = 1;

        assert_eq!(
            state.handle_input(&InputEvent::Cancel),
            InputResult::Handled
        );

        assert_eq!(state.step, WizardStep::Provider);
        assert_eq!(state.selected, 2);
        assert!(state.take_action().is_none());
    }

    #[test]
    fn provider_validation_error_clears_stale_fetch_action() {
        let mut state = state();
        state.action = Some(CustomEndpointWizardAction::FetchModels);
        state.display_name = "Acme Gateway".to_owned();
        state.provider_id = "Acme".to_owned();

        assert_eq!(state.submit_provider(), InputResult::Handled);

        assert_eq!(state.step, WizardStep::ValidationError);
        assert!(state.take_action().is_none());
    }

    #[test]
    fn multiple_selected_fetched_models_are_reviewed_one_by_one() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.apply_fetched_models(vec![
            CustomEndpointFetchedModel {
                id: "qwen2".to_owned(),
                owned_by: Some("lab-a".to_owned()),
                created: Some(1),
            },
            CustomEndpointFetchedModel {
                id: "qwen3".to_owned(),
                owned_by: Some("lab-b".to_owned()),
                created: Some(2),
            },
        ]);

        assert!(state.queue_selected_fetched_models_for_review());
        assert_eq!(state.step, WizardStep::ModelIdentity);
        assert_eq!(state.draft_model.model_id, "qwen2");

        assert_eq!(state.submit_model_identity(), InputResult::Handled);
        assert_eq!(state.submit_model_capabilities(), InputResult::Handled);

        assert_eq!(state.models.len(), 1);
        assert_eq!(state.models[0].model_id, "qwen2");
        assert_eq!(state.step, WizardStep::ModelIdentity);
        assert_eq!(state.draft_model.model_id, "qwen3");
        assert_eq!(state.draft_model.alias, "acme/qwen3");
        assert_eq!(
            state.draft_model.source,
            CustomEndpointModelSource::Fetched {
                owned_by: Some("lab-b".to_owned()),
                created: Some(2),
            }
        );

        assert_eq!(state.submit_model_identity(), InputResult::Handled);
        assert_eq!(state.submit_model_capabilities(), InputResult::Handled);

        assert_eq!(state.step, WizardStep::AddedModels);
        assert_eq!(
            state
                .models
                .iter()
                .map(|model| model.model_id.as_str())
                .collect::<Vec<_>>(),
            vec!["qwen2", "qwen3"]
        );
    }

    #[test]
    fn provider_text_input_edits_display_name_and_provider_id() {
        let mut state = state();

        assert_eq!(
            state.handle_input(&InputEvent::Paste("Acme Gateway".to_owned())),
            InputResult::Handled
        );
        assert_eq!(state.display_name, "Acme Gateway");

        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(&InputEvent::Paste("Acm".to_owned()));
        state.handle_input(&InputEvent::Backspace);
        state.handle_input(&InputEvent::Insert('e'));

        assert_eq!(state.provider_id, "Ace");
    }

    #[test]
    fn provider_tab_moves_to_next_field() {
        let mut state = state();

        assert_eq!(
            state.handle_input(&InputEvent::Insert('\t')),
            InputResult::Handled
        );

        assert_eq!(state.selected, 1);
        assert!(state.display_name.is_empty());
    }

    #[test]
    fn model_identity_model_id_input_auto_updates_blank_alias() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.step = WizardStep::ModelIdentity;
        state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "");

        assert_eq!(
            state.handle_input(&InputEvent::Paste("qwen2".to_owned())),
            InputResult::Handled
        );

        assert_eq!(state.draft_model.model_id, "qwen2");
        assert_eq!(state.draft_model.alias, "acme/qwen2");
    }

    #[test]
    fn empty_save_clears_stale_action_and_shows_validation_error() {
        let mut state = state();
        state.action = Some(CustomEndpointWizardAction::FetchModels);
        state.models.clear();
        state.step = WizardStep::Review;

        assert_eq!(state.submit_review(), InputResult::Handled);

        assert!(state.take_action().is_none());
        assert_eq!(state.step, WizardStep::ValidationError);
        assert_eq!(
            state.validation_error.as_deref(),
            Some("Add at least one model before saving.")
        );
    }

    #[test]
    fn save_action_contains_reviewed_provider_and_model_draft() {
        let mut state = state();
        state.display_name = "Acme Gateway".to_owned();
        state.provider_id = "acme".to_owned();
        state.api_type = ApiType::OpenAi;
        state.base_url = "https://gateway.example.com/v1".to_owned();
        state.auth_source = CustomEndpointAuthDraft::EnvVar("ACME_API_KEY".to_owned());
        state.models.push(CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Manual,
            model_id: "qwen2.5-coder-32b-instruct".to_owned(),
            alias: "acme/qwen2.5-coder-32b-instruct".to_owned(),
            display_name: Some("Qwen 2.5 Coder 32B".to_owned()),
            max_context_tokens: Some(131_072),
            max_output_tokens: Some(8_192),
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::Effort {
                values: vec![
                    ReasoningEffort::Low,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                disable_supported: true,
            },
        });
        state.step = WizardStep::Review;

        state.submit_review();

        let Some(CustomEndpointWizardAction::Save(draft)) = state.take_action() else {
            panic!("expected save action");
        };
        assert_eq!(draft.provider_id, "acme");
        assert_eq!(draft.api_type, ApiType::OpenAi);
        assert_eq!(draft.models[0].alias, "acme/qwen2.5-coder-32b-instruct");
    }

    #[test]
    fn effort_reasoning_page_edits_selected_values_before_saving_model() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
        state.step = WizardStep::ReasoningType;
        state.selected = 2;

        assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningEffort);

        assert_eq!(
            state.handle_input(&InputEvent::Insert(' ')),
            InputResult::Handled
        );
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(
            state.handle_input(&InputEvent::Insert(' ')),
            InputResult::Handled
        );
        assert_eq!(state.submit_step(), InputResult::Handled);

        assert_eq!(
            state.draft_model.reasoning,
            ReasoningCapability::Effort {
                values: vec![
                    ReasoningEffort::Minimal,
                    ReasoningEffort::Medium,
                    ReasoningEffort::High,
                ],
                disable_supported: true,
            }
        );
    }

    #[test]
    fn budget_reasoning_page_edits_token_limits_before_saving_model() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
        state.step = WizardStep::ReasoningType;
        state.selected = 3;

        assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningBudget);

        assert_eq!(
            state.handle_input(&InputEvent::Paste("1024".to_owned())),
            InputResult::Handled
        );
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        assert_eq!(
            state.handle_input(&InputEvent::Paste("8192".to_owned())),
            InputResult::Handled
        );
        assert_eq!(state.submit_step(), InputResult::Handled);

        assert_eq!(
            state.draft_model.reasoning,
            ReasoningCapability::BudgetTokens {
                min: Some(1024),
                max: Some(8192),
                disable_supported: true,
            }
        );
    }

    #[test]
    fn combined_reasoning_page_reaches_effort_and_budget_detail_pages() {
        let mut state = state();
        state.provider_id = "acme".to_owned();
        state.draft_model = CustomEndpointWizardState::empty_model(&state.provider_id, "reasoner");
        state.step = WizardStep::ReasoningType;
        state.selected = 4;

        assert_eq!(state.submit_reasoning_type(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningCombined);

        state.selected = 1;
        assert_eq!(state.submit_step(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningEffort);
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(&InputEvent::Action(KeybindingAction::SelectDown));
        state.handle_input(&InputEvent::Insert(' '));
        assert_eq!(state.submit_step(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningCombined);

        state.selected = 2;
        assert_eq!(state.submit_step(), InputResult::Handled);
        assert_eq!(state.step, WizardStep::ReasoningBudget);
        assert_eq!(
            state.handle_input(&InputEvent::Paste("4096".to_owned())),
            InputResult::Handled
        );
        assert_eq!(state.submit_step(), InputResult::Handled);

        assert_eq!(
            state.draft_model.reasoning,
            ReasoningCapability::Combined {
                toggle: true,
                effort: vec![ReasoningEffort::Low, ReasoningEffort::Medium],
                budget: Some(neo_ai::ReasoningBudget {
                    min: Some(4096),
                    max: None,
                }),
                disable_supported: true,
            }
        );
    }
}
