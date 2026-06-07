use crate::config::AppConfig;

pub fn execute(prompt: &[String], _config: &AppConfig) -> String {
    format!("{}\n", prompt.join(" "))
}
