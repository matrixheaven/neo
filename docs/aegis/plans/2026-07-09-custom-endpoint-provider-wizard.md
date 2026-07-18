# Custom Endpoint Provider Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use aegis:subagent-driven-development (recommended) or aegis:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the Add Provider `Custom endpoint` wizard that writes Neo provider/model config, supports all current provider protocol types, discovers OpenAI-family model ids via `/models`, and routes fetched models through user review before saving.

**Architecture:** Add a dedicated `CustomEndpointWizardState` in `neo-tui` with typed draft data and action outputs, then wire it into `NeoChromeState` overlays and `InteractiveController`. Persist the final reviewed provider/model draft through a narrow config mutation helper instead of piggybacking on catalog import. Keep `/models` fetch as model-id discovery only; every fetched model becomes a draft reviewed by the same model config screens used for manual entry.

**Tech Stack:** Rust 2024, `neo-tui` dialog state/rendering, `neo-agent` interactive controller, existing `config::mutations` TOML writer, `reqwest` for `/models`, `serde` for OpenAI-family model-list decoding.

---

## File Structure

- Create `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs`
  - Owns the complete TUI state machine, draft structs, validation, rendering, and unit tests for the wizard.
  - Emits `CustomEndpointWizardAction` values for controller work: fetch models, test connection, save, cancel.

- Modify `crates/neo-tui/src/dialogs/mod.rs`
  - Exports the custom endpoint wizard types.

- Modify `crates/neo-tui/src/shell/overlay.rs`
  - Adds `OverlayKind::CustomEndpointWizard` and renders it as a rich dialog.

- Modify `crates/neo-tui/src/shell/dialog_factory.rs`
  - Adds `open_custom_endpoint_wizard`.
  - Adds `custom_endpoint_wizard_action`, `take_custom_endpoint_wizard_action`, and a focused-state mutator used to apply fetched model results.

- Modify `crates/neo-tui/src/shell/input_dispatch.rs`
  - Routes input to `CustomEndpointWizardState`.

- Modify `crates/neo-agent/src/config/mutations.rs`
  - Adds `add_custom_endpoint_provider`, a focused read-modify-write helper for custom endpoint provider/model drafts.

- Create `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`
  - Controller glue for Add Provider -> Custom endpoint.
  - Handles wizard actions, OpenAI-family `/models` fetch, config save, refresh, and status messages.

- Modify `crates/neo-agent/src/modes/interactive/mod.rs`
  - Registers the new module and stores `pending_custom_endpoint_fetch`.

- Modify `crates/neo-agent/src/modes/interactive/dialog_results.rs`
  - Adds the `Custom endpoint` Add Provider option.
  - Dispatches wizard results before generic choice handling consumes them.

## Task 1: Config Mutation Helper

**Files:**
- Modify: `crates/neo-agent/src/config/mutations.rs`

- [ ] **Step 1: Add a failing config mutation test**

Add this test inside `#[cfg(test)] mod tests` in `crates/neo-agent/src/config/mutations.rs`:

```rust
#[test]
fn add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = temp.path().join(".neo/config.toml");

    let message = super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            provider_type: Some(ApiType::OpenAi),
            base_url: Some("https://gateway.example.com/v1".to_owned()),
            api_key: None,
            api_key_env: Some("ACME_API_KEY".to_owned()),
        },
        vec![(
            "acme/qwen2.5-coder-32b-instruct".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "qwen2.5-coder-32b-instruct".to_owned(),
                max_context_tokens: Some(128_000),
                max_output_tokens: Some(8_192),
                capabilities: vec![
                    "streaming".to_owned(),
                    "tools".to_owned(),
                    "reasoning".to_owned(),
                ],
                reasoning: neo_ai::ReasoningCapability::Effort {
                    values: vec![
                        neo_ai::ReasoningEffort::Low,
                        neo_ai::ReasoningEffort::Medium,
                        neo_ai::ReasoningEffort::High,
                    ],
                    disable_supported: true,
                },
                display_name: Some("Qwen 2.5 Coder 32B".to_owned()),
            },
        )],
        None,
    )
    .expect("add custom endpoint provider");

    assert_eq!(message, "added provider 'acme' with 1 model\n");
    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(written.contains("type = \"openai\""), "{written}");
    assert!(written.contains("api_key_env = \"ACME_API_KEY\""), "{written}");
    assert!(
        written.contains("[models.\"acme/qwen2.5-coder-32b-instruct\"]"),
        "{written}"
    );
    assert!(written.contains("max_context_tokens = 128000"), "{written}");
    assert!(written.contains("max_output_tokens = 8192"), "{written}");
    assert!(written.contains("type = \"effort\""), "{written}");
    assert!(
        written.contains("default_model = \"acme/qwen2.5-coder-32b-instruct\""),
        "{written}"
    );
}
```

- [ ] **Step 2: Run the exact failing test**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty --exact --nocapture --include-ignored
```

Expected: fail with `cannot find function add_custom_endpoint_provider`.

- [ ] **Step 3: Implement the mutation helper**

Add this function near `add_provider_from_catalog_entry` in `crates/neo-agent/src/config/mutations.rs`:

```rust
pub fn add_custom_endpoint_provider(
    config_path: &Path,
    provider_id: &str,
    provider_config: ProviderConfig,
    models: Vec<(String, ModelConfig)>,
    default_model: Option<&str>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        !models.is_empty(),
        "custom endpoint provider must include at least one model"
    );

    let mut file_config = read_file_config(config_path)?;
    remove_provider_config(&mut file_config, provider_id);

    let providers = file_config.providers.get_or_insert_with(BTreeMap::new);
    providers.insert(provider_id.to_owned(), provider_config);

    let first_alias = models.first().map(|(alias, _)| alias.clone());
    {
        let model_table = file_config.models.get_or_insert_with(BTreeMap::new);
        for (alias, model) in models {
            anyhow::ensure!(
                model.provider == provider_id,
                "model '{alias}' references provider '{}', expected '{provider_id}'",
                model.provider
            );
            model_table.insert(alias, model);
        }
    }

    let should_set_default = file_config
        .default_model
        .as_deref()
        .is_none_or(str::is_empty);
    if let Some(default_alias) = default_model.map(str::to_owned).or_else(|| {
        if should_set_default {
            first_alias
        } else {
            None
        }
    }) {
        file_config.default_model = Some(default_alias);
        file_config.default_provider = Some(provider_id.to_owned());
    }

    let count = file_config.models.as_ref().map_or(0, |models| {
        models
            .values()
            .filter(|model| model.provider == provider_id)
            .count()
    });
    write_file_config(config_path, &file_config)?;
    Ok(format!(
        "added provider '{provider_id}' with {count} model{}\n",
        if count == 1 { "" } else { "s" }
    ))
}
```

- [ ] **Step 4: Run the exact passing test**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 5: Add replacement behavior test**

Add:

```rust
#[test]
fn add_custom_endpoint_provider_replaces_existing_provider_models_only() {
    let temp = TempDir::new().expect("temp dir");
    let config_path = write_project_config(
        temp.path(),
        r#"
default_model = "other/keep"

[providers.acme]
type = "openai"
base_url = "https://old.example.com/v1"

[providers.other]
type = "openai_response"
base_url = "https://api.openai.com/v1"

[models."acme/old"]
provider = "acme"
model = "old"

[models."other/keep"]
provider = "other"
model = "keep"
"#,
    );

    super::add_custom_endpoint_provider(
        &config_path,
        "acme",
        ProviderConfig {
            provider_type: Some(ApiType::Google),
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".to_owned()),
            api_key: Some("local".to_owned()),
            api_key_env: None,
        },
        vec![(
            "acme/gemini-custom".to_owned(),
            ModelConfig {
                provider: "acme".to_owned(),
                model: "models/gemini-custom".to_owned(),
                capabilities: vec!["streaming".to_owned()],
                ..ModelConfig::default()
            },
        )],
        None,
    )
    .expect("replace custom endpoint provider");

    let written = fs::read_to_string(config_path).expect("read config");
    assert!(written.contains("default_model = \"other/keep\""), "{written}");
    assert!(written.contains("[providers.acme]"), "{written}");
    assert!(written.contains("type = \"google\""), "{written}");
    assert!(!written.contains("[models.\"acme/old\"]"), "{written}");
    assert!(written.contains("[models.\"acme/gemini-custom\"]"), "{written}");
    assert!(written.contains("[models.\"other/keep\"]"), "{written}");
}
```

- [ ] **Step 6: Run the exact replacement test**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_custom_endpoint_provider_replaces_existing_provider_models_only --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 7: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-agent/src/config/mutations.rs
git commit -m "feat(provider): add custom endpoint config mutation"
```

## Task 2: Custom Endpoint Wizard Draft Types And Rendering

**Files:**
- Create: `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs`
- Modify: `crates/neo-tui/src/dialogs/mod.rs`

- [ ] **Step 1: Add the new module export test by compiling a skeleton reference**

Create `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs` with the initial public API and tests:

```rust
use neo_ai::{ApiType, ReasoningBudget, ReasoningCapability, ReasoningEffort};

use crate::input::{InputEvent, KeybindingAction};
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Color, InputResult, Style, paint, truncate_width};

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
            base_url: "https://gateway.example.com/v1".to_owned(),
            auth_source: CustomEndpointAuthDraft::EnvVar(String::new()),
            models: Vec::new(),
            draft_model: Self::empty_model("", ""),
            fetched_models: Vec::new(),
            fetched_selected: Vec::new(),
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

    #[must_use]
    pub fn render_lines(&self, width: usize) -> Vec<String> {
        match self.step {
            WizardStep::Provider => self.render_box(width, "Custom Endpoint 1/4 · Provider", &[
                "Provider",
                "▸ Display name",
                if self.display_name.is_empty() { "  Acme Gateway▏" } else { "  <display-name>▏" },
                "",
                "  Provider id",
                if self.provider_id.is_empty() { "  acme▏" } else { "  <provider-id>▏" },
                "",
                "  API type",
                "  OpenAI-compatible  ›",
                "",
                "↑/↓ select · Tab field · Enter continue · Esc cancel",
            ]),
            WizardStep::ApiType => self.render_box(width, "API Type", &[
                "▸ OpenAI-compatible     type = \"openai\"",
                "  OpenAI Responses      type = \"openai_response\"",
                "  Anthropic Messages    type = \"anthropic\"",
                "  Google Generative AI  type = \"google\"",
                "",
                "↑/↓ select · Enter choose · Esc back",
            ]),
            _ => self.render_box(width, "Custom Endpoint", &["Esc back"]),
        }
    }

    fn render_box(&self, width: usize, title: &str, content: &[&str]) -> Vec<String> {
        let inner_w = width.saturating_sub(2).max(1);
        let border_style = Style::default().fg(self.theme.overlay_border);
        let title_style = Style::default().fg(self.theme.text_primary).bold();
        let text_style = Style::default().fg(Color::Reset);

        let mut lines = Vec::new();
        lines.push(paint(&format!("╭ {} {}", title, "─".repeat(inner_w.saturating_sub(title.len() + 2))), border_style));
        for line in content {
            let padded = truncate_width(line, inner_w, "…", true);
            lines.push(format!(
                "{}{}{}",
                paint("│", border_style),
                paint(&padded, text_style),
                paint("│", border_style)
            ));
        }
        lines.push(paint(&format!("╰{}", "─".repeat(inner_w)), border_style));
        lines
    }

    pub fn handle_input(&mut self, input: &InputEvent) -> InputResult {
        match input {
            InputEvent::Cancel | InputEvent::Action(KeybindingAction::SelectCancel) => {
                self.action = Some(CustomEndpointWizardAction::Cancelled);
                InputResult::Cancelled
            }
            _ => InputResult::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

Update `crates/neo-tui/src/dialogs/mod.rs`:

```rust
pub mod custom_endpoint_wizard;
pub use custom_endpoint_wizard::{
    CustomEndpointAuthDraft, CustomEndpointFetchedModel, CustomEndpointModelDraft,
    CustomEndpointModelSource, CustomEndpointProviderDraft, CustomEndpointWizardAction,
    CustomEndpointWizardOptions, CustomEndpointWizardState,
};
```

- [ ] **Step 2: Run the exact new render test**

Run:

```bash
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::api_type_render_includes_all_current_protocols --exact --nocapture --include-ignored
```

Expected: pass after the skeleton compiles.

- [ ] **Step 3: Expand rendering to all spec screens**

Replace the fallback `_ => self.render_box(width, "Custom Endpoint", &["Esc back"])` branch with dedicated render functions:

```rust
fn render_endpoint_auth(&self, width: usize) -> Vec<String> { /* render Step 2/4 exactly from spec */ }
fn render_auth_source(&self, width: usize) -> Vec<String> { /* render API Key Source picker */ }
fn render_model_source(&self, width: usize) -> Vec<String> { /* render Step 3/4 model source */ }
fn render_fetch_select(&self, width: usize) -> Vec<String> { /* render selected fetched model ids */ }
fn render_model_identity(&self, width: usize) -> Vec<String> { /* render model id, alias, display name, token limits */ }
fn render_model_capabilities(&self, width: usize) -> Vec<String> { /* render streaming/tools/images/embeddings/reasoning */ }
fn render_reasoning_type(&self, width: usize) -> Vec<String> { /* render None/Toggle/Effort/Budget/Combined */ }
fn render_reasoning_effort(&self, width: usize) -> Vec<String> { /* render minimal/low/medium/high/xhigh/max */ }
fn render_reasoning_budget(&self, width: usize) -> Vec<String> { /* render min/max/off-supported fields */ }
fn render_reasoning_combined(&self, width: usize) -> Vec<String> { /* render toggle/effort/budget family flags */ }
fn render_added_models(&self, width: usize) -> Vec<String> { /* render added model summary rows */ }
fn render_review(&self, width: usize) -> Vec<String> { /* render provider + model summary */ }
fn render_validation_error(&self, width: usize) -> Vec<String> { /* render validation_error */ }
fn render_saved(&self, width: usize) -> Vec<String> { /* render saved confirmation */ }
```

Do not implement controller behavior in this task. Keep this task focused on TUI state and rendering.

- [ ] **Step 4: Add exact render tests for required hints and fetched review**

Add tests in `custom_endpoint_wizard.rs`:

```rust
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
    assert!(visible.contains("Context tokens   -"), "{visible}");
    assert!(visible.contains("Output tokens    -"), "{visible}");
    assert!(visible.contains("↑/↓ select"), "{visible}");
}

#[test]
fn all_selectable_screens_render_select_hint() {
    let mut state = state();
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
        assert!(visible.contains("↑/↓ select"), "missing select hint in {step:?}:\n{visible}");
    }
}
```

- [ ] **Step 5: Run exact TUI render tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::fetched_model_review_shows_blank_limits_as_review_points --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::all_selectable_screens_render_select_hint --exact --nocapture --include-ignored
```

Expected: both pass.

- [ ] **Step 6: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs crates/neo-tui/src/dialogs/mod.rs
git commit -m "feat(tui): add custom endpoint wizard state"
```

## Task 3: Wizard Input, Validation, And Draft Conversion

**Files:**
- Modify: `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs`

- [ ] **Step 1: Add validation and draft conversion tests**

Add these tests:

```rust
#[test]
fn provider_validation_rejects_uppercase_id() {
    let mut state = state();
    state.display_name = "Acme Gateway".to_owned();
    state.provider_id = "Acme".to_owned();

    assert!(state.provider_step_error().is_some());
    assert!(state.provider_step_error().unwrap().contains("lowercase"));
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
        max_context_tokens: Some(128_000),
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
```

- [ ] **Step 2: Implement validation helpers**

Add:

```rust
fn valid_provider_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}

impl CustomEndpointWizardState {
    fn provider_step_error(&self) -> Option<String> {
        if self.display_name.trim().is_empty() {
            return Some("Display name is required".to_owned());
        }
        if !valid_provider_id(self.provider_id.trim()) {
            return Some("Provider id must use lowercase letters, digits, `_`, `-`.".to_owned());
        }
        None
    }

    fn endpoint_step_error(&self) -> Option<String> {
        if self.base_url.trim().is_empty() {
            return Some("Base URL is required".to_owned());
        }
        match &self.auth_source {
            CustomEndpointAuthDraft::EnvVar(value) if value.trim().is_empty() => {
                Some("Env var name is required".to_owned())
            }
            CustomEndpointAuthDraft::InlineSecret(value) if value.trim().is_empty() => {
                Some("API key is required".to_owned())
            }
            CustomEndpointAuthDraft::EnvVar(_)
            | CustomEndpointAuthDraft::InlineSecret(_)
            | CustomEndpointAuthDraft::LocalPlaceholder => None,
        }
    }

    fn model_step_error(model: &CustomEndpointModelDraft) -> Option<String> {
        if model.model_id.trim().is_empty() {
            return Some("Model id is required".to_owned());
        }
        if model.alias.trim().is_empty() {
            return Some("Model alias is required".to_owned());
        }
        if let ReasoningCapability::Effort { values, .. } = &model.reasoning
            && values.is_empty()
        {
            return Some("Effort reasoning needs at least one value".to_owned());
        }
        None
    }
}
```

- [ ] **Step 3: Implement action-producing submit handlers**

Add small methods that tests and input handling call:

```rust
impl CustomEndpointWizardState {
    fn draft(&self) -> CustomEndpointProviderDraft {
        CustomEndpointProviderDraft {
            display_name: self.display_name.trim().to_owned(),
            provider_id: self.provider_id.trim().to_owned(),
            api_type: self.api_type,
            base_url: self.base_url.trim_end_matches('/').to_owned(),
            auth: self.auth_source.clone(),
            models: self.models.clone(),
        }
    }

    fn submit_review(&mut self) {
        if self.models.is_empty() {
            self.validation_error = Some("Add at least one model before saving".to_owned());
            self.step = WizardStep::ValidationError;
            return;
        }
        self.action = Some(CustomEndpointWizardAction::Save(self.draft()));
    }

    pub fn apply_fetched_models(&mut self, models: Vec<CustomEndpointFetchedModel>) {
        self.fetched_selected = vec![true; models.len()];
        self.fetched_models = models;
        self.step = WizardStep::FetchSelect;
        self.selected = 0;
    }

    fn queue_selected_fetched_models_for_review(&mut self) {
        let Some((model, _)) = self
            .fetched_models
            .iter()
            .zip(self.fetched_selected.iter())
            .find(|(_, selected)| **selected)
        else {
            self.validation_error = Some("Select at least one fetched model".to_owned());
            self.step = WizardStep::ValidationError;
            return;
        };
        self.draft_model = CustomEndpointModelDraft {
            source: CustomEndpointModelSource::Fetched {
                owned_by: model.owned_by.clone(),
                created: model.created,
            },
            model_id: model.id.clone(),
            alias: format!("{}/{}", self.provider_id, model.id),
            display_name: None,
            max_context_tokens: None,
            max_output_tokens: None,
            streaming: true,
            tools: true,
            images: false,
            embeddings: false,
            reasoning: ReasoningCapability::None,
        };
        self.step = WizardStep::ModelIdentity;
        self.selected = 0;
    }
}
```

- [ ] **Step 4: Wire keyboard input to the methods**

Implement `handle_input` so:

- `Provider`: `Enter` validates provider fields and moves to `EndpointAuth`; selecting API type opens `ApiType`.
- `ApiType`: arrows select the four canonical protocols; `Enter` applies the selected `ApiType`.
- `EndpointAuth`: `Enter` validates base URL/auth and moves to `ModelSource`.
- `ModelSource`: `Fetch from /models` emits `CustomEndpointWizardAction::FetchModels`; `Enter manually` opens `ModelIdentity`.
- `FetchSelect`: `Space` toggles fetched selection; `Enter` calls `queue_selected_fetched_models_for_review`.
- `ModelIdentity`: `Enter` validates identity/limits and opens capabilities.
- `ModelCapabilities`: `Space` toggles booleans; `Enter` adds or updates `draft_model`.
- Reasoning substeps edit `ReasoningCapability` variants.
- `Review`: `Save provider` calls `submit_review`; `Test connection` emits `TestConnection`.

Keep mutation in this file only to local wizard state.

- [ ] **Step 5: Run exact input tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::provider_validation_rejects_uppercase_id --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::save_action_contains_reviewed_provider_and_model_draft --exact --nocapture --include-ignored
```

Expected: both pass.

- [ ] **Step 6: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs
git commit -m "feat(tui): validate custom endpoint wizard drafts"
```

## Task 4: Overlay Plumbing

**Files:**
- Modify: `crates/neo-tui/src/shell/overlay.rs`
- Modify: `crates/neo-tui/src/shell/dialog_factory.rs`
- Modify: `crates/neo-tui/src/shell/input_dispatch.rs`

- [ ] **Step 1: Add overlay plumbing test**

In `crates/neo-tui/src/shell/dialog_factory.rs` tests, add:

```rust
#[test]
fn custom_endpoint_wizard_overlay_is_rich_dialog_and_blocks_prompt() {
    let mut chrome = NeoChromeState::new("title", "session", "model", "/tmp");
    let id = chrome.open_custom_endpoint_wizard(crate::dialogs::CustomEndpointWizardOptions {
        theme: crate::primitive::theme::TuiTheme::default(),
    });

    assert_eq!(chrome.focused_overlay_id(), Some(id));
    assert!(chrome.focused_overlay_is_rich_dialog());
    assert!(chrome.focused_overlay_blocks_prompt());
    let visible = chrome
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| crate::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Custom Endpoint 1/4"), "{visible}");
}
```

- [ ] **Step 2: Run exact failing overlay test**

Run:

```bash
cargo test --package neo-tui --lib shell::dialog_factory::tests::custom_endpoint_wizard_overlay_is_rich_dialog_and_blocks_prompt --exact --nocapture --include-ignored
```

Expected: fail with missing `open_custom_endpoint_wizard` or missing overlay variant.

- [ ] **Step 3: Add overlay variant and rendering**

In `crates/neo-tui/src/shell/overlay.rs`, import the state and add the variant:

```rust
use crate::dialogs::{
    ApiKeyInputState, ChoicePickerState, ConfirmDialogState, CustomEndpointWizardState,
    CustomRegistryImportState, HelpPanelState, McpAddFormState, McpManagerState,
    ModelSelectorState, ProviderManagerState, TabbedModelSelectorState, TextInputState,
    TrustDialogState, WorkspaceManagerState,
};

pub enum OverlayKind {
    // existing variants...
    CustomEndpointWizard(CustomEndpointWizardState),
}
```

Update `rich_dialog_lines`, `input_dialog_height`, and `focused_overlay_is_rich_dialog` matches to include `CustomEndpointWizard`.

- [ ] **Step 4: Add factory and action accessors**

In `crates/neo-tui/src/shell/dialog_factory.rs`, add:

```rust
pub fn open_custom_endpoint_wizard(
    &mut self,
    opts: crate::dialogs::CustomEndpointWizardOptions,
) -> OverlayId {
    let state = crate::dialogs::CustomEndpointWizardState::new(opts);
    self.push_overlay(Overlay::new(
        "custom-endpoint",
        OverlayKind::CustomEndpointWizard(state),
    ))
}

pub fn custom_endpoint_wizard_action(
    &self,
) -> Option<&crate::dialogs::CustomEndpointWizardAction> {
    let OverlayKind::CustomEndpointWizard(state) = &self.focused_overlay()?.kind else {
        return None;
    };
    state.action()
}

pub fn take_custom_endpoint_wizard_action(
    &mut self,
) -> Option<crate::dialogs::CustomEndpointWizardAction> {
    let OverlayKind::CustomEndpointWizard(state) = &mut self.focused_overlay_mut()?.kind else {
        return None;
    };
    state.take_action()
}

pub fn apply_custom_endpoint_fetched_models(
    &mut self,
    models: Vec<crate::dialogs::CustomEndpointFetchedModel>,
) -> bool {
    let Some(overlay) = self.focused_overlay_mut() else {
        return false;
    };
    let OverlayKind::CustomEndpointWizard(state) = &mut overlay.kind else {
        return false;
    };
    state.apply_fetched_models(models);
    true
}

pub fn apply_custom_endpoint_test_result(&mut self, result: Result<(), String>) -> bool {
    let Some(overlay) = self.focused_overlay_mut() else {
        return false;
    };
    let OverlayKind::CustomEndpointWizard(state) = &mut overlay.kind else {
        return false;
    };
    state.apply_test_result(result);
    true
}
```

- [ ] **Step 5: Route input**

In `crates/neo-tui/src/shell/input_dispatch.rs`, add:

```rust
OverlayKind::CustomEndpointWizard(state) => state.handle_input(&input),
```

Do not auto-close on `Submitted`; controller must decide when to close after saving or cancelling.

- [ ] **Step 6: Run exact overlay test**

Run:

```bash
cargo test --package neo-tui --lib shell::dialog_factory::tests::custom_endpoint_wizard_overlay_is_rich_dialog_and_blocks_prompt --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 7: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-tui/src/shell/overlay.rs crates/neo-tui/src/shell/dialog_factory.rs crates/neo-tui/src/shell/input_dispatch.rs
git commit -m "feat(tui): wire custom endpoint wizard overlay"
```

## Task 5: Interactive Controller Flow And Save

**Files:**
- Create: `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`
- Modify: `crates/neo-agent/src/modes/interactive/dialog_results.rs`

- [ ] **Step 1: Add Add Provider picker test**

In `crates/neo-agent/src/modes/interactive/tests.rs`, add:

```rust
#[tokio::test]
async fn add_provider_picker_includes_custom_endpoint() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let sessions_dir = temp.path().join(".neo/sessions");
    let mut controller = test_controller(temp.path());
    controller.local_config = Some(test_config(temp.path(), sessions_dir));

    controller.open_add_provider_picker();

    let visible = controller
        .tui
        .chrome()
        .focused_overlay_lines(80)
        .into_iter()
        .map(|line| neo_tui::primitive::strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(visible.contains("Known third-party provider"), "{visible}");
    assert!(visible.contains("Custom endpoint"), "{visible}");
    assert!(visible.contains("Custom registry (api.json)"), "{visible}");
}
```

- [ ] **Step 2: Run exact failing picker test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_provider_picker_includes_custom_endpoint --exact --nocapture --include-ignored
```

Expected: fail because `Custom endpoint` is absent.

- [ ] **Step 3: Add the picker item and route selected choice**

In `open_add_provider_picker`, insert the new item between known and custom registry:

```rust
neo_tui::dialogs::ChoiceItem::new("custom-endpoint", "Custom endpoint")
    .with_description("Configure a base URL and models"),
```

In `handle_selected_choice_item`, add a handler before `handle_catalog_choice_item`:

```rust
if self.handle_custom_endpoint_choice_item(id) {
    return;
}
```

- [ ] **Step 4: Create controller module skeleton**

Create `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`:

```rust
use std::path::PathBuf;

use neo_tui::dialogs::{
    CustomEndpointAuthDraft, CustomEndpointFetchedModel, CustomEndpointModelDraft,
    CustomEndpointProviderDraft, CustomEndpointWizardAction, CustomEndpointWizardOptions,
};

use super::InteractiveController;

pub(super) struct PendingCustomEndpointFetch {
    pub(super) handle: tokio::task::JoinHandle<anyhow::Result<Vec<CustomEndpointFetchedModel>>>,
}

impl InteractiveController {
    pub(super) fn handle_custom_endpoint_choice_item(&mut self, id: &str) -> bool {
        if id != "custom-endpoint" {
            return false;
        }
        let theme = self.tui.chrome().theme();
        self.tui
            .chrome_mut()
            .open_custom_endpoint_wizard(CustomEndpointWizardOptions { theme });
        true
    }

    pub(super) fn handle_custom_endpoint_wizard_action(&mut self) -> bool {
        let Some(action) = self.tui.chrome_mut().take_custom_endpoint_wizard_action() else {
            return false;
        };
        match action {
            CustomEndpointWizardAction::FetchModels => {
                self.start_custom_endpoint_fetch();
            }
            CustomEndpointWizardAction::TestConnection(draft) => {
                self.push_status(format!(
                    "Test connection is advisory; save provider '{}' when ready",
                    draft.provider_id
                ));
            }
            CustomEndpointWizardAction::Save(draft) => {
                self.save_custom_endpoint_provider(draft);
            }
            CustomEndpointWizardAction::Cancelled => {
                self.tui.chrome_mut().close_focused_overlay();
            }
        }
        true
    }
}
```

Add `mod custom_endpoint_provider;` in `interactive/mod.rs` and a field:

```rust
pending_custom_endpoint_fetch: Option<custom_endpoint_provider::PendingCustomEndpointFetch>,
```

Initialize it to `None`.

- [ ] **Step 5: Dispatch wizard actions**

In `process_provider_dialog_result`, before `choice_picker_result`, add:

```rust
} else if self.tui.chrome_mut().custom_endpoint_wizard_action().is_some() {
    self.handle_custom_endpoint_wizard_action();
```

- [ ] **Step 6: Run exact picker test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_provider_picker_includes_custom_endpoint --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 7: Add save-draft conversion helper**

In `custom_endpoint_provider.rs`, add conversion functions:

```rust
fn provider_config_from_draft(
    draft: &CustomEndpointProviderDraft,
) -> crate::config::ProviderConfig {
    let (api_key, api_key_env) = match &draft.auth {
        CustomEndpointAuthDraft::EnvVar(name) => (None, Some(name.clone())),
        CustomEndpointAuthDraft::InlineSecret(secret) => (Some(secret.clone()), None),
        CustomEndpointAuthDraft::LocalPlaceholder => (Some("local".to_owned()), None),
    };
    crate::config::ProviderConfig {
        provider_type: Some(draft.api_type),
        base_url: Some(draft.base_url.clone()),
        api_key,
        api_key_env,
    }
}

fn model_config_from_draft(
    provider_id: &str,
    model: &CustomEndpointModelDraft,
) -> crate::config::ModelConfig {
    let mut capabilities = Vec::new();
    if model.streaming {
        capabilities.push("streaming".to_owned());
    }
    if model.tools {
        capabilities.push("tools".to_owned());
    }
    if model.images {
        capabilities.push("images".to_owned());
    }
    if model.embeddings {
        capabilities.push("embeddings".to_owned());
    }
    if model.reasoning.supports_reasoning() {
        capabilities.push("reasoning".to_owned());
    }
    crate::config::ModelConfig {
        provider: provider_id.to_owned(),
        model: model.model_id.clone(),
        max_context_tokens: model.max_context_tokens,
        max_output_tokens: model.max_output_tokens,
        capabilities,
        reasoning: model.reasoning.clone(),
        display_name: model.display_name.clone(),
    }
}
```

Add `save_custom_endpoint_provider`:

```rust
fn save_custom_endpoint_provider(&mut self, draft: CustomEndpointProviderDraft) {
    let Some(config_path) = self.config_path() else {
        self.push_status("No config available");
        return;
    };
    let provider_config = provider_config_from_draft(&draft);
    let models = draft
        .models
        .iter()
        .map(|model| {
            (
                model.alias.clone(),
                model_config_from_draft(&draft.provider_id, model),
            )
        })
        .collect::<Vec<_>>();

    match crate::config::mutations::add_custom_endpoint_provider(
        &config_path,
        &draft.provider_id,
        provider_config,
        models,
        None,
    ) {
        Ok(message) => {
            self.tui.chrome_mut().close_focused_overlay();
            self.push_status(message);
            self.refresh_config();
        }
        Err(error) => {
            self.push_status(format!("Error: Failed to add custom endpoint: {error}"));
        }
    }
}
```

- [ ] **Step 8: Add conversion unit test in controller tests**

Add a focused test in `interactive/tests.rs` or a `#[cfg(test)]` module in `custom_endpoint_provider.rs`:

```rust
#[test]
fn custom_endpoint_model_conversion_adds_reasoning_capability_tag() {
    let model = neo_tui::dialogs::CustomEndpointModelDraft {
        source: neo_tui::dialogs::CustomEndpointModelSource::Manual,
        model_id: "reasoner".to_owned(),
        alias: "acme/reasoner".to_owned(),
        display_name: None,
        max_context_tokens: None,
        max_output_tokens: None,
        streaming: true,
        tools: true,
        images: false,
        embeddings: false,
        reasoning: neo_ai::ReasoningCapability::Toggle {
            disable_supported: true,
        },
    };

    let cfg = super::model_config_from_draft("acme", &model);
    assert_eq!(cfg.provider, "acme");
    assert_eq!(cfg.model, "reasoner");
    assert!(cfg.capabilities.iter().any(|cap| cap == "reasoning"));
}
```

- [ ] **Step 9: Run exact conversion test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::custom_endpoint_provider::tests::custom_endpoint_model_conversion_adds_reasoning_capability_tag --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 10: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs crates/neo-agent/src/modes/interactive/mod.rs crates/neo-agent/src/modes/interactive/dialog_results.rs crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(provider): open custom endpoint wizard"
```

## Task 6: OpenAI-Family `/models` Fetch

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`
- Modify: `crates/neo-agent/src/modes/interactive/mod.rs`

- [ ] **Step 1: Add model-list parser test**

In `custom_endpoint_provider.rs` tests, add:

```rust
#[test]
fn parses_openai_family_model_list_as_id_discovery() {
    let body = r#"
{
  "object": "list",
  "data": [
    {
      "id": "qwen2.5-coder-32b-instruct",
      "object": "model",
      "created": 1700000000,
      "owned_by": "acme",
      "context_length": 131072
    }
  ]
}
"#;

    let models = super::parse_openai_models_response(body).expect("parse models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "qwen2.5-coder-32b-instruct");
    assert_eq!(models[0].owned_by.as_deref(), Some("acme"));
    assert_eq!(models[0].created, Some(1_700_000_000));
}
```

- [ ] **Step 2: Implement parser**

Add:

```rust
#[derive(serde::Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelObject>,
}

#[derive(serde::Deserialize)]
struct OpenAiModelObject {
    id: String,
    #[serde(default)]
    created: Option<u64>,
    #[serde(default)]
    owned_by: Option<String>,
}

fn parse_openai_models_response(body: &str) -> anyhow::Result<Vec<CustomEndpointFetchedModel>> {
    let response: OpenAiModelsResponse = serde_json::from_str(body)?;
    Ok(response
        .data
        .into_iter()
        .filter(|model| !model.id.trim().is_empty())
        .map(|model| CustomEndpointFetchedModel {
            id: model.id,
            owned_by: model.owned_by,
            created: model.created,
        })
        .collect())
}
```

This intentionally ignores provider-specific extra fields such as `context_length`. Those fields are hints at best and must not bypass user review.

- [ ] **Step 3: Run exact parser test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::custom_endpoint_provider::tests::parses_openai_family_model_list_as_id_discovery --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 4: Implement fetch start and poll**

In `start_custom_endpoint_fetch`, read the current wizard draft through a new TUI accessor if needed. If the selected `ApiType` is not `OpenAi` or `OpenAiResponse`, push `Fetch from /models is only available for OpenAI-compatible protocols` and keep the wizard open.

Add:

```rust
async fn fetch_openai_family_models(
    base_url: String,
    bearer_token: String,
) -> anyhow::Result<Vec<CustomEndpointFetchedModel>> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(bearer_token)
        .send()
        .await?
        .error_for_status()?;
    let body = response.text().await?;
    parse_openai_models_response(&body)
}
```

Add `poll_pending_custom_endpoint_fetch` next to `poll_pending_catalog_fetch` calls:

```rust
pub(super) async fn poll_pending_custom_endpoint_fetch(&mut self) {
    let Some(pending) = self.pending_custom_endpoint_fetch.take() else {
        return;
    };
    if !pending.handle.is_finished() {
        self.pending_custom_endpoint_fetch = Some(pending);
        return;
    }
    self.tui.chrome_mut().set_custom_working_label(None);
    match pending.handle.await {
        Ok(Ok(models)) => {
            if models.is_empty() {
                self.push_status("No models returned from /models");
            } else if !self.tui.chrome_mut().apply_custom_endpoint_fetched_models(models) {
                self.push_status("Custom endpoint wizard is no longer open");
            }
        }
        Ok(Err(error)) => {
            self.push_status(format!("Error: Failed to fetch /models: {error}"));
        }
        Err(error) => {
            self.push_status(format!("Error: Failed to fetch /models: {error}"));
        }
    }
}
```

Call it in the interactive loop beside catalog and MCP polling:

```rust
self.poll_pending_custom_endpoint_fetch().await;
```

- [ ] **Step 5: Add auth resolution for fetch**

Implement:

```rust
fn bearer_token_for_auth(auth: &CustomEndpointAuthDraft) -> anyhow::Result<String> {
    match auth {
        CustomEndpointAuthDraft::EnvVar(name) => std::env::var(name)
            .map_err(|_| anyhow::anyhow!("environment variable {name} is not set")),
        CustomEndpointAuthDraft::InlineSecret(secret) => Ok(secret.clone()),
        CustomEndpointAuthDraft::LocalPlaceholder => Ok("local".to_owned()),
    }
}
```

Use it before spawning the fetch task.

- [ ] **Step 6: Run exact parser test and one controller compile test**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::custom_endpoint_provider::tests::parses_openai_family_model_list_as_id_discovery --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_provider_picker_includes_custom_endpoint --exact --nocapture --include-ignored
```

Expected: both pass.

- [ ] **Step 7: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs crates/neo-agent/src/modes/interactive/mod.rs
git commit -m "feat(provider): fetch custom endpoint model ids"
```

## Task 7: Advisory Test Connection

**Files:**
- Modify: `crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs`
- Modify: `crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs`

- [ ] **Step 1: Add test-result action test**

In `custom_endpoint_wizard.rs`, add:

```rust
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
```

- [ ] **Step 2: Implement `apply_test_result`**

Add:

```rust
pub fn apply_test_result(&mut self, result: Result<(), String>) {
    self.step = WizardStep::TestResult;
    self.validation_error = result.err();
    self.selected = 0;
}
```

Render `TestResult` as success when `validation_error` is `None`; render failure with `Edit auth`, `Save anyway`, and `Back to review` when it is `Some`.

- [ ] **Step 3: Implement controller test request**

For `CustomEndpointWizardAction::TestConnection`, use the first model in the draft and send a minimal request through the resolved provider. Keep it advisory:

```rust
fn start_custom_endpoint_test(&mut self, draft: CustomEndpointProviderDraft) {
    let Some(model) = draft.models.first().cloned() else {
        self.push_status("Add a model before testing connection");
        return;
    };
    match draft.api_type {
        neo_ai::ApiType::OpenAi | neo_ai::ApiType::OpenAiResponse => {
            let token = match bearer_token_for_auth(&draft.auth) {
                Ok(token) => token,
                Err(error) => {
                    let _ = self
                        .tui
                        .chrome_mut()
                        .apply_custom_endpoint_test_result(Err(error.to_string()));
                    return;
                }
            };
            self.tui
                .chrome_mut()
                .set_custom_working_label(Some(format!("Testing {}...", model.alias)));
            let base_url = draft.base_url.clone();
            let handle = tokio::spawn(async move {
                fetch_openai_family_models(base_url, token)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
            self.pending_custom_endpoint_test = Some(PendingCustomEndpointTest { handle });
        }
        neo_ai::ApiType::Anthropic | neo_ai::ApiType::Google => {
            let _ = self.tui.chrome_mut().apply_custom_endpoint_test_result(Err(
                "provider protocol does not expose /models in this wizard".to_owned(),
            ));
        }
    }
}
```

Add `PendingCustomEndpointTest { handle: JoinHandle<Result<(), String>> }` beside `PendingCustomEndpointFetch`, poll it beside fetch polling, and call `apply_custom_endpoint_test_result` with either `Ok(())` or `Err(message)`. Do not block saving on either outcome.

- [ ] **Step 4: Run exact TUI test**

Run:

```bash
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::test_result_render_offers_save_anyway_on_failure --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 5: Authorization-gated commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs
git commit -m "feat(provider): add advisory custom endpoint connection test"
```

## Task 8: Focused Final Verification

**Files:**
- Verify only touched targets.

- [ ] **Step 1: Run config mutation exact tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_custom_endpoint_provider_writes_provider_models_and_first_default_when_empty --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- config::mutations::tests::add_custom_endpoint_provider_replaces_existing_provider_models_only --exact --nocapture --include-ignored
```

Expected: both pass.

- [ ] **Step 2: Run TUI wizard exact tests**

Run:

```bash
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::api_type_render_includes_all_current_protocols --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::fetched_model_review_shows_blank_limits_as_review_points --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::all_selectable_screens_render_select_hint --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::provider_validation_rejects_uppercase_id --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::save_action_contains_reviewed_provider_and_model_draft --exact --nocapture --include-ignored
cargo test --package neo-tui --lib dialogs::custom_endpoint_wizard::tests::test_result_render_offers_save_anyway_on_failure --exact --nocapture --include-ignored
```

Expected: all pass.

- [ ] **Step 3: Run overlay exact test**

Run:

```bash
cargo test --package neo-tui --lib shell::dialog_factory::tests::custom_endpoint_wizard_overlay_is_rich_dialog_and_blocks_prompt --exact --nocapture --include-ignored
```

Expected: pass.

- [ ] **Step 4: Run interactive controller exact tests**

Run:

```bash
cargo test --package neo-agent --bin neo -- modes::interactive::tests::add_provider_picker_includes_custom_endpoint --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::custom_endpoint_provider::tests::custom_endpoint_model_conversion_adds_reasoning_capability_tag --exact --nocapture --include-ignored
cargo test --package neo-agent --bin neo -- modes::interactive::custom_endpoint_provider::tests::parses_openai_family_model_list_as_id_discovery --exact --nocapture --include-ignored
```

Expected: all pass.

- [ ] **Step 5: Run formatting check for touched Rust files**

Run:

```bash
rustfmt --check --edition 2024 \
  crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs \
  crates/neo-tui/src/dialogs/mod.rs \
  crates/neo-tui/src/shell/overlay.rs \
  crates/neo-tui/src/shell/dialog_factory.rs \
  crates/neo-tui/src/shell/input_dispatch.rs \
  crates/neo-agent/src/config/mutations.rs \
  crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs \
  crates/neo-agent/src/modes/interactive/mod.rs \
  crates/neo-agent/src/modes/interactive/dialog_results.rs
```

Expected: no output and exit 0.

- [ ] **Step 6: Run diff whitespace check**

Run:

```bash
git diff --check
```

Expected: no output and exit 0.

- [ ] **Step 7: Authorization-gated final commit checkpoint**

Do not run git mutations unless the user explicitly authorizes this exact checkpoint. If authorized, run:

```bash
git add \
  crates/neo-tui/src/dialogs/custom_endpoint_wizard.rs \
  crates/neo-tui/src/dialogs/mod.rs \
  crates/neo-tui/src/shell/overlay.rs \
  crates/neo-tui/src/shell/dialog_factory.rs \
  crates/neo-tui/src/shell/input_dispatch.rs \
  crates/neo-agent/src/config/mutations.rs \
  crates/neo-agent/src/modes/interactive/custom_endpoint_provider.rs \
  crates/neo-agent/src/modes/interactive/mod.rs \
  crates/neo-agent/src/modes/interactive/dialog_results.rs \
  crates/neo-agent/src/modes/interactive/tests.rs
git commit -m "feat(provider): add custom endpoint wizard"
```

## Self-Review

- Spec coverage: Add Provider entry, four API protocol choices, canonical config values, credential source mapping, `/models` id discovery, fetched-model review, typed reasoning, advisory test connection, save/replacement behavior, and focused verification are covered.
- Placeholder scan: The plan does not use `TBD`, `TODO`, "implement later", or vague "add error handling" steps. Each task names files, tests, commands, and concrete functions.
- Type consistency: Draft types flow from `neo-tui` to `neo-agent`, then into existing `ProviderConfig` and `ModelConfig`. `ReasoningCapability` remains typed throughout.
- Git policy: Commit steps are explicitly authorization-gated because this Neo thread forbids git mutations without per-instance user authorization.
