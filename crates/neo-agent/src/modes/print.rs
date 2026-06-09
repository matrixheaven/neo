use crate::config::AppConfig;

pub async fn execute(
    prompt: &[String],
    config: &AppConfig,
    session_target: Option<crate::modes::run::SessionTarget<'_>>,
    session_name: Option<&str>,
) -> anyhow::Result<String> {
    let turn = if let Some(session_target) = session_target {
        crate::modes::run::run_prompt_with_session_target(session_target, prompt, config).await?
    } else {
        crate::modes::run::run_prompt(prompt, config).await?
    };
    crate::modes::run::apply_session_name(config, &turn.session_id, session_name)?;
    Ok(format!("{}\n", turn.assistant_text))
}
