use std::{fmt::Write as _, path::Path};

use anyhow::Context as _;
use neo_extensions::{
    MarketplaceClient, MarketplacePackageRecord, PackageInstallKind, PackageInstaller, PackageKind,
    validate_package,
};

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
) -> anyhow::Result<String> {
    let spec = PackageSpec::parse(package)?;
    let client = MarketplaceClient::from_env()?;
    let resolved = client
        .resolve(PackageKind::from(kind), &spec.id, spec.version.as_deref())
        .await?;
    let downloaded = client.download_package(&resolved).await?;
    let installed = PackageInstaller::new(root).install(&downloaded.package, kind)?;
    Ok(format!(
        "{} installed {}\tmarketplace\t{}\n",
        installed.id,
        downloaded.package.manifest.version,
        installed.root.display()
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
