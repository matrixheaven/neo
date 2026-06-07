use std::{fmt::Write as _, path::Path};

use neo_sdk::{ResourceContent, SkillLoadOptions, load_skill};

pub fn show(path: &Path) -> anyhow::Result<String> {
    let skill = load_skill(path, SkillLoadOptions::default())?;
    let mut output = String::new();
    let _ = writeln!(output, "name: {}", skill.manifest.name);
    let _ = writeln!(output, "description: {}", skill.manifest.description);
    if let Some(version) = &skill.manifest.version {
        let _ = writeln!(output, "version: {version}");
    }
    let _ = writeln!(output, "entrypoint: {}", skill.manifest.entrypoint);
    let _ = writeln!(output, "resources: {}", skill.resources.len());
    for resource in &skill.resources {
        let kind = match &resource.content {
            ResourceContent::Text(_) => "text",
            ResourceContent::Binary(_) => "binary",
        };
        let _ = writeln!(output, "- {} ({kind})", resource.spec.path);
    }
    output.push('\n');
    output.push_str(&skill.body);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    Ok(output)
}
