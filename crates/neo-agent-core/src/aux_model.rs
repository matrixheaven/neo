//! Shared request driver for auxiliary model calls (session titles,
//! compaction summaries).
//!
//! Auxiliary calls resolve their reasoning intensity through
//! [`aux_reasoning_selection`] instead of the interactive chat selection, and
//! retry once with the cheapest declared effort when a provider rejects an
//! explicit reasoning disable.

use futures::StreamExt;
use neo_ai::{
    AiError, AiStreamEvent, ChatMessage, ChatRequest, ModelClient, ModelSpec, ReasoningEffort,
    ReasoningSelection, RequestOptions, aux_reasoning_selection,
};
use tokio_util::sync::CancellationToken;

/// Stream one auxiliary completion and return the concatenated text.
///
/// Only `TextDelta` events are collected; thinking and tool lifecycle events
/// are ignored, matching the historical title-generation behavior.
///
/// `on_progress` receives the accumulated character count after each delta so
/// callers can drive progress UI; pass `None` for fire-and-forget calls.
///
/// # Retry contract
/// When the resolved plan sent an explicit reasoning disable and the provider
/// answered with a protocol error (the typical "cannot be disabled" 400), the
/// request is rebuilt once with the cheapest declared effort. Any other error
/// — and a second failure — is returned unchanged.
pub async fn aux_stream_text(
    client: &dyn ModelClient,
    model: &ModelSpec,
    messages: Vec<ChatMessage>,
    mut options: RequestOptions,
    policy: &neo_ai::AuxReasoning,
    cancel_token: Option<&CancellationToken>,
    mut on_progress: Option<&mut (dyn FnMut(usize) + Send)>,
) -> Result<String, AiError> {
    let capability = &model.capabilities.reasoning;
    let plan = aux_reasoning_selection(capability, policy);
    if plan.coerced {
        tracing::warn!(
            policy = %policy_display(policy),
            "auxiliary reasoning policy did not match the model capability; \
             using the resolved fallback plan"
        );
    }
    options.reasoning = plan.selection;
    options.disable_reasoning = plan.disable_reasoning;

    let request = ChatRequest {
        model: model.clone(),
        messages,
        tools: Vec::new(),
        options,
    };
    match collect_aux_text(client, &request, cancel_token, &mut on_progress).await {
        Ok(text) => Ok(text),
        Err(error)
            if plan.disable_reasoning
                && matches!(error, AiError::Protocol { .. })
                && !capability_effort_values(capability).is_empty() =>
        {
            tracing::warn!(
                error = %error,
                "provider rejected the auxiliary reasoning disable; retrying once \
                 with the cheapest declared effort"
            );
            let mut retry_request = request.clone();
            retry_request.options.reasoning = ReasoningSelection::Effort {
                effort: capability_effort_values(capability)[0].clone(),
            };
            retry_request.options.disable_reasoning = false;
            if cancel_token.is_some_and(|token| token.is_cancelled()) {
                return Err(AiError::Cancelled);
            }
            collect_aux_text(client, &retry_request, cancel_token, &mut on_progress).await
        }
        Err(error) => Err(error),
    }
}

fn capability_effort_values(capability: &neo_ai::ReasoningCapability) -> &[ReasoningEffort] {
    match capability {
        neo_ai::ReasoningCapability::Effort { values, .. }
        | neo_ai::ReasoningCapability::Combined { effort: values, .. } => values,
        neo_ai::ReasoningCapability::None
        | neo_ai::ReasoningCapability::Toggle { .. }
        | neo_ai::ReasoningCapability::BudgetTokens { .. } => &[],
    }
}

fn policy_display(policy: &neo_ai::AuxReasoning) -> String {
    match policy {
        neo_ai::AuxReasoning::Auto => "auto".to_owned(),
        neo_ai::AuxReasoning::Off => "off".to_owned(),
        neo_ai::AuxReasoning::Effort(value) => value.clone(),
    }
}

async fn collect_aux_text(
    client: &dyn ModelClient,
    request: &ChatRequest,
    cancel_token: Option<&CancellationToken>,
    on_progress: &mut Option<&mut (dyn FnMut(usize) + Send)>,
) -> Result<String, AiError> {
    let mut stream = client.stream_chat(request.clone());
    let mut text = String::new();
    while let Some(event) = stream.next().await {
        if cancel_token.is_some_and(|token| token.is_cancelled()) {
            return Err(AiError::Cancelled);
        }
        if let AiStreamEvent::TextDelta { text: delta } = event? {
            text.push_str(&delta);
            if let Some(callback) = on_progress {
                (**callback)(text.len());
            }
        }
    }
    Ok(text)
}

#[cfg(test)]
#[path = "test_cases/aux_model.rs"]
mod tests;
