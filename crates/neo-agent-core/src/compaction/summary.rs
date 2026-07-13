use std::sync::Arc;

use neo_ai::ModelClient;
use tokio_util::sync::CancellationToken;

use super::{
    CompactionError, CompactionSource, CompactionStrategy, compute_compact_count,
    generate_compaction_summary,
};
use crate::compaction::projection::project_for_summary;
use crate::events::{AgentEvent, CompactionPhase, CompactionReason};
use crate::runtime::context_budget::ContextBudgetSnapshot;
use crate::runtime::{estimate_message_tokens, estimate_messages_tokens};
use crate::{AgentConfig, AgentContext, CompactionSummary};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionOutcome {
    pub compacted_message_count: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub summary_tokens: usize,
    pub projection_omitted_tokens: usize,
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn run_full_compaction<F>(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    context: &mut AgentContext,
    reason: CompactionReason,
    snapshot: ContextBudgetSnapshot,
    custom_instruction: Option<&str>,
    cancel_token: &CancellationToken,
    mut emit: F,
) -> Result<CompactionOutcome, CompactionError>
where
    F: FnMut(AgentEvent) + Send,
{
    let Some(settings) = &config.compaction else {
        return Ok(CompactionOutcome {
            compacted_message_count: 0,
            tokens_before: snapshot.projected_tokens,
            tokens_after: snapshot.projected_tokens,
            summary_tokens: 0,
            projection_omitted_tokens: 0,
        });
    };
    if !settings.enabled {
        return Ok(CompactionOutcome {
            compacted_message_count: 0,
            tokens_before: snapshot.projected_tokens,
            tokens_after: snapshot.projected_tokens,
            summary_tokens: 0,
            projection_omitted_tokens: 0,
        });
    }

    let messages = context.messages().to_vec();
    let strategy = CompactionStrategy {
        trigger_ratio: settings.trigger_ratio,
        max_recent_messages: settings
            .keep_recent_messages
            .min(settings.max_recent_messages),
        max_recent_size_ratio: 0.2,
        reserved_context_tokens: settings.reserved_context_tokens,
    };
    let source = match reason {
        CompactionReason::Manual => CompactionSource::Manual,
        CompactionReason::Threshold => CompactionSource::Auto,
    };
    let compacted_count = compute_compact_count(
        &messages,
        source,
        &strategy,
        snapshot.effective_max_context_tokens.unwrap_or(0),
    );
    if compacted_count == 0 {
        return Err(CompactionError::NoBoundary);
    }

    emit(AgentEvent::CompactionStarted {
        reason,
        tokens_before: snapshot.projected_tokens,
        message_count: messages.len(),
    });
    emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Estimating,
        percent: 0,
    });

    let messages_to_compact = &messages[..compacted_count];
    let projection = project_for_summary(messages_to_compact, &snapshot.projection);

    emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::SelectingBoundary,
        percent: 15,
    });
    emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Summarizing,
        percent: 15,
    });

    let summary_text = generate_compaction_summary(
        model,
        config,
        &projection.messages,
        custom_instruction,
        cancel_token,
        |summary_chars| {
            if summary_chars > 0 {
                emit(AgentEvent::CompactionProgress {
                    phase: CompactionPhase::Summarizing,
                    percent: 85,
                });
            }
        },
    )
    .await?;

    let kept_messages = &messages[compacted_count..];
    let summary_tokens =
        estimate_message_tokens(&crate::AgentMessage::system_text(summary_text.as_str()));
    let tokens_after = snapshot.fixed_overhead_tokens
        + snapshot.tool_schema_tokens
        + summary_tokens
        + estimate_messages_tokens(kept_messages);
    let summary = CompactionSummary {
        summary: summary_text,
        tokens_before: snapshot.projected_tokens,
        tokens_after,
        first_kept_message_index: compacted_count,
    };

    emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Applying,
        percent: 100,
    });
    emit(AgentEvent::CompactionApplied {
        summary: summary.clone(),
    });
    context.apply_compaction(summary);

    Ok(CompactionOutcome {
        compacted_message_count: compacted_count,
        tokens_before: snapshot.projected_tokens,
        tokens_after,
        summary_tokens,
        projection_omitted_tokens: projection.omitted_tokens,
    })
}

#[cfg(test)]
mod tests {
    use neo_ai::AiStreamEvent;
    use tokio_util::sync::CancellationToken;

    use crate::compaction::projection::{ProjectionMode, ProjectionPlan};
    use crate::events::CompactionReason;
    use crate::harness::FakeHarness;
    use crate::runtime::context_budget::ContextBudgetEstimator;
    use crate::{
        AgentConfig, AgentMessage, AgentToolCall, CompactionSettings, Content, StopReason,
    };

    use super::*;

    fn fake_summary_harness() -> FakeHarness {
        FakeHarness::from_events([
            AiStreamEvent::MessageStart {
                id: "summary".to_owned(),
            },
            AiStreamEvent::TextDelta {
                text: "summary".to_owned(),
            },
            AiStreamEvent::MessageEnd {
                stop_reason: neo_ai::StopReason::EndTurn,
                usage: None,
            },
        ])
    }

    fn context_with_old_large_tool_result() -> crate::AgentContext {
        let mut context = crate::AgentContext::new();
        context.append_message(AgentMessage::assistant(
            Vec::new(),
            vec![AgentToolCall {
                id: "call".into(),
                name: "Read".into(),
                raw_arguments: "{}".into(),
            }],
            StopReason::ToolUse,
        ));
        context.append_message(AgentMessage::tool_result(
            "call",
            "Read",
            vec![Content::text("x".repeat(16_000))],
            false,
        ));
        context.append_message(AgentMessage::user_text("after"));
        context
    }

    #[tokio::test]
    async fn full_compaction_uses_summary_projection_before_llm_request() {
        let harness = fake_summary_harness();
        let model = harness.client();
        let mut context = context_with_old_large_tool_result();
        let config =
            AgentConfig::for_model(harness.model()).with_compaction(CompactionSettings::new(1, 1));
        let snapshot = ContextBudgetEstimator::snapshot(
            &config,
            &context,
            ProjectionPlan {
                enabled: true,
                cutoff_index: context.messages().len(),
                min_tool_result_tokens: 100,
                keep_recent_messages: 0,
                mode: ProjectionMode::SummaryInput,
            },
        );

        let outcome = run_full_compaction(
            &model,
            &config,
            &mut context,
            CompactionReason::Threshold,
            snapshot,
            None,
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .expect("compaction should succeed");

        assert!(outcome.projection_omitted_tokens > 0);
        assert!(context.compaction_summary().is_some());
    }
}
