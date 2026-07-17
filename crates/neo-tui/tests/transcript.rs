use neo_agent_core::instructions::{
    IgnoredInstructionBundle, InstructionBundleMetadata, InstructionEpochData,
    InstructionEpochOutcome, InstructionFailure, InstructionFailureKind, InstructionOmissionReason,
    InstructionReplacement, InstructionScopeData, InstructionScopeKind,
};
use neo_tui::primitive::theme::TuiTheme;
use neo_tui::primitive::{Color, Component, Expandable, Finalization};
use neo_tui::transcript::TranscriptPane;
use neo_tui::transcript::{InstructionCardComponent, TranscriptEntry};

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == 0x1b {
            index += 1;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if (0x40..=0x7e).contains(&byte) || byte == b'\x07' {
                    break;
                }
            }
            continue;
        }
        let Some(ch) = text[index..].chars().next() else {
            break;
        };
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

#[test]
fn canonical_snapshot_retains_full_history_after_terminal_commit() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    let status_lines = (0..12)
        .map(|index| format!("status line {index:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    pane.push_status(status_lines);

    let update = pane.render_terminal_update(80, 6);
    pane.acknowledge_history(&update.history);

    let canonical = pane.frame_ansi_lines().join("\n");
    let canonical = strip_ansi(&canonical);
    assert!(canonical.contains("status line 00"));
    assert!(canonical.contains("status line 11"));
}

#[test]
fn terminal_update_does_not_replay_committed_history() {
    let mut pane = TranscriptPane::new(80, 6);
    pane.set_live_chrome_height(0);
    pane.push_status("committed status");

    let first = pane.render_terminal_update(80, 6);
    let first_history = first
        .history
        .iter()
        .flat_map(|block| block.lines.iter())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(strip_ansi(&first_history).contains("committed status"));

    pane.acknowledge_history(&first.history);
    let second = pane.render_terminal_update(80, 6);
    assert!(second.history.is_empty());
    assert!(!strip_ansi(&second.live.join("\n")).contains("committed status"));
}

// ── Instruction epoch card (path-scoped AGENTS.md instructions) ─────────────

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

fn instruction_bundle(
    path: &std::path::Path,
    revision: &str,
    tokens: u64,
    sources: u32,
    imports: u32,
) -> InstructionBundleMetadata {
    InstructionBundleMetadata {
        display_path: path.to_path_buf(),
        revision: revision.to_owned(),
        token_estimate: tokens,
        byte_size: tokens * 4,
        source_count: sources,
        import_count: imports,
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
            instruction_bundle(&global_dir, "a1b2c3d4", 8_200, 1, 0),
            instruction_bundle(&workspace, "e5f60718", 17_400, 2, 1),
            instruction_bundle(&nested_dir, "7af13c2e", 31_800, 3, 2),
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
        model_content: Some(format!(
            "system rules {INSTRUCTION_SENTINEL} with absolute path /home/user/.neo/AGENTS.md"
        )),
    }
}

fn instruction_card(epoch: InstructionEpochData) -> InstructionCardComponent {
    InstructionCardComponent::new(epoch, instruction_workspace(), Some(instruction_home()))
}

fn rendered_text(lines: &[neo_tui::primitive::Line]) -> String {
    lines
        .iter()
        .map(neo_tui::primitive::Line::text)
        .collect::<Vec<_>>()
        .join("\n")
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
            "↻ Instructions updated · crates/neo-tui/**",
            theme.status_warn,
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
                display_path: instruction_workspace().join("crates/neo-tui"),
                previous_revision: "e5f60718".to_owned(),
                new_revision: "7af13c2e".to_owned(),
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
                assert!(text.contains("revision 7af13c2e"), "{text}");
            }
            InstructionEpochOutcome::PartiallyLoaded => {
                // needed 92K (selected 57.4K + ignored 34.6K), admitted 57.4K.
                assert!(
                    text.contains("92K of 57.4K tokens · 2 bundles ignored"),
                    "{text}"
                );
            }
            InstructionEpochOutcome::Blocked => {
                assert!(text.contains("Missing import: ~/.neo/CX.md"), "{text}");
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
fn expanded_instruction_card_lists_loaded_ignored_imports_and_redacted_paths() {
    let theme = TuiTheme::default();
    let epoch = base_instruction_epoch(InstructionEpochOutcome::PartiallyLoaded);
    let mut component = instruction_card(epoch);
    component.set_expanded(true);

    let lines = component.render_with_theme(100, &theme);
    let text = rendered_text(&lines);

    // Sections: scope, loaded, ignored, imports, revision.
    assert!(text.contains("Scope"), "{text}");
    assert!(text.contains("~/.neo/**"), "{text}");
    assert!(text.contains("\n  workspace\n"), "{text}");
    assert!(text.contains("crates/neo-tui/**"), "{text}");
    assert!(text.contains("Loaded"), "{text}");
    assert!(text.contains("~/.neo/AGENTS.md"), "{text}");
    assert!(text.contains("./AGENTS.md"), "{text}");
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
    assert!(text.contains("./AGENTS.md · 1 import"), "{text}");
    assert!(
        text.contains("crates/neo-tui/AGENTS.md · 2 imports"),
        "{text}"
    );
    assert!(text.contains("Revision"), "{text}");
    assert!(text.contains("a1b2c3d4"), "{text}");
    assert!(text.contains("e5f60718"), "{text}");
    assert!(text.contains("7af13c2e"), "{text}");

    // Paths are workspace-relative or ~/ relative: never absolute home or
    // workspace prefixes, and never the instruction body.
    assert!(!text.contains("/home/user"), "{text}");
    assert!(!text.contains("/workspace/neo"), "{text}");
    assert!(!text.contains(INSTRUCTION_SENTINEL), "{text}");

    let copied = component.copy_text();
    assert!(copied.contains("crates/neo-tui/AGENTS.md"), "{copied}");
    assert!(copied.contains("~/.neo/AGENTS.md"), "{copied}");
    assert!(!copied.contains("/home/user"), "{copied}");
    assert!(!copied.contains(INSTRUCTION_SENTINEL), "{copied}");
}
