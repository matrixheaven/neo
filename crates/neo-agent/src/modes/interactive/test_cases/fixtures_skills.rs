//! Interactive test fixtures: skill store scaffolding (moved from `mod.rs`).

use neo_agent_core::skills::{
    LoadedSkill, SkillHostMetadata, SkillManifest, SkillSource, SkillStore, SkillToolDependency,
};
use std::path::PathBuf;

use super::fixtures_sessions::*;

pub fn skill_store_with_refactor_skill() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![LoadedSkill {
            name: "refactor".to_owned(),
            root: PathBuf::from("builtin/refactor"),
            manifest: SkillManifest {
                name: "refactor".to_owned(),
                description: "Refactor with project conventions".to_owned(),
                when_to_use: None,
                disable_model_invocation: false,
                arguments: Vec::new(),
            },
            body: "Refactor safely.".to_owned(),
            source: SkillSource::Builtin,
            host_metadata: SkillHostMetadata::default(),
        }],
    )
}

pub fn skill_store_with_two_prompt_skills() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![
            LoadedSkill {
                name: "skill_one".to_owned(),
                root: test_workspace_root().join("builtin/skill_one"),
                manifest: SkillManifest {
                    name: "skill_one".to_owned(),
                    description: "First skill".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                },
                body: "ONE: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata {
                    interface: None,
                    dependencies: vec![SkillToolDependency {
                        value: "reviewServer".to_owned(),
                        description: Some("Review MCP server".to_owned()),
                    }],
                },
            },
            LoadedSkill {
                name: "skill_two".to_owned(),
                root: test_workspace_root().join("builtin/skill_two"),
                manifest: SkillManifest {
                    name: "skill_two".to_owned(),
                    description: "Second skill".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: false,
                    arguments: Vec::new(),
                },
                body: "TWO: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
        ],
    )
}

pub fn skill_store_with_interactive_preflight_skills() -> SkillStore {
    SkillStore::load(
        &[],
        &[],
        vec![
            LoadedSkill {
                name: "self-evo".to_owned(),
                root: PathBuf::from("builtin/self-evo"),
                manifest: SkillManifest {
                    name: "self-evo".to_owned(),
                    description: "Distill session learning into reusable skills".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: true,
                    arguments: Vec::new(),
                },
                body: "SELF EVO: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
            LoadedSkill {
                name: "create-skill".to_owned(),
                root: PathBuf::from("builtin/create-skill"),
                manifest: SkillManifest {
                    name: "create-skill".to_owned(),
                    description: "Create a reusable skill from instructions".to_owned(),
                    when_to_use: None,
                    disable_model_invocation: true,
                    arguments: Vec::new(),
                },
                body: "CREATE SKILL: $ARGUMENTS".to_owned(),
                source: SkillSource::Builtin,
                host_metadata: SkillHostMetadata::default(),
            },
        ],
    )
}
