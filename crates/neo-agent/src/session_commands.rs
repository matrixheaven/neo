use std::{ffi::OsStr, fs};

use anyhow::Context;

use crate::config::AppConfig;

pub fn list(config: &AppConfig) -> anyhow::Result<String> {
    if !config.sessions_dir.exists() {
        return Ok("no sessions\n".to_owned());
    }

    let mut sessions = fs::read_dir(&config.sessions_dir)
        .with_context(|| {
            format!(
                "failed to read sessions directory {}",
                config.sessions_dir.display()
            )
        })?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension() == Some(OsStr::new("json")))
                .then(|| path.file_stem().map(OsStr::to_owned))
                .flatten()
        })
        .map(|name| name.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    sessions.sort_unstable();

    if sessions.is_empty() {
        Ok("no sessions\n".to_owned())
    } else {
        Ok(format!("{}\n", sessions.join("\n")))
    }
}

pub fn show(session_id: &str, config: &AppConfig) -> anyhow::Result<String> {
    let path = config.sessions_dir.join(format!("{session_id}.json"));
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session {}", path.display()))?;
    Ok(format!("{content}\n"))
}
