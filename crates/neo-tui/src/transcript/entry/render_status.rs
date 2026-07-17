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
) -> Vec<Line> {
    let (header, color) = match data.phase {
        RetryPhase::Waiting => {
            let elapsed_ms = super::monotonic_time_ms().saturating_sub(data.started_at_ms);
            let remaining_ms = data.delay_ms.saturating_sub(elapsed_ms);
            (
                format!(
                    "Reconnecting {}/{} · retry in {} · esc interrupt",
                    data.retry,
                    data.max_retries,
                    format_retry_delay(remaining_ms)
                ),
                theme.status_warn,
            )
        }
        RetryPhase::Connecting => (
            format!(
                "Reconnecting {}/{} · connecting · esc interrupt",
                data.retry, data.max_retries
            ),
            theme.brand,
        ),
        RetryPhase::Exhausted => (
            format!(
                "Reconnect failed after {} {}",
                data.retry,
                if data.retry == 1 {
                    "attempt"
                } else {
                    "attempts"
                }
            ),
            theme.status_error,
        ),
    };
    let mut lines = super::styled_wrap(&header, width, Style::default().fg(color).bold());
    if !data.message.is_empty() {
        let title = if data.error_code == "provider.transport_error" {
            "Network"
        } else {
            let title = neo_agent_core::error_info(&data.error_code).title;
            title.strip_suffix(" Error").unwrap_or(title)
        };
        lines.extend(super::styled_wrap(
            &format!("{title} · {}", data.message),
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

fn compaction_pulse_char(activity_frame: usize) -> char {
    // A subtle shimmer on the leading edge of the filled bar.
    const PULSE: &[char] = &['▓', '▒', '▓', '█'];
    PULSE[activity_frame % PULSE.len()]
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_compaction(
    phase: Option<neo_agent_core::CompactionPhase>,
    percent: u8,
    compacted_message_count: usize,
    tokens_before: usize,
    tokens_after: usize,
    width: usize,
    theme: &TuiTheme,
    activity_frame: usize,
) -> Vec<Line> {
    let is_complete = percent >= 100 && phase == Some(neo_agent_core::CompactionPhase::Applying);
    if is_complete {
        let text = format!(
            "✔ Compaction complete: {compacted_message_count} messages · {} → {} tokens",
            super::format_token_count_usize(tokens_before),
            super::format_token_count_usize(tokens_after),
        );
        return super::styled_wrap(&text, width, Style::default().fg(theme.status_ok).bold());
    }

    let phase_label = phase.map_or_else(
        || "Compacting".to_owned(),
        |phase| match phase {
            neo_agent_core::CompactionPhase::Estimating => "Estimating".to_owned(),
            neo_agent_core::CompactionPhase::SelectingBoundary => "Selecting boundary".to_owned(),
            neo_agent_core::CompactionPhase::Summarizing => "Summarizing".to_owned(),
            neo_agent_core::CompactionPhase::Applying => "Applying".to_owned(),
        },
    );

    // Warm-up -> working -> almost done colour progression.
    let (label_color, bar_color) = match percent {
        0..=29 => (theme.status_warn, theme.status_warn),
        30..=69 => (theme.brand, theme.brand),
        _ => (theme.status_ok, theme.status_ok),
    };

    let bar_width = 12;
    let filled = ((percent as usize).min(100) * bar_width).div_ceil(100);
    let empty = bar_width.saturating_sub(filled);

    // Header: neutral product colour for context, bold for visibility.
    let mut lines = Vec::new();
    let header = format!(
        "◈ Compacting context… {compacted_message_count} messages · {} tokens",
        super::format_token_count_usize(tokens_before)
    );
    lines.extend(super::styled_wrap(
        &header,
        width,
        Style::default().fg(theme.text_primary).bold(),
    ));

    // Progress line: fixed ◈ icon, phase label, animated bar, percentage.
    let mut spans = vec![
        Span::styled("◈ ", Style::default().fg(theme.brand).bold()),
        Span::styled(
            format!("{phase_label} "),
            Style::default().fg(label_color).bold(),
        ),
        Span::styled("[", Style::default().fg(theme.text_muted)),
    ];

    // Filled portion with a subtle pulse on the leading edge.
    if filled > 0 {
        let pulse_char = compaction_pulse_char(activity_frame);
        for i in 0..filled {
            let ch = if i == filled - 1 { pulse_char } else { '█' };
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().fg(bar_color).bold(),
            ));
        }
    }

    // Empty portion.
    for _ in 0..empty {
        spans.push(Span::styled("░", Style::default().fg(theme.text_muted)));
    }

    spans.push(Span::styled("]", Style::default().fg(theme.text_muted)));
    spans.push(Span::styled(
        format!(" {percent}%"),
        Style::default().fg(label_color).bold(),
    ));
    lines.push(Line::from_spans(spans));

    lines
}
