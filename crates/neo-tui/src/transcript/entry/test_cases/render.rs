use super::*;
use crate::primitive::theme::TuiTheme;

#[test]
fn welcome_banner_has_correct_width_and_logo() {
    let data = BannerData {
        title: "Welcome to Neo!".to_owned(),
        subtitle: "Send /help for help information.".to_owned(),
        directory: "/tmp/neo".to_owned(),
        session: "test".to_owned(),
        model: "deepseek/deepseek-v4-pro".to_owned(),
        version: "0.1.0".to_owned(),
        mcp: None,
    };
    let lines = render_banner::render_welcome_banner(&data, 60, &TuiTheme::default());
    for line in &lines {
        let width = crate::primitive::visible_width(&line.to_ansi());
        assert!(
            width == 60 || width == 0,
            "line width mismatch: {:?}",
            line.text()
        );
    }
    // The right edge of both logo rows should use the left half-block
    // glyph, not the square corner glyph '┐'.
    for logo_line in [&lines[2], &lines[3], &lines[4]] {
        assert!(!logo_line.text().contains('┐'));
    }
    assert!(
        lines[2]
            .text()
            .contains("\u{2590}\u{2588}\u{259b}  \u{2588}\u{258c}  Welcome to Neo!")
    );
    assert!(
        lines[3].text().contains(
            "\u{2590}\u{2588} \u{2588} \u{2588}\u{258c}  Send /help for help information."
        )
    );
    assert!(
        lines[4]
            .text()
            .contains("\u{2590}\u{2588}  \u{2599}\u{2588}\u{258c}")
    );
    let ansi = lines[2].to_ansi();
    assert!(ansi.contains("\x1b[38;2;63;247;255m"));
    assert!(ansi.contains("\x1b[38;2;255;79;216m"));
    assert!(ansi.contains("\x1b[38;2;138;92;255m"));
}

#[test]
fn thinking_block_expands_full_text() {
    let content = "one two three four five six seven eight nine ten eleven twelve";
    let collapsed = TranscriptEntry::ThinkingBlock {
        parts: vec![ThinkingPart::new(content, None)],
        kind: ThinkingKind::Unknown,
        phase: ThinkingPhase::Complete,
        expanded: false,
    }
    .render(14, &TuiTheme::default())
    .into_iter()
    .map(|line| line.text().clone())
    .collect::<Vec<_>>();
    let expanded = TranscriptEntry::ThinkingBlock {
        parts: vec![ThinkingPart::new(content, None)],
        kind: ThinkingKind::Unknown,
        phase: ThinkingPhase::Complete,
        expanded: true,
    }
    .render(14, &TuiTheme::default())
    .into_iter()
    .map(|line| line.text().clone())
    .collect::<Vec<_>>();

    assert!(
        collapsed
            .iter()
            .any(|line| line.contains("ctrl+o to expand"))
    );
    assert!(
        !expanded
            .iter()
            .any(|line| line.contains("ctrl+o to expand"))
    );
    assert!(expanded.len() > collapsed.len());
}

#[test]
fn skill_activation_renders_aggregate_collapsed_preview() {
    let entry = TranscriptEntry::skill_invocation(
        vec!["skill_one".to_owned(), "skill_two".to_owned()],
        SkillInvocationSource::Manual,
        SkillInvocationOutcome::Activated,
        "\
foo
bar
test test test
bonjour
hello
test test test test
hola
amigo",
    );
    let lines = entry
        .render(60, &TuiTheme::default())
        .into_iter()
        .collect::<Vec<_>>();
    let text = lines.iter().map(Line::text).collect::<Vec<_>>();

    assert_eq!(text[0], "✦ Skill activated: skill_one, skill_two · manual");
    assert!(text[1].starts_with("━"));
    assert_eq!(text[2], "foo");
    assert_eq!(text[3], "bar");
    assert_eq!(text[4], "test test test");
    assert_eq!(text[5], "… 5 more lines (ctrl+o to expand)");
    assert!(
        !text.iter().any(|line| line.contains("/skill:")),
        "{text:?}"
    );

    let header_spans = lines[0].spans();
    assert_eq!(header_spans[0].text(), "✦ Skill activated: ");
    assert_eq!(
        header_spans[0].style().fg,
        Some(TuiTheme::default().status_warn)
    );
    assert_eq!(header_spans[1].text(), "skill_one, skill_two");
    assert_eq!(header_spans[1].style().fg, Some(TuiTheme::default().brand));
    assert_eq!(
        lines[2].spans()[0].style().fg,
        Some(TuiTheme::default().text_muted)
    );
    assert!(lines[2].spans()[0].style().italic);
}

#[test]
fn skill_activation_expands_full_body() {
    let entry = TranscriptEntry::skill_invocation(
        vec!["skill_one".to_owned(), "skill_two".to_owned()],
        SkillInvocationSource::Manual,
        SkillInvocationOutcome::Activated,
        "foo\nbar\ntest test test\nbonjour\nhello\ntest test test test\nhola\namigo",
    );
    let mut entry = entry;
    if let TranscriptEntry::SkillActivation { expanded, .. } = &mut entry {
        *expanded = true;
    }
    let lines = entry
        .render(60, &TuiTheme::default())
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();

    assert_eq!(lines[0], "✦ Skill activated: skill_one, skill_two · manual");
    assert!(lines.contains(&"bonjour".to_owned()));
    assert!(lines.contains(&"amigo".to_owned()));
    assert!(!lines.iter().any(|l| l.contains("ctrl+o to expand")));
}

#[test]
fn skill_activation_preserves_source_at_narrow_width() {
    let entry = TranscriptEntry::skill_invocation(
        vec!["using-aegis".to_owned()],
        SkillInvocationSource::Auto,
        SkillInvocationOutcome::Activated,
        "",
    );

    let header = entry.render(24, &TuiTheme::default())[0].text().clone();

    assert!(
        header.contains("· auto"),
        "source should remain visible: {header}"
    );
    assert!(visible_width(&header) <= 24, "header should fit: {header}");
}

#[test]
fn compaction_in_progress_renders_estimated_static_progress_bar() {
    let entry = TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 70,
        compacted_message_count: 0,
        tokens_before: 0,
        tokens_after: 0,
    };
    let lines = entry
        .render_with_activity_frame(80, &TuiTheme::default(), 0)
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("");
    assert!(text.contains("Compacting context"), "{text}");
    assert!(text.contains("Summarizing"), "{text}");
    assert!(text.contains("~70%"), "{text}");
    assert!(text.contains('█'), "{text}");
    assert!(!text.contains('▓'), "{text}");
    assert!(!text.contains('▒'), "{text}");
}

#[test]
fn compaction_render_is_independent_of_activity_frame() {
    let entry = TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 70,
        compacted_message_count: 0,
        tokens_before: 0,
        tokens_after: 0,
    };
    let frame_zero = entry
        .render_with_activity_frame(80, &TuiTheme::default(), 0)
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    let frame_one = entry
        .render_with_activity_frame(80, &TuiTheme::default(), 1)
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>();
    assert_eq!(frame_zero, frame_one);
}

#[test]
fn compaction_narrow_width_keeps_one_stable_line() {
    let entry = TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Summarizing),
        percent: 70,
        compacted_message_count: 0,
        tokens_before: 0,
        tokens_after: 0,
    };
    let lines = entry.render(24, &TuiTheme::default());
    assert_eq!(lines.len(), 1);
    let text = lines[0].text().clone();
    assert!(text.contains("Summarizing"), "{text}");
    assert!(text.contains("~70%"), "{text}");
    assert!(visible_width(&text) <= 24, "{text}");
}

#[test]
fn compaction_complete_renders_token_reduction() {
    let entry = TranscriptEntry::Compaction {
        phase: Some(neo_agent_core::CompactionPhase::Applying),
        percent: 100,
        compacted_message_count: 852,
        tokens_before: 192_000,
        tokens_after: 24_000,
    };
    let lines = entry
        .render_with_activity_frame(80, &TuiTheme::default(), 0)
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("");
    assert!(text.contains("Compaction complete"), "{text}");
    assert!(text.contains("852"), "{text}");
    assert!(text.contains("192k"), "{text}");
    assert!(text.contains("24k"), "{text}");
    assert!(!entry.has_visible_animation());
}

fn plan_prompt_data(
    selected: usize,
    feedback_active: bool,
    feedback_input: String,
) -> ApprovalPromptData {
    use neo_agent_core::{ApprovalAction, ApprovalOption, PermissionOperation};
    ApprovalPromptData {
        request: ApprovalRequest {
            turn: 1,
            id: "test-id".to_owned(),
            operation: PermissionOperation::PlanTransition,
            presentation: ApprovalPresentation::Plan {
                title: "Plan Review".to_owned(),
                path: None,
                markdown: String::new(),
                summary: Some("Ready?".to_owned()),
            },
            options: vec![
                ApprovalOption {
                    label: "Approve".to_owned(),
                    description: None,
                    action: ApprovalAction::ApprovePlan { selection: None },
                },
                ApprovalOption {
                    label: "Suggestion: Keep 85% window".to_owned(),
                    description: Some("Keep compaction window at 85%.".to_owned()),
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: Some("Keep compaction at 85%.".to_owned()),
                    },
                },
                ApprovalOption {
                    label: "Reject".to_owned(),
                    description: None,
                    action: ApprovalAction::RejectPlan,
                },
                ApprovalOption {
                    label: "Reject with feedback".to_owned(),
                    description: None,
                    action: ApprovalAction::RevisePlan {
                        preset_feedback: None,
                    },
                },
            ],

            workflow_origin: None,
        },
        selected,
        feedback_input,
        feedback_active,
        expanded: false,
        state: ApprovalDisplayState::Pending,
    }
}

#[test]
fn approval_prompt_renders_canonical_options() {
    let data = plan_prompt_data(0, false, String::new());
    let lines = TranscriptEntry::ApprovalPrompt(data)
        .render(80, &TuiTheme::default())
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    assert!(text.contains("1. Approve"), "{text}");
    assert!(text.contains("2. Suggestion: Keep 85% window"), "{text}");
    assert!(text.contains("Keep compaction window at 85%."), "{text}");
    assert!(text.contains("3. Reject"), "{text}");
}

#[test]
fn approval_prompt_highlights_selected_revision_feedback() {
    let data = plan_prompt_data(1, true, "Keep compaction at 85%.".to_owned());
    let lines = TranscriptEntry::ApprovalPrompt(data)
        .render(80, &TuiTheme::default())
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    assert!(text.contains("feedback: Keep compaction at 85%."), "{text}");
}

#[test]
fn approval_prompt_hides_feedback_until_input_is_active() {
    let data = plan_prompt_data(3, false, String::new());
    let lines = TranscriptEntry::ApprovalPrompt(data)
        .render(80, &TuiTheme::default())
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    assert!(!text.contains("feedback:"), "{text}");
}

#[test]
fn approval_prompt_shows_feedback_when_input_is_active() {
    let data = plan_prompt_data(3, true, String::new());
    let lines = TranscriptEntry::ApprovalPrompt(data)
        .render(80, &TuiTheme::default())
        .into_iter()
        .map(|l| l.text().clone())
        .collect::<Vec<_>>();
    let text = lines.join("\n");
    assert!(text.contains("feedback: ▌"), "{text}");
}

#[test]
fn edit_approval_prompt_follows_global_expansion() {
    use neo_agent_core::{
        ApprovalAction, ApprovalOption, EditApprovalChange, EditApprovalPresentation,
        PermissionOperation,
    };

    let changes = (0..4)
        .map(|index| EditApprovalChange {
            path: std::path::PathBuf::from(format!("src/file{index}.rs")),
            replacements: 1,
            added: 1,
            removed: 1,
            diff: format!(
                "--- src/file{index}.rs\n+++ src/file{index}.rs\n@@ -12 +12 @@\n-old{index}\n+new{index}\n"
            ),
        })
        .collect();
    let mut entry = TranscriptEntry::ApprovalPrompt(ApprovalPromptData {
        request: ApprovalRequest {
            turn: 1,
            id: "edit-approval".to_owned(),
            operation: PermissionOperation::FileWrite,
            presentation: ApprovalPresentation::Edit {
                title: "Edit 4 files?".to_owned(),
                edit: EditApprovalPresentation {
                    files: 4,
                    replacements: 4,
                    added: 4,
                    removed: 4,
                    changes,
                },
            },
            options: vec![ApprovalOption {
                label: "Allow".to_owned(),
                description: None,
                action: ApprovalAction::PermitOnce,
            }],

            workflow_origin: None,
        },
        selected: 0,
        feedback_input: String::new(),
        feedback_active: false,
        expanded: false,
        state: ApprovalDisplayState::Pending,
    });

    assert!(entry.is_expandable());
    let theme = TuiTheme::default();
    let collapsed_lines = entry.render(64, &theme);
    assert!(
        collapsed_lines
            .iter()
            .flat_map(Line::spans)
            .any(|span| { span.text() == "+4" && span.style().fg == Some(theme.diff_added) })
    );
    assert!(
        collapsed_lines
            .iter()
            .flat_map(Line::spans)
            .any(|span| { span.text() == "-4" && span.style().fg == Some(theme.diff_removed) })
    );
    let collapsed = collapsed_lines
        .into_iter()
        .map(|line| line.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        collapsed.contains("4 files · 4 replacements"),
        "{collapsed}"
    );
    // Collapsed: the per-file stat row shows the omitted file path while the
    // full diff details stay hidden behind the expand hint.
    assert!(
        collapsed.contains("src/file2.rs"),
        "collapsed stat row should show the file path: {collapsed}"
    );
    assert!(
        collapsed.contains("diff details hidden"),
        "collapsed card should hide the full diff: {collapsed}"
    );
    assert!(!collapsed.contains("old2"), "{collapsed}");
    assert!(
        collapsed.contains('╭') && collapsed.contains('╰'),
        "{collapsed}"
    );

    assert!(entry.set_expanded(true));
    let expanded = entry
        .render(64, &TuiTheme::default())
        .into_iter()
        .map(|line| line.text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(expanded.contains("src/file2.rs"), "{expanded}");
    assert!(!expanded.contains("files · 1 replacements"), "{expanded}");
    assert!(expanded.contains("12 - old2"), "{expanded}");
    assert!(expanded.contains("12 + new2"), "{expanded}");
}
