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
    InstructionRegistryConfig, InstructionResolver, InstructionScopeKind, SourceIo, SourceMetadata,
    find_agents_file, select_agents_file_name,
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
