use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, bail};
use neo_agent_core::{WorkspaceAccessRoot, WorkspaceAccessRootKind};
use serde::{Deserialize, Serialize};

use crate::{json_store, path_key::project_key};

const WORKSPACES_FILE: &str = "workspaces.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceStoreData {
    pub schema_version: u32,
    #[serde(default)]
    pub projects: BTreeMap<String, WorkspaceProject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceProject {
    #[serde(default)]
    pub entries: Vec<WorkspaceEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceEntry {
    pub path: PathBuf,
    pub enabled: bool,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceStore {
    path: PathBuf,
}

impl Default for WorkspaceStoreData {
    fn default() -> Self {
        Self {
            schema_version: 1,
            projects: BTreeMap::new(),
        }
    }
}

impl WorkspaceEntry {
    pub(crate) fn read_only(path: PathBuf) -> Self {
        Self {
            path,
            enabled: true,
            read: true,
            write: false,
        }
    }
}

impl WorkspaceStore {
    pub(crate) fn from_home() -> anyhow::Result<Self> {
        let home = crate::config::neo_home().context(
            "NEO_HOME or platform home directory is required to resolve workspace store",
        )?;
        Ok(Self {
            path: home.join(WORKSPACES_FILE),
        })
    }

    #[cfg(test)]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn read_project(&self, project_dir: &Path) -> anyhow::Result<WorkspaceProject> {
        let data = self.read()?;
        Ok(data
            .projects
            .get(&project_key(project_dir)?)
            .cloned()
            .unwrap_or_default())
    }

    pub(crate) fn write_project(
        &self,
        project_dir: &Path,
        project: WorkspaceProject,
    ) -> anyhow::Result<()> {
        let key = project_key(project_dir)?;
        json_store::update(&self.path, "workspace", |data: &mut WorkspaceStoreData| {
            data.projects.insert(key, project);
        })
    }

    fn read(&self) -> anyhow::Result<WorkspaceStoreData> {
        json_store::read_or_default(&self.path, "workspace")
    }
}

pub(crate) fn validate_new_workspace_entry(
    project_dir: &Path,
    project: &WorkspaceProject,
    path: &Path,
) -> anyhow::Result<WorkspaceEntry> {
    if path.as_os_str().is_empty() {
        bail!("Workspace path is required");
    }
    if !path.exists() {
        bail!("Workspace path does not exist: {}", path.display());
    }
    let canonical_project = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project directory {}",
            project_dir.display()
        )
    })?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize workspace path {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Workspace path is not a directory: {}", canonical.display());
    }
    if canonical == canonical_project || canonical.starts_with(&canonical_project) {
        bail!("Directory is already inside the primary workspace");
    }
    if project.entries.iter().any(|entry| entry.path == canonical) {
        bail!("Directory is already configured");
    }
    if project
        .entries
        .iter()
        .any(|entry| entry.path.starts_with(&canonical) || canonical.starts_with(&entry.path))
    {
        bail!("Directory overlaps another added workspace");
    }
    Ok(WorkspaceEntry::read_only(canonical))
}

pub(crate) fn access_roots_from_project(project: &WorkspaceProject) -> Vec<WorkspaceAccessRoot> {
    project
        .entries
        .iter()
        .filter_map(|entry| {
            if !entry.enabled || !entry.read || !entry.path.is_absolute() {
                return None;
            }
            let path = entry.path.canonicalize().ok()?;
            if !path.is_dir() {
                return None;
            }
            Some(WorkspaceAccessRoot {
                path,
                kind: WorkspaceAccessRootKind::Added,
                read: true,
                write: entry.read && entry.write,
            })
        })
        .collect()
}
#[cfg(test)]
mod test_cases {
    use super::*;

    #[allow(clippy::needless_pass_by_value)]
    fn symlink_created(result: std::io::Result<()>) -> bool {
        result.is_ok()
    }

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[path = "store.rs"]
    mod store;
    #[path = "validation.rs"]
    mod validation;
}
