use std::sync::Arc;
use std::time::Duration;

use neo_ai::ModelClient;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::config::{AgentConfig, CompactionSettings};
use super::error::AgentRuntimeError;
use super::events::EventEmitter;
use super::turn_loop::emit_effective_context_window;
use crate::compaction::{self, CompactionStrategy};
use crate::{
    AgentEvent, AgentMessage, CompactionPhase, CompactionReason, CompactionSource,
    CompactionSummary, sanitize_tool_exchange_messages,
};

pub(super) async fn maybe_compact(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    emitter: &mut EventEmitter,
    cancel_token: &CancellationToken,
) {
    const MIN_REDUCTION_TOKENS: usize = 1024;
    let max_rounds = config.compaction.as_ref().map_or(5, |s| s.max_rounds);

    for round in 0..max_rounds {
        let Some(trigger) = evaluate_compaction_need(config, emitter) else {
            break;
        };

        let compacted_count = compute_compacted_count(&trigger);
        if compacted_count == 0 {
            if trigger.force {
                let _ = emitter.send_error(AgentRuntimeError::Compaction(
                    compaction::CompactionError::NoBoundary,
                ));
            }
            break;
        }

        let (_, target_summary_chars) = emit_compaction_started(
            emitter,
            &trigger.messages,
            compacted_count,
            trigger.force,
            trigger.used_tokens,
            round == 0,
        )
        .await;

        let strategy = build_compaction_strategy(&trigger.settings);
        let (mut progress_rx, mut summary_rx) = spawn_summary_task(
            model,
            config,
            // Pass the full message history — generate_with_retry computes its own
            // split boundary internally (which may shrink on retry).
            &trigger.messages,
            trigger.custom_instruction.as_deref(),
            cancel_token,
            &strategy,
            trigger.max_context_tokens,
        );

        let Some(((summary_text, actual_compacted_count), progress_percent)) =
            run_summary_progress_loop(
                emitter,
                &mut progress_rx,
                &mut summary_rx,
                target_summary_chars,
            )
            .await
        else {
            break;
        };

        let tokens_before = trigger.used_tokens;
        apply_compaction_result(
            emitter,
            config,
            &trigger.messages,
            actual_compacted_count,
            summary_text,
            trigger.used_tokens,
            progress_percent,
        )
        .await;

        // Stop if the round produced too little reduction (diminishing returns).
        let tokens_after = super::estimate_messages_tokens(emitter.context.messages());
        if tokens_before.saturating_sub(tokens_after) < MIN_REDUCTION_TOKENS {
            break;
        }
    }
}

/// Bundled information produced by [`evaluate_compaction_need`] when compaction
/// should proceed.
struct CompactionTrigger {
    force: bool,
    custom_instruction: Option<String>,
    messages: Vec<AgentMessage>,
    used_tokens: usize,
    max_context_tokens: usize,
    settings: CompactionSettings,
}

/// Build the [`CompactionStrategy`] derived from [`CompactionSettings`].
fn build_compaction_strategy(settings: &CompactionSettings) -> CompactionStrategy {
    CompactionStrategy {
        trigger_ratio: settings.trigger_ratio,
        // Use keep_recent_messages as the auto-compaction retention limit so
        // the configured value directly controls how many messages survive.
        max_recent_messages: settings
            .keep_recent_messages
            .min(settings.max_recent_messages),
        max_recent_size_ratio: 0.2,
        reserved_context_tokens: settings.reserved_context_tokens,
    }
}

/// Evaluate whether compaction should run based on settings, a manual request,
/// and token thresholds. Returns `None` when no compaction is needed.
fn evaluate_compaction_need(
    config: &AgentConfig,
    emitter: &EventEmitter,
) -> Option<CompactionTrigger> {
    let settings = config.compaction?;
    if !settings.enabled {
        return None;
    }

    let (force, custom_instruction) = match config.manual_compact_request.lock() {
        Ok(mut request) => {
            let instruction = request.take();
            (instruction.is_some(), instruction)
        }
        Err(poisoned) => {
            let instruction = poisoned.into_inner().take();
            (instruction.is_some(), instruction)
        }
    };

    // Clone the messages out of the context so we can borrow `emitter` mutably
    // for event emission while still referencing the pre-compaction history.
    // Drop any trailing incomplete tool turn first so it is not treated as a
    // safe suffix boundary.
    let messages = sanitize_tool_exchange_messages(emitter.context.messages());
    let max_context_tokens = super::config::effective_max_context_tokens(config);
    let used_tokens = super::estimate_messages_tokens(&messages);

    let strategy = build_compaction_strategy(&settings);

    // Trigger compaction when:
    // 1. Manually requested via `/compact`, OR
    // 2. Token estimate exceeds the configured absolute threshold, OR
    // 3. Token estimate exceeds the ratio-based threshold of max_context_tokens.
    let ratio_triggered = strategy.should_compact(used_tokens, max_context_tokens);
    let absolute_triggered = used_tokens > settings.max_estimated_tokens;
    if !force && !ratio_triggered && !absolute_triggered {
        return None;
    }

    Some(CompactionTrigger {
        force,
        custom_instruction,
        messages,
        used_tokens,
        max_context_tokens,
        settings,
    })
}

/// Compute how many leading messages to compact. Only applies the fit-to-window
/// constraint when the model actually advertises a context window.
fn compute_compacted_count(trigger: &CompactionTrigger) -> usize {
    let source = if trigger.force {
        CompactionSource::Manual
    } else {
        CompactionSource::Auto
    };
    let strategy = build_compaction_strategy(&trigger.settings);
    compaction::compute_compact_count(
        &trigger.messages,
        source,
        &strategy,
        // Only apply the fit-to-window constraint when the model actually
        // advertises a context window. The trigger threshold
        // (max_estimated_tokens) is NOT the window — it's the compaction
        // trigger point — so passing it as the fit window would shrink
        // compaction to near-zero.
        trigger.max_context_tokens,
    )
}

/// Emit `CompactionStarted` (only when `first_round` is true) and the early
/// progress phases, then compute the messages to compact and the target summary
/// size.
async fn emit_compaction_started(
    emitter: &mut EventEmitter,
    messages: &[AgentMessage],
    compacted_count: usize,
    force: bool,
    used_tokens: usize,
    first_round: bool,
) -> (Vec<AgentMessage>, usize) {
    let reason = if force {
        CompactionReason::Manual
    } else {
        CompactionReason::Threshold
    };
    let message_count = messages.len();
    if first_round {
        emitter.emit(AgentEvent::CompactionStarted {
            reason,
            tokens_before: used_tokens,
            message_count,
        });
    }
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Estimating,
        percent: 0,
    });

    // Brief pause so the near-instant Estimating phase is visible in the TUI.
    tokio::time::sleep(Duration::from_millis(120)).await;

    let messages_to_compact = messages[..compacted_count].to_vec();
    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::SelectingBoundary,
        percent: 15,
    });

    // Brief pause so SelectingBoundary is visible too.
    tokio::time::sleep(Duration::from_millis(120)).await;

    emitter.emit(AgentEvent::CompactionProgress {
        phase: CompactionPhase::Summarizing,
        percent: 15,
    });

    // Estimate the target summary size from the rendered input so the progress
    // bar can advance proportionally to the streaming LLM output, similar to
    // kimi-code's swarm progress estimator.
    let rendered_input_chars = compaction::render_messages_to_text(&messages_to_compact).len();
    let target_summary_chars = (rendered_input_chars / 10).max(500);

    (messages_to_compact, target_summary_chars)
}

/// Spawn the summary LLM in its own task and return the progress and result
/// channels.
///
/// Uses [`compaction::generate_with_retry`] internally, which retries on
/// empty/truncated summaries by shrinking the prefix. The result carries both
/// the summary text and the actual compacted count (which may differ from the
/// initial count after prefix shrink).
#[allow(clippy::type_complexity)]
fn spawn_summary_task(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages: &[AgentMessage],
    custom_instruction: Option<&str>,
    cancel_token: &CancellationToken,
    strategy: &CompactionStrategy,
    max_context_tokens: usize,
) -> (
    mpsc::UnboundedReceiver<usize>,
    oneshot::Receiver<Result<(String, usize), compaction::CompactionError>>,
) {
    let (progress_tx, progress_rx) = mpsc::unbounded_channel::<usize>();
    let (summary_tx, summary_rx) =
        oneshot::channel::<Result<(String, usize), compaction::CompactionError>>();
    let summary_model = Arc::clone(model);
    let summary_config = config.clone();
    let summary_messages = messages.to_vec();
    let summary_instruction = custom_instruction.map(str::to_owned);
    let summary_cancel = cancel_token.child_token();
    let summary_strategy = strategy.clone();
    let max_retry_attempts = config
        .compaction
        .as_ref()
        .map_or(5, |s| s.max_retry_attempts);
    let progress_tx_for_task = progress_tx.clone();
    tokio::spawn(async move {
        let result = compaction::generate_with_retry(
            &summary_model,
            &summary_config,
            &summary_messages,
            &summary_strategy,
            max_context_tokens,
            summary_instruction.as_deref(),
            &summary_cancel,
            max_retry_attempts,
            |len| {
                let _ = progress_tx_for_task.send(len);
            },
        )
        .await;
        let _ = summary_tx.send(result);
        drop(progress_tx);
    });
    (progress_rx, summary_rx)
}

/// Drive the progress bar while waiting for the summary LLM to complete.
/// Returns `None` (after sending an error) on failure, or `Some(((text,
/// actual_compacted_count), progress_percent))` on success.
async fn run_summary_progress_loop(
    emitter: &mut EventEmitter,
    progress_rx: &mut mpsc::UnboundedReceiver<usize>,
    summary_rx: &mut oneshot::Receiver<Result<(String, usize), compaction::CompactionError>>,
    target_summary_chars: usize,
) -> Option<((String, usize), u8)> {
    let mut progress_percent: u8 = 15;
    let mut last_emitted_percent = progress_percent;
    let mut progress_tick = tokio::time::interval(Duration::from_millis(200));
    progress_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Skip the immediate first tick.
    let _ = progress_tick.tick().await;

    let summary_result: (String, usize);
    loop {
        tokio::select! {
            _ = progress_tick.tick() => {
                // Keep the bar inching forward even if the model stalls.
                if progress_percent < 84 {
                    progress_percent += 1;
                    if progress_percent != last_emitted_percent {
                        last_emitted_percent = progress_percent;
                        emitter.emit(AgentEvent::CompactionProgress {
                            phase: CompactionPhase::Summarizing,
                            percent: progress_percent,
                        });
                    }
                }
            }
            Some(summary_chars) = progress_rx.recv() => {
                // Map growing summary length to 15..=85%.
                let stream_percent = 15 + ((summary_chars.min(target_summary_chars) * 70)
                    .div_ceil(target_summary_chars))
                    .min(70);
                progress_percent = progress_percent.max(u8::try_from(stream_percent).unwrap_or(u8::MAX));
                if progress_percent != last_emitted_percent {
                    last_emitted_percent = progress_percent;
                    emitter.emit(AgentEvent::CompactionProgress {
                        phase: CompactionPhase::Summarizing,
                        percent: progress_percent,
                    });
                }
            }
            result = &mut *summary_rx => {
                match result {
                    Ok(Ok(summary)) => summary_result = summary,
                    Ok(Err(err)) => {
                        let _ = emitter.send_error(AgentRuntimeError::Compaction(err));
                        return None;
                    }
                    Err(_) => {
                        let _ = emitter.send_error(AgentRuntimeError::Compaction(
                            compaction::CompactionError::Llm("summary task aborted".to_owned()),
                        ));
                        return None;
                    }
                }
                break;
            }
        }
    }

    // Drain any progress update that arrived just before the summary completed
    // so the bar reaches its streamed cap before switching to Applying.
    while let Ok(summary_chars) = progress_rx.try_recv() {
        let stream_percent = 15
            + ((summary_chars.min(target_summary_chars) * 70).div_ceil(target_summary_chars))
                .min(70);
        progress_percent = progress_percent.max(u8::try_from(stream_percent).unwrap_or(u8::MAX));
        if progress_percent != last_emitted_percent {
            last_emitted_percent = progress_percent;
            emitter.emit(AgentEvent::CompactionProgress {
                phase: CompactionPhase::Summarizing,
                percent: progress_percent,
            });
        }
    }

    Some((summary_result, progress_percent))
}

/// Build the [`CompactionSummary`], animate the progress bar to 100%, emit
/// `CompactionApplied`, and refresh the effective context window display.
///
/// The `messages` parameter is the snapshot taken before the summary task was
/// spawned. If the live context has changed since (undo/clear during the LLM
/// call), the stale summary is discarded silently.
async fn apply_compaction_result(
    emitter: &mut EventEmitter,
    config: &AgentConfig,
    messages: &[AgentMessage],
    compacted_count: usize,
    summary_text: String,
    used_tokens: usize,
    mut progress_percent: u8,
) {
    // Staleness check: if the user undid/cleared messages while the summary
    // was being generated, applying it now would clobber the new state.
    let current_messages = emitter.context.messages();
    if compaction::is_stale(messages, current_messages) {
        return;
    }

    let kept_messages = &messages[compacted_count..];
    let tokens_after =
        summary_text.len().div_ceil(4) + super::estimate_messages_tokens(kept_messages);

    let summary = CompactionSummary {
        summary: summary_text,
        tokens_before: used_tokens,
        tokens_after,
        first_kept_message_index: compacted_count,
    };

    // Smoothly animate from the streamed cap to 100% so the user does not see
    // a frozen bar followed by an abrupt jump to complete.
    let animation_steps: u8 = 15;
    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
    let step = (f32::from(100 - progress_percent) / f32::from(animation_steps)).ceil() as u8;
    for _ in 0..animation_steps {
        if progress_percent >= 100 {
            break;
        }
        progress_percent = (progress_percent + step).min(100);
        tokio::time::sleep(Duration::from_millis(20)).await;
        emitter.emit(AgentEvent::CompactionProgress {
            phase: CompactionPhase::Applying,
            percent: progress_percent,
        });
    }

    emitter.emit(AgentEvent::CompactionApplied { summary });

    let turn = emitter.context.turns.saturating_add(1);
    emit_effective_context_window(config, emitter, turn).await;
}
