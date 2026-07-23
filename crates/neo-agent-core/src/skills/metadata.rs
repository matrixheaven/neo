//! Neo host metadata (`agents/neo.yaml`) model, validation, load, and serialization.
//!
//! This optional sidecar carries human-facing display labels and declared MCP
//! dependencies. It is never model-facing prose; `SKILL.md` remains the single
//! owner of invocation policy and model context.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const MAX_DISPLAY_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 256;
const MAX_DEPENDENCY_VALUE_CHARS: usize = 128;

/// Parsed host metadata from `agents/neo.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillHostMetadata {
    pub interface: Option<SkillInterface>,
    #[serde(default)]
    pub dependencies: Vec<SkillToolDependency>,
}

/// Human-facing display labels consumed by TUI completion and `ListSkills`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillInterface {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
}

/// Declared MCP server dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkillToolDependency {
    pub value: String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct HostMetadataFile {
    #[serde(default)]
    interface: Option<InterfaceFile>,
    #[serde(default)]
    dependencies: DepsFile,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct InterfaceFile {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    short_description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct DepsFile {
    #[serde(default)]
    tools: Vec<DepToolFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DepToolFile {
    #[serde(rename = "type")]
    dep_type: String,
    value: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Serialize)]
struct HostMetadataOutput<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    interface: Option<&'a SkillInterface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependencies: Option<DepsOutput<'a>>,
}

#[derive(Serialize)]
struct DepsOutput<'a> {
    tools: Vec<DepToolOutput<'a>>,
}

#[derive(Serialize)]
struct DepToolOutput<'a> {
    #[serde(rename = "type")]
    dep_type: &'static str,
    value: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'a str>,
}

/// Load and validate `agents/neo.yaml` from `skill_root`.
///
/// Missing or empty files return empty metadata without diagnostics. Structural
/// YAML errors fall back to empty metadata. Invalid fields and unsupported
/// dependency types are diagnosed and omitted without discarding valid fields.
#[must_use]
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
    let file: HostMetadataFile = match serde_yaml::from_str(&raw) {
        Ok(file) => file,
        Err(error) => {
            return (
                SkillHostMetadata::default(),
                vec![format!(
                    "invalid host metadata in {}: {error}",
                    sidecar_path.display()
                )],
            );
        }
    };
    metadata_from_file(file, &sidecar_path)
}

fn metadata_from_file(file: HostMetadataFile, path: &Path) -> (SkillHostMetadata, Vec<String>) {
    let mut diagnostics = Vec::new();
    let interface = file.interface.and_then(|interface| {
        let interface = SkillInterface {
            display_name: normalize_string(
                interface.display_name,
                "interface.display_name",
                MAX_DISPLAY_NAME_CHARS,
                path,
                &mut diagnostics,
            ),
            short_description: normalize_string(
                interface.short_description,
                "interface.short_description",
                MAX_DESCRIPTION_CHARS,
                path,
                &mut diagnostics,
            ),
        };
        (!interface.is_empty()).then_some(interface)
    });

    let mut dependencies = Vec::new();
    for (index, dependency) in file.dependencies.tools.into_iter().enumerate() {
        let field = format!("dependencies.tools[{index}]");
        if dependency.dep_type.trim() != "mcp" {
            diagnostics.push(format!(
                "invalid host metadata in {}: {field}.type supports only `mcp`",
                path.display()
            ));
            continue;
        }
        let Some(value) = normalize_string(
            Some(dependency.value),
            &format!("{field}.value"),
            MAX_DEPENDENCY_VALUE_CHARS,
            path,
            &mut diagnostics,
        ) else {
            continue;
        };
        if value.starts_with("mcp__") {
            diagnostics.push(format!(
                "invalid host metadata in {}: {field}.value must be an MCP server identifier, not a namespaced tool name",
                path.display()
            ));
            continue;
        }
        dependencies.push(SkillToolDependency {
            value,
            description: normalize_string(
                dependency.description,
                &format!("{field}.description"),
                MAX_DESCRIPTION_CHARS,
                path,
                &mut diagnostics,
            ),
        });
    }

    (
        SkillHostMetadata {
            interface,
            dependencies,
        },
        diagnostics,
    )
}

fn normalize_string(
    raw: Option<String>,
    field: &str,
    max_chars: usize,
    path: &Path,
    diagnostics: &mut Vec<String>,
) -> Option<String> {
    let raw = raw?;
    let value = raw.trim();
    let reason = if value.is_empty() {
        Some("must not be empty".to_owned())
    } else if value.contains(['\n', '\r']) {
        Some("must be a single line".to_owned())
    } else if value.chars().count() > max_chars {
        Some(format!("must be at most {max_chars} characters"))
    } else {
        None
    };
    if let Some(reason) = reason {
        diagnostics.push(format!(
            "invalid host metadata in {}: {field} {reason}",
            path.display()
        ));
        return None;
    }
    Some(value.to_owned())
}

/// Normalize runtime metadata with the same field contract as the sidecar.
pub(crate) fn validate_host_metadata(
    metadata: SkillHostMetadata,
    source: &Path,
) -> Result<SkillHostMetadata, Vec<String>> {
    let file = HostMetadataFile {
        interface: metadata.interface.map(|interface| InterfaceFile {
            display_name: interface.display_name,
            short_description: interface.short_description,
        }),
        dependencies: DepsFile {
            tools: metadata
                .dependencies
                .into_iter()
                .map(|dependency| DepToolFile {
                    dep_type: "mcp".to_owned(),
                    value: dependency.value,
                    description: dependency.description,
                })
                .collect(),
        },
    };
    let (metadata, diagnostics) = metadata_from_file(file, source);
    if diagnostics.is_empty() {
        Ok(metadata)
    } else {
        Err(diagnostics)
    }
}

/// Serialize host metadata to canonical `agents/neo.yaml` YAML.
#[must_use]
pub fn serialize_host_metadata(metadata: &SkillHostMetadata) -> Option<String> {
    if metadata.is_empty() {
        return None;
    }
    let dependencies = (!metadata.dependencies.is_empty()).then(|| DepsOutput {
        tools: metadata
            .dependencies
            .iter()
            .map(|dependency| DepToolOutput {
                dep_type: "mcp",
                value: &dependency.value,
                description: dependency.description.as_deref(),
            })
            .collect(),
    });
    serde_yaml::to_string(&HostMetadataOutput {
        interface: metadata.interface.as_ref(),
        dependencies,
    })
    .ok()
}

impl SkillInterface {
    fn is_empty(&self) -> bool {
        self.display_name.is_none() && self.short_description.is_none()
    }
}

impl SkillHostMetadata {
    /// Whether the sidecar has no meaningful fields.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.interface.as_ref().is_none_or(SkillInterface::is_empty) && self.dependencies.is_empty()
    }

    /// Human-readable display name, falling back to the canonical skill name.
    #[must_use]
    pub fn display_name<'a>(&'a self, canonical_name: &'a str) -> &'a str {
        self.interface
            .as_ref()
            .and_then(|interface| interface.display_name.as_deref())
            .unwrap_or(canonical_name)
    }

    /// Short description, falling back to `None` (callers use the manifest description).
    #[must_use]
    pub fn short_description(&self) -> Option<&str> {
        self.interface
            .as_ref()
            .and_then(|interface| interface.short_description.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialization_roundtrips_through_sidecar_parser() {
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
        let file: HostMetadataFile = serde_yaml::from_str(&serialized).expect("parse YAML");
        let (parsed, diagnostics) = metadata_from_file(file, Path::new("test"));
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(original, parsed);
    }

    #[test]
    fn invalid_fields_are_diagnosed_without_discarding_valid_fields() {
        let file: HostMetadataFile = serde_yaml::from_str(
            "interface:\n  display_name: '  Schema Review  '\n  short_description: |\n    invalid\n    multiline\ndependencies:\n  tools:\n    - type: http\n      value: ignored\n    - type: mcp\n      value: '  registry  '\n      description: Registry MCP\n",
        )
        .expect("parse YAML");
        let (metadata, diagnostics) = metadata_from_file(file, Path::new("agents/neo.yaml"));
        assert_eq!(metadata.display_name("fallback"), "Schema Review");
        assert_eq!(metadata.short_description(), None);
        assert_eq!(metadata.dependencies[0].value, "registry");
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn empty_metadata_serializes_to_none() {
        assert_eq!(serialize_host_metadata(&SkillHostMetadata::default()), None);
    }
}
