use super::chat_request::workspace_context_message;
use super::config::AgentConfig;
use super::context::AgentContext;
use super::tokens::{
    estimate_message_tokens, estimate_messages_tokens, estimate_tool_specs_tokens,
};
use crate::compaction::projection::{ProjectionPlan, project_for_request, project_for_summary};
use crate::events::ContextWindowSource;

const SMALL_WINDOW_MAX_TOKENS: usize = 128_000;
const SMALL_WINDOW_TRIGGER_RATIO: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBudgetSnapshot {
    pub turn: u32,
    pub durable_tokens: usize,
    pub fixed_overhead_tokens: usize,
    pub tool_schema_tokens: usize,
    pub raw_effective_tokens: usize,
    pub projected_tokens: usize,
    pub max_context_tokens: Option<usize>,
    pub effective_max_context_tokens: Option<usize>,
    pub absolute_max_tokens: Option<usize>,
    pub trigger_tokens: Option<usize>,
    pub reserved_headroom_tokens: usize,
    pub remaining_to_trigger: Option<usize>,
    pub remaining_to_max: Option<usize>,
    pub source: ContextWindowSource,
    pub projection: ProjectionPlan,
}

pub struct ContextBudgetEstimator;

impl ContextBudgetEstimator {
    #[must_use]
    pub fn snapshot(
        config: &AgentConfig,
        context: &AgentContext,
        projection: ProjectionPlan,
    ) -> ContextBudgetSnapshot {
        let fixed_overhead_tokens = fixed_overhead_tokens(config, context);
        let tool_schema_tokens = *config
            .cached_tool_spec_tokens
            .get_or_init(|| estimate_tool_specs_tokens(&config.tools));
        let durable_tokens = context.estimated_tokens();
        let raw_effective_tokens = durable_tokens + fixed_overhead_tokens + tool_schema_tokens;
        let projected_tokens = projected_effective_tokens(
            context,
            projection,
            fixed_overhead_tokens,
            tool_schema_tokens,
        );
        let max_context_tokens = config
            .model
            .capabilities
            .max_context_tokens
            .map(|tokens| tokens as usize);
        let observed_max_context_tokens = *config
            .observed_max_context_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (effective_max_context_tokens, source) =
            effective_context_window(max_context_tokens, observed_max_context_tokens);
        let reserved_headroom_tokens = config
            .compaction
            .as_ref()
            .map_or(0, |settings| settings.reserved_context_tokens);
        let absolute_max_tokens = config
            .compaction
            .as_ref()
            .map(|settings| settings.max_estimated_tokens);
        let trigger_tokens = trigger_tokens(config, effective_max_context_tokens);
        let remaining_to_trigger =
            trigger_tokens.map(|tokens| tokens.saturating_sub(projected_tokens));
        let remaining_to_max =
            effective_max_context_tokens.map(|tokens| tokens.saturating_sub(projected_tokens));

        ContextBudgetSnapshot {
            turn: context.turns().saturating_add(1),
            durable_tokens,
            fixed_overhead_tokens,
            tool_schema_tokens,
            raw_effective_tokens,
            projected_tokens,
            max_context_tokens,
            effective_max_context_tokens,
            absolute_max_tokens,
            trigger_tokens,
            reserved_headroom_tokens,
            remaining_to_trigger,
            remaining_to_max,
            source,
            projection,
        }
    }
}

fn fixed_overhead_tokens(config: &AgentConfig, context: &AgentContext) -> usize {
    let system_tokens = config.system_prompt.as_ref().map_or(0, |prompt| {
        estimate_message_tokens(&crate::AgentMessage::system_text(prompt.as_str()))
    });
    let workspace_tokens =
        workspace_context_message(config).map_or(0, |message| estimate_message_tokens(&message));
    let transform_tokens = config
        .context_append_transform
        .as_ref()
        .map_or(0, |transform| {
            estimate_messages_tokens(&transform(context.messages()))
        });

    system_tokens + workspace_tokens + transform_tokens
}

fn projected_effective_tokens(
    context: &AgentContext,
    projection: ProjectionPlan,
    fixed_overhead_tokens: usize,
    tool_schema_tokens: usize,
) -> usize {
    let projected_message_tokens = match projection.mode {
        crate::compaction::projection::ProjectionMode::None => context.estimated_tokens(),
        crate::compaction::projection::ProjectionMode::Request => {
            project_for_request(context.messages(), &projection).projected_tokens
        }
        crate::compaction::projection::ProjectionMode::SummaryInput => {
            project_for_summary(context.messages(), &projection).projected_tokens
        }
    };

    projected_message_tokens + fixed_overhead_tokens + tool_schema_tokens
}

fn effective_context_window(
    configured: Option<usize>,
    observed: Option<usize>,
) -> (Option<usize>, ContextWindowSource) {
    match (configured, observed) {
        (Some(configured), Some(observed)) if observed < configured => {
            (Some(observed), ContextWindowSource::ObservedOverflow)
        }
        (Some(configured), _) => (Some(configured), ContextWindowSource::Configured),
        (None, Some(observed)) => (Some(observed), ContextWindowSource::ObservedOverflow),
        (None, None) => (None, ContextWindowSource::MissingModelWindow),
    }
}

fn trigger_tokens(
    config: &AgentConfig,
    effective_max_context_tokens: Option<usize>,
) -> Option<usize> {
    let max_tokens = effective_max_context_tokens?;
    let configured_ratio = config
        .compaction
        .as_ref()
        .map_or(SMALL_WINDOW_TRIGGER_RATIO, |settings| {
            settings.trigger_ratio
        });
    let ratio = if max_tokens <= SMALL_WINDOW_MAX_TOKENS {
        SMALL_WINDOW_TRIGGER_RATIO
    } else {
        configured_ratio
    };

    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    Some((max_tokens as f64 * ratio) as usize)
}

#[cfg(test)]
mod tests {
    use neo_ai::ToolSpec;

    use super::super::config::{AgentConfig, CompactionSettings, observe_context_overflow};
    use super::super::context::AgentContext;
    use super::*;
    use crate::compaction::projection::ProjectionPlan;
    use crate::harness::fake_model;
    use crate::{AgentMessage, Content};

    #[test]
    fn budget_includes_system_workspace_transform_and_tools() {
        let tool = ToolSpec {
            name: "LargeSchemaTool".to_owned(),
            description: "tool description ".repeat(80),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "payload": { "type": "string", "description": "schema ".repeat(160) }
                }
            }),
        };
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::user_text("history ".repeat(100)));
        let config = AgentConfig::for_model(fake_model())
            .with_system_prompt("system ".repeat(40))
            .with_tools(vec![tool])
            .with_context_append_transform(|_| {
                vec![AgentMessage::system_text("transform ".repeat(40))]
            })
            .with_compaction(CompactionSettings::new(usize::MAX, 4));

        let snapshot =
            ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

        assert!(snapshot.fixed_overhead_tokens > 0);
        assert!(snapshot.tool_schema_tokens > 0);
        assert!(snapshot.raw_effective_tokens > context.estimated_tokens());
    }

    #[test]
    fn budget_uses_observed_overflow_when_smaller() {
        let mut config = AgentConfig::for_model(fake_model())
            .with_compaction(CompactionSettings::new(usize::MAX, 4));
        config.model.capabilities.max_context_tokens = Some(200_000);
        observe_context_overflow(&config, 100_000);
        let context = AgentContext::new();

        let snapshot =
            ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

        assert_eq!(snapshot.effective_max_context_tokens, Some(85_000));
        assert_eq!(snapshot.source, ContextWindowSource::ObservedOverflow);
    }

    #[test]
    fn small_window_uses_lower_trigger_ratio() {
        let mut config = AgentConfig::for_model(fake_model())
            .with_compaction(CompactionSettings::new(usize::MAX, 4));
        config.model.capabilities.max_context_tokens = Some(64_000);
        let context = AgentContext::new();

        let snapshot =
            ContextBudgetEstimator::snapshot(&config, &context, ProjectionPlan::disabled());

        assert_eq!(snapshot.trigger_tokens, Some(51_200));
    }

    #[test]
    fn budget_uses_projected_tokens_for_remaining_counts() {
        let mut context = AgentContext::new();
        context.append_message(AgentMessage::tool_result(
            "old_call",
            "Read",
            vec![Content::text("x".repeat(16_000))],
            false,
        ));
        let mut config = AgentConfig::for_model(fake_model())
            .with_compaction(CompactionSettings::new(usize::MAX, 4));
        config.model.capabilities.max_context_tokens = Some(10_000);
        let plan = ProjectionPlan {
            enabled: true,
            cutoff_index: context.messages().len(),
            min_tool_result_tokens: 100,
            keep_recent_messages: 0,
            mode: crate::compaction::projection::ProjectionMode::Request,
        };

        let snapshot = ContextBudgetEstimator::snapshot(&config, &context, plan);

        assert!(snapshot.projected_tokens < snapshot.raw_effective_tokens);
        assert_eq!(
            snapshot.remaining_to_max,
            Some(10_000usize.saturating_sub(snapshot.projected_tokens))
        );
    }
}
