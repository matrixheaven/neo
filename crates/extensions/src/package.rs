use std::{
    env,
    fmt::Write as _,
    fs,
    io::Cursor,
    path::{Component, Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tar::{Archive, EntryType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageKind {
    Extension,
    PromptPack,
    Theme,
}

impl std::fmt::Display for PackageKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Extension => formatter.write_str("extension"),
            Self::PromptPack => formatter.write_str("prompt-pack"),
            Self::Theme => formatter.write_str("theme"),
        }
    }
}

impl PackageKind {
    #[must_use]
    pub const fn install_kind(self) -> PackageInstallKind {
        match self {
            Self::Extension => PackageInstallKind::Extension,
            Self::PromptPack => PackageInstallKind::PromptPack,
            Self::Theme => PackageInstallKind::Theme,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageManifest {
    pub kind: PackageKind,
    pub id: String,
    pub version: String,
    pub entry: PathBuf,
    pub archive: PackageArchive,
    pub signature: PackageSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageArchive {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSignature {
    pub algorithm: String,
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPackage {
    pub manifest_path: PathBuf,
    pub archive_path: PathBuf,
    pub manifest: PackageManifest,
}

pub struct DownloadedPackage {
    _temp_dir: tempfile::TempDir,
    pub package: ValidatedPackage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageInstallKind {
    Extension,
    PromptPack,
    Theme,
}

impl From<PackageInstallKind> for PackageKind {
    fn from(value: PackageInstallKind) -> Self {
        match value {
            PackageInstallKind::Extension => Self::Extension,
            PackageInstallKind::PromptPack => Self::PromptPack,
            PackageInstallKind::Theme => Self::Theme,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub id: String,
    pub root: PathBuf,
    pub entry: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageValidationError {
    #[error("failed to read package manifest {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse package manifest {path}: {source}")]
    ParseManifest {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("package id {id:?} is not a safe package directory name")]
    UnsafePackageId { id: String },
    #[error("package entry path {path:?} is unsafe")]
    UnsafeEntryPath { path: PathBuf },
    #[error("archive path {path:?} must be relative to the package manifest")]
    UnsafeArchiveReference { path: PathBuf },
    #[error("failed to read package archive {path}: {source}")]
    ReadArchive {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("archive digest mismatch for {path}: expected {expected}, got {actual}")]
    DigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("unsupported signature algorithm {algorithm:?}")]
    UnsupportedSignatureAlgorithm { algorithm: String },
    #[error("invalid package signature metadata: {message}")]
    InvalidSignatureMetadata { message: String },
    #[error("invalid package signature for {path}")]
    InvalidSignature { path: PathBuf },
    #[error("failed to read package archive entries {path}: {source}")]
    ReadArchiveEntries {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("archive entry path {path:?} is unsafe")]
    UnsafeArchivePath { path: PathBuf },
    #[error("archive symlink {path:?} target {target:?} escapes package root")]
    UnsafeSymlink { path: PathBuf, target: PathBuf },
    #[error("archive hard link {path:?} target {target:?} escapes package root")]
    UnsafeArchiveLink { path: PathBuf, target: PathBuf },
    #[error("package archive {archive} does not contain entry {entry}")]
    MissingEntry { archive: PathBuf, entry: PathBuf },
    #[error("package kind mismatch: expected {expected}, got {actual}")]
    KindMismatch {
        expected: PackageKind,
        actual: PackageKind,
    },
    #[error("failed to create package install directory {path}: {source}")]
    CreateDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to remove package install directory {path}: {source}")]
    RemoveDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to extract package archive {archive} into {destination}: {source}")]
    ExtractArchive {
        archive: PathBuf,
        destination: PathBuf,
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePackageRecord {
    pub kind: PackageKind,
    pub id: String,
    pub version: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePackageVersion {
    pub kind: PackageKind,
    pub id: String,
    pub version: String,
    pub manifest_url: String,
    pub archive_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplaceSearchResponse {
    #[serde(default)]
    pub packages: Vec<MarketplacePackageRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePackageResponse {
    pub package: MarketplacePackageVersion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarketplacePublishResponse {
    pub package: MarketplacePackageRecord,
}

#[derive(Debug, Serialize)]
struct MarketplacePublishRequest<'a> {
    manifest: &'a PackageManifest,
    archive_base64: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error("NEO_MARKETPLACE_URL must be set to use marketplace packages")]
    MissingUrl,
    #[error("invalid marketplace URL {url:?}: {message}")]
    InvalidUrl { url: String, message: String },
    #[error("failed to build marketplace URL {path:?}: {message}")]
    BuildUrl { path: String, message: String },
    #[error("marketplace package URL {url:?} must stay under the configured marketplace origin")]
    ExternalPackageUrl { url: String },
    #[error("marketplace request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("marketplace returned HTTP {status} for {url}")]
    HttpStatus {
        status: reqwest::StatusCode,
        url: String,
    },
    #[error("failed to write downloaded package file {path}: {source}")]
    WritePackageFile {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Package(#[from] PackageValidationError),
}

#[derive(Clone)]
pub struct MarketplaceClient {
    base_url: reqwest::Url,
    http: reqwest::Client,
}

impl MarketplaceClient {
    pub fn from_env() -> Result<Self, MarketplaceError> {
        let url = env::var("NEO_MARKETPLACE_URL").map_err(|_| MarketplaceError::MissingUrl)?;
        Self::new(&url)
    }

    pub fn new(url: &str) -> Result<Self, MarketplaceError> {
        let base_url = reqwest::Url::parse(url).map_err(|source| MarketplaceError::InvalidUrl {
            url: url.to_owned(),
            message: source.to_string(),
        })?;
        Ok(Self {
            base_url,
            http: reqwest::Client::new(),
        })
    }

    pub async fn search(
        &self,
        kind: PackageKind,
        query: &str,
    ) -> Result<Vec<MarketplacePackageRecord>, MarketplaceError> {
        let response = self
            .http
            .get(self.url("/api/v1/marketplace/packages/search")?)
            .query(&[("kind", kind.to_string()), ("q", query.to_owned())])
            .send()
            .await?;
        Ok(self
            .json::<MarketplaceSearchResponse>(response)
            .await?
            .packages)
    }

    pub async fn resolve(
        &self,
        kind: PackageKind,
        id: &str,
        version: Option<&str>,
    ) -> Result<MarketplacePackageVersion, MarketplaceError> {
        let version = version.unwrap_or("latest");
        let response = self
            .http
            .get(self.url(&format!(
                "/api/v1/marketplace/packages/{kind}/{id}/{version}"
            ))?)
            .send()
            .await?;
        Ok(self
            .json::<MarketplacePackageResponse>(response)
            .await?
            .package)
    }

    pub async fn download_package(
        &self,
        package: &MarketplacePackageVersion,
    ) -> Result<DownloadedPackage, MarketplaceError> {
        let temp_dir =
            tempfile::tempdir().map_err(|source| MarketplaceError::WritePackageFile {
                path: PathBuf::from(".neo-package.toml"),
                source,
            })?;
        let manifest_response = self
            .http
            .get(self.url(&package.manifest_url)?)
            .send()
            .await?;
        let manifest_bytes = self.bytes(manifest_response).await?;
        let manifest_path = temp_dir.path().join(".neo-package.toml");
        fs::write(&manifest_path, &manifest_bytes).map_err(|source| {
            MarketplaceError::WritePackageFile {
                path: manifest_path.clone(),
                source,
            }
        })?;

        let manifest_text = std::str::from_utf8(&manifest_bytes).map_err(|source| {
            MarketplaceError::WritePackageFile {
                path: manifest_path.clone(),
                source: std::io::Error::new(std::io::ErrorKind::InvalidData, source),
            }
        })?;
        let manifest: PackageManifest = toml::from_str(manifest_text).map_err(|source| {
            PackageValidationError::ParseManifest {
                path: manifest_path.clone(),
                source,
            }
        })?;
        validate_archive_reference(&manifest.archive.path)?;

        let archive_response = self
            .http
            .get(self.url(&package.archive_url)?)
            .send()
            .await?;
        let archive_bytes = self.bytes(archive_response).await?;
        let archive_path = temp_dir.path().join(&manifest.archive.path);
        if let Some(parent) = archive_path.parent() {
            fs::create_dir_all(parent).map_err(|source| MarketplaceError::WritePackageFile {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&archive_path, archive_bytes).map_err(|source| {
            MarketplaceError::WritePackageFile {
                path: archive_path,
                source,
            }
        })?;

        let package = validate_package(&manifest_path)?;
        Ok(DownloadedPackage {
            _temp_dir: temp_dir,
            package,
        })
    }

    pub async fn publish(
        &self,
        package: &ValidatedPackage,
    ) -> Result<MarketplacePackageRecord, MarketplaceError> {
        let archive = fs::read(&package.archive_path).map_err(|source| {
            PackageValidationError::ReadArchive {
                path: package.archive_path.clone(),
                source,
            }
        })?;
        let request = MarketplacePublishRequest {
            manifest: &package.manifest,
            archive_base64: STANDARD.encode(archive),
        };
        let response = self
            .http
            .post(self.url("/api/v1/marketplace/packages/publish")?)
            .json(&request)
            .send()
            .await?;
        Ok(self
            .json::<MarketplacePublishResponse>(response)
            .await?
            .package)
    }

    fn url(&self, path: &str) -> Result<reqwest::Url, MarketplaceError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|source| MarketplaceError::BuildUrl {
                path: path.to_owned(),
                message: source.to_string(),
            })?;
        if same_origin(&self.base_url, &url) {
            Ok(url)
        } else {
            Err(MarketplaceError::ExternalPackageUrl {
                url: url.to_string(),
            })
        }
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> Result<T, MarketplaceError> {
        Ok(Self::error_for_status(response)?.json::<T>().await?)
    }

    async fn bytes(&self, response: reqwest::Response) -> Result<Vec<u8>, MarketplaceError> {
        Ok(Self::error_for_status(response)?.bytes().await?.to_vec())
    }

    fn error_for_status(
        response: reqwest::Response,
    ) -> Result<reqwest::Response, MarketplaceError> {
        let status = response.status();
        if status.is_success() {
            Ok(response)
        } else {
            Err(MarketplaceError::HttpStatus {
                status,
                url: response.url().to_string(),
            })
        }
    }
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

#[derive(Debug, Clone)]
pub struct PackageInstaller {
    root: PathBuf,
}

impl PackageInstaller {
    #[must_use]
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn install(
        &self,
        package: &ValidatedPackage,
        expected_kind: PackageInstallKind,
    ) -> Result<InstalledPackage, PackageValidationError> {
        let expected_kind = PackageKind::from(expected_kind);
        if package.manifest.kind != expected_kind {
            return Err(PackageValidationError::KindMismatch {
                expected: expected_kind,
                actual: package.manifest.kind,
            });
        }

        let destination = self.root.join(&package.manifest.id);
        if destination.exists() {
            fs::remove_dir_all(&destination).map_err(|source| {
                PackageValidationError::RemoveDirectory {
                    path: destination.clone(),
                    source,
                }
            })?;
        }
        fs::create_dir_all(&destination).map_err(|source| {
            PackageValidationError::CreateDirectory {
                path: destination.clone(),
                source,
            }
        })?;

        let archive_bytes = fs::read(&package.archive_path).map_err(|source| {
            PackageValidationError::ReadArchive {
                path: package.archive_path.clone(),
                source,
            }
        })?;
        Archive::new(Cursor::new(archive_bytes))
            .unpack(&destination)
            .map_err(|source| PackageValidationError::ExtractArchive {
                archive: package.archive_path.clone(),
                destination: destination.clone(),
                source,
            })?;

        Ok(InstalledPackage {
            id: package.manifest.id.clone(),
            root: destination.clone(),
            entry: destination.join(&package.manifest.entry),
        })
    }
}

pub fn validate_package(
    manifest_path: impl AsRef<Path>,
) -> Result<ValidatedPackage, PackageValidationError> {
    let manifest_path = manifest_path.as_ref();
    let manifest_text = fs::read_to_string(manifest_path).map_err(|source| {
        PackageValidationError::ReadManifest {
            path: manifest_path.to_path_buf(),
            source,
        }
    })?;
    let manifest: PackageManifest =
        toml::from_str(&manifest_text).map_err(|source| PackageValidationError::ParseManifest {
            path: manifest_path.to_path_buf(),
            source,
        })?;
    validate_package_id(&manifest.id)?;
    validate_entry_path(&manifest.entry)?;
    validate_archive_reference(&manifest.archive.path)?;

    let package_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let archive_path = package_dir.join(&manifest.archive.path);
    let archive_bytes =
        fs::read(&archive_path).map_err(|source| PackageValidationError::ReadArchive {
            path: archive_path.clone(),
            source,
        })?;
    verify_archive_digest(&archive_path, &manifest.archive.sha256, &archive_bytes)?;
    verify_archive_signature(&archive_path, &manifest.signature, &archive_bytes)?;
    validate_archive_entries(&archive_path, &archive_bytes, &manifest.entry)?;

    Ok(ValidatedPackage {
        manifest_path: manifest_path.to_path_buf(),
        archive_path,
        manifest,
    })
}

fn validate_package_id(id: &str) -> Result<(), PackageValidationError> {
    let is_safe_segment = !id.is_empty()
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if is_safe_segment {
        Ok(())
    } else {
        Err(PackageValidationError::UnsafePackageId { id: id.to_owned() })
    }
}

fn validate_entry_path(path: &Path) -> Result<(), PackageValidationError> {
    normalize_archive_path(path)
        .map(|_| ())
        .map_err(|()| PackageValidationError::UnsafeEntryPath {
            path: path.to_path_buf(),
        })
}

fn validate_archive_reference(path: &Path) -> Result<(), PackageValidationError> {
    normalize_archive_path(path).map(|_| ()).map_err(|()| {
        PackageValidationError::UnsafeArchiveReference {
            path: path.to_path_buf(),
        }
    })
}

fn verify_archive_digest(
    archive_path: &Path,
    expected: &str,
    archive_bytes: &[u8],
) -> Result<(), PackageValidationError> {
    let actual = hex_sha256(archive_bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(PackageValidationError::DigestMismatch {
            path: archive_path.to_path_buf(),
            expected: expected.to_owned(),
            actual,
        })
    }
}

fn verify_archive_signature(
    archive_path: &Path,
    signature: &PackageSignature,
    archive_bytes: &[u8],
) -> Result<(), PackageValidationError> {
    if !signature.algorithm.eq_ignore_ascii_case("ed25519") {
        return Err(PackageValidationError::UnsupportedSignatureAlgorithm {
            algorithm: signature.algorithm.clone(),
        });
    }

    let public_key = STANDARD.decode(&signature.public_key).map_err(|source| {
        PackageValidationError::InvalidSignatureMetadata {
            message: source.to_string(),
        }
    })?;
    let signature_bytes = STANDARD.decode(&signature.signature).map_err(|source| {
        PackageValidationError::InvalidSignatureMetadata {
            message: source.to_string(),
        }
    })?;
    let public_key: [u8; 32] =
        public_key
            .try_into()
            .map_err(|_| PackageValidationError::InvalidSignatureMetadata {
                message: "ed25519 public key must be 32 bytes".to_owned(),
            })?;
    let signature_bytes: [u8; 64] = signature_bytes.try_into().map_err(|_| {
        PackageValidationError::InvalidSignatureMetadata {
            message: "ed25519 signature must be 64 bytes".to_owned(),
        }
    })?;
    let verifying_key = VerifyingKey::from_bytes(&public_key).map_err(|source| {
        PackageValidationError::InvalidSignatureMetadata {
            message: source.to_string(),
        }
    })?;
    let signature = Signature::from_bytes(&signature_bytes);
    verifying_key
        .verify(archive_bytes, &signature)
        .map_err(|_| PackageValidationError::InvalidSignature {
            path: archive_path.to_path_buf(),
        })
}

fn validate_archive_entries(
    archive_path: &Path,
    archive_bytes: &[u8],
    package_entry: &Path,
) -> Result<(), PackageValidationError> {
    let expected_entry = normalize_archive_path(package_entry).map_err(|()| {
        PackageValidationError::UnsafeEntryPath {
            path: package_entry.to_path_buf(),
        }
    })?;
    let mut contains_entry = false;
    let mut archive = Archive::new(Cursor::new(archive_bytes));
    let entries =
        archive
            .entries()
            .map_err(|source| PackageValidationError::ReadArchiveEntries {
                path: archive_path.to_path_buf(),
                source,
            })?;
    for entry in entries {
        let entry = entry.map_err(|source| PackageValidationError::ReadArchiveEntries {
            path: archive_path.to_path_buf(),
            source,
        })?;
        let path = entry
            .path()
            .map_err(|source| PackageValidationError::ReadArchiveEntries {
                path: archive_path.to_path_buf(),
                source,
            })?;
        let raw_path = path.into_owned();
        let normalized = normalize_archive_path(&raw_path).map_err(|()| {
            PackageValidationError::UnsafeArchivePath {
                path: raw_path.clone(),
            }
        })?;
        if normalized == expected_entry {
            contains_entry = true;
        }
        if entry.header().entry_type() == EntryType::Symlink {
            let Some(link_name) =
                entry
                    .link_name()
                    .map_err(|source| PackageValidationError::ReadArchiveEntries {
                        path: archive_path.to_path_buf(),
                        source,
                    })?
            else {
                continue;
            };
            let link_target = link_name.into_owned();
            validate_symlink_target(&raw_path, &link_target)?;
        } else if entry.header().entry_type() == EntryType::Link {
            let Some(link_name) =
                entry
                    .link_name()
                    .map_err(|source| PackageValidationError::ReadArchiveEntries {
                        path: archive_path.to_path_buf(),
                        source,
                    })?
            else {
                continue;
            };
            let link_target = link_name.into_owned();
            validate_hardlink_target(&raw_path, &link_target)?;
        }
    }
    if contains_entry {
        Ok(())
    } else {
        Err(PackageValidationError::MissingEntry {
            archive: archive_path.to_path_buf(),
            entry: package_entry.to_path_buf(),
        })
    }
}

fn validate_symlink_target(path: &Path, target: &Path) -> Result<(), PackageValidationError> {
    let Some(parent) = normalize_archive_path(path).ok().and_then(|components| {
        components
            .get(..components.len().saturating_sub(1))
            .map(<[_]>::to_vec)
    }) else {
        return Err(PackageValidationError::UnsafeSymlink {
            path: path.to_path_buf(),
            target: target.to_path_buf(),
        });
    };
    if target.is_absolute() {
        return Err(PackageValidationError::UnsafeSymlink {
            path: path.to_path_buf(),
            target: target.to_path_buf(),
        });
    }

    let mut resolved = parent;
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => resolved.push(segment.to_owned()),
            Component::ParentDir => {
                if resolved.pop().is_none() {
                    return Err(PackageValidationError::UnsafeSymlink {
                        path: path.to_path_buf(),
                        target: target.to_path_buf(),
                    });
                }
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(PackageValidationError::UnsafeSymlink {
                    path: path.to_path_buf(),
                    target: target.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

fn validate_hardlink_target(path: &Path, target: &Path) -> Result<(), PackageValidationError> {
    if normalize_archive_path(target).is_err() {
        return Err(PackageValidationError::UnsafeArchiveLink {
            path: path.to_path_buf(),
            target: target.to_path_buf(),
        });
    }
    Ok(())
}

fn normalize_archive_path(path: &Path) -> Result<Vec<std::ffi::OsString>, ()> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => components.push(segment.to_owned()),
            Component::ParentDir | Component::Prefix(_) | Component::RootDir => return Err(()),
        }
    }
    if components.is_empty() {
        Err(())
    } else {
        Ok(components)
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}
