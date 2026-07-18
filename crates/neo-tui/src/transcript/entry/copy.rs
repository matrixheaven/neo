use super::{BannerData, GoalCardKind, TranscriptEntry};
use crate::primitive::theme::TuiTheme;
use crate::transcript::{
    DelegateCardComponent, DelegateGroupComponent, SwarmCardComponent, ToolCallComponent,
    WorkflowCardComponent,
};

pub(super) fn complex_copy_parts(entry: &TranscriptEntry) -> (&'static str, String) {
    if let Some(parts) = utility_copy_parts(entry) {
        return parts;
    }
    card_copy_parts(entry)
}

fn utility_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Banner(data) => Some(("Banner", copy_banner(data))),
        TranscriptEntry::ToolRun { component } => Some(("Tool", copy_tool(component))),
        TranscriptEntry::ShellRun { component } => Some(("Shell", component.copy_text())),
        TranscriptEntry::Compaction {
            compacted_message_count,
            tokens_before,
            tokens_after,
            ..
        } => Some((
            "Compact",
            copy_compaction(*compacted_message_count, *tokens_before, *tokens_after),
        )),
        _ => None,
    }
}

fn card_copy_parts(entry: &TranscriptEntry) -> (&'static str, String) {
    match entry {
        TranscriptEntry::GoalCard {
            kind,
            objective,
            detail,
            turns,
        } => (
            "Goal",
            copy_goal(*kind, objective, detail.as_deref(), *turns),
        ),
        TranscriptEntry::SkillActivation {
            names,
            source,
            outcome,
            body,
            ..
        } => ("Skill", copy_skill(names, *source, *outcome, body)),
        TranscriptEntry::UserMessage { .. }
        | TranscriptEntry::AssistantMessage { .. }
        | TranscriptEntry::ThinkingBlock { .. }
        | TranscriptEntry::ApprovalPrompt(_)
        | TranscriptEntry::Image { .. }
        | TranscriptEntry::Status { .. }
        | TranscriptEntry::RetryStatus { .. }
        | TranscriptEntry::McpStartupStatus { .. }
        | TranscriptEntry::QueuedMessage { .. } => unreachable!("simple copy parts handled above"),
        TranscriptEntry::Banner(_)
        | TranscriptEntry::ToolRun { .. }
        | TranscriptEntry::ShellRun { .. }
        | TranscriptEntry::Compaction { .. } => unreachable!("utility copy parts handled above"),
        TranscriptEntry::Delegate { component } => ("Agent", copy_delegate(component)),
        TranscriptEntry::DelegateGroup { component } => ("Agents", copy_delegate_group(component)),
        TranscriptEntry::DelegateSwarm { component } => ("Swarm", copy_swarm(component)),
        TranscriptEntry::Workflow { component } => ("Workflow", copy_workflow(component)),
        TranscriptEntry::InstructionEpoch { component } => ("Instructions", component.copy_text()),
    }
}

pub(super) fn simple_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    text_copy_parts(entry)
        .or_else(|| status_copy_parts(entry))
        .or_else(|| media_copy_parts(entry))
}

fn text_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::UserMessage { content, .. } => Some(("You", content.clone())),
        TranscriptEntry::AssistantMessage { content } => Some(("Assistant", content.clone())),
        TranscriptEntry::ThinkingBlock { content, .. } => Some(("Thinking", content.clone())),
        TranscriptEntry::QueuedMessage { text, is_steer } => {
            let label = if *is_steer { "Steer" } else { "Queued" };
            Some((label, text.clone()))
        }
        _ => None,
    }
}

fn status_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Status { text, .. } => Some(("Status", text.clone())),
        TranscriptEntry::RetryStatus { data } => Some(("Retry", data.message.clone())),
        TranscriptEntry::McpStartupStatus { data } => Some(("MCP", data.message())),
        TranscriptEntry::ApprovalPrompt(data) => Some(("Approval", data.title().to_owned())),
        _ => None,
    }
}

fn media_copy_parts(entry: &TranscriptEntry) -> Option<(&'static str, String)> {
    match entry {
        TranscriptEntry::Image { metadata, .. } => Some(("Image", metadata.clone())),
        _ => None,
    }
}

fn copy_banner(data: &BannerData) -> String {
    format!(
        "{}\nSession: {}\nModel: {}\nWorkspace: {}",
        data.title, data.session, data.model, data.directory
    )
}

fn copy_tool(component: &ToolCallComponent) -> String {
    let state = component.state();
    let detail = state
        .result
        .as_ref()
        .filter(|result| !result.is_empty())
        .or_else(|| {
            state
                .arguments
                .as_ref()
                .filter(|arguments| !arguments.is_empty())
        })
        .cloned()
        .unwrap_or_default();
    format!("{} {} ({detail})", state.status.marker(), state.name)
}

fn copy_compaction(
    compacted_message_count: usize,
    tokens_before: usize,
    tokens_after: usize,
) -> String {
    format!(
        "Compacted {compacted_message_count} messages · {} → {} tokens",
        super::format_token_count_usize(tokens_before),
        super::format_token_count_usize(tokens_after),
    )
}

fn copy_goal(
    kind: GoalCardKind,
    objective: &str,
    detail: Option<&str>,
    turns: Option<u32>,
) -> String {
    format!(
        "{kind:?} goal: {objective}\n{}\n{}",
        detail.unwrap_or(""),
        turns.map_or_else(String::new, |turn| format!("Turns: {turn}"))
    )
}

fn copy_skill(
    names: &[String],
    source: neo_agent_core::SkillInvocationSource,
    outcome: neo_agent_core::SkillInvocationOutcome,
    body: &str,
) -> String {
    let status = match outcome {
        neo_agent_core::SkillInvocationOutcome::Activated => "activated",
        neo_agent_core::SkillInvocationOutcome::Failed => "failed",
    };
    let source = match source {
        neo_agent_core::SkillInvocationSource::Auto => "auto",
        neo_agent_core::SkillInvocationSource::Manual => "manual",
    };
    let header = format!("Skill {status}: {} · {source}", names.join(", "));
    if body.trim().is_empty() {
        header
    } else {
        format!("{header}\n{}", body.trim())
    }
}

fn copy_delegate(component: &DelegateCardComponent) -> String {
    let lines = component.render_with_theme(200, &TuiTheme::default());
    lines
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy_delegate_group(component: &DelegateGroupComponent) -> String {
    let lines = component.render_with_theme(200, &TuiTheme::default());
    lines
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy_swarm(component: &SwarmCardComponent) -> String {
    let lines = component.render_with_theme(200, &TuiTheme::default());
    lines
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>()
        .join("\n")
}

fn copy_workflow(component: &WorkflowCardComponent) -> String {
    let lines = component.render_with_theme(200, &TuiTheme::default());
    lines
        .into_iter()
        .map(|line| line.text().clone())
        .collect::<Vec<_>>()
        .join("\n")
}
