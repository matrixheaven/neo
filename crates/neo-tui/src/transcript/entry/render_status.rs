use super::{Color, RetryPhase, RetryStatusData, Span, StatusSeverity, Style, TuiTheme};
use crate::primitive::Line;

#[allow(clippy::needless_pass_by_value)]
fn severity_color(severity: StatusSeverity, theme: &TuiTheme) -> Color {
    match severity {
        StatusSeverity::Info => theme.brand,
        StatusSeverity::Warning => theme.status_warn,
        StatusSeverity::Error => theme.status_error,
    }
}

pub(super) fn render_status(
    text: &str,
    severity: Option<StatusSeverity>,
    width: usize,
    theme: &TuiTheme,
) -> Vec<Line> {
    let Some(severity) = severity else {
        return super::styled_wrap(text, width, status_style(theme));
    };
    let style = Style::default().fg(severity_color(severity, theme)).bold();
    super::styled_wrap(text, width, style)
}

pub(super) fn status_style(theme: &TuiTheme) -> Style {
    Style::default().fg(theme.text_muted)
}

pub(super) fn render_retry_status(
    data: &RetryStatusData,
    width: usize,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    const SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let spinner = SPINNER[activity_frame % SPINNER.len()];
    let (header, color) = match data.phase {
        RetryPhase::Waiting => {
            let elapsed_ms = super::monotonic_time_ms().saturating_sub(data.started_at_ms);
            let remaining_ms = data.delay_ms.saturating_sub(elapsed_ms);
            (
                format!(
                    "{spinner} Reconnecting {}/{} · retry in {} · esc interrupt",
                    data.retry,
                    data.max_retries,
                    format_retry_delay(remaining_ms)
                ),
                theme.status_warn,
            )
        }
        RetryPhase::Connecting => (
            format!(
                "{spinner} Reconnecting {}/{} · connecting · esc interrupt",
                data.retry, data.max_retries
            ),
            theme.brand,
        ),
        RetryPhase::Exhausted => {
            let header = match data.retry {
                0 => "Reconnect failed · retry disabled".to_owned(),
                1 => "Reconnect failed after 1 retry".to_owned(),
                retry => format!("Reconnect failed after {retry} retries"),
            };
            (header, theme.status_error)
        }
    };
    let mut lines = super::styled_wrap(&header, width, Style::default().fg(color).bold());
    if !data.message.is_empty() {
        let (title, message) = if data.error_code == "provider.transport_error" {
            (
                "Network",
                data.message
                    .strip_prefix("transport error: ")
                    .unwrap_or(data.message.as_str()),
            )
        } else {
            let title = neo_agent_core::error_info(&data.error_code).title;
            (
                title.strip_suffix(" Error").unwrap_or(title),
                data.message.as_str(),
            )
        };
        lines.extend(super::styled_wrap(
            &format!("{title} · {message}"),
            width,
            status_style(theme),
        ));
    }
    lines
}

fn format_retry_delay(delay_ms: u64) -> String {
    let seconds = delay_ms.saturating_add(999) / 1_000;
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

fn compaction_active_color(
    phase: Option<neo_agent_core::CompactionPhase>,
    theme: &TuiTheme,
) -> Color {
    match phase {
        Some(neo_agent_core::CompactionPhase::Estimating)
        | Some(neo_agent_core::CompactionPhase::SelectingBoundary) => theme.status_warn,
        Some(neo_agent_core::CompactionPhase::Summarizing) => theme.brand,
        Some(neo_agent_core::CompactionPhase::Applying) => theme.status_ok,
        None => theme.brand,
    }
}

fn compaction_short_phase_label(phase: Option<neo_agent_core::CompactionPhase>) -> &'static str {
    match phase {
        Some(neo_agent_core::CompactionPhase::Estimating) => "Estimating",
        Some(neo_agent_core::CompactionPhase::SelectingBoundary) => "Selecting",
        Some(neo_agent_core::CompactionPhase::Summarizing) => "Summarizing",
        Some(neo_agent_core::CompactionPhase::Applying) => "Applying",
        None => "Compacting",
    }
}

fn compaction_progress_line(
    phase_label: &str,
    percent: u8,
    bar_width: usize,
    active_color: Color,
    theme: &TuiTheme,
) -> Line {
    let filled = (usize::from(percent.min(100)) * bar_width).div_ceil(100);
    let empty = bar_width.saturating_sub(filled);
    let active_style = Style::default().fg(active_color).bold();
    let muted_style = Style::default().fg(theme.text_muted);
    let mut spans = vec![
        Span::styled("◈ ", Style::default().fg(theme.brand).bold()),
        Span::styled(format!("{phase_label} "), active_style),
        Span::styled("[", muted_style),
    ];
    for _ in 0..filled {
        spans.push(Span::styled("█", active_style));
    }
    for _ in 0..empty {
        spans.push(Span::styled("░", muted_style));
    }
    spans.push(Span::styled("]", muted_style));
    spans.push(Span::styled(format!(" ~{percent:02}%"), active_style));
    Line::from_spans(spans)
}

pub(super) fn render_compaction(
    entry: &super::TranscriptEntry,
    width: usize,
    theme: &TuiTheme,
    _activity_frame: usize,
) -> Vec<Line> {
    let super::TranscriptEntry::Compaction {
        phase,
        percent,
        compacted_message_count,
        tokens_before,
        tokens_after,
    } = entry
    else {
        return Vec::new();
    };
    let (phase, percent, compacted_message_count, tokens_before, tokens_after) = (
        *phase,
        *percent,
        *compacted_message_count,
        *tokens_before,
        *tokens_after,
    );
    let is_complete = percent >= 100 && phase == Some(neo_agent_core::CompactionPhase::Applying);
    if is_complete {
        let text = format!(
            "✔ Compaction complete: {compacted_message_count} messages · {} → {} tokens",
            super::format_token_count_usize(tokens_before),
            super::format_token_count_usize(tokens_after),
        );
        return super::styled_wrap(&text, width, Style::default().fg(theme.status_ok).bold());
    }

    let active_color = compaction_active_color(phase, theme);
    let short_phase_label = compaction_short_phase_label(phase);
    if width < 24 {
        return vec![Line::styled(
            format!("◈ compacting ~{percent:02}%"),
            Style::default().fg(active_color).bold(),
        )];
    }
    if width < 64 {
        let bar_width = if width >= 40 { 6 } else { 2 };
        return vec![compaction_progress_line(
            short_phase_label,
            percent,
            bar_width,
            active_color,
            theme,
        )];
    }

    let header = format!(
        "◈ Compacting context… {compacted_message_count} messages · {} tokens",
        super::format_token_count_usize(tokens_before)
    );
    let mut lines = super::styled_wrap(
        &header,
        width,
        Style::default().fg(theme.text_primary).bold(),
    );
    lines.push(compaction_progress_line(
        match phase {
            Some(neo_agent_core::CompactionPhase::Estimating) => "Estimating",
            Some(neo_agent_core::CompactionPhase::SelectingBoundary) => "Selecting boundary",
            Some(neo_agent_core::CompactionPhase::Summarizing) => "Summarizing",
            Some(neo_agent_core::CompactionPhase::Applying) => "Applying",
            None => "Compacting",
        },
        percent,
        12,
        active_color,
        theme,
    ));
    lines
}
