//! providers behavior (moved from `mod.rs`).

use super::*;

/// Regression: the model display label must never stitch the provider onto
/// an already-qualified alias. When `default_model` is a `<provider>/<model>`
/// alias, `default_model_label()` returns it as-is; otherwise it prefixes
/// `default_provider/`. This avoids both
/// `deepseek/minimax-.../MiniMax-M2` (stale provider) and
/// `minimax-.../minimax-.../MiniMax-M2` (double prefix).
#[test]
fn default_model_label_never_double_prefixes() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r#"
default_model = "minimax-cn-coding-plan/MiniMax-M2"
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
base_url = "https://deepseek.example/v1"

[providers."minimax-cn-coding-plan"]
type = "anthropic"
base_url = "https://api.minimaxi.com/anthropic/v1"

[models."minimax-cn-coding-plan/MiniMax-M2"]
provider = "minimax-cn-coding-plan"
model = "MiniMax-M2"
"#,
    );
    let config = load_config(config_path, project_dir);
    // Alias is used as-is: no stale deepseek prefix, no double minimax prefix.
    assert_eq!(
        config.default_model_label(),
        "minimax-cn-coding-plan/MiniMax-M2"
    );
}

/// When `default_model` has no `/` (a plain model id), the label prefixes
/// `default_provider/`.
#[test]
fn default_model_label_prefixes_provider_for_plain_model_id() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r#"
default_model = "deepseek-v4-pro"
default_provider = "deepseek"

[providers.deepseek]
type = "openai"
base_url = "https://deepseek.example/v1"
"#,
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.default_model_label(), "deepseek/deepseek-v4-pro");
}

#[test]
fn default_model_label_resolves_unqualified_alias() {
    let (_temp, config_path, project_dir) = temp_project_config(
        r#"
default_model = "fast"
default_provider = "openai"

[providers.openai]
type = "openai_response"

[models.fast]
provider = "openai"
model = "gpt-4.1"
"#,
    );
    let config = load_config(config_path, project_dir);
    assert_eq!(config.default_model_label(), "openai/gpt-4.1");
}
