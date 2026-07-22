use neo_ai::ToolSpec;

use crate::ToolResult;
use crate::skills::SkillStoreHandle;

pub(super) fn invoke_skill_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "Skill".to_owned(),
        description: "Invoke an available skill by name with arguments. \
            BLOCKING REQUIREMENT: when a skill from the available skills listing matches \
            the user's request or current task, you MUST call this tool instead of \
            attempting the work with free-form text. Do not re-invoke a skill whose \
            instructions are already present in the conversation."
            .to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "Name of the skill to invoke"
                },
                "arguments": {
                    "type": "object",
                    "description": "Named arguments for the skill"
                }
            },
            "required": ["skill"]
        }),
    }
}

pub(super) fn execute_invoke_skill(
    skills: Option<&SkillStoreHandle>,
    arguments: &serde_json::Value,
) -> ToolResult {
    let Some(skills) = skills else {
        return ToolResult::error("skill system is not enabled");
    };
    let request = match skill_tool_request(arguments) {
        Ok(request) => request,
        Err(message) => return ToolResult::error(message),
    };
    let Some(skill) = skills.get(&request.skill_name) else {
        return ToolResult::error(format!("skill `{}` is not available", request.skill_name));
    };
    if skill_is_manual_only(&skill) {
        return ToolResult::error(format!(
            "skill `{}` is marked manual-only and can only be invoked via /skill:{}",
            request.skill_name, request.skill_name
        ));
    }

    let invocation = request.into_invocation();

    match crate::skills::expand_skill_body(&skill, &invocation) {
        Ok(body) => ToolResult::ok(body),
        Err(err) => ToolResult::error(format!(
            "failed to expand skill `{}`: {err}",
            invocation.name
        )),
    }
}

struct SkillToolRequest {
    skill_name: String,
    arguments: serde_json::Map<String, serde_json::Value>,
}

impl SkillToolRequest {
    fn into_invocation(self) -> crate::skills::SkillInvocation {
        crate::skills::SkillInvocation {
            name: self.skill_name,
            raw_arguments: serde_json::to_string(&self.arguments).unwrap_or_default(),
            positional: Vec::new(),
            named: string_skill_arguments(self.arguments),
        }
    }
}

fn skill_tool_request(arguments: &serde_json::Value) -> Result<SkillToolRequest, String> {
    let skill_name = arguments
        .get("skill")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "Skill requires a `skill` string argument".to_owned())?;
    let arguments = arguments
        .get("arguments")
        .and_then(|value| value.as_object())
        .cloned()
        .unwrap_or_default();
    Ok(SkillToolRequest {
        skill_name: skill_name.to_owned(),
        arguments,
    })
}

fn string_skill_arguments(
    arguments: serde_json::Map<String, serde_json::Value>,
) -> std::collections::HashMap<String, String> {
    arguments
        .into_iter()
        .filter_map(|(key, value)| value.as_str().map(|string| (key, string.to_owned())))
        .collect()
}

/// Format the inner `arguments` object of a Skill tool call into a readable
/// `key: value` multi-line string for display in the `SkillActivation` card.
/// Returns an empty string when the skill was invoked without extra arguments.
pub(super) fn format_skill_tool_arguments(tool_arguments: &serde_json::Value) -> String {
    let Some(inner) = tool_arguments.get("arguments").and_then(|v| v.as_object()) else {
        return String::new();
    };
    let lines: Vec<String> = inner
        .iter()
        .filter_map(|(key, value)| {
            let display = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => return None,
                other => other.to_string(),
            };
            Some(format!("{key}: {display}"))
        })
        .collect();
    lines.join("\n")
}

fn skill_is_manual_only(skill: &crate::skills::LoadedSkill) -> bool {
    skill.manifest.disable_model_invocation
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillArgument, SkillManifest, SkillSource, SkillStore};
    use serde_json::json;
    use std::path::PathBuf;

    fn write_skill(root: &std::path::Path, name: &str, manual_only: bool, body: &str) {
        let skill_dir = root.join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                r"---
name: {name}
description: {name} skill
arguments:
  - name: target
    required: true
disableModelInvocation: {manual_only}
---
{body}
"
            ),
        )
        .expect("skill file");
    }

    fn skill_arguments(arguments: serde_json::Value) -> serde_json::Value {
        arguments
    }

    fn skill_store(root: &std::path::Path) -> SkillStoreHandle {
        SkillStoreHandle::new(
            SkillStore::load(&[], &[root.to_path_buf()], Vec::new()),
        )
    }

    #[test]
    fn execute_invoke_skill_expands_named_string_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", false, "Review $target.");
        let store = skill_store(temp.path());

        let result = execute_invoke_skill(
            Some(&store),
            &skill_arguments(json!({
                "skill": "review",
                "arguments": {
                    "target": "src/lib.rs",
                    "ignored": 42
                }
            })),
        );

        assert_eq!(result, ToolResult::ok("Review src/lib.rs.\n"));
    }

    #[test]
    fn execute_invoke_skill_rejects_disabled_missing_and_flow_cases() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "manual-flow", true, "Manual only.");
        let store = skill_store(temp.path());

        let no_store = execute_invoke_skill(None, &skill_arguments(json!({"skill": "review"})));
        assert_eq!(no_store.content, "skill system is not enabled");
        assert!(no_store.is_error);

        let missing_name =
            execute_invoke_skill(Some(&store), &skill_arguments(json!({"arguments": {}})));
        assert_eq!(
            missing_name.content,
            "Skill requires a `skill` string argument"
        );

        let missing_skill =
            execute_invoke_skill(Some(&store), &skill_arguments(json!({"skill": "review"})));
        assert_eq!(missing_skill.content, "skill `review` is not available");

        let flow = execute_invoke_skill(
            Some(&store),
            &skill_arguments(json!({"skill": "manual-flow"})),
        );
        assert_eq!(
            flow.content,
            "skill `manual-flow` is marked manual-only and can only be invoked via /skill:manual-flow"
        );
    }

    #[test]
    fn execute_invoke_skill_allows_multiple_invocations_in_same_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", false, "Review $target.");
        write_skill(temp.path(), "summarize", false, "Summarize $target.");
        let store = skill_store(temp.path());

        let first = execute_invoke_skill(
            Some(&store),
            &skill_arguments(json!({
                "skill": "review",
                "arguments": {"target": "src/lib.rs"}
            })),
        );
        let second = execute_invoke_skill(
            Some(&store),
            &skill_arguments(json!({
                "skill": "summarize",
                "arguments": {"target": "src/main.rs"}
            })),
        );

        assert_eq!(first, ToolResult::ok("Review src/lib.rs.\n"));
        assert_eq!(second, ToolResult::ok("Summarize src/main.rs.\n"));
    }

    #[test]
    fn skill_tool_request_converts_only_string_named_arguments() {
        let request = skill_tool_request(&json!({
            "skill": "review",
            "arguments": {
                "target": "src/lib.rs",
                "count": 3,
                "flag": true
            }
        }))
        .expect("request");
        let invocation = request.into_invocation();

        assert_eq!(invocation.name, "review");
        assert_eq!(
            invocation.named.get("target"),
            Some(&"src/lib.rs".to_owned())
        );
        assert!(!invocation.named.contains_key("count"));
        assert!(!invocation.named.contains_key("flag"));
        assert_eq!(
            invocation.raw_arguments,
            r#"{"count":3,"flag":true,"target":"src/lib.rs"}"#
        );
    }

    #[test]
    fn skill_is_manual_only_tracks_disable_model_invocation() {
        let auto = crate::skills::LoadedSkill {
            name: "auto".to_owned(),
            root: PathBuf::from("/tmp/auto"),
            manifest: SkillManifest {
                name: "auto".to_owned(),
                description: "Auto".to_owned(),
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::<SkillArgument>::new(),
            },
            body: String::new(),
            source: SkillSource::default(),
            host_metadata: crate::skills::SkillHostMetadata::default(),
        };
        let manual = crate::skills::LoadedSkill {
            manifest: SkillManifest {
                disable_model_invocation: true,
                ..auto.manifest.clone()
            },
            ..auto.clone()
        };

        assert!(!skill_is_manual_only(&auto));
        assert!(skill_is_manual_only(&manual));
    }

    #[test]
    fn format_skill_tool_arguments_formats_inner_arguments() {
        let args = json!({
            "skill": "review",
            "arguments": {
                "target": "src/lib.rs",
                "severity": "high"
            }
        });
        let body = super::format_skill_tool_arguments(&args);
        assert_eq!(body, "severity: high\ntarget: src/lib.rs");
    }

    #[test]
    fn format_skill_tool_arguments_empty_when_no_inner_arguments() {
        let args = json!({ "skill": "brainstorming" });
        let body = super::format_skill_tool_arguments(&args);
        assert!(body.is_empty());
    }

    #[test]
    fn format_skill_tool_arguments_skips_null_values() {
        let args = json!({
            "skill": "review",
            "arguments": {
                "target": "src/lib.rs",
                "optional": null
            }
        });
        let body = super::format_skill_tool_arguments(&args);
        assert_eq!(body, "target: src/lib.rs");
    }
}
