use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::{
    session::atomic_file::{
        AtomicWriteStatus, replace_existing_file_atomic_status, write_file_atomic_create_new,
    },
    skills::{LoadedSkill, SkillManifest, SkillSource},
};

const SUB_SKILL: &str = include_str!("sub-skill.md");
const SELF_EVO: &str = include_str!("self-evo.md");
const MCP_CONFIG: &str = include_str!("mcp-config.md");
const CREATE_SKILL: &str = include_str!("create-skill.md");

const BUILTIN_SOURCES: &[&str] = &[SUB_SKILL, SELF_EVO, MCP_CONFIG, CREATE_SKILL];
const REMOVED_BUILTINS: &[&str] = &["define-goal"];

#[derive(Debug, thiserror::Error)]
pub enum BuiltinSkillError {
    #[error("failed to load built-in skill: {0}")]
    Load(#[from] crate::skills::SkillLoadError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Extract built-in skills from the binary into `~/.neo/skills/.builtin/`.
/// `.builtin` is Neo-managed and refreshed from the current binary on each
/// extraction. Users should customize skills by copying them outside `.builtin`.
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
        refresh_builtin_file(&path, source.as_bytes())?;
    }

    // Discover extracted skills on disk. This is what the runtime will actually use.
    let skills = crate::skills::discovery::discover_skills(&builtin_dir, SkillSource::Builtin)
        .map_err(BuiltinSkillError::Load)?;
    Ok(skills
        .into_iter()
        .filter(|skill| !REMOVED_BUILTINS.contains(&skill.name.as_str()))
        .collect())
}

fn refresh_builtin_file(path: &Path, content: &[u8]) -> io::Result<()> {
    let status = match fs::symlink_metadata(path) {
        Ok(_) => replace_existing_file_atomic_status(path, content),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match write_file_atomic_create_new(path, content) {
                Ok(status) => Ok(status),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    replace_existing_file_atomic_status(path, content)
                }
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    }?;
    match status {
        AtomicWriteStatus::Durable => Ok(()),
        AtomicWriteStatus::CommittedUnsynced(error) => Err(error),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_extraction_never_exposes_partial_skill_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let skills_dir = temp.path().join("skills");
        extract_builtin_skills(&skills_dir).expect("seed built-ins");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));

        let workers: Vec<_> = (0..8)
            .map(|_| {
                let skills_dir = skills_dir.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..16 {
                        extract_builtin_skills(&skills_dir).expect("extract built-ins");
                    }
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("worker");
        }
    }
}
