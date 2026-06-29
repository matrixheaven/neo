use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::skills::{LoadedSkill, SkillManifest, SkillSource};

const SUB_SKILL: &str = include_str!("sub-skill.md");
const SELF_EVO: &str = include_str!("self-evo.md");
const MCP_CONFIG: &str = include_str!("mcp-config.md");

const BUILTIN_SOURCES: &[&str] = &[SUB_SKILL, SELF_EVO, MCP_CONFIG];
const REMOVED_BUILTINS: &[&str] = &["define-goal"];

#[derive(Debug, thiserror::Error)]
pub enum BuiltinSkillError {
    #[error("failed to load built-in skill: {0}")]
    Load(#[from] crate::skills::SkillLoadError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Extract built-in skills from the binary into `~/.neo/skills/.builtin/`.
/// Existing files are left untouched so user edits are preserved. To force a
/// re-extract, the user can delete `~/.neo/skills/.builtin/`.
pub fn extract_builtin_skills(
    user_skills_dir: &Path,
) -> Result<Vec<LoadedSkill>, BuiltinSkillError> {
    let builtin_dir = user_skills_dir.join(".builtin");
    fs::create_dir_all(&builtin_dir)?;
    prune_removed_builtins(&builtin_dir)?;

    for source in BUILTIN_SOURCES {
        let skill = load_builtin_skill(source)?;
        let skill_dir = builtin_dir.join(&skill.name);
        fs::create_dir_all(&skill_dir)?;
        let path = skill_dir.join("SKILL.md");
        if !path.exists() {
            fs::write(&path, source)?;
        }
    }

    // Discover extracted skills on disk. This is what the runtime will actually use.
    let skills = crate::skills::discovery::discover_skills(&builtin_dir, SkillSource::Builtin)
        .map_err(BuiltinSkillError::Load)?;
    Ok(skills
        .into_iter()
        .filter(|skill| !REMOVED_BUILTINS.contains(&skill.name.as_str()))
        .collect())
}

pub fn builtin_skills() -> Result<Vec<LoadedSkill>, BuiltinSkillError> {
    BUILTIN_SOURCES
        .iter()
        .map(|source| load_builtin_skill(source))
        .collect()
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

fn prune_removed_builtins(builtin_dir: &Path) -> Result<(), BuiltinSkillError> {
    for name in REMOVED_BUILTINS {
        let path = builtin_dir.join(name);
        if path.exists() {
            fs::remove_dir_all(path)?;
        }
    }
    Ok(())
}
