//! Shared path-aware activation context envelope.
//!
//! One renderer serves both automatic `Skill` calls and manual `/skill:*`
//! activation so resource resolution never depends on the activation route.

use crate::skills::LoadedSkill;

/// Render the activated skill envelope for model context injection.
///
/// The envelope carries the absolute package root, optional MCP dependencies,
/// and the expanded skill body. The model resolves relative resource paths
/// against `root`.
#[must_use]
pub fn render_skill_context(skill: &LoadedSkill, instructions: &str) -> String {
    let mut xml = format!(
        "<neo-skill-loaded name=\"{name}\" source=\"{source}\" root=\"{root}\">",
        name = escape_xml(&skill.name),
        source = skill_source_label(skill),
        root = escape_xml(&skill.root.display().to_string()),
    );

    if !skill.host_metadata.dependencies.is_empty() {
        xml.push_str("\n<dependencies>\n");
        for dep in &skill.host_metadata.dependencies {
            xml.push_str(&format!(
                "  <mcp value=\"{}\">",
                escape_xml(&dep.value)
            ));
            if let Some(ref desc) = dep.description {
                xml.push_str(&escape_xml(desc));
            }
            xml.push_str("</mcp>\n");
        }
        xml.push_str("</dependencies>\n");
    }

    xml.push_str(&format!(
        "\n<instructions>\n{instructions}\n</instructions>\n</neo-skill-loaded>"
    ));
    xml
}

fn skill_source_label(skill: &LoadedSkill) -> &'static str {
    match skill.source {
        crate::skills::SkillSource::Builtin => "builtin",
        crate::skills::SkillSource::Extra => "extra",
        crate::skills::SkillSource::User => "user",
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillHostMetadata, SkillInterface, SkillManifest, SkillSource};
    use crate::skills::{SkillToolDependency, LoadedSkill};
    use std::path::PathBuf;

    fn make_skill(
        name: &str,
        root: &str,
        source: SkillSource,
        metadata: SkillHostMetadata,
    ) -> LoadedSkill {
        LoadedSkill {
            name: name.to_owned(),
            root: PathBuf::from(root),
            manifest: SkillManifest {
                name: name.to_owned(),
                description: "test".to_owned(),
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
            },
            body: "test body".to_owned(),
            source,
            host_metadata: metadata,
        }
    }

    #[test]
    fn render_skill_context_includes_name_source_root_and_body() {
        let skill = make_skill(
            "review",
            "/tmp/skills/review",
            SkillSource::User,
            SkillHostMetadata::default(),
        );
        let ctx = render_skill_context(&skill, "## Instructions\n\nReview the code.");

        assert!(ctx.contains("name=\"review\""));
        assert!(ctx.contains("source=\"user\""));
        assert!(ctx.contains("root=\"/tmp/skills/review\""));
        assert!(ctx.contains("## Instructions"));
        assert!(!ctx.contains("<dependencies>"));
    }

    #[test]
    fn render_skill_context_includes_dependencies_when_present() {
        let skill = make_skill(
            "schema-review",
            "/tmp/skills/schema-review",
            SkillSource::Extra,
            SkillHostMetadata {
                interface: None,
                dependencies: vec![SkillToolDependency {
                    value: "jsonSchemaRegistry".to_owned(),
                    description: Some("Schema registry MCP".to_owned()),
                }],
            },
        );
        let ctx = render_skill_context(&skill, "Review schemas.");

        assert!(ctx.contains("name=\"schema-review\""));
        assert!(ctx.contains("source=\"extra\""));
        assert!(ctx.contains("<dependencies>"));
        assert!(ctx.contains("<mcp value=\"jsonSchemaRegistry\">Schema registry MCP</mcp>"));
    }

    #[test]
    fn render_skill_context_escapes_xml_special_chars_in_name_and_path() {
        let skill = make_skill(
            "code<>&\"review",
            "/tmp/skills/<bad>path",
            SkillSource::User,
            SkillHostMetadata::default(),
        );
        let ctx = render_skill_context(&skill, "body");

        // Raw special chars must not appear as XML delimiters.
        assert!(!ctx.contains("name=\"code<"));
        assert!(!ctx.contains("root=\"/tmp/skills/<bad>"));
        assert!(ctx.contains("&lt;"));
        assert!(ctx.contains("&gt;"));
        assert!(ctx.contains("&amp;"));
        assert!(ctx.contains("&quot;"));
    }
}
