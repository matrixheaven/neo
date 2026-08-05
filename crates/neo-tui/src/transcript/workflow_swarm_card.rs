use neo_agent_core::multi_agent::{AgentSnapshot, SwarmChildSnapshot, SwarmSnapshot};

use crate::primitive::theme::TuiTheme;
use crate::primitive::{Line, Style};

use super::workflow_delegate_card::{
    WorkflowSummaryRender, count_agents, render_agent_row, render_summary_header,
    selected_agent_indices,
};

struct SwarmChildRow<'a> {
    swarm: &'a SwarmSnapshot,
    child: &'a SwarmChildSnapshot,
}

#[must_use]
pub(super) fn swarm_counts(swarms: &[SwarmSnapshot]) -> super::workflow_delegate_card::AgentCounts {
    count_agents(
        swarms
            .iter()
            .flat_map(|swarm| swarm.children.iter().map(|child| &child.agent)),
    )
}

#[must_use]
pub(super) fn render_workflow_swarm_card(
    swarms: &[SwarmSnapshot],
    width: usize,
    max_rows: usize,
    now_ms: Option<u64>,
    theme: &TuiTheme,
) -> Option<WorkflowSummaryRender> {
    if swarms.is_empty() || max_rows == 0 {
        return None;
    }
    let mut children = swarms
        .iter()
        .flat_map(|swarm| {
            swarm
                .children
                .iter()
                .map(move |child| SwarmChildRow { swarm, child })
        })
        .collect::<Vec<_>>();
    if children.is_empty() {
        return None;
    }
    let counts = swarm_counts(swarms);
    let detail = if swarms.len() == 1 {
        Some(swarms[0].description.as_str())
    } else {
        None
    };
    let multiple_detail = (swarms.len() > 1).then(|| format!("{} groups", swarms.len()));
    let mut lines = vec![render_summary_header(
        "Workflow Swarms",
        multiple_detail.as_deref().or(detail),
        counts,
        width,
        theme,
    )];
    if max_rows == 1 {
        return Some(WorkflowSummaryRender { lines });
    }

    children.sort_by(|left, right| {
        left.swarm
            .swarm_id
            .cmp(&right.swarm.swarm_id)
            .then(left.child.item_index.cmp(&right.child.item_index))
    });
    let agent_refs = children
        .iter()
        .map(|row| &row.child.agent)
        .collect::<Vec<&AgentSnapshot>>();
    let content_rows = max_rows - 1;
    let pressured = content_rows < children.len();
    let visible_rows = if pressured {
        content_rows.saturating_sub(1)
    } else {
        content_rows
    };
    let indexes = selected_agent_indices(&agent_refs, visible_rows, pressured);
    let omitted = children.len().saturating_sub(indexes.len());
    let visible_count = indexes.len();
    for (visible_index, child_index) in indexes.into_iter().enumerate() {
        let row = &children[child_index];
        let is_last = visible_index + 1 == visible_count && omitted == 0;
        let identity_prefix = swarm_identity_prefix(swarms, row);
        lines.push(render_agent_row(
            &row.child.agent,
            if is_last { "└─ " } else { "├─ " },
            &identity_prefix,
            width,
            now_ms,
            theme,
        ));
    }
    if omitted > 0 && lines.len() < max_rows {
        lines.push(
            Line::styled(
                format!("└─ … {omitted} child rows omitted"),
                Style::default().fg(theme.text_muted),
            )
            .truncate_to_width(width),
        );
    }

    Some(WorkflowSummaryRender { lines })
}

fn swarm_identity_prefix(swarms: &[SwarmSnapshot], row: &SwarmChildRow<'_>) -> String {
    let item = row.child.item_index + 1;
    if swarms.len() == 1 {
        return format!("{item} · ");
    }
    let description = row.swarm.description.trim();
    let description_is_unique = !description.is_empty()
        && swarms
            .iter()
            .filter(|swarm| swarm.description.trim() == description)
            .count()
            == 1;
    let label = if description_is_unique {
        description
    } else {
        row.swarm.swarm_id.as_str()
    };
    format!("{label} / {item} · ")
}
