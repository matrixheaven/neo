use neo_agent_core::{
    ApprovalAction, ApprovalOption, ApprovalPresentation, ApprovalRequest, ApprovalResolution,
    EditApprovalChange, EditApprovalPresentation, PermissionOperation, WriteApprovalChange,
    WriteApprovalPresentation, WriteApprovalPreview,
};
use neo_tui::primitive::{strip_ansi, visible_width};
use neo_tui::transcript::{TranscriptEntry, TranscriptPane};
use std::path::PathBuf;

fn shell_options() -> Vec<ApprovalOption> {
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
fn shell_request(
    id: &str,
    command: &str,
    cwd: Option<&str>,
    options: Vec<ApprovalOption>,
) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::Shell,
        presentation: ApprovalPresentation::Command {
            title: "Run this command?".to_owned(),
            command: command.to_owned(),
            cwd: cwd.map(PathBuf::from),
        },
        options,

        workflow_origin: None,
    }
}
fn approved_resolution() -> ApprovalResolution {
    ApprovalResolution::Selected {
        action: ApprovalAction::PermitOnce,
        label: "Approved".to_owned(),
        feedback: None,
    }
}
fn edit_request(id: &str, prefix: &str, files: usize) -> ApprovalRequest {
    let changes = (0..files)
        .map(|index| EditApprovalChange {
            path: PathBuf::from(format!("src/{prefix}_{index}.rs")),
            replacements: 1,
            added: 1,
            removed: 1,
            diff: format!(
                "--- src/{prefix}_{index}.rs\n+++ src/{prefix}_{index}.rs\n@@ -12 +12 @@\n-old{index}\n+new{index}\n"
            ),
        })
        .collect();
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::FileWrite,
        presentation: ApprovalPresentation::Edit {
            title: format!("Edit {files} files?"),
            edit: EditApprovalPresentation {
                files,
                replacements: files,
                added: files,
                removed: files,
                changes,
            },
        },
        options: shell_options(),

        workflow_origin: None,
    }
}
fn workflow_request(id: &str) -> ApprovalRequest {
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::WorkflowLaunch,
        presentation: ApprovalPresentation::Workflow {
            title: "Launch workflow?".to_owned(),
            workflow: neo_agent_core::WorkflowApprovalPresentation {
                name: "reviewed".to_owned(),
                description: "A reviewed workflow".to_owned(),
                phases: vec!["work: Do the work".to_owned()],
                args: "{}".to_owned(),
                line_count: 2,
                byte_count: 27,
                source: "neo.phase('work')\nreturn {}".to_owned(),
                warning: "Launch approval authorizes orchestration only.".to_owned(),
            },
        },
        options: vec![ApprovalOption {
            label: "Launch".to_owned(),
            description: None,
            action: ApprovalAction::LaunchWorkflow,
        }],
        workflow_origin: None,
    }
}
fn plain(line: &str) -> String {
    strip_ansi(line).trim_end().to_owned()
}
fn plain_frame(transcript: &mut TranscriptPane, width: usize, height: usize) -> Vec<String> {
    transcript
        .render_frame(width, height)
        .expect("render frame")
        .iter()
        .map(|line| plain(line))
        .collect()
}
fn write_request(id: &str, prefix: &str, files: usize) -> ApprovalRequest {
    let changes = (0..files)
        .map(|index| WriteApprovalChange {
            path: PathBuf::from(format!("src/{prefix}_{index}.rs")),
            line_count: 3,
            added: 3,
            removed: 0,
            preview: WriteApprovalPreview::Created {
                content: format!("fn {prefix}_{index}() {{}}\n// line 2\n// line 3\n"),
            },
        })
        .collect();
    ApprovalRequest {
        turn: 1,
        id: id.to_owned(),
        operation: PermissionOperation::FileWrite,
        presentation: ApprovalPresentation::Write {
            title: format!("Write {files} files?"),
            write: WriteApprovalPresentation {
                files,
                created: files,
                overwritten: 0,
                added: files * 3,
                removed: 0,
                changes,
            },
        },
        options: shell_options(),

        workflow_origin: None,
    }
}

#[test]
fn approval_resolution_updates_the_matching_inline_card() {
    let mut transcript_pane = TranscriptPane::new(100, 24);
    let request = shell_request("bash-1", "sleep 5", None, shell_options());

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: request.clone(),
    });
    transcript_pane.select_approval("bash-1", 1, "scratch feedback", true);

    let resolution = ApprovalResolution::Selected {
        action: ApprovalAction::Reject,
        label: "Reject".to_owned(),
        feedback: None,
    };
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalResolved {
        turn: 1,
        request_id: request.id.clone(),
        resolution: resolution.clone(),
    });

    let card = transcript_pane
        .transcript()
        .approval("bash-1")
        .expect("matching approval card");
    assert_eq!(
        card.state,
        neo_tui::transcript::ApprovalDisplayState::Resolved(resolution)
    );
    assert!(!card.feedback_active);
    assert!(
        card.feedback_input.is_empty(),
        "resolved cards must drop interactive feedback"
    );

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    assert!(
        frame.iter().any(|line| line.trim() == "approval: Rejected"),
        "frame should show the resolved reject status from the event: {frame:?}"
    );
    assert!(
        !frame.iter().any(|line| line.contains("↑/↓ select")),
        "resolved card must not keep the interactive prompt: {frame:?}"
    );
}

#[test]
fn batch_write_approval_follows_global_expansion() {
    let mut pane = TranscriptPane::new(64, 80);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: write_request("write-1", "batch", 4),
    });

    // Collapsed: only first 2 + last file paths visible.
    let collapsed = plain_frame(&mut pane, 64, 80);
    assert!(
        collapsed.iter().any(|line| line.contains("batch_0.rs")),
        "first file visible collapsed: {collapsed:?}"
    );
    assert!(
        collapsed.iter().any(|line| line.contains("batch_1.rs")),
        "second file visible collapsed: {collapsed:?}"
    );
    assert!(
        collapsed.iter().any(|line| line.contains("batch_3.rs")),
        "last file visible collapsed: {collapsed:?}"
    );
    assert!(
        !collapsed.iter().any(|line| line.contains("batch_2.rs")),
        "middle file omitted collapsed: {collapsed:?}"
    );

    // Toggle global expansion.
    pane.set_tool_output_expanded(true);
    let expanded = plain_frame(&mut pane, 64, 80);
    for index in 0..4 {
        assert!(
            expanded
                .iter()
                .any(|line| line.contains(&format!("batch_{index}.rs"))),
            "file {index} visible expanded: {expanded:?}"
        );
    }
}

#[test]
fn finalizing_transcript_preserves_queued_approvals_before_exit() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=2 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            request: shell_request(
                &format!("historical-{number}"),
                &command,
                None,
                shell_options(),
            ),
        });
    }
    transcript_pane.finalize_interrupted_live_entries();

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_request("current", "printf current", None, shell_options()),
    });
    transcript_pane.resolve_approval("current", &approved_resolution());

    let ids = transcript_pane
        .transcript()
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::ApprovalPrompt(data) => Some(data.id()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(ids.contains(&"historical-1"));
    assert!(ids.contains(&"historical-2"));
    assert!(ids.contains(&"current"));
}

#[test]
fn queued_edit_approval_inherits_current_global_expansion() {
    let mut pane = TranscriptPane::new(64, 80);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: edit_request("edit-1", "first", 1),
    });
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: edit_request("edit-2", "queued", 4),
    });
    pane.set_tool_output_expanded(true);
    pane.resolve_approval("edit-1", &approved_resolution());

    let approval = pane
        .transcript()
        .approval("edit-2")
        .expect("queued approval promoted");
    assert!(approval.expanded);
    let frame = plain_frame(&mut pane, 64, 80);
    assert!(frame.iter().any(|line| line.contains("queued_2.rs")));
}

#[test]
fn resolved_workflow_approval_keeps_source_and_expansion() {
    let mut pane = TranscriptPane::new(100, 80);
    let request = workflow_request("workflow-1");
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: request.clone(),
    });
    pane.set_tool_output_expanded(true);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalResolved {
        turn: 1,
        request_id: request.id.clone(),
        resolution: ApprovalResolution::Selected {
            action: ApprovalAction::LaunchWorkflow,
            label: "Launch".to_owned(),
            feedback: None,
        },
    });

    let card = pane
        .transcript()
        .approval("workflow-1")
        .expect("workflow approval card");
    assert!(card.expanded);
    assert_eq!(card.request, request);
    assert!(pane.toggle_tool_output_expanded());
    assert!(!pane.tool_output_expanded());

    let frame = plain_frame(&mut pane, 100, 80).join("\n");
    assert!(frame.contains("approval: Launch"), "{frame}");
    assert!(frame.contains("neo.phase('work')"), "{frame}");
    assert!(frame.contains("return {}"), "{frame}");
    assert!(!frame.contains("↵ confirm"), "resolved frame: {frame}");
}

#[test]
fn transcript_pane_advances_next_queued_approval_after_resolution() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=2 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            request: shell_request(&format!("bash-{number}"), &command, None, shell_options()),
        });
    }
    transcript_pane.resolve_approval("bash-1", &approved_resolution());

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    assert!(frame.iter().any(|line| line.contains("Approved")));
    assert!(frame.iter().any(|line| line.contains("$ printf 2")));
    assert!(!frame.iter().any(|line| line.contains("queued:")));
}

#[test]
fn transcript_pane_edit_approval_follows_global_expansion() {
    let mut pane = TranscriptPane::new(64, 80);
    pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: edit_request("edit-1", "verified", 4),
    });

    // Collapsed: the card shows the per-file stat row (path + +/- counts)
    // inside its frame and hides the omitted file's diff details.
    let collapsed = plain_frame(&mut pane, 64, 80);
    assert!(
        collapsed.iter().any(|line| line.contains("verified_2.rs")),
        "collapsed stat row should show the file path: {collapsed:?}"
    );
    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("diff details hidden")),
        "collapsed card should hide the full diff: {collapsed:?}"
    );
    assert!(
        !collapsed.iter().any(|line| line.contains("old2")),
        "collapsed card should not show the omitted file's diff: {collapsed:?}"
    );
    let collapsed_bottom = collapsed
        .iter()
        .rposition(|line| line.trim_start().starts_with('╰'))
        .expect("collapsed bottom frame");

    pane.set_tool_output_expanded(true);
    let expanded = plain_frame(&mut pane, 64, 80);
    assert!(
        expanded.iter().any(|line| line.contains("verified_2.rs")),
        "expanded card shows the file path: {expanded:?}"
    );
    assert!(
        expanded.iter().any(|line| line.contains("old2")),
        "expanded card reveals the full diff details: {expanded:?}"
    );
    let expanded_bottom = expanded
        .iter()
        .rposition(|line| line.trim_start().starts_with('╰'))
        .expect("expanded bottom frame");
    assert!(expanded_bottom > collapsed_bottom);
}

#[test]
fn transcript_pane_places_approval_after_matching_tool_and_renders_resolution_lightly() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf 1" }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "tool-2".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "printf 2" }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_request("tool-1", "printf 1", None, shell_options()),
    });

    let frame = plain_frame(&mut transcript_pane, 100, 24);
    let tool_1 = frame
        .iter()
        .position(|line| line.contains("$ printf 1"))
        .expect("first tool");
    let approval = frame
        .iter()
        .position(|line| line.contains("Run this command?"))
        .expect("approval");
    let tool_2 = frame
        .iter()
        .position(|line| line.contains("$ printf 2"))
        .expect("second tool");
    assert!(tool_1 < approval);
    assert!(
        approval < tool_2,
        "approval should stay near matching tool: {frame:?}"
    );

    transcript_pane.resolve_approval("tool-1", &approved_resolution());
    let resolved = plain_frame(&mut transcript_pane, 100, 24);
    assert!(
        resolved
            .iter()
            .any(|line| line.trim() == "approval: Approved"),
        "resolved approval should be lightweight: {resolved:?}"
    );
    assert!(
        !resolved
            .iter()
            .any(|line| line.chars().all(|ch| ch == '\u{2500}') && line.len() > 20),
        "resolved approval should not keep yellow divider bars: {resolved:?}"
    );
}

#[test]
fn transcript_pane_renders_inline_bash_approval_prompt() {
    let mut transcript_pane = TranscriptPane::new(100, 16);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ToolExecutionStarted {
        turn: 1,
        id: "bash-1".to_owned(),
        name: "Bash".to_owned(),
        arguments: serde_json::json!({ "command": "echo hello" }),

        workflow_origin: None,
        output_ref: None,
    });
    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: shell_request(
            "bash-1",
            "echo hello",
            Some("/Users/chenyuanhao/Workspace/neo"),
            shell_options(),
        ),
    });

    let frame = plain_frame(&mut transcript_pane, 100, 16);
    let using = frame
        .iter()
        .position(|line| line.contains("Using Bash"))
        .expect("running bash tool");
    let approval = frame
        .iter()
        .position(|line| line.contains("Run this command?"))
        .expect("inline approval prompt");

    assert!(using < approval);
    assert!(
        frame
            .iter()
            .any(|line| line.contains("cwd: /Users/chenyuanhao/Workspace/neo"))
    );
    assert!(frame.iter().any(|line| line.contains("$ echo hello")));
    // Request options are rendered as-is — no synthetic session option.
    assert!(frame.iter().any(|line| line.contains("1. Approve once")));
    assert!(frame.iter().any(|line| line.contains("2. Reject")));
    assert!(
        !frame
            .iter()
            .any(|line| line.contains("Approve for this session")),
        "transcript must not invent session options: {frame:?}"
    );
    assert!(
        frame.iter().any(|line| {
            line.contains("↑/↓ select")
                && line.contains("number keys choose")
                && line.contains("↵ confirm")
        }),
        "approval prompt should show the keyboard hint: {frame:?}"
    );

    transcript_pane.resize(36, 24);
    let narrow = plain_frame(&mut transcript_pane, 36, 24);
    assert!(
        narrow.iter().all(|line| visible_width(line) <= 34),
        "approval prompt lines should fit narrow transcript width: {narrow:?}"
    );
}

#[test]
fn earliest_pending_approval_owns_visible_window_until_resolved() {
    let mut transcript_pane = TranscriptPane::new(100, 24);

    for number in 1..=3 {
        let command = format!("printf {number}");
        transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
            request: shell_request(&format!("bash-{number}"), &command, None, shell_options()),
        });
    }

    // The earliest unresolved approval owns the visible window: its card
    // rows are visible while later approval cards stay in the document
    // (their rows are counted by the layout) but outside the visible slice
    // until the blocking entry resolves.
    let slice = transcript_pane
        .render_visible_slice(100, 40)
        .iter()
        .map(|line| plain(line))
        .collect::<Vec<_>>();
    assert!(
        slice.iter().any(|line| line.contains("$ printf 1")),
        "earliest approval card must own the visible window: {slice:?}"
    );
    assert!(
        !slice.iter().any(|line| line.contains("$ printf 2")),
        "later approval must stay outside the visible window: {slice:?}"
    );
    assert!(
        !slice.iter().any(|line| line.contains("$ printf 3")),
        "later approval must stay outside the visible window: {slice:?}"
    );
    assert!(
        transcript_pane.document().total_rows() > slice.len(),
        "later approval cards remain in the document: {slice:?}"
    );
    assert_eq!(
        transcript_pane.earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "bash-1".to_owned()
        ))
    );

    // Resolving the earliest approval advances the blocking focus to the
    // next card, which then owns the visible window.
    transcript_pane.resolve_approval("bash-1", &approved_resolution());
    let advanced = transcript_pane
        .render_visible_slice(100, 40)
        .iter()
        .map(|line| plain(line))
        .collect::<Vec<_>>();
    assert!(
        advanced.iter().any(|line| line.contains("$ printf 2")),
        "resolved focus moves to the next approval card: {advanced:?}"
    );
    assert!(
        !advanced.iter().any(|line| line.contains("$ printf 1")),
        "the resolved card leaves the visible window: {advanced:?}"
    );
    assert!(
        !advanced.iter().any(|line| line.contains("$ printf 3")),
        "later approval stays outside the visible window: {advanced:?}"
    );
    assert_eq!(
        transcript_pane.earliest_blocking_entry(),
        Some(neo_tui::transcript::BlockingEntryKind::Approval(
            "bash-2".to_owned()
        ))
    );
}

#[test]
fn transcript_pane_renders_write_approval_from_request() {
    let mut transcript_pane = TranscriptPane::new(100, 18);

    transcript_pane.apply_agent_event(neo_agent_core::AgentEvent::ApprovalRequested {
        request: ApprovalRequest {
            turn: 1,
            id: "write-1".to_owned(),
            operation: PermissionOperation::FileWrite,
            presentation: ApprovalPresentation::Tool {
                title: "Write file?".to_owned(),
                details: vec!["path: src/lib.rs".to_owned()],
            },
            options: shell_options(),

            workflow_origin: None,
        },
    });

    let frame = plain_frame(&mut transcript_pane, 100, 18);
    assert!(frame.iter().any(|line| line.contains("Write file?")));
    assert!(frame.iter().any(|line| line.contains("path: src/lib.rs")));
}
