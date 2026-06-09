use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::{
    DiscoveredExtension, ExtensionDiscovery, ExtensionDiscoveryError, ExtensionLifecycleStore,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledExtension {
    pub manifest_path: PathBuf,
    pub root: PathBuf,
    pub source: String,
    pub manifest: crate::ExtensionManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstalledExtension {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionInstallError {
    #[error(transparent)]
    Discovery(#[from] ExtensionDiscoveryError),
    #[error("extension source {path} does not exist")]
    SourceNotFound { path: PathBuf },
    #[error("extension source {path} does not contain neo-extension.toml")]
    MissingManifest { path: PathBuf },
    #[error("extension source {path} did not contain exactly one extension manifest")]
    AmbiguousSource { path: PathBuf },
    #[error("extension {id:?} is not installed")]
    NotInstalled { id: String },
    #[error("extension {id:?} resolved outside extension root {root}")]
    OutsideExtensionRoot { id: String, root: PathBuf },
    #[error("failed to read extension source registry {path}: {source}")]
    ReadRegistry {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse extension source registry {path}: {source}")]
    ParseRegistry {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to create extension directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to remove extension directory {path}: {source}")]
    RemoveDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to copy extension file {from} to {to}: {source}")]
    CopyFile {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write extension source registry {path}: {source}")]
    WriteRegistry {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to clone git extension source {url}: {stderr}")]
    GitClone { url: String, stderr: String },
}

#[derive(Debug, Clone)]
pub struct ExtensionInstaller {
    root: PathBuf,
    state_path: PathBuf,
    registry_path: PathBuf,
}

impl ExtensionInstaller {
    #[must_use]
    pub fn new(
        root: impl AsRef<Path>,
        state_path: impl AsRef<Path>,
        registry_path: impl AsRef<Path>,
    ) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            state_path: state_path.as_ref().to_path_buf(),
            registry_path: registry_path.as_ref().to_path_buf(),
        }
    }

    #[must_use]
    pub fn lifecycle(&self) -> ExtensionLifecycleStore {
        ExtensionLifecycleStore::new(&self.state_path)
    }

    pub fn install(
        &self,
        source: impl AsRef<Path>,
    ) -> Result<InstalledExtension, ExtensionInstallError> {
        let source = ExtensionSource::LocalPath {
            path: source.as_ref().to_path_buf(),
        };
        self.install_source(&source)
    }

    pub fn install_git(&self, source: &str) -> Result<InstalledExtension, ExtensionInstallError> {
        let source = ExtensionSource::GitUrl {
            url: source.to_owned(),
        };
        self.install_source(&source)
    }

    pub fn update(&self, extension_id: &str) -> Result<InstalledExtension, ExtensionInstallError> {
        let registry = self.read_registry()?;
        let Some(entry) = registry.extensions.get(extension_id) else {
            return Err(ExtensionInstallError::NotInstalled {
                id: extension_id.to_owned(),
            });
        };
        self.install_source(&entry.source)
    }

    pub fn uninstall(
        &self,
        extension_id: &str,
    ) -> Result<UninstalledExtension, ExtensionInstallError> {
        let destination = self.root.join(extension_id);
        if !destination.exists() {
            return Err(ExtensionInstallError::NotInstalled {
                id: extension_id.to_owned(),
            });
        }
        ensure_inside_root(&self.root, &destination, extension_id)?;

        fs::remove_dir_all(&destination).map_err(|source| {
            ExtensionInstallError::RemoveDirectory {
                path: destination.clone(),
                source,
            }
        })?;

        let mut registry = self.read_registry()?;
        registry.extensions.remove(extension_id);
        self.write_registry(&registry)?;

        Ok(UninstalledExtension {
            id: extension_id.to_owned(),
            root: destination,
        })
    }

    pub fn source_for(&self, extension_id: &str) -> Result<Option<String>, ExtensionInstallError> {
        Ok(self
            .read_registry()?
            .extensions
            .get(extension_id)
            .map(|entry| entry.source.display()))
    }

    fn install_source(
        &self,
        source: &ExtensionSource,
    ) -> Result<InstalledExtension, ExtensionInstallError> {
        match source {
            ExtensionSource::LocalPath { path } => self.install_from_directory(path, source),
            ExtensionSource::GitUrl { url } => {
                let temp = tempfile::tempdir().map_err(|source| {
                    ExtensionInstallError::CreateDirectory {
                        path: self.root.clone(),
                        source,
                    }
                })?;
                clone_git(url, temp.path())?;
                self.install_from_directory(temp.path(), source)
            }
        }
    }

    fn install_from_directory(
        &self,
        source_dir: &Path,
        stored_source: &ExtensionSource,
    ) -> Result<InstalledExtension, ExtensionInstallError> {
        if !source_dir.exists() {
            return Err(ExtensionInstallError::SourceNotFound {
                path: source_dir.to_path_buf(),
            });
        }
        if !source_dir.join("neo-extension.toml").exists() {
            return Err(ExtensionInstallError::MissingManifest {
                path: source_dir.to_path_buf(),
            });
        }

        let source_extension = single_discovered(source_dir)?;
        validate_package_id(&source_extension.manifest.id, &self.root)?;
        let destination = self.root.join(&source_extension.manifest.id);
        replace_directory(source_dir, &destination)?;

        let installed_extension = single_discovered(&destination)?;
        let mut registry = self.read_registry()?;
        registry.extensions.insert(
            installed_extension.manifest.id.clone(),
            ExtensionSourceEntry {
                source: stored_source.clone(),
            },
        );
        self.write_registry(&registry)?;

        Ok(InstalledExtension {
            manifest_path: installed_extension.manifest_path,
            root: installed_extension.root,
            source: stored_source.display(),
            manifest: installed_extension.manifest,
        })
    }

    fn read_registry(&self) -> Result<ExtensionSourceRegistry, ExtensionInstallError> {
        if !self.registry_path.exists() {
            return Ok(ExtensionSourceRegistry::default());
        }
        let content = fs::read_to_string(&self.registry_path).map_err(|source| {
            ExtensionInstallError::ReadRegistry {
                path: self.registry_path.clone(),
                source,
            }
        })?;
        toml::from_str(&content).map_err(|source| ExtensionInstallError::ParseRegistry {
            path: self.registry_path.clone(),
            source,
        })
    }

    fn write_registry(
        &self,
        registry: &ExtensionSourceRegistry,
    ) -> Result<(), ExtensionInstallError> {
        if let Some(parent) = self.registry_path.parent() {
            fs::create_dir_all(parent).map_err(|source| {
                ExtensionInstallError::CreateDirectory {
                    path: parent.to_path_buf(),
                    source,
                }
            })?;
        }
        let content = toml::to_string_pretty(registry).map_err(|source| {
            ExtensionInstallError::WriteRegistry {
                path: self.registry_path.clone(),
                source: std::io::Error::other(source),
            }
        })?;
        fs::write(&self.registry_path, content).map_err(|source| {
            ExtensionInstallError::WriteRegistry {
                path: self.registry_path.clone(),
                source,
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ExtensionSource {
    LocalPath { path: PathBuf },
    GitUrl { url: String },
}

impl ExtensionSource {
    fn display(&self) -> String {
        match self {
            Self::LocalPath { path } => path.display().to_string(),
            Self::GitUrl { url } => url.clone(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ExtensionSourceRegistry {
    #[serde(default)]
    extensions: BTreeMap<String, ExtensionSourceEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ExtensionSourceEntry {
    source: ExtensionSource,
}

fn single_discovered(source: &Path) -> Result<DiscoveredExtension, ExtensionInstallError> {
    let discovered = ExtensionDiscovery::new(source).discover()?;
    if discovered.len() != 1 {
        return Err(ExtensionInstallError::AmbiguousSource {
            path: source.to_path_buf(),
        });
    }
    Ok(discovered.into_iter().next().expect("length checked"))
}

fn validate_package_id(id: &str, root: &Path) -> Result<(), ExtensionInstallError> {
    let is_safe_segment = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));

    if is_safe_segment {
        Ok(())
    } else {
        Err(ExtensionInstallError::OutsideExtensionRoot {
            id: id.to_owned(),
            root: root.to_path_buf(),
        })
    }
}

fn replace_directory(from: &Path, to: &Path) -> Result<(), ExtensionInstallError> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(|source| ExtensionInstallError::CreateDirectory {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    if to.exists() {
        fs::remove_dir_all(to).map_err(|source| ExtensionInstallError::RemoveDirectory {
            path: to.to_path_buf(),
            source,
        })?;
    }
    copy_directory(from, to)
}

fn ensure_inside_root(
    root: &Path,
    destination: &Path,
    extension_id: &str,
) -> Result<(), ExtensionInstallError> {
    let root = root
        .canonicalize()
        .map_err(|source| ExtensionInstallError::ReadRegistry {
            path: root.to_path_buf(),
            source,
        })?;
    let destination =
        destination
            .canonicalize()
            .map_err(|source| ExtensionInstallError::RemoveDirectory {
                path: destination.to_path_buf(),
                source,
            })?;
    if destination.starts_with(&root) {
        Ok(())
    } else {
        Err(ExtensionInstallError::OutsideExtensionRoot {
            id: extension_id.to_owned(),
            root,
        })
    }
}

fn copy_directory(from: &Path, to: &Path) -> Result<(), ExtensionInstallError> {
    fs::create_dir_all(to).map_err(|source| ExtensionInstallError::CreateDirectory {
        path: to.to_path_buf(),
        source,
    })?;
    for entry in fs::read_dir(from).map_err(|source| ExtensionInstallError::CreateDirectory {
        path: from.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| ExtensionInstallError::CreateDirectory {
            path: from.to_path_buf(),
            source,
        })?;
        let source_path = entry.path();
        let destination_path = to.join(entry.file_name());
        if source_path.is_dir() {
            copy_directory(&source_path, &destination_path)?;
        } else {
            fs::copy(&source_path, &destination_path).map_err(|source| {
                ExtensionInstallError::CopyFile {
                    from: source_path,
                    to: destination_path,
                    source,
                }
            })?;
        }
    }
    Ok(())
}

fn clone_git(source: &str, destination: &Path) -> Result<(), ExtensionInstallError> {
    let output = Command::new("git")
        .args(["clone", "--depth", "1", source])
        .arg(destination)
        .output()
        .map_err(|source_error| ExtensionInstallError::GitClone {
            url: source.to_owned(),
            stderr: source_error.to_string(),
        })?;
    if !output.status.success() {
        return Err(ExtensionInstallError::GitClone {
            url: source.to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}
