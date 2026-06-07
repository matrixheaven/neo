use crate::config::AppConfig;

pub fn execute(config: &AppConfig) -> String {
    format!(
        "neo interactive placeholder: model={} mode={}\n",
        config.default_model, config.defaults.mode
    )
}
