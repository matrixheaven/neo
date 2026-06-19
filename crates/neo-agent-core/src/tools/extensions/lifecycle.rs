use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{DiscoveredExtension, ExtensionDiscovery, ExtensionDiscoveryError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionStatus {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStateSource {
    Default,
    StateFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionLifecycleStatus {
    pub id: String,
    pub name: String,
    pub version: String,
    pub manifest_path: PathBuf,
    pub status: ExtensionStatus,
    pub source: LifecycleStateSource,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionLifecycleError {
    #[error(transparent)]
    Discovery(#[from] ExtensionDiscoveryError),
    #[error("failed to read extension lifecycle state {path}: {source}")]
    ReadState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse extension lifecycle state {path}: {source}")]
    ParseState {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to create extension lifecycle state directory {path}: {source}")]
    CreateStateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write extension lifecycle state {path}: {source}")]
    WriteState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to replace extension lifecycle state {path}: {source}")]
    ReplaceState {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("extension {id:?} not found")]
    NotFound { id: String },
    #[error("extension {id:?} is disabled")]
    Disabled { id: String },
}

#[derive(Debug, Clone)]
pub struct ExtensionLifecycleStore {
    state_path: PathBuf,
}

impl ExtensionLifecycleStore {
    #[must_use]
    pub fn new(state_path: impl AsRef<Path>) -> Self {
        Self {
            state_path: state_path.as_ref().to_path_buf(),
        }
    }

    pub fn statuses(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<Vec<ExtensionLifecycleStatus>, ExtensionLifecycleError> {
        let discovered = discover(root.as_ref())?;
        let state = self.read_state()?;
        Ok(discovered
            .into_iter()
            .map(|extension| lifecycle_status(extension, &state))
            .collect())
    }

    pub fn status(
        &self,
        root: impl AsRef<Path>,
        extension_id: &str,
    ) -> Result<ExtensionLifecycleStatus, ExtensionLifecycleError> {
        self.statuses(root)?
            .into_iter()
            .find(|status| status.id == extension_id)
            .ok_or_else(|| ExtensionLifecycleError::NotFound {
                id: extension_id.to_owned(),
            })
    }

    pub fn enable(
        &self,
        root: impl AsRef<Path>,
        extension_id: &str,
    ) -> Result<ExtensionLifecycleStatus, ExtensionLifecycleError> {
        self.set_enabled(root, extension_id, true)
    }

    pub fn disable(
        &self,
        root: impl AsRef<Path>,
        extension_id: &str,
    ) -> Result<ExtensionLifecycleStatus, ExtensionLifecycleError> {
        self.set_enabled(root, extension_id, false)
    }

    pub fn ensure_enabled(
        &self,
        root: impl AsRef<Path>,
        extension_id: &str,
    ) -> Result<ExtensionLifecycleStatus, ExtensionLifecycleError> {
        let status = self.status(root, extension_id)?;
        if status.status == ExtensionStatus::Disabled {
            return Err(ExtensionLifecycleError::Disabled {
                id: extension_id.to_owned(),
            });
        }
        Ok(status)
    }

    fn set_enabled(
        &self,
        root: impl AsRef<Path>,
        extension_id: &str,
        enabled: bool,
    ) -> Result<ExtensionLifecycleStatus, ExtensionLifecycleError> {
        self.status(root.as_ref(), extension_id)?;
        let mut state = self.read_state()?;
        state
            .extensions
            .insert(extension_id.to_owned(), LifecycleStateEntry { enabled });
        self.write_state(&state)?;
        self.status(root, extension_id)
    }

    fn read_state(&self) -> Result<LifecycleStateFile, ExtensionLifecycleError> {
        if !self.state_path.exists() {
            return Ok(LifecycleStateFile::default());
        }
        let content = fs::read_to_string(&self.state_path).map_err(|source| {
            ExtensionLifecycleError::ReadState {
                path: self.state_path.clone(),
                source,
            }
        })?;
        toml::from_str(&content).map_err(|source| ExtensionLifecycleError::ParseState {
            path: self.state_path.clone(),
            source,
        })
    }

    fn write_state(&self, state: &LifecycleStateFile) -> Result<(), ExtensionLifecycleError> {
        if let Some(parent) = self.state_path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                ExtensionLifecycleError::CreateStateDirectory {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }

        let temp_path = self.temp_path();
        let content = toml::to_string_pretty(state).map_err(|source| {
            ExtensionLifecycleError::WriteState {
                path: temp_path.clone(),
                source: std::io::Error::other(source),
            }
        })?;
        fs::write(&temp_path, content).map_err(|source| ExtensionLifecycleError::WriteState {
            path: temp_path.clone(),
            source,
        })?;
        fs::rename(&temp_path, &self.state_path).map_err(|source| {
            let _ = fs::remove_file(&temp_path);
            ExtensionLifecycleError::ReplaceState {
                path: self.state_path.clone(),
                source,
            }
        })?;
        Ok(())
    }

    fn temp_path(&self) -> PathBuf {
        let mut temp_path = self.state_path.clone();
        let extension = self
            .state_path
            .extension()
            .and_then(|extension| extension.to_str())
            .map_or_else(|| "tmp".to_owned(), |extension| format!("{extension}.tmp"));
        temp_path.set_extension(extension);
        temp_path
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LifecycleStateFile {
    #[serde(default)]
    extensions: BTreeMap<String, LifecycleStateEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct LifecycleStateEntry {
    enabled: bool,
}

fn discover(root: &Path) -> Result<Vec<DiscoveredExtension>, ExtensionLifecycleError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    Ok(ExtensionDiscovery::new(root).discover()?)
}

fn lifecycle_status(
    extension: DiscoveredExtension,
    state: &LifecycleStateFile,
) -> ExtensionLifecycleStatus {
    let state_entry = state.extensions.get(&extension.manifest.id);
    let enabled = state_entry.is_none_or(|entry| entry.enabled);
    ExtensionLifecycleStatus {
        id: extension.manifest.id,
        name: extension.manifest.name,
        version: extension.manifest.version,
        manifest_path: extension.manifest_path,
        status: if enabled {
            ExtensionStatus::Enabled
        } else {
            ExtensionStatus::Disabled
        },
        source: if state_entry.is_some() {
            LifecycleStateSource::StateFile
        } else {
            LifecycleStateSource::Default
        },
    }
}
