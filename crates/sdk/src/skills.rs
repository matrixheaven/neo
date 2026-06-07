use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default = "default_entrypoint")]
    pub entrypoint: String,
    #[serde(default)]
    pub resources: Vec<SkillResource>,
}

fn default_entrypoint() -> String {
    "SKILL.md".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillResource {
    pub path: String,
    #[serde(default)]
    pub kind: ResourceKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    #[default]
    Text,
    Binary,
    Executable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSkill {
    pub root: PathBuf,
    pub manifest: SkillManifest,
    pub body: String,
    pub resources: Vec<LoadedResource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedResource {
    pub spec: SkillResource,
    pub absolute_path: PathBuf,
    pub content: ResourceContent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceContent {
    Text(String),
    Binary(Vec<u8>),
}

impl ResourceContent {
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            Self::Binary(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SkillLoadOptions {
    pub load_resources: bool,
}

impl Default for SkillLoadOptions {
    fn default() -> Self {
        Self {
            load_resources: true,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillLoadError {
    #[error("failed to read skill file {path}: {source}")]
    ReadSkill {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("skill file {0} is missing TOML frontmatter")]
    MissingFrontmatter(PathBuf),
    #[error("failed to parse skill frontmatter in {path}: {source}")]
    ParseFrontmatter {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("resource path {path} escapes skill root {root}")]
    ResourceEscapesRoot { path: String, root: PathBuf },
    #[error("failed to read skill resource {path}: {source}")]
    ReadResource {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn load_skill(
    root: impl AsRef<Path>,
    options: SkillLoadOptions,
) -> Result<LoadedSkill, SkillLoadError> {
    let root = root.as_ref().to_path_buf();
    let skill_path = root.join("SKILL.md");
    let source = fs::read_to_string(&skill_path).map_err(|source| SkillLoadError::ReadSkill {
        path: skill_path.clone(),
        source,
    })?;
    let (frontmatter, body) = split_frontmatter(&source)
        .ok_or_else(|| SkillLoadError::MissingFrontmatter(skill_path.clone()))?;
    let manifest: SkillManifest =
        toml::from_str(frontmatter).map_err(|source| SkillLoadError::ParseFrontmatter {
            path: skill_path,
            source,
        })?;
    let resources = if options.load_resources {
        manifest
            .resources
            .iter()
            .map(|resource| load_resource(&root, resource))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    Ok(LoadedSkill {
        root,
        manifest,
        body: body.trim_start_matches('\n').to_owned(),
        resources,
    })
}

fn split_frontmatter(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix("---\n")?;
    let (frontmatter, body) = rest.split_once("\n---")?;
    Some((frontmatter, body.strip_prefix('\n').unwrap_or(body)))
}

fn load_resource(root: &Path, resource: &SkillResource) -> Result<LoadedResource, SkillLoadError> {
    let absolute_path = safe_join(root, &resource.path)?;
    let bytes = fs::read(&absolute_path).map_err(|source| SkillLoadError::ReadResource {
        path: absolute_path.clone(),
        source,
    })?;
    let content = match resource.kind {
        ResourceKind::Text | ResourceKind::Executable => {
            ResourceContent::Text(String::from_utf8_lossy(&bytes).into_owned())
        }
        ResourceKind::Binary => ResourceContent::Binary(bytes),
    };

    Ok(LoadedResource {
        spec: resource.clone(),
        absolute_path,
        content,
    })
}

fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, SkillLoadError> {
    let path = Path::new(relative);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(SkillLoadError::ResourceEscapesRoot {
                    path: relative.into(),
                    root: root.to_path_buf(),
                });
            }
        }
    }
    Ok(root.join(normalized))
}
