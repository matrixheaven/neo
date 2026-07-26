//! Ordinary host-compiled workflow definitions (design §40).
//!
//! Built-ins are immutable registry definitions that use only public Lua host
//! APIs. They receive no privileged host functions and resolve through the same
//! paired `.lua` + `.workflow.toml` path as user/project definitions.

use super::registry::BuiltinWorkflowDefinition;

const DEEP_RESEARCH_MANIFEST: &str = include_str!("deep-research.workflow.toml");
const DEEP_RESEARCH_SOURCE: &str = include_str!("deep-research.lua");
const CODE_REVIEW_MANIFEST: &str = include_str!("code-review.workflow.toml");
const CODE_REVIEW_SOURCE: &str = include_str!("code-review.lua");
const LARGE_REFACTOR_MANIFEST: &str = include_str!("large-refactor.workflow.toml");
const LARGE_REFACTOR_SOURCE: &str = include_str!("large-refactor.lua");

/// All ordinary built-in workflow definition pairs shipped with Neo.
///
/// Order is stable (name ascending) so revision vectors and list projections are
/// deterministic across platforms.
#[must_use]
pub fn builtin_workflow_definitions() -> Vec<BuiltinWorkflowDefinition> {
    let mut defs = vec![
        BuiltinWorkflowDefinition {
            name: "code-review".to_owned(),
            manifest_bytes: CODE_REVIEW_MANIFEST.as_bytes().to_vec(),
            source_bytes: CODE_REVIEW_SOURCE.as_bytes().to_vec(),
        },
        BuiltinWorkflowDefinition {
            name: "deep-research".to_owned(),
            manifest_bytes: DEEP_RESEARCH_MANIFEST.as_bytes().to_vec(),
            source_bytes: DEEP_RESEARCH_SOURCE.as_bytes().to_vec(),
        },
        BuiltinWorkflowDefinition {
            name: "large-refactor".to_owned(),
            manifest_bytes: LARGE_REFACTOR_MANIFEST.as_bytes().to_vec(),
            source_bytes: LARGE_REFACTOR_SOURCE.as_bytes().to_vec(),
        },
    ];
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// Look up one built-in by exact registry name.
#[must_use]
pub fn builtin_workflow_definition(name: &str) -> Option<BuiltinWorkflowDefinition> {
    builtin_workflow_definitions()
        .into_iter()
        .find(|def| def.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::definition::{resolve_paired_definition, source_sha256_hex};
    use crate::workflow::limits::WorkflowLimits;
    use crate::workflow::state::WorkflowSourceOrigin;

    #[test]
    fn builtin_pairs_match_embedded_source_hashes() {
        let limits = WorkflowLimits::default();
        for def in builtin_workflow_definitions() {
            let expected = source_sha256_hex(&def.source_bytes);
            assert!(
                std::str::from_utf8(&def.manifest_bytes)
                    .expect("utf8")
                    .contains(&expected),
                "manifest for {} must embed source_sha256 {expected}",
                def.name
            );
            resolve_paired_definition(
                &def.name,
                &def.manifest_bytes,
                &def.source_bytes,
                WorkflowSourceOrigin::Builtin,
                Some(format!("builtin://{}", def.name)),
                &limits,
            )
            .unwrap_or_else(|err| panic!("resolve {}: {err}", def.name));
        }
    }
}
