use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub mod arguments;
pub mod builtin;
pub mod discovery;

pub use arguments::{
    SkillArgumentError, SkillInvocation, expand_skill_body, parse_skill_invocation,
};
pub use discovery::{SkillSource, discover_skills};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    #[serde(default, rename = "type")]
    pub skill_type: SkillType,
    #[serde(default, alias = "whenToUse", alias = "when_to_use")]
    pub when_to_use: Option<String>,
    #[serde(
        default,
        alias = "disableModelInvocation",
        alias = "disable_model_invocation"
    )]
    pub disable_model_invocation: bool,
    #[serde(default)]
    pub arguments: Vec<SkillArgument>,
    #[serde(default, alias = "slashCommands", alias = "slash_commands")]
    pub slash_commands: Vec<String>,
}

impl SkillManifest {
    #[must_use]
    pub fn auto_invokable(&self) -> bool {
        !self.disable_model_invocation && !matches!(self.skill_type, SkillType::Flow)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillType {
    #[default]
    Prompt,
    Inline,
    Flow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillArgument {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    pub name: String,
    pub root: PathBuf,
    pub manifest: SkillManifest,
    pub body: String,
    pub source: SkillSource,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillLoadError {
    #[error("failed to read skill file {path}: {source}")]
    ReadSkill {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("skill file {0} is missing YAML frontmatter")]
    MissingFrontmatter(PathBuf),
    #[error("failed to parse skill frontmatter in {path}: {source}")]
    ParseFrontmatter {
        path: PathBuf,
        source: serde_yaml::Error,
    },
}

#[derive(Debug, Clone, Default)]
pub struct SkillStore {
    skills: HashMap<String, LoadedSkill>,
}

impl SkillStore {
    pub fn load(
        project_dir: Option<&Path>,
        user_dirs: &[PathBuf],
        extra_dirs: &[PathBuf],
        builtin_skills: Vec<LoadedSkill>,
    ) -> Result<Self, SkillLoadError> {
        let mut skills: HashMap<String, LoadedSkill> = HashMap::new();

        for skill in builtin_skills {
            skills.insert(skill.name.clone(), skill);
        }

        for dir in extra_dirs {
            for skill in discover_skills(dir, SkillSource::Extra)? {
                skills.insert(skill.name.clone(), skill);
            }
        }

        for dir in user_dirs {
            for skill in discover_skills(dir, SkillSource::User)? {
                skills.insert(skill.name.clone(), skill);
            }
        }

        if let Some(project_dir) = project_dir {
            let project_skills_dir = project_dir.join(".neo").join("skills");
            let agents_skills_dir = project_dir.join(".agents").join("skills");
            for dir in [project_skills_dir, agents_skills_dir] {
                for skill in discover_skills(&dir, SkillSource::Project)? {
                    skills.insert(skill.name.clone(), skill);
                }
            }
        }

        Ok(Self { skills })
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&LoadedSkill> {
        self.skills.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LoadedSkill> {
        self.skills.values()
    }

    #[must_use]
    pub fn auto_invokable(&self) -> Vec<&LoadedSkill> {
        self.skills
            .values()
            .filter(|skill| skill.manifest.auto_invokable())
            .collect()
    }

    #[must_use]
    pub fn available_for_slash(&self) -> Vec<&LoadedSkill> {
        self.skills.values().collect()
    }
}

pub fn load_skill_file(
    path: &Path,
    skill_source: SkillSource,
) -> Result<LoadedSkill, SkillLoadError> {
    let source = std::fs::read_to_string(path).map_err(|source| SkillLoadError::ReadSkill {
        path: path.to_path_buf(),
        source,
    })?;
    let (frontmatter, body) = split_frontmatter(&source)
        .ok_or_else(|| SkillLoadError::MissingFrontmatter(path.to_path_buf()))?;
    let manifest: SkillManifest = serde_yaml::from_str(frontmatter).map_err(|parse_err| {
        SkillLoadError::ParseFrontmatter {
            path: path.to_path_buf(),
            source: parse_err,
        }
    })?;
    let root = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let name = manifest.name.clone();

    Ok(LoadedSkill {
        name,
        root,
        manifest,
        body: body.trim_start_matches('\n').to_owned(),
        source: skill_source,
    })
}

#[must_use]
pub fn split_frontmatter(source: &str) -> Option<(&str, &str)> {
    let rest = source
        .strip_prefix("---\r\n")
        .or_else(|| source.strip_prefix("---\n"))?;
    let separator_start = rest.find("\n---")?;
    let frontmatter = rest[..separator_start]
        .strip_suffix('\r')
        .unwrap_or(&rest[..separator_start]);
    let after_separator = rest[separator_start + 1..].strip_prefix("---")?;
    let body = if let Some(body) = after_separator.strip_prefix("\r\n") {
        body
    } else if let Some(body) = after_separator.strip_prefix('\n') {
        body
    } else if after_separator.is_empty() {
        after_separator
    } else {
        return None;
    };
    Some((frontmatter, body))
}
