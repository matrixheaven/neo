use std::{collections::HashMap, fs, path::Path};

use neo_agent_core::skills::{
    LoadedSkill, SkillArgument, SkillHostMetadata, SkillInvocation, SkillLoadError, SkillManifest,
    SkillSource, SkillStore,
    builtin::builtin_skills,
    discovery::{discover_skills, user_skill_dirs},
    expand_skill_body, load_skill_file, parse_skill_invocation,
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
whenToUse: When the user asks for a code review
---

# Reviewer

Use focused findings.
",
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.manifest.name, "reviewer");
    assert_eq!(skill.manifest.description, "Review repository changes");
    assert!(!skill.manifest.disable_model_invocation);
    assert_eq!(
        skill.manifest.when_to_use,
        Some("When the user asks for a code review".into())
    );
    assert!(skill.body.contains("Use focused findings."));
    assert_eq!(skill.root, dir.path());
}

#[test]
fn load_skill_repairs_unquoted_description_containing_colon_space() {
    let dir = tempfile::tempdir().unwrap();
    let description =
        "Use when the conversation contains an explicit `TDD Route: strict` decision.";
    write(
        dir.path().join("SKILL.md"),
        &format!("---\nname: test-driven-development\ndescription: {description}\n---\nbody\n"),
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.manifest.description, description);
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
        },
        body: "Review $target in $mode mode. Also see $0 and $1.".into(),
        source: SkillSource::default(),
        host_metadata: SkillHostMetadata::default(),
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
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
        },
        body: "Summarize the following.".into(),
        source: SkillSource::default(),
        host_metadata: SkillHostMetadata::default(),
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
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
        },
        body: "Explore the idea: $task".into(),
        source: SkillSource::default(),
        host_metadata: SkillHostMetadata::default(),
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
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![],
        },
        body: "Start the brainstorming process.".into(),
        source: SkillSource::default(),
        host_metadata: SkillHostMetadata::default(),
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
            when_to_use: None,
            disable_model_invocation: false,
            arguments: vec![SkillArgument {
                name: "target".into(),
                description: None,
                required: true,
                default: None,
            }],
        },
        body: "Review $target.".into(),
        source: SkillSource::default(),
        host_metadata: SkillHostMetadata::default(),
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
fn user_skill_dirs_contains_only_neo_skills() {
    let home = Path::new("/home/alice/.neo");
    assert_eq!(user_skill_dirs(home), vec![home.join("skills")]);
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

    let skills = discover_skills(dir.path(), SkillSource::default()).0;
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert!(names.contains(&"superpowers"));
    assert!(names.contains(&"superpowers/brainstorming"));
}

#[test]
fn discover_skills_loads_package_root_once() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: package-root
description: Package root
---
Root skill.
",
    );

    let skills = discover_skills(dir.path(), SkillSource::default()).0;
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert_eq!(names, vec!["package-root"]);
}

#[test]
fn discover_skills_ignores_resource_dirs_under_skill_package() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("parent").join("SKILL.md"),
        r"---
name: parent
description: Parent skill
---
Parent skill.
",
    );
    for resource_dir in ["agents", "references", "scripts", "assets"] {
        write(
            dir.path()
                .join("parent")
                .join(resource_dir)
                .join("SKILL.md"),
            &format!(
                r"---
name: {resource_dir}
description: Resource dir
---
Resource dir.
"
            ),
        );
    }

    let skills = discover_skills(dir.path(), SkillSource::default()).0;
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert_eq!(names, vec!["parent"]);
}

#[test]
fn discover_skills_still_finds_children_under_skills_dir() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("parent").join("SKILL.md"),
        r"---
name: parent
description: Parent skill
---
Parent skill.
",
    );
    write(
        dir.path()
            .join("parent")
            .join("skills")
            .join("child")
            .join("SKILL.md"),
        r"---
name: child
description: Child skill
---
Child skill.
",
    );

    let skills = discover_skills(dir.path(), SkillSource::default()).0;
    let names: Vec<_> = skills.iter().map(|skill| skill.name.as_str()).collect();

    assert_eq!(names, vec!["parent", "parent/child"]);
}

#[test]
fn discovery_is_bounded_cycle_safe_and_keeps_valid_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");

    // Valid sibling.
    write(
        root.join("valid").join("SKILL.md"),
        r"---
name: valid
description: Valid skill
---
Valid body.
",
    );
    write(
        root.join("valid").join("agents").join("neo.yaml"),
        "display_name: [invalid\n",
    );

    // Malformed sibling.
    write(
        root.join("malformed").join("SKILL.md"),
        r"---
name: [invalid
---
Broken.
",
    );

    let depth_six = (1..=6).fold(root.clone(), |path, depth| {
        path.join(format!("depth-{depth}"))
    });
    write(
        depth_six.join("SKILL.md"),
        r"---
name: depth-six
description: Depth six skill
---
Depth six body.
",
    );

    let linked_target = dir.path().join("linked-target");
    write(
        linked_target.join("SKILL.md"),
        r"---
name: linked
description: Linked skill
---
Linked body.
",
    );
    let linked_view = root.join("linked-view");
    let linked_created = create_dir_symlink(&linked_target, &linked_view);

    let cycle_dir = root.join("cycle");
    write(
        cycle_dir.join("SKILL.md"),
        r"---
name: cycle
description: Cycle skill
---
Cycle body.
",
    );
    let cycle_link = cycle_dir.join("loop");
    let cycle_created = create_dir_symlink(&cycle_dir, &cycle_link);

    let (skills, diagnostics) = discover_skills(&root, SkillSource::default());
    let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();

    assert!(names.contains(&"valid"), "valid sibling missing: {names:?}");
    assert!(
        names.contains(&"depth-six"),
        "depth-six skill missing: {names:?}"
    );

    if linked_created {
        let linked = skills
            .iter()
            .find(|skill| skill.name == "linked")
            .expect("linked skill should be discovered");
        assert_eq!(linked.root, linked_view);
    }

    if cycle_created {
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.path == cycle_link
                    && diagnostic
                        .message
                        .contains("symlink cycle or already-visited")
            }),
            "expected cycle diagnostic: {diagnostics:?}"
        );
    }

    let malformed_diag = diagnostics
        .iter()
        .any(|d| d.path == root.join("malformed").join("SKILL.md"));
    assert!(malformed_diag, "expected diagnostic for malformed skill");
    assert!(
        diagnostics
            .iter()
            .any(|d| d.path == root.join("valid").join("agents").join("neo.yaml")),
        "expected diagnostic for malformed host metadata"
    );
}

#[test]
fn skill_store_reports_duplicate_qualified_names_within_a_tier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    for root in [&first, &second] {
        write(
            root.join("package").join("SKILL.md"),
            "---\nname: duplicate\ndescription: Duplicate skill\n---\nBody.\n",
        );
    }

    let store = SkillStore::load(&[], &[first, second.clone()], Vec::new());
    let loaded = store.get("duplicate").expect("duplicate skill loaded");
    assert!(loaded.root.starts_with(&second));
    assert!(store.diagnostics().iter().any(|diagnostic| {
        diagnostic.path == loaded.root
            && diagnostic
                .message
                .contains("duplicate qualified skill name `duplicate` within extra tier")
    }));
}

#[test]
fn discovery_diagnoses_non_directory_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("not-a-directory");
    write(&root, "not a directory");

    let (skills, diagnostics) = discover_skills(&root, SkillSource::User);
    assert!(skills.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, root);
    assert!(diagnostics[0].message.contains("not a directory"));
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

    let store = SkillStore::load(&[user_dir.path().join("skills")], &[], Vec::new());

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
        when_to_use: Some("When needed".into()),
        disable_model_invocation: true,
        arguments: vec![SkillArgument {
            name: "target".into(),
            description: Some("target file".into()),
            required: true,
            default: None,
        }],
    };

    let value = serde_json::to_value(&manifest).unwrap();

    assert_eq!(
        value,
        json!({
            "name": "shape",
            "description": "Stable manifest",
            "whenToUse": "When needed",
            "disableModelInvocation": true,
            "arguments": [
                { "name": "target", "description": "target file", "required": true }
            ]
        })
    );
}

#[test]
fn loads_canonical_manifest_without_retired_execution_types() {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: canonical
description: Has only canonical fields
disableModelInvocation: true
arguments:
  - name: target
    required: true
---

# Canonical

No retired fields.
",
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.manifest.name, "canonical");
    assert_eq!(skill.manifest.description, "Has only canonical fields");
    assert!(skill.manifest.disable_model_invocation);
    assert!(!skill.manifest.auto_invokable());
    assert_eq!(skill.manifest.arguments.len(), 1);
    assert_eq!(skill.manifest.arguments[0].name, "target");
}

#[test]
fn neo_host_metadata_loads_and_invalid_optional_metadata_falls_back() {
    use neo_agent_core::skills::SkillHostMetadata;

    // Valid sidecar.
    let dir = tempfile::tempdir().unwrap();
    let agents_dir = dir.path().join("agents");
    fs::create_dir_all(&agents_dir).unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: schema-review
description: Review schemas
---
# Schema Review
",
    );
    write(
        agents_dir.join("neo.yaml"),
        r#"interface:
  display_name: "Schema Review"
  short_description: "Review JSON schemas"
dependencies:
  tools:
    - type: mcp
      value: jsonSchemaRegistry
      description: "Registry MCP"
"#,
    );

    let skill = load_skill_file(&dir.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill.display_name(), "Schema Review");
    assert_eq!(skill.short_description(), Some("Review JSON schemas"));
    assert_eq!(skill.host_metadata.dependencies.len(), 1);
    assert_eq!(
        skill.host_metadata.dependencies[0].value,
        "jsonSchemaRegistry"
    );

    // Malformed sidecar — skill still loads, metadata falls back.
    let dir2 = tempfile::tempdir().unwrap();
    let agents_dir2 = dir2.path().join("agents");
    fs::create_dir_all(&agents_dir2).unwrap();
    write(
        dir2.path().join("SKILL.md"),
        r"---
name: malformed
description: Broken sidecar
---
Body.
",
    );
    write(agents_dir2.join("neo.yaml"), "%%% not yaml %%%");

    let skill2 = load_skill_file(&dir2.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill2.host_metadata, SkillHostMetadata::default());

    // No sidecar file at all.
    let dir3 = tempfile::tempdir().unwrap();
    write(
        dir3.path().join("SKILL.md"),
        r"---
name: no-sidecar
description: No agents dir
---
Body.
",
    );

    let skill3 = load_skill_file(&dir3.path().join("SKILL.md"), SkillSource::default()).unwrap();

    assert_eq!(skill3.host_metadata, SkillHostMetadata::default());
}

#[test]
fn host_metadata_does_not_change_model_visible_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let agents_dir = dir.path().join("agents");
    fs::create_dir_all(&agents_dir).unwrap();
    write(
        dir.path().join("SKILL.md"),
        r"---
name: fancy-skill
description: Model-facing description
---
# Fancy Skill
",
    );
    write(
        agents_dir.join("neo.yaml"),
        r#"interface:
  display_name: "Fancy Display"
  short_description: "Human picker summary"
dependencies:
  tools:
    - type: mcp
      value: someServer
"#,
    );

    let store = SkillStore::load(&[], &[dir.path().to_path_buf()], vec![]);
    let catalog = store.available_skills_prompt();

    // Sidecar metadata must NOT appear in the model-visible catalog.
    assert!(!catalog.contains("Fancy Display"));
    assert!(!catalog.contains("Human picker summary"));
    assert!(!catalog.contains("someServer"));
    // Canonical manifest description still appears.
    assert!(catalog.contains("Model-facing description"));
}

fn write(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

#[cfg(unix)]
fn create_dir_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).expect("create directory symlink");
    true
}

#[cfg(windows)]
fn create_dir_symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).expect("create directory symlink");
    true
}

#[cfg(not(any(unix, windows)))]
fn create_dir_symlink(_target: &Path, _link: &Path) -> bool {
    false
}
