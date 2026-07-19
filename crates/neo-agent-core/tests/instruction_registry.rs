//! Integration tests for the pure path-scoped AGENTS.md instruction engine.
//!
//! Every fixture uses a fresh tempdir and canonicalizes paths before
//! comparison (macOS maps `/var` to `/private/var`). Tests never touch the
//! process environment or shared cwd.

use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use neo_agent_core::instructions::{
    AdmissionCandidate, AgentInstructionState, FilesystemSourceIo, InstructionAdmission,
    InstructionBudget, InstructionEpochData, InstructionEpochOutcome, InstructionFailureKind,
    InstructionFingerprint, InstructionOmissionReason, InstructionPreflightDecision,
    InstructionReconcileKind, InstructionReconcileRequest, InstructionRegistry,
    InstructionRegistryConfig, InstructionResolver, InstructionScopeKind, MAX_SOURCE_BYTES,
    SourceIo, SourceMetadata, find_agents_file, select_agents_file_name,
};

/// Creates a canonicalized tempdir root plus a `workspace` directory.
fn workspace_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().canonicalize().expect("canonical tempdir");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    (temp, workspace)
}

fn config_for(workspace: &Path, neo_home: Option<PathBuf>) -> InstructionRegistryConfig {
    InstructionRegistryConfig {
        primary_workspace: workspace.to_path_buf(),
        neo_home,
        project_trusted: true,
    }
}

/// Depth-6 import chain exceeding the maximum depth of 5.
fn write_depth_chain(workspace: &Path) {
    fs::write(workspace.join("AGENTS.md"), "@./i1.md\n").expect("depth root");
    for depth in 1..6 {
        let next = format!("@./i{}.md\n", depth + 1);
        fs::write(workspace.join(format!("i{depth}.md")), next).expect("chain link");
    }
    fs::write(workspace.join("i6.md"), "leaf\n").expect("chain leaf");
}

/// A 33-source import graph exceeding the 32-source maximum.
fn write_wide_graph(workspace: &Path) {
    let mut root = String::new();
    for index in 1..=32 {
        writeln!(root, "@./f{index:02}.md").expect("write");
        fs::write(workspace.join(format!("f{index:02}.md")), "x\n").expect("source");
    }
    fs::write(workspace.join("AGENTS.md"), root).expect("wide root");
}

#[test]
fn resolver_merges_target_chains_general_to_specific_without_siblings() {
    let (_temp, workspace) = workspace_fixture();
    let ui_src = workspace.join("crates/ui/src");
    fs::create_dir_all(&ui_src).expect("ui src dir");
    fs::create_dir_all(workspace.join("docs")).expect("docs dir");
    fs::write(workspace.join("AGENTS.md"), "root rules\n").expect("root agents");
    fs::write(workspace.join("crates/AGENTS.md"), "crates rules\n").expect("crates agents");
    fs::write(workspace.join("crates/ui/AGENTS.md"), "ui rules\n").expect("ui agents");
    fs::write(workspace.join("docs/AGENTS.md"), "docs rules\n").expect("docs agents");
    fs::write(ui_src.join("lib.rs"), "pub fn probe() {}\n").expect("probe file");

    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let scopes = resolver
        .discover_scopes(std::slice::from_ref(&ui_src))
        .expect("discover scopes");

    let crates = workspace.join("crates");
    let ui = crates.join("ui");
    assert_eq!(scopes.workspace_root.as_deref(), Some(workspace.as_path()));
    assert_eq!(scopes.nested, vec![crates.clone(), ui.clone()]);

    let rendering: Vec<PathBuf> = scopes
        .rendering_order()
        .into_iter()
        .map(|(_, dir)| dir)
        .collect();
    assert_eq!(
        rendering,
        vec![workspace.clone(), crates, ui],
        "rendering stays general-to-specific"
    );

    let docs = workspace.join("docs");
    assert!(
        !scopes.all_scope_dirs().contains(&docs),
        "sibling docs scope must never appear"
    );
}

#[test]
fn resolver_expands_only_standalone_imports_outside_fences_in_place() {
    let (_temp, workspace) = workspace_fixture();
    let home = workspace.parent().expect("root").join("home");
    let neo_home = home.join(".neo");
    fs::create_dir_all(&neo_home).expect("neo home");
    fs::write(neo_home.join("CX.md"), "GLOBAL CX\n").expect("cx file");
    fs::write(workspace.join("rules & regs.md"), "RULE BODY\n").expect("rules file");
    let agents = "\
# Rules

@./rules & regs.md
@~/.neo/CX.md
@@./x.md
See @docs/rules.md inline.
```markdown
@./fenced.md
```
@https://example.com/rules.md
@$HOME/secret.md
";
    fs::write(workspace.join("AGENTS.md"), agents).expect("agents file");

    let resolver =
        InstructionResolver::with_home(&config_for(&workspace, Some(neo_home)), home.clone())
            .expect("resolver");
    let bundle = resolver
        .load_bundle(&workspace)
        .expect("load bundle")
        .expect("bundle present");
    let expanded = &bundle.expanded;

    let rules_display = workspace
        .join("rules & regs.md")
        .display()
        .to_string()
        .replace('&', "&amp;");
    let rules_wrapper = format!(
        "<included_instructions path=\"{rules_display}\">\nRULE BODY\n</included_instructions>"
    );
    assert!(expanded.contains(&rules_wrapper), "expanded:\n{expanded}");

    let cx_wrapper =
        "<included_instructions path=\"~/.neo/CX.md\">\nGLOBAL CX\n</included_instructions>";
    assert!(expanded.contains(cx_wrapper), "expanded:\n{expanded}");

    for literal in [
        "@@./x.md",
        "See @docs/rules.md inline.",
        "@./fenced.md",
        "@https://example.com/rules.md",
        "@$HOME/secret.md",
    ] {
        assert!(
            expanded.contains(literal),
            "literal form must stay byte-identical: {literal}\nexpanded:\n{expanded}"
        );
    }
    assert_eq!(
        expanded.matches("<included_instructions").count(),
        2,
        "only the two standalone directives expand:\n{expanded}"
    );
}

#[test]
fn resolver_expands_local_markdown_links_but_not_images_code_or_urls() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("CX.md"), "CX RULES\n").expect("cx file");
    for name in ["image.md", "inline-code.md", "fenced.md"] {
        fs::write(workspace.join(name), format!("{name} MUST NOT LOAD\n")).expect("fixture");
    }
    let agents = r"Read [CX.md](./CX.md) before acting.
![diagram](./image.md)
`[inline](./inline-code.md)`
```markdown
[fenced](./fenced.md)
```
[web](https://example.com/rules.md) [section](#local)
";
    fs::write(workspace.join("AGENTS.md"), agents).expect("agents file");

    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let bundle = resolver
        .load_bundle(&workspace)
        .expect("load bundle")
        .expect("bundle present");
    let expanded = &bundle.expanded;

    assert!(expanded.contains("Read [CX.md](./CX.md)"), "{expanded}");
    assert!(expanded.contains("CX RULES"), "{expanded}");
    for sentinel in [
        "image.md MUST NOT LOAD",
        "inline-code.md MUST NOT LOAD",
        "fenced.md MUST NOT LOAD",
    ] {
        assert!(!expanded.contains(sentinel), "{expanded}");
    }
    assert_eq!(
        expanded.matches("<included_instructions").count(),
        1,
        "{expanded}"
    );
}

#[test]
fn resolver_rejects_casefold_collision_and_canonical_escape() {
    // Case-insensitive filesystems (default macOS/Windows) cannot hold two
    // case-folded variants in one directory, so the collision path is tested
    // through the pure entry classifier.
    let variants = vec![OsString::from("AGENTS.md"), OsString::from("agents.MD")];
    let err = select_agents_file_name(Path::new("dir"), &variants)
        .expect_err("two case-folded variants are ambiguous");
    assert_eq!(
        err.failure_kind(),
        InstructionFailureKind::AmbiguousAgentsFile
    );

    let one = select_agents_file_name(
        Path::new("dir"),
        &[OsString::from("notes.txt"), OsString::from("Agents.md")],
    )
    .expect("single variant");
    assert_eq!(one.as_deref(), Some(OsStr::new("Agents.md")));

    let none = select_agents_file_name(Path::new("dir"), &[OsString::from("notes.txt")])
        .expect("no variant");
    assert!(none.is_none());

    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "root\n").expect("agents file");
    let found = find_agents_file(&workspace)
        .expect("find")
        .expect("present");
    assert_eq!(found.file_name(), Some(OsStr::new("AGENTS.md")));

    // A `..` import that canonicalizes outside both roots is untrusted.
    let outside = workspace.parent().expect("root").join("outside.md");
    fs::write(&outside, "SECRET\n").expect("outside file");
    fs::write(workspace.join("AGENTS.md"), "@../outside.md\n").expect("agents file");
    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let err = resolver
        .load_bundle(&workspace)
        .expect_err("escape must be rejected");
    assert_eq!(err.failure_kind(), InstructionFailureKind::UntrustedImport);
}

#[tokio::test]
async fn epoch_metadata_preserves_import_paths_in_expansion_order() {
    let (_temp, workspace) = workspace_fixture();
    fs::create_dir_all(workspace.join("docs")).expect("docs dir");
    fs::write(
        workspace.join("AGENTS.md"),
        "@./first.md\n@./docs/second.md\n",
    )
    .expect("agents file");
    fs::write(workspace.join("first.md"), "FIRST\n").expect("first import");
    fs::write(workspace.join("docs/second.md"), "SECOND\n").expect("second import");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");

    let (epoch, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &AgentInstructionState::default(),
            )
            .await,
    );

    assert_eq!(
        epoch.selected_bundles[0].import_paths,
        [workspace.join("first.md"), workspace.join("docs/second.md")]
    );
}

#[cfg(unix)]
#[test]
fn resolver_rejects_root_agents_symlink_outside_allowed_roots() {
    use std::os::unix::fs::symlink;

    let (_temp, workspace) = workspace_fixture();
    let outside = workspace.parent().expect("root").join("outside.md");
    fs::write(&outside, "EXTERNAL RULES\n").expect("outside file");
    symlink(&outside, workspace.join("AGENTS.md")).expect("root agents symlink");

    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let err = resolver
        .load_bundle(&workspace)
        .expect_err("root AGENTS.md must not canonicalize outside allowed roots");

    assert_eq!(err.failure_kind(), InstructionFailureKind::UntrustedImport);
}

#[cfg(unix)]
#[tokio::test]
async fn cached_bundle_invalidates_when_root_agents_symlink_is_retargeted() {
    use std::os::unix::fs::symlink;

    let (_temp, workspace) = workspace_fixture();
    let first = workspace.join("first.md");
    let second = workspace.join("second.md");
    let agents = workspace.join("AGENTS.md");
    fs::write(&first, "FIRST ROOT RULES\n").expect("first rules");
    fs::write(&second, "SECOND ROOT RULES\n").expect("second rules");
    symlink(&first, &agents).expect("initial root symlink");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (initial, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    state.apply_epoch(&initial, &fingerprint);
    fs::remove_file(&agents).expect("remove first symlink");
    symlink(&second, &agents).expect("retarget root symlink");

    let (updated, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::ToolPreflight, vec![workspace]),
                &state,
            )
            .await,
    );

    assert_eq!(updated.outcome, InstructionEpochOutcome::Updated);
    let content = updated.model_content.as_deref().expect("updated authority");
    assert!(content.contains("SECOND ROOT RULES"), "{content}");
    assert!(!content.contains("FIRST ROOT RULES"), "{content}");
}

#[cfg(unix)]
#[tokio::test]
async fn cached_bundle_invalidates_when_import_symlink_is_retargeted() {
    use std::os::unix::fs::symlink;

    let (_temp, workspace) = workspace_fixture();
    let first = workspace.join("first.md");
    let second = workspace.join("second.md");
    let active = workspace.join("active.md");
    fs::write(workspace.join("AGENTS.md"), "@./active.md\n").expect("agents file");
    fs::write(&first, "FIRST IMPORT RULES\n").expect("first rules");
    fs::write(&second, "SECOND IMPORT RULES\n").expect("second rules");
    symlink(&first, &active).expect("initial import symlink");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (initial, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    state.apply_epoch(&initial, &fingerprint);
    fs::remove_file(&active).expect("remove first symlink");
    symlink(&second, &active).expect("retarget import symlink");

    let (updated, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::ToolPreflight, vec![workspace]),
                &state,
            )
            .await,
    );

    assert_eq!(updated.outcome, InstructionEpochOutcome::Updated);
    let content = updated.model_content.as_deref().expect("updated authority");
    assert!(content.contains("SECOND IMPORT RULES"), "{content}");
    assert!(!content.contains("FIRST IMPORT RULES"), "{content}");
}

fn admission_candidate(
    kind: InstructionScopeKind,
    scope_dir: &str,
    token_estimate: u64,
) -> AdmissionCandidate {
    AdmissionCandidate {
        kind,
        scope_dir: PathBuf::from(scope_dir),
        metadata: neo_agent_core::instructions::InstructionBundleMetadata {
            display_path: PathBuf::from(scope_dir),
            revision: format!("rev-{scope_dir}"),
            token_estimate,
            byte_size: 0,
            source_count: 1,
            import_count: 0,
            import_paths: Vec::new(),
        },
        content: format!("content-{scope_dir}"),
    }
}

#[test]
fn admission_uses_dynamic_cap_and_keeps_atomic_bundles_in_priority_order() {
    assert_eq!(
        InstructionBudget::from_context(Some(1_048_576), 200_000).nominal,
        131_072
    );
    assert_eq!(
        InstructionBudget::from_context(Some(131_072), 40_000).actual,
        40_000
    );

    let ancestor = admission_candidate(InstructionScopeKind::Ancestor, "a", 100);
    let root = admission_candidate(InstructionScopeKind::WorkspaceRoot, "a/ws", 100);
    let shallow = admission_candidate(InstructionScopeKind::Nested, "a/ws/crates", 100);
    let deep = admission_candidate(InstructionScopeKind::Nested, "a/ws/crates/ui", 100);
    let global = admission_candidate(InstructionScopeKind::Global, "a/home/.neo", 100);
    let scrambled = || {
        vec![
            ancestor.clone(),
            deep.clone(),
            global.clone(),
            shallow.clone(),
            root.clone(),
        ]
    };

    // Full admission follows global -> root -> deepest nested -> shallow
    // nested -> nearest ancestor.
    let budget = InstructionBudget {
        nominal: 65_536,
        actual: 500,
    };
    let full = InstructionAdmission::select(scrambled(), budget);
    let admitted: Vec<&Path> = full
        .admitted
        .iter()
        .map(|c| c.scope_dir.as_path())
        .collect();
    assert_eq!(
        admitted,
        [
            Path::new("a/home/.neo"),
            Path::new("a/ws"),
            Path::new("a/ws/crates/ui"),
            Path::new("a/ws/crates"),
            Path::new("a"),
        ]
    );
    assert!(full.ignored.is_empty());

    // A tight budget keeps atomic bundles in priority order and ignores the
    // remainder as whole units.
    let tight_budget = InstructionBudget {
        nominal: 65_536,
        actual: 300,
    };
    let tight = InstructionAdmission::select(scrambled(), tight_budget);
    let admitted: Vec<&Path> = tight
        .admitted
        .iter()
        .map(|c| c.scope_dir.as_path())
        .collect();
    assert_eq!(
        admitted,
        [
            Path::new("a/home/.neo"),
            Path::new("a/ws"),
            Path::new("a/ws/crates/ui"),
        ]
    );
    let ignored: Vec<(&Path, InstructionOmissionReason)> = tight
        .ignored
        .iter()
        .map(|i| (i.display_path.as_path(), i.reason))
        .collect();
    assert_eq!(
        ignored,
        [
            (
                Path::new("a/ws/crates"),
                InstructionOmissionReason::OverBudget
            ),
            (Path::new("a"), InstructionOmissionReason::OverBudget),
        ]
    );

    // Model rendering is global -> outer ancestors -> root -> shallowest
    // nested -> deepest.
    let rendering: Vec<PathBuf> = InstructionAdmission::rendering_order(full.admitted)
        .into_iter()
        .map(|c| c.scope_dir)
        .collect();
    assert_eq!(
        rendering,
        [
            PathBuf::from("a/home/.neo"),
            PathBuf::from("a"),
            PathBuf::from("a/ws"),
            PathBuf::from("a/ws/crates"),
            PathBuf::from("a/ws/crates/ui"),
        ]
    );
}

fn reconcile_request(
    kind: InstructionReconcileKind,
    target_directories: Vec<PathBuf>,
) -> InstructionReconcileRequest {
    InstructionReconcileRequest {
        agent_id: "main".to_owned(),
        kind,
        target_directories,
        budget: InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        deferred_tool_ids: vec!["call-1".to_owned()],
    }
}

fn expect_defer(
    decision: InstructionPreflightDecision,
) -> (InstructionEpochData, InstructionFingerprint) {
    match decision {
        InstructionPreflightDecision::Defer { epoch, fingerprint } => (epoch, fingerprint),
        InstructionPreflightDecision::Proceed { .. } => panic!("expected Defer, got Proceed"),
        InstructionPreflightDecision::Block { epoch, .. } => {
            panic!("expected Defer, got Block: {:?}", epoch.failure)
        }
    }
}

fn expect_proceed(decision: InstructionPreflightDecision, state: &mut AgentInstructionState) {
    match decision {
        InstructionPreflightDecision::Proceed { fingerprint } => {
            state.last_epoch_fingerprint = Some(fingerprint.hash);
        }
        InstructionPreflightDecision::Defer { epoch, .. } => {
            panic!("expected Proceed, got Defer: {:?}", epoch.outcome)
        }
        InstructionPreflightDecision::Block { epoch, .. } => {
            panic!("expected Proceed, got Block: {:?}", epoch.failure)
        }
    }
}

fn expect_block(
    decision: InstructionPreflightDecision,
) -> (InstructionEpochData, InstructionFingerprint) {
    match decision {
        InstructionPreflightDecision::Block { epoch, fingerprint } => (epoch, fingerprint),
        other => panic!("expected Block, got {}", decision_name(&other)),
    }
}

#[tokio::test]
async fn identical_content_and_failure_fingerprints_do_not_create_new_epochs() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "V1\n").expect("v1");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    // First activation defers with the initial revision.
    let (epoch, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Activated);
    let revision_v1 = epoch.selected_bundles[0].revision.clone();
    state.apply_epoch(&epoch, &fingerprint);

    // An mtime-only rewrite (identical bytes) returns Proceed.
    fs::write(workspace.join("AGENTS.md"), "V1\n").expect("v1 again");
    expect_proceed(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
        &mut state,
    );

    // Changed bytes create an Updated epoch with a replacement revision.
    fs::write(workspace.join("AGENTS.md"), "V2\n").expect("v2");
    let (epoch, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Updated);
    assert_eq!(epoch.replacements.len(), 1);
    assert_eq!(epoch.replacements[0].previous_revision, revision_v1);
    assert_ne!(epoch.replacements[0].new_revision, revision_v1);
    let revision_v2 = epoch.replacements[0].new_revision.clone();
    state.apply_epoch(&epoch, &fingerprint);

    // A missing import blocks the bundle with a typed failure.
    fs::write(workspace.join("AGENTS.md"), "V3\n@./missing.md\n").expect("v3");
    let (epoch, fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Blocked);
    assert_eq!(
        epoch.failure.as_ref().map(|f| f.kind),
        Some(InstructionFailureKind::MissingImport)
    );
    state.apply_epoch(&epoch, &fingerprint);

    // The same source + failure kind + fingerprint does not re-epoch.
    expect_proceed(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
        &mut state,
    );

    // A fixed source replaces the last visible revision and defers again.
    fs::write(workspace.join("AGENTS.md"), "V4\n").expect("v4");
    let (epoch, _fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Updated);
    assert_eq!(epoch.replacements[0].previous_revision, revision_v2);

    // Distinct blocked states of one kind must never collapse into Proceed.
    same_kind_blocked_states_with_different_details_reblock(&registry, &workspace, &mut state)
        .await;
}

#[tokio::test]
async fn same_active_chain_ignores_capacity_already_consumed_by_its_authority() {
    let (_temp, workspace) = workspace_fixture();
    let nested = workspace.join("nested");
    fs::create_dir_all(&nested).expect("nested dir");
    fs::write(workspace.join("AGENTS.md"), "ROOT\n").expect("root rules");
    fs::write(nested.join("AGENTS.md"), "NESTED\n").expect("nested rules");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (epoch, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![nested.clone()],
                ),
                &state,
            )
            .await,
    );
    assert!(epoch.ignored_bundles.is_empty());
    state.apply_epoch(&epoch, &fingerprint);

    let request = InstructionReconcileRequest {
        budget: InstructionBudget {
            nominal: 65_536,
            actual: 0,
        },
        ..reconcile_request(InstructionReconcileKind::ToolPreflight, vec![nested])
    };
    expect_proceed(registry.reconcile(request, &state).await, &mut state);
}

#[tokio::test]
async fn changed_invalid_utf8_bytes_emit_new_blocked_epoch() {
    let (_temp, workspace) = workspace_fixture();
    let source = workspace.join("AGENTS.md");
    fs::write(&source, [0xFF, b'A']).expect("invalid bytes A");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (first_epoch, first_fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    let first_failure = first_epoch.failure.as_ref().expect("first failure");
    assert_eq!(first_failure.kind, InstructionFailureKind::InvalidEncoding);
    let first_failure_fingerprint = first_failure.fingerprint.clone();
    let first_detail = first_failure.detail.clone();
    state.apply_epoch(&first_epoch, &first_fingerprint);

    fs::write(&source, [0xFF, b'B']).expect("invalid bytes B");
    let (second_epoch, second_fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    let second_failure = second_epoch.failure.as_ref().expect("second failure");
    assert_eq!(second_failure.kind, InstructionFailureKind::InvalidEncoding);
    assert_ne!(second_failure.fingerprint, first_failure_fingerprint);
    assert_ne!(second_fingerprint.hash, first_fingerprint.hash);
    assert_eq!(second_failure.detail, first_detail);
    assert_eq!(
        second_failure.detail,
        format!("source `{}` is not valid UTF-8", source.display())
    );
    state.apply_epoch(&second_epoch, &second_fingerprint);

    expect_proceed(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::ToolPreflight, vec![workspace]),
                &state,
            )
            .await,
        &mut state,
    );
}

#[tokio::test]
async fn removed_epoch_revokes_prior_instruction_authority() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "OLD AUTHORITY\n").expect("agents file");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (activated, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    state.apply_epoch(&activated, &fingerprint);
    fs::remove_file(workspace.join("AGENTS.md")).expect("remove agents file");

    let (removed, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &state,
            )
            .await,
    );

    assert_eq!(removed.outcome, InstructionEpochOutcome::Removed);
    let authority = removed
        .model_content
        .as_deref()
        .expect("removal must carry model-visible authority");
    assert!(authority.contains("complete current path-scoped instruction snapshot"));
    assert!(authority.contains("No path-scoped instruction bundles are currently active"));
    assert!(!authority.contains("OLD AUTHORITY"));
}

#[tokio::test]
async fn replayed_epoch_fingerprint_prevents_unchanged_duplicate() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "stable rules\n").expect("agents file");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");

    let (epoch, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.clone()],
                ),
                &AgentInstructionState::default(),
            )
            .await,
    );
    let mut replayed = AgentInstructionState::default();
    replayed.apply_epoch_visibility(&epoch);

    assert!(replayed.last_epoch_fingerprint.is_some());
    assert!(matches!(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::ToolPreflight, vec![workspace],),
                &replayed,
            )
            .await,
        InstructionPreflightDecision::Proceed { .. }
    ));
}

#[tokio::test]
async fn zero_instruction_budget_does_not_inject_omission_notice() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "rules\n").expect("agents file");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let request = InstructionReconcileRequest {
        budget: InstructionBudget {
            nominal: 65_536,
            actual: 0,
        },
        ..reconcile_request(InstructionReconcileKind::ToolPreflight, vec![workspace])
    };

    let (epoch, _) = expect_defer(
        registry
            .reconcile(request, &AgentInstructionState::default())
            .await,
    );

    assert_eq!(epoch.outcome, InstructionEpochOutcome::PartiallyLoaded);
    assert_eq!(epoch.selected_bundles.len(), 0);
    assert_eq!(epoch.ignored_bundles.len(), 1);
    assert_eq!(epoch.model_content, None);
}

#[tokio::test]
async fn rendered_cost_admission_can_skip_unfittable_high_priority_bundle() {
    let (_temp, workspace) = workspace_fixture();
    let nested = workspace.join("nested");
    fs::create_dir_all(&nested).expect("nested dir");
    fs::write(
        workspace.join("AGENTS.md"),
        format!("HIGH-PRIORITY {}\n", "x".repeat(360)),
    )
    .expect("root agents");
    fs::write(nested.join("AGENTS.md"), "LOW-FITS\n").expect("nested agents");
    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let root_body_tokens = resolver
        .load_bundle(&workspace)
        .expect("root bundle")
        .expect("root present")
        .token_estimate;
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let request = InstructionReconcileRequest {
        budget: InstructionBudget {
            nominal: 65_536,
            actual: root_body_tokens,
        },
        ..reconcile_request(
            InstructionReconcileKind::ToolPreflight,
            vec![nested.clone()],
        )
    };

    let (epoch, _) = expect_defer(
        registry
            .reconcile(request, &AgentInstructionState::default())
            .await,
    );

    assert_eq!(epoch.outcome, InstructionEpochOutcome::PartiallyLoaded);
    // Rendered-cost admission may demote all bundles if the omission notice
    // pushes the total over budget.
    assert_eq!(epoch.selected_bundles.len(), 0);
    assert_eq!(epoch.ignored_bundles.len(), 2);
}

#[tokio::test]
async fn omission_notice_demotes_admitted_bundles_until_the_notice_fits() {
    let (_temp, workspace) = workspace_fixture();
    let nested = workspace.join("nested");
    fs::create_dir_all(&nested).expect("nested dir");
    fs::write(workspace.join("AGENTS.md"), "ROOT-FITS\n").expect("root agents");
    fs::write(
        nested.join("AGENTS.md"),
        format!("NESTED-IGNORED {}\n", "x".repeat(2_000)),
    )
    .expect("nested agents");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut observed_partial_notice = false;

    for actual in 1..=512 {
        let request = InstructionReconcileRequest {
            budget: InstructionBudget {
                nominal: 65_536,
                actual,
            },
            ..reconcile_request(
                InstructionReconcileKind::ToolPreflight,
                vec![nested.clone()],
            )
        };
        let (epoch, _) = expect_defer(
            registry
                .reconcile(request, &AgentInstructionState::default())
                .await,
        );
        if !epoch.selected_bundles.is_empty() && !epoch.ignored_bundles.is_empty() {
            let content = epoch.model_content.as_deref().unwrap_or_else(|| {
                panic!(
                    "selected authority without its omission notice at budget {actual}: {epoch:#?}"
                )
            });
            assert!(
                content.contains("Ignored instruction bundles:"),
                "{content}"
            );
            observed_partial_notice = true;
            break;
        }
    }

    assert!(
        observed_partial_notice,
        "fixture must reach a whole-bundle partial selection with a visible notice"
    );
}

/// Two different blocked states of the SAME kind (`LimitExceeded` carries no
/// single failure path, so only display-safe detail distinguishes them) must
/// produce different fingerprints and a fresh Block, while a truly identical
/// failure still returns Proceed.
async fn same_kind_blocked_states_with_different_details_reblock(
    registry: &InstructionRegistry,
    workspace: &Path,
    state: &mut AgentInstructionState,
) {
    // First blocked state: an import-depth violation naming `./i6.md`.
    write_depth_chain(workspace);
    let (epoch, fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.to_path_buf()],
                ),
                state,
            )
            .await,
    );
    assert_eq!(
        epoch.failure.as_ref().map(|f| f.kind),
        Some(InstructionFailureKind::LimitExceeded)
    );
    // The DTO and the model-visible notice name the failing source even
    // though the limit failure has no single display path.
    let failure = epoch.failure.as_ref().expect("failure");
    assert!(
        failure.detail.contains("import depth exceeds 5") && failure.detail.contains("i6.md"),
        "detail: {}",
        failure.detail
    );
    let notice = epoch.model_content.as_deref().unwrap_or_default();
    assert!(
        notice.contains("import depth exceeds 5") && notice.contains("i6.md"),
        "notice: {notice}"
    );
    let depth_fingerprint = fingerprint.hash.clone();
    state.apply_epoch(&epoch, &fingerprint);

    // The truly identical blocked state still returns Proceed.
    expect_proceed(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.to_path_buf()],
                ),
                state,
            )
            .await,
        state,
    );

    // A different violation of the same kind (graph > 32 sources) is a new
    // blocked state: different fingerprint, fresh Block — never Proceed.
    write_wide_graph(workspace);
    let (epoch, fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.to_path_buf()],
                ),
                state,
            )
            .await,
    );
    assert_eq!(
        epoch.failure.as_ref().map(|f| f.kind),
        Some(InstructionFailureKind::LimitExceeded)
    );
    assert!(
        epoch
            .failure
            .as_ref()
            .expect("failure")
            .detail
            .contains("exceeds 32 sources")
    );
    assert_ne!(
        fingerprint.hash, depth_fingerprint,
        "distinct blocked states must never share a fingerprint"
    );
    state.apply_epoch(&epoch, &fingerprint);

    // And that new blocked state, repeated identically, returns Proceed.
    expect_proceed(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![workspace.to_path_buf()],
                ),
                state,
            )
            .await,
        state,
    );
}

fn decision_name(decision: &InstructionPreflightDecision) -> &'static str {
    match decision {
        InstructionPreflightDecision::Proceed { .. } => "Proceed",
        InstructionPreflightDecision::Defer { .. } => "Defer",
        InstructionPreflightDecision::Block { .. } => "Block",
    }
}

#[tokio::test]
async fn missing_results_are_not_cached_across_reconcile_calls() {
    let (_temp, workspace) = workspace_fixture();
    let ui_src = workspace.join("crates/ui/src");
    fs::create_dir_all(&ui_src).expect("ui src");
    fs::write(workspace.join("AGENTS.md"), "root\n").expect("root agents");

    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    // Baseline activates the workspace root scope.
    let (epoch, fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::Baseline, vec![ui_src.clone()]),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Ready);
    state.apply_epoch(&epoch, &fingerprint);

    // No nested AGENTS.md exists yet: identical selection -> Proceed.
    expect_proceed(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![ui_src.clone()],
                ),
                &state,
            )
            .await,
        &mut state,
    );

    // A newly created nested AGENTS.md is discovered on the very next call.
    fs::write(workspace.join("crates/ui/AGENTS.md"), "ui rules\n").expect("ui agents");
    let (epoch, _fingerprint) = expect_defer(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![ui_src.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(epoch.outcome, InstructionEpochOutcome::Activated);
    let ui = workspace.join("crates/ui");
    assert!(
        epoch.scopes.iter().any(|scope| scope.display_path == ui),
        "scopes: {:?}",
        epoch.scopes
    );
}

/// Source I/O that denies every byte read (portable unreadable source).
#[derive(Debug)]
struct DenyReadIo;

impl SourceIo for DenyReadIo {
    fn read_metadata(&self, path: &Path) -> io::Result<SourceMetadata> {
        FilesystemSourceIo.read_metadata(path)
    }

    fn read_bytes(&self, _path: &Path) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied by test",
        ))
    }
}

/// Source I/O whose metadata changes on every call (portable race).
#[derive(Debug, Default)]
struct UnstableIo {
    tick: AtomicU64,
}

impl SourceIo for UnstableIo {
    fn read_metadata(&self, path: &Path) -> io::Result<SourceMetadata> {
        let mut metadata = FilesystemSourceIo.read_metadata(path)?;
        metadata.modified = Some(
            SystemTime::UNIX_EPOCH + Duration::from_secs(self.tick.fetch_add(1, Ordering::SeqCst)),
        );
        Ok(metadata)
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        FilesystemSourceIo.read_bytes(path)
    }
}

#[derive(Debug)]
struct RecordingBoundedIo {
    reported_len: u64,
    full_reads: AtomicU64,
    bounded_reads: AtomicU64,
    requested_limit: AtomicU64,
}

impl RecordingBoundedIo {
    fn new(reported_len: u64) -> Self {
        Self {
            reported_len,
            full_reads: AtomicU64::new(0),
            bounded_reads: AtomicU64::new(0),
            requested_limit: AtomicU64::new(0),
        }
    }
}

impl SourceIo for RecordingBoundedIo {
    fn read_metadata(&self, _path: &Path) -> io::Result<SourceMetadata> {
        Ok(SourceMetadata {
            len: self.reported_len,
            modified: Some(SystemTime::UNIX_EPOCH),
            is_file: true,
        })
    }

    fn read_bytes(&self, _path: &Path) -> io::Result<Vec<u8>> {
        self.full_reads.fetch_add(1, Ordering::SeqCst);
        Ok(b"unbounded read must not run".to_vec())
    }

    fn read_bytes_bounded(&self, _path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
        self.bounded_reads.fetch_add(1, Ordering::SeqCst);
        self.requested_limit.store(
            u64::try_from(max_bytes).unwrap_or(u64::MAX),
            Ordering::SeqCst,
        );
        Ok(vec![b'x'; max_bytes])
    }
}

#[test]
fn resolver_rejects_oversized_metadata_before_reading_source() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "small fixture\n").expect("agents file");
    let source_io = Arc::new(RecordingBoundedIo::new(MAX_SOURCE_BYTES + 1));
    let resolver =
        InstructionResolver::with_source_io(&config_for(&workspace, None), None, source_io.clone())
            .expect("resolver");

    let err = resolver
        .load_bundle(&workspace)
        .expect_err("oversized metadata must block before reading bytes");

    assert_eq!(err.failure_kind(), InstructionFailureKind::LimitExceeded);
    assert_eq!(source_io.full_reads.load(Ordering::SeqCst), 0);
    assert_eq!(source_io.bounded_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn resolver_bounds_source_reads_at_limit_plus_one() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "small fixture\n").expect("agents file");
    let source_io = Arc::new(RecordingBoundedIo::new(MAX_SOURCE_BYTES));
    let resolver =
        InstructionResolver::with_source_io(&config_for(&workspace, None), None, source_io.clone())
            .expect("resolver");

    let err = resolver
        .load_bundle(&workspace)
        .expect_err("limit-plus-one sentinel must block oversized source");

    assert_eq!(err.failure_kind(), InstructionFailureKind::LimitExceeded);
    assert_eq!(source_io.full_reads.load(Ordering::SeqCst), 0);
    assert_eq!(source_io.bounded_reads.load(Ordering::SeqCst), 1);
    assert_eq!(
        source_io.requested_limit.load(Ordering::SeqCst),
        MAX_SOURCE_BYTES + 1,
    );
}

struct BundleCase {
    _temp: tempfile::TempDir,
    workspace: PathBuf,
}

impl BundleCase {
    fn new() -> Self {
        let (temp, workspace) = workspace_fixture();
        Self {
            _temp: temp,
            workspace,
        }
    }

    fn write(&self, relative: &str, content: &[u8]) {
        let path = self.workspace.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dirs");
        }
        fs::write(path, content).expect("write fixture");
    }

    fn expect_failure(&self, kind: InstructionFailureKind) {
        let resolver =
            InstructionResolver::new(&config_for(&self.workspace, None)).expect("resolver");
        let err = resolver
            .load_bundle(&self.workspace)
            .expect_err("bundle must be blocked");
        assert_eq!(err.failure_kind(), kind, "error: {err}");
    }
}

#[tokio::test]
async fn resolver_reports_every_atomic_structural_and_integrity_failure() {
    // Missing import; the readable sibling subset must not be injected.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./good.md\n@./nope.md\n");
    case.write("good.md", b"GOOD BODY\n");
    case.expect_failure(InstructionFailureKind::MissingImport);
    let registry = InstructionRegistry::new(config_for(&case.workspace, None)).expect("registry");
    let state = AgentInstructionState::default();
    let (epoch, _fingerprint) = expect_block(
        registry
            .reconcile(
                reconcile_request(
                    InstructionReconcileKind::ToolPreflight,
                    vec![case.workspace.clone()],
                ),
                &state,
            )
            .await,
    );
    assert_eq!(
        epoch.failure.as_ref().map(|f| f.kind),
        Some(InstructionFailureKind::MissingImport)
    );
    assert!(epoch.selected_bundles.is_empty());
    let notice = epoch.model_content.as_deref().unwrap_or_default();
    assert!(
        !notice.contains("GOOD BODY"),
        "readable subset must never leak into the notice: {notice}"
    );

    // Unreadable source (portable permission failure via scripted I/O).
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./denied.md\n");
    case.write("denied.md", b"DENIED\n");
    let resolver = InstructionResolver::with_source_io(
        &config_for(&case.workspace, None),
        None,
        Arc::new(DenyReadIo),
    )
    .expect("resolver");
    let err = resolver
        .load_bundle(&case.workspace)
        .expect_err("denied read must block the bundle");
    assert_eq!(err.failure_kind(), InstructionFailureKind::UnreadableSource);

    // Invalid UTF-8.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./bad.md\n");
    case.write("bad.md", &[0x66, 0xFF, 0xFE, 0x61]);
    case.expect_failure(InstructionFailureKind::InvalidEncoding);

    // Special import: a directory is not a readable Markdown source.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./subdir\n");
    fs::create_dir_all(case.workspace.join("subdir")).expect("subdir");
    case.expect_failure(InstructionFailureKind::UnreadableSource);

    // Include cycle.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./a.md\n");
    case.write("a.md", b"@./b.md\n");
    case.write("b.md", b"@./a.md\n");
    case.expect_failure(InstructionFailureKind::IncludeCycle);

    // Import depth 6 (maximum is 5).
    let case = BundleCase::new();
    write_depth_chain(&case.workspace);
    case.expect_failure(InstructionFailureKind::LimitExceeded);

    // Source 33 in one graph (maximum is 32).
    let case = BundleCase::new();
    write_wide_graph(&case.workspace);
    case.expect_failure(InstructionFailureKind::LimitExceeded);

    // One source larger than 1 MiB.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"@./big.md\n");
    case.write("big.md", &vec![b'a'; 1_048_577]);
    case.expect_failure(InstructionFailureKind::LimitExceeded);

    // Complete graph larger than 8 MiB (each source individually legal).
    let case = BundleCase::new();
    let mut root = String::new();
    for index in 1..=9 {
        writeln!(root, "@./g{index}.md").expect("write");
        case.write(&format!("g{index}.md"), &vec![b'b'; 1_048_476]);
    }
    case.write("AGENTS.md", root.as_bytes());
    case.expect_failure(InstructionFailureKind::LimitExceeded);

    // Untrusted import leaving both roots.
    let case = BundleCase::new();
    let outside = case.workspace.parent().expect("root").join("outside.md");
    fs::write(&outside, b"SECRET\n").expect("outside");
    case.write("AGENTS.md", b"@../outside.md\n");
    case.expect_failure(InstructionFailureKind::UntrustedImport);

    // Ambiguous case-folded AGENTS.md variants in one directory.
    let err = select_agents_file_name(
        Path::new("dir"),
        &[OsString::from("AGENTS.md"), OsString::from("agents.MD")],
    )
    .expect_err("collision");
    assert_eq!(
        err.failure_kind(),
        InstructionFailureKind::AmbiguousAgentsFile
    );

    // Twice-changing unstable source.
    let case = BundleCase::new();
    case.write("AGENTS.md", b"racing\n");
    let resolver = InstructionResolver::with_source_io(
        &config_for(&case.workspace, None),
        None,
        Arc::new(UnstableIo::default()),
    )
    .expect("resolver");
    let err = resolver
        .load_bundle(&case.workspace)
        .expect_err("unstable source must block the bundle");
    assert_eq!(err.failure_kind(), InstructionFailureKind::UnstableSource);
}
