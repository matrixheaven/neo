use std::{collections::HashMap, fs, path::Path};

use neo_agent_core::skills::{
    LoadedSkill, SkillArgument, SkillInvocation, SkillLoadError, SkillManifest, SkillSource,
    SkillStore, SkillType, builtin::builtin_skills, discovery::discover_skills, expand_skill_body,
    load_skill_file, parse_skill_invocation,
};
use serde_json::json;

#[test]
fn load_skill_parses_yaml_frontmatter_and_body() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: reviewer
description: Review repository changes
type: prompt
whenToUse: When the user asks for a code review
---

# Reviewer

Use focused findings.
",
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.manifest.name, "reviewer");
    assert_eq!(skill.manifest.description, "Review repository changes");
    assert!(matches!(skill.manifest.skill_type, SkillType::Prompt));
    assert_eq!(
        skill.manifest.when_to_use,
        Some("When the user asks for a code review".into())
    );
    assert!(skill.body.contains("Use focused findings."));
    assert_eq!(skill.root, dir.path());
}

#[test]
fn load_skill_parses_arguments() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: review-pr
description: Review a pull request
arguments:
  - name: pr_ref
    description: The PR to review
    required: true
  - name: mode
    default: quick
---
Review $pr_ref in $mode mode.
",
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.manifest.arguments.len(), 2);
    assert_eq!(skill.manifest.arguments[0].name, "pr_ref");
    assert!(skill.manifest.arguments[0].required);
    assert_eq!(skill.manifest.arguments[1].name, "mode");
    assert_eq!(skill.manifest.arguments[1].default, Some("quick".into()));
}

#[test]
fn expand_skill_body_replaces_named_and_positional_arguments() {
    let skill = LoadedSkill {
        name: "review".into(),
        root: std::path::PathBuf::from("/tmp/skills/review"),
        manifest: SkillManifest {
            name: "review".into(),
            description: "Review".into(),
            skill_type: SkillType::Prompt,
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![
                SkillArgument {
                    name: "target".into(),
                    description: None,
                    required: true,
                    default: None,
                },
                SkillArgument {
                    name: "mode".into(),
                    description: None,
                    required: false,
                    default: Some("quick".into()),
                },
            ],
            slash_commands: vec![],
        },
        body: "Review $target in $mode mode. Also see $0 and $1.".into(),
        source: SkillSource::default(),
    };
    let invocation = SkillInvocation {
        name: "review".into(),
        raw_arguments: "src/lib.rs thorough".into(),
        positional: vec!["src/lib.rs".into(), "thorough".into()],
        named: HashMap::default(),
    };

    let expanded = expand_skill_body(&skill, &invocation).unwrap();

    assert_eq!(
        expanded,
        "Review src/lib.rs in thorough mode. Also see src/lib.rs and thorough."
    );
}

#[test]
fn expand_skill_body_appends_arguments_when_no_placeholders() {
    let skill = LoadedSkill {
        name: "summarize".into(),
        root: std::path::PathBuf::from("/tmp/skills/summarize"),
        manifest: SkillManifest {
            name: "summarize".into(),
            description: "Summarize".into(),
            skill_type: SkillType::Prompt,
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
            slash_commands: vec![],
        },
        body: "Summarize the following.".into(),
        source: SkillSource::default(),
    };
    let invocation = SkillInvocation {
        name: "summarize".into(),
        raw_arguments: "a long document".into(),
        positional: vec!["a".into(), "long".into(), "document".into()],
        named: HashMap::default(),
    };

    let expanded = expand_skill_body(&skill, &invocation).unwrap();

    assert_eq!(
        expanded,
        "Summarize the following.\n\nARGUMENTS: a long document"
    );
}

#[test]
fn expand_skill_body_substitutes_undeclared_named_argument_via_placeholder() {
    // A skill with no declared arguments but a body that references `$task`.
    // The model passed an undeclared `task` argument — it must be substituted
    // rather than rejected (the brainstorming scenario).
    let skill = LoadedSkill {
        name: "brainstorming".into(),
        root: std::path::PathBuf::from("/tmp/skills/brainstorming"),
        manifest: SkillManifest {
            name: "brainstorming".into(),
            description: "Brainstorm".into(),
            skill_type: SkillType::Prompt,
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
            slash_commands: vec![],
        },
        body: "Explore the idea: $task".into(),
        source: SkillSource::default(),
    };
    let mut named = HashMap::new();
    named.insert("task".to_string(), "refactor skills".to_string());
    let invocation = SkillInvocation {
        name: "brainstorming".into(),
        raw_arguments: "{\"task\":\"refactor skills\"}".into(),
        positional: vec![],
        named,
    };

    let expanded = expand_skill_body(&skill, &invocation).unwrap();

    assert_eq!(expanded, "Explore the idea: refactor skills");
}

#[test]
fn expand_skill_body_passes_undeclared_argument_through_when_no_placeholder() {
    // No declared arguments, no `$task` placeholder in body: the undeclared
    // argument must not error; it rides along in the ARGUMENTS trailer.
    let skill = LoadedSkill {
        name: "brainstorming".into(),
        root: std::path::PathBuf::from("/tmp/skills/brainstorming"),
        manifest: SkillManifest {
            name: "brainstorming".into(),
            description: "Brainstorm".into(),
            skill_type: SkillType::Prompt,
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
            slash_commands: vec![],
        },
        body: "Start the brainstorming process.".into(),
        source: SkillSource::default(),
    };
    let mut named = HashMap::new();
    named.insert("task".to_string(), "refactor skills".to_string());
    let invocation = SkillInvocation {
        name: "brainstorming".into(),
        raw_arguments: "{\"task\":\"refactor skills\"}".into(),
        positional: vec![],
        named,
    };

    let expanded = expand_skill_body(&skill, &invocation).unwrap();

    assert_eq!(
        expanded,
        "Start the brainstorming process.\n\nARGUMENTS: {\"task\":\"refactor skills\"}"
    );
}

#[test]
fn expand_skill_body_still_fails_on_missing_required_argument() {
    // Tolerance of undeclared arguments must not weaken the required check.
    let skill = LoadedSkill {
        name: "review".into(),
        root: std::path::PathBuf::from("/tmp/skills/review"),
        manifest: SkillManifest {
            name: "review".into(),
            description: "Review".into(),
            skill_type: SkillType::Prompt,
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: None,
                required: true,
                default: None,
            }],
            slash_commands: vec![],
        },
        body: "Review $target.".into(),
        source: SkillSource::default(),
    };
    // An undeclared arg is passed but the required `target` is missing.
    let mut named = HashMap::new();
    named.insert("extra".to_string(), "noise".to_string());
    let invocation = SkillInvocation {
        name: "review".into(),
        raw_arguments: "{\"extra\":\"noise\"}".into(),
        positional: vec![],
        named,
    };

    let err = expand_skill_body(&skill, &invocation).unwrap_err();

    assert_eq!(err.to_string(), "missing required skill argument `target`");
}

#[test]
fn parse_skill_invocation_handles_named_and_positional_args() {
    let invocation = parse_skill_invocation("#123 --mode=full").unwrap();

    assert_eq!(invocation.positional, vec!["#123"]);
    assert_eq!(invocation.named.get("mode"), Some(&"full".into()));
}

#[test]
fn discover_skills_finds_subskills() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("superpowers").join("SKILL.md"),
        r"---
name: superpowers
description: Superpowers collection
---
Parent skill.
",
    );
    write(
        dir.path()
            .join("superpowers")
            .join("skills")
            .join("brainstorming")
            .join("SKILL.md"),
        r"---
name: brainstorming
description: Brainstorm ideas
---
Brainstorm.
",
    );

    let skills = discover_skills(dir.path(), SkillSource::default()).unwrap();
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert!(names.contains(&"superpowers"));
    assert!(names.contains(&"superpowers/brainstorming"));
}

#[test]
fn skill_store_tiers_override() {
    // With the project tier removed, skills load only from user/extra dirs.
    // A user skill is the sole source for a given name.
    let user_dir = tempfile::tempdir().unwrap();

    write(
        user_dir
            .path()
            .join("skills")
            .join("shared")
            .join("SKILL.md"),
        r"---
name: shared
description: user version
---
user
",
    );

    let store = SkillStore::load(&[user_dir.path().join("skills")], &[], Vec::new()).unwrap();

    assert_eq!(
        store.get("shared").unwrap().manifest.description,
        "user version"
    );
}

#[test]
fn builtin_skills_load() {
    let skills = builtin_skills().unwrap();
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert!(!names.contains(&"define-goal"));
    assert!(names.contains(&"mcp-config"));
    assert!(names.contains(&"sub-skill"));
}

#[test]
fn load_skill_rejects_invalid_yaml() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: [invalid, list]
---
body
",
    );

    let err = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap_err();

    assert!(matches!(err, SkillLoadError::ParseFrontmatter { .. }));
}

#[test]
fn skill_manifest_serializes_with_stable_shape() {
    let manifest = SkillManifest {
        name: "shape".into(),
        description: "Stable manifest".into(),
        skill_type: SkillType::Prompt,
        when_to_use: Some("When needed".into()),
        disable_model_invocation: true,
        arguments: vec![SkillArgument {
            name: "target".into(),
            description: Some("target file".into()),
            required: true,
            default: None,
        }],
        slash_commands: vec!["/shape".into()],
    };

    let value = serde_json::to_value(&manifest).unwrap();

    assert_eq!(
        value,
        json!({
            "name": "shape",
            "description": "Stable manifest",
            "type": "prompt",
            "whenToUse": "When needed",
            "disableModelInvocation": true,
            "arguments": [
                { "name": "target", "description": "target file", "required": true }
            ],
            "slashCommands": ["/shape"]
        })
    );
}

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}
