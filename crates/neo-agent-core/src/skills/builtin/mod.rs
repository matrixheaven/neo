use std::path::PathBuf;

use crate::skills::{LoadedSkill, SkillManifest, SkillSource};

const WRITE_GOAL: &str = include_str!("write-goal.md");
const UPDATE_CONFIG: &str = include_str!("update-config.md");
const MCP_CONFIG: &str = include_str!("mcp-config.md");
const CUSTOM_THEME: &str = include_str!("custom-theme.md");

#[derive(Debug, thiserror::Error)]
pub enum BuiltinSkillError {
    #[error("failed to load built-in skill: {0}")]
    Load(#[from] crate::skills::SkillLoadError),
}

pub fn builtin_skills() -> Result<Vec<LoadedSkill>, BuiltinSkillError> {
    let mut skills = Vec::new();
    for source in [WRITE_GOAL, UPDATE_CONFIG, MCP_CONFIG, CUSTOM_THEME] {
        let skill = load_builtin_skill(source)?;
        skills.push(skill);
    }
    Ok(skills)
}

#[allow(clippy::unnecessary_wraps)]
fn load_builtin_skill(source: &str) -> Result<LoadedSkill, BuiltinSkillError> {
    let (frontmatter, body) = crate::skills::split_frontmatter(source)
        .expect("built-in skills must have valid frontmatter");
    let manifest: SkillManifest = serde_yaml::from_str(frontmatter)
        .expect("built-in skills must have valid YAML frontmatter");
    let name = manifest.name.clone();

    Ok(LoadedSkill {
        name,
        root: PathBuf::from("."),
        manifest,
        body: body.trim_start_matches('\n').to_owned(),
        source: SkillSource::Builtin,
    })
}
