use std::{
    collections::HashSet,
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use neo_sdk::{SkillLoadOptions, load_skill};

const CONFIG_DIR: &str = ".neo";
const SYSTEM_PROMPT_FILE: &str = "SYSTEM.md";
const APPEND_SYSTEM_PROMPT_FILE: &str = "APPEND_SYSTEM.md";
const CONTEXT_FILE_CANDIDATES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

pub(crate) fn load_system_prompt(
    project_dir: &Path,
    explicit_system_prompt: Option<&str>,
    explicit_append_system_prompts: &[String],
    explicit_skill_paths: &[PathBuf],
    no_context_files: bool,
    project_trusted: bool,
) -> anyhow::Result<Option<String>> {
    let system_prompt = match explicit_system_prompt {
        Some(prompt) => Some(resolve_prompt_input(prompt, "system prompt")?),
        None => read_first_existing(&system_prompt_candidates(project_dir), "system prompt")?,
    };
    let mut append_prompts = if explicit_append_system_prompts.is_empty() {
        read_first_existing(
            &append_system_prompt_candidates(project_dir),
            "append system prompt",
        )?
        .into_iter()
        .collect()
    } else {
        explicit_append_system_prompts
            .iter()
            .map(|prompt| resolve_prompt_input(prompt, "append system prompt"))
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    append_prompts.extend(load_explicit_skill_prompts(explicit_skill_paths)?);
    if !no_context_files
        && let Some(project_context) =
            format_project_context(&load_context_files(project_dir, project_trusted)?)
    {
        append_prompts.push(project_context);
    }

    Ok(join_system_prompt_parts(system_prompt, append_prompts))
}

#[derive(Debug, Clone)]
struct ContextFile {
    path: PathBuf,
    content: String,
}

fn load_context_files(
    project_dir: &Path,
    project_trusted: bool,
) -> anyhow::Result<Vec<ContextFile>> {
    let mut context_files = Vec::new();
    let mut seen = HashSet::new();

    if let Some(home) = home_dir()
        && let Some(global_context) = load_context_file_from_dir(&home.join(CONFIG_DIR))?
    {
        seen.insert(global_context.path.clone());
        context_files.push(global_context);
    }

    if project_trusted {
        for directory in project_context_directories(project_dir) {
            if let Some(context_file) = load_context_file_from_dir(&directory)?
                && seen.insert(context_file.path.clone())
            {
                context_files.push(context_file);
            }
        }
    }

    Ok(context_files)
}

fn project_context_directories(project_dir: &Path) -> Vec<PathBuf> {
    project_dir
        .ancestors()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn load_context_file_from_dir(dir: &Path) -> anyhow::Result<Option<ContextFile>> {
    for file_name in CONTEXT_FILE_CANDIDATES {
        let path = dir.join(file_name);
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read context file {}", path.display()))?;
        return Ok(Some(ContextFile { path, content }));
    }
    Ok(None)
}

fn format_project_context(context_files: &[ContextFile]) -> Option<String> {
    if context_files.is_empty() {
        return None;
    }

    let mut prompt =
        String::from("<project_context>\n\nProject-specific instructions and guidelines:\n\n");
    for context_file in context_files {
        writeln!(
            prompt,
            "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n",
            context_file.path.display(),
            context_file.content.trim_end()
        )
        .expect("writing to String should not fail");
    }
    prompt.push_str("</project_context>");
    Some(prompt)
}

fn load_explicit_skill_prompts(paths: &[PathBuf]) -> anyhow::Result<Vec<String>> {
    paths
        .iter()
        .map(|path| {
            let skill = load_skill(
                path,
                SkillLoadOptions {
                    load_resources: false,
                },
            )
            .with_context(|| format!("failed to load skill {}", path.display()))?;
            Ok(format!(
                "<skill name=\"{}\" description=\"{}\">\n{}\n</skill>",
                skill.manifest.name,
                skill.manifest.description,
                skill.body.trim()
            ))
        })
        .collect()
}

fn resolve_prompt_input(input: &str, description: &str) -> anyhow::Result<String> {
    let path = Path::new(input);
    if path.exists() {
        return fs::read_to_string(path)
            .with_context(|| format!("failed to read {description} {}", path.display()));
    }
    Ok(input.to_owned())
}

fn normalize_prompt(prompt: &str) -> String {
    prompt.trim().to_owned()
}

fn join_system_prompt_parts(
    system_prompt: Option<String>,
    append_prompts: Vec<String>,
) -> Option<String> {
    let parts = system_prompt
        .into_iter()
        .chain(append_prompts)
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
        let prompt = join_system_prompt_parts(
            Some(" base instructions\n".to_owned()),
            vec!["\nappend instructions ".to_owned()],
        );

        assert_eq!(
            prompt.as_deref(),
            Some("base instructions\n\nappend instructions")
        );
    }

    #[test]
    fn join_system_prompt_parts_omits_empty_parts() {
        let prompt = join_system_prompt_parts(Some(" \n".to_owned()), vec!["append".to_owned()]);

        assert_eq!(prompt.as_deref(), Some("append"));
    }

    #[test]
    fn project_context_directories_returns_ancestors_before_project() {
        let directories = project_context_directories(Path::new("/workspace/repo/crate"));

        assert_eq!(
            directories,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/workspace"),
                PathBuf::from("/workspace/repo"),
                PathBuf::from("/workspace/repo/crate"),
            ]
        );
    }

    #[test]
    fn format_project_context_uses_pi_project_instruction_shape() {
        let prompt = format_project_context(&[ContextFile {
            path: PathBuf::from("/repo/AGENTS.md"),
            content: "Follow repo rules.\n".to_owned(),
        }]);

        assert_eq!(
            prompt.as_deref(),
            Some(
                "<project_context>\n\nProject-specific instructions and guidelines:\n\n<project_instructions path=\"/repo/AGENTS.md\">\nFollow repo rules.\n</project_instructions>\n\n</project_context>"
            )
        );
    }
}
