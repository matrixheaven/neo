use std::{fmt::Write as _, path::Path};

use anyhow::{Context, bail};
use neo_extensions::{
    ExtensionDiscovery, ExtensionInstaller, ExtensionLifecycleStatus, ExtensionLifecycleStore,
    ExtensionRunner, ExtensionStatus, LifecycleStateSource,
};
use neo_sdk::{RpcOutcome, RpcRequest};
use serde_json::Value;

pub fn list(root: &Path, state_path: &Path, registry_path: &Path) -> anyhow::Result<String> {
    let discovered = discover(root)?;
    if discovered.is_empty() {
        return Ok("no extensions\n".to_owned());
    }

    let mut output = String::new();
    let store = lifecycle_store(state_path);
    let installer = installer(root, state_path, registry_path);
    for extension in discovered {
        let lifecycle = store.status(root, &extension.manifest.id)?;
        let source = installer
            .source_for(&extension.manifest.id)?
            .unwrap_or_else(|| "-".to_owned());
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            extension.manifest.id,
            extension.manifest.name,
            extension.manifest.version,
            format_extension_status(lifecycle.status),
            format_state_source(lifecycle.source),
            source,
            extension.manifest_path.display()
        );
    }
    Ok(output)
}

pub fn install(
    root: &Path,
    state_path: &Path,
    registry_path: &Path,
    source: &str,
) -> anyhow::Result<String> {
    let installed = installer(root, state_path, registry_path).install(source)?;
    Ok(format!(
        "{} installed {}\t{}\n",
        installed.manifest.id, installed.manifest.version, installed.source
    ))
}

pub fn update(
    root: &Path,
    state_path: &Path,
    registry_path: &Path,
    extension_id: &str,
) -> anyhow::Result<String> {
    let updated = installer(root, state_path, registry_path).update(extension_id)?;
    Ok(format!(
        "{} updated {}\t{}\n",
        updated.manifest.id, updated.manifest.version, updated.source
    ))
}

pub fn uninstall(
    root: &Path,
    state_path: &Path,
    registry_path: &Path,
    extension_id: &str,
) -> anyhow::Result<String> {
    let uninstalled = installer(root, state_path, registry_path).uninstall(extension_id)?;
    Ok(format!(
        "{} uninstalled\t{}\n",
        uninstalled.id,
        uninstalled.root.display()
    ))
}

pub fn status(root: &Path, state_path: &Path, extension_id: &str) -> anyhow::Result<String> {
    let status = lifecycle_store(state_path).status(root, extension_id)?;
    Ok(format_status(&status))
}

pub fn enable(root: &Path, state_path: &Path, extension_id: &str) -> anyhow::Result<String> {
    let status = lifecycle_store(state_path).enable(root, extension_id)?;
    Ok(format!(
        "{} {}\n",
        status.id,
        format_extension_status(status.status)
    ))
}

pub fn disable(root: &Path, state_path: &Path, extension_id: &str) -> anyhow::Result<String> {
    let status = lifecycle_store(state_path).disable(root, extension_id)?;
    Ok(format!(
        "{} {}\n",
        status.id,
        format_extension_status(status.status)
    ))
}

pub async fn call(
    root: &Path,
    state_path: &Path,
    extension_id: &str,
    method: &str,
    params: &str,
) -> anyhow::Result<String> {
    let params = serde_json::from_str::<Value>(params)
        .with_context(|| format!("failed to parse extension params JSON: {params}"))?;
    lifecycle_store(state_path).ensure_enabled(root, extension_id)?;
    let extension = discover(root)?
        .into_iter()
        .find(|extension| extension.manifest.id == extension_id)
        .with_context(|| format!("extension {extension_id:?} not found in {}", root.display()))?;
    let mut runner = ExtensionRunner::spawn(extension.manifest.transport)?;
    let response = runner
        .request(RpcRequest::new("neo-cli-1", method, params))
        .await?;
    let RpcOutcome::Success { result } = response.outcome else {
        bail!("extension returned an unexpected failure response");
    };
    Ok(format!("{}\n", serde_json::to_string(&result)?))
}

fn discover(root: &Path) -> anyhow::Result<Vec<neo_extensions::DiscoveredExtension>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    ExtensionDiscovery::new(root)
        .discover()
        .with_context(|| format!("failed to discover extensions under {}", root.display()))
}

fn lifecycle_store(state_path: &Path) -> ExtensionLifecycleStore {
    ExtensionLifecycleStore::new(state_path)
}

fn installer(root: &Path, state_path: &Path, registry_path: &Path) -> ExtensionInstaller {
    ExtensionInstaller::new(root, state_path, registry_path)
}

fn format_status(status: &ExtensionLifecycleStatus) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\n",
        status.id,
        status.name,
        status.version,
        format_extension_status(status.status),
        format_state_source(status.source),
        status.manifest_path.display()
    )
}

fn format_extension_status(status: ExtensionStatus) -> &'static str {
    match status {
        ExtensionStatus::Enabled => "enabled",
        ExtensionStatus::Disabled => "disabled",
    }
}

fn format_state_source(source: LifecycleStateSource) -> &'static str {
    match source {
        LifecycleStateSource::Default => "default",
        LifecycleStateSource::StateFile => "state_file",
    }
}
