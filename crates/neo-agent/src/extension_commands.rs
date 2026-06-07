use std::{fmt::Write as _, path::Path};

use anyhow::{Context, bail};
use neo_extensions::{ExtensionDiscovery, ExtensionRunner};
use neo_sdk::{RpcOutcome, RpcRequest};
use serde_json::Value;

pub fn list(root: &Path) -> anyhow::Result<String> {
    let discovered = discover(root)?;
    if discovered.is_empty() {
        return Ok("no extensions\n".to_owned());
    }

    let mut output = String::new();
    for extension in discovered {
        let _ = writeln!(
            output,
            "{}\t{}\t{}\t{}",
            extension.manifest.id,
            extension.manifest.name,
            extension.manifest.version,
            extension.manifest_path.display()
        );
    }
    Ok(output)
}

pub async fn call(
    root: &Path,
    extension_id: &str,
    method: &str,
    params: &str,
) -> anyhow::Result<String> {
    let params = serde_json::from_str::<Value>(params)
        .with_context(|| format!("failed to parse extension params JSON: {params}"))?;
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
