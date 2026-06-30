//! Resource loading for system prompts, skills, and project context files.
//!
//! Trust gating: project-local resources (`AGENTS.md`, `CLAUDE.md`,
//! `.neo/SYSTEM.md`, `.neo/APPEND_SYSTEM.md`, project skills, and any future
//! project-local MCP configuration) must only be loaded when the workspace is
//! trusted. User-global resources under `~/.neo` are always loaded.

use std::{
    collections::HashSet,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use crate::config::expand_user_path;
#[cfg(test)]
use crate::config::expand_user_path_with_home;
use crate::trust::find_context_files_in_dir;

use anyhow::Context;
use neo_agent_core::skills::{LoadedSkill, SkillStore, builtin::builtin_skills, discovery};

const SYSTEM_PROMPT_FILE: &str = "SYSTEM.md";
const APPEND_SYSTEM_PROMPT_FILE: &str = "APPEND_SYSTEM.md";

/// Load the system prompt for a turn.
///
/// Trust gate: project-local `.neo/SYSTEM.md` and `.neo/APPEND_SYSTEM.md` are
/// not loaded. Only user-global files under `~/.neo` are considered. If a
/// project-local system prompt path is introduced later, it must be gated by
/// `project_trusted` the same way `load_context_files` gates project context
/// files.
pub(crate) fn load_system_prompt(
    project_dir: &Path,
    project_trusted: bool,
    skill_store: &SkillStore,
) -> anyhow::Result<Option<String>> {
    let system_prompt = read_first_existing(&system_prompt_candidates(), "system prompt")?;
    let mut append_prompts: Vec<String> =
        read_first_existing(&append_system_prompt_candidates(), "append system prompt")?
            .into_iter()
            .collect();
    if let Some(available_skills) = format_available_skills(skill_store) {
        append_prompts.push(available_skills);
    }
    if let Some(project_context) =
        format_project_context(&load_context_files(project_dir, project_trusted)?)
    {
        append_prompts.push(project_context);
    }

    Ok(join_system_prompt_parts(system_prompt, append_prompts))
}

/// Load the skill store for the session.
///
/// Trust gate: project-local skills (e.g. `.agents/skills/`) are not loaded.
/// Only user-global skills under `~/.neo/skills`, extra skill directories from
/// config, and built-in skills are included. If project-local skill discovery
/// is introduced later, it must be skipped when `project_trusted` is `false`.
pub(crate) fn load_skill_store(
    user_dir: Option<&Path>,
    extra_dirs: &[String],
    skill_path: &[String],
) -> anyhow::Result<SkillStore> {
    let mut extra = Vec::new();
    for dir in extra_dirs {
        extra.push(expand_user_path(PathBuf::from(dir)));
    }
    for dir in skill_path {
        extra.push(expand_user_path(PathBuf::from(dir)));
    }
    let mut user = Vec::new();
    if let Some(user_dir) = user_dir {
        user.extend(discovery::user_skill_dirs(user_dir));
    }
    SkillStore::load(&user, &extra, builtin_skills()?).map_err(anyhow::Error::from)
}

#[derive(Debug, Clone)]
struct ContextFile {
    path: PathBuf,
    content: String,
}

/// Load project context files (`AGENTS.md`, `CLAUDE.md`, and variants).
///
/// Trust gate: project-local context files are loaded only when
/// `project_trusted` is `true`. User-global context files under `~/.neo` are
/// always loaded. This gate must remain in place for all project-local context
/// file loading.
fn load_context_files(
    project_dir: &Path,
    project_trusted: bool,
) -> anyhow::Result<Vec<ContextFile>> {
    let mut context_files = Vec::new();
    let mut seen = HashSet::new();

    if let Some(home) = crate::config::neo_home()
        && let Some(global_context) = load_context_file_from_dir(&home)?
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
    let Some(path) = find_context_files_in_dir(dir).into_iter().next() else {
        return Ok(None);
    };
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read context file {}", path.display()))?;
    Ok(Some(ContextFile { path, content }))
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

fn format_available_skills(skill_store: &SkillStore) -> Option<String> {
    let skills: Vec<&LoadedSkill> = skill_store.auto_invokable();
    if skills.is_empty() {
        return None;
    }
    let mut prompt = String::from("<available_skills>\n");
    for skill in skills {
        write_available_skill(&mut prompt, skill);
    }
    prompt.push_str("</available_skills>");
    Some(prompt)
}

fn write_available_skill(prompt: &mut String, skill: &LoadedSkill) {
    prompt.push_str("<skill name=\"");
    prompt.push_str(&skill.name);
    prompt.push_str("\" description=\"");
    prompt.push_str(&xml_escape(&skill.manifest.description));
    prompt.push('"');
    if let Some(when) = &skill.manifest.when_to_use {
        prompt.push_str(" whenToUse=\"");
        prompt.push_str(&xml_escape(when));
        prompt.push('"');
    }
    prompt.push_str(">\n");
    write_skill_arguments(prompt, skill);
    prompt.push_str("</skill>\n");
}

fn write_skill_arguments(prompt: &mut String, skill: &LoadedSkill) {
    if skill.manifest.arguments.is_empty() {
        return;
    }
    prompt.push_str("<arguments>\n");
    for arg in &skill.manifest.arguments {
        write_skill_argument(prompt, arg);
    }
    prompt.push_str("</arguments>\n");
}

fn write_skill_argument(prompt: &mut String, arg: &neo_agent_core::skills::SkillArgument) {
    prompt.push_str("<arg name=\"");
    prompt.push_str(&arg.name);
    prompt.push('"');
    if arg.required {
        prompt.push_str(" required=\"true\"");
    }
    if let Some(default) = &arg.default {
        prompt.push_str(" default=\"");
        prompt.push_str(&xml_escape(default));
        prompt.push('"');
    }
    prompt.push_str(" />\n");
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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

fn system_prompt_candidates() -> Vec<PathBuf> {
    resource_candidates(SYSTEM_PROMPT_FILE)
}

fn append_system_prompt_candidates() -> Vec<PathBuf> {
    resource_candidates(APPEND_SYSTEM_PROMPT_FILE)
}

fn resource_candidates(file_name: &str) -> Vec<PathBuf> {
    // System/append prompts live only under the neo home (`~/.neo`).
    crate::config::neo_home()
        .map(|home| vec![home.join(file_name)])
        .unwrap_or_default()
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

    #[test]
    fn skill_path_tilde_expands_to_user_home() {
        assert_eq!(
            expand_user_path_with_home(
                PathBuf::from("~/.agents/skills"),
                Some(Path::new("/home/alice")),
            ),
            PathBuf::from("/home/alice/.agents/skills")
        );
    }
}
