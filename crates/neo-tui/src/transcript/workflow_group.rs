use neo_agent_core::session::ToolOutputStore;

use crate::primitive::Line;
use crate::primitive::theme::TuiTheme;

use super::WorkflowCardComponent;
use super::store::ExpandedOutputCache;
use super::workflow_delegate_card::render_workflow_delegate_card;
use super::workflow_swarm_card::render_workflow_swarm_card;

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

/// Render the complete Workflow group: the main card, the optional Delegate
/// summary, and the optional DelegateSwarm summary, always in that fixed
/// sibling order.
///
/// No terminal-height budget is applied and no child rows are omitted: every
/// structural row renders into the document, and the document owns scrolling
/// and the visible window. Expanded direct tools read their bounded visible
/// complete-output range through `output_store` (never the complete file).
#[must_use]
pub(crate) fn render_workflow_group(
    component: &WorkflowCardComponent,
    width: usize,
    theme: &TuiTheme,
    output_store: Option<&ToolOutputStore>,
    viewport_rows: usize,
    output_cache: &mut ExpandedOutputCache,
) -> WorkflowGroupRender {
    WorkflowGroupRender {
        main: component.render_main_with_theme(
            width,
            theme,
            output_store,
            viewport_rows,
            output_cache,
        ),
        delegates: render_workflow_delegate_card(
            component.delegates(),
            width,
            component.now_ms(),
            theme,
        )
        .map(|rendered| rendered.lines),
        swarms: render_workflow_swarm_card(component.swarms(), width, component.now_ms(), theme)
            .map(|rendered| rendered.lines),
    }
}
