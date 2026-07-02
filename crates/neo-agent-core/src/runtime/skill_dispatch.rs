use neo_ai::ToolSpec;

use crate::ToolResult;
use crate::skills::SkillStore;

pub(super) fn invoke_skill_tool_spec() -> ToolSpec {
    ToolSpec {
        name: "Skill".to_owned(),
        description: "Invoke an available skill by name with arguments. Use this when the user's request matches a skill's description or whenToUse.".to_owned(),
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
    skills: Option<&SkillStore>,
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
    if skill_is_manual_only(skill) {
        return ToolResult::error(format!(
            "skill `{}` is type `flow` and can only be invoked manually via /skill:{}",
            request.skill_name, request.skill_name
        ));
    }

    let invocation = request.into_invocation();

    match crate::skills::expand_skill_body(skill, &invocation) {
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

fn skill_is_manual_only(skill: &crate::skills::LoadedSkill) -> bool {
    matches!(skill.manifest.skill_type, crate::skills::SkillType::Flow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{SkillArgument, SkillManifest, SkillSource, SkillType};
    use serde_json::json;
    use std::path::PathBuf;

    fn write_skill(root: &std::path::Path, name: &str, skill_type: &str, body: &str) {
        let skill_dir = root.join(name);
        std::fs::create_dir_all(&skill_dir).expect("skill dir");
        std::fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                r"---
name: {name}
description: {name} skill
type: {skill_type}
arguments:
  - name: target
    required: true
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

    fn skill_store(root: &std::path::Path) -> SkillStore {
        SkillStore::load(&[], &[root.to_path_buf()], Vec::new()).expect("skill store")
    }

    #[test]
    fn execute_invoke_skill_expands_named_string_arguments() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", "prompt", "Review $target.");
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
        write_skill(temp.path(), "manual-flow", "flow", "Manual only.");
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
            "skill `manual-flow` is type `flow` and can only be invoked manually via /skill:manual-flow"
        );
    }

    #[test]
    fn execute_invoke_skill_allows_multiple_invocations_in_same_turn() {
        let temp = tempfile::tempdir().expect("tempdir");
        write_skill(temp.path(), "review", "prompt", "Review $target.");
        write_skill(temp.path(), "summarize", "prompt", "Summarize $target.");
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
    fn skill_is_manual_only_tracks_flow_manifest_type() {
        let prompt = crate::skills::LoadedSkill {
            name: "prompt".to_owned(),
            root: PathBuf::from("/tmp/prompt"),
            manifest: SkillManifest {
                name: "prompt".to_owned(),
                description: "Prompt".to_owned(),
                skill_type: SkillType::Prompt,
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::<SkillArgument>::new(),
                slash_commands: Vec::new(),
            },
            body: String::new(),
            source: SkillSource::default(),
        };
        let flow = crate::skills::LoadedSkill {
            manifest: SkillManifest {
                skill_type: SkillType::Flow,
                ..prompt.manifest.clone()
            },
            ..prompt.clone()
        };

        assert!(!skill_is_manual_only(&prompt));
        assert!(skill_is_manual_only(&flow));
    }
}
