use crate::primitive::Line;
use crate::primitive::theme::TuiTheme;

use super::WorkflowCardComponent;
use super::workflow_delegate_card::{count_agents, render_workflow_delegate_card};
use super::workflow_swarm_card::{render_workflow_swarm_card, swarm_counts};

pub(crate) struct WorkflowGroupRender {
    main: Vec<Line>,
    delegates: Option<Vec<Line>>,
    swarms: Option<Vec<Line>>,
}

impl WorkflowGroupRender {
    #[must_use]
    pub(crate) fn into_lines(self) -> Vec<Line> {
        let mut lines = self.main;
        if let Some(delegates) = self.delegates {
            lines.extend(delegates);
        }
        if let Some(swarms) = self.swarms {
            lines.extend(swarms);
        }
        lines
    }
}

#[must_use]
pub(crate) fn render_workflow_group(
    component: &WorkflowCardComponent,
    width: usize,
    available_rows: usize,
    theme: &TuiTheme,
) -> WorkflowGroupRender {
    if available_rows == 0 {
        return WorkflowGroupRender {
            main: Vec::new(),
            delegates: None,
            swarms: None,
        };
    }

    let has_delegates = !component.delegates().is_empty();
    let has_swarms = component
        .swarms()
        .iter()
        .any(|swarm| !swarm.children.is_empty());
    let minimum_rows = 2 + usize::from(has_delegates) + usize::from(has_swarms);
    if available_rows < minimum_rows {
        let folded_counts = folded_child_counts(component);
        let main = component.render_main_with_theme(
            width,
            available_rows,
            folded_counts.as_deref(),
            theme,
        );
        return WorkflowGroupRender {
            main: main.0,
            delegates: None,
            swarms: None,
        };
    }

    let main_full = component
        .render_main_with_theme(width, usize::MAX, None, theme)
        .0
        .len();
    let delegate_full = render_workflow_delegate_card(
        component.delegates(),
        width,
        usize::MAX,
        component.now_ms(),
        theme,
    )
    .map_or(0, |rendered| rendered.lines.len());
    let swarm_full = render_workflow_swarm_card(
        component.swarms(),
        width,
        usize::MAX,
        component.now_ms(),
        theme,
    )
    .map_or(0, |rendered| rendered.lines.len());
    let desired = [main_full, delegate_full, swarm_full];
    let mut budgets = [2, usize::from(has_delegates), usize::from(has_swarms)];
    let mut remaining = available_rows.saturating_sub(minimum_rows);
    while remaining > 0 {
        let mut changed = false;
        for index in 0..budgets.len() {
            if budgets[index] < desired[index] {
                budgets[index] += 1;
                remaining -= 1;
                changed = true;
                if remaining == 0 {
                    break;
                }
            }
        }
        if !changed {
            break;
        }
    }

    let main = component.render_main_with_theme(width, budgets[0], None, theme);
    let delegates = render_workflow_delegate_card(
        component.delegates(),
        width,
        budgets[1],
        component.now_ms(),
        theme,
    );
    let swarms = render_workflow_swarm_card(
        component.swarms(),
        width,
        budgets[2],
        component.now_ms(),
        theme,
    );
    WorkflowGroupRender {
        main: main.0,
        delegates: delegates.map(|rendered| rendered.lines),
        swarms: swarms.map(|rendered| rendered.lines),
    }
}

fn folded_child_counts(component: &WorkflowCardComponent) -> Option<String> {
    let mut parts = Vec::new();
    if !component.delegates().is_empty() {
        parts.push(format!(
            "Delegates {}",
            count_agents(component.delegates()).text()
        ));
    }
    if component
        .swarms()
        .iter()
        .any(|swarm| !swarm.children.is_empty())
    {
        let counts = swarm_counts(component.swarms());
        if counts.total() > 0 {
            parts.push(format!("Swarms {}", counts.text()));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}
