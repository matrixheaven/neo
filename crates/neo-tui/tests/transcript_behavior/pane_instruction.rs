use neo_agent_core::instructions::{
    IgnoredInstructionBundle, InstructionBundleMetadata, InstructionEpochData,
    InstructionEpochOutcome, InstructionFailure, InstructionFailureKind, InstructionOmissionReason,
    InstructionReplacement, InstructionScopeData, InstructionScopeKind,
};
use neo_tui::primitive::strip_ansi;
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Finalization};
use neo_tui::transcript::TranscriptPane;
use neo_tui::transcript::{InstructionCardComponent, TranscriptEntry};

const INSTRUCTION_SENTINEL: &str = "INSTRUCTION-BODY-SECRET-SENTINEL";

fn instruction_workspace() -> std::path::PathBuf {
    std::path::PathBuf::from("/workspace/neo")
}
fn instruction_home() -> std::path::PathBuf {
    std::path::PathBuf::from("/home/user")
}
fn instruction_scope(path: &std::path::Path, kind: InstructionScopeKind) -> InstructionScopeData {
    InstructionScopeData {
        display_path: path.to_path_buf(),
        kind,
        revision: Some("7af13c2e".to_owned()),
        token_estimate: 31_800,
    }
}
fn base_instruction_epoch(outcome: InstructionEpochOutcome) -> InstructionEpochData {
    let workspace = instruction_workspace();
    let home = instruction_home();
    let global_dir = home.join(".neo");
    let nested_dir = workspace.join("crates/neo-tui");
    InstructionEpochData {
        agent_id: "main".to_owned(),
        generation: 3,
        outcome,
        scopes: vec![
            instruction_scope(&global_dir, InstructionScopeKind::Global),
            instruction_scope(&workspace, InstructionScopeKind::WorkspaceRoot),
            instruction_scope(&nested_dir, InstructionScopeKind::Nested),
        ],
        selected_bundles: vec![
            instruction_bundle(&global_dir, "a1b2c3d4", 8_200, 1, Vec::new()),
            instruction_bundle(
                &workspace,
                "e5f60718",
                17_400,
                2,
                vec![workspace.join("docs/testing.md")],
            ),
            instruction_bundle(
                &nested_dir,
                "7af13c2e",
                31_800,
                3,
                vec![global_dir.join("CX.md"), nested_dir.join("docs/testing.md")],
            ),
        ],
        ignored_bundles: vec![
            IgnoredInstructionBundle {
                display_path: workspace.join("crates"),
                revision: "99001122".to_owned(),
                token_estimate: 22_100,
                reason: InstructionOmissionReason::OverBudget,
            },
            IgnoredInstructionBundle {
                display_path: nested_dir.join("src"),
                revision: "33445566".to_owned(),
                token_estimate: 12_500,
                reason: InstructionOmissionReason::OverBudget,
            },
        ],
        replacements: vec![],
        failure: None,
        deferred_tool_ids: vec!["tool-1".to_owned()],
        budget: neo_agent_core::instructions::InstructionBudget {
            nominal: 65_536,
            actual: 65_536,
        },
        body_revisions: None,
        model_content: Some(format!(
            "system rules {INSTRUCTION_SENTINEL} with absolute path /home/user/.neo/AGENTS.md"
        )),
    }
}
fn instruction_card(epoch: InstructionEpochData) -> InstructionCardComponent {
    InstructionCardComponent::new(
        epoch,
        instruction_workspace(),
        Some(instruction_home().join(".neo")),
    )
}
fn rendered_text(lines: &[neo_tui::primitive::Line]) -> String {
    lines
        .iter()
        .map(neo_tui::primitive::Line::text)
        .collect::<Vec<_>>()
        .join("\n")
}
fn instruction_bundle(
    path: &std::path::Path,
    revision: &str,
    tokens: u64,
    sources: u32,
    import_paths: Vec<std::path::PathBuf>,
) -> InstructionBundleMetadata {
    let import_count = u32::try_from(import_paths.len()).unwrap_or(u32::MAX);
    InstructionBundleMetadata {
        display_path: path.to_path_buf(),
        revision: revision.to_owned(),
        token_estimate: tokens,
        byte_size: tokens * 4,
        source_count: sources,
        import_count,
        import_paths,
    }
}
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
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
fn instruction_order(pane: &TranscriptPane) -> Vec<String> {
    pane.transcript()
        .entries()
        .iter()
        .map(|entry| match entry {
            TranscriptEntry::InstructionEpoch { component } => {
                format!("card:{}", component.id())
            }
            TranscriptEntry::ToolRun { component } => format!("tool:{}", component.id()),
            TranscriptEntry::AssistantMessage { .. } => "assistant".to_owned(),
            _ => "other".to_owned(),
        })
        .collect()
}
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}

#[test]
fn expanded_instruction_card_lists_loaded_ignored_imports_and_redacted_paths() {
    let theme = TuiTheme::default();
    let epoch = base_instruction_epoch(InstructionEpochOutcome::PartiallyLoaded);
    let mut component = instruction_card(epoch);
    component.set_expanded(true);

    let lines = component.render_with_theme(100, &theme);
    let text = rendered_text(&lines);

    // Sections: scope, loaded, ignored, imports.
    assert!(text.contains("Scope"), "{text}");
    assert!(text.contains("$NEO_HOME/**"), "{text}");
    assert!(text.contains("\n  workspace\n"), "{text}");
    assert!(text.contains("crates/neo-tui/**"), "{text}");
    assert!(text.contains("Loaded"), "{text}");
    assert!(text.contains("$NEO_HOME/AGENTS.md"), "{text}");
    assert!(text.contains("\n  AGENTS.md"), "{text}");
    assert!(!text.contains("./AGENTS.md"), "{text}");
    assert!(text.contains("crates/neo-tui/AGENTS.md"), "{text}");
    assert!(text.contains("8.2K"), "{text}");
    assert!(text.contains("17.4K"), "{text}");
    assert!(text.contains("31.8K"), "{text}");
    assert!(text.contains("Ignored"), "{text}");
    assert!(text.contains("crates/AGENTS.md"), "{text}");
    assert!(text.contains("22.1K"), "{text}");
    assert!(text.contains("budget exceeded"), "{text}");
    assert!(text.contains("crates/neo-tui/src/AGENTS.md"), "{text}");
    assert!(text.contains("12.5K"), "{text}");
    assert!(text.contains("Imports"), "{text}");
    assert!(text.contains("docs/testing.md"), "{text}");
    assert!(text.contains("$NEO_HOME/CX.md"), "{text}");
    assert!(text.contains("crates/neo-tui/docs/testing.md"), "{text}");
    assert!(!text.contains("AGENTS.md · 1 import"), "{text}");
    assert!(!text.contains("Revision"), "{text}");
    assert!(!text.contains("a1b2c3d4"), "{text}");
    assert!(!text.contains("e5f60718"), "{text}");
    assert!(!text.contains("7af13c2e"), "{text}");

    // Paths are workspace-relative or ~/ relative: never absolute home or
    // workspace prefixes, and never the instruction body.
    assert!(!text.contains("/home/user"), "{text}");
    assert!(!text.contains("/workspace/neo"), "{text}");
    assert!(!text.contains(INSTRUCTION_SENTINEL), "{text}");

    let copied = component.copy_text();
    assert!(copied.contains("crates/neo-tui/AGENTS.md"), "{copied}");
    assert!(copied.contains("$NEO_HOME/AGENTS.md"), "{copied}");
    assert!(!copied.contains("/home/user"), "{copied}");
    assert!(!copied.contains(INSTRUCTION_SENTINEL), "{copied}");
}

#[test]
fn instruction_card_does_not_retain_model_content() {
    let component = instruction_card(base_instruction_epoch(InstructionEpochOutcome::Activated));

    assert!(!format!("{component:?}").contains(INSTRUCTION_SENTINEL));
}

#[test]
fn instruction_card_never_renders_unknown_absolute_paths_or_failure_detail() {
    let secret_path = std::path::PathBuf::from("/private/secret/instructions.md");
    let detail_sentinel = "FREE-FORM-FAILURE-DETAIL-SECRET";
    let mut epoch = base_instruction_epoch(InstructionEpochOutcome::Blocked);
    epoch.scopes.clear();
    epoch.selected_bundles.clear();

    for display_path in [secret_path.clone(), std::path::PathBuf::new()] {
        epoch.failure = Some(InstructionFailure {
            display_path,
            kind: InstructionFailureKind::MissingImport,
            fingerprint: "fp".to_owned(),
            detail: format!("missing {} {detail_sentinel}", secret_path.display()),
        });
        let component = instruction_card(epoch.clone());
        let text = rendered_text(&component.render_with_theme(100, &TuiTheme::default()));
        let debug = format!("{component:?}");

        assert!(!text.contains(&secret_path.display().to_string()), "{text}");
        assert!(!text.contains(detail_sentinel), "{text}");
        assert!(!debug.contains(detail_sentinel), "{debug}");
        assert!(text.contains("Missing import"), "{text}");
    }
}

#[cfg(unix)]
#[test]
fn instruction_card_redacts_canonical_paths_under_symlinked_neo_home() {
    let temp = tempfile::tempdir().expect("tempdir");
    let neo_home = temp.path().join("neo-home");
    let neo_home_link = temp.path().join("neo-home-link");
    std::fs::create_dir_all(&neo_home).expect("neo home");
    std::os::unix::fs::symlink(&neo_home, &neo_home_link).expect("neo home symlink");
    let neo_home_canon = neo_home_link
        .canonicalize()
        .expect("canonicalize neo home symlink");
    let mut epoch = base_instruction_epoch(InstructionEpochOutcome::Activated);
    epoch
        .scopes
        .last_mut()
        .unwrap()
        .display_path
        .clone_from(&neo_home_canon);
    epoch
        .selected_bundles
        .last_mut()
        .unwrap()
        .display_path
        .clone_from(&neo_home_canon);
    let component =
        InstructionCardComponent::new(epoch, instruction_workspace(), Some(neo_home_canon.clone()));
    let text = rendered_text(&component.render_with_theme(100, &TuiTheme::default()));

    assert!(text.contains("$NEO_HOME/**"), "{text}");
    assert!(text.contains("AGENTS.md"), "{text}");
    assert!(!text.contains("<outside-workspace>"), "{text}");
    assert!(
        !text.contains(&neo_home_canon.display().to_string()),
        "{text}"
    );
}

#[test]
fn instruction_card_redacts_custom_neo_home() {
    let custom_neo_home = std::path::PathBuf::from("/custom/neo-home");
    let mut epoch = base_instruction_epoch(InstructionEpochOutcome::Activated);
    epoch.scopes[0].display_path.clone_from(&custom_neo_home);
    epoch.selected_bundles[0]
        .display_path
        .clone_from(&custom_neo_home);
    let mut component = InstructionCardComponent::new(
        epoch,
        instruction_workspace(),
        Some(custom_neo_home.clone()),
    );
    component.set_expanded(true);

    let text = rendered_text(&component.render_with_theme(100, &TuiTheme::default()));
    assert!(text.contains("$NEO_HOME/AGENTS.md"), "{text}");
    assert!(
        !text.contains(&custom_neo_home.display().to_string()),
        "{text}"
    );
}

#[test]
fn instruction_card_renders_outcome_metadata_without_model_content() {
    let theme = TuiTheme::default();

    let cases: [(InstructionEpochOutcome, &str, Color); 7] = [
        (
            InstructionEpochOutcome::Ready,
            "◆ Instructions ready · crates/neo-tui/**",
            theme.brand,
        ),
        (
            InstructionEpochOutcome::Activated,
            "◆ Instructions loaded · crates/neo-tui/**",
            theme.brand,
        ),
        (
            InstructionEpochOutcome::Reactivated,
            "◆ Instructions reactivated · crates/neo-tui/**",
            theme.brand,
        ),
        (
            InstructionEpochOutcome::Updated,
            "↻ User instructions reloaded · $NEO_HOME/AGENTS.md",
            theme.brand,
        ),
        (
            InstructionEpochOutcome::PartiallyLoaded,
            "⚠ Instructions partially loaded · crates/neo-tui/**",
            theme.status_warn,
        ),
        (
            InstructionEpochOutcome::Blocked,
            "✕ Instructions blocked · crates/neo-tui/**",
            theme.status_error,
        ),
        (
            InstructionEpochOutcome::Removed,
            "− Instructions removed · crates/neo-tui/**",
            theme.text_muted,
        ),
    ];

    for (outcome, expected_header, expected_color) in cases {
        let mut epoch = base_instruction_epoch(outcome);
        if outcome == InstructionEpochOutcome::Updated {
            epoch.replacements = vec![InstructionReplacement {
                display_path: instruction_home().join(".neo"),
                previous_revision: "e5f60718".to_owned(),
                new_revision: "f44fdb8312b288ed4f70c4489efcf8416b4f9d31b380f62b44945bc3101e7c47"
                    .to_owned(),
            }];
        }
        if outcome == InstructionEpochOutcome::Blocked {
            epoch.failure = Some(InstructionFailure {
                display_path: instruction_home().join(".neo/CX.md"),
                kind: InstructionFailureKind::MissingImport,
                fingerprint: "fp".to_owned(),
                detail: format!("import `/home/user/.neo/CX.md` not found {INSTRUCTION_SENTINEL}"),
            });
        }
        let component = instruction_card(epoch);
        assert_eq!(component.id(), "instruction-epoch-main-3");

        // The card is a finalized semantic entry, not a live spinner.
        assert_eq!(component.finalization(), Finalization::Finalized);
        let entry = TranscriptEntry::InstructionEpoch {
            component: component.clone(),
        };
        assert_eq!(entry.finalization(), Finalization::Finalized);
        assert!(!entry.has_visible_animation());
        assert!(entry.is_expandable());

        let lines = component.render_with_theme(100, &theme);
        let text = rendered_text(&lines);

        // Exact compact label and outcome styling.
        assert_eq!(lines[0].text(), expected_header, "outcome {outcome:?}");
        assert_eq!(
            lines[0].spans()[0].style().fg,
            Some(expected_color),
            "outcome {outcome:?}"
        );

        // Secret instruction body never renders.
        assert!(
            !text.contains(INSTRUCTION_SENTINEL),
            "outcome {outcome:?} leaked model content: {text}"
        );

        match outcome {
            InstructionEpochOutcome::Ready => {
                // 1+2+3 sources, 0+1+2 imports, 8.2K+17.4K+31.8K tokens.
                assert!(
                    text.contains("6 sources · 3 imports · 57.4K tokens"),
                    "{text}"
                );
            }
            InstructionEpochOutcome::Activated => {
                assert!(
                    text.contains("AGENTS.md · 2 imports · 31.8K tokens"),
                    "{text}"
                );
            }
            InstructionEpochOutcome::Updated => {
                assert!(text.contains("Applied to current session"), "{text}");
                assert!(!text.contains("f44fdb83"), "{text}");
            }
            InstructionEpochOutcome::PartiallyLoaded => {
                // Needed 92K against the 64K effective instruction budget.
                assert!(
                    text.contains("92K of 64K tokens · 2 bundles ignored"),
                    "{text}"
                );
            }
            InstructionEpochOutcome::Blocked => {
                assert!(text.contains("Missing import: $NEO_HOME/CX.md"), "{text}");
                assert!(!text.contains("/home/user"), "{text}");
            }
            InstructionEpochOutcome::Reactivated | InstructionEpochOutcome::Removed => {}
        }

        // Copy text is built from metadata only.
        let copied = component.copy_text();
        assert!(
            !copied.contains(INSTRUCTION_SENTINEL),
            "outcome {outcome:?} copied model content: {copied}"
        );
        assert!(!copied.contains("/home/user"), "{copied}");

        // Expansion via the entry route (Ctrl+O path).
        let mut entry = TranscriptEntry::InstructionEpoch { component };
        assert!(entry.set_expanded(true));
        assert!(!entry.set_expanded(true));
        assert!(entry.set_expanded(false));
    }
}

#[test]
fn instruction_card_with_unset_roots_never_exposes_absolute_paths() {
    let mut epoch = base_instruction_epoch(InstructionEpochOutcome::Activated);
    epoch.scopes = vec![instruction_scope(
        std::path::Path::new("/private/secret"),
        InstructionScopeKind::Nested,
    )];
    epoch.selected_bundles[0].display_path = "/private/secret".into();
    let component = InstructionCardComponent::new(epoch, std::path::PathBuf::new(), None);
    let text = rendered_text(&component.render_with_theme(100, &TuiTheme::default()));

    assert!(text.contains("<outside-workspace>"), "{text}");
    assert!(!text.contains("/private/secret"), "{text}");
}

#[test]
fn finalized_instruction_card_does_not_drift_after_later_updates() {
    let mut pane = TranscriptPane::new(80, 24);
    pane.set_workspace_root("/workspace/neo");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "read-1".to_owned(),
        name: "Read".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
        turn: 1,
        id: "grep-1".to_owned(),
        name: "Grep".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
        epoch: instruction_test_epoch(3, &["read-1", "grep-1"]),
    });
    assert!(matches!(
        pane.transcript().entries().first(),
        Some(TranscriptEntry::InstructionEpoch { .. })
    ));

    // Later turn activity: assistant text, the replanned tool batch, turn
    // completion, and a follow-up epoch with no deferred placeholders.
    pane.apply_agent_event(neo_agent_core::AgentEvent::TextDelta {
        turn: 1,
        text: "Working on it.".to_owned(),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        arguments: serde_json::json!({ "path": "README.md" }),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
        turn: 1,
        id: "read-2".to_owned(),
        name: "Read".to_owned(),
        result: neo_agent_core::ToolResult::ok("done"),

        workflow_origin: None,
        output_ref: None,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::TurnFinished {
        turn: 1,
        stop_reason: neo_agent_core::StopReason::EndTurn,
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
        epoch: instruction_test_epoch(4, &[]),
    });

    let entries = pane.transcript().entries();
    assert!(
        matches!(
            entries.first(),
            Some(TranscriptEntry::InstructionEpoch { component })
                if component.id() == "instruction-epoch-main-3"
        ),
        "the finalized card must not drift to the transcript bottom: {:?}",
        instruction_order(&pane)
    );
    assert!(
        matches!(
            entries.last(),
            Some(TranscriptEntry::InstructionEpoch { component })
                if component.id() == "instruction-epoch-main-4"
        ),
        "an epoch without placeholders appends after later activity: {:?}",
        instruction_order(&pane)
    );
    assert_eq!(
        pane.transcript().entry_finalization(0),
        Some(Finalization::Finalized)
    );
}

#[test]
fn instruction_epoch_uses_injected_neo_home_for_redaction() {
    let custom_neo_home = std::path::PathBuf::from("/custom/neo-home");
    let mut epoch = instruction_test_epoch(4, &[]);
    epoch.scopes[0].display_path.clone_from(&custom_neo_home);
    epoch.scopes[0].kind = InstructionScopeKind::Global;
    epoch.selected_bundles[0]
        .display_path
        .clone_from(&custom_neo_home);

    let mut pane = TranscriptPane::new(80, 24);
    pane.set_workspace_root("/workspace/neo");
    pane.set_neo_home(Some(custom_neo_home.clone()));
    pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch { epoch });

    let TranscriptEntry::InstructionEpoch { component } = &pane.transcript().entries()[0] else {
        panic!("expected instruction epoch card");
    };
    let text = component.copy_text();
    assert!(text.contains("$NEO_HOME/**"), "{text}");
    assert!(
        !text.contains(&custom_neo_home.display().to_string()),
        "{text}"
    );
}

#[test]
fn replayed_instruction_epoch_has_identical_order_and_no_duplicate_card() {
    const DEFERRED: [(&str, &str); 3] =
        [("read-1", "Read"), ("grep-1", "Grep"), ("bash-1", "Bash")];
    const RETRIED: [(&str, &str); 3] = [("read-2", "Read"), ("grep-2", "Grep"), ("bash-2", "Bash")];
    let deferred_ids = DEFERRED.map(|(id, _)| id);
    let epoch = instruction_test_epoch(3, &deferred_ids);

    let replay = |pane: &mut TranscriptPane| {
        for (id, name) in DEFERRED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolCallStarted {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
            });
        }
        // Deferred calls receive provider-valid non-error results without
        // executing. The runtime emits those results before the instruction
        // epoch, so already-finalized placeholders must still be absorbed.
        for (id, name) in DEFERRED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                result: neo_agent_core::ToolResult::ok("deferred by instruction epoch"),

                workflow_origin: None,
                output_ref: None,
            });
        }
        pane.apply_agent_event(neo_agent_core::AgentEvent::InstructionEpoch {
            epoch: epoch.clone(),
        });
        // The model replans and re-issues the batch under fresh ids.
        for (id, name) in RETRIED {
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                arguments: serde_json::json!({}),

                workflow_origin: None,
                output_ref: None,
            });
            pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionFinished {
                turn: 1,
                id: id.to_owned(),
                name: name.to_owned(),
                result: neo_agent_core::ToolResult::ok("done"),

                workflow_origin: None,
                output_ref: None,
            });
        }
    };

    let mut pane = TranscriptPane::new(80, 24);
    pane.set_workspace_root("/workspace/neo");
    replay(&mut pane);
    let first_order = instruction_order(&pane);

    replay(&mut pane);
    let second_order = instruction_order(&pane);

    let expected = vec![
        "card:instruction-epoch-main-3".to_owned(),
        "tool:read-1".to_owned(),
        "tool:grep-1".to_owned(),
        "tool:bash-1".to_owned(),
        "tool:read-2".to_owned(),
        "tool:grep-2".to_owned(),
        "tool:bash-2".to_owned(),
    ];
    assert_eq!(first_order, expected);
    assert_eq!(
        second_order, expected,
        "replay must reconstruct the same visible order via deferred_tool_ids"
    );
    for (id, _) in DEFERRED {
        assert!(
            pane.transcript().is_tool_run_suppressed(id),
            "deferred placeholder {id} stays absorbed after replay"
        );
    }
    for (id, _) in RETRIED {
        assert!(
            !pane.transcript().is_tool_run_suppressed(id),
            "retried tool {id} must stay visible"
        );
    }

    let frame = plain_frame(&mut pane, 80, 24);
    let card_rows = frame
        .iter()
        .filter(|line| line.contains("Instructions loaded"))
        .count();
    assert_eq!(
        card_rows, 1,
        "identical epochs never produce duplicate cards: {frame:?}"
    );
}
