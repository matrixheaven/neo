use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

const TRUST_FILE: &str = "trust.json";
const CONTEXT_FILE_CANDIDATES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Where a trust decision originates: the current working directory or an
/// ancestor that was explicitly trusted/untrusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustSource {
    CurrentDir,
    Ancestor(PathBuf),
}

impl TrustSource {
    /// The filesystem directory this trust source refers to.
    #[must_use]
    pub fn target(&self, project_dir: &Path) -> PathBuf {
        match self {
            Self::CurrentDir => project_dir.to_path_buf(),
            Self::Ancestor(path) => path.clone(),
        }
    }
}

/// A kind of project-local input that can influence Neo's behavior and therefore
/// requires a trust decision before it is loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustInputKind {
    ContextFile,
    NeoDir,
    AgentsSkillsDir,
}

/// Trust-sensitive inputs discovered in or above the project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectTrustInputs {
    pub current_dir: PathBuf,
    pub detected: Vec<(PathBuf, TrustInputKind)>,
    pub parent_candidates: Vec<PathBuf>,
}

/// The resolved trust decision for a project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectTrustDecision {
    Trusted { source: TrustSource },
    Untrusted { source: TrustSource },
    Unknown { inputs: ProjectTrustInputs },
}

/// The trust state carried through `AppConfig` for startup routing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProjectTrustState {
    Trusted {
        target: PathBuf,
    },
    Untrusted {
        target: PathBuf,
    },
    Unknown {
        inputs: ProjectTrustInputs,
    },
    #[default]
    NotRequired,
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectTrustStore {
    path: PathBuf,
}

impl ProjectTrustStore {
    pub(crate) fn from_home() -> anyhow::Result<Self> {
        let home = crate::config::neo_home()
            .context("NEO_HOME or HOME is required to resolve project trust store")?;
        Ok(Self {
            path: home.join(TRUST_FILE),
        })
    }

    #[cfg(test)]
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Persist a trust decision for `project_dir`.
    ///
    /// `Some(true)` trusts the directory, `Some(false)` explicitly untrusts it,
    /// and `None` removes any stored decision.
    pub(crate) fn set(&self, project_dir: &Path, value: Option<bool>) -> anyhow::Result<()> {
        let key = project_key(project_dir)?;
        let mut data = self.read()?;
        if let Some(value) = value {
            data.insert(key, value);
        } else {
            data.remove(&key);
        }
        self.write(&data)
    }

    pub(crate) fn get(&self, project_dir: &Path) -> anyhow::Result<Option<bool>> {
        let data = self.read()?;
        Ok(data.get(&project_key(project_dir)?).copied())
    }

    fn write(&self, data: &BTreeMap<String, bool>) -> anyhow::Result<()> {
        let parent = self.path.parent().context("trust store has no parent")?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create trust store directory {}",
                parent.display()
            )
        })?;
        let content = serde_json::to_string_pretty(&data).context("serialize trust store")?;
        let mut temp =
            tempfile::NamedTempFile::new_in(parent).context("create temporary trust store file")?;
        temp.write_all(content.as_bytes())
            .context("write temporary trust store file")?;
        temp.persist(&self.path)
            .map_err(|err| anyhow::anyhow!("failed to persist trust store: {err}"))?;
        Ok(())
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
        match serde_json::from_str::<BTreeMap<String, bool>>(&content) {
            Ok(data) => Ok(data),
            Err(err) => {
                let backup = self.path.with_extension("json.bak");
                if backup.exists() {
                    fs::remove_file(&backup).with_context(|| {
                        format!(
                            "failed to remove old trust store backup {}",
                            backup.display()
                        )
                    })?;
                }
                fs::rename(&self.path, &backup).with_context(|| {
                    format!(
                        "failed to back up corrupted trust store to {}",
                        backup.display()
                    )
                })?;
                tracing::warn!(
                    "trust store {} was corrupted ({err}); backed up to {}. Starting fresh.",
                    self.path.display(),
                    backup.display()
                );
                Ok(BTreeMap::new())
            }
        }
    }
}

/// Resolve whether project context should be loaded for `project_dir`.
///
/// `yolo` always returns an explicit untrusted decision so that yolo mode never
/// silently trusts project instructions. The caller (config load) is responsible
/// for mapping yolo to `ProjectTrustState::NotRequired` so that no dialog is shown.
pub(crate) fn resolve_project_trust_decision(
    project_dir: &Path,
    yolo: bool,
    store: &ProjectTrustStore,
) -> anyhow::Result<ProjectTrustDecision> {
    let project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;

    if yolo {
        return Ok(ProjectTrustDecision::Untrusted {
            source: TrustSource::CurrentDir,
        });
    }

    let inputs = collect_project_trust_inputs(&project_dir)?;
    if inputs.detected.is_empty() && inputs.parent_candidates.is_empty() {
        return Ok(ProjectTrustDecision::Trusted {
            source: TrustSource::CurrentDir,
        });
    }

    // Current directory decision takes precedence.
    if let Some(value) = store.get(&project_dir)? {
        return Ok(if value {
            ProjectTrustDecision::Trusted {
                source: TrustSource::CurrentDir,
            }
        } else {
            ProjectTrustDecision::Untrusted {
                source: TrustSource::CurrentDir,
            }
        });
    }

    // Otherwise inherit from the nearest ancestor with a stored decision.
    for ancestor in project_dir.ancestors().skip(1) {
        if !inputs_in_dir(ancestor).is_empty()
            && let Some(value) = store.get(ancestor)?
        {
            let canonical_ancestor = ancestor.canonicalize().with_context(|| {
                format!("failed to canonicalize ancestor {}", ancestor.display())
            })?;
            return Ok(if value {
                ProjectTrustDecision::Trusted {
                    source: TrustSource::Ancestor(canonical_ancestor),
                }
            } else {
                ProjectTrustDecision::Untrusted {
                    source: TrustSource::Ancestor(canonical_ancestor),
                }
            });
        }
    }

    Ok(ProjectTrustDecision::Unknown { inputs })
}

/// Convert trust discovery inputs into the data struct consumed by the TUI trust
/// dialog.
pub(crate) fn trust_dialog_data_from_inputs(
    inputs: ProjectTrustInputs,
) -> neo_tui::dialogs::TrustDialogData {
    neo_tui::dialogs::TrustDialogData {
        current_dir: inputs.current_dir,
        detected: inputs
            .detected
            .into_iter()
            .map(|(path, kind)| neo_tui::dialogs::TrustDialogInput {
                path,
                kind: match kind {
                    TrustInputKind::ContextFile => {
                        neo_tui::dialogs::TrustDialogInputKind::ContextFile
                    }
                    TrustInputKind::NeoDir => neo_tui::dialogs::TrustDialogInputKind::NeoDir,
                    TrustInputKind::AgentsSkillsDir => {
                        neo_tui::dialogs::TrustDialogInputKind::AgentsSkillsDir
                    }
                },
            })
            .collect(),
        parent_candidates: inputs.parent_candidates,
    }
}

/// Discover trust-sensitive inputs in `project_dir` and its ancestors.
///
/// * Detected entries are canonical paths of items found in the current directory.
/// * Parent candidates are canonical paths of ancestor directories that contain inputs.
/// * Duplicates are removed by canonical path so case-insensitive filesystems do
///   not report the same path twice.
pub(crate) fn collect_project_trust_inputs(
    project_dir: &Path,
) -> anyhow::Result<ProjectTrustInputs> {
    let canonical_project_dir = project_dir.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize project dir {}",
            project_dir.display()
        )
    })?;

    let mut detected = Vec::new();
    let mut seen = HashSet::new();
    let mut parent_candidates = Vec::new();
    let mut seen_candidates = HashSet::new();

    for (index, directory) in project_dir.ancestors().enumerate() {
        let dir_inputs = inputs_in_dir(directory);
        if dir_inputs.is_empty() {
            continue;
        }

        if index == 0 {
            for (path, kind) in dir_inputs {
                let canonical = path.canonicalize().with_context(|| {
                    format!(
                        "failed to canonicalize detected trust input {}",
                        path.display()
                    )
                })?;
                if seen.insert(canonical.clone()) {
                    detected.push((canonical, kind));
                }
            }
        } else {
            let canonical = directory.canonicalize().with_context(|| {
                format!(
                    "failed to canonicalize parent candidate {}",
                    directory.display()
                )
            })?;
            if seen_candidates.insert(canonical.clone()) {
                parent_candidates.push(canonical);
            }
        }
    }

    detected.sort_by(|a, b| a.0.cmp(&b.0));
    parent_candidates.sort();

    Ok(ProjectTrustInputs {
        current_dir: canonical_project_dir,
        detected,
        parent_candidates,
    })
}

fn inputs_in_dir(directory: &Path) -> Vec<(PathBuf, TrustInputKind)> {
    let mut result = Vec::new();
    for file_name in CONTEXT_FILE_CANDIDATES {
        let path = directory.join(file_name);
        if path.exists() {
            result.push((path, TrustInputKind::ContextFile));
        }
    }
    let neo_dir = directory.join(".neo");
    if neo_dir.is_dir() {
        result.push((neo_dir, TrustInputKind::NeoDir));
    }
    let agents_skills_dir = directory.join(".agents").join("skills");
    if agents_skills_dir.is_dir() {
        result.push((agents_skills_dir, TrustInputKind::AgentsSkillsDir));
    }
    result
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
    fn corrupted_trust_store_is_backed_up_and_treated_as_empty() {
        let root = TempDir::new().expect("tempdir");
        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        fs::create_dir_all(root.path()).expect("create root");
        fs::write(&store.path, "not json").expect("write corrupted trust");

        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("create project");

        assert_eq!(store.get(&project).expect("read after corruption"), None);
        assert!(store.path.with_extension("json.bak").exists());
    }

    #[test]
    fn collect_project_trust_inputs_detects_project_and_ancestor_inputs() {
        let root = TempDir::new().expect("tempdir");
        let project = root.path().join("repo/crate");
        fs::create_dir_all(&project).expect("create project");

        let empty = collect_project_trust_inputs(&project).expect("collect empty");
        assert!(empty.detected.is_empty());
        assert!(empty.parent_candidates.is_empty());

        fs::write(root.path().join("repo/AGENTS.md"), "rules").expect("write agents");
        let populated = collect_project_trust_inputs(&project).expect("collect populated");
        assert!(!populated.detected.is_empty() || !populated.parent_candidates.is_empty());
    }

    #[test]
    fn trust_decision_is_unknown_when_inputs_exist_without_store_entry() {
        let root = TempDir::new().expect("tempdir");
        let project = root.path().join("project");
        fs::create_dir_all(&project).expect("create project");
        fs::write(project.join("AGENTS.md"), "rules").expect("write agents");

        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        let decision = resolve_project_trust_decision(&project, false, &store).expect("resolve");

        assert!(matches!(decision, ProjectTrustDecision::Unknown { .. }));
    }

    #[test]
    fn trust_decision_inherits_trusted_ancestor() {
        let root = TempDir::new().expect("tempdir");
        let repo = root.path().join("repo");
        let project = repo.join("crate");
        fs::create_dir_all(&project).expect("create project");
        fs::write(repo.join("AGENTS.md"), "rules").expect("write agents");

        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        store.set(&repo, Some(true)).expect("trust repo");

        let canonical_repo = repo.canonicalize().expect("canonicalize repo");
        let decision = resolve_project_trust_decision(&project, false, &store).expect("resolve");
        assert!(matches!(
            decision,
            ProjectTrustDecision::Trusted {
                source: TrustSource::Ancestor(ref ancestor),
            } if ancestor == &canonical_repo
        ));
    }

    #[test]
    fn trust_decision_inherits_untrusted_ancestor() {
        let root = TempDir::new().expect("tempdir");
        let repo = root.path().join("repo");
        let project = repo.join("crate");
        fs::create_dir_all(&project).expect("create project");
        fs::write(repo.join("AGENTS.md"), "rules").expect("write agents");

        let store = ProjectTrustStore::new(root.path().join("trust.json"));
        store.set(&repo, Some(false)).expect("untrust repo");

        let canonical_repo = repo.canonicalize().expect("canonicalize repo");
        let decision = resolve_project_trust_decision(&project, false, &store).expect("resolve");
        assert!(matches!(
            decision,
            ProjectTrustDecision::Untrusted {
                source: TrustSource::Ancestor(ref ancestor),
            } if ancestor == &canonical_repo
        ));
    }

    #[test]
    fn trust_inputs_detect_neo_directory_and_agents_skills() {
        let root = TempDir::new().expect("tempdir");
        let project = root.path().join("project");
        fs::create_dir_all(project.join(".neo")).expect("create .neo");
        fs::create_dir_all(project.join(".agents").join("skills")).expect("create skills");

        let inputs = collect_project_trust_inputs(&project).expect("collect");
        assert_eq!(inputs.detected.len(), 2);
        assert!(
            inputs
                .detected
                .iter()
                .any(|(_, kind)| *kind == TrustInputKind::NeoDir)
        );
        assert!(
            inputs
                .detected
                .iter()
                .any(|(_, kind)| *kind == TrustInputKind::AgentsSkillsDir)
        );
    }

    #[test]
    fn trust_parent_candidates_include_ancestors_with_inputs() {
        let root = TempDir::new().expect("tempdir");
        let grandparent = root.path().join("grandparent");
        let parent = grandparent.join("parent");
        let project = parent.join("project");
        fs::create_dir_all(&project).expect("create project");
        fs::write(grandparent.join("AGENTS.md"), "rules").expect("write grandparent agents");
        fs::create_dir_all(parent.join(".neo")).expect("create parent .neo");

        let canonical_parent = parent.canonicalize().expect("canonicalize parent");
        let canonical_grandparent = grandparent
            .canonicalize()
            .expect("canonicalize grandparent");

        let inputs = collect_project_trust_inputs(&project).expect("collect");
        assert!(inputs.detected.is_empty());
        assert_eq!(inputs.parent_candidates.len(), 2);
        assert!(inputs.parent_candidates.contains(&canonical_parent));
        assert!(inputs.parent_candidates.contains(&canonical_grandparent));
    }
}
