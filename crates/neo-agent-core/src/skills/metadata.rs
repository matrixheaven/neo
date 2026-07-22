//! Neo host metadata (`agents/neo.yaml`) model, validation, load, and serialization.
//!
//! This optional sidecar carries human-facing display labels and declared MCP
//! dependencies. It is never model-facing prose; `SKILL.md` remains the single
//! owner of invocation policy and model context.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::skills::SkillLoadError;

/// Parsed host metadata from `agents/neo.yaml`.
///
/// An absent or empty sidecar is normal and produces `SkillHostMetadata::default()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillHostMetadata {
    pub interface: Option<SkillInterface>,
    pub dependencies: Vec<SkillToolDependency>,
}

/// Human-facing display labels consumed by TUI completion and `ListSkills`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
}

/// Declared tool dependency (currently only MCP server identifiers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillToolDependency {
    pub value: String,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// YAML wire shapes — kept separate from the public runtime types so the
// deserializer owns validation (e.g. rejecting unsupported dependency types).
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DependencyType {
    Mcp,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct HostMetadataFile {
    interface: Option<InterfaceFile>,
    #[serde(default)]
    dependencies: DepsFile,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct InterfaceFile {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
struct DepsFile {
    #[serde(default)]
    tools: Vec<DepToolFile>,
}

#[derive(Debug, Deserialize)]
struct DepToolFile {
    #[serde(rename = "type")]
    dep_type: DependencyType,
    value: String,
    #[serde(default)]
    description: Option<String>,
}

/// Load and validate `agents/neo.yaml` from `skill_root`.
///
/// Returns metadata plus a list of non-fatal diagnostic strings. Missing file
/// returns `(default(), [])`. Malformed YAML returns `(default(), [diagnostic])`.
pub fn load_host_metadata(skill_root: &Path) -> (SkillHostMetadata, Vec<String>) {
    let sidecar_path = skill_root.join("agents").join("neo.yaml");
    let raw = match std::fs::read_to_string(&sidecar_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (SkillHostMetadata::default(), Vec::new());
        }
        Err(error) => {
            return (
                SkillHostMetadata::default(),
                vec![format!(
                    "failed to read host metadata {}: {error}",
                    sidecar_path.display()
                )],
            );
        }
    };
    if raw.trim().is_empty() {
        return (SkillHostMetadata::default(), Vec::new());
    }
    match parse_and_validate(&raw, &sidecar_path) {
        Ok(metadata) => (metadata, Vec::new()),
        Err(diagnostics) => (SkillHostMetadata::default(), diagnostics),
    }
}

fn parse_and_validate(raw: &str, path: &Path) -> Result<SkillHostMetadata, Vec<String>> {
    let file: HostMetadataFile = serde_yaml::from_str(raw).map_err(|err| {
        vec![format!(
            "invalid host metadata in {}: {err}",
            path.display()
        )]
    })?;

    Ok(SkillHostMetadata {
        interface: file.interface.map(|iface| SkillInterface {
            display_name: validate_display_name(iface.display_name),
            short_description: validate_short_description(iface.short_description),
        }),
        dependencies: file
            .dependencies
            .tools
            .into_iter()
            .map(|dep| SkillToolDependency {
                value: dep.value,
                description: dep.description,
            })
            .collect(),
    })
}

fn validate_display_name(raw: Option<String>) -> Option<String> {
    let value = raw.filter(|s| !s.is_empty())?;
    let trimmed = value.trim();
    if trimmed.chars().count() > 64 {
        return None;
    }
    Some(trimmed.to_owned())
}

fn validate_short_description(raw: Option<String>) -> Option<String> {
    let value = raw.filter(|s| !s.is_empty())?;
    let trimmed = value.trim();
    if trimmed.chars().count() > 256 {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Serialize host metadata to a canonical `agents/neo.yaml` string.
///
/// Returns `None` when there is no meaningful data to persist (empty interface
/// and no dependencies).
pub fn serialize_host_metadata(metadata: &SkillHostMetadata) -> Option<String> {
    let has_interface = metadata
        .interface
        .as_ref()
        .is_some_and(|iface| iface.display_name.is_some() || iface.short_description.is_some());
    if !has_interface && metadata.dependencies.is_empty() {
        return None;
    }

    let mut yaml = "interface:\n".to_owned();
    if let Some(iface) = &metadata.interface {
        if let Some(ref name) = iface.display_name {
            yaml.push_str(&format!("  display_name: \"{}\"\n", escape_yaml_string(name)));
        }
        if let Some(ref desc) = iface.short_description {
            yaml.push_str(&format!(
                "  short_description: \"{}\"\n",
                escape_yaml_string(desc)
            ));
        }
    }

    if !metadata.dependencies.is_empty() {
        yaml.push_str("\ndependencies:\n  tools:\n");
        for dep in &metadata.dependencies {
            yaml.push_str(&format!(
                "    - type: mcp\n      value: \"{}\"\n",
                escape_yaml_string(&dep.value)
            ));
            if let Some(ref desc) = dep.description {
                yaml.push_str(&format!(
                    "      description: \"{}\"\n",
                    escape_yaml_string(desc)
                ));
            }
        }
    }
    Some(yaml)
}

fn escape_yaml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

impl SkillHostMetadata {
    /// Human-readable display name, falling back to the canonical skill name.
    #[must_use]
    pub fn display_name<'a>(&'a self, canonical_name: &'a str) -> &'a str {
        self.interface
            .as_ref()
            .and_then(|iface| iface.display_name.as_deref())
            .unwrap_or(canonical_name)
    }

    /// Short description, falling back to `None` (callers use the manifest description).
    #[must_use]
    pub fn short_description(&self) -> Option<&str> {
        self.interface
            .as_ref()
            .and_then(|iface| iface.short_description.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_roundtrips_through_parse() {
        let original = SkillHostMetadata {
            interface: Some(SkillInterface {
                display_name: Some("Schema Review".into()),
                short_description: Some("Review schemas".into()),
            }),
            dependencies: vec![SkillToolDependency {
                value: "jsonSchemaRegistry".into(),
                description: Some("Registry MCP".into()),
            }],
        };
        let serialized = serialize_host_metadata(&original).expect("non-empty");
        let parsed = match parse_and_validate(&serialized, Path::new("test")) {
            Ok(v) => v,
            Err(d) => panic!("failed to parse own serialization: {d:?}"),
        };
        assert_eq!(original, parsed);
    }

    #[test]
    fn empty_metadata_serializes_to_none() {
        assert_eq!(serialize_host_metadata(&SkillHostMetadata::default()), None);
    }
}
