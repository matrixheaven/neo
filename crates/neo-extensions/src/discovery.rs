use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "runner")]
    pub transport: ExtensionTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ExtensionTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: Vec<ExtensionEnv>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtensionEnv {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredExtension {
    pub manifest_path: PathBuf,
    pub root: PathBuf,
    pub manifest: ExtensionManifest,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionDiscoveryError {
    #[error("failed to read extension directory {path}: {source}")]
    ReadDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to read extension manifest {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse extension manifest {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("duplicate extension id {id:?} in {first} and {second}")]
    DuplicateId {
        id: String,
        first: PathBuf,
        second: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct ExtensionDiscovery {
    root: PathBuf,
}

impl ExtensionDiscovery {
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn discover(&self) -> Result<Vec<DiscoveredExtension>, ExtensionDiscoveryError> {
        let mut manifests = Vec::new();
        collect_manifest_paths(&self.root, &mut manifests)?;
        manifests.sort();

        let mut by_id: BTreeMap<String, DiscoveredExtension> = BTreeMap::new();
        for manifest_path in manifests {
            let manifest_text = fs::read_to_string(&manifest_path).map_err(|source| {
                ExtensionDiscoveryError::ReadManifest {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
            let manifest: ExtensionManifest = toml::from_str(&manifest_text).map_err(|source| {
                ExtensionDiscoveryError::ParseManifest {
                    path: manifest_path.clone(),
                    source,
                }
            })?;
            let discovered = DiscoveredExtension {
                root: manifest_path.parent().unwrap_or(&self.root).to_path_buf(),
                manifest_path: manifest_path.clone(),
                manifest,
            };
            if let Some(first) = by_id.get(&discovered.manifest.id) {
                return Err(ExtensionDiscoveryError::DuplicateId {
                    id: discovered.manifest.id.clone(),
                    first: first.manifest_path.clone(),
                    second: manifest_path,
                });
            }
            by_id.insert(discovered.manifest.id.clone(), discovered);
        }

        Ok(by_id.into_values().collect())
    }
}

fn collect_manifest_paths(
    root: &Path,
    output: &mut Vec<PathBuf>,
) -> Result<(), ExtensionDiscoveryError> {
    let entries = fs::read_dir(root).map_err(|source| ExtensionDiscoveryError::ReadDirectory {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| ExtensionDiscoveryError::ReadDirectory {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_manifest_paths(&path, output)?;
        } else if path
            .file_name()
            .is_some_and(|name| name == "neo-extension.toml")
        {
            output.push(path);
        }
    }
    Ok(())
}
