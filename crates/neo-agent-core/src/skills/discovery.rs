use std::path::{Path, PathBuf};

use super::{LoadedSkill, SkillLoadError, load_skill_file};

const RESOURCE_DIRS: &[&str] = &["references", "scripts", "assets"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkillSource {
    #[default]
    Builtin,
    Extra,
    User,
}

pub fn discover_skills(
    root: &Path,
    source: SkillSource,
) -> Result<Vec<LoadedSkill>, SkillLoadError> {
    let mut skills = Vec::new();
    if !root.is_dir() {
        return Ok(skills);
    }

    skills.extend(discover_recursive(root, source, "")?);
    Ok(skills)
}

fn discover_recursive(
    dir: &Path,
    source: SkillSource,
    prefix: &str,
) -> Result<Vec<LoadedSkill>, SkillLoadError> {
    let mut skills = Vec::new();
    let own_skill_file = dir.join("SKILL.md");
    let own_prefix = if own_skill_file.is_file() {
        let skill = load_skill_file(&own_skill_file, source)?;
        let name = if prefix.is_empty() {
            skill.name.clone()
        } else {
            format!("{prefix}/{}", skill.name)
        };
        skills.push(LoadedSkill { name, ..skill });
        skills
            .last()
            .map_or_else(|| prefix.to_owned(), |s| s.name.clone())
    } else {
        prefix.to_owned()
    };

    let mut entries = std::fs::read_dir(dir)
        .map_err(|source| SkillLoadError::ReadSkill {
            path: dir.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| SkillLoadError::ReadSkill {
            path: dir.to_path_buf(),
            source,
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if own_skill_file.is_file()
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|file_name| RESOURCE_DIRS.contains(&file_name))
            {
                continue;
            }
            skills.extend(discover_recursive(&path, source, &own_prefix)?);
        }
    }

    Ok(skills)
}

#[must_use]
pub fn user_skill_dirs(user_dir: &Path) -> Vec<PathBuf> {
    vec![user_dir.join("skills")]
}
