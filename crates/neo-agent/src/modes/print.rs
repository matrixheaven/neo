use crate::config::AppConfig;

pub async fn execute(prompt: &[String], config: &AppConfig) -> anyhow::Result<String> {
    let turn = crate::modes::run::run_prompt(prompt, config).await?;
    Ok(format!("{}\n", turn.assistant_text))
}
