use crate::config::AppConfig;

pub fn execute(config: &AppConfig) -> String {
    format!(
        "neo interactive\nmodel: {}/{}\nmode: {}\ncommands: print, run, resume, sessions, models, config, mcp\n",
        config.default_provider, config.default_model, config.defaults.mode
    )
}
