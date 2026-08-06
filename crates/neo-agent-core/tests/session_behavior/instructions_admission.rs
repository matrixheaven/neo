use super::instructions::config_for;
use super::instructions::workspace_fixture;
use super::instructions::write_depth_chain;
use super::instructions::write_wide_graph;
use neo_agent_core::instructions::{
    AdmissionCandidate, AgentInstructionState, InstructionAdmission, InstructionBudget,
    InstructionEpochData, InstructionEpochOutcome, InstructionFailureKind, InstructionFingerprint,
    InstructionOmissionReason, InstructionPreflightDecision, InstructionReconcileKind,
    InstructionReconcileRequest, InstructionRegistry, InstructionResolver, InstructionScopeKind,
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn admission_candidate(
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

pub(crate) fn reconcile_request(
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

pub(crate) fn expect_defer(
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

pub(crate) fn expect_proceed(
    decision: InstructionPreflightDecision,
    state: &mut AgentInstructionState,
) {
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

pub(crate) fn expect_block(
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
async fn removed_epoch_deactivates_without_resending_body_on_revisit() {
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

    let (removed, removed_fingerprint) = expect_defer(
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
    let update = removed
        .model_content
        .as_deref()
        .expect("removal must carry model-visible active state");
    assert!(update.contains("<instruction_active_state"));
    assert!(!update.contains("<active_instruction"));
    assert!(!update.contains("OLD AUTHORITY"));
    state.apply_epoch(&removed, &removed_fingerprint);

    fs::write(workspace.join("AGENTS.md"), "OLD AUTHORITY\n").expect("restore agents file");
    let (revisited, _) = expect_defer(
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
    let revisit_update = revisited.model_content.as_deref().expect("revisit update");
    assert!(revisit_update.contains("<active_instruction"));
    assert!(!revisit_update.contains("<instruction_revision"));
    assert!(!revisit_update.contains("OLD AUTHORITY"));
    assert_eq!(revisited.body_revisions, Some(BTreeMap::new()));
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

pub(crate) async fn same_kind_blocked_states_with_different_details_reblock(
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

pub(crate) fn decision_name(decision: &InstructionPreflightDecision) -> &'static str {
    match decision {
        InstructionPreflightDecision::Proceed { .. } => "Proceed",
        InstructionPreflightDecision::Defer { .. } => "Defer",
        InstructionPreflightDecision::Block { .. } => "Block",
    }
}
