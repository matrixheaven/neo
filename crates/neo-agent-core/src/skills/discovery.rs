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
#[must_use]
pub fn discover_skills(
    root: &Path,
    source: SkillSource,
) -> (Vec<LoadedSkill>, Vec<SkillDiagnostic>) {
    let mut skills = Vec::new();
    let mut diagnostics = Vec::new();
    if !validate_root(root, &mut diagnostics) {
        return (skills, diagnostics);
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
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

    let mut dir_count: usize = 0;
    let mut entry_count: usize = 0;
    // Iterative stack: (dir_path, prefix, depth)
    let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.to_path_buf(), String::new(), 0)];

    'walk: while let Some((dir, prefix, depth)) = stack.pop() {
        dir_count += 1;
        if dir_count > MAX_DIRECTORIES {
            diagnostics.push(SkillDiagnostic::new(
                &dir,
                format!("discovery directory limit reached ({MAX_DIRECTORIES})"),
            ));
            break;
        }

        let has_own_skill = dir.join("SKILL.md").is_file();
        let own_prefix = load_dir_skill(&dir, &prefix, source, &mut skills, &mut diagnostics);

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
            match classify_entry(entry, &dir, has_own_skill, &mut diagnostics) {
                EntryKind::Subdir(path) => subdirs.push(path),
                EntryKind::Skip => {}
            }
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

/// Validate the discovery root, recording a diagnostic when it is unusable.
///
/// Returns `true` when traversal should proceed. A missing root is a no-op
/// (`true` is not returned, but no diagnostic is recorded).
fn validate_root(root: &Path, diagnostics: &mut Vec<SkillDiagnostic>) -> bool {
    match std::fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => true,
        Ok(_) => {
            diagnostics.push(SkillDiagnostic::new(
                root,
                "skill discovery root is not a directory".to_owned(),
            ));
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            diagnostics.push(SkillDiagnostic::new(
                root,
                format!("cannot inspect discovery root: {error}"),
            ));
            false
        }
    }
}

/// Load `dir/SKILL.md` when present, returning the prefix child directories
/// should inherit.
fn load_dir_skill(
    dir: &Path,
    prefix: &str,
    source: SkillSource,
    skills: &mut Vec<LoadedSkill>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> String {
    let skill_file = dir.join("SKILL.md");
    if !skill_file.is_file() {
        return prefix.to_owned();
    }
    match load_skill_file_with_diagnostics(&skill_file, source) {
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
                .map_or_else(|| prefix.to_owned(), |s| s.name.clone())
        }
        Err(err) => {
            diagnostics.push(SkillDiagnostic::new(
                &skill_file,
                format!("failed to load skill: {err}"),
            ));
            prefix.to_owned()
        }
    }
}

/// Outcome of inspecting a single directory entry during traversal.
enum EntryKind {
    /// A subdirectory that should be enqueued for traversal.
    Subdir(PathBuf),
    /// A non-directory or ignored resource directory.
    Skip,
}

fn classify_entry(
    entry: Result<std::fs::DirEntry, std::io::Error>,
    dir: &Path,
    has_own_skill: bool,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> EntryKind {
    let entry = match entry {
        Ok(entry) => entry,
        Err(err) => {
            diagnostics.push(SkillDiagnostic::new(
                dir,
                format!("failed to read directory entry: {err}"),
            ));
            return EntryKind::Skip;
        }
    };
    let metadata = match std::fs::metadata(entry.path()) {
        Ok(metadata) => metadata,
        Err(err) => {
            diagnostics.push(SkillDiagnostic::new(
                entry.path(),
                format!("cannot stat entry: {err}"),
            ));
            return EntryKind::Skip;
        }
    };
    if !metadata.is_dir() {
        return EntryKind::Skip;
    }
    if has_own_skill
        && entry
            .file_name()
            .to_str()
            .is_some_and(|file_name| RESOURCE_DIRS.contains(&file_name))
    {
        return EntryKind::Skip;
    }
    EntryKind::Subdir(entry.path())
}

fn canonicalize_for_visited(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[must_use]
pub fn user_skill_dirs(user_dir: &Path) -> Vec<PathBuf> {
    vec![user_dir.join("skills")]
}
