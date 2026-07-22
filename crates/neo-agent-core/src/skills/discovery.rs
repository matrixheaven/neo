use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{LoadedSkill, SkillDiagnostic, load_skill_file_with_diagnostics};

const RESOURCE_DIRS: &[&str] = &["agents", "references", "scripts", "assets"];
const MAX_DEPTH: usize = 6;
const MAX_DIRECTORIES: usize = 2_000;
const MAX_ENTRIES: usize = 20_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SkillSource {
    #[default]
    Builtin,
    Extra,
    User,
}

/// Discover skills under `root` with bounded, fail-soft traversal.
///
/// Returns loaded skills and non-fatal diagnostics. A malformed skill or
/// directory read failure never prevents other skills from loading.
pub fn discover_skills(
    root: &Path,
    source: SkillSource,
) -> (Vec<LoadedSkill>, Vec<SkillDiagnostic>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            diagnostics.push(SkillDiagnostic::new(
                root,
                "skill discovery root is not a directory".to_owned(),
            ));
            return (skills, diagnostics);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (skills, diagnostics);
        }
        Err(error) => {
            diagnostics.push(SkillDiagnostic::new(
                root,
                format!("cannot inspect discovery root: {error}"),
            ));
            return (skills, diagnostics);
        }
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut dir_count: usize = 0;
    let mut entry_count: usize = 0;

    // Iterative stack: (dir_path, prefix, depth)
    let mut stack: Vec<(PathBuf, String, usize)> = Vec::new();
    match canonicalize_for_visited(root) {
        Ok(canonical) => {
            visited.insert(canonical);
        }
        Err(err) => {
            diagnostics.push(SkillDiagnostic::new(
                root,
                format!("cannot canonicalize discovery root: {err}"),
            ));
            return (skills, diagnostics);
        }
    }
    stack.push((root.to_path_buf(), String::new(), 0));

    'walk: while let Some((dir, prefix, depth)) = stack.pop() {
        dir_count += 1;
        if dir_count > MAX_DIRECTORIES {
            diagnostics.push(SkillDiagnostic::new(
                &dir,
                format!("discovery directory limit reached ({MAX_DIRECTORIES})"),
            ));
            break;
        }
        let own_skill_file = dir.join("SKILL.md");
        let has_own_skill = own_skill_file.is_file();
        let own_prefix = if has_own_skill {
            match load_skill_file_with_diagnostics(&own_skill_file, source) {
                Ok((skill, load_diagnostics)) => {
                    diagnostics.extend(load_diagnostics);
                    let name = if prefix.is_empty() {
                        skill.name.clone()
                    } else {
                        format!("{prefix}/{}", skill.name)
                    };
                    skills.push(LoadedSkill { name, ..skill });
                    skills
                        .last()
                        .map_or_else(|| prefix.clone(), |s| s.name.clone())
                }
                Err(err) => {
                    diagnostics.push(SkillDiagnostic::new(
                        &own_skill_file,
                        format!("failed to load skill: {err}"),
                    ));
                    prefix.clone()
                }
            }
        } else {
            prefix.clone()
        };

        if depth >= MAX_DEPTH {
            continue;
        }

        let mut entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                diagnostics.push(SkillDiagnostic::new(
                    &dir,
                    format!("failed to read directory: {err}"),
                ));
                continue;
            }
        };

        let mut subdirs: Vec<PathBuf> = Vec::new();
        loop {
            let Some(entry) = entries.next() else {
                break;
            };
            entry_count += 1;
            if entry_count > MAX_ENTRIES {
                diagnostics.push(SkillDiagnostic::new(
                    &dir,
                    format!("discovery entry limit reached ({MAX_ENTRIES})"),
                ));
                break 'walk;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    diagnostics.push(SkillDiagnostic::new(
                        &dir,
                        format!("failed to read directory entry: {err}"),
                    ));
                    continue;
                }
            };
            let metadata = match std::fs::metadata(entry.path()) {
                Ok(metadata) => metadata,
                Err(err) => {
                    diagnostics.push(SkillDiagnostic::new(
                        entry.path(),
                        format!("cannot stat entry: {err}"),
                    ));
                    continue;
                }
            };
            if !metadata.is_dir() {
                continue;
            }
            if has_own_skill
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|file_name| RESOURCE_DIRS.contains(&file_name))
            {
                continue;
            }
            subdirs.push(entry.path());
        }

        // Push in reverse order to preserve sorted traversal (we pop from back).
        subdirs.sort();
        for subdir in subdirs.into_iter().rev() {
            match canonicalize_for_visited(&subdir) {
                Ok(canonical) => {
                    if !visited.insert(canonical) {
                        diagnostics.push(SkillDiagnostic::new(
                            &subdir,
                            "symlink cycle or already-visited directory".to_owned(),
                        ));
                        continue;
                    }
                }
                Err(err) => {
                    diagnostics.push(SkillDiagnostic::new(
                        &subdir,
                        format!("cannot canonicalize directory: {err}"),
                    ));
                    continue;
                }
            }
            stack.push((subdir, own_prefix.clone(), depth + 1));
        }
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (skills, diagnostics)
}

fn canonicalize_for_visited(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[must_use]
pub fn user_skill_dirs(user_dir: &Path) -> Vec<PathBuf> {
    vec![user_dir.join("skills")]
}
