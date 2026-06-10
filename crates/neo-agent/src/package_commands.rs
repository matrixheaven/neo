use std::{fmt::Write as _, fs, path::Path};

use anyhow::Context as _;
use neo_extensions::{
    MarketplaceClient, MarketplacePackageRecord, PackageInstallKind, PackageInstaller, PackageKind,
    PublisherTrustState, ValidatedPackage, validate_package, validate_package_with_trust,
};
use serde::{Deserialize, Serialize};

use crate::trust;

const PACKAGE_INSTALL_METADATA_FILE: &str = ".neo-package-install.toml";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackageInstallMetadata {
    pub id: String,
    pub version: String,
    pub source: String,
    pub publisher: String,
    pub trust: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageSpec {
    id: String,
    version: Option<String>,
}

pub async fn search(kind: PackageKind, query: &str) -> anyhow::Result<String> {
    let packages = MarketplaceClient::from_env()?.search(kind, query).await?;
    Ok(format_package_records(
        &packages,
        "no marketplace packages\n",
    ))
}

pub async fn install_from_marketplace(
    kind: PackageInstallKind,
    package: &str,
    root: &Path,
    project_dir: &Path,
) -> anyhow::Result<String> {
    let spec = PackageSpec::parse(package)?;
    let client = MarketplaceClient::from_env()?;
    let resolved = client
        .resolve(PackageKind::from(kind), &spec.id, spec.version.as_deref())
        .await?;
    let downloaded = client.download_package(&resolved).await?;
    let trust_store = trust::package_trust_store(project_dir);
    let package = validate_package_with_trust(&downloaded.package.manifest_path, &trust_store)
        .with_context(|| {
            format!(
                "failed to validate trusted marketplace package {}",
                downloaded.package.manifest_path.display()
            )
        })?;
    let installed = PackageInstaller::new(root).install(&package, kind)?;
    write_install_metadata(&installed.root, &package)?;
    Ok(format!(
        "{} installed {}\tmarketplace\t{}\t{}\t{}\n",
        installed.id,
        package.manifest.version,
        package.manifest.publisher.id,
        package.trust_state.label(),
        installed.root.display()
    ))
}

pub async fn update_from_marketplace(
    kind: PackageInstallKind,
    package: &str,
    root: &Path,
    project_dir: &Path,
) -> anyhow::Result<String> {
    let package_dir = root.join(package);
    let metadata = read_install_metadata(&package_dir)?
        .with_context(|| format!("package {package:?} is not installed from marketplace"))?;
    anyhow::ensure!(
        metadata.source == "marketplace",
        "package {package:?} is not installed from marketplace"
    );
    let output = install_from_marketplace(kind, &metadata.id, root, project_dir).await?;
    Ok(output.replacen(" installed ", " updated ", 1))
}

pub fn uninstall_package(package: &str, root: &Path) -> anyhow::Result<String> {
    let destination = root.join(package);
    anyhow::ensure!(
        destination.exists(),
        "package {package:?} is not installed in {}",
        root.display()
    );
    fs::remove_dir_all(&destination)
        .with_context(|| format!("failed to remove package {}", destination.display()))?;
    Ok(format!(
        "{package} uninstalled\t{}\n",
        destination.display()
    ))
}

pub async fn publish(kind: PackageKind, path: &Path) -> anyhow::Result<String> {
    let package = validate_package_manifest_path(path)?;
    anyhow::ensure!(
        package.manifest.kind == kind,
        "{kind} publish requires a {kind} package, got {}",
        package.manifest.kind
    );
    let published = MarketplaceClient::from_env()?.publish(&package).await?;
    Ok(format!(
        "{} published {}\tmarketplace\n",
        published.id, published.version
    ))
}

fn validate_package_manifest_path(path: &Path) -> anyhow::Result<neo_extensions::ValidatedPackage> {
    let manifest_path = if path.is_dir() {
        path.join(".neo-package.toml")
    } else {
        path.to_path_buf()
    };
    validate_package(&manifest_path)
        .with_context(|| format!("failed to validate package {}", manifest_path.display()))
}

fn format_package_records(packages: &[MarketplacePackageRecord], empty: &str) -> String {
    if packages.is_empty() {
        return empty.to_owned();
    }

    let mut output = String::new();
    for package in packages {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}",
            package.id,
            package.version,
            package.name.as_deref().unwrap_or("-"),
            package.description.as_deref().unwrap_or("-"),
            package.publisher.as_deref().unwrap_or("-")
        );
    }
    output
}

pub(crate) fn read_install_metadata(
    package_dir: &Path,
) -> anyhow::Result<Option<PackageInstallMetadata>> {
    let path = package_dir.join(PACKAGE_INSTALL_METADATA_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("failed to read package metadata {}", path.display()))?;
    let metadata = toml::from_str(&content)
        .with_context(|| format!("failed to parse package metadata {}", path.display()))?;
    Ok(Some(metadata))
}

pub(crate) fn metadata_for_installed_file(
    package_root: &Path,
    file_path: &Path,
) -> anyhow::Result<Option<PackageInstallMetadata>> {
    let mut current = file_path.parent();
    while let Some(directory) = current {
        if directory == package_root {
            return Ok(None);
        }
        if let Some(metadata) = read_install_metadata(directory)? {
            return Ok(Some(metadata));
        }
        current = directory.parent();
    }
    Ok(None)
}

fn write_install_metadata(root: &Path, package: &ValidatedPackage) -> anyhow::Result<()> {
    let metadata = PackageInstallMetadata {
        id: package.manifest.id.clone(),
        version: package.manifest.version.clone(),
        source: "marketplace".to_owned(),
        publisher: package.manifest.publisher.id.clone(),
        trust: trust_label(&package.trust_state).to_owned(),
    };
    let content = toml::to_string_pretty(&metadata)?;
    fs::write(root.join(PACKAGE_INSTALL_METADATA_FILE), content)
        .with_context(|| format!("failed to write package metadata under {}", root.display()))
}

fn trust_label(state: &PublisherTrustState) -> &'static str {
    state.label()
}

impl PackageSpec {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let raw = raw.trim();
        anyhow::ensure!(!raw.is_empty(), "package id cannot be empty");
        let (id, version) = match raw.rsplit_once('@') {
            Some((id, version)) if !id.is_empty() && !version.is_empty() => {
                (id.to_owned(), Some(version.to_owned()))
            }
            Some(_) => anyhow::bail!("package spec must be id or id@version"),
            None => (raw.to_owned(), None),
        };
        Ok(Self { id, version })
    }
}
