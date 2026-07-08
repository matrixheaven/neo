use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context as _, bail};
use neo_agent_core::{WorkspaceAccessRoot, WorkspaceAccessRootKind};
use serde::{Deserialize, Serialize};

const WORKSPACES_FILE: &str = "workspaces.json";
const LOCK_RETRY_ATTEMPTS: usize = 50;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(20);

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
        let parent = self
            .path
            .parent()
            .context("workspace store has no parent")?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create workspace store directory {}",
                parent.display()
            )
        })?;
        let _lock = WorkspaceStoreLock::acquire(&self.path)?;
        let mut data = self.read()?;
        data.projects.insert(project_key(project_dir)?, project);
        self.write(&data)
    }

    fn read(&self) -> anyhow::Result<WorkspaceStoreData> {
        if !self.path.exists() {
            return Ok(WorkspaceStoreData::default());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read workspace store {}", self.path.display()))?;
        if content.trim().is_empty() {
            return Ok(WorkspaceStoreData::default());
        }
        match serde_json::from_str::<WorkspaceStoreData>(&content) {
            Ok(data) => Ok(data),
            Err(err) => {
                let backup = self.path.with_extension("json.bak");
                if backup.exists() {
                    fs::remove_file(&backup).with_context(|| {
                        format!(
                            "failed to remove old workspace store backup {}",
                            backup.display()
                        )
                    })?;
                }
                fs::rename(&self.path, &backup).with_context(|| {
                    format!(
                        "failed to back up corrupted workspace store to {}",
                        backup.display()
                    )
                })?;
                tracing::warn!(
                    "workspace store {} was corrupted ({err}); backed up to {}. Starting fresh.",
                    self.path.display(),
                    backup.display()
                );
                Ok(WorkspaceStoreData::default())
            }
        }
    }

    fn write(&self, data: &WorkspaceStoreData) -> anyhow::Result<()> {
        let parent = self
            .path
            .parent()
            .context("workspace store has no parent")?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create workspace store directory {}",
                parent.display()
            )
        })?;
        let content = serde_json::to_string_pretty(data).context("serialize workspace store")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)
            .context("create temporary workspace store file")?;
        temp.write_all(content.as_bytes())
            .context("write temporary workspace store file")?;
        temp.persist(&self.path)
            .map_err(|err| anyhow::anyhow!("failed to persist workspace store: {err}"))?;
        Ok(())
    }
}

struct WorkspaceStoreLock {
    path: PathBuf,
}

impl WorkspaceStoreLock {
    fn acquire(store_path: &Path) -> anyhow::Result<Self> {
        let lock_path = store_path.with_extension("json.lock");
        for attempt in 0..LOCK_RETRY_ATTEMPTS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { path: lock_path }),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if attempt + 1 == LOCK_RETRY_ATTEMPTS {
                        break;
                    }
                    thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!(
                            "failed to create workspace store lock {}",
                            lock_path.display()
                        )
                    });
                }
            }
        }
        bail!(
            "timed out waiting for workspace store lock {}",
            lock_path.display()
        );
    }
}

impl Drop for WorkspaceStoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
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

fn project_key(project_dir: &Path) -> anyhow::Result<String> {
    let canonical = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;
    let Some(key) = canonical.to_str() else {
        bail!("project dir is not valid UTF-8: {}", canonical.display());
    };
    Ok(key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_writes_project_entries_under_canonical_key() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let added = root.path().join("added");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&added).expect("added");
        let store = WorkspaceStore::new(root.path().join("workspaces.json"));
        let entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));

        store
            .write_project(
                &project,
                WorkspaceProject {
                    entries: vec![entry.clone()],
                },
            )
            .expect("write project");

        let loaded = store.read_project(&project).expect("read project");
        assert_eq!(loaded.entries, vec![entry]);
    }

    #[test]
    fn new_entry_defaults_to_enabled_read_only() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let added = root.path().join("added");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&added).expect("added");

        let entry = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &added)
            .expect("entry");

        assert!(entry.enabled);
        assert!(entry.read);
        assert!(!entry.write);
    }

    #[test]
    fn validation_rejects_directory_inside_primary_workspace() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let nested = project.join("nested");
        fs::create_dir_all(&nested).expect("nested");

        let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &nested)
            .expect_err("reject nested");

        assert!(err.to_string().contains("primary workspace"));
    }

    #[test]
    fn validation_rejects_missing_path_with_clear_error() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("project");
        let missing = root.path().join("missing");

        let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &missing)
            .expect_err("reject missing");

        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validation_rejects_file_path_with_clear_error() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let file = root.path().join("file.txt");
        fs::create_dir_all(&project).expect("project");
        fs::write(&file, "not a directory").expect("file");

        let err = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &file)
            .expect_err("reject file path");

        assert!(err.to_string().contains("not a directory"));
    }

    #[test]
    fn validation_canonicalizes_symlink_directory() {
        let root = tempfile::tempdir().expect("root");
        let project = root.path().join("project");
        let target = root.path().join("target");
        let link = root.path().join("link");
        fs::create_dir_all(&project).expect("project");
        fs::create_dir_all(&target).expect("target");
        if !symlink_created(create_dir_symlink(&target, &link)) {
            return;
        }

        let entry = validate_new_workspace_entry(&project, &WorkspaceProject::default(), &link)
            .expect("symlink dir entry");

        assert_eq!(entry.path, target.canonicalize().expect("canonical target"));
    }

    #[test]
    fn access_roots_skip_disabled_entries() {
        let root = tempfile::tempdir().expect("root");
        let added = root.path().join("added");
        fs::create_dir_all(&added).expect("added");
        let mut entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));
        entry.enabled = false;

        let roots = access_roots_from_project(&WorkspaceProject {
            entries: vec![entry],
        });

        assert!(roots.is_empty());
    }

    #[test]
    fn access_roots_skip_write_only_entries() {
        let root = tempfile::tempdir().expect("root");
        let added = root.path().join("added");
        fs::create_dir_all(&added).expect("added");
        let mut entry = WorkspaceEntry::read_only(added.canonicalize().expect("canonical added"));
        entry.read = false;
        entry.write = true;

        let roots = access_roots_from_project(&WorkspaceProject {
            entries: vec![entry],
        });

        assert!(roots.is_empty());
    }

    #[test]
    fn access_roots_skip_relative_entries() {
        let roots = access_roots_from_project(&WorkspaceProject {
            entries: vec![WorkspaceEntry::read_only(PathBuf::from("relative"))],
        });

        assert!(roots.is_empty());
    }

    #[test]
    fn access_roots_canonicalize_existing_dirs() {
        let root = tempfile::tempdir().expect("root");
        let added = root.path().join("added");
        fs::create_dir_all(&added).expect("added");
        let non_canonical = added.join("..").join("added");

        let roots = access_roots_from_project(&WorkspaceProject {
            entries: vec![WorkspaceEntry::read_only(non_canonical)],
        });

        assert_eq!(roots.len(), 1);
        assert_eq!(
            roots[0].path,
            added.canonicalize().expect("canonical added")
        );
    }

    #[test]
    fn corrupted_store_is_backed_up_and_treated_as_empty() {
        let root = tempfile::tempdir().expect("root");
        let path = root.path().join("workspaces.json");
        fs::write(&path, "not json").expect("write corrupted");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("project");
        let store = WorkspaceStore::new(path.clone());

        let loaded = store.read_project(&project).expect("read after corruption");

        assert!(loaded.entries.is_empty());
        assert!(path.with_extension("json.bak").exists());
    }

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
}
