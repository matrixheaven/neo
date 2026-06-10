use std::{
    collections::BTreeMap,
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
    #[serde(default)]
    pub publisher: PackagePublisher,
    pub archive: PackageArchive,
    pub signature: PackageSignature,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePublisher {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageArchive {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageSignature {
    pub algorithm: String,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub public_key_id: String,
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPackage {
    pub manifest_path: PathBuf,
    pub archive_path: PathBuf,
    pub manifest: PackageManifest,
    pub trust_state: PublisherTrustState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublisherTrustState {
    SelfSigned,
    Trusted {
        publisher_id: String,
        root: String,
        key_id: String,
    },
}

impl PublisherTrustState {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::SelfSigned => "self-signed",
            Self::Trusted { .. } => "trusted",
        }
    }
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
    #[error("package publisher metadata is required for trusted package validation")]
    MissingPublisher,
    #[error("package publisher {publisher_id:?} key {key_id:?} is not trusted")]
    UntrustedPublisher {
        publisher_id: String,
        key_id: String,
    },
    #[error("package publisher {publisher_id:?} key {key_id:?} is revoked")]
    RevokedPublisherKey {
        publisher_id: String,
        key_id: String,
    },
    #[error(
        "package publisher {publisher_id:?} key {key_id:?} does not match the local trust store"
    )]
    PublisherKeyMismatch {
        publisher_id: String,
        key_id: String,
    },
    #[error("failed to read package trust store: {source}")]
    TrustStore { source: PackageTrustError },
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

#[derive(Debug, Clone)]
pub struct PackageTrustStore {
    path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageTrustData {
    #[serde(default)]
    pub publishers: BTreeMap<String, TrustedPublisher>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPublisher {
    pub id: String,
    pub name: String,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub keys: BTreeMap<String, TrustedPublisherKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPublisherKey {
    pub id: String,
    pub public_key: String,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_reason: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum PackageTrustError {
    #[error("failed to read package trust store {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse package trust store {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("failed to write package trust store {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("publisher id cannot be empty")]
    EmptyPublisherId,
    #[error("publisher key id cannot be empty")]
    EmptyKeyId,
    #[error("publisher {publisher_id:?} key {key_id:?} is not trusted")]
    UnknownPublisherKey {
        publisher_id: String,
        key_id: String,
    },
}

impl PackageTrustStore {
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn trust_publisher(
        &mut self,
        publisher_id: &str,
        name: &str,
        root: &str,
        key_id: &str,
        public_key: &str,
        account_id: Option<String>,
    ) -> Result<(), PackageTrustError> {
        ensure_non_empty_publisher(publisher_id)?;
        ensure_non_empty_key(key_id)?;
        let mut data = self.read()?;
        let publisher = data
            .publishers
            .entry(publisher_id.to_owned())
            .or_insert_with(|| TrustedPublisher {
                id: publisher_id.to_owned(),
                name: name.to_owned(),
                root: root.to_owned(),
                account_id: account_id.clone(),
                keys: BTreeMap::new(),
            });
        name.clone_into(&mut publisher.name);
        root.clone_into(&mut publisher.root);
        publisher.account_id = account_id;
        publisher.keys.insert(
            key_id.to_owned(),
            TrustedPublisherKey {
                id: key_id.to_owned(),
                public_key: public_key.to_owned(),
                revoked: false,
                revoked_reason: None,
            },
        );
        self.write(&data)
    }

    pub fn remove_publisher(&mut self, publisher_id: &str) -> Result<bool, PackageTrustError> {
        let mut data = self.read()?;
        let removed = data.publishers.remove(publisher_id).is_some();
        self.write(&data)?;
        Ok(removed)
    }

    pub fn revoke_publisher_key(
        &mut self,
        publisher_id: &str,
        key_id: &str,
        reason: &str,
    ) -> Result<(), PackageTrustError> {
        let mut data = self.read()?;
        let key = data
            .publishers
            .get_mut(publisher_id)
            .and_then(|publisher| publisher.keys.get_mut(key_id))
            .ok_or_else(|| PackageTrustError::UnknownPublisherKey {
                publisher_id: publisher_id.to_owned(),
                key_id: key_id.to_owned(),
            })?;
        key.revoked = true;
        key.revoked_reason = (!reason.trim().is_empty()).then(|| reason.to_owned());
        self.write(&data)
    }

    pub fn list_publishers(&self) -> Result<Vec<TrustedPublisher>, PackageTrustError> {
        Ok(self.read()?.publishers.into_values().collect())
    }

    fn publisher_key(
        &self,
        publisher_id: &str,
        key_id: &str,
    ) -> Result<Option<(TrustedPublisher, TrustedPublisherKey)>, PackageTrustError> {
        let data = self.read()?;
        Ok(data.publishers.get(publisher_id).and_then(|publisher| {
            publisher
                .keys
                .get(key_id)
                .map(|key| (publisher.clone(), key.clone()))
        }))
    }

    fn read(&self) -> Result<PackageTrustData, PackageTrustError> {
        if !self.path.exists() {
            return Ok(PackageTrustData::default());
        }
        let content = fs::read_to_string(&self.path).map_err(|source| PackageTrustError::Read {
            path: self.path.clone(),
            source,
        })?;
        if content.trim().is_empty() {
            return Ok(PackageTrustData::default());
        }
        toml::from_str(&content).map_err(|source| PackageTrustError::Parse {
            path: self.path.clone(),
            source,
        })
    }

    fn write(&self, data: &PackageTrustData) -> Result<(), PackageTrustError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| PackageTrustError::Write {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let content = toml::to_string_pretty(data).map_err(|source| PackageTrustError::Write {
            path: self.path.clone(),
            source: std::io::Error::other(source),
        })?;
        fs::write(&self.path, content).map_err(|source| PackageTrustError::Write {
            path: self.path.clone(),
            source,
        })
    }
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
        if same_origin(&self.base_url, &url) || allow_cross_origin_marketplace_packages() {
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

fn allow_cross_origin_marketplace_packages() -> bool {
    env::var("NEO_MARKETPLACE_ALLOW_CROSS_ORIGIN")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
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
    validate_package_inner(manifest_path.as_ref(), None)
}

pub fn validate_package_with_trust(
    manifest_path: impl AsRef<Path>,
    trust_store: &PackageTrustStore,
) -> Result<ValidatedPackage, PackageValidationError> {
    validate_package_inner(manifest_path.as_ref(), Some(trust_store))
}

fn validate_package_inner(
    manifest_path: &Path,
    trust_store: Option<&PackageTrustStore>,
) -> Result<ValidatedPackage, PackageValidationError> {
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
    let trust_state = match trust_store {
        Some(store) => verify_publisher_trust(&manifest, store)?,
        None => PublisherTrustState::SelfSigned,
    };
    validate_archive_entries(&archive_path, &archive_bytes, &manifest.entry)?;

    Ok(ValidatedPackage {
        manifest_path: manifest_path.to_path_buf(),
        archive_path,
        manifest,
        trust_state,
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

fn verify_publisher_trust(
    manifest: &PackageManifest,
    trust_store: &PackageTrustStore,
) -> Result<PublisherTrustState, PackageValidationError> {
    if manifest.publisher.id.trim().is_empty()
        || manifest.signature.root.trim().is_empty()
        || manifest.signature.public_key_id.trim().is_empty()
    {
        return Err(PackageValidationError::MissingPublisher);
    }
    let publisher_id = manifest.publisher.id.clone();
    let key_id = manifest.signature.public_key_id.clone();
    let Some((publisher, key)) = trust_store
        .publisher_key(&publisher_id, &key_id)
        .map_err(|source| PackageValidationError::TrustStore { source })?
    else {
        return Err(PackageValidationError::UntrustedPublisher {
            publisher_id,
            key_id,
        });
    };
    if key.revoked {
        return Err(PackageValidationError::RevokedPublisherKey {
            publisher_id,
            key_id,
        });
    }
    if publisher.root != manifest.signature.root || key.public_key != manifest.signature.public_key
    {
        return Err(PackageValidationError::PublisherKeyMismatch {
            publisher_id,
            key_id,
        });
    }
    Ok(PublisherTrustState::Trusted {
        publisher_id,
        root: publisher.root,
        key_id,
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

fn ensure_non_empty_publisher(publisher_id: &str) -> Result<(), PackageTrustError> {
    if publisher_id.trim().is_empty() {
        Err(PackageTrustError::EmptyPublisherId)
    } else {
        Ok(())
    }
}

fn ensure_non_empty_key(key_id: &str) -> Result<(), PackageTrustError> {
    if key_id.trim().is_empty() {
        Err(PackageTrustError::EmptyKeyId)
    } else {
        Ok(())
    }
}
