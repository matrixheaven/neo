use neo_agent_core::instructions::{
    InstructionBundleMetadata, InstructionEpochData, InstructionEpochOutcome, InstructionScopeData,
    InstructionScopeKind,
};
use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
    PermissionOperation,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Finalization, strip_ansi};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane, TranscriptStore};

fn approved_resolution() -> ApprovalResolution {
    ApprovalResolution::Selected {
        action: ApprovalAction::PermitOnce,
        label: "Approved".to_owned(),
        feedback: None,
    }
}
fn request_test_approval(pane: &mut TranscriptPane) {
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_test_request("approval-1", "printf 1"),
    });
}
fn plain_rows(store: &TranscriptStore) -> Vec<String> {
    store
        .render_rows(80, &TuiTheme::default())
        .into_iter()
        .map(|row| strip_ansi(&row.to_ansi()).trim_end().to_owned())
        .collect()
}
fn plain_slice(pane: &mut TranscriptPane) -> Vec<String> {
    pane.render_visible_slice(80, 20)
        .into_iter()
        .map(|line| strip_ansi(&line).trim_end().to_owned())
        .collect()
}
fn instruction_test_epoch(generation: u64, deferred_tool_ids: &[&str]) -> InstructionEpochData {
    let nested = std::path::PathBuf::from("/workspace/neo/crates/neo-tui");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation,
        outcome: InstructionEpochOutcome::Activated,
        scopes: vec![InstructionScopeData {
            display_path: nested.clone(),
            kind: InstructionScopeKind::Nested,
            revision: Some("7af13c2e".to_owned()),
            token_estimate: 31_800,
        }],
        selected_bundles: vec![InstructionBundleMetadata {
            display_path: nested,
            revision: "7af13c2e".to_owned(),
            token_estimate: 31_800,
            byte_size: 127_200,
            source_count: 3,
            import_count: 2,
            import_paths: Vec::new(),
        }],
        ignored_bundles: Vec::new(),
        replacements: Vec::new(),
        failure: None,
        deferred_tool_ids: deferred_tool_ids
            .iter()
            .map(|id| (*id).to_owned())
            .collect(),
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: Some("scoped rules".to_owned()),
    }
}
fn instruction_order(store: &TranscriptStore) -> Vec<String> {
    store
        .entries()
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => {
                format!("card:{}", component.id())
            }
            TranscriptEntry::ToolRun { component } => format!("tool:{}", component.id()),
            _ => "other".to_owned(),
        })
        .collect()
}
fn shell_test_request(id: &str, command: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: command.to_owned(),
            cwd: None,
        },
        options: shell_test_options(),

        workflow_origin: None,
    }
}
fn shell_test_options() -> Vec<ApprovalOption> {
    vec![
        ApprovalOption {
            label: "Approve once".to_owned(),
            description: None,
            action: ApprovalAction::PermitOnce,
        },
        ApprovalOption {
            label: "Reject".to_owned(),
            description: None,
            action: ApprovalAction::Reject,
        },
    ]
}

#[test]
fn active_assistant_is_live_until_finish() {
    let mut store = TranscriptStore::new();
    store.start_assistant();

    assert_eq!(store.entry_finalization(0), Some(Finalization::Live));

    store.finish_assistant();

    assert_eq!(store.entry_finalization(0), Some(Finalization::Finalized));
}

#[test]
fn cached_frame_renderer_preserves_commentary_phase() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "commentary-1".to_owned(),
        phase: neo_ai::MessagePhase::Commentary,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Checking the cache".to_owned(),
    });

    let frame = pane
        .render_frame(80, 20)
        .expect("cached frame should render");
    let frame = frame
        .into_iter()
        .map(|line| strip_ansi(&line))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        frame.contains("▸ Checking the cache"),
        "cached frame uses Commentary marker: {frame}"
    );
    assert!(
        !frame.contains("● Checking"),
        "cached frame does not fall back to legacy marker: {frame}"
    );
}

#[test]
fn entry_ids_survive_in_place_updates_and_track_removal() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::status("first"));
    store.start_assistant();

    let ids = store.entry_ids().to_vec();
    let revisions = store.entry_revisions().to_vec();

    store.append_assistant_delta("answer");

    assert_eq!(store.entry_ids(), ids);
    assert_eq!(store.entry_revisions()[0], revisions[0]);
    assert!(store.entry_revisions()[1] > revisions[1]);

    store.remove(0);

    assert_eq!(store.entry_ids(), &ids[1..]);
    assert_eq!(store.entry_revisions().len(), 1);
}

#[test]
fn instruction_epoch_replaces_deferred_placeholders_at_earliest_position() {
    let mut store = TranscriptStore::new();
    store.push_tool_run("read-1", "Read", None);
    store.push_tool_run("grep-1", "Grep", None);
    store.push_tool_run("bash-1", "Bash", None);

    // Deferred ids arrive in provider batch order, not transcript order; the
    // card must still land at the earliest placeholder's canonical position.
    let epoch = instruction_test_epoch(3, &["bash-1", "read-1", "grep-1"]);
    let card_id = store.insert_instruction_epoch(
        &epoch,
        std::path::PathBuf::from("/workspace/neo"),
        Some(std::path::PathBuf::from("/home/user")),
        false,
    );

    assert!(matches!(
        store.entries().first(),
        Some(TranscriptEntry::InstructionEpoch { .. })
    ));
    assert_eq!(store.entry_ids().first(), Some(&card_id));
    for id in ["read-1", "grep-1", "bash-1"] {
        assert!(
            store.is_tool_run_suppressed(id),
            "deferred placeholder {id} must be absorbed"
        );
    }
    assert_eq!(
        store.entries().len(),
        4,
        "placeholders are suppressed, never deleted"
    );

    // The model replans and re-issues the batch under fresh ids; the retried
    // tools append after the fixed card instead of displacing it.
    store.push_tool_run("read-2", "Read", None);
    store.push_tool_run("grep-2", "Grep", None);
    store.push_tool_run("bash-2", "Bash", None);

    assert_eq!(
        instruction_order(&store),
        [
            "card:instruction-epoch-main-3",
            "tool:read-1",
            "tool:grep-1",
            "tool:bash-1",
            "tool:read-2",
            "tool:grep-2",
            "tool:bash-2",
        ]
    );
    for id in ["read-2", "grep-2", "bash-2"] {
        assert!(
            !store.is_tool_run_suppressed(id),
            "retried tool {id} must stay visible"
        );
    }
    assert_eq!(
        store.entry_finalization(0),
        Some(Finalization::Finalized),
        "the instruction card is a finalized semantic entry"
    );
}

#[test]
fn no_op_entry_mutation_keeps_revision_stable() {
    let mut store = TranscriptStore::new();
    store.push(TranscriptEntry::status("ready"));
    let revision = store.entry_revisions()[0];

    assert!(!store.mutate_entry(0, |_| false));
    assert_eq!(store.entry_revisions()[0], revision);
}

#[test]
fn resolved_approval_ignores_repeated_request() {
    let mut pane = TranscriptPane::new(80, 12);
    request_test_approval(&mut pane);
    pane.resolve_approval("approval-1", &approved_resolution());
    let revision = pane.transcript().entry_revisions()[0];

    request_test_approval(&mut pane);

    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
    assert_eq!(pane.transcript().entry_revisions()[0], revision);
}

#[test]
fn retry_status_countdown_formats_long_delay() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::RetryScheduled {
        turn: 1,
        retry: 1,
        max_retries: 5,
        delay_ms: 3_878_000,
        error_code: "provider.transport_error".to_owned(),
        message: "error decoding response body".to_owned(),
    });

    let rows = plain_rows(pane.transcript()).join("\n");
    assert!(
        rows.contains("Reconnecting 1/5 · retry in 1h 04m 38s · esc interrupt"),
        "long retry delay: {rows}"
    );
}

#[test]
fn streaming_assistant_uses_the_same_rows_after_finish() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.start_assistant();
    store.append_assistant_delta("working");
    let streaming = plain_rows(&store);

    store.finish_assistant();
    let complete = plain_rows(&store);

    assert_eq!(streaming, complete);
    assert!(
        complete
            .iter()
            .any(|row| row.contains("●") && row.contains("working"))
    );
}

#[test]
fn transcript_store_renders_entries_without_draining_them() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::banner("Welcome to neo"));
    store.push(TranscriptEntry::user_message("hello"));

    let first = plain_rows(&store);
    let second = plain_rows(&store);

    assert!(first.iter().any(|row| row.contains("Welcome to neo")));
    assert!(
        first
            .iter()
            .any(|row| row.contains("✨") && row.contains("hello"))
    );
    assert_eq!(first, second);
    assert_eq!(store.entries().len(), 2);
}

#[test]
fn transcript_store_uses_explicit_entry_names_and_tool_runs() {
    let mut store = TranscriptStore::new();

    store.push(TranscriptEntry::user_message("hello"));
    store.push(TranscriptEntry::assistant_message("world"));
    store.push(TranscriptEntry::status("ready"));
    store.push_tool_run("tool-1", "Bash", Some(r#"{"command":"pwd"}"#.to_owned()));

    assert!(matches!(
        store.entries()[0],
        TranscriptEntry::UserMessage { .. }
    ));
    assert!(matches!(
        store.entries()[1],
        TranscriptEntry::AssistantMessage { .. }
    ));
    assert!(matches!(store.entries()[2], TranscriptEntry::Status { .. }));
    assert!(matches!(
        store.entries()[3],
        TranscriptEntry::ToolRun { .. }
    ));
}

#[test]
fn unknown_message_phase_preserves_legacy_rendering() {
    let mut pane = TranscriptPane::new(80, 20);
    pane.apply_agent_event(neo_agent_core::AgentEvent::MessageStarted {
        turn: 1,
        id: "unknown-1".to_owned(),
        phase: neo_ai::MessagePhase::Unknown,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Legacy answer".to_owned(),
    });

    let live = plain_slice(&mut pane).join("\n");
    assert!(
        live.contains("● Legacy answer"),
        "legacy assistant rendering: {live}"
    );
    assert!(!live.contains("▸"), "Unknown is not commentary: {live}");
}
