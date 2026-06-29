# Spec C: Context Compaction Enhancement

**Date:** 2026-06-29
**Status:** Approved (design phase)
**Crates affected:** `neo-agent-core`, `neo-agent`

## Motivation

Neo's compaction system has a solid foundation (ported from kimi-code's `agent/compaction/`) but is missing 5 key capabilities that kimi-code's `FullCompaction` class has:

1. **Single-round only.** If one compaction pass doesn't reduce tokens below the threshold, the agent runs with an over-long context anyway. kimi-code runs multiple rounds until the threshold is met or reduction is negligible.
2. **No overflow recovery.** When the provider returns a context-overflow error, the error propagates immediately. kimi-code catches it, triggers compaction, and retries.
3. **No staleness detection.** If the user undoes or clears history during the LLM summary call, the stale summary is applied to the wrong context. kimi-code snapshots history before the call and aborts if it changed.
4. **Hard-fail on empty summary.** An empty LLM response immediately fails. kimi-code retries with a shrunk prefix up to 5 times.
5. **No observed-max adaptation.** The configured `max_context_tokens` may be inaccurate. kimi-code records the actual overflow point and adapts thresholds downward.

Reference: kimi-code's `full.ts` (498 lines) + `strategy.ts` (220 lines) + 53 test cases in `full.test.ts` (2338 lines).

## Design

All changes are incremental on neo's existing `compaction/mod.rs` — no rewrite.

### C-1: Multi-Round Compaction

**Current:** `run_compaction()` runs one pass and returns.

**After:** loops up to `MAX_COMPACTION_ROUNDS` times, re-checking the threshold after each pass:

```rust
const MAX_COMPACTION_ROUNDS: usize = 5;
const MIN_REDUCTION_TOKENS: usize = 1024;

pub async fn run_compaction(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    context: &mut AgentContext,
    events: &mut Vec<AgentEvent>,
    source: CompactionSource,
    cancel_token: &CancellationToken,
) -> Result<bool, CompactionError> {
    let strategy = CompactionStrategy::from_config(config);
    let force = matches!(source, CompactionSource::Manual);
    let mut round = 0;
    let mut total_compacted = false;

    loop {
        round += 1;
        if round > MAX_COMPACTION_ROUNDS {
            break;
        }

        let messages = context.messages();
        let used_tokens = estimate_messages_tokens(messages);
        let max_tokens = effective_max_context_tokens(config);

        // Check if compaction is still needed
        if !force && !strategy.should_compact(used_tokens, max_tokens) {
            break;
        }

        // Compute safe split boundary
        let compacted_count =
            compute_compact_count(messages, source, &strategy, max_tokens);
        if compacted_count == 0 {
            if !total_compacted {
                return Err(CompactionError::NoBoundary);
            }
            break; // Already compacted, no more safe boundaries
        }

        let tokens_before = used_tokens;

        // Emit events: Started (round 1 only), Progress
        if round == 1 {
            let reason = if force { CompactionReason::Manual } else { CompactionReason::Threshold };
            events.push(AgentEvent::CompactionStarted {
                reason,
                tokens_before,
                message_count: messages.len(),
            });
        }
        events.push(AgentEvent::CompactionProgress {
            phase: CompactionPhase::Summarizing,
            percent: 70,
        });

        // Generate summary (with retry — see C-4).
        // Pass the full messages slice; generate_with_retry internally
        // computes the safe compacted_count and may shrink it on retry.
        // Returns the actual count used so we can split consistently.
        let (summary_text, actual_compacted_count) = generate_with_retry(
            model, config, messages, &strategy, max_tokens, cancel_token,
        ).await?;

        // Staleness check — see C-5
        let current_messages = context.messages();
        if is_stale(messages, current_messages) {
            events.push(AgentEvent::CompactionProgress {
                phase: CompactionPhase::Applying,
                percent: 100,
            });
            return Ok(total_compacted); // Don't apply stale summary
        }

        let kept = &messages[actual_compacted_count..];
        let tokens_after = estimate_message_tokens_summary(&summary_text)
            + estimate_messages_tokens(kept);

        let summary = CompactionSummary {
            summary: summary_text,
            tokens_before,
            tokens_after,
            first_kept_message_index: actual_compacted_count,
        };

        events.push(AgentEvent::CompactionProgress {
            phase: CompactionPhase::Applying,
            percent: 90,
        });
        events.push(AgentEvent::CompactionApplied { summary });
        context.apply_compaction(summary);

        total_compacted = true;

        // Stop if reduction is too small to be worth another round
        if tokens_before.saturating_sub(tokens_after) < MIN_REDUCTION_TOKENS {
            break;
        }
    }

    Ok(total_compacted)
}
```

**Key details:**
- `CompactionStarted` emitted only on round 1; `CompactionApplied` on every round.
- Round 2+ compacts the already-compacted context (previous summary + retained messages).
- The `force` flag (manual compaction) is only checked on round 1 — subsequent rounds always check the threshold.
- After the loop, a single `CompactionApplied` event with final state is sufficient for the TUI to update.

### C-2: Provider Overflow Recovery

**Current:** Provider context-overflow errors propagate from `run_model_turn` up through the turn loop.

**After:** The turn loop catches overflow errors, triggers compaction, and retries the model turn once.

```rust
// crates/neo-agent-core/src/runtime/turn_loop.rs

const OVERFLOW_STATUS_RECOVERY_RATIO: f64 = 0.5;

// In run_agent_turn, after run_model_turn_with_fallback:
match model_result {
    Err(ref err) if should_recover_from_overflow(err, context, config) => {
        // 1. Record observed overflow point
        let estimated = estimate_messages_tokens(context.messages());
        observe_context_overflow(config, estimated);

        // 2. Trigger blocking compaction
        let mut compact_events = Vec::new();
        let compacted = run_compaction(
            model, config, context, &mut compact_events,
            CompactionSource::Auto, cancel_token,
        ).await?;

        if compacted {
            events.extend(compact_events);
            // 3. Retry model turn (once, no recursion)
            run_model_turn_with_fallback(context, config, emitter, cancel_token).await
        } else {
            return Err(AgentRuntimeError::Model(err.into()));
        }
    }
    other => other,
}
```

**Recovery conditions** (ported from kimi-code's `shouldRecoverFromContextOverflow`):

```rust
fn should_recover_from_overflow(
    err: &AgentRuntimeError,
    context: &AgentContext,
    config: &AgentConfig,
) -> bool {
    let AgentRuntimeError::Model(ai_err) = err else {
        return false;
    };
    match ai_err {
        AiError::ContextOverflow { .. } => true,
        AiError::Server { status: 413, .. } => {
            // Only recover if the request was large enough that
            // overflow is plausible (not a provider-side limit)
            let estimated = estimate_messages_tokens(context.messages());
            let max = effective_max_context_tokens(config);
            max > 0 && (estimated as f64) > (max as f64) * OVERFLOW_STATUS_RECOVERY_RATIO
        }
        _ => false,
    }
}
```

A small request hitting 413 indicates a provider-side limit, not context overflow — compaction won't help.

### C-3: Observed-Max Adaptation

**Current:** `max_context_tokens` comes solely from `ModelSpec::capabilities.max_context_tokens` (configured value).

**After:** When the provider reports overflow, record the estimated token count as the observed maximum. Future compaction thresholds use `min(configured, observed)`.

```rust
// crates/neo-agent-core/src/runtime/config.rs

const OVERFLOW_SAFETY_RATIO: f64 = 0.85;

pub struct AgentConfig {
    // ... existing fields ...
    /// Runtime-observed context overflow point.
    /// Set when provider reports overflow; used to cap effective max.
    pub observed_max_context_tokens: std::sync::Mutex<Option<usize>>,
}

/// Effective max context tokens, considering observed overflow.
pub fn effective_max_context_tokens(config: &AgentConfig) -> usize {
    let configured = config.model.capabilities.max_context_tokens
        .unwrap_or(0) as usize;
    let observed = config.observed_max_context_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|v| ((v as f64) * OVERFLOW_SAFETY_RATIO) as usize);

    match (configured, observed) {
        (0, Some(o)) => o,
        (c, Some(o)) => c.min(o),
        (c, None) => c,
    }
}

/// Record an observed overflow point.
/// Only updates if the new value is smaller than the current observation
/// (conservative — never increases the effective max).
fn observe_context_overflow(config: &AgentConfig, estimated_tokens: usize) {
    let safe = ((estimated_tokens as f64) * OVERFLOW_SAFETY_RATIO) as usize;
    let mut guard = config.observed_max_context_tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match *guard {
        Some(current) if safe < current => *guard = Some(safe),
        None => *guard = Some(safe),
        _ => {}
    }
}
```

**Design decisions:**
- `Mutex<Option<usize>>` — the observed max is runtime state, not config. `Mutex` because `AgentConfig` is shared (`Arc`).
- Safety ratio 0.85 — the observed overflow point is the edge; backing off 15% gives headroom.
- Monotonically decreasing — `observe_context_overflow` only updates if the new value is smaller, never larger. This prevents a single anomalous small overflow from being overwritten.

### C-4: Empty-Summary Retry + Prefix Shrink

**Current:** `generate_compaction_summary()` returns `CompactionError::Empty` on an empty summary. One shot.

**After:** Retry up to `MAX_COMPACTION_RETRY_ATTEMPTS` times, shrinking the prefix each time:

```rust
const MAX_COMPACTION_RETRY_ATTEMPTS: u32 = 5;

async fn generate_with_retry(
    model: &Arc<dyn ModelClient>,
    config: &AgentConfig,
    messages: &[AgentMessage],
    strategy: &CompactionStrategy,
    max_tokens: usize,
    cancel_token: &CancellationToken,
) -> Result<(String, usize), CompactionError> {
    let mut compacted_count = compute_compact_count(
        messages, CompactionSource::Auto, strategy, max_tokens,
    );

    for attempt in 0..MAX_COMPACTION_RETRY_ATTEMPTS {
        if compacted_count == 0 {
            return Err(CompactionError::NoBoundary);
        }

        if cancel_token.is_cancelled() {
            return Err(CompactionError::Cancelled);
        }

        let prefix = &messages[..compacted_count];
        match generate_compaction_summary(
            model, config, prefix, None, cancel_token, |_| {},
        ).await {
            Ok(summary) if !summary.trim().is_empty() => return Ok((summary, compacted_count)),
            Ok(_) => {
                // Empty summary → shrink prefix
                compacted_count = reduce_compact_count(messages, compacted_count);
                continue;
            }
            Err(CompactionError::Llm(msg)) if is_retryable_compaction_error(&msg) => {
                compacted_count = reduce_compact_count(messages, compacted_count);
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(CompactionError::Truncated(MAX_COMPACTION_RETRY_ATTEMPTS))
}

/// Find a safe split point smaller than `current_count`.
fn reduce_compact_count(
    messages: &[AgentMessage],
    current_count: usize,
) -> usize {
    if current_count <= 1 {
        return 0;
    }
    for index in (0..current_count - 1).rev() {
        if can_split_after(messages, index) {
            return index + 1;
        }
    }
    0
}

fn is_retryable_compaction_error(msg: &str) -> bool {
    // Retry on rate-limit, server errors, network errors during compaction
    let lower = msg.to_lowercase();
    lower.contains("rate limit") || lower.contains("429")
        || lower.contains("timeout") || lower.contains("connection")
}
```

**Why shrink the prefix?** If the model returns empty, it's likely because the input was too large for the summarization call itself. Shrinking the prefix reduces input tokens and gives the model a better chance of producing a summary.

Note: with Spec A's structured errors, `is_retryable_compaction_error` can check `AiError::is_retryable()` on the error variant instead of string matching.

### C-5: Staleness Detection

**Current:** The summary is applied to `context` after the LLM call completes, regardless of whether history changed during the call.

**After:** Snapshot the history before the LLM call; abort if it changed by the time the summary is ready.

```rust
/// Check whether the current context messages differ from the snapshot
/// taken before compaction began.
fn is_stale(
    snapshot: &[AgentMessage],
    current: &[AgentMessage],
) -> bool {
    if current.len() < snapshot.len() {
        return true; // Messages were removed (undo/clear)
    }
    // Compare element-by-element up to snapshot length
    snapshot.iter()
        .zip(current.iter())
        .any(|(a, b)| a != b)
}
```

`AgentMessage` derives `PartialEq`, so direct comparison works.

**When staleness is detected:** The compaction returns `Ok(false)` (not an error) — the summary is simply not applied. The user's undo/clear intent is respected. No `CompactionApplied` event is emitted for the stale round.

### CompactionError Extensions

```rust
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("compaction LLM call failed: {0}")]
    Llm(String),
    #[error("compaction produced an empty summary")]
    Empty,
    #[error("compaction cancelled")]
    Cancelled,
    #[error("no safe compaction boundary found in the current history")]
    NoBoundary,
    #[error("compaction truncated: model returned empty/truncated after {0} attempts")]
    Truncated(u32),
    #[error("compaction stale: history changed during summarization")]
    Stale,
}
```

### Configuration Additions

```rust
// crates/neo-agent/src/config/mod.rs
pub struct RuntimeCompactionConfig {
    // ... existing fields ...
    pub max_rounds: usize,             // default: 5
    pub max_retry_attempts: u32,       // default: 5
}
```

```toml
[runtime.compaction]
# ... existing fields ...
max_rounds = 5
max_retry_attempts = 5
```

## Migration Impact

### `crates/neo-agent-core/src/`

| File | Change |
|---|---|
| `compaction/mod.rs` | `run_compaction()` → multi-round loop; add `generate_with_retry()`, `reduce_compact_count()`, `is_stale()`; new `CompactionError::Truncated`, `CompactionError::Stale` variants |
| `runtime/turn_loop.rs` | Model turn error handling: catch overflow → compaction → retry |
| `runtime/config.rs` | `AgentConfig` add `observed_max_context_tokens: Mutex<Option<usize>>`; add `effective_max_context_tokens()`, `observe_context_overflow()` |
| `runtime/compaction_trigger.rs` | `maybe_compact()` uses `effective_max_context_tokens()` instead of raw configured value |

### `crates/neo-agent/src/`

| File | Change |
|---|---|
| `config/mod.rs` | `RuntimeCompactionConfig` add `max_rounds`, `max_retry_attempts` |
| `config/types.rs` | `FileRuntimeCompactionConfig` add corresponding fields |

## Testing Strategy

Targeted at matching kimi-code's 53 test cases. Focus on the 5 new capabilities:

### Multi-round compaction (C-1)

- First round doesn't reduce below threshold → second round triggers
- Reduction < 1024 tokens → stops after first round
- `MAX_COMPACTION_ROUNDS` limit reached → stops
- `CompactionStarted` emitted only on round 1
- `CompactionApplied` emitted on every round
- Manual compaction: force checked on round 1 only, threshold checked on subsequent rounds

### Overflow recovery (C-2)

- `ContextOverflow` error → triggers compaction → retry succeeds
- `Server { status: 413 }` with large request (> 50% max) → triggers compaction
- `Server { status: 413 }` with small request (< 50% max) → does NOT trigger compaction
- Compaction after overflow fails → error propagates
- Non-overflow errors (429, 401) → do NOT trigger compaction recovery (handled by retry/fallback)

### Observed-max (C-3)

- `observe_context_overflow(200_000)` → `effective_max` = 170_000 (200k × 0.85)
- Second overflow at 150_000 → updates to 127_500 (smaller wins)
- Second overflow at 250_000 → does NOT update (larger ignored)
- `effective_max` = `min(configured, observed)` when both exist
- `effective_max` = configured when no observation exists

### Empty-summary retry (C-4)

- First attempt empty → shrinks prefix → second attempt succeeds
- All 5 attempts empty → `CompactionError::Truncated(5)`
- `reduce_compact_count()` finds smaller safe split point
- `reduce_compact_count()` returns 0 when no smaller split exists
- Cancellation during retry → `CompactionError::Cancelled`
- Retryable LLM error (rate limit during compaction) → shrinks prefix + retries

### Staleness (C-5)

- Summary succeeds, history unchanged → applies normally
- History shortened during summary (undo) → `is_stale()` returns true → does not apply
- History modified during summary (message content changed) → stale → does not apply
- History appended during summary (user sent follow-up) → NOT stale (append is additive, prefix is unchanged)
