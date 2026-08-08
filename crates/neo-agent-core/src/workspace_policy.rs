use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::session::atomic_file;

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
    let lexical_root = candidate
        .ancestors()
        .find(|ancestor| ancestor.canonicalize().is_ok_and(|path| path == root))
        .unwrap_or(root);
    let relative = candidate.strip_prefix(lexical_root).map_err(|_| {
        WorkspaceAccessError::PathOutsideWorkspace {
            path: candidate.to_path_buf(),
        }
    })?;
    let mut current = lexical_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if atomic_file::is_reparse_or_symlink(&metadata) => {
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

pub(crate) fn normalize_path(path: &Path) -> PathBuf {
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
#[path = "test_cases/workspace_policy.rs"]
mod tests;
