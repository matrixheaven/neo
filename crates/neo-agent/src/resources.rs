use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::Context;

const CONFIG_DIR: &str = ".neo";
const SYSTEM_PROMPT_FILE: &str = "SYSTEM.md";
const APPEND_SYSTEM_PROMPT_FILE: &str = "APPEND_SYSTEM.md";

pub(crate) fn load_system_prompt(project_dir: &Path) -> anyhow::Result<Option<String>> {
    let system_prompt =
        read_first_existing(&system_prompt_candidates(project_dir), "system prompt")?;
    let append_prompt = read_first_existing(
        &append_system_prompt_candidates(project_dir),
        "append system prompt",
    )?;

    Ok(join_system_prompt_parts([system_prompt, append_prompt]))
}

fn normalize_prompt(prompt: &str) -> String {
    prompt.trim().to_owned()
}

fn join_system_prompt_parts(parts: [Option<String>; 2]) -> Option<String> {
    let parts = parts
        .into_iter()
        .flatten()
        .map(|part| normalize_prompt(&part))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn read_first_existing(paths: &[PathBuf], description: &str) -> anyhow::Result<Option<String>> {
    for path in paths {
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(path)
            .with_context(|| format!("failed to read {description} {}", path.display()))?;
        return Ok(Some(content));
    }
    Ok(None)
}

fn system_prompt_candidates(project_dir: &Path) -> Vec<PathBuf> {
    resource_candidates(project_dir, SYSTEM_PROMPT_FILE)
}

fn append_system_prompt_candidates(project_dir: &Path) -> Vec<PathBuf> {
    resource_candidates(project_dir, APPEND_SYSTEM_PROMPT_FILE)
}

fn resource_candidates(project_dir: &Path, file_name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![project_dir.join(CONFIG_DIR).join(file_name)];
    if let Some(home) = home_dir() {
        candidates.push(home.join(CONFIG_DIR).join(file_name));
    }
    candidates
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_system_prompt_parts_trims_and_separates_non_empty_parts() {
        let prompt = join_system_prompt_parts([
            Some(" base instructions\n".to_owned()),
            Some("\nappend instructions ".to_owned()),
        ]);

        assert_eq!(
            prompt.as_deref(),
            Some("base instructions\n\nappend instructions")
        );
    }

    #[test]
    fn join_system_prompt_parts_omits_empty_parts() {
        let prompt = join_system_prompt_parts([Some(" \n".to_owned()), Some("append".to_owned())]);

        assert_eq!(prompt.as_deref(), Some("append"));
    }
}
