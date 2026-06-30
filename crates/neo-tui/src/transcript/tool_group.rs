//! Tree-style grouping for consecutive same-tool calls (read/grep/glob/find).
//!
//! Mirrors the Neo read-group layout: a single header line summarizing
//! the whole batch, followed by one indented row per call using `├─`/`└─`
//! branch characters. Used by [`crate::transcript::TranscriptPane`] so a run of
//! consecutive reads renders as one card instead of N cards.

use crate::primitive::Style;
use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Span};
use crate::shell::ToolStatusKind;

use super::tool_call::ToolCallState;
use super::tool_renderers::key_argument;

const FILE_PREVIEW_LIMIT: usize = 5;

/// Group of consecutive tool states that share the same (case-insensitive)
/// tool name and belong to the same turn.
#[derive(Debug, Clone)]
pub struct ToolGroup<'a> {
    pub tool: String,
    pub states: Vec<&'a ToolCallState>,
}

/// Render a tree-style group card for a run of same-tool calls.
///
/// Header variants:
/// - all succeeded: `● Read {n} files · {lines} lines`
/// - running (some pending): `● Reading {n} files`
/// - all failed: `✗ Read {n} files · failed`
/// - mixed: `● Read {n} files · {lines} lines · {f} failed`
///
/// Body: one `├─`/`└─` row per file (path, status chip). Files beyond
/// [`FILE_PREVIEW_LIMIT`] collapse into a single `… {n} more files` row unless
/// the group is expanded.
///
/// `width` is the content width (without the gutter) that the caller will
/// apply later; rows are truncated so they do not exceed it.
#[must_use]
pub fn render_tool_group(
    group: &ToolGroup,
    width: usize,
    theme: &TuiTheme,
    expanded: bool,
) -> Vec<Line> {
    let n = group.states.len();
    let unit = group_unit(&group.tool);
    let any_running = group
        .states
        .iter()
        .any(|s| matches!(s.status, ToolStatusKind::Pending | ToolStatusKind::Running));
    let failed_count = group
        .states
        .iter()
        .filter(|s| matches!(s.status, ToolStatusKind::Failed))
        .count();
    let all_failed = failed_count == n;

    let mut rows = Vec::new();

    // ---- Header -------------------------------------------------------
    // The header reads `● Read 3 files · 484 lines`. The tool name (Read/
    // Grep/...) uses the brand color; the symbol + count use the status color;
    // the chip (`· 484 lines`) uses weak text.
    let verb_past = group_verb_past(&group.tool);
    let verb_prog = group_verb_progressive(&group.tool);
    let (symbol, symbol_color, count, chip) = if any_running {
        // Match the finished-state layout: only the tool name stays branded;
        // the status symbol and count use the ok status color.
        ("●", theme.status_ok, format!("{n} {unit}"), String::new())
    } else if all_failed {
        (
            "✗",
            theme.status_error,
            format!("{n} {unit}"),
            " · failed".to_owned(),
        )
    } else {
        let total: usize = group
            .states
            .iter()
            .filter(|s| matches!(s.status, ToolStatusKind::Succeeded))
            .filter_map(|s| s.result.as_deref())
            .map(|r| r.lines().count())
            .sum();
        let chip = if failed_count > 0 {
            format!(" · {total} lines · {failed_count} failed")
        } else {
            format!(" · {total} lines")
        };
        ("●", theme.status_ok, format!("{n} {unit}"), chip)
    };
    let weak = Style::default().fg(theme.text_muted);
    // The tool name uses the progressive verb while running, the past verb
    // once finished.
    let name = if any_running { verb_prog } else { verb_past };
    rows.push(Line::from_spans(vec![
        Span::styled(format!("{symbol} "), Style::default().fg(symbol_color)),
        Span::styled(name, Style::default().fg(theme.brand).bold()),
        Span::styled(format!(" {count}"), Style::default().fg(symbol_color)),
        Span::styled(chip, weak),
    ]));

    // ---- Body: per-file tree rows ------------------------------------
    let preview = if expanded {
        n
    } else {
        n.min(FILE_PREVIEW_LIMIT)
    };
    let weak = Style::default().fg(theme.text_muted);
    for (idx, state) in group.states.iter().take(preview).enumerate() {
        let is_last = idx == preview.min(n) - 1;
        let branch = if is_last { "└─" } else { "├─" };
        let path = key_argument(state.arguments.as_deref());
        let tail = per_file_tail(state, &group.tool);
        rows.push(Line::from_spans(vec![
            Span::styled(format!("  {branch} "), weak),
            Span::styled(
                path,
                Style::default().fg(if matches!(state.status, ToolStatusKind::Failed) {
                    theme.status_error
                } else {
                    theme.text_primary
                }),
            ),
            Span::styled(tail, weak),
        ]));
    }
    if !expanded && n > FILE_PREVIEW_LIMIT {
        let extra = n - FILE_PREVIEW_LIMIT;
        rows.push(Line::styled(format!("  … {extra} more {unit}"), weak));
    }
    rows.into_iter()
        .map(|row| row.truncate_to_width(width))
        .collect()
}

/// The countable noun for the tool ("files" for read/glob/find, "patterns"
/// for grep). Falls back to "files".
fn group_unit(lower: &str) -> &'static str {
    match lower {
        "Grep" => "patterns",
        _ => "files",
    }
}

/// The past-tense verb for the group header ("Read", "Grep", "List", ...).
/// Capitalized so the header reads `● Read 3 files`, not `● read 3 files`.
fn group_verb_past(lower: &str) -> &'static str {
    match lower {
        "Grep" => "Grep",
        "Glob" => "Glob",
        "Find" => "Find",
        "List" => "List",
        _ => "Read",
    }
}

/// The progressive (running) verb for the group header ("Reading",
/// "Grepping", ...).
fn group_verb_progressive(lower: &str) -> &'static str {
    match lower {
        "Grep" => "Grepping",
        "Glob" => "Globbing",
        "Find" => "Finding",
        "List" => "Listing",
        _ => "Reading",
    }
}

/// The `· ...` chip appended to each per-file row, reflecting that file's
/// status.
fn per_file_tail(state: &ToolCallState, lower: &str) -> String {
    match state.status {
        ToolStatusKind::Pending | ToolStatusKind::Running => " · reading…".to_owned(),
        ToolStatusKind::Failed => " · failed".to_owned(),
        ToolStatusKind::Cancelled => " · cancelled".to_owned(),
        ToolStatusKind::Succeeded => {
            let result = state.result.as_deref().unwrap_or("");
            match lower {
                "Read" | "Write" => format!(" · {} lines", result.lines().count()),
                "Grep" => {
                    let matches = grep_match_count(result);
                    format!(" · {matches} matches")
                }
                _ => {
                    let count = result.lines().filter(|l| !l.is_empty()).count();
                    format!(" · {count}")
                }
            }
        }
    }
}

/// Count grep matches from a `path:line:match` result body.
fn grep_match_count(result: &str) -> usize {
    result.lines().filter(|line| !line.is_empty()).count()
}
