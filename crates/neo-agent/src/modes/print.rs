use crate::config::AppConfig;

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    session_id: Option<&str>,
) -> anyhow::Result<String> {
    let turn = if let Some(session_id) = session_id {
        crate::modes::run::run_prompt_with_session_id(session_id, prompt, config).await?
    } else {
        crate::modes::run::run_prompt(prompt, config).await?
    };
    Ok(format!("{}\n", turn.assistant_text))
}
