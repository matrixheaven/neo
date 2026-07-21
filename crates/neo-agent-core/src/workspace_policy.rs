use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceAccessRootKind {
    Primary,
    Added,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAccessRoot {
    pub path: PathBuf,
    pub kind: WorkspaceAccessRootKind,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAccessPolicy {
    roots: Vec<WorkspaceAccessRoot>,
}

#[derive(Debug, Error)]
pub enum WorkspaceAccessError {
    #[error("path is outside workspace: {path}")]
    PathOutsideWorkspace { path: PathBuf },
    #[error("path is not readable: {path}")]
    ReadDenied { path: PathBuf },
    #[error("path is not writable: {path}")]
    WriteDenied { path: PathBuf },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl WorkspaceAccessPolicy {
    pub fn new(primary_root: impl AsRef<Path>) -> Result<Self, WorkspaceAccessError> {
        let primary = primary_root.as_ref().canonicalize()?;
        Ok(Self {
            roots: vec![WorkspaceAccessRoot {
                path: primary,
                kind: WorkspaceAccessRootKind::Primary,
                read: true,
                write: true,
            }],
        })
    }

    pub fn with_roots(
        primary_root: impl AsRef<Path>,
        roots: impl IntoIterator<Item = WorkspaceAccessRoot>,
    ) -> Result<Self, WorkspaceAccessError> {
        let mut policy = Self::new(primary_root)?;
        policy.roots.extend(roots.into_iter().filter_map(|root| {
            let path = root.path.canonicalize().ok()?;
            path.is_dir().then_some(WorkspaceAccessRoot {
                path,
                kind: root.kind,
                read: root.read,
                write: root.read && root.write,
            })
        }));
        Ok(policy)
    }

    #[must_use]
    pub fn roots(&self) -> &[WorkspaceAccessRoot] {
        &self.roots
    }

    #[must_use]
    pub fn primary_root(&self) -> Option<&Path> {
        self.roots
            .iter()
            .find(|root| root.kind == WorkspaceAccessRootKind::Primary)
            .map(|root| root.path.as_path())
    }

    pub fn resolve_read_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let candidate = self.absolute_candidate(path);
        let canonical = match candidate.canonicalize() {
            Ok(canonical) => canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.resolve_missing_read_path(&candidate);
            }
            Err(error) => return Err(WorkspaceAccessError::Io(error)),
        };
        let Some(root) = self.containing_root(&canonical) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace { path: canonical });
        };
        if root.read {
            Ok(canonical)
        } else {
            Err(WorkspaceAccessError::ReadDenied { path: canonical })
        }
    }

    fn resolve_missing_read_path(&self, candidate: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let parent = candidate.parent().map_or_else(
            || self.primary_root().unwrap_or(Path::new(".")).to_path_buf(),
            Path::to_path_buf,
        );
        let canonical_parent = parent.canonicalize()?;
        let file_name =
            candidate
                .file_name()
                .ok_or_else(|| WorkspaceAccessError::PathOutsideWorkspace {
                    path: candidate.to_path_buf(),
                })?;
        let Some(root) = self.containing_root(&canonical_parent) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace {
                path: canonical_parent,
            });
        };
        let resolved = canonical_parent.join(file_name);
        if root.read {
            Ok(resolved)
        } else {
            Err(WorkspaceAccessError::ReadDenied { path: resolved })
        }
    }

    pub fn resolve_write_path(&self, path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
        let candidate = normalize_path(&self.absolute_candidate(path));
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => return self.resolve_existing_write_path(&candidate),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(WorkspaceAccessError::Io(error)),
        }

        let parent = candidate.parent().map_or_else(
            || self.primary_root().unwrap_or(Path::new(".")).to_path_buf(),
            Path::to_path_buf,
        );
        let canonical_parent = canonicalize_nearest_existing_parent(&parent)?;
        let file_name =
            candidate
                .file_name()
                .ok_or_else(|| WorkspaceAccessError::PathOutsideWorkspace {
                    path: candidate.clone(),
                })?;
        let Some(root) = self.containing_root(&canonical_parent) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace {
                path: canonical_parent,
            });
        };
        reject_link_components(&candidate, &root.path)?;
        let resolved = canonical_parent.join(file_name);
        if root.read && root.write {
            Ok(resolved)
        } else {
            Err(WorkspaceAccessError::WriteDenied { path: resolved })
        }
    }

    fn resolve_existing_write_path(
        &self,
        candidate: &Path,
    ) -> Result<PathBuf, WorkspaceAccessError> {
        let canonical = candidate
            .canonicalize()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound => WorkspaceAccessError::PathOutsideWorkspace {
                    path: candidate.to_path_buf(),
                },
                _ => WorkspaceAccessError::Io(error),
            })?;
        let Some(root) = self.containing_root(&canonical) else {
            return Err(WorkspaceAccessError::PathOutsideWorkspace { path: canonical });
        };
        reject_link_components(candidate, &root.path)?;
        if root.read && root.write {
            Ok(canonical)
        } else {
            Err(WorkspaceAccessError::WriteDenied { path: canonical })
        }
    }

    #[must_use]
    pub fn display_path(&self, path: &Path) -> String {
        let normalized = normalize_path(path);
        if let Some(primary) = self.primary_root()
            && let Ok(relative) = normalized.strip_prefix(primary)
        {
            if relative.as_os_str().is_empty() {
                return ".".to_owned();
            }
            return relative.display().to_string();
        }
        normalized.display().to_string()
    }

    fn absolute_candidate(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.primary_root().unwrap_or(Path::new(".")).join(path)
        }
    }

    fn containing_root(&self, canonical_path: &Path) -> Option<&WorkspaceAccessRoot> {
        self.roots
            .iter()
            .filter(|root| canonical_path.starts_with(&root.path))
            .max_by_key(|root| root.path.components().count())
    }
}

fn reject_link_components(candidate: &Path, root: &Path) -> Result<(), WorkspaceAccessError> {
    let relative =
        candidate
            .strip_prefix(root)
            .map_err(|_| WorkspaceAccessError::PathOutsideWorkspace {
                path: candidate.to_path_buf(),
            })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if is_reparse_or_symlink(&metadata) => {
                return Err(WorkspaceAccessError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "refusing symlink or reparse point in write path: {}",
                        current.display()
                    ),
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(WorkspaceAccessError::Io(error)),
        }
    }
    Ok(())
}

fn is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink() || platform_reparse_point(metadata)
}

#[cfg(windows)]
fn platform_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn platform_reparse_point(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn canonicalize_nearest_existing_parent(path: &Path) -> Result<PathBuf, WorkspaceAccessError> {
    let mut current = path.to_path_buf();
    loop {
        match current.canonicalize() {
            Ok(canonical) => {
                if current == *path {
                    return Ok(canonical);
                }
                // Reconstruct the full path by appending the remaining segments.
                let remaining = path.strip_prefix(&current).unwrap_or(Path::new(""));
                return Ok(canonical.join(remaining));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Walk up to parent.
                if let Some(parent) = current.parent() {
                    current = parent.to_path_buf();
                } else {
                    return Err(WorkspaceAccessError::Io(error));
                }
            }
            Err(error) => return Err(WorkspaceAccessError::Io(error)),
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::Normal(_) => normalized.push(component.as_os_str()),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn added_root(path: PathBuf, read: bool, write: bool) -> WorkspaceAccessRoot {
        WorkspaceAccessRoot {
            path,
            kind: WorkspaceAccessRootKind::Added,
            read,
            write,
        }
    }

    #[test]
    fn read_allows_primary_relative_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("src.txt");
        std::fs::write(&file, "hello").expect("write");
        let policy = WorkspaceAccessPolicy::new(dir.path()).expect("policy");

        let resolved = policy
            .resolve_read_path(Path::new("src.txt"))
            .expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn read_allows_absolute_path_inside_added_read_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "mod lib;").expect("write");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                false,
            )],
        )
        .expect("policy");

        let resolved = policy.resolve_read_path(&file).expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
    }

    #[test]
    fn read_allows_missing_file_inside_readable_root_when_parent_exists() {
        let primary = tempfile::tempdir().expect("primary");
        let path = primary.path().join("missing.txt");
        let policy = WorkspaceAccessPolicy::new(primary.path()).expect("policy");

        let resolved = policy.resolve_read_path(&path).expect("resolve");

        assert_eq!(
            resolved,
            primary
                .path()
                .canonicalize()
                .expect("canonical primary")
                .join("missing.txt")
        );
    }

    #[test]
    fn read_allows_added_root_supplied_as_symlink_path() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let links = tempfile::tempdir().expect("links");
        let link = links.path().join("added-link");
        if !symlink_created(create_dir_symlink(added.path(), &link)) {
            return;
        }
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "mod lib;").expect("write");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(link.clone(), true, false)],
        )
        .expect("policy");

        let resolved = policy.resolve_read_path(&file).expect("resolve");

        assert_eq!(resolved, file.canonicalize().expect("canonical file"));
        assert_eq!(
            policy.roots()[1].path,
            added.path().canonicalize().expect("canonical added")
        );
    }

    #[test]
    fn with_roots_ignores_missing_and_non_directory_added_roots() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let non_directory = added.path().join("file.txt");
        std::fs::write(&non_directory, "not a directory").expect("write");
        let missing = added.path().join("missing");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [
                added_root(non_directory, true, false),
                added_root(missing, true, true),
            ],
        )
        .expect("policy");

        assert_eq!(policy.roots().len(), 1);
        assert_eq!(policy.roots()[0].kind, WorkspaceAccessRootKind::Primary);
    }

    #[test]
    fn read_denies_absolute_path_inside_read_disabled_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let file = added.path().join("lib.rs");
        std::fs::write(&file, "mod lib;").expect("write");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                false,
                false,
            )],
        )
        .expect("policy");

        let err = policy.resolve_read_path(&file).expect_err("denied");

        assert!(matches!(err, WorkspaceAccessError::ReadDenied { .. }));
    }

    #[test]
    fn write_allows_new_file_inside_added_write_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("new.txt");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                true,
            )],
        )
        .expect("policy");

        let resolved = policy.resolve_write_path(&path).expect("resolve");

        assert_eq!(
            resolved,
            added
                .path()
                .canonicalize()
                .expect("canonical added")
                .join("new.txt")
        );
    }

    #[test]
    fn write_denies_new_file_inside_read_only_added_root() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("new.txt");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                false,
            )],
        )
        .expect("policy");

        let err = policy.resolve_write_path(&path).expect_err("denied");

        assert!(matches!(err, WorkspaceAccessError::WriteDenied { .. }));
    }

    #[test]
    fn write_denies_existing_symlink_escape() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "secret").expect("write");
        let link = added.path().join("link.txt");
        if !symlink_created(create_symlink(&outside_file, &link)) {
            return;
        }
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                true,
            )],
        )
        .expect("policy");

        let err = policy.resolve_write_path(&link).expect_err("denied");

        assert!(matches!(
            err,
            WorkspaceAccessError::PathOutsideWorkspace { .. }
        ));
    }

    #[test]
    fn write_denies_dangling_symlink_escape_without_creating_target() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("missing.txt");
        let link = added.path().join("link.txt");
        if !symlink_created(create_symlink(&outside_file, &link)) {
            return;
        }
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                true,
                true,
            )],
        )
        .expect("policy");

        let err = policy.resolve_write_path(&link).expect_err("denied");

        assert!(matches!(
            err,
            WorkspaceAccessError::PathOutsideWorkspace { .. }
        ));
        assert!(!outside_file.exists());
    }

    #[test]
    fn write_denies_root_without_read_even_when_write_true() {
        let primary = tempfile::tempdir().expect("primary");
        let added = tempfile::tempdir().expect("added");
        let path = added.path().join("new.txt");
        let policy = WorkspaceAccessPolicy::with_roots(
            primary.path(),
            [added_root(
                added.path().canonicalize().expect("canonical added"),
                false,
                true,
            )],
        )
        .expect("policy");

        let err = policy.resolve_write_path(&path).expect_err("denied");

        assert!(matches!(err, WorkspaceAccessError::WriteDenied { .. }));
        assert!(!policy.roots()[1].write);
    }

    #[test]
    fn read_rejects_symlink_escape() {
        let primary = tempfile::tempdir().expect("primary");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("secret.txt");
        std::fs::write(&outside_file, "secret").expect("write");
        let link = primary.path().join("link.txt");
        if !symlink_created(create_symlink(&outside_file, &link)) {
            return;
        }
        let policy = WorkspaceAccessPolicy::new(primary.path()).expect("policy");

        let err = policy.resolve_read_path(&link).expect_err("escape denied");

        assert!(matches!(
            err,
            WorkspaceAccessError::PathOutsideWorkspace { .. }
        ));
    }

    #[cfg(unix)]
    fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(unix)]
    fn symlink_created(result: std::io::Result<()>) -> bool {
        result.expect("symlink");
        true
    }

    #[cfg(windows)]
    fn symlink_created(result: std::io::Result<()>) -> bool {
        result.is_ok()
    }
}
