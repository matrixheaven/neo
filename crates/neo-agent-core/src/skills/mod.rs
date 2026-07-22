use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

pub mod arguments;
pub mod builtin;
pub mod discovery;
pub mod metadata;

pub use arguments::{
    SkillArgumentError, SkillInvocation, expand_skill_body, parse_skill_invocation,
};
pub use discovery::{SkillSource, discover_skills};
pub use metadata::{
    SkillHostMetadata, SkillInterface, SkillToolDependency, load_host_metadata,
    serialize_host_metadata,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
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
}

impl SkillManifest {
    #[must_use]
    pub const fn auto_invokable(&self) -> bool {
        !self.disable_model_invocation
    }
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
    pub host_metadata: SkillHostMetadata,
}

impl LoadedSkill {
    #[must_use]
    pub fn is_builtin_extracted(&self) -> bool {
        self.root.components().any(|c| c.as_os_str() == ".builtin")
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        self.host_metadata.display_name(&self.name)
    }

    #[must_use]
    pub fn short_description(&self) -> Option<&str> {
        self.host_metadata.short_description()
    }
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
    /// Load skills from all tiers. If `user_dir` is provided, built-in skills
    /// are extracted into `user_dir/skills/.builtin/` first and then loaded
    /// from disk, so users can inspect and override them.
    pub fn load(
        user_dirs: &[PathBuf],
        extra_dirs: &[PathBuf],
        builtin_skills: Vec<LoadedSkill>,
    ) -> Result<Self, SkillLoadError> {
        let mut skills: HashMap<String, LoadedSkill> = HashMap::new();

        // Extract built-in skills into the first user dir (usually ~/.neo) so
        // they are visible on disk and can be overridden by user skills.
        let extracted = if let Some(user_dir) = user_dirs.first() {
            crate::skills::builtin::extract_builtin_skills(user_dir).unwrap_or(builtin_skills)
        } else {
            builtin_skills
        };
        for skill in extracted {
            skills.insert(skill.name.clone(), skill);
        }

        for dir in extra_dirs {
            for skill in discover_skills(dir, SkillSource::Extra)? {
                skills.insert(skill.name.clone(), skill);
            }
        }

        for dir in user_dirs {
            for skill in discover_skills(dir, SkillSource::User)? {
                if skill.is_builtin_extracted() {
                    continue;
                }
                skills.insert(skill.name.clone(), skill);
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
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    #[must_use]
    pub fn auto_invokable(&self) -> Vec<&LoadedSkill> {
        let mut skills: Vec<_> = self
            .skills
            .values()
            .filter(|skill| skill.manifest.auto_invokable())
            .collect();
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        skills
    }

    #[must_use]
    pub fn available_skills_prompt(&self) -> String {
        let skills = self.auto_invokable();
        let mut prompt = String::from("<available_skills>\n");
        prompt.push_str(
            "DISREGARD any earlier skill listings. Current available skills:\n\n\
             Skills are reusable capabilities. When a skill matches your current task, \
             invoke it with the Skill tool instead of doing the work manually.\n\n\
             MANDATORY: When the user mentions a task that a skill listed below could \
             help with, or when the user explicitly asks to use a skill, your FIRST \
             action must be a Skill tool call for that skill. Do not start any work, \
             exploration, or planning before invoking the matching skill.\n",
        );

        let groups: [(SkillSource, &str); 3] = [
            (SkillSource::User, "User"),
            (SkillSource::Extra, "Extra"),
            (SkillSource::Builtin, "Built-in"),
        ];
        for (source, label) in groups {
            let group_skills: Vec<_> = skills
                .iter()
                .filter(|skill| skill.source == source)
                .collect();
            if group_skills.is_empty() {
                continue;
            }
            let _ = write!(prompt, "\n### {label}\n");
            for skill in group_skills {
                write_available_skill(&mut prompt, skill);
            }
        }
        if skills.is_empty() {
            prompt.push_str("\nNo skills are currently available.\n");
        }
        prompt.push_str("</available_skills>");
        prompt
    }
}

fn write_available_skill(prompt: &mut String, skill: &LoadedSkill) {
    let _ = writeln!(prompt, "- {}: {}", skill.name, skill.manifest.description);
    if let Some(when) = &skill.manifest.when_to_use {
        let _ = writeln!(prompt, "  When to use: {when}");
    }
    if skill.manifest.arguments.is_empty() {
        return;
    }
    prompt.push_str("<arguments>\n");
    for argument in &skill.manifest.arguments {
        let _ = write!(prompt, "<arg name=\"{}\"", argument.name);
        if argument.required {
            prompt.push_str(" required=\"true\"");
        }
        if let Some(default) = &argument.default {
            let _ = write!(prompt, " default=\"{}\"", escape_xml(default));
        }
        prompt.push_str(" />\n");
    }
    prompt.push_str("</arguments>\n");
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[derive(Debug, Clone, Default)]
pub struct SkillStoreHandle {
    inner: Arc<RwLock<SkillStore>>,
}

impl SkillStoreHandle {
    #[must_use]
    pub fn new(store: SkillStore) -> Self {
        Self {
            inner: Arc::new(RwLock::new(store)),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> SkillStore {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn replace(&self, store: SkillStore) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = store;
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<LoadedSkill> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned()
    }

    pub fn with_store<T>(&self, f: impl FnOnce(&SkillStore) -> T) -> T {
        let store = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&store)
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
    let manifest = parse_skill_manifest(frontmatter).map_err(|parse_err| {
        SkillLoadError::ParseFrontmatter {
            path: path.to_path_buf(),
            source: parse_err,
        }
    })?;
    let root = path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let name = manifest.name.clone();
    let (host_metadata, _diagnostics) = metadata::load_host_metadata(&root);

    Ok(LoadedSkill {
        name,
        root,
        manifest,
        body: body.trim_start_matches('\n').to_owned(),
        source: skill_source,
        host_metadata,
    })
}

fn parse_skill_manifest(frontmatter: &str) -> Result<SkillManifest, serde_yaml::Error> {
    match serde_yaml::from_str(frontmatter) {
        Ok(manifest) => Ok(manifest),
        Err(original_error) => {
            let Some(repaired) = repair_frontmatter_scalar_fields(frontmatter) else {
                return Err(original_error);
            };
            serde_yaml::from_str(&repaired).map_err(|_| original_error)
        }
    }
}

// Match Codex's bounded repair for third-party scalar prose while keeping strict YAML primary.
fn repair_frontmatter_scalar_fields(frontmatter: &str) -> Option<String> {
    let mut changed = false;
    let mut block_scalar_indent: Option<usize> = None;
    let mut repaired_lines = Vec::new();
    for line in frontmatter.lines() {
        let indent = line
            .chars()
            .take_while(|character| *character == ' ')
            .count();
        if let Some(block_indent) = block_scalar_indent {
            if line.trim().is_empty() || indent > block_indent {
                repaired_lines.push(line.to_owned());
                continue;
            }
            block_scalar_indent = None;
        }

        let Some((key, value)) = line.split_once(':') else {
            repaired_lines.push(line.to_owned());
            continue;
        };
        if key.trim().is_empty() || !value.chars().next().is_none_or(char::is_whitespace) {
            repaired_lines.push(line.to_owned());
            continue;
        }

        let trimmed_start = value.trim_start();
        let leading_whitespace = &value[..value.len() - trimmed_start.len()];
        let mut scalar = trimmed_start;
        let mut comment = "";
        for (index, character) in trimmed_start.char_indices() {
            if character == '#'
                && (index == 0
                    || trimmed_start[..index]
                        .chars()
                        .next_back()
                        .is_some_and(char::is_whitespace))
            {
                let comment_start = trimmed_start[..index].trim_end().len();
                scalar = &trimmed_start[..comment_start];
                comment = &trimmed_start[comment_start..];
                break;
            }
        }

        let scalar = scalar.trim_end();
        let Some(first_char) = scalar.chars().next() else {
            repaired_lines.push(line.to_owned());
            continue;
        };
        if matches!(first_char, '|' | '>') {
            block_scalar_indent = Some(indent);
            repaired_lines.push(line.to_owned());
            continue;
        }
        if matches!(first_char, '\'' | '"') {
            repaired_lines.push(line.to_owned());
            continue;
        }

        let has_colon_separator = scalar
            .chars()
            .zip(scalar.chars().skip(1))
            .any(|(character, next)| character == ':' && next.is_whitespace());
        let invalid_flow_like_scalar = matches!(first_char, '[' | '{' | '@' | '`')
            && serde_yaml::from_str::<serde_yaml::Value>(scalar).is_err();
        if !has_colon_separator && !invalid_flow_like_scalar {
            repaired_lines.push(line.to_owned());
            continue;
        }

        let quoted_scalar = format!("'{}'", scalar.replace('\'', "''"));
        repaired_lines.push(format!(
            "{key}:{leading_whitespace}{quoted_scalar}{comment}"
        ));
        changed = true;
    }
    changed.then(|| repaired_lines.join("\n"))
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
