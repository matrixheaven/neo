use super::instructions_admission::expect_block;
use super::instructions_admission::expect_defer;
use super::instructions_admission::expect_proceed;
use super::instructions_admission::reconcile_request;
use neo_agent_core::{
    AgentEvent,
    instructions::{
        AgentInstructionState, FilesystemSourceIo, InstructionBundleMetadata, InstructionEpochData,
        InstructionEpochOutcome, InstructionFailureKind, InstructionReconcileKind,
        InstructionRegistry, InstructionRegistryConfig, InstructionResolver, InstructionScopeData,
        InstructionScopeKind, MAX_SOURCE_BYTES, SourceIo, SourceMetadata, select_agents_file_name,
    },
    session::{JsonlSessionReader, JsonlSessionWriter, SessionEventPersistence},
};
use std::fmt::Write as _;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

pub(crate) fn instruction_epoch(
    generation: u64,
    revision: &str,
    model_content: Option<&str>,
) -> InstructionEpochData {
    let scope = std::path::PathBuf::from("/workspace");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation,
        outcome: InstructionEpochOutcome::Activated,
        scopes: vec![InstructionScopeData {
            display_path: scope.clone(),
            kind: InstructionScopeKind::WorkspaceRoot,
            revision: Some(revision.to_owned()),
            token_estimate: 12,
        }],
        selected_bundles: vec![InstructionBundleMetadata {
            display_path: scope,
            revision: revision.to_owned(),
            token_estimate: 12,
            byte_size: 64,
            source_count: 1,
            import_count: 0,
            import_paths: Vec::new(),
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: Vec::new(),
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: model_content.map(str::to_owned),
    }
}

#[tokio::test]
async fn instruction_epoch_persists_once_and_replays_model_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("session.jsonl");
    let epoch = instruction_epoch(1, "rev-1", Some("scoped rules body"));
    let event = AgentEvent::InstructionEpoch { epoch };

    // The epoch event is the single persisted source: the persistence layer
    // emits it exactly once and never synthesizes a MessageAppended copy.
    let mut persistence = SessionEventPersistence::default();
    let persisted = persistence.persisted_events(&event);
    assert_eq!(persisted, vec![event]);

    let mut writer = JsonlSessionWriter::create(&path)
        .await
        .expect("create session");
    for persisted_event in &persisted {
        writer.append(persisted_event).await.expect("append epoch");
    }
    writer.flush().await.expect("flush");

    let wire = std::fs::read_to_string(&path).expect("read wire");
    assert_eq!(
        wire.matches("\"InstructionEpoch\"").count(),
        1,
        "epoch persisted exactly once: {wire}"
    );
    assert!(
        !wire.contains("MessageAppended"),
        "no duplicate MessageAppended copy: {wire}"
    );

    let context = JsonlSessionReader::replay_context(&path)
        .await
        .expect("replay context");
    assert_eq!(context.instruction_state().visible_generation, 1);
    assert_eq!(
        context
            .instruction_state()
            .visible_revisions
            .get(std::path::Path::new("/workspace"))
            .map(String::as_str),
        Some("rev-1")
    );
    assert_eq!(context.messages().len(), 1);
    let message = context.messages().first().expect("instruction injection");
    assert!(message.is_injection_variant("instruction_epoch"));
    assert_eq!(message.text(), "scoped rules body");
}

pub(crate) fn workspace_fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().canonicalize().expect("canonical tempdir");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");
    (temp, workspace)
}

pub(crate) fn config_for(workspace: &Path, neo_home: Option<PathBuf>) -> InstructionRegistryConfig {
    InstructionRegistryConfig {
        primary_workspace: workspace.to_path_buf(),
        neo_home,
        project_trusted: true,
    }
}

pub(crate) fn write_depth_chain(workspace: &Path) {
    fs::write(workspace.join("AGENTS.md"), "@./i1.md\n").expect("depth root");
    for depth in 1..6 {
        let next = format!("@./i{}.md\n", depth + 1);
        fs::write(workspace.join(format!("i{depth}.md")), next).expect("chain link");
    }
    fs::write(workspace.join("i6.md"), "leaf\n").expect("chain leaf");
}

pub(crate) fn write_wide_graph(workspace: &Path) {
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
fn resolver_selects_only_exact_agents_name_and_rejects_canonical_escape() {
    assert_eq!(
        select_agents_file_name(&[
            OsString::from("notes.txt"),
            OsString::from("agents.md"),
            OsString::from("Agents.md"),
        ]),
        None,
    );
    assert_eq!(
        select_agents_file_name(&[
            OsString::from("agents.md"),
            OsString::from("AGENTS.md"),
            OsString::from("Agents.md"),
        ]),
        Some(OsString::from("AGENTS.md")),
    );

    let (_temp, workspace) = workspace_fixture();
    let docs = workspace.join("docs/customization");
    fs::create_dir_all(&docs).expect("docs directory");
    fs::write(docs.join("agents.md"), "[self](./agents.md)\n").expect("ordinary docs file");
    let resolver = InstructionResolver::new(&config_for(&workspace, None)).expect("resolver");
    let scopes = resolver.discover_scopes(&[docs]).expect("discover scopes");
    assert!(scopes.nested.is_empty());

    // A `..` import that canonicalizes outside both roots is untrusted.
    let outside = workspace.parent().expect("root").join("outside.md");
    fs::write(&outside, "SECRET\n").expect("outside file");
    fs::write(workspace.join("AGENTS.md"), "@../outside.md\n").expect("agents file");
    let err = resolver
        .load_bundle(&workspace)
        .expect_err("escape must be rejected");
    assert_eq!(err.failure_kind(), InstructionFailureKind::UntrustedImport);
}

#[tokio::test]
async fn depth_six_cycle_back_edge_expands_once_and_proceeds_after_visibility() {
    let (_temp, workspace) = workspace_fixture();
    fs::write(workspace.join("AGENTS.md"), "ROOT BODY\n[A1](./a1.md)\n").expect("agents file");
    fs::write(workspace.join("a1.md"), "A1 BODY\n[A2](./a2.md)\n").expect("a1 file");
    fs::write(workspace.join("a2.md"), "A2 BODY\n[A3](./a3.md)\n").expect("a2 file");
    fs::write(workspace.join("a3.md"), "A3 BODY\n[A4](./a4.md)\n").expect("a3 file");
    fs::write(workspace.join("a4.md"), "A4 BODY\n[A5](./a5.md)\n").expect("a4 file");
    fs::write(
        workspace.join("a5.md"),
        "A5 BODY\n[Root again](./AGENTS.md)\n",
    )
    .expect("a5 file");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

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
    assert!(epoch.failure.is_none());
    let content = epoch.model_content.as_deref().expect("model content");
    for body in [
        "ROOT BODY",
        "A1 BODY",
        "A2 BODY",
        "A3 BODY",
        "A4 BODY",
        "A5 BODY",
    ] {
        assert_eq!(content.matches(body).count(), 1, "{body}: {content}");
    }

    state.apply_epoch(&epoch, &fingerprint);
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
async fn identical_nested_scope_reuses_retained_body_but_keeps_scope_state() {
    let (_temp, workspace) = workspace_fixture();
    let nested = workspace.join(".head-check");
    fs::create_dir_all(&nested).expect("nested scope");
    fs::write(workspace.join("AGENTS.md"), "shared rules\n").expect("root agents");
    fs::write(nested.join("AGENTS.md"), "shared rules\n").expect("nested agents");
    let registry = InstructionRegistry::new(config_for(&workspace, None)).expect("registry");
    let mut state = AgentInstructionState::default();

    let (baseline, baseline_fingerprint) = expect_defer(
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
    assert_eq!(
        baseline
            .model_content
            .as_deref()
            .expect("baseline model content")
            .matches("<instruction_revision")
            .count(),
        1
    );
    state.apply_epoch(&baseline, &baseline_fingerprint);

    let (nested_epoch, _) = expect_defer(
        registry
            .reconcile(
                reconcile_request(InstructionReconcileKind::ToolPreflight, vec![nested]),
                &state,
            )
            .await,
    );
    let nested_content = nested_epoch
        .model_content
        .as_deref()
        .expect("nested model content");
    assert_eq!(
        nested_content.matches("<instruction_revision").count(),
        0,
        "the shared body must not be appended again: {nested_content}"
    );
    assert_eq!(
        nested_content.matches("<active_instruction").count(),
        2,
        "both scope identities remain active: {nested_content}"
    );
    assert_eq!(nested_epoch.body_revisions, Some(BTreeMap::new()));
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

#[derive(Debug)]
pub(crate) struct DenyReadIo;

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

#[derive(Debug, Default)]
pub(crate) struct UnstableIo {
    pub(crate) tick: AtomicU64,
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
pub(crate) struct RecordingBoundedIo {
    pub(crate) reported_len: u64,
    pub(crate) full_reads: AtomicU64,
    pub(crate) bounded_reads: AtomicU64,
    pub(crate) requested_limit: AtomicU64,
}

impl RecordingBoundedIo {
    pub(crate) fn new(reported_len: u64) -> Self {
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

pub(crate) struct BundleCase {
    pub(crate) _temp: tempfile::TempDir,
    pub(crate) workspace: PathBuf,
}

impl BundleCase {
    pub(crate) fn new() -> Self {
        let (temp, workspace) = workspace_fixture();
        Self {
            _temp: temp,
            workspace,
        }
    }

    pub(crate) fn write(&self, relative: &str, content: &[u8]) {
        let path = self.workspace.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent dirs");
        }
        fs::write(path, content).expect("write fixture");
    }

    pub(crate) fn expect_failure(&self, kind: InstructionFailureKind) {
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
