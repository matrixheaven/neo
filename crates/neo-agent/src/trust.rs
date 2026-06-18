use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

const CONFIG_DIR: &str = ".neo";
const TRUST_FILE: &str = "trust.json";
const CONTEXT_FILE_CANDIDATES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

#[derive(Debug, Clone)]
pub(crate) struct ProjectTrustStore {
    path: PathBuf,
}

impl ProjectTrustStore {
    pub(crate) fn from_home() -> anyhow::Result<Self> {
        let home = home_dir().context("HOME is required to resolve project trust store")?;
        Ok(Self {
            path: home.join(CONFIG_DIR).join(TRUST_FILE),
        })
    }

    #[cfg(test)]
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    #[cfg(test)]
    fn set(&self, project_dir: &Path, value: Option<bool>) -> anyhow::Result<()> {
        let key = project_key(project_dir)?;
        let mut data = self.read()?;
        if let Some(value) = value {
            data.insert(key, value);
        } else {
            data.remove(&key);
        }
        let parent = self.path.parent().context("trust store has no parent")?;
        fs::create_dir_all(parent)?;
        fs::write(
            &self.path,
            serde_json::to_string_pretty(&data).context("serialize trust store")?,
        )?;
        Ok(())
    }

    pub(crate) fn get(&self, project_dir: &Path) -> anyhow::Result<Option<bool>> {
        let data = self.read()?;
        Ok(data.get(&project_key(project_dir)?).copied())
    }

    fn read(&self) -> anyhow::Result<BTreeMap<String, bool>> {
        if !self.path.exists() {
            return Ok(BTreeMap::new());
        }
        let content = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read trust store {}", self.path.display()))?;
        if content.trim().is_empty() {
            return Ok(BTreeMap::new());
        }
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse trust store {}", self.path.display()))
    }
}

pub(crate) fn resolve_project_trust(project_dir: &Path, yolo: bool) -> anyhow::Result<bool> {
    if yolo {
        return Ok(false);
    }
    if !has_project_trust_inputs(project_dir) {
        return Ok(true);
    }
    Ok(ProjectTrustStore::from_home()?
        .get(project_dir)?
        .unwrap_or(false))
}

pub(crate) fn has_project_trust_inputs(project_dir: &Path) -> bool {
    if project_dir.join(CONFIG_DIR).is_dir() {
        return true;
    }
    project_dir.ancestors().any(|directory| {
        CONTEXT_FILE_CANDIDATES
            .iter()
            .any(|file_name| directory.join(file_name).exists())
            || directory.join(".agents").join("skills").is_dir()
    })
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

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn trust_store_writes_sorted_json_and_reads_canonical_project_paths() {
        let root = TempDir::new().expect("tempdir");
        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        let alpha = root.path().join("alpha");
        let beta = root.path().join("beta");
        fs::create_dir_all(&alpha).expect("create alpha");
        fs::create_dir_all(&beta).expect("create beta");

        store.set(&beta, Some(false)).expect("set beta");
        store.set(&alpha, Some(true)).expect("set alpha");

        let content = fs::read_to_string(root.path().join("trust.json")).expect("read trust");
        let alpha_index = content.find("alpha").expect("alpha in trust");
        let beta_index = content.find("beta").expect("beta in trust");
        assert!(alpha_index < beta_index, "trust keys should be sorted");
        assert_eq!(store.get(&alpha).expect("get alpha"), Some(true));
        assert_eq!(store.get(&beta).expect("get beta"), Some(false));
    }

    #[test]
    fn clearing_trust_removes_project_key() {
        let root = TempDir::new().expect("tempdir");
        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("create project");

        store.set(&project, Some(true)).expect("set trust");
        store.set(&project, None).expect("clear trust");

        assert_eq!(store.get(&project).expect("get trust"), None);
    }

    #[test]
    fn has_project_trust_inputs_detects_project_and_ancestor_inputs() {
        let root = TempDir::new().expect("tempdir");
        let project = root.path().join("repo/crate");
        fs::create_dir_all(&project).expect("create project");

        assert!(!has_project_trust_inputs(&project));

        fs::write(root.path().join("repo/AGENTS.md"), "rules").expect("write agents");
        assert!(has_project_trust_inputs(&project));
    }
}
