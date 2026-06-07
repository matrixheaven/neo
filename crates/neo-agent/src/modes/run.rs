use crate::config::AppConfig;

pub fn execute(prompt: &[String], config: &AppConfig) -> String {
    let prompt = prompt.join(" ");
    format!(
        "run placeholder: model={} prompt={prompt}\n",
        config.default_model
    )
}

pub fn resume(session_id: &str, config: &AppConfig) -> String {
    format!(
        "resume placeholder: session_id={session_id} sessions_dir={}\n",
        config.sessions_dir.display()
    )
}

pub fn list_models(config: &AppConfig) -> String {
    format!(
        "models:\n- {} (configured default)\n- fake (local placeholder)\n",
        config.default_model
    )
}

pub fn list_mcp_servers(_config: &AppConfig) -> String {
    "no MCP servers configured\n".to_owned()
}
